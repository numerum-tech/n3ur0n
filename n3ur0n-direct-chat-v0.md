# N3UR0N — Mode chat direct (spec d'implémentation v0)

> **Statut** : spec validée 2026-06-04, non implémentée.
> **Périmètre** : API locale + UI embarquée uniquement. `protocol_version` inchangé (`n3ur0n/0.3`), aucune migration SQL, aucun nouveau verbe.
> **Décisions actées avec l'utilisateur (2026-06-04)** : (1) toggle libre au sein de la discussion active — le mode est porté par chaque message ; (2) le mode direct utilise le LLM du planner, avec override texte du nom de modèle ; (3) planner requis — sans planner, le 503 actuel reste.

## 1. Objet

Permettre, depuis l'UI de chat, un échange classique utilisateur ↔ modèle qui **ne compile pas de plan** : pas de transformation de la demande en séquence d'exécution de capacités. Un seul appel LLM par message (contre 2-3 en mode auto), latence réduite, comportement prévisible.

Le mode direct n'est **pas** un nouveau planner d'orchestration : c'est l'équivalent du chemin `reflect_only` existant de `PlanExecPlanner` (déjà emprunté quand le plan compilé est vide ou invalide), sans l'appel compile en amont.

## 2. Ce qui ne change pas

- Protocole fil, verbes, signatures : intacts. Tout se joue derrière `/api/v0`.
- Schéma SQLite : le mode est par message, rien n'est persisté côté conversation. Les `Turn` existants (`User`/`Assistant`) suffisent ; une conversation mixte auto/direct est cohérente par construction (la trace est simplement vide pour les tours directs).
- Verrouillage par conversation, sémaphore `planner_slots`, cache LRU : le dispatch direct passe par `NodeRuntime.handle_user_message{,_streaming}` comme aujourd'hui.
- Contrat SSE : aucun nouveau type d'événement. Séquence directe = `PlanReady{steps:[]}` → `Reflecting` → `Final` (ou `Error`), déjà gérée par l'UI.
- Node sans planner : `/api/v0/conversations/:id/messages` continue de répondre 503.

## 3. Design

### 3.1 `crates/node` — `DirectChatPlanner`

Nouveau fichier `crates/node/src/planner/direct.rs`.

Structure : `{ llm_backend: Arc<dyn Backend>, model_hint: Option<String> }` — les mêmes champs que la partie reflect de `PlanExecPlanner`, construits depuis le même `OpenAIConfig`.

Flux de `dispatch_inner` :

1. `state.push_user(message)` + `persist_last`.
2. Émettre `PlanReady{steps: []}` (streaming).
3. Construire les messages : system prompt dédié (persona assistant simple ; conserver les règles d'honnêteté du reflect concernant les actions à effets de bord — le modèle ne doit pas prétendre avoir exécuté quoi que ce soit), puis `state.to_chat_messages(MAX_CONTEXT_TURNS)`. **Attention** : contrairement à `reflect_only`, le message utilisateur est déjà dans `state` à ce stade — ne pas le re-suffixer (le reflect le re-déclare parce qu'il insère un blackboard entre-temps ; ici il n'y a pas de blackboard).
4. Émettre `Reflecting`.
5. `llm_backend.invoke("chat", {messages, temperature, model})` — modèle = override de la requête sinon `model_hint`.
6. `state.push_assistant(content, model_used)` + `persist_last`, émettre `Final`.
7. Retourner `DispatchOutcome { reply, model, trace: vec![] }`.

`MAX_CONTEXT_TURNS` est actuellement une constante privée de `plan_exec.rs` — la déplacer dans `planner/mod.rs` (ou la dupliquer avec note ; préférer le déplacement).

### 3.2 Passage du mode et de l'override — `DispatchOptions`

Le trait `Planner::dispatch{,_streaming}` ne véhicule ni mode ni modèle. Extension de signature avec un paramètre `opts: DispatchOptions` :

- `DispatchOptions { model_override: Option<String> }`, `#[derive(Debug, Clone, Default)]`.
- Une seule impl existante (`PlanExecPlanner`) à adapter — elle ignore `model_override` en v1 (l'override ne s'applique qu'au mode direct, décision actée).
- Alternative rejetée : construire un `DirectChatPlanner` par requête avec le modèle dedans — évite de toucher le trait mais disperse la construction dans la couche HTTP ; le trait est interne au workspace, le coût du changement est nul.

`NodeRuntime` détient `planner: Arc<dyn Planner>` (auto) **et** `direct: Arc<dyn Planner>`. `handle_user_message{,_streaming}` gagne `(mode: DispatchMode, opts: DispatchOptions)` et route vers l'un ou l'autre. `DispatchMode { Auto, Direct }` avec `Default = Auto`.

### 3.3 Conflit à résoudre : verrou modèle dans `OpenAIBackend`

`build_request` (`crates/adapters/src/openai.rs`) **verrouille `model` au `default_model`** quoi que le caller envoie — protection délibérée contre les noms de modèles hallucinés par le compile, et contre les callers réseau. Le doc-comment de `OpenAIConfig.default_model` (« Overridable in each invoke payload ») est mensonger — à corriger.

Résolution retenue : champ `allow_model_override: bool` sur `OpenAIConfig`, défaut `false`. `build_runtime` le met à `true` **uniquement** pour l'instance backend du planner/direct (backend privé, jamais exposé au réseau — le handler de verbes ne le voit pas). Le verrou reste intégral pour tout backend servant des caps réseau. Justification : la menace (« caller hallucine un modèle ») ne s'applique pas à un choix explicite de l'utilisateur local dans sa propre UI ; un nom invalide produit une erreur upstream propre, remontée en événement SSE `Error`.

### 3.4 `crates/server` — API

`ConversationMessageRequest` étendu :

```
{ message: String, mode?: "auto" | "direct", model?: String }
```

- `mode` absent → `auto` (rétrocompatible). Valeur inconnue → 400.
- `model` : trim, longueur max 128, ignoré silencieusement si `mode != direct` (v1). Pas de validation contre une liste — un nom invalide échoue à l'appel upstream et l'erreur est montrée à l'utilisateur (risque assumé v1 ; v2 possible : endpoint read-only adossé au probe `/v1/models` existant pour un vrai sélecteur).
- Mêmes champs sur `/messages` et `/messages/stream` (parsing partagé dans `prepare_dispatch`).
- `build_runtime` construit le `DirectChatPlanner` à partir du même `OpenAIConfig` que le planner — zéro nouvelle config CLI/env.
- Corriger le doc-comment CLI de `--planner-llm-base-url` (« defaults to the chat backend's URL » est faux : le flag est requis).

### 3.5 UI (`crates/server/ui/`)

- **Toggle** auto ⇄ direct dans le composer. Le mode est lu à l'envoi de chaque message ; on peut alterner librement dans la même discussion. État mémorisé par conversation en `localStorage` (clé `n3ur0n_chat_mode:<conv_id>` ; pas de persistance serveur).
- **Champ modèle** : input texte optionnel, visible uniquement en mode direct, placeholder = modèle par défaut du planner. Vide = défaut. Mémorisé en `localStorage` (clé globale, pas par conversation).
- **Stepper** : en mode direct, ne pas afficher « no plan — answering directly » (faux : on n'a pas essayé de planifier). Statut dédié, ex. « direct · composing reply… ». Le frontend connaît le mode qu'il a envoyé — pas besoin d'information serveur supplémentaire.
- **i18n** : nouvelles clés dans `locales/en.json` + `locales/fr.json` (toggle, tooltip, statut stepper, placeholder modèle). Rappel : ajout de clés = rebuild (catalogues embarqués rust-embed).
- **Historique** : les tours directs se rendent comme des bulles user/assistant ordinaires — aucun changement de rendu.

### 3.6 RBAC

Les routes conversations n'ont aucune garde de permission spécifique (incohérence préexistante : `/chat` exige `CHAT_USE`, `/conversations/:id/messages` rien au-delà de `require_authed`). Le mode direct n'aggrave rien mais l'expose. **Hors scope ici** — à traiter dans un chantier RBAC phase 2 ; noter seulement que si une perm est ajoutée plus tard, elle doit couvrir les deux modes uniformément.

## 4. Tests

- **Unit (`crates/node`)** : `DirectChatPlanner` avec backend mock — turns User+Assistant persistés dans l'ordre, trace vide, `model_override` propagé dans les args d'invoke, séquence d'événements SSE exacte, erreur backend → propagation propre sans Assistant fantôme.
- **Unit (`crates/adapters`)** : `allow_model_override=true` laisse passer un `model` caller ; `false` (défaut) le verrouille — non-régression du comportement réseau.
- **Intégration (`crates/server`)** : requête sans `mode` → comportement auto inchangé ; `mode:"direct"` → réponse + turns en base ; alternance auto/direct dans la même conversation → historique cohérent ; `mode` invalide → 400 ; node sans runtime → 503 inchangé.
- **Smoke** : ajout d'un POST `mode:"direct"` dans `docker/cluster-smoke.sh` sur node-a.

## 5. Hors scope (v1)

- Sélecteur de modèles énuméré (endpoint read-only sur les backends `openai_compat` / probe `/v1/models`).
- Chat direct sans planner configuré (runtime direct-only).
- Streaming token-par-token de la réponse (le protocole SSE actuel émet `Final` d'un bloc ; le streaming upstream est désactivé partout en v0.1).
- Persistance serveur du mode par conversation.
- Garde de permission dédiée sur les routes conversations (chantier RBAC).

## 6. Estimation

~300-400 lignes réparties : `direct.rs` ~120-150, retouches trait/runtime/bootstrap ~60, HTTP ~40, adapters ~20, UI+i18n ~80-100, tests ~100. Risque faible ; le seul point sensible est le toucher au verrou modèle (§3.3), circonscrit par le flag opt-in.

# Recommandations Planner — N3UR0N v0.1 → v0.2

**Date** : 2026-05-12
**Statut** : ~~input pour la prochaine itération~~ → **largement absorbé par les releases 0.2.0 et 0.3.0**. Conservé comme trace de raisonnement et pour les sous-chantiers non encore livrés.
**Portée** : qualité de planification du `LocalPlanner` quand le backend LLM est un modèle local de classe 3B–13B.
**Méthode** : analyse statique du code (`crates/node/src/planner/*`, `crates/core/src/capability.rs`, `crates/adapters/src/openai.rs`) confrontée à la doxa industrielle sur le tool-use avec petits modèles.
**Lecteur cible** : implémenteur de la v0.2, mainteneur de l'archi v0 et reviewer technique.

---

## ⚠ État d'implémentation (2026-05-12)

Confrontation rapide aux releases 0.2.0 + 0.3.0 :

| Reco | Statut | Livré dans |
|---|---|---|
| §4.1 enrichissement `CapabilityDecl` (`examples`, `disambiguation`, `negative_examples`, `output_semantic`) | ✅ Livré | 0.2.0 (`PROTOCOL_VERSION = "n3ur0n/0.2"`) |
| §4.2 retrieval BM25 sur le catalogue | ✅ Livré | 0.2.0 (`crates/node/src/planner/retrieval.rs`) |
| §4.3 constrained decoding (GBNF / JSON Schema) | ✅ Livré | 0.2.0 (quand backend supporte) |
| §4.4 escalade planner→planner | ✅ Livré | 0.2.0 (`PlanCompiler` cascade across known peers) |
| §4.5 trancher `LLMPlanner` vs `PlanExecPlanner` | ✅ Tranché | `PlanExecPlanner` est le planner v2 (cf. CHANGELOG 0.2.0) |
| Itération E "capability binding & wiring UX" (suggérée dans les échanges ultérieurs) | ✅ Livré | 0.3.0 (TOML manifest + master-detail Settings UI + composer) |

Sous-chantiers **non livrés** et toujours pertinents :

- §5.1 versioning métadonnée (`cap.version` ajouté en v0.2 mais sans politique de bump documentée).
- §5.5 obligation `examples` non-vide (en v0.2 le planner *skip* les caps sans exemples — soft enforcement, pas hard).
- §7 jeu d'évaluation canonique 50-prompts + métriques formelles — pas formalisé.
- §6 itération D Cascading detailed thresholds — implémentée mais pas mesurée.

Les sections du document qui *prescrivent* du travail futur restent lisibles mais doivent être interprétées comme historiques. Les sections de *diagnostic* (§3 gaps, §4 reco) gardent leur valeur explicative.

---

## 0. Préambule

Cette itération suppose acquis le scaffold v0.1 (workspace cargo, deux planners coexistants, exécuteur de plan déterministe). Elle ne re-discute pas les choix de stack ni les invariants protocolaires. Elle se concentre sur **la qualité de la décision de planification**, qui est devenue le goulot d'étranglement de l'expérience utilisateur dès qu'on dépasse 3–5 capacités exposées par le réseau.

Le document est opiniâtre. Les recommandations sont ordonnées par ROI décroissant et chacune porte un effort estimé. Les questions ouvertes sont isolées dans la §5 — elles ne doivent pas être tranchées silencieusement pendant l'implémentation.

---

## 1. Synthèse (TL;DR)

L'architecture actuelle est plus saine que la moyenne des systèmes "agentiques" maison. Le `PlanExecPlanner` (plan-then-execute, deux appels LLM, exécuteur déterministe) est la bonne forme structurelle pour des modèles de 7–13B. Les heuristiques défensives (`looks_like_malformed_tool_call`, `first_unresolved_template`, fallback `reflect_only` sur parse error) attestent une discipline de récupération d'erreur cohérente avec une vraie expérience terrain.

Cependant, **trois leviers de qualité ont été ratés à v0.1** et bloquent le scaling :

1. **`CapabilityDecl` est sous-spécifié.** Sans `examples`, sans `disambiguation`, sans contre-exemples, le planner doit deviner la sémantique des caps à partir d'une `description` free-form. La compensation actuelle — règles génériques inscrites en dur dans le `compile_system_prompt` — déplace l'effort éditorial au mauvais endroit du système.
2. **Le catalogue est dumpé intégralement** dans le contexte de chaque appel LLM (jusqu'à 500 peers × N caps). Insoutenable au-delà de quelques dizaines de capacités. Aucun retrieval, aucun ranking.
3. **Le constrained decoding est partiel.** `format: "json"` garantit la validité JSON mais pas la conformité au schéma `Plan`. La validation post-hoc rattrape, au prix d'un fallback dégradé.

Deux leviers secondaires sont déjà *préparés* par le code mais pas *câblés* : l'escalade planner→planner (`EXCLUDED_CAP_NAMES = ["plan"]`) et la déprécation explicite du `LLMPlanner` ReAct au profit du `PlanExecPlanner`.

Le travail recommandé pour la v0.2 tient en cinq chantiers, classés par ROI plus bas.

---

## 2. Acquis (ne pas défaire)

Avant les recommandations, fixer ce qui est correct dans le code actuel afin de ne pas le rétrograder par inadvertance.

**Structure plan-then-execute.** Un appel LLM pour compiler un `Plan` typé, exécution déterministe en topologie, deuxième appel LLM pour synthétiser la réponse. Cette forme est optimale pour les modèles 7–13B : elle borne les opportunités de dérive et autorise une parallélisation des steps indépendants (gérée par `MAX_CONCURRENT_STEPS=4`).

**Référence à valeurs typée (`${stepid.path}`).** La syntaxe `${...}` purement déclarative — pas d'arithmétique, pas de conditionnels — est la bonne discipline. Elle évite la classe la plus douloureuse d'erreurs des petits modèles (essayer d'effectuer un calcul dans une chaîne de template). Le fail-fast `first_unresolved_template` est un complément essentiel.

**Validation structurelle (`validate_plan`).** Unicité des `id`, appartenance au catalogue, références `depends_on` existantes, détection de cycle via Kahn. Ces invariants doivent être *renforcés* (cf. §4.3), pas affaiblis.

**Exclusion de `plan` du catalogue exposé au planner.** Le commentaire "`keeps us from recursing plan→plan when v0.2 ships PlanBackend`" trahit une vision juste : la planification distante doit être une capacité du réseau, pas un cas particulier hard-codé. **À conserver.**

**Heuristiques de détection de bavure (`looks_like_malformed_tool_call`).** Empiriquement utiles tant que le `LLMPlanner` ReAct existe. À déprécier *avec* le planner qui les nourrit (cf. §4.5), pas avant.

**Honesty rules dans le `reflect_system_prompt`.** L'instruction "if no tool ran successfully, say plainly that you cannot" adresse directement la confabulation d'exécution typique des petits modèles. À conserver et à renforcer (cf. §4.3 sur les contraintes structurelles).

---

## 3. Diagnostic des gaps (ordonné par ROI)

### 3.1 Métadonnées de capacité indigentes — ROI maximal

Le contrat actuel de `CapabilityDecl` :

```
name           — string unique
description    — string free-form
schema_in/out  — JSON Schema
mode           — Free | Restricted
pricing        — Option<string>
tags           — Vec<string>
lobe_ids       — Vec<string>
```

Ce qui manque pour qu'un petit LLM puisse choisir et appeler correctement :

- **`examples: Vec<{args, expected_shape, intent}>`** — quelques exemples canoniques d'usage. Un 7B fine-tuné par few-shot bat un 70B sans exemples sur le matching capacité↔intention.
- **`disambiguation: Option<string>`** — texte explicite "préférer cette cap à X quand…, ne pas confondre avec Y qui fait…". C'est *l'ennemi* du planner d'avoir trois caps aux descriptions vagues qui pourraient toutes répondre.
- **`negative_examples: Vec<{user_intent, why_not}>`** — contre-exemples explicites "n'utilise pas pour…". Plus efficace que de l'interdire dans le prompt central, parce que la portée est par-cap.
- **`output_shape_hint: Option<string>`** — texte court décrivant ce que le résultat *signifie* (pas seulement sa structure JSON). Aide le reflect step à composer une réponse correcte sans halluciner.

Conséquence actuelle : `compile_system_prompt` dans `plan_exec.rs` doit inscrire en dur des règles génériques comme "`reverse` is for reversing strings, NOT for translating. `echo` just passes args through". Ce texte est :

1. **Maintenu dans le mauvais crate.** Il devrait vivre avec la cap chez son publisher.
2. **Non versionné par-cap.** Une mise à jour de la sémantique d'une cap nécessite un patch du `node`.
3. **Inopérant au-delà des quelques exemples cités.** Il ne se généralise pas à des caps qui n'existaient pas quand le prompt a été écrit.

**Effort estimé** : 1–2 jours. Migration `describe_self`, mise à jour de `Catalog::to_openai_tools`, mise à jour du `compile_system_prompt` pour injecter les exemples par cap.

**Dépendances** : version du protocole (`protocol_version: "n3ur0n/0.1"` → `n3ur0n/0.1.1` rétrocompatible si les nouveaux champs sont tous `#[serde(default)]`).

### 3.2 Catalogue non filtré — risque de scaling

Aujourd'hui `Catalog::build` charge jusqu'à 500 peers et `to_openai_tools` les dump intégralement dans le system prompt. Avec un description moyen de ~50 tokens + un `schema_in` sérialisé ~30 tokens par cap, on sature un contexte de 8k tokens dès ~80 caps. Au-delà :

- Le modèle "oublie" les caps citées en haut du catalogue (effet *lost in the middle*).
- Le coût d'inférence devient dominé par le prompt, pas la réponse.
- La latence du `compile` augmente linéairement avec la taille du réseau.

Le code n'a **aucun mécanisme de filtrage** : ni par tags, ni par embedding, ni par historique d'usage, ni par lobe explicite.

**Solutions par ordre de complexité croissante** :

1. **Filtrage statique par tags.** Le user dit explicitement les domaines pertinents lors de la création de conversation. Trivial mais pousse l'effort sur l'utilisateur.
2. **Top-K par BM25 sur la requête.** Index lexical sur `name + description + tags`. ~200 lignes Rust, zéro dépendance lourde. Couvre 80% des cas.
3. **Top-K par embedding sémantique.** Précalcul des embeddings des caps à la cache `describe_self`. Nécessite un modèle d'embedding local (sentence-transformers via candle, ou délégation à une cap réseau `embed`). Plus puissant, plus coûteux.
4. **Hybrid retrieval + reranker.** BM25 → top-50 → reranker LLM → top-10. Industriel, sans doute *over-engineered* pour v0.2.

**Recommandation** : viser (2) en v0.2 avec une trappe d'évolution vers (3). BM25 sur le catalogue suffit jusqu'à plusieurs milliers de caps si les descriptions sont bonnes — ce qui devient vrai après §3.1.

**Effort estimé** : 2–3 jours pour BM25 + intégration dans `Catalog::build_for_query(user_msg)`. Tests sur jeu synthétique de 100+ caps.

### 3.3 Constrained decoding partiel — ROI moyen, gain de robustesse

Le `PlanExecPlanner` force `format: "json"` (mode Ollama/llama.cpp) + `temperature: 0.0`. C'est mieux que rien mais :

- `format: "json"` garantit la validité syntaxique JSON, pas la conformité au schéma `Plan`. Un modèle peut sortir `{"steps": [...]}` au lieu de `{"plan": [...]}` — valide JSON, invalide Plan.
- Les modèles 3–7B hallucinent souvent des champs supplémentaires (`"description": "...", "reasoning": "..."`) qui passent le parser mais polluent.
- La validation a posteriori (`validate_plan`) déclenche un fallback `reflect_only` qui *masque l'erreur sans la corriger* — le user obtient une réponse dégradée sans savoir pourquoi.

**Solution** : grammaire constrainte via llama.cpp GBNF ou xgrammar pour les backends qui le supportent. Caractéristiques :

- La grammaire **interdit littéralement les tokens** qui sortiraient du schéma. Pas de fallback nécessaire.
- Compilation à partir du schéma `Plan` lui-même (auto-générable depuis `serde_json::Schema` ou `schemars`).
- Sur les backends qui ne supportent pas GBNF (OpenAI API hors mode `response_format: json_schema`), conserver le fallback actuel.

**Effort estimé** : 1 jour pour générer la grammaire + ajouter un champ `grammar` dans le payload d'invoke du `chat` backend. Le backend `OpenAIBackend` doit le propager sous le bon nom (`grammar` pour llama.cpp, `response_format` avec JSON Schema pour OpenAI).

**Limite** : Ollama ne supporte pas GBNF directement à ce jour (cf. issue ollama/ollama#3616 — vérifier l'état). Si Ollama reste le backend de référence, le gain est partiel. Voir §5.4 sur le choix de backend cible.

### 3.4 Absence d'escalade planner→planner — ROI lié à l'effet réseau

L'architecture *prévoit* la capacité `plan` distante (cf. exclusion explicite dans `EXCLUDED_CAP_NAMES`). Le code ne *l'utilise pas*.

Aujourd'hui, en cas de plan invalide / parse error, `PlanExecPlanner` fallback sur `reflect_only` — c'est-à-dire qu'il abandonne la planification et demande au même LLM local de répondre directement. C'est sous-optimal :

- Si le LLM local n'a pas su planifier, il n'a probablement pas non plus la connaissance pour répondre.
- Le réseau N3UR0N a *par construction* des peers qui exposent des modèles plus gros.

**Proposition** : introduire un trait `PlanCompiler` distinct du `Planner` :

```
trait PlanCompiler {
    async fn compile(&self, user_msg: &str, catalog: &Catalog) -> Result<Plan>;
    fn confidence(&self, plan: &Plan) -> f32;  // 0..1
}
```

Implémentations :
- `LocalLLMCompiler` (équivalent du compile step actuel).
- `RemotePlanCompiler` qui invoque la capacité `plan` chez un peer.
- `CascadingCompiler { local, remote, threshold }` qui essaie le local, mesure la confiance, escalade si trop basse.

Mesures de confiance candidates (par ordre de coût) :
- Nombre de steps (plan vide ou plan > 8 steps suspect).
- Présence de capacités hallucinées rejetées par validation.
- Score de log-probabilité moyen sur les tokens du plan (si le backend le retourne).
- Validation par un second appel LLM critique (coûteux).

**Effort estimé** : 2 jours pour le trait + 1 jour pour exposer `plan` comme capacité du backend lui-même (un publisher peut exposer son local LLM comme planner pour le réseau).

**Dépendance** : §3.1 (sans bonnes métadonnées de cap, le planner distant n'est pas meilleur que le local).

### 3.5 Coexistence non tranchée `LLMPlanner` / `PlanExecPlanner`

Le code embarque deux planners distincts dont la relation n'est pas documentée :

- `LLMPlanner` : ReAct, `MAX_TOOL_TURNS=6`, heuristiques anti-bavure.
- `PlanExecPlanner` : compile + execute + reflect.

Trois positions possibles :

1. **Garder les deux, par profil de modèle.** ReAct pour modèles ≥14B avec tool-use fine-tuné ; plan-then-execute pour modèles plus petits. Justifiable mais nécessite une logique de sélection automatique ou un flag explicite par config.
2. **Déprécier `LLMPlanner`.** Le code de `looks_like_malformed_tool_call` est un aveu : c'est un planner fragile sur petits modèles. Si les petits modèles sont la cible, le supprimer simplifie.
3. **Fusionner.** Une seule implémentation qui *peut* émettre des steps un-par-un (ReAct dégénéré) ou un plan complet, selon une heuristique.

Aucune de ces options n'est intrinsèquement supérieure — mais l'état actuel (les deux coexistent sans contrat) est le pire choix. Il complexifie les tests, dilue le focus, et masque l'évolution du contrat de planning.

**Recommandation** : trancher pour (1) ou (2) avant tout autre travail planner. Si (1), documenter le critère de sélection dans `CLAUDE.md`. Si (2), supprimer `crates/node/src/planner/llm.rs` et les heuristiques associées.

**Effort estimé** : décision = 1h. Implémentation = 0,5j (suppression) ou 1j (sélection auto).

---

## 4. Recommandations détaillées

### 4.1 Enrichir `CapabilityDecl` (chantier prioritaire)

**Action** : Ajouter les champs suivants à `crates/core/src/capability.rs`, tous `#[serde(default, skip_serializing_if = "...")]` pour rétrocompatibilité :

| Champ | Type | Rôle |
|---|---|---|
| `examples` | `Vec<CapabilityExample>` | 2–5 exemples canoniques par cap |
| `disambiguation` | `Option<String>` | "préférer à X quand…, ne pas confondre avec Y…" |
| `negative_examples` | `Vec<NegativeExample>` | intentions qui *semblent* matcher mais ne doivent pas appeler cette cap |
| `output_semantic` | `Option<String>` | description courte de ce que signifie la sortie (pas son schéma) |

Structure suggérée :

```
CapabilityExample {
    user_intent: String,    // "traduire 'hello' en français"
    args: Value,            // {"text": "hello", "target": "fr"}
    expected_output: Value, // {"translation": "bonjour"}
}

NegativeExample {
    user_intent: String,    // "inverser les mots de cette phrase"
    why_not: String,        // "cette cap inverse caractères, pas mots — préférer wordreverse"
}
```

**Côté planner** : `compile_system_prompt` ne déclare plus les règles génériques en dur. À la place, pour chaque cap dans `Catalog`, il inclut :
- description
- schema_in compact (sans `$id`, sans `examples` JSON Schema redondants)
- 2 exemples (intent → args)
- disambiguation si présente
- 1–2 negative_examples si présents

Le prompt central reste générique ("output JSON, no markdown, ${...} for refs, no arithmetic"). Les règles *sémantiques* migrent vers la métadonnée.

**Impact attendu** : qualité de planification améliorée pour toutes les caps présentes et futures, sans modification du `node`. Effet d'autant plus marqué que le nombre de publishers grandit (l'effort éditorial se distribue).

**Risque** : un publisher mal-disant pollue son entrée. Pas critique en v0.2 — la communauté s'auto-régule via le directory et l'UX cartographique (post-v0.1).

### 4.2 Introduire un retrieval léger sur le catalogue

**Action** : Modifier `Catalog::build` en `Catalog::build_for_query(&self_id, &registry, &db, &user_query, top_k)` :

1. Construire la liste exhaustive comme aujourd'hui (avec exclusion de `plan`).
2. Indexer en BM25 sur le champ concaténé `name + description + tags + disambiguation`.
3. Ranker contre `user_query` et retourner les top-K (par défaut K=20).
4. **Toujours** inclure les capacités locales (du registre `self`) — elles sont gratuites à invoquer, le user les a explicitement configurées.

**Variante plus sûre** : garder le `build` actuel comme `build_full`, exposer `build_for_query` à côté, brancher le `PlanExecPlanner` sur `build_for_query`. Permet de faire tourner les tests existants sans modification.

**Effort** : 2–3j incluant index BM25 (cf. crate `tantivy` ou impl maison ~150 lignes).

**Pré-requis** : §4.1 (les descriptions s'enrichissent → BM25 devient discriminant).

### 4.3 Renforcer la validation et le constrained decoding

**Action côté validation (déjà partiellement faite)** :

- `validate_plan` doit aussi vérifier que `args` est conforme au `schema_in` de la cap. Aujourd'hui le code laisse passer un step dont les args ne valideront pas le schéma JSON déclaré par la cap — l'erreur n'apparaît qu'à l'exécution, après envoi réseau.
- Si un step a un `depends_on` *déclaré* qui n'est pas justifié par une référence `${...}` ni par une vraie dépendance d'effet, logger un warning. C'est souvent signe d'une planification confuse.
- Borner explicitement le nombre de steps (config `MAX_PLAN_STEPS`, défaut 8). Au-delà, refuser et retourner une erreur diagnostique. Un petit modèle qui sort 15 steps est presque toujours en train de halluciner une décomposition fictive.

**Action côté constrained decoding** :

- Étendre `Backend` pour accepter un champ optionnel `grammar` ou `response_schema` dans l'invoke `chat`.
- `OpenAIBackend` propage : (a) `response_format: {type: "json_schema", json_schema: {...}}` pour OpenAI ≥ 2024-08, (b) `grammar: "..."` pour llama.cpp, (c) ignore silencieusement pour les backends qui ne supportent pas.
- `PlanExecPlanner` génère la grammaire à partir du `Plan` schema au build-time (constante au démarrage).

**Effort** : 1–2j. Le plus gros est la génération de grammaire GBNF depuis un schéma serde, faisable à la main pour `Plan` (~50 lignes) plutôt qu'avec un générateur générique.

### 4.4 Câbler l'escalade planner→planner

**Action** :

1. Introduire `trait PlanCompiler` (cf. §3.4) dans `crates/node/src/planner/`.
2. Refactor `PlanExecPlanner` pour qu'il prenne un `Arc<dyn PlanCompiler>` au lieu d'un `Arc<dyn Backend>` direct. Le `Backend` reste injecté dans le `LocalLLMCompiler`.
3. Exposer la capacité `plan` côté `crates/adapters/` : un nouveau `PlannerAsCapability` qui wrap un `PlanCompiler` et l'expose comme une cap `plan` invocable par le réseau. Schéma d'entrée : `{user_intent: string, available_caps: [...]}`. Schéma de sortie : `{plan: [...]}`.
4. Implémenter `RemotePlanCompiler` qui invoque la cap `plan` chez un peer choisi.
5. Implémenter `CascadingCompiler` avec mesure de confiance (cf. §3.4 sur les heuristiques).

**Configuration runtime** : variables d'env `N3UR0N_PLANNER_REMOTE_FALLBACK=n3:abc...` pour désigner le peer planner, `N3UR0N_PLANNER_CONFIDENCE_THRESHOLD=0.5`.

**Effort** : 3–4j incluant tests d'intégration (cluster Docker à 3 nodes dont un publisher de cap `plan`).

**Pré-requis** : §4.1, §4.2. Sans bonnes métadonnées et sans retrieval, l'escalade ne donne pas de gain — le planner distant souffre des mêmes maux.

### 4.5 Trancher la coexistence des planners

**Décision recommandée** : déprécier `LLMPlanner` (option 2 de §3.5). Justifications :

- Le code mort à supprimer est minime ; le code à *maintenir* (heuristiques anti-bavure, parsing de tool_calls hétérogènes selon le format upstream) est non-trivial.
- Les modèles 14B+ qui font bien le ReAct font *aussi* bien le plan-then-execute. L'inverse n'est pas vrai.
- Le `PlanExecPlanner` a la propriété structurelle de parallélisation des steps que la boucle ReAct n'a pas.
- Garder deux planners brouille les tests et les benchmarks futurs (cf. §6).

**Action** :

1. Supprimer `crates/node/src/planner/llm.rs` et `crates/node/src/planner/tool_call.rs`.
2. Mettre à jour `Planner` trait pour ne plus exposer `LLMPlanner` dans les re-exports.
3. Updater `CLAUDE.md` pour refléter le choix et inscrire la justification.
4. Conserver `LLMPlanner` dans une branche git séparée si on veut le ressortir un jour pour les gros modèles.

**Si la décision est de garder les deux** (option 1) : ajouter une variable `N3UR0N_PLANNER_MODE=plan_exec|llm_react` + un switch automatique basé sur le `default_model` du backend (>14B = ReAct, sinon plan-then-execute). Documenter le seuil et la logique de fallback.

**Effort** : 0,5j (suppression) ou 1,5j (sélection automatique).

---

## 5. Questions à trancher avant implémentation

Ces points ne peuvent pas être tranchés silencieusement par l'implémenteur. Ils ont un impact protocolaire ou un coût d'évolution important.

### 5.1 Métadonnée signée vs déclarative

Quand un publisher modifie sa `disambiguation` ou ses `examples`, le `describe_self` change. Faut-il :

- (a) Versionner les `CapabilityDecl` (hash de contenu, exposé comme `cap_version`) ?
- (b) Laisser les consumers re-fetcher en best-effort ?

Position par défaut : (b) en v0.2, (a) à inscrire en dette pour quand la cap aura un effet économique (cf. archi §11 sur le modèle économique du registre).

### 5.2 Granularité du retrieval

Faut-il que le retrieval (§4.2) soit :

- Par requête utilisateur (ce qui implique de recalculer le ranking à chaque dispatch — coût acceptable avec BM25) ?
- Par conversation (cacher le top-K une fois, le réutiliser sur les messages suivants) ?

Position par défaut : par requête. La conversation peut dériver, le cache vieillit mal. Optimisation possible si profilage montre un goulot.

### 5.3 Politique d'escalade

Si le planner local a une confiance basse mais qu'aucun peer distant ne répond, faut-il :

- Tenter le plan local malgré tout (politique "best effort") ?
- Refuser et demander à l'utilisateur de reformuler (politique "fail loud") ?

Cette question touche à la philosophie produit. Position par défaut : "best effort" avec annotation explicite dans le trace UI ("plan compilé avec confiance basse, résultat à vérifier"). Ne pas masquer l'incertitude.

### 5.4 Backend de référence pour constrained decoding

Le constrained decoding (§4.3) n'est pas uniformément supporté :

- llama.cpp : GBNF natif, mature.
- vLLM : outlines, xgrammar — solide.
- Ollama : support partiel, en évolution. État à vérifier au moment de l'implémentation.
- OpenAI : `response_format: json_schema` depuis 2024-08, fonctionne bien.

Faut-il faire de llama.cpp le backend de référence officiel pour les déploiements consumer (au détriment de la simplicité d'installation d'Ollama) ?

Position par défaut : Ollama reste cible UX, llama.cpp documenté comme cible "qualité maximale". Le code propage la grammaire si supportée, sinon dégrade silencieusement.

### 5.5 Effort d'amorçage des `examples`

Imposer `examples` non-vide pour publier une cap publique ? Conséquences :

- Pour : qualité globale du réseau, alignement avec l'argument du document.
- Contre : friction à la publication, risque de "fake examples" si trop strict.

Position par défaut : champs optionnels en v0.2, monitoring du taux de remplissage. Devenir obligatoire en v0.3 si la qualité observée le justifie.

---

## 6. Séquencement proposé

Ordre concret pour l'itération, avec dépendances explicites. Chaque chantier est conçu pour être livrable et mesurable indépendamment.

**Itération A — Métadonnées et déprécation** (~1 semaine)

1. Trancher §5.5 et §3.5 (déprécation `LLMPlanner`).
2. Implémenter §4.1 (enrichissement `CapabilityDecl`) — protocole + serde + migration de `describe_self`.
3. Updater `compile_system_prompt` pour injecter les nouveaux champs.
4. Mettre à jour `EchoBackend` et `OpenAIBackend` pour exposer des `examples` réalistes sur leur cap.
5. Tests : cluster Docker à 3 nodes, vérifier amélioration du planning sur un jeu de prompts canonique.

**Itération B — Retrieval + validation** (~1 semaine)

1. Implémenter BM25 sur le catalogue (§4.2).
2. Renforcer `validate_plan` (validation args contre `schema_in`, borne `MAX_PLAN_STEPS`).
3. Ajouter la propagation de grammaire dans `Backend::invoke` (§4.3) — sans encore brancher la grammaire complète.
4. Benchmark : comparer qualité du compile avant/après sur un catalogue synthétique à 100+ caps.

**Itération C — Constrained decoding complet** (~3 jours)

1. Générer la grammaire GBNF pour `Plan` (à la main, ~50 lignes).
2. Brancher dans `OpenAIBackend` pour llama.cpp.
3. Vérifier le fallback gracieux sur Ollama et OpenAI.

**Itération D — Escalade planner→planner** (~1 semaine)

1. Introduire `trait PlanCompiler` (§4.4).
2. Implémenter `LocalLLMCompiler` (équivalent fonctionnel de l'actuel), `RemotePlanCompiler`, `CascadingCompiler`.
3. Exposer la cap `plan` côté adaptateur.
4. Tests cluster Docker avec node-A (consumer, petit modèle) → node-C (publisher cap `plan`, gros modèle).

**Décisions hors-séquence** : §5.1 (versioning métadonnées) et §5.4 (backend de référence) peuvent être prises à n'importe quel moment ; les inscrire en dette explicite si reportées.

---

## 7. Métriques de succès

Sans métrique objective, l'itération risque de rester un exercice d'esthétique. Proposer :

**Jeu d'évaluation canonique** : 50 prompts utilisateur couvrant (a) requêtes mono-cap triviales, (b) chaînes 2-steps, (c) chaînes 3+ steps avec ref, (d) requêtes hors-domaine où le plan doit être vide, (e) requêtes ambiguës où la disambiguation devrait trancher. Maintenir ce set en `tests/planner_eval/`.

**Métriques par planner config** (Local 7B no-retrieval ; Local 7B + BM25 ; Local 7B + BM25 + grammar ; Cascading 7B→70B) :

1. **Taux de plan valide** : `validate_plan` passe du premier coup.
2. **Taux de plan correct** : le plan résout effectivement l'intention (jugé par eval LLM externe ou humain).
3. **Taux de cap correctement choisie** : pour les requêtes mono-cap, la bonne cap est sélectionnée.
4. **Latence p50, p95** du compile step.
5. **Tokens consommés** par dispatch (prompt + completion).

**Seuil de release v0.2** : amélioration de ≥20% sur (2) pour Local 7B + BM25 + métadonnées enrichies, par rapport à v0.1 baseline. À ajuster après mesure baseline.

---

## 8. Annexe — Références code

Chemins et symboles touchés par chaque recommandation, pour faciliter la planification du diff.

| Reco | Fichier(s) | Symboles |
|---|---|---|
| §4.1 | `crates/core/src/capability.rs` | `CapabilityDecl` + nouveaux types |
| §4.1 | `crates/node/src/planner/plan_exec.rs` | `compile_system_prompt` |
| §4.1 | `crates/node/src/planner/catalog.rs` | `to_openai_tools` |
| §4.2 | `crates/node/src/planner/catalog.rs` | `Catalog::build` → `build_for_query` |
| §4.2 | `crates/node/src/planner/plan_exec.rs` | `dispatch_inner` (passage de la query au catalog) |
| §4.3 | `crates/node/src/planner/plan.rs` | `validate_plan` |
| §4.3 | `crates/adapters/src/openai.rs` | propagation `grammar` / `response_format` |
| §4.3 | `crates/adapters/src/lib.rs` | trait `Backend` (extension du payload `invoke chat`) |
| §4.4 | `crates/node/src/planner/mod.rs` | trait `PlanCompiler` (nouveau) |
| §4.4 | `crates/node/src/planner/plan_exec.rs` | refactor `PlanExecPlanner` pour utiliser `PlanCompiler` |
| §4.5 | `crates/node/src/planner/llm.rs` | suppression |
| §4.5 | `crates/node/src/planner/tool_call.rs` | suppression |
| §4.5 | `CLAUDE.md` | section "Capacity planner v0.1" → "v0.2" |

---

## 9. Hors-périmètre explicite

Pour borner cette itération et éviter le glissement, les chantiers suivants sont **explicitement reportés** :

- Streaming de la réponse finale (token-by-token) — orthogonal au problème de planning.
- UI cartographique sémantique du réseau (post-v0.1 stack §16).
- Synapses 1:N et fédération de lobes (archi §11, décision bloquante hors planner).
- Modèle économique des caps (archi §11, idem).
- Migration vers un planner fine-tuné dédié (Berkeley Gorilla, NexusRaven) — peut bénéficier des chantiers §4.1 mais nécessite une infra de training hors-scope v0.2.

Ces points doivent rester inscrits dans `n3ur0n-architecture-v0.md` §11 et ne pas être réintroduits silencieusement.

---

*Fin du document. Lecture estimée : 25 minutes. Itération recommandée : 3–4 semaines en série, ~2 semaines en parallélisant A+C et B+D avec deux contributeurs.*

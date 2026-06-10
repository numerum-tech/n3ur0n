# N3UR0N — Planner : améliorations v1 (spec d'implémentation pour agent de code)

**Date** : 2026-06-10
**Statut** : recommandations validées sur lecture du code (worktree du 2026-06-10, post-blob). À implémenter par ordre de priorité ; chaque item est indépendamment livrable sauf dépendances notées.
**Contexte** : issu de l'analyse critique du positionnement écosystème (gateways/orchestrateurs/durabilité/protocoles, juin 2026). Synthèse : la valeur différenciante du planner est la coordination *inter-domaines* de capacités ; les manques bloquants sont la durabilité et la maîtrise du contexte ; le levier d'adoption est la compatibilité avec les interfaces qui ont gagné (API OpenAI, Agent Skills).

**Lecture préalable obligatoire** : `CLAUDE.md` (invariants + discipline de dépendances), `crates/node/src/planner/{mod.rs, plan.rs, plan_exec.rs, compiler.rs, retrieval.rs, catalog.rs}`, `crates/node/src/conversation.rs`. L'exécuteur de plan vit dans `planner/plan.rs` (`execute_plan_streaming`), **pas** dans `plan_exec.rs`.

**Invariants à ne pas casser** :
- Aucune dépendance HTTP/SQL dans `core` ; `node` ne dépend jamais d'`axum`/`clap`/`tauri` (tableau CLAUDE.md).
- Pas de service externe (Temporal, Redis…) : SQLite est la seule persistance autorisée.
- Deux appels LLM par dispatch (compile + reflect) reste la référence ; toute amélioration qui en ajoute doit le justifier.
- Le protocole fil (verbes, envelopes signées) ne change pas pour ces items.
- Trace UI : ordre de déclaration du plan préservé (déterminisme existant à conserver).

---

## P1 — Persistance des tool turns au fil de l'exécution (durabilité, phase 1)

**Problème.** `PlanExecPlanner::dispatch_inner` (plan_exec.rs, étape 5) persiste les paires tool_call/tool_result **après** le retour complet de `execute_plan_streaming`. Un crash ou un kill du process en cours d'exécution perd toute la trace : le turn user est en base, aucun tool turn, pas d'assistant turn. Aucune table de plan dans `storage`.

**Approche.**
1. Persister chaque paire tool_call/tool_result au moment où `StepDone` est émis (l'exécuteur a déjà le hook événementiel), tout en conservant l'ordre déterministe : persister avec un `seq` pré-attribué en ordre de plan, pas en ordre de complétion.
2. Nouvelle table `plan_runs` (migration SQL versionnée) : `id, conversation_id, plan_json, status (running|done|failed), created_at, finished_at`. Insérer à la compile, clore au reflect.
3. Phase 1 = journal write-only (observabilité + intégrité après crash). La *reprise* d'un plan interrompu est une phase 2 explicitement hors scope ici — ne pas la commencer sans décision.

**Critères d'acceptation.**
- Test : kill simulé (drop du future) après 2 steps sur un plan de 4 → les 2 paires tool sont en base, `plan_runs.status = 'running'` (orphelin détectable).
- Test existant de déterminisme de trace inchangé.
- Aucun changement d'API publique HTTP.

**Effort estimé** : S–M. **Dépendances** : aucune.

---

## P2 — Passage par référence blob entre steps (maîtrise du contexte)

**Problème.** `PlanRun::blackboard_summary()` (plan_exec.rs) sérialise chaque résultat de step **en entier** dans le prompt reflect, sans limite de taille. Un step renvoyant un gros JSON (ou, désormais, une cap produisant un document) sature le contexte d'un modèle 7B. Avec la couche blob livrée, le mécanisme de référence existe déjà côté protocole.

**Approche.**
1. Troncature défensive immédiate : plafond par entrée du blackboard summary (constante, ~1 Ko) avec suffixe `…[truncated, N bytes]`.
2. Si un résultat de step contient une `BlobRef` (core/blob.rs), le summary rend la référence (hash, taille, mime) — jamais le contenu.
3. Extension de la résolution `${stepid.path}` (plan.rs, `resolve_value`) : quand le chemin résout vers une `BlobRef` et que la cap aval déclare accepter un blob en entrée, transmettre la référence telle quelle ; le transfert effectif passe par `blob_client`/`blob_resolve`, pas par le planner. Le payload binaire ne transite ni par le blackboard ni par un prompt.

**Critères d'acceptation.**
- Test : step produisant 100 Ko de JSON → prompt reflect borné, dispatch réussi.
- Test : plan à 2 steps où s1 produit un blob et s2 le consomme par `${s1.blob}` → aucun passage du contenu par le blackboard.
- La troncature n'altère pas les petits résultats (< plafond).

**Effort estimé** : M. **Dépendances** : chantier blob committé (fait).

---

## P3 — Revalidation des args après résolution des templates

**Problème.** `validate_plan` (plan.rs) valide les args contre `schema_in` **seulement** quand ils ne contiennent aucun template. À l'exécution, `spawn_step` vérifie uniquement qu'il ne reste pas de `${...}` non résolu — les args *résolus* ne sont jamais revalidés contre le schéma. Une erreur de type du LLM (string là où le schéma attend un int) part en invoke réseau signé et échoue chez le pair.

**Approche.** Dans `spawn_step` après `resolve_value` : réutiliser `validate_args_against_schema` (déjà présent, jsonschema Draft7) sur les args résolus. En cas d'échec : `TraceEntry.error` local, pas d'invoke réseau. Même politique de tolérance que la validation existante (schéma vide/non-objet = accepte tout).

**Critères d'acceptation.**
- Test : plan dont s2 reçoit `${s1.value}` (int) dans un champ déclaré string → erreur locale, aucun appel réseau (mock compteur d'invokes).
- Les plans valides existants passent inchangés (suite de tests planner verte).

**Effort estimé** : S. **Dépendances** : aucune. Quick win — bon premier item.

---

## P4 — Admission par backend, pas seulement par dispatch

**Problème.** `MAX_CONCURRENT_STEPS = 4` (plan.rs) est un sémaphore **par dispatch** : il borne le fan-out d'un plan mais (a) ne sérialise pas deux steps du même plan visant le même backend mono-GPU — contrairement à ce que son commentaire prétend ; (b) ne protège pas contre N dispatches simultanés × 4 steps frappant le même upstream.

**Approche.**
1. Registre de sémaphores keyé par identité de backend/peer (endpoint pour les remotes, nom de backend pour les locaux), partagé au niveau `Node` ou runtime (pas par dispatch).
2. Limite par défaut : 1 pour les backends locaux type Ollama (mono-GPU), configurable par backend dans `backends/<name>.toml` (champ optionnel `max_concurrency`).
3. Conserver la borne globale par dispatch existante en plus (défense en profondeur).

**Critères d'acceptation.**
- Test : plan à 3 steps indépendants sur le même backend local `max_concurrency=1` → exécution sérialisée (vérifiable par timestamps ou compteur de concurrence max dans un mock).
- Steps sur des backends distincts restent concurrents.
- Champ TOML optionnel, rétrocompatible (absence = défaut).

**Effort estimé** : M. **Dépendances** : aucune.

---

## P5 — Boucle de réputation locale dans le ranking et la confidence

**Problème.** `Catalog::build_for_query` classe les caps remote par BM25 pur sur les manifestes auto-déclarés ; la `confidence` du compiler ne sert qu'à émettre l'événement UI `LowConfidence`. Aucun apprentissage de l'expérience : un pair qui échoue systématiquement reste aussi bien classé qu'un pair fiable.

**Approche.**
1. Table `cap_outcomes` (migration) : `(peer_id, capability, ok_count, err_count, total_latency_ms, last_seen)`. Alimentée à la persistance des tool turns (synergie avec P1).
2. Score de fiabilité lissé (laplace : `(ok+1)/(ok+err+2)`) blendé dans le ranking de `build_for_query` — pondération modeste (ex. multiplicateur 0.5–1.0 du score BM25) pour ne pas écraser la pertinence sémantique.
3. Cache négatif : pair avec M échecs consécutifs récents → exclu du catalogue pendant T minutes (constantes configurables).
4. Faire agir la confidence : sous le seuil 0.5, si un `CascadingCompiler` est configuré, l'escalade existe déjà — sinon tenter **une** re-compilation avec température réduite avant d'abandonner.

**Critères d'acceptation.**
- Test : deux caps équivalentes au BM25, l'une avec historique d'échecs → la fiable est classée première.
- Test : cache négatif expire après T.
- Aucun changement de protocole fil ; tout est local à l'instance.

**Effort estimé** : M–L. **Dépendances** : P1 (point d'alimentation propre).
**Note conceptuelle** : c'est l'amorce du registre « hebbien » (co-activation renforcée) et un élément de réponse à la question ouverte anti-free-riding (archi §11) — le consigner dans le doc d'archi par note datée au moment de l'implémentation, sans trancher la question §11 silencieusement.

---

## P6 — Façade OpenAI-compatible du dispatch

**Problème.** Le planner n'est consommable que par l'UI maison (`/api/v0/conversations/:id/messages`). Le standard de fait pour consommer « un modèle » est l'API chat completions d'OpenAI ; tout outil existant (Open WebUI, LiteLLM, n8n, IDE…) sait parler ça et rien d'autre.

**Approche.**
1. Route locale `POST /api/v0/openai/v1/chat/completions` (crate `server`, même politique loopback/RBAC que le reste de l'API locale) : mappe `messages` → dispatch (dernier message user = input ; historique → `ConversationState` éphémère ou conversation dédiée), `model` → `DispatchOptions.model_override` + sélection `auto`/`direct` (convention : `model: "n3ur0n-auto"` / `"n3ur0n-direct"` / nom de modèle réel).
2. `stream: true` → SSE chunks OpenAI ; mapper `DispatchEvent` : `StepStart/StepDone` en commentaires ou champ `n3ur0n_trace` d'extension, `Final` en chunks de contenu + `[DONE]`.
3. Réponse non-streaming : objet chat completion standard (`choices[0].message.content`, `model`, `usage` omis ou approximé).

**Critères d'acceptation.**
- `curl` du smoke script : completion non-streaming et streaming contre un node avec backend echo.
- Open WebUI pointé sur le node fonctionne sans configuration spéciale (test manuel documenté dans le README du docker cluster).
- Aucun impact sur l'API conversations existante.

**Effort estimé** : M. **Dépendances** : aucune (P2 recommandé avant pour les réponses volumineuses).

---

## Hors scope explicite (ne pas implémenter sans décision)

- Reprise automatique des plans interrompus (P1 phase 2).
- Import Agent Skills (SKILL.md → cap prompted) : levier d'adoption validé conceptuellement, mais demande une décision produit sur le mapping frontmatter → descriptor.
- Politique de flux inter-domaines (étiquetage trust-domain des steps + règles d'egress des données par classe de blob) : différenciateur stratégique, à spécifier dans un doc dédié avant code.
- Embeddings/reranker en remplacement de BM25 (retrieval.rs le note : post-v0.2).

## Ordre de réalisation suggéré

P3 (quick win, confiance dans la chaîne) → P1 → P2 → P4 → P6 → P5. Chaque item : branche dédiée, tests d'abord quand le critère le permet, `cargo fmt` + `clippy -D warnings` + `cargo test --workspace` verts avant merge (la CI bloque sinon), note datée dans les docs touchés si un comportement documenté change.

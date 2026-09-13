# Revue du moteur de planification — pertinence et rapidité

**Statut : rapport d'analyse. Aucun changement de code n'a été appliqué.**
Date : 2026-09-13. Branche analysée : `feat/lobes-and-chat-addressing` (commit de tête `f377aec`).
Rien de ce document n'est acté : c'est une base de décision, pas une décision.

---

## 1. Méthode et conventions de preuve

Le contenu vient d'un débat adversarial en **trois rounds** entre deux modèles (Claude Opus 5 comme contradicteur, GPT-5 Codex comme analyste), chaque round contestant le précédent. Toute affirmation structurante a été **revérifiée directement dans le code** avant d'être retenue ici : plusieurs conclusions du round 1 ont été corrigées ou retirées aux rounds 2 et 3 (§8).

Trois niveaux de preuve, jamais mélangés :

| Marque | Sens |
|---|---|
| **[V]** | Vérifié dans le code de cette branche, référence fichier:ligne donnée. |
| **[M]** | Mesure antérieure du projet, non réexécutée ici. |
| **[H]** | Hypothèse falsifiable — prédiction, pas constat. Le critère de falsification est donné. |

Aucun benchmark n'a été exécuté pendant cette revue. Les chiffres **[M]** proviennent de la suite d'éval et du bake-off déjà consignés dans `CLAUDE.md` et `n3ur0n-planner-selection-v0.md`.

---

## 2. Verdict macro

**La macro-architecture est conservée : `compile → DAG parallèle borné → reflect`.**

Elle est déjà la combinaison saine de trois patrons du secteur, et non un choix maison à rattraper :

- **Plan-and-Solve** — planification séparée de l'exécution ;
- **ReWOO** — plan préalable avec variables `${step.path}`, aucun appel LLM entre les steps [V] `plan.rs:30-63`, `plan.rs:386-416` ;
- **LLMCompiler** — ordonnancement parallèle des steps dont les dépendances sont satisfaites [V] `plan.rs:547-575`, `plan.rs:833-899`.

Aucun retour à ReAct : retiré en 0.2.0 pour des raisons mesurées (sur-planning, latence). Rien dans cette revue ne le remet en cause.

**En revanche, la thèse « à 165 caps, il ne reste que la qualité des manifestes à améliorer » est rejetée.** Elle reposait sur `tool_valid 100 % / tool_exact 88 %` [M], or la notation actuelle réduit un plan à un `BTreeSet<String>` de **noms de capacités** [V] `planner_eval.rs:179-181`. Elle ne note ni le peer, ni les arguments, ni les dépendances, ni la cardinalité, ni l'ordre, ni le résultat exécuté. Les 12 points manquants ne sont donc **pas attribuables** en l'état : ils peuvent venir de la discrimination entre quasi-doublons, de la synthèse d'arguments, ou de la structure du DAG, et rien ne permet aujourd'hui de trancher.

C'est le même schéma que trois fois plus tôt dans ce projet : **l'instrument de mesure est le facteur limitant.**

---

## 3. Défauts vérifiés dans le code

Tous marqués **[V]**, avec la référence. Ce sont des constats, pas des opinions.

### 3.1 Pertinence

**D1 — Le compile n'a aucune mémoire conversationnelle.**
Le compiler ne reçoit que `planner_text` et le catalogue [V] `plan_exec.rs:227-276`. L'historique n'est fourni qu'au reflect [V] `plan_exec.rs:711-715`. Un tour comme « refais-le sur le deuxième document » ou « et en anglais ? » ne peut donc ni sélectionner ni remplir correctement un plan.

**D2 — Une erreur de parse JSON est convertie en abstention légitime.**
`LocalLLMCompiler` transforme un JSON non parsable en plan vide [V] `compiler.rs:135-142`, et `resolve_plan` traite un plan vide comme une décision correcte, sans retry [V] `plan_exec.rs:291-314`. Le système **ne peut pas distinguer** « aucun outil nécessaire » de « la sortie de compilation est cassée ». Les deux produisent la même réponse et la même trace.

**D3 — `default_confidence` n'implémente pas sa propre documentation.**
Sa doc annonce quatre tiers dont `0.0` sur échec de validation structurelle et `0.5` conditionné à un message user « long (> 20 tokens) ». L'implémentation ne fait ni l'un ni l'autre : elle ignore son paramètre `_catalog`, n'appelle jamais `validate_plan`, et **ne reçoit pas le message user** — elle ne peut donc pas l'implémenter [V] `compiler.rs:147-168`. Conséquence : un plan invalide de 1 à 8 steps reçoit `0.9`, et un plan vide reçoit `0.5`, ce qui ne déclenche pas l'escalade de cascade (`>= threshold`) [V] `compiler.rs:288-305`.

**D4 — Les exemples négatifs sont indexés comme des exemples positifs.**
`searchable_text` concatène `negative_examples.user_intent` dans le même document que les intentions positives [V] `retrieval.rs:139-143`, texte ensuite utilisé tel quel par BM25 **et** par le modèle d'embedding [V] `retriever.rs:133-145`. Une capacité voit donc son score **monter** pour exactement l'intention qui la déclare inapplicable. Cas concret dans les fixtures : `fetch_url` porte le négatif « Find the official Rust website », voisin immédiat du cas qui attend `web_search`.

C'est une incohérence de contrat entre couches : le même champ est un signal de rejet pour le LLM et un signal positif pour le retriever.

**D5 — `@file:` est collecté puis jeté.**
`MentionScope` collecte les fichiers, mais `is_empty()` les ignore et le scoping ne transmet que peers / lobes / capabilities [V] `mention.rs:255-260`, `plan_exec.rs:233-267`. Aucun autre consommateur n'existe. Le modèle doit donc interpréter la mention comme du texte.

**D6 — L'historique masqué laisse passer les valeurs par la prose.**
`filter_visible_for_llm` masque les `ToolCall`/`ToolResult` antérieurs au dernier Assistant, mais **conserve tous les User et Assistant** [V] `conversation.rs:406-423`. Une valeur d'outil d'un tour précédent reste donc visible si l'Assistant l'a citée. Le reflect reçoit simultanément cet Assistant historique, le blackboard courant, et une **seconde copie** du message user [V] `plan_exec.rs:711-759`. C'est un chemin cohérent avec `bug-215` (reflect citant la chaîne inversée d'un tour antérieur) — sans que ce bug ait été reproduit pendant cette revue.

**D7 — Les arguments sont le trou restant du décodage contraint.**
La grammaire contraint la forme et les noms d'outils, mais laisse `args` en objet JSON libre [V] `grammar.rs:60-63`, `grammar.rs:225-247`. Sous Ollama, deux énumérations indépendantes sont envoyées pour le peer et la capacité [V] `grammar.rs:34-58` : un peer valide couplé à la mauvaise capacité reste syntaxiquement acceptable. Et dès qu'un template `${...}` apparaît, la validation de tout l'objet est reportée à l'exécution, où seul le premier segment du chemin est contrôlé [V] `plan.rs:124-193`.

### 3.2 Rapidité

**D8 — Un plan vide paie deux appels LLM pour n'exécuter aucun outil.**
Le compile est toujours appelé, et un plan vide part immédiatement vers `reflect_only`, qui refait un `invoke("chat")` [V] `plan_exec.rs:269-314`, `plan_exec.rs:761-768`. Coût du préfill compile gaspillé, avec les chiffres mesurés (876 fixes + ~161 marginaux par capacité) [M] :

| Catalogue | Préfill compile |
|---|---|
| 20 caps | ≈ 4 096 tokens |
| 12 locales + 20 remotes | ≈ 6 028 tokens |

**D9 — `MAX_TOTAL_STEPS` et le budget de 300 s ne bornent rien pendant un round.**
Les deux ne sont testés qu'au moment de décider d'un round supplémentaire [V] `plan_exec.rs:1062`. Un premier plan de 8 steps peut donc être suivi d'un second de 8, intégralement exécuté : **le plafond effectif est 16, pas 12**. Symétriquement, les 300 s n'interrompent ni un compile, ni un plan en vol, ni le reflect.

**D10 — Le déclencheur `depth >= 3` taxe les plans sains.**
Tout plan de profondeur ≥ 3 déclenche une continuation, **même complet et même entièrement réussi** [V] `plan_exec.rs:1055-1075`. C'est un appel LLM dépensé sur un signal qui n'est pas une observation d'incomplétude, mais une propriété du plan déjà accepté.

**D11 — Une découverte réseau par invocation.**
`send_signed` appelle inconditionnellement `discover_recipient` (un GET `/health`) avant le POST signé [V] `client.rs:94`. Chaque step distant en paie une [V] `plan.rs:778-809` ; avec blobs, l'upload d'entrée et le download de sortie en ajoutent chacun une [V] `blob_resolve.rs:179-247`.

Impact réel, nuancé — les steps prêts partent par lots de 4 [V] `plan.rs:769-775` :

| Forme du DAG | RTT sur le chemin critique |
|---|---|
| 6 steps indépendants | ≈ 2 |
| chaîne de 6 steps | ≈ 6 |
| avec blobs | jusqu'à 3 découvertes séquentielles par step |

Soit **[H]** 2–30 ms en LAN (marginal devant un compile de plusieurs secondes) mais **100–900 ms en WAN** (non marginal). Le peer visé est déjà connu : `ToolDef` transporte l'id complet et l'endpoint [V] `catalog.rs:11-17`, et la réponse peut toujours être vérifiée contre cet id comme elle l'est aujourd'hui contre celui du health [V] `client.rs:116-125`.

**D12 — Catalogue et index BM25 reconstruits à chaque dispatch.**
`Catalog::build` charge jusqu'à **500 peers** (littéral au point d'appel) [V] `plan_exec.rs:227-231`, parse chaque `describe_self_cached`, clone son tableau `capabilities` et désérialise chaque entrée [V] `catalog.rs:74-138`. Puis `BM25Index::build` retokenise tous les manifestes [V] `retriever.rs:175`, `retrieval.rs:44`. Enfin `score(query, doc_idx)` **retokenise la query pour chaque document** [V] `retrieval.rs:82`.

**D13 — Le cache d'embeddings est clé sur la version, pas sur le contenu.**
Sa clé est `(model, peer, cap, version)` et il n'est pas borné [V] `retriever.rs:58-68`, `retriever.rs:167-172`. Un manifeste modifié sans bump de version conserve un vecteur périmé. Conséquence méthodologique immédiate : toute ablation de D4 en mode hybride doit reconstruire le `Retriever`, sinon elle réutilise précisément le vecteur qu'elle prétend invalider.

**D14 — Le schéma est maintenu à la main.**
GBNF et JSON Schema portent une consigne « keep in sync » [V] `grammar.rs:76-83`, et trois dialectes de contrainte sont envoyés simultanément en supposant que l'upstream ignore ceux qu'il ne connaît pas [V] `compiler.rs:112-123`. C'est de la compatibilité best-effort, pas une négociation de capacité. Ajouter un champ à `PlanStep` sans toucher aux deux grammaires produirait une divergence silencieuse.

---

## 4. Techniques du secteur : ce qui s'applique ici

| Technique | Verdict | Motif |
|---|---|---|
| **Plan-and-Solve** | Déjà absorbé | Compile / exécution / synthèse déjà séparés. |
| **ReWOO** | Déjà absorbé | Variables `${step.path}`, aucun LLM entre steps. À conserver. |
| **LLMCompiler (scheduler)** | Déjà absorbé | Ordonnancement parallèle borné présent. |
| **LLMCompiler (joiner LLM)** | **Rejeté** | Un troisième appel systématique à 7B ne se paie pas. Un joiner déterministe instrumenté est préférable. |
| **Décodage contraint** | **Partiellement à faire** | Le gain restant est la liaison **pair-exacte** (aujourd'hui deux énumérations indépendantes sous Ollama) et, prudemment, des args typés par outil. |
| **Progressive disclosure / MCP tool search** | **Rejeté pour l'instant** | Le top-K avant compile *est* déjà une divulgation progressive, non agentique. Un outil `search_tools` appelable réintroduirait une boucle séquentielle, injustifiée à r@20 hybride = 100 % [M]. |
| **Code mode** | **Rejeté** | Le DAG JSON est déjà le DSL minimal. Générer du code élargit la surface de sûreté sans résoudre la discrimination. |
| **Self-consistency** | **Rejeté en production** | 2–5 compiles multiplient la latence et il manque un arbitre fiable. Utile hors ligne pour repérer les cas instables. |
| **Reranking cross-encoder** | **Non justifié aujourd'hui** | K=20 conserve déjà la cible. À reconsidérer seulement pour descendre à K=5–10. **[H]** sur la machine du 7B, un reranker concurrencerait les ressources d'inférence. |
| **Few-shot dynamique** | **Oui, forme limitée** | Aujourd'hui les deux premiers exemples sont toujours rendus [V] `plan_exec.rs:1238-1259`. Choisir l'exemple positif le plus proche + un contraste pertinent. |
| **Meilleurs manifestes** | **Oui, mais pas en volume** | Sous forme de **contrastes pairwise**, pas de prose supplémentaire : ajouter des exemples a déjà produit des effets non locaux, documentés en `n3ur0n-planner-selection-v0.md:153-163`. |

---

## 5. Rapidité : ce que la mesure a tué

### 5.1 Chemin critique d'un dispatch

lock conversation → slot planner → chargement LRU/SQLite → persistance du tour user → **construction catalogue (500 peers)** → retrieval (+ appel embedding en hybride) → rendu prompt → **compile LLM** → validation (+ éventuel recompile correctif) → journal SQLite → **DAG** → éventuelle continuation → **reflect LLM** → persistance.

**[H]** hors outils distants lents, les deux préfills/générations LLM dominent largement SQLite, BM25 et le rendu Rust. **La suite d'éval ne chronomètre aujourd'hui que le compile** [V] `planner_eval.rs:272-278`, jamais le dispatch complet : cette hypothèse n'est pas mesurée chez nous.

### 5.2 La réutilisation KV de préfixe ne vaut pas le détour — chiffré

C'était une piste séduisante : rendre déterministe l'ordre des blocs de capacités pour qu'un serveur amont réutilise son cache KV entre tours. **Le chiffrage la tue.**

Le catalogue est trié par score puis tronqué [V] `catalog.rs:217-245`, et une cache KV s'arrête au **premier bloc différent** — donc le Jaccard ne suffit pas, il faut un *préfixe* commun. Sous un modèle pessimiste de sous-ensembles aléatoires parmi 165 caps :

- deux top-20 partagent en moyenne `20²/165 ≈ 2,4` capacités ;
- Jaccard moyen ≈ 6,4 % ;
- probabilité que le premier élément de l'union appartienne aux deux ≈ 6–7 %.

**Même avec 2,4 membres communs, ils ne forment presque jamais un préfixe commun.** On ne réutiliserait que les ~876 tokens fixes, sur ~4 096. Le seul cas favorable est celui des capacités locales, déjà placées avant les remotes [V] `catalog.rs:237-244` : 4 locales stables donnent ≈ 1 520 tokens de préfixe, 12 donnent ≈ 2 808.

**Critère de reprise :** ne poursuivre que si la médiane du `longest_common_prefix` entre tours successifs d'une même conversation atteint **au moins 8 blocs** (≈ 2 164 tokens réutilisables). En dessous de 2 blocs, le gain additionnel est d'environ 322 tokens — négligeable.

S'ajoute une réserve **[V]** : le client HTTP n'expose aucune clé de cache ou de session [V] `openai.rs:317-344`. Tout bénéfice dépendrait d'une cache automatique de l'amont, **non vérifiée dans ce dépôt**.

### 5.3 Ce qui est parallélisable, et ce qui ne l'est pas

Parallélisable : les steps sans dépendances (déjà fait) ; BM25 et l'embedding de la query, si l'API séparait query et cache-miss du catalogue ; la préparation blob et la résolution des destinataires, préchargeables après validation.

**Non parallélisable sans spéculation incorrecte** : le compile avant le catalogue filtré ; un step dépendant avant résolution de ses `${refs}` ; le reflect avant les résultats ; la continuation avant de connaître les erreurs.

**Spéculation : non.** Ne jamais exécuter un outil avant validation — le coût d'un faux positif avec effet de bord dépasse le gain. Seuls le préchauffage du modèle et la résolution réseau sont des spéculations acceptables.

---

## 6. Propositions, classées après trois rounds

Classement par `(gain × confiance) / coût`. **P0 n'est pas un gain produit** : c'est le préalable qui rend les autres mesurables. Cette distinction a été concédée au round 2 après contestation.

### P0 — Rendre l'évaluation pair-aware et argument-aware · *préalable, pas gain*

La suite ne peut pas aujourd'hui exprimer « ce peer précis », « plusieurs plans défendables », ni noter des arguments. Forme proposée, qui garde le cas ordinaire compact :

```json
"accept": [ { "steps": ["translate"] } ]                    // n'importe quel peer
"accept": [ { "steps": ["llm_large::chat"] } ]              // ce peer exactement
"accept": [ { "steps": ["fetch_url"] },
            { "steps": ["fetch_url", "chat"] } ]            // deux plans acceptables
```

Pour les arguments, **ni égalité exacte** (trop fragile) **ni juge LLM** (non reproductible, coûteux à 7B) : des **invariants déterministes déclarés par cas**, via JSON Pointer — `eq`, `one_of`, `text_eq`, `contains_all`, `number_range`, `ref{step,path}`, `absent` — enchaînés après validation contre le `schema_in` réel.

L'égalité exacte est réservée aux valeurs qui *doivent* être copiées : URL, montant, code langue, chaîne utilisateur, enum, référence. Pour un champ rédactionnel (« écris un haïku sur la pluie »), on vérifie `contains_all: ["haiku", "rain"]` — que les contraintes critiques ont survécu, pas que la formulation est élégante.

*Falsifié si* les métriques enrichies reproduisent exactement le classement actuel sans découvrir un seul échec nouveau.

### P1 — Séparer l'échec de compilation de l'abstention (D2 + D3)

Distinguer `parse_error` de « plan vide intentionnel », autoriser un retry sur le premier, appeler `validate_plan` dans le score de confiance, calibrer le seuil. Coût faible, confiance haute — et c'est un défaut de **correction**, pas de performance.

*Falsifié si*, sur ≥ 5 runs par cas avec le modèle cible, parse et validation restent à 100 % au premier passage et la cascade n'altère aucun échec.

### P2 — Supprimer la continuation fondée sur la profondeur (D10)

Prédicat de remplacement, **sans ambiguïté** :

```
continue = trace contient au moins une erreur
         ET trace.len() < MAX_TOTAL_STEPS
         ET elapsed < DISPATCH_BUDGET_SECS
```

Justifie un second round : **une invocation prévue n'a pas produit de résultat utilisable.**
Ne le justifie pas : profondeur élevée, nombre de steps élevé, résultat court, plan atteignant 8 steps, incertitude sur la complétude.

Motif : après une exécution entièrement réussie, le runtime ne détient **aucune spécification formelle du but** lui permettant de prouver qu'il manque quelque chose. Dépenser un LLM sur ce doute est une taxe systématique. Le coût assumé est de renoncer à rattraper un plan « partiel mais réussi » — arbitrage explicite, pas oubli.

*Évolution ultérieure* : si `Plan` gagnait une liste de `deliverables` référencés (il ne contient aujourd'hui que `plan: Vec<PlanStep>` [V] `plan.rs:48-52`), le prédicat pourrait devenir `erreur OU un deliverable déclaré ne se résout pas dans le blackboard` — toujours déterministe.

### P3 — Rendre les budgets effectifs (D9)

Tronquer ou rejeter le round 2 à `MAX_TOTAL_STEPS - trace.len()`, poser une deadline globale, revérifier après compile. Coût faible, gain de sûreté et de p95.

### P4 — Supprimer la découverte réseau redondante (D11)

Signer directement pour `tool.peer_id`, déjà résolu et présent dans la trace avant l'appel [V] `plan.rs:741-763`. Remonté dans le top 5 au round 3 après contestation : ne retire aucun appel LLM, mais coût faible et p95 WAN nettement meilleur.

### P5 — Corriger la sémantique retrieval des négatifs (D4)

Le rappel@20 étant déjà à 100 % [M], **il ne peut pas monter** : c'est la mauvaise métrique. Le bug prédit une montée du **distracteur interdit**, pas une disparition de la cible. Mesurer, pour chaque paire (requête négative `q`, capacité interdite `d`) :

```
rank_lift = median(rank_without(d,q) − rank_with(d,q))
negative_attraction@5 = proportion de distracteurs interdits dans le top-5
marge = score(cible) − score(distracteur)
```

Le bug est confirmé si `rank_lift > 0`, si `negative_attraction@5` baisse après retrait, et/ou si la marge augmente — **sans** baisse du rappel pair-aware de la cible. Il est infirmé si les quatre restent plats.

Réserve honnête : pour BM25 un résultat plat serait surprenant (normalisation par longueur de document) mais possible. **Pour les embeddings le signe est moins prévisible** : encoder « ne pas utiliser pour X » peut théoriquement déplacer le vecteur dans une direction utile, même si les modèles n'encodent généralement pas une négation comme un poids négatif.

Puis, seulement ensuite : few-shot dynamique contrastif parmi les caps retenues.

### P6 — Branche `answer` dans le compile (D8) · *conditionnée*

Remplacer la sortie unique par une union `{"plan":[...]}` **ou** `{"answer":"..."}`, la seconde branche devenant directement le tour assistant — **un appel LLM au lieu de deux** pour tout tour sans outil.

**Cette proposition a été contestée et corrigée.** Telle que formulée au round 2 elle se contredisait avec D1 : le compile n'ayant pas d'historique, sa branche `answer` répondrait sans contexte là où le reflect en dispose — une régression franche sur « et en anglais ? ». La résolution retenue impose un **ordre de chantier**, P6 n'étant pas déployable seul :

1. définir une projection de contexte dédiée au compiler, **bornée à ~256 tokens** ;
2. y mettre le dernier tour user en clair, les pièces jointes, et des **handles opaques** vers les résultats historiques — jamais leurs valeurs en clair ;
3. étendre le DSL de références aux handles historiques (`${history.last_assistant}`, `${history.call_x.result}`) ;
4. *seulement alors*, introduire l'union `plan` / `answer` ;
5. sur `answer`, persister directement sans appeler `reflect_only`.

La stabilisation du préfixe catalogue (§5.2) n'est **pas** un prérequis : le catalogue reste dans le premier message système, un contexte ajouté après ne déplace pas ce préfixe.

Arithmétique de rentabilité, avec un surcoût de 256 tokens payé sur **tous** les dispatches :

```
seuil ≈ 256 / part_no_tool
```

À 33 % de no-tool, le reflect évité doit coûter plus de ~776 tokens — ce qu'il dépasse largement, puisqu'il porte aussi une génération autorégressive et un aller-retour backend, pas seulement son préfill.

**Réserve dirimante sur le 33 %** : les 14 cas `none` + `trap` sur 42 de la suite [V] décrivent **notre conception de la suite, pas le trafic réel**. Aucune mesure de la part réelle de tours sans outil n'existe aujourd'hui. **[H]** P6 reste rentable au-dessus de 20–25 % de réponses directes ; **en dessous de 10 %, ne pas la déployer** — le contexte ajouté à tous les compiles dominerait les reflects économisés. *Mesurer la part réelle est donc un prérequis de P6.*

### P7 — Fast path phatique

**Également contesté et corrigé.** Le round 2 exigeait 0 faux négatif sur ~3 000 cas (borne à 0,1 % par la règle de trois), critère hors d'atteinte avec une suite de 42 cas — la recommandation revenait donc à « ne pas faire ». Le round 3 a **retiré l'argument statistique** : il ne s'applique pas à un domaine fermé défini par construction.

C'est une **décision de routage produit**, pas un classifieur :

```
attachments.is_empty()
ET parse_mentions(text).is_empty()
ET normalized_full_text ∈ PHATIC_WHITELIST
```

Égalité sur le message **entier**, après normalisation de casse, espaces et ponctuation terminale — donc « merci d'envoyer le fichier » ne matche pas. Liste initiale : *bonjour, salut, bonsoir, merci, merci beaucoup, hello, hi, hey, thanks, thank you, ok, d'accord, compris*. Les deux préconditions sont déterministes et vérifiables [V] `mention.rs:125-208`, `conversation.rs:44-49`.

Conséquence assumée : même si un publisher expose une capacité `greet`, « bonjour » seul reste direct. **Ce n'est pas un faux négatif, c'est la politique de routage** — et elle doit être écrite comme telle.

P7 n'est pas dominée par P6 : P7 évite le catalogue, le retrieval **et** les ~4 096 tokens de compile ; P6 les paie toujours pour produire `answer`. P7 couvre peu de tours, mais les couvre avec une certitude structurelle.

### P8 — Catalogue et index incrémentaux (D12)

Un `CatalogSnapshot` partagé portant les `ToolDef` désérialisés, le texte indexable préconstruit, l'index BM25 inversé, les blocs de prompt pré-rendus, et une empreinte **par contenu** (pas par `version`, cf. D13). Invalidation ciblée : seules les caps d'un peer dont le `describe_self_cached` a changé. Un index BM25 persistant en SQLite n'est nécessaire qu'au-delà de ~50 000 caps.

*Ne pas implémenter avant les spans détaillés* : falsifié si l'instrumentation montre catalogue + retrieval + rendu < 2 % du p50 et < 5 % du p95.

### P9 — Liaison pair-exacte sous contrainte (D7)

Un identifiant `tool` unique ou un `oneOf` compact plutôt que deux énumérations indépendantes, la validation runtime restant en place. Gain de robustesse réel, mais `tool_valid` est déjà à 100 % [M] et un schéma complexe risque de faire régresser un 7B.

### P10 — Cache de plans, cross-encoder, joiner LLM, self-consistency

Gardés en réserve, gain incertain. Pour le cache de plans : ne cacher **que** le plan, jamais les résultats, clé incluant texte exact, scope résolu, catalogue, modèle et version du compiler ; observer d'abord les empreintes sans rien stocker. Falsifié si le taux de hit exact est < 5 %.

---

## 7. Seuils falsifiables

### 7.1 Limite dure actuelle **[V]**

L'architecture **ne peut pas** achever :

- une tâche nécessitant plus de **16 invocations indispensables** (8 steps × 2 rounds — le plafond contractuel visé étant 12, cf. D9) ;
- une tâche nécessitant **plus d'une frontière adaptative** : observer, replanifier, exécuter, puis devoir observer et replanifier encore. Le second round est le dernier.

Symptôme observable : `rounds == 2`, budget de steps épuisé, objectif non satisfait. Aucun troisième compile ne peut être déclenché.

### 7.2 Limite opérationnelle défendue pour un 7B **[H]**

Fiable sans changement de macro-architecture jusqu'à : **8 steps au total, profondeur séquentielle 4, une seule frontière adaptative.** Au-delà, taux de complétion correcte prédit **< 90 %**, même avec un retrieval parfait.

Base de la prédiction : la suite ne couvre aujourd'hui **aucune chaîne de plus de 2 outils** — les 7 cas `chain` ont tous exactement 2 outils [V]. Les 95 % / 88 % [M] ne valident donc rien au-delà de la profondeur 2, et le runtime traite déjà `depth >= 3` comme suspect.

*Protocole de falsification* : chaînes synthétiques de profondeur 2, 4, 6, 8 ; mêmes outils, arguments déterministes, références vérifiables ; 20 runs par profondeur ; succès = tous les steps requis une fois, refs exactes, résultat final correct, aucun round inutile. **Position infirmée si les profondeurs 6 et 8 tiennent chacune ≥ 90 %.** Confirmée si la chute sous 90 % arrive à la profondeur 6 ou avant.

### 7.3 Seuil en nombre de capacités

**Pas de seuil sur le prompt** : top-20 le borne indépendamment de 165, 1 000 ou 10 000 caps. Le seuil pertinent y est un seuil de **qualité**, observable : rappel pair-aware@20 < 99 % ou MRR de la cible < 0,5. À ce moment la sélection one-shot n'est plus assez alimentée, et il faut une deuxième phase — retrieval hiérarchique ou reranking, pas nécessairement ReAct.

**Mais il existe bien un seuil hors prompt** (D12), que le refus initial d'en donner un occultait. **[H]**, non profilé :

| Volume | Coût catalogue + indexation par dispatch |
|---|---|
| ≤ 2 000 caps | quelques dizaines de ms — secondaire |
| ~10 000 caps | 100–300 ms |
| ~20 000 caps | 200–600 ms, soit 10–20 % d'un compile |
| ~50 000 caps | peut approcher ou dépasser 1 s |

**Seuil d'intervention retenu : 10 000 capacités ou 250 ms p95 sur catalogue + retrieval, au premier des deux atteint.** Avec la limite de 500 peers, 10 000 caps correspondent à 20 caps par peer en moyenne — pas un volume absurde. Attendre 20 000 serait accepter que le problème soit déjà visible par l'utilisateur.

---

## 8. Ce que le débat a corrigé

Consigné parce que les conclusions initiales auraient orienté le travail à côté :

| Round 1 disait | Corrigé en |
|---|---|
| « Enrichir l'éval » est le rang 1 du classement ROI | C'est un **instrument**, pas un gain produit. Retiré du classement fonctionnel, gardé comme préalable (P0). |
| Falsifier D4 en mesurant `r@K`, MRR, tool-exact | `r@20` est déjà à 100 % et **ne peut pas monter**. Métrique refaite autour du rang du **distracteur** (P5). |
| `discover_recipient` mentionné en passant | Remonté au top 5 (P4) — mais la prémisse « 6 steps = 6 RTT critiques » était **trop forte** : ≈ 2 RTT pour un DAG large, 6 pour une chaîne. |
| Réponse longue sur la pertinence, courte sur la rapidité | P6 et P7 ajoutés : les seules propositions qui **retirent un appel LLM**. |
| Branche `answer` proposée telle quelle | Contredisait D1. Conditionnée à un ordre de chantier et à une mesure de trafic (P6). |
| Fast path exigeant 0 FN sur 3 000 cas | Critère hors d'atteinte ⇒ équivalait à « ne pas faire ». Requalifié en **politique de routage** sur domaine fermé (P7). |
| Refus de tout seuil en nombre de capacités | Vrai pour le prompt, **faux pour la préparation** : seuil donné à 10 000 caps / 250 ms p95 (§7.3). |
| Stabiliser l'ordre du catalogue pour réutiliser le KV | Chiffré puis **abandonné** : ~6–7 % de chance de préfixe commun (§5.2). |

---

## 9. Ce qui reste ouvert

- **La part réelle de tours sans outil** n'est pas mesurée. C'est le prérequis chiffré de P6, et le seul chiffre dont nous disposons (33 %) décrit la suite de test, pas l'usage.
- **`bug-215`** (reflect citant la valeur d'un tour antérieur) a un chemin plausible — D6 — mais n'a pas été reproduit pendant cette revue. La correction proposée est stricte : quand le blackboard courant est non vide, ne pas fournir au reflect les Assistant historiques en prose ; l'historique en clair n'est admissible que sur un tour direct, où il *est* la matière de la conversation.
- **`@file:` (D5)** : décider s'il devient une ressource résolue transmise au plan, ou s'il reste du texte. En l'état il promet à l'utilisateur quelque chose que le planner ne tient pas.
- **P0 modifie les fixtures**, pas seulement le code de notation. Le chiffrage de ce coût n'a pas été fait.
- Aucune **instrumentation de latence bout-en-bout** n'existe : la suite ne chronomètre que le compile. Plusieurs **[H]** de ce rapport ne sont pas décidables sans elle.

---

## 10. Si un seul chantier devait être retenu

**P1** (séparer l'échec de compilation de l'abstention) : coût faible, confiance haute, et c'est un défaut de correction — aujourd'hui le système ne peut pas distinguer « je n'ai besoin d'aucun outil » de « ma sortie était cassée », et répond identiquement dans les deux cas.

Si deux : **P1 + P2** (supprimer la continuation par profondeur). La seconde retire une dépense LLM systématique sur des plans sains, avec un prédicat de remplacement qui tient en trois lignes.

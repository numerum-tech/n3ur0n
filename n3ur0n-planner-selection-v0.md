# N3UR0N — Sélection de capacité par le planner (brainstorm v0)

> **Statut** : brainstorm ouvert, 2026-07-28. **Rien n'est acté.** Ce document sert à cadrer la refonte de la sélection d'outils avant tout code.
> **Périmètre** : compile step du `PlanExecPlanner` (catalogue, retrieval, decoding contraint, cadrage par mention explicite et par lobe). Aucun impact protocole fil attendu — à réévaluer si la granularité de la synapse est tranchée autrement (§8).
> **Deux questions ouvertes §11 de l'architecture sont touchées** — elles sont remontées en §8, pas tranchées ici.

## 1. Objet

Le planner compile un DAG en **un seul coup** et doit donc choisir le bon outil **du premier coup, sans retour d'exécution**. Ce document part de ce que fait l'état de l'art pour choisir un outil, situe honnêtement N3UR0N dedans, et propose un ordre de travaux.

Le déclencheur : la fiabilité de sélection mesurée (12 % → 100 % de `tool_valid` en 0.4.2) repose aujourd'hui **entièrement sur la formulation du prompt**. C'est un mur porteur sans fondation.

## 2. Constat sur le code (vérifié 2026-07-28)

| Fait | Emplacement |
|---|---|
| Les grammaires de decoding contraint sont **statiques**, indépendantes du catalogue. `peer` et `capability` sont des **chaînes libres** (`{"type":"string","minLength":1}` / `step-peer ::= string`). Rien n'empêche structurellement un outil halluciné. | `planner/grammar.rs` (GBNF + JSON Schema) |
| `Catalog::to_openai_tools()` existe et n'est **appelé nulle part**. Le post-training « function calling » des modèles n'est pas utilisé — la sélection passe par de la prose dans le system prompt. | `planner/catalog.rs` |
| Le retrieval est **BM25 lexical**, et ne porte que sur les caps **distantes**. Les caps locales ne sont ni indexées ni classées ni plafonnées. | `planner/catalog.rs`, `planner/retrieval.rs` |
| Le garde-fou de taille de prompt est **consultatif** : il logge un warning au-delà de `COMPILE_PROMPT_TOKEN_BUDGET` et compile quand même. Rien ne tronque — le modèle tronque silencieusement à sa place. | `planner/compiler.rs` |
| Un échec de `validate_plan` **tue le plan entier** et retombe sur `reflect_only` (réponse sans aucun outil). Pas de retry. | `planner/plan_exec.rs`, `planner/plan.rs` |
| `CapabilityDecl.lobe_ids` existe, est saisissable dans le manifeste, transite dans `describe_self`, arrive jusqu'au catalogue — et **n'est lu par personne**. Champ déclaré, inerte. | `core/capability.rs`, `server/settings.rs` |
| `alias` est câblé de bout en bout (config → `describe_self` → `PeerRecord` → UI/CLI) mais **aucune résolution alias → peer** n'existe. Le prompt de compile montre au modèle `short_peer(peer_id)` (12 caractères du hash), jamais l'alias. | `core/protocol.rs`, `storage/peers.rs`, `planner/catalog.rs` |

## 3. Comment le champ choisit un outil

Quatre mécanismes, du plus au moins structurant :

1. **API de tool-calling native.** Les outils passent dans un tableau `tools` dédié, pas en prose. Les modèles sont post-entraînés sur ce format exact et le fournisseur **impose** que le nom émis soit l'un de ceux fournis. La sélection se fait dans la passe avant. Plafond pratique avant dégradation : ~20–50 outils.
2. **Retrieval sur les outils.** Au-delà du plafond : indexer les descriptions, ne présenter que le top-K pertinent pour la requête.
3. **Sélection hiérarchique / divulgation progressive.** Choisir d'abord un espace de noms (serveur, domaine), puis l'outil dedans. Ou exposer un méta-outil `search_tools` qui charge les définitions à la demande.
4. **Boucle de récupération (ReAct).** Mauvais outil → le modèle voit l'erreur → il corrige. **C'est le vrai filet de sécurité du champ**, et il porte plus de charge qu'on ne le crédite.

S'y ajoute la richesse des descriptions (exemples, contre-exemples, désambiguïsation) — **N3UR0N est en avance sur la norme** sur ce point précis, via `examples`, `negative_examples`, `disambiguation`, `output_semantic`.

## 4. La tension de design à nommer

Le compile one-shot est un choix délibéré et défendable : 2 appels LLM, exécution parallèle, déterminisme, pas de boucle ReAct (`LLMPlanner` a été supprimé en 0.2.0 pour ces raisons).

Mais il **abandonne le mécanisme 4**. Conséquence directe : la précision de sélection doit être **plus haute** que celle d'un agent classique, pas seulement équivalente. Or aujourd'hui N3UR0N est plus faible sur le mécanisme 1 (prose, noms non contraints) et plus faible sur le 2 (lexical, sur un produit bilingue). C'est là qu'est le problème, pas dans le prompt.

## 5. Travaux proposés, par levier décroissant

### 5.1 Construire la grammaire par dispatch depuis le catalogue

Remplacer `peer` + `capability` par un champ unique `tool`, énuméré sur les entrées réelles du catalogue :

```
step-tool ::= "\"tool\"" ws ":" ws ("\"abc123::chat\"" | "\"def456::translate\"" | ...)
```

Un outil halluciné devient **impossible** au lieu d'être rattrapé après coup. Corrige aussi un défaut que deux énumérations séparées garderaient : un `peer` valide apparié à une `capability` que ce peer n'expose pas. Le split côté serveur est trivial.

Le commentaire actuel de `grammar.rs` justifie le statique par « éviter de ré-émettre les schémas par cap à chaque dispatch » : ce raisonnement est **juste pour les schémas d'args**, et **faux pour une énumération de noms** (quelques centaines d'octets de concaténation).

C'est ce qui récupère l'essentiel de ce qu'apporterait le tool-calling natif — des noms valides imposés — sans renoncer au DAG. Le tool-calling natif ne sait pas exprimer un graphe de dépendances avec `${refs}` : il n'est pas réellement disponible ici.

### 5.2 Retrieval hybride au lieu de BM25 seul

Produit bilingue EN/FR. Une requête française contre une description anglaise score ~0 en recouvrement de termes ; BM25 ignore que *traduire* et *translate* sont liés. Ajouter des embeddings (un LLM local tourne déjà, un endpoint d'embedding coûte peu) et garder BM25 pour les termes rares/exacts.

**Mode de défaillance actuel, pas une préoccupation d'échelle.**

### 5.3 Un retry avec l'erreur de validation réinjectée

`validate_plan` échoue → recompiler **une fois** avec l'erreur en annexe (« step s2 : outil X absent du catalogue ; choisir parmi … »). Récupère une part du mécanisme 4 pour **un appel LLM supplémentaire uniquement en cas d'échec**. Transforme un repli sec « aucun outil » en seconde chance.

### 5.4 Classer les caps locales et rendre le budget contraignant

Indexer les locales dans le même BM25 avec un plancher (toujours garder le top-K local), et **tronquer** au budget en loggant ce qui est tombé. Une troncature explicite par nous vaut mieux qu'une troncature silencieuse par le modèle.

### 5.5 Sélection hiérarchique — plus tard

Peer d'abord, puis cap. L'espace de noms `peer::cap` a déjà la structure. Ne vaut le coup qu'à catalogue vraiment large.

> À noter : `RemotePlanCompiler` / `CascadingCompiler` est déjà un **routeur** — déléguer les sélections difficiles à un modèle plus gros. 5.1–5.3 le feront se déclencher moins souvent.

## 6. Couche de cadrage : mention explicite et lobe

**C'est le mécanisme 0** : quand l'utilisateur sait déjà où il veut aller, il n'y a plus de sélection à deviner. C'est la précision la plus haute disponible, et c'est gratuit.

### 6.1 Principe

Un caractère déclencheur dans le message cadre le catalogue **avant** retrieval :

```
« demande à @google la météo à Lomé »   → catalogue réduit aux caps du peer résolu
« @medical résume ce compte rendu »      → catalogue réduit aux caps portant ce lobe
```

Le cadrage est fait par le **parseur**, pas par le modèle. La mention reste dans le texte envoyé au compile (elle porte du sens : « … la météo à Lomé »), mais le périmètre est déjà tranché.

### 6.2 Pourquoi ça s'emboîte proprement

- **Avec 5.1** : catalogue cadré → énumération de la grammaire minuscule → sélection quasi déterministe. `@` et l'énumération sont le même mécanisme à deux échelles.
- **Avec 5.2** : une mention est un **filtre explicite**, pas une estimation classée — on court-circuite BM25 sur ce périmètre.
- **Avec 5.4** : cadrer, c'est aussi la façon la plus directe de tenir le budget de prompt.
- **Avec les lobes** : `lobe_ids` voyage **déjà** dans `CapabilityDecl` jusqu'au catalogue (§2). Le cadrage par lobe est un filtre sur `ToolDef.cap.lobe_ids` — **zéro ajout sur le fil**. C'est la bonne nouvelle pour « que ça tienne plus tard ».

### 6.3 Ce qui doit être cadré avant d'écrire du code

**a. L'alias n'a aucun poids cryptographique.** `instance_id` est `hash(pubkey)` : auto-vérifiable. Un alias est une chaîne de vanité **auto-déclarée** dans `describe_self`, que rien ne lie à quoi que ce soit. Deux peers peuvent revendiquer `@google`.

Conséquence non négociable : **un alias ne doit jamais être la clé de routage.** C'est une commodité locale de saisie et d'affichage qui se résout en `instance_id`, et la résolution doit être **locale d'abord** (annuaire de l'utilisateur, petnames assignés par lui prioritaires). Sinon le premier squatteur qui annonce `@google` détourne toutes les mentions.

C'est le problème du triangle de Zooko, et la réponse honnête est le **petname** : un nommage local, pas un espace de noms global.

**b. Ambiguïté peer / lobe.** `@x` désigne-t-il une instance ou un lobe ? Soit un ordre de résolution documenté, soit deux sigils distincts. À trancher avant l'UI, pas après.

**c. Parsing.** `@` entre en collision avec les adresses e-mail : « envoie à bob@example.com » ne doit pas être lu comme `@example`. Règle : `@` en début de token, précédé d'un blanc ou du début de ligne. Prévoir un échappement.

**d. Découvrabilité.** Sans autocomplétion sur `@` alimentée par l'annuaire local, la fonctionnalité est invisible. L'affordance UI fait partie de la fonctionnalité, pas d'un lot ultérieur.

**e. Appartenance à un lobe non vérifiée.** Une cap peut déclarer `lobe_ids = ["medical"]` sans que rien ne le contrôle. Le protocole d'appartenance aux lobes est en v0.5. **Avant lui, le cadrage par lobe est purement indicatif** — à ne pas présenter à l'utilisateur comme une garantie.

## 6bis. Résultat mesuré — le sur-planning était surtout un problème de modèle (2026-07-29)

Bake-off sur la suite d'éval, **3 runs par cas** (66 runs par modèle), endpoint Ollama local, suite et code identiques d'un modèle à l'autre :

| modèle | exact | valid | 1er passage | none | trap | chain | p50 |
|---|---|---|---|---|---|---|---|
| **qwen2.5:7b** | **100 %** | 100 % | 95 % | **100 %** | **100 %** | 100 % | 2711 ms |
| llama3.1:8b | 86 % | 100 % | 95 % | 83 % | 50 % | 75 % | 2087 ms |
| qwen2.5:3b | 68 % | 79 % | 73 % | 50 % | 0 % | 50 % | 978 ms |
| llama3.2:3b | 50 % | 86 % | 82 % | 67 % | 50 % | 25 % | 1979 ms |
| qwen2.5:0.5b | 41 % | 57 % | 59 % | 17 % | 0 % | 25 % | 604 ms |

Trois conclusions :

1. **`qwen2.5:7b` règle le sur-planning.** Les catégories d'abstention passent de 83 %/50 % (llama3.1:8b) à 100 %/100 %. Confirmé en runtime sur le cluster : `« What is 17 plus 25? »` produisait `[random_int, chat]` et une réponse incohérente avec llama3.1:8b ; avec qwen2.5:7b, **0 step** et « The sum of 17 and 25 is 42. » `llama3.1:8b` reste le défaut documenté et le défaut de `docker/compose.yml` — **à changer**.
2. **Falaise entre 3B et 7B, indépendante de la famille.** Les deux 3B échouent largement le seuil `tool_valid ≥ 95 %` (79 % et 86 %). 7B est le plancher praticable pour cette tâche. Un 0.5B est inutilisable (41 %).
3. **La suite est saturée en haut.** 100 % sur 22 cas × 3 runs ne discrimine plus rien : impossible de mesurer la marge restante de qwen2.5:7b. L'extension de la suite (§7) change donc d'objectif — non plus mesurer la faiblesse de llama, mais **trouver les modes d'échec de qwen**.

**Effet secondaire — cela tranche la question du retry (§5.3).** Le retry recouvre 3/3 plans invalides avec qwen2.5:7b (1er passage 95 % → 100 % après retry, exact reste à 100 %) : bénéfice net, aucun dommage. Avec les modèles faibles il est inutile ou nuisible — 0 récupération sur 27 tentatives en 0.5B, et sur llama3.1:8b son seul effet observé avait été de repêcher un cas `none` sur-planifié vers l'exécution d'un outil injustifié. Le retry est donc **à conserver, adossé à un modèle qui tient le seuil** ; ce n'est pas un correctif pour un modèle trop petit.

**Reste ouvert en runtime** : `« Translate … into French »` route encore vers `chat` (1 step) au lieu du plan vide attendu. Défendable (chat sait traduire) mais contraire à la règle du prompt. À couvrir par la suite étendue.

## 6ter. Suite durcie — ce que des capacités complexes révèlent (2026-07-29)

La suite d'origine tournait sur les 4 caps `UtilityBackend` : triviales, mono-argument, toutes sur **un seul** peer. D'où le 100 % de qwen2.5:7b — la suite ne discriminait plus rien.

Ajout de `crates/node/tests/fixtures/planner_caps.json` : 9 capacités sur 5 peers, fusionnées au catalogue réel. Chacune ajoute une pression précise — recouvrement sémantique (`translate` vs `chat`, `summarize` vs `extract_keywords`, `web_search` vs `fetch_url`), nom dupliqué (`chat` sur deux peers), schémas multi-arguments (`convert_currency` : 3 requis ; `translate` : enum `formality`), pièges thématiques (`weather_forecast`, `convert_currency`). 20 cas ajoutés, dont une catégorie `disambig`. `PLANNER_EVAL_CATALOG=basic` restaure l'ancien catalogue.

**Séparation des modèles, 2 runs × 42 cas (84 runs) :**

| modèle | exact (facile) | exact (dur) | tool-valid (dur) | disambig | none |
|---|---|---|---|---|---|
| qwen2.5:7b | 100 % | **95 %** | 100 % | 86 % | 86 % |
| llama3.1:8b | 86 % | **67 %** | **93 % — sous le seuil** | 43 % | 43 % |

La suite durcie sépare bien plus nettement (14 points d'écart → 28), et **llama3.1:8b passe sous le seuil `tool_valid ≥ 95 %`**. Elle a aussi de la marge : qwen n'est plus au plafond.

**Quatre enseignements :**

1. **Les distracteurs dégradent l'abstention, même pour un bon modèle.** Catégorie `none` de qwen : 100 % (catalogue trivial) → 75 % (catalogue riche) avant correction des cas. C'est l'effet observé sur le cluster, désormais quantifié : plus le réseau grandit, plus le sur-planning revient. Le plancher de pertinence (§5.4) n'est pas une optimisation, c'est un correctif.

2. **La règle « traduction / résumé → plan vide » du prompt de compile est fausse dès qu'une cap dédiée existe.** Deux cas `none` échouaient en choisissant `translate` / `summarize` — le modèle avait raison, l'attente était périmée. Le prompt code en dur une liste d'intentions à traiter sans outil, indépendamment du catalogue. **Bug de prompt à corriger** : la règle doit être conditionnelle à l'existence d'une cap correspondante.

3. **Les exemples positifs font le travail ; les exemples négatifs sont faibles.** `search_unknown_url` échouait de façon déterministe (`fetch_url` au lieu de `web_search`) alors que `fetch_url` portait un `negative_example` reprenant l'intention quasi mot pour mot. Ajouter **un** exemple positif « find the official website for X » sur `web_search` a corrigé le cas. **Règle d'écriture** : couvrir par des exemples positifs sur la bonne cap les formulations qu'on veut voir gagner ; ne pas compter sur un contre-exemple porté par la mauvaise cap.

4. **L'écriture d'exemples a des effets non locaux — et c'est le point le plus gênant.** Ce même ajout d'un exemple à `web_search` a fait basculer deux cas sans rapport (`rewrite_not_translate`, `fetch_known_url`), net 98 % → 95 %. Dans un réseau où des publishers indépendants rédigent leurs caps, **une modification chez l'un perturbe la sélection chez les autres**. Conséquence : toute retouche d'exemples doit passer par la suite d'éval ; on ne peut pas régler une cap isolément.

**Limite de la suite elle-même** : `expect_tools` fait une égalité exacte d'ensemble, ce qui punit des réponses défendables. `« Fetch <url> and tell me what it says »` → `{fetch_url, chat}` est raisonnable ; `« Rewrite this sentence »` → `{chat}` ou `{}` le sont tous les deux. Prochain correctif de la suite : accepter **plusieurs ensembles valides** par cas plutôt qu'un seul.

## 6quater. Correctifs findings 1 & 2 — appliqués et mesurés (2026-07-30)

### Ce qui a été changé

**Finding 2 — règle du prompt de compile rendue relative au catalogue.** L'ancienne règle énumérait des *types de tâche* (« traduction, définitions, arithmétique → plan vide ») indépendamment du catalogue, ce qui contredisait la règle voisine « une skill est PERTINENTE seulement si sa description correspond à l'intention » et rendait invisible toute cap dédiée à ces tâches. Remplacée par : utiliser une skill **dédiée** si elle existe, répondre directement sinon — avec une clause explicite qu'une skill de chat généraliste n'est *dédiée à rien* (sans quoi le modèle route « bonjour » vers `chat`).

**Finding 1 — caps locales classées et bornées (`LOCAL_TOP_K = 12`).** Elles contournaient totalement le classement et étaient non bornées : un opérateur avec douze skills les présentait toutes, à chaque message. Elles sont désormais scorées et bornées comme les distantes. Le filtre est extrait en `Catalog::filter_for_query`, appelé aussi par la suite d'éval — sinon on mesurait un chemin que personne n'exécute.

### Résultat (suite durcie, 2 runs × 42 cas)

| | qwen avant | **qwen après** | llama avant | llama après |
|---|---|---|---|---|
| tool-exact | 95 % | **98 %** | 67 % | 69 % |
| tool-valid | 100 % | **100 %** | 93 % *(sous seuil)* | **100 %** |
| disambig | 86 % | **100 %** | 43 % | 57 % |
| none | 86 % | 86 % | 43 % | 57 % |
| chain | 100 % | 100 % | 71 % | 57 % |

Seul échec restant sur qwen : `rewrite_not_translate`, un cas de test ambigu (`{chat}` et `{}` sont tous deux défendables pour « reformule cette phrase »), pas un défaut du planner.

**Hypothèse infirmée** : la liste codée en dur devait servir de béquille aux modèles faibles. Sa suppression a *amélioré* l'abstention de llama3.1:8b (`none` 43 % → 57 %). Ce n'était pas une béquille, c'était une contradiction.

### Résultat négatif important — pas de plancher de pertinence

Le plancher recommandé au §5.4 (« couper les caps sous X % du meilleur score ») a été **implémenté, mesuré, puis retiré**. Avec un retrieval purement lexical il transforme une faiblesse de *classement* (inoffensive : le modèle voit quand même la cap et la choisit correctement) en **perte de capacité silencieuse**. Deux mesures l'ont tué :

- `« …then summarise what you find. »` → `summarize` score 0.384, rel 0.17 → **coupé**
  `« …then summarize what you find. »` → `summarize` score 1.113, rel 0.49 → gardé
  **Une seule lettre** (orthographe britannique/américaine) décide si la capacité existe. Régression mesurée : catégorie `chain` 100 % → 86 %.
- `« Quelle heure est-il maintenant ? »` sur un catalogue anglais score **0.000 partout**. Produit EN/FR : une requête sur laquelle BM25 n'a aucun avis est un cas courant, pas exotique. Couper là viderait le catalogue et ferait perdre `time` à un francophone qui demande l'heure.

**Conclusion** : classer et borner est sûr (on ne retire une cap que si une autre la dépasse) ; couper sur un seuil absolu ne l'est pas. Le plancher ne redevient envisageable qu'**après** le retrieval sémantique (§5.2) — l'ordre des travaux §7 doit donc mettre 5.2 avant tout re-essai de plancher.

### Troisième confirmation de la non-localité

Trier systématiquement le catalogue (au lieu de ne trier que lorsqu'il faut élaguer) a fait échouer un cas `chain` sans rapport — `reverse` émis sans son argument requis `text` — au seul motif que **l'ordre des skills dans le prompt avait changé**. Corrigé en ne triant que si le groupe dépasse sa borne. Troisième occurrence du même phénomène après §6ter #4 : *toute* perturbation du prompt (exemples, ordre, formulation) a des effets non locaux et doit passer par la suite d'éval.

## 7. Ordre de travaux proposé

| # | Travail | Dépendances | Mesurable par |
|---|---|---|---|
| 1 | 5.1 grammaire énumérée par dispatch | aucune | suite d'éval existante (22 cas, `tool_valid`) |
| 2 | 5.3 retry avec erreur réinjectée | aucune | éval : taux de repli `reflect_only` |
| 3 | 5.4 classement des locales + budget contraignant | aucune | taille de prompt loggée, cas à gros catalogue à ajouter |
| 4 | 6.x cadrage par mention explicite | 5.1 (bénéfice), UI | nouveaux cas d'éval avec mention |
| 5 | 5.2 retrieval hybride | choix d'un modèle d'embedding | cas d'éval cross-lingues à ajouter |

1 à 3 sont des changements contenus dans `grammar.rs` / `plan_exec.rs` / `catalog.rs`, mesurables **avant** de s'engager sur plus gros. La suite d'éval existe déjà : chaque étape doit être prouvée, pas supposée.

## 8. Questions ouvertes remontées (non tranchées ici)

Deux questions du §11 de l'architecture sont directement touchées. Conformément à la règle « ne pas trancher silencieusement » :

**a. Granularité de la synapse (1:1 / 1:lobe / 1:capability).** Décider à quoi `@` se lie *est* cette question qui remonte. Trois cibles possibles pour une seule syntaxe : `@alias` (instance), `@lobe` (fédération), `@alias.cap` (capacité). Le choix contraint l'UI, la résolution, et potentiellement le fil si la portée devient une notion protocolaire.

**b. Position juridique sur les noms de marque (`@google`, `@adobe`).** L'exemple de travail est littéralement `@google`. Si les alias sont auto-déclarés **et** visibles globalement via `describe_self`, N3UR0N opère de fait un espace de noms squattable. S'ils sont des **petnames locaux** (§6.3a), il n'en opère aucun — ce qui désamorce la question au lieu de la trancher.

L'option petname est recommandée pour cette raison autant que pour la sécurité, **mais elle reste à valider explicitement** : c'est une prise de position, pas un détail d'implémentation.

**c. Hors §11, à trancher aussi** : ordre de résolution peer/lobe (§6.3b), et si le cadrage par lobe est exposé à l'utilisateur avant que le protocole d'appartenance v0.5 existe (§6.3e).

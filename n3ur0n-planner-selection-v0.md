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

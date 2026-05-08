# N3UR0N — Document d'architecture (draft 0)

*Statut : draft de travail. Reflète l'état des réflexions au 7 mai 2026. À affiner.*

**Amendement 2026-05-08** : §5.2 — la structure du message inclut désormais explicitement le champ `sender_public_key` à côté de `signature`. Le hash du `sender_public_key` doit correspondre à `sender_id` (auto-vérification). Sans ce champ sur le fil, le destinataire ne peut pas matérialiser la clé publique pour vérifier la signature en l'absence de registre id→pk. Le reste de §5.2 reste valide.

---

## 1. Préambule

Ce document consolide la conception de N3UR0N à l'issue de la phase de brainstorm. Il n'est pas une spécification figée : c'est un instantané qui sert de base de discussion et d'implémentation pour la version minimale viable (v0.1). Les décisions explicitement reportées sont marquées comme telles. Les compromis assumés sont nommés. Les questions encore ouvertes sont listées en fin de document.

Le document est volontairement opiniâtre : il choisit, plutôt que de présenter des options sans verdict. Les choix peuvent être révisés, mais ils ne sont pas dilués dans des conditionnels.

---

## 2. Vision

N3UR0N est un système distribué pour publier, découvrir, composer et invoquer des capacités d'intelligence artificielle déployées par des acteurs hétérogènes — individus, communautés, entreprises, institutions — sans autorité centrale qui contrôle l'accès aux capacités elles-mêmes.

L'unité de déploiement est l'**instance n3ur0n** : un logiciel qu'un opérateur installe sur sa propre infrastructure et qui agit comme passerelle entre une capacité IA backend (qu'il a déployée ou à laquelle il a accès) et le réseau global d'autres instances.

L'ambition de décentralisation porte sur la couche IA elle-même. Les couches d'infrastructure (annuaires, bootstrap, gouvernance des défauts) peuvent être centralisées de façon transparente et contestable, à condition que cette centralisation soit toujours optionnelle et fork-friendly.

Le différenciateur central n'est pas le protocole d'invocation (qui est largement résolu par MCP, OpenAPI, JSON-RPC), mais l'effet réseau pair-à-pair entre instances et l'expérience de navigation cartographique sémantique au-dessus.

---

## 3. Vocabulaire et taxonomie

La métaphore cérébrale qui a inspiré le nom du projet est conservée comme **registre de représentation et d'interface**, non comme prétention fonctionnelle. Le système ne fonctionne pas comme un cerveau ; il s'organise et se navigue à la manière d'une carte cérébrale. Cette précision est importante car elle évite des promesses biologiques que l'architecture ne peut pas honorer (transmission directe d'apprentissage, intégration unifiée, plasticité spontanée).

### Taxonomie corrigée

**Atome** — unité de capacité primitive, indivisible (ex. tokeniser un texte, calculer un embedding, détecter une langue). Brique de composition.

**Dendrite** — interface d'entrée typée d'une instance, par laquelle elle reçoit des invocations.

**Soma** — référence interne à la capacité IA backend que l'instance encapsule. Le soma n'est pas l'IA ; il en est la prise.

**Axone** — pipeline de sortie d'une instance, qui peut se ramifier vers plusieurs destinataires.

**Synapse** — connexion configurée et activée entre deux instances. Porte l'authentification, le format, la souscription éventuelle, les politiques de transformation. C'est l'unité atomique de la configuration d'un n3ur0n.

**N3ur0n (instance)** — unité déployable. Passerelle (gateway) sans intelligence propre. Ne fait que routage, identification, application de politique, médiation de format. Toute l'intelligence vit en aval, dans le backend que l'instance encapsule.

**Lobe** — fédération nommée d'instances partageant une catégorie de capacité, une affiliation, ou un usage commun. Plusieurs typologies (voir section 7).

**Faisceau** — chemin de routage privilégié entre clusters d'instances fortement couplés. Émergent, pas configuré.

**Glie** — infrastructure de support transversale (logs, métriques, observabilité, audit). Pas dans le scope v0.1.

### Décisions de vocabulaire

Le mot "neurone" sans préfixe désigne toujours l'instance n3ur0n, jamais une IA. Quand on parle de l'intelligence elle-même, on dit "backend" ou "capacité". Cette discipline lexicale évite la confusion classique où "neurone" oscille entre le matériel, le logiciel, et la fonction.

---

## 4. Architecture en couches

Le système est conceptuellement structuré en cinq couches, du plus interne au plus externe :

### 4.1 Backend IA

L'intelligence réelle. Modèles, inférence, sémantique, état. Hébergée par l'opérateur de l'instance. Le système N3UR0N ne prescrit aucun framework ni format pour cette couche : un backend peut être un LLM local, une API distante, un script Python, un cluster GPU, ou n'importe quoi d'autre. L'instance n3ur0n l'invoque via un adaptateur.

### 4.2 Instance n3ur0n

Couche passerelle. Sans intelligence propre. Responsabilités : exposer les capacités du backend selon le protocole standard, gérer l'identité cryptographique de l'instance, signer et vérifier les échanges, appliquer les politiques de souscription locales, maintenir le répertoire local de pairs connus, router les invocations entrantes vers le backend et les invocations sortantes vers d'autres instances.

L'instance n3ur0n est ce que l'opérateur installe. C'est aussi l'unité que les autres voient sur le réseau.

### 4.3 Identité et autorisation

Couche transversale. Toujours active. Comprend deux aspects orthogonaux :

*Identification cryptographique* — chaque message du protocole est signé par la clé privée de l'instance émettrice. Le destinataire vérifie la signature avant tout traitement. Sans signature valide, un message est rejeté en bloc, qu'on soit en mode libre ou restreint. Cette couche est non négociable et toujours présente.

*Autorisation utilisateur (souscription)* — couche optionnelle, à la discrétion du destinataire. Permet d'exiger qu'une relation (compte, abonnement, paiement) existe entre l'utilisateur appelant et l'opérateur appelé, indépendamment de la signature transport.

### 4.4 Lobe

Fédération d'instances. Pas une infrastructure spéciale ; une convention de coordination. Un lobe peut avoir un type (voir section 7), une politique d'admission, un éventuel répertoire d'index propre. Une instance peut appartenir à plusieurs lobes simultanément.

### 4.5 Surface utilisateur

UI/UX par laquelle l'utilisateur interagit avec son instance et, par effet de réseau, avec le cluster global. Inclut l'éditeur de prompt, le panneau de souscriptions actives, et — dans une version ultérieure — la visualisation cartographique sémantique. Pas dans le scope v0.1, sinon en version minimale CLI ou API REST.

---

## 5. Identité et signature

### 5.1 Identifiant d'instance

L'identifiant canonique d'une instance est le **hash SHA-256 de sa clé publique Ed25519**, encodé en base32 sans padding. Format proposé : `n3:` suivi de l'encodage. Exemple : `n3:abcd1234efgh5678...`.

Ce choix est dérivé : l'identifiant est calculable à partir de la clé publique, ce qui le rend auto-vérifiable et impossible à falsifier sans casser Ed25519. Aucun registre n'est nécessaire pour résoudre l'identifiant en clé publique.

Une instance peut déclarer un **alias humain** optionnel (par exemple `@alice` ou `@dreamers.collective`). Les alias n'ont pas de garantie d'unicité globale ni de durabilité ; ils sont attribués librement et peuvent entrer en conflit. La résolution alias → identifiant canonique passe par les répertoires (cf. section 8). Un appelant prudent vérifie systématiquement que l'identifiant canonique correspond à ce qu'il attendait, indépendamment de l'alias.

### 5.2 Format de message

Tout message du protocole, quel que soit le verbe, a la structure suivante :

- `sender_id` — identifiant canonique de l'émetteur
- `recipient_id` — identifiant canonique du destinataire attendu
- `timestamp` — instant d'émission, format ISO 8601 UTC
- `nonce` — chaîne aléatoire unique pour ce message
- `payload` — corps du message, dépend du verbe
- `signature` — signature Ed25519 de l'émetteur sur la concaténation canonique des cinq champs précédents

Le destinataire :

- Vérifie la signature avec la clé publique correspondant à `sender_id`.
- Vérifie que `recipient_id` correspond bien à son propre identifiant.
- Vérifie que `timestamp` est dans une fenêtre acceptable (recommandation : ±5 minutes).
- Vérifie que `nonce` n'a pas déjà été observé récemment (anti-replay, fenêtre suggérée : 1 heure).

Tout échec entraîne le rejet du message sans traitement.

### 5.3 Clés et rotation

À v0.1, une instance possède exactement une paire de clés. Elle est générée localement à l'installation et persistée en clair sur le disque de l'opérateur (chiffrement de la clé privée hors scope v0.1).

La rotation de clé n'est pas supportée à v0.1. Si l'opérateur perd ou révoque sa clé, son instance change d'identifiant canonique : pour le réseau, c'est une nouvelle instance. Les souscriptions liées à l'ancienne identité sont perdues. Cette limitation est documentée explicitement et constitue une dette à payer en v0.2.

Conséquence pratique : à v0.1, l'identité de l'opérateur humain et l'identité de l'instance sont confondues. Une seule clé pour les deux. Cette confusion sera levée plus tard avec un mécanisme d'identité utilisateur de plus haut niveau référençant plusieurs identifiants d'instance.

---

## 6. Autorisation et souscription

### 6.1 Mode libre vs mode restreint

Pour chaque capacité qu'elle expose, une instance déclare son **mode d'accès** :

- *Libre* — toute invocation correctement signée est acceptée, sans vérification de relation préalable.
- *Restreint* — l'invocation n'est acceptée que si l'identifiant de l'émetteur est dans la whitelist du destinataire, ou si le payload contient un `subscription_token` valide.

Le mode est déclaré par capacité, pas par instance. Une même instance peut exposer une capacité en mode libre et une autre en mode restreint.

Les trois opérations méta de découverte (cf. section 8) sont **toujours en mode libre**. La méta-couche est publique par construction.

### 6.2 Modèle économique

N3UR0N ne prescrit pas de modèle économique global. Chaque opérateur décide si ses capacités sont gratuites, payantes, freemium, sponsorisées. Les modes payants reposent sur la couche d'autorisation : pour invoquer une capacité payante, l'utilisateur doit avoir établi une relation commerciale avec l'opérateur de l'instance appelée.

Le mécanisme d'octroi des souscriptions est **hors du protocole v0.1**. Chaque opérateur conçoit son propre flow d'onboarding : formulaire web, intégration Stripe, validation manuelle, accord hors-ligne. Le protocole n'a besoin que de transporter le résultat — un `subscription_token` opaque dans le payload, dont le format est défini par chaque opérateur.

### 6.3 Anti-abus en mode libre

Le mode libre n'est pas synonyme d'abandon de contrôle. Puisque tout message est signé, une instance en mode libre peut :

- Identifier de façon certaine chaque appelant.
- Appliquer des rate-limits par identifiant d'appelant.
- Bannir dynamiquement des identifiants spécifiques.
- Logger toutes les invocations de manière auditable.
- Basculer en mode restreint en cas de pattern d'abus détecté.

Une capacité en mode libre est ouverte sans contrat préalable, mais pas sans vigilance.

### 6.4 Polymorphisme du verbe "joindre"

Un utilisateur peut "joindre" un lobe au sens de configurer son instance pour interagir avec lui. La sémantique précise dépend du type de lobe (cf. section 7) et du mode d'accès des capacités exposées :

- Pour un lobe communautaire à capacités libres, "joindre" est essentiellement une déclaration d'affiliation. L'instance s'enregistre auprès du répertoire du lobe et peut désormais router des invocations vers ses membres sans souscription supplémentaire.
- Pour un lobe à capacités restreintes ou payantes, "joindre" suppose en plus l'établissement de souscriptions via les flows hors-protocole de chaque opérateur.

Le mot "joindre" cache donc deux opérations à granularité différente. C'est accepté à v0.1 pour préserver l'élégance UX, mais documenté comme polymorphe.

---

## 7. Lobes et gouvernance

### 7.1 Typologie des lobes

Tous les lobes ne se gouvernent pas de la même façon. Quatre archétypes coexistent et doivent être typés explicitement :

**Lobe communautaire** (ex. `@dreamers`) — adhésion ouverte, identité construite par la pratique. Les capacités exposées sont typiquement libres. Gouvernance auto-organisée, normes implicites.

**Lobe attesté de marque** (ex. `@google`, `@adobe`) — adhésion contrôlée par l'entité titulaire de la marque ou son délégataire. L'attestation est cryptographique : seuls les opérateurs portant une signature valide de l'autorité du lobe sont reconnus comme membres légitimes. Les conflits de nommage avec des marques déposées sont à traiter explicitement (cf. section 11.3).

**Lobe fonctionnel** (ex. `@translation`, `@code-review`) — adhésion ouverte, mais conditionnée à la démonstration de la capacité (benchmark à l'admission ou évaluation continue). Pas d'autorité de marque ; la fonction définit l'admission.

**Lobe d'infrastructure ou canal** (ex. `@youtube` comme sink d'affichage) — moins une famille de capacités IA qu'un protocole d'intégration avec un service externe. Souvent passerelle vers un système non-N3UR0N.

À v0.1, **seuls les lobes communautaires sont supportés**. Les autres typologies relèvent de v0.2 et au-delà, parce qu'elles requièrent des mécanismes d'attestation et de gouvernance qui sont eux-mêmes des sous-projets.

### 7.2 Le lobe comme convention, pas comme infrastructure

Un lobe est défini par un nom, un type, et un répertoire de membres. Il n'a pas d'existence séparée des instances qui le composent. Le répertoire d'un lobe peut être maintenu par :

- Une instance n3ur0n spécialisée qui déclare la capacité d'indexation étendue (cf. section 8.4) et se présente comme registre du lobe.
- Plusieurs instances concurrentes qui se synchronisent par gossip (v0.2+).
- Une simple convention out-of-band (un fichier `.lobe.json` partagé) en l'absence de tout mécanisme automatique.

À v0.1, on retient la première option : un lobe a un n3ur0n-registre. Si plusieurs registres existent pour un même nom de lobe, c'est aux utilisateurs de choisir lequel ils consultent. Aucune autorité protocolaire ne tranche.

---

## 8. Découverte et indexation

### 8.1 Principe : l'indexation est intrinsèque

Chaque instance n3ur0n maintient un répertoire local de pairs connus. Cette capacité de répertoire est intrinsèque au protocole : tout n3ur0n y participe, indépendamment de sa capacité IA principale. Il n'y a pas de "registre" comme infrastructure séparée. Un registre est simplement un n3ur0n qui investit plus de ressources dans cette fonction.

### 8.2 Structure du répertoire local

Pour chaque pair connu, l'instance stocke :

- L'identifiant canonique (hash de clé publique).
- L'endpoint URL où le pair est joignable.
- L'alias humain s'il en a déclaré un.
- Une copie en cache de sa dernière `describe_self()`, avec timestamp.
- Le timestamp de la dernière interaction réussie.
- Optionnellement, la source de la découverte (par quel autre pair on l'a appris).

Taille du répertoire à v0.1 : plafonnée à 1000 entrées avec éviction LRU. Cohérence : éventuelle. Fraîcheur de la copie de `describe_self()` : TTL d'une heure, re-pull à la demande au-delà.

### 8.3 Trois opérations méta

Le protocole expose trois verbes universels, présents sur toute instance, toujours en mode libre côté autorisation :

**`describe_self()`** — retourne la fiche d'identité officielle de l'instance : identifiant canonique, endpoint, alias optionnel, version de protocole, timestamp de mise à jour, et liste des capacités exposées (avec pour chacune sa déclaration formelle, son mode d'accès, et éventuellement les paramètres tarifaires).

**`get_known_peers(limit, filter?)`** — retourne jusqu'à `limit` pairs du répertoire local. Le filtre optionnel permet de restreindre aux pairs déclarant une capacité donnée. Aucune garantie de fraîcheur ni d'exhaustivité.

**`ping()`** — répond avec un timestamp signé. Sert à mesurer la latence et à confirmer la liveness.

### 8.4 Capacité d'indexation étendue

Une instance peut, optionnellement, déclarer la capacité supplémentaire `extended_index`. Elle s'engage alors à maintenir un répertoire bien plus grand (ordre de 10⁵ entrées ou plus), à offrir une opération de requête plus riche (`query_index(criteria)` avec filtres composés sur capacité, lobe, alias, etc.), et à actualiser ces données activement.

Cette capacité ne confère aucun privilège protocolaire. Une instance d'indexation étendue est un n3ur0n comme un autre, qui se concurrence avec les autres instances offrant le même service. Elle peut être gratuite ou payante selon la décision de son opérateur.

### 8.5 Stratégie de propagation à v0.1

À v0.1, la propagation se fait **sur demande**, par cascade à profondeur 1 :

Quand l'instance a besoin d'une capacité non présente dans son répertoire local, elle interroge `get_known_peers(filter=capability)` sur 3 à 5 pairs choisis aléatoirement parmi ceux qu'elle connaît. Pour chaque résultat, elle pull `describe_self()` du candidat, vérifie la pertinence, l'ajoute à son répertoire local. Pas de récursion multi-hop à v0.1 ; pas de gossip périodique.

Cette stratégie est volontairement frugale. Elle suffit à démontrer l'effet réseau pour un cluster de quelques centaines à quelques milliers d'instances. Au-delà, une stratégie type DHT ou gossip structuré devient nécessaire, et c'est un sujet de v0.2 ou v1.

### 8.6 Bootstrap

Le bootstrap n'est pas un problème système. Chaque opérateur configure son instance avec un ou plusieurs pairs initiaux au moment du déploiement. Le binaire de référence ships avec une configuration par défaut pointant vers un n3ur0n de bootstrap maintenu par le projet, mais cette configuration est triviale à override. Aucun mécanisme particulier de bootstrap n'est dans le scope du protocole : c'est de la configuration locale.

---

## 9. Spécification du protocole v0.1

### 9.1 Transport

JSON sur HTTPS. Chaque instance expose un endpoint unique sous le path `/n3ur0n/v0`. Les méthodes du protocole sont des POST avec un corps JSON. Authentification mTLS optionnelle pour les déploiements en environnement contrôlé, non requise par défaut.

### 9.2 Format de message (rappel)

Tout corps de requête comme de réponse a la structure :

```
sender_id, recipient_id, timestamp, nonce, payload, signature
```

Encodé en JSON. La signature couvre la concaténation canonique des cinq champs précédents.

### 9.3 Verbes minimaux

- `describe_self`
- `get_known_peers`
- `ping`
- `invoke` — invocation d'une capacité du backend. Le payload contient le nom de la capacité ciblée, ses arguments selon le schéma déclaré, et optionnellement un `subscription_token`.

Aucun autre verbe n'est requis à v0.1. Toute fonction supplémentaire (gestion de souscriptions, gouvernance de lobe, paiement, etc.) est hors-protocole et passe par des canaux hors-bande définis par chaque opérateur.

### 9.4 Format de déclaration de capacité

Pour chaque capacité exposée, l'instance déclare dans son `describe_self()` :

- Un nom court unique au sein de l'instance.
- Un schéma d'entrée et un schéma de sortie au format JSON Schema.
- Une description en langage naturel pour les humains.
- Un mode d'accès (`free` ou `restricted`).
- Une indication tarifaire optionnelle (chaîne libre à v0.1, à structurer en v0.2).
- Un ou plusieurs tags pour la découverte.
- Optionnellement, l'identifiant du ou des lobes auxquels cette capacité est rattachée.

La compatibilité avec le format de tools de MCP est recherchée. Un mapping bidirectionnel doit pouvoir être implémenté trivialement, pour que les serveurs MCP existants puissent être encapsulés dans une instance n3ur0n sans réécriture.

---

## 10. Limites assumées de v0.1

Les compromis suivants sont délibérés et documentés. Ils ne sont pas des oublis ; ils sont les conditions de possibilité d'un livrable en quelques semaines.

**Aucune défense contre les attaques Sybil.** Un acteur malveillant peut déployer arbitrairement d'instances bidons et polluer les répertoires. À v0.1, on assume une communauté petite et coopérative. La défense relève de v0.2 (stake économique, proof-of-work léger, ou réputation distribuée).

**Aucune vérification des capacités déclarées.** Une instance qui prétend offrir une capacité peut renvoyer du charabia. Aucun benchmark, aucune évaluation. Quality est un problème de v0.2 (oracles d'évaluation, réputation par usage).

**Aucune confidentialité des métadonnées.** Toute instance peut énumérer le réseau via les opérations méta. Pas de mode "unlisted" à v0.1. Les métadonnées (qui expose quoi, qui parle à qui) sont publiques.

**Cohérence éventuelle uniquement.** Deux instances peuvent avoir des vues différentes du réseau. Aucun consensus, aucun quorum. Acceptation explicite de la divergence transitoire.

**Échelle limitée à environ 10⁴ instances actives.** La stratégie de découverte naïve craque au-delà. Pour v0.1, c'est un horizon largement suffisant.

**Pas de rotation de clé.** Perdre sa clé, c'est perdre son identité réseau et toutes ses souscriptions. Limitation à payer en v0.2.

**Pas de gestion d'état riche.** Une invocation est une requête-réponse. Pas de session, pas de streaming, pas de subscription temps-réel. Les capacités stateful doivent gérer leur propre état hors-protocole.

**Pas de pipelines orchestrés.** À v0.1, une invocation appelle une seule capacité. La composition multi-étapes (`ask X then ask Y then show with Z`) est entièrement de la responsabilité du client appelant — typiquement, le backend IA orchestrateur de l'utilisateur. Le protocole ne formalise pas la planification.

**Pas de visualisation cartographique cérébrale.** L'UX cartographique, qui est pourtant un différenciateur stratégique, est explicitement reportée. À v0.1, les opérations passent par CLI ou API REST. La couche visuelle est un projet à part qui consommera ce protocole.

---

## 11. Questions ouvertes à trancher avant le code

Les questions suivantes ne peuvent pas être différées au-delà du début de l'implémentation :

### 11.1 Granularité de la synapse

Quand un utilisateur "active une synapse" vers `@google`, à quoi souscrit-il exactement ?

- *Synapse 1:1* — vers une instance précise de l'opérateur Google. Clair, mais bound à un fournisseur.
- *Synapse 1:lobe* — vers le lobe `@google`, qui route ensuite. Redondance, mais facturation floue.
- *Synapse 1:capability* — vers une capacité fonctionnelle, plusieurs fournisseurs concurrent. Marketplace véritable, complexité maximale.

À v0.1, en l'absence de lobes attestés et de mécanisme de marché, la question se pose peu : une souscription est de facto 1:1. Mais le format du `subscription_token` doit être conçu pour ne pas exclure les options 1:lobe et 1:capability en v0.2.

### 11.2 Mécanisme anti-free-riding pour les lobes communautaires

Pour les capacités payantes, le free-riding est résolu par le péage. Pour les lobes communautaires à capacités libres, qu'est-ce qui empêche l'asymétrie consommation/contribution ?

Trois familles de réponse possibles :

- *Acceptation explicite* — l'éthos du don. Marche tant que les contributeurs trouvent une motivation non-monétaire.
- *Réputation/contribution mesurée* — un oracle d'évaluation suit les contributions et déprioritise les free-riders.
- *Crédits internes au lobe* — économie locale non-monétaire (karma).

Décision reportée à v0.2 ; chaque lobe communautaire choisira son régime.

### 11.3 Position juridique sur les noms de marques

Le namespace des lobes inclut-il librement les noms de marques (`@google`, `@adobe`) ? Trois positions :

- *Namespace officiel uniquement* — un lobe portant un nom de marque n'existe qu'avec attestation cryptographique de la marque. Plus propre légalement.
- *Namespace sans marques* — interdiction des noms de marques. Capacités similaires renommées (`@search.web`, `@image.generation`).
- *Namespace ouvert avec disclaimers* — n'importe qui peut créer un lobe avec un nom de marque, avec mention de non-affiliation.

Ce choix doit être fait avant tout lancement public, parce qu'il détermine la viabilité juridique du projet.

### 11.4 Localisation du planner

Quand un prompt utilisateur déclenche une invocation multi-étapes ("ask `@google` then ask `@adobe`"), quel composant fait le planning ?

- Le backend IA local de l'utilisateur (planner attaché au n3ur0n personnel).
- Une instance n3ur0n spécialisée déclarée comme planner.
- Un service centralisé hors du réseau N3UR0N.

À v0.1, cette question ne se pose pas formellement : le protocole ne planifie rien, c'est l'affaire du client. Mais en v0.2, dès que les pipelines deviennent un cas d'usage central, il faudra trancher où vit cette intelligence et qui la rémunère.

### 11.5 Modèle économique du registre par défaut

Le n3ur0n de bootstrap maintenu par le projet est un point de centralisation pratique. Trois options de gouvernance :

- *Maintenu par le promoteur initial* sans gouvernance formelle. Risque de capture.
- *Gouverné par une fondation ou un collectif* défini avant le lancement.
- *Multi-registres concurrents* dès le départ, avec sélection par l'utilisateur lors de l'installation.

Le choix doit être public dès la première version distribuée.

---

## 12. Trajectoire indicative

**v0.1** — Protocole minimal décrit dans ce document. Cluster jusqu'à ~1000 instances. Communauté restreinte et coopérative. CLI/API REST seulement.

**v0.2** — Lobes typés (communautaire, fonctionnel attesté). Mécanismes anti-free-riding par lobe. Gossip périodique léger. Rotation de clé. Souscriptions payantes structurées.

**v0.3** — Découverte distribuée type DHT. Identité utilisateur découplée de l'identité d'instance. Premiers paiements intégrés. Confidentialité partielle (instances unlisted).

**v1.0** — Visualisation cartographique sémantique (l'UX cérébrale). Marketplace de capacités avec marché concurrentiel sur les services fonctionnels. Gouvernance distribuée des lobes. Sécurité durcie contre Sybil et empoisonnement.

Cette trajectoire est indicative et chaque jalon doit être validé par l'usage réel avant d'avancer au suivant. La tentation principale à éviter : sur-spécifier les versions futures avant que la précédente ne tourne réellement.

---

## 13. Méta — comment lire et faire évoluer ce document

Ce document est un draft. Il a deux fonctions :

D'abord, **figer les décisions prises** pour qu'elles ne soient pas reperdues à chaque conversation. Les sections 3 à 9 sont des décisions, pas des propositions. Si elles changent, c'est une modification consciente, datée, justifiée.

Ensuite, **rendre les compromis et reports visibles**. Les sections 10 et 11 sont la honnête comptabilité de ce qu'on n'a pas résolu. Elles existent pour que personne — ni l'auteur, ni un lecteur futur — ne soit surpris par une lacune cachée.

Toute évolution du document doit préserver cette structure : décisions fermes d'un côté, dettes assumées de l'autre. Glisser une dette dans une section de décision, c'est mentir à soi-même. Glisser une décision floue dans une section de dette, c'est procrastiner.

Le document grandit par couches. À mesure que les questions ouvertes se tranchent, leurs réponses migrent vers les sections de décision et la dette s'efface. Si une décision passée se révèle mauvaise, elle doit être contestée et revue dans le document, pas implicitement abandonnée dans le code.

---

*Fin du draft 0.*

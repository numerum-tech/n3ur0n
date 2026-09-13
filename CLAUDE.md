# OpenWolf

This project uses OpenWolf for context management. The always-on rules live in `.claude/rules/openwolf.md`; the hooks handle bookkeeping (anatomy index, memory log, read tracking) automatically.

For the full operating protocol (session handoff, memory discipline, bug logging), load the `openwolf` skill, or read `.wolf/OPENWOLF.md`. Regenerate the session handoff with `/handoff`.


# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## État du dépôt (mis à jour 2026-07-28)

Releases : **0.1.0** (protocole initial), **0.2.0** (planner v2, BM25, GBNF, cascade), **0.3.0** (manifestes TOML, hot-reload caps, master-detail UI), **0.4.0** (open source, i18n EN/FR, RBAC phase 1, composer tous bindings, backend hot-reload), **0.4.2** (précision planner + suite d'éval, `version` dans `/health`, nommage des assets de release ; absorbe aussi ce qui avait shippé sans version en 0.4.1 : mode chat direct, couche blob, id tronqué à 20 octets, durcissement `OpenAIBackend`). Version workspace = `0.4.2` ; `protocol_version` fil = `n3ur0n/0.3` (inchangé). Cf. [CHANGELOG.md](CHANGELOG.md) et [ROADMAP.md](ROADMAP.md).

**Note d'écart 2026-07-28 — frontend.** Le dossier `frontend/` (scaffold SvelteKit du commit initial `1453c6b`) était **mort** : jamais buildé par la CI, jamais servi, 214 LOC de boilerplate dont la page d'accueil disait elle-même « pre-implementation scaffold ». Supprimé. L'UI web réelle est et reste `crates/server/ui/` — **JS vanilla en ES modules + CSS, sans bundler ni package.json** (~4,3k LOC : `app.js` 3500, `auth.js`, `i18n.js`, `icons.js`, `index.html`, `style.css`, `locales/{en,fr}.json`). Elle est embarquée par rust-embed (`#[folder = "ui/"]`, [http.rs](crates/server/src/http.rs)) et sert **aussi** le desktop (`frontendDist: "../server/ui"` dans `crates/desktop/tauri.conf.json`). Les mentions de SvelteKit / `adapter-static` / `bits-ui` / `pnpm --filter frontend` dans les docs antérieures sont **fausses** ; corrigées ici et par la note du même jour en tête de [project-tech-stack.md](project-tech-stack.md).

Documents de spec et de référence :

- [n3ur0n-architecture-v0.md](n3ur0n-architecture-v0.md) — architecture v0.1 + amendements 2026-05-08 (sender_public_key) et 2026-05-12 (AccessMode ternaire, CapabilityDecl v0.2, planner local).
- [project-tech-stack.md](project-tech-stack.md) — stack technique v0.1 + amendement 2026-05-12 (crate `node`, backend runtime-instantiation, bindings v0.3, hot-reload ArcSwap).
- [n3ur0n-capability-manifest-v0.md](n3ur0n-capability-manifest-v0.md) — brouillon de format de manifeste (spec conceptuelle ; le split caps/backends effectif diverge, voir note d'écart en tête du doc).
- [n3ur0n-blob-protocol-v0.md](n3ur0n-blob-protocol-v0.md) — spec blob protocol (endpoint `/n3ur0n/v0/blobs`, classes A–D, panneau Files utilisateur ; amendement 2026-06-04 inclus). **Implémentée 2026-06-05** : `core/blob.rs`, `server/{blobs,blob_gc,files_api}.rs`, `node/{blob_client,blob_resolve}.rs`, verbe `blob_ticket`, attachments dans `UserInput`.
- [n3ur0n-blob-lifecycle-v0.md](n3ur0n-blob-lifecycle-v0.md) — **référence, 2026-09-13, en anglais** : cycle de vie complet d'un fichier entre deux nœuds (classes A–D, tickets, quotas, TTL, GC, contrôle d'accès), avec les écarts connus entre la spec et le code.
- [n3ur0n-direct-chat-v0.md](n3ur0n-direct-chat-v0.md) — mode chat direct (API locale + UI ; un appel LLM/message, toggle auto/direct).
- [n3ur0n-planner-selection-v0.md](n3ur0n-planner-selection-v0.md) — **brainstorm ouvert 2026-07-28**, rien d'acté : refonte de la sélection d'outils (grammaire énumérée par dispatch, retrieval hybride, retry sur erreur de validation, classement des caps locales) + couche de cadrage par mention explicite `@` et par lobe. Remonte deux questions ouvertes §11 archi (granularité de la synapse, noms de marque).
- [n3ur0n-planner-recommendations-v0.md](n3ur0n-planner-recommendations-v0.md) — recommandations planner v0.1→v0.2 (largement absorbées par 0.2.0/0.3.0, conservé comme trace de raisonnement).
- [n3ur0n-planner-brainstorm.md](n3ur0n-planner-brainstorm.md) — brainstorm initial planner (référence pour comprendre les choix v0.2).

**Règle de précédence** : si un détail de stack contredit l'architecture, l'architecture prime ; le doc de stack est mis à jour pour refléter le compromis (cf. `project-tech-stack.md` §17). Si une décision implémentation contredit un doc, le **code prime** sur les docs — les écarts sont consignés par notes datées en tête des docs concernés.

Avant tout travail d'implémentation, lire au minimum architecture + stack + leurs amendements 2026-05-12. Les sections "Limites assumées" (§10 archi) et "Limites et reports" (§16 stack) listent les compromis explicites de v0.1 ; ne pas réintroduire de fonctionnalité sans note datée.

## Ce que N3UR0N est (et n'est pas)

Système distribué pair-à-pair pour publier et invoquer des **capacités d'IA** sans autorité centrale. L'unité déployée est l'**instance n3ur0n** : passerelle (gateway) sans intelligence propre, qui encapsule un backend IA (LLM local, API, MCP server, script…) et l'expose au réseau.

**N'est pas** un cerveau, ni un protocole d'inférence, ni un nouveau format de tools. Le différenciateur est l'effet réseau pair-à-pair entre instances + UX cartographique sémantique (UX cartographique reportée post-v0.1).

### Discipline lexicale (impose-la dans le code et les commentaires)

- "neurone" / "n3ur0n" = **instance gateway**, jamais une IA.
- "backend" / "capacité" = l'intelligence elle-même.
- Vocabulaire métier : *atome, dendrite, soma, axone, synapse, lobe, faisceau, glie* — voir archi §3. Sont des **registres de représentation**, pas des prétentions biologiques.

## Architecture en couches (résumé)

1. **Backend IA** — externe au protocole, branché via adaptateur.
2. **Instance n3ur0n** — gateway : routage, identité crypto, signature, politique de souscription, répertoire local.
3. **Identité & autorisation** — Ed25519 par message (non négociable) + souscription optionnelle au choix du destinataire.
4. **Lobe** — fédération nommée d'instances. v0.1 : seuls les lobes **communautaires** sont supportés.
5. **Surface utilisateur** — CLI / API REST + UI web statique (une seule codebase `crates/server/ui/`, servie soit par le shell desktop Tauri, soit par le binaire serveur).

## Invariants protocolaires non négociables

- Identifiant canonique = `n3:` + Base32(**20 premiers octets** de SHA-256(clé publique Ed25519)). Auto-vérifiable, pas de registre requis. **Amendement 2026-07-18** : le hash est tronqué à 20 octets (160 bits → 32 chars Base32 ; avant : 32 octets → 52 chars) pour raccourcir l'id. Troncature sur les **octets du hash**, pas sur la chaîne Base32 (`ID_HASH_BYTES` dans `core/identity.rs`). Collision ~2^80, seconde-préimage ~2^160. Pas de bump `protocol_version` (aucun réseau déployé au moment du changement).
- Tout message porte `sender_id, recipient_id, timestamp, nonce, verb, payload, sender_public_key, signature`. La **signature** couvre le **JCS** (RFC 8785, `serde_jcs`) de l'envelope (tout sauf `signature`). Le champ `sender_public_key` accompagne le message sur le fil ; le destinataire vérifie `hash(sender_public_key) == sender_id` puis utilise cette clé pour vérifier la signature. (Voir amendement 2026-05-08 dans `n3ur0n-architecture-v0.md`.)
- **Sans canonicalisation, les signatures divergent silencieusement** — ne jamais sérialiser à la main pour signer.
- Vérifications obligatoires côté destinataire : binding pk↔id, signature, `recipient_id`, fenêtre timestamp ±5 min, anti-replay nonce sur 1h.
- Trois verbes méta (`describe_self`, `get_known_peers`, `ping`) sont **toujours en mode libre**. Ne jamais restreindre.
- Quatrième verbe : `invoke`. Pas de session, pas de streaming protocolaire, pas de pipeline orchestré côté protocole. **Amendement 2026-06-05** : cinquième verbe `blob_ticket` (couche blob), autorisé **uniquement** sur `/n3ur0n/v0/blobs` — jamais dispatché via `/messages` (cf. spec blob §1).
- Mode d'accès (`free` / `restricted`) déclaré **par capacité**, pas par instance.

## Asymétrie consumer / publisher (structurante pour le stack)

- **Consumer** = client final. Aucun listener public. Shell desktop Tauri 2. Pas de cert TLS, pas de NAT à traverser.
- **Publisher** = opérateur exposant des capacités. Listener `/n3ur0n/v0` HTTPS obligatoire. Headless typiquement (VPS, homelab).
- Même core Rust pour les deux modes ; ils diffèrent par le **shell** (Tauri vs axum + rust-embed) et la **config par défaut** du listener.
- Un consumer peut basculer en publisher hybride via toggle UI ("Publish to network") qui démarre le listener axum dans le même process Tauri.

## Layout actuel (workspace cargo)

```
n3ur0n/
├── Cargo.toml                 # workspace + lints workspace-wide
├── crates/
│   ├── core/                  # lib : protocole, crypto, types — AUCUNE dép HTTP/SQL
│   │   ├── identity.rs        # InstanceId, Keypair, PublicKey
│   │   ├── message.rs         # Envelope, SignedMessage (avec sender_public_key)
│   │   ├── verify.rs          # verify_envelope (pure, Clock injectable)
│   │   ├── protocol.rs        # payloads typés des verbes
│   │   ├── capability.rs      # CapabilityDecl, AccessMode
│   │   ├── blob.rs            # BlobRef, classes A–D, tickets
│   │   └── error.rs
│   ├── adapters/              # lib : trait Backend + EchoBackend + OpenAIBackend
│   ├── storage/               # lib : SQLite + r2d2, repos peers + nonces (+ index blobs)
│   │   └── migrations/        # SQL versionné via schema_version table
│   ├── node/                  # lib : orchestration runtime
│   │   ├── identity_file.rs   # load/save keys.json (0600)
│   │   ├── registry.rs        # CapabilityRegistry
│   │   ├── node.rs            # Node (keypair + db + backend + registry + clock)
│   │   ├── handler.rs         # handle_request : verify → anti-replay → dispatch
│   │   ├── planner/           # PlanExecPlanner, DirectChatPlanner, catalog, retrieval BM25
│   │   │                      #   NB : l'exécuteur de plan vit dans planner/plan.rs
│   │   └── blob_client.rs / blob_resolve.rs  # upload/download + résolution refs blob
│   ├── server/                # lib + bin : axum + clap (publisher)
│   │   ├── lib.rs             # http::app(node), bootstrap
│   │   ├── http.rs            # /n3ur0n/v0/messages + /api/v0
│   │   ├── blobs.rs / blob_gc.rs / files_api.rs  # endpoint blobs + GC + panneau Files
│   │   ├── bootstrap.rs       # config dirs, load_node, create_identity
│   │   ├── cli.rs             # init / serve / keys
│   │   └── main.rs
│       ├── ui/                # UI web : JS vanilla ES modules + CSS, AUCUN build step
│       │                      #   app.js, auth.js, i18n.js, icons.js, index.html,
│       │                      #   style.css, locales/{en,fr}.json — embarqués par
│       │                      #   rust-embed (`#[folder = "ui/"]`, http.rs) et servis
│       │                      #   aussi au desktop (tauri.conf.json `frontendDist`).
│   └── desktop/               # shell Tauri 2 (profil consumer, scaffold 0.4.0)
└── .gitignore                 # ignore /target, runtime files, secrets
```

**Discipline de dépendances (à respecter strictement)** :

| Crate | Peut dépendre de | Ne doit JAMAIS dépendre de |
|---|---|---|
| `core` | serde, crypto, time | HTTP, SQL, IO du système |
| `storage` | core, rusqlite, r2d2 | HTTP, axum |
| `adapters` | core, reqwest | SQL, axum |
| `node` | core, storage, adapters, tokio | axum, clap, tauri |
| `server` | tout ce qui précède + axum, clap | tauri |
| `desktop` (à venir) | tout ce qui précède + tauri | axum |

Si `core` veut importer `axum` ou `rusqlite`, c'est une erreur de couche.

**Lints workspace** : `unsafe_code = "forbid"`, `unreachable_pub = "warn"`, `missing_debug_implementations = "warn"`, `clippy::all = "warn"`. Hérités via `[lints]\nworkspace = true` dans chaque `Cargo.toml` de crate.

## Choix de stack à respecter

| Domaine | Choix | Refus explicites (ne pas réintroduire sans discussion) |
|---|---|---|
| Langage core | Rust stable edition 2024 | Node, Python, Go |
| Async | `tokio` (full) | `async-std`, `smol` |
| HTTP serveur | `axum` 0.7+ | `actix`, `warp`, `rocket` |
| HTTP client | `reqwest` rustls | OpenSSL backend |
| TLS | `rustls` | OpenSSL |
| Crypto | `ed25519-dalek` 2.x, `sha2` | autres impls Ed25519 |
| JSON canonique | `serde_jcs` | sérialisation maison |
| Stockage | `rusqlite` bundled | Postgres, MySQL, Redis (services externes interdits v0.1) |
| Frontend | JS vanilla (ES modules) + CSS, servi statiquement depuis `crates/server/ui/`, aucun bundler — voir note 2026-07-28 ci-dessous | React/Next, Vue/Nuxt (SSR incompatible avec embed statique) ; réintroduire un framework + build step demande une décision explicite |
| Shell desktop | Tauri 2.x | Electron, Wails, Neutralino |
| CLI | `clap` v4 derive | structopt, argh |
| Logs | `tracing` + `tracing-subscriber` | `log` direct, `slog` |

## Commandes (validées sur scaffold actuel)

Workspace Rust opérationnel : `cargo check --workspace`, `cargo test --workspace` passent.

### CLI publisher

```bash
n3ur0n init                            # genère keys.json (0600) + sqlite
n3ur0n serve --port 4242 --endpoint http://... [--bootstrap http://peer1:4242 --bootstrap http://peer2:4242] \
   [--lobe medical --lobe legal-fr] \
   [--backend echo|openai|ollama] [--openai-base-url URL] [--openai-model NAME] [--openai-api-key TOKEN]
n3ur0n keys                            # affiche instance_id
n3ur0n send --endpoint http://node-b:4242 --verb ping
n3ur0n send --endpoint http://node-b:4242 --verb invoke \
   --payload '{"capability":"echo","args":{"x":1}}'

# Peer directory
n3ur0n peers list
n3ur0n peers refresh --endpoint http://node-b:4242    # signed describe_self → upsert
n3ur0n peers discover --capability echo               # cascade depth-1, random fan-out 5
```

`--config-dir` lu via flag OU env `N3UR0N_CONFIG_DIR`. `--bootstrap` lu via flag répété OU env `N3UR0N_BOOTSTRAP_PEERS` (CSV). `--lobe` lu via flag répété OU env `N3UR0N_LOBES` (CSV), sinon `instance.toml` ; modifiable à chaud par `PUT /api/v0/settings/lobes` (cf. archi §9.3bis : `cap.lobe_ids ⊆ instance.lobe_ids`, 5 lobes max, appartenance **non vérifiée**). Backend args lus aussi via env `N3UR0N_BACKEND`, `N3UR0N_OPENAI_BASE_URL`, `N3UR0N_OPENAI_MODEL`, `N3UR0N_OPENAI_API_KEY`.

### Backends

- `echo` (défaut) : retour identité de `args`. Tests + smoke.
- `openai` / `ollama` : `OpenAIBackend` (crates/adapters/src/openai.rs). Couvre OpenAI, Ollama, llama.cpp server, vLLM. Capability unique `chat` :
  - input : soit `{prompt: "..."}`, soit `{messages: [{role, content}, ...], temperature?, max_tokens?, model?}`
  - output : `{model, message: {role, content}, finish_reason}`
  - alias `--backend ollama` = `--backend openai` + base_url default `http://localhost:11434`
  - bearer token optionnel via `--openai-api-key` / env `N3UR0N_OPENAI_API_KEY`
  - streaming pas supporté v0.1 (force `stream:false` côté upstream)

Smoke validé : `n3ur0n serve --backend ollama --openai-model qwen2.5:0.5b` puis `n3ur0n send --verb invoke --payload '{"capability":"chat","args":{"prompt":"..."}}'` → réponse LLM signée bout-en-bout.

### Cluster Docker (test)

```bash
docker compose -f docker/compose.yml up -d --build
bash docker/cluster-smoke.sh        # 6 pings + describe_self + invoke
# Transfert de blob réel sur le fil (ignoré par défaut, exige le cluster) :
cargo test -p n3ur0n-node --test cluster_blob_transfer -- --ignored --nocapture
docker compose -f docker/compose.yml down -v
```

3 nodes (`node-a`/`node-b`/`node-c`) sur ports hôte 4242/4243/4244, réseau bridge interne `n3uronnet` (**sous-réseau figé `172.28.42.0/24`, dernier octet = port publié** : node-a `.42`/4242, node-b `.43`/4243, …). Sans épinglage, Docker attribue les adresses dans l'ordre de démarrage : recréer un conteneur redistribue les autres, et un nom mis en cache par un navigateur pointe alors vers un autre nœud. Pour savoir à quel nœud une page appartient, lire l'**identifiant d'instance** dans l'en-tête de l'UI, pas le nom d'hôte. Volumes par nœud. Healthcheck via `/n3ur0n/v0/health` (renvoie `{status, instance_id, protocol_version}`).

- **Répartition des capacités** (aucun nœud n'a tout, un n'a rien, un annonce ce qu'il ne peut pas servir) : `node-a` `time`+`random_int`, `node-b` `rename_file`+`reverse`+`string_length`, `node-c` `chat` (Ollama hôte, fonctionne), `node-d` `chat` (LLM LAN volontairement injoignable), `node-e` rien.
- `N3UR0N_ALIAS` / `--alias` : nom lisible annoncé dans `describe_self` (cluster : `seed`, `toolbox`, `brain-local`, `brain-lan`, `consumer`). **Étiquette, pas adresse** — `@peer:` ne résout que par `n3:` id, car un alias est une affirmation qu'un nœud fait sur lui-même et irait au premier squatteur. `@peer:self` désigne l'instance courante.
- `N3UR0N_CAPS` (CSV) restreint ce qu'un nœud publie parmi ce que son backend déclare. Un backend compilé est tout-ou-rien : sans ce filtre, deux nœuds sur le même backend sont deux publieurs identiques et le réseau n'a jamais à choisir.
- `node-c` : backend Ollama via `host.docker.internal:11434` (host Ollama réutilisé via `extra_hosts: host-gateway`). Modèle override par env `OLLAMA_MODEL` (défaut `qwen2.5:0.5b`), base URL override par `OLLAMA_BASE_URL`.
- `node-b` bootstrappe automatiquement depuis `node-a` (env `N3UR0N_BOOTSTRAP_PEERS`). Idem `node-c`, `node-d`, `node-e` : **node-a est le seed**, il ne bootstrappe depuis personne.
- `node-e` n'a **volontairement aucune capacité propre** (profil consumer) : son dossier de manifestes est vide par défaut. Ne pas le « réparer ».
- La découverte est **automatique au démarrage** (walk transitif depuis le seed) et **passive à la première rencontre** d'un pair inconnu (reverse-announce + `describe_self` en tâche de fond). Elle **ne se rejoue pas** ensuite : le descripteur d'un pair déjà connu reste figé jusqu'à un `POST /api/v0/peers/refresh` — c'est ce que fait le bouton de rafraîchissement des Compétences.

### Capacity planner (v0.2 — PlanExec, mis à jour 2026-05-12)

Le user **dialogue uniquement avec son instance**. L'instance compile un plan typé (DAG de steps avec refs `${...}`), l'exécute au-dessus des capacités du réseau, puis synthétise la réponse (reflect). **Deux appels LLM par dispatch** (compile + reflect — les steps peuvent eux-mêmes invoquer des caps LLM, mais ce sont des invokes réseau, pas des appels du planner), pas une boucle ReAct. Exécution **parallèle bornée** : tout step dont les dépendances sont satisfaites part concurremment, plafonné par `MAX_CONCURRENT_STEPS=4` (`planner/plan.rs`). Streaming SSE des événements de dispatch (`PlanReady`, `StepStart/StepDone`, `LowConfidence`, `Reflecting`, `Final`).

Impl : `PlanExecPlanner` dans `crates/node/src/planner/plan_exec.rs`. Le trait `PlanCompiler` permet l'escalade vers un peer remote exposant la cap `plan` (cascade). Constrained decoding GBNF / JSON Schema activé quand le backend le supporte (`crates/node/src/planner/grammar.rs`). Retrieval BM25 sur le catalogue avant compile (`crates/node/src/planner/retrieval.rs`).

`LLMPlanner` (ReAct boucle, présent en 0.1.0) **a été supprimé** en 0.2.0 — toute référence dans des docs anciennes est obsolète.

**Modèle planner : `qwen2.5:7b` (défaut depuis 2026-07-29, mesuré).** Bake-off sur la suite d'éval (`scripts/planner-eval.sh <modèle> <runs>`) : qwen2.5:7b 95 % tool-exact sur la suite durcie contre 67 % pour llama3.1:8b, qui passe *sous* le seuil `tool_valid ≥ 95 %`. **7B est le plancher praticable** — qwen2.5:3b 68 %, llama3.2:3b 50 %, qwen2.5:0.5b 41 % sur la suite d'origine. Ne pas revenir à llama3.1:8b : son mode d'échec dominant est le sur-planning (il invoque un outil là où il faut répondre directement). Détail et méthode : [n3ur0n-planner-selection-v0.md](n3ur0n-planner-selection-v0.md) §6bis/§6ter.

Cf `n3ur0n-planner-brainstorm.md` pour le brainstorm complet (3 modes, 4 niveaux, limites assumées).

**Flow runtime** :
```
browser → /api/v0/conversations/:id/messages {message}
       → middleware client_id (cookie)
       → ownership check (404 sinon)
       → conv_lock[id] mutex (sérialise même conv)
       → planner_slots semaphore (limite parallèle LLM)
       → load ConversationState (cache LRU OR SQLite)
       → planner.dispatch (compile → execute DAG (parallèle borné) → reflect)
       → persist chaque turn (transaction atomique)
       → return {reply, trace, model}
```

**Limites configurables** (env / CLI) :
- `N3UR0N_PLANNER_MODE=llm|none`
- `N3UR0N_PLANNER_LLM_BASE_URL`, `N3UR0N_PLANNER_LLM_MODEL`, `N3UR0N_PLANNER_LLM_API_KEY`
- `N3UR0N_MAX_CONCURRENT_PLANNERS=4`
- `N3UR0N_MAX_ACTIVE_CONVERSATIONS=50`
- `MAX_CONTEXT_TURNS=16` (constante code). `MAX_TOOL_TURNS` n'a plus de sens avec `PlanExecPlanner` (plan compilé en un coup, pas de boucle ReAct) — la limite équivalente est `MAX_PLAN_STEPS` (défaut 8, configurable).

**Conversations API** (cookie `n3ur0n_client_id` pour isolation, généré server-side) :
| Route | Rôle |
|---|---|
| `POST /api/v0/conversations` | Créer (returns id) |
| `GET /api/v0/conversations` | Liste filtrée par client_id |
| `GET /api/v0/conversations/:id` | Détail + turns (404 si pas owner) |
| `PATCH /api/v0/conversations/:id` | Rename |
| `DELETE /api/v0/conversations/:id` | Cascade delete |
| `POST /api/v0/conversations/:id/messages` | Dispatch via planner. Retourne `{reply, model, trace}`. 503 si pas de planner. |

### Web chat UI (browser → planner → peers)

`http://localhost:4242/ui/`. Layout : sidebar conversations + main pane composer. Pas de dropdown peer/cap par défaut — le planner décide. Tool calls/results sont visibles en bulles `tool` collapsibles.

**Routes legacy / advanced** (manual mode) :
- `POST /api/v0/chat {peer_endpoint, prompt|messages}` → signed invoke direct
- `POST /api/v0/invoke {peer_endpoint, capability, args}` → signed invoke générique
- `POST /api/v0/peers/refresh|discover` → directory ops

Local API (non signée, loopback-only en prod) :

| Route | Méthode | Rôle |
|---|---|---|
| `/api/v0/peers` | GET | Liste répertoire local + caps mises en cache |
| `/api/v0/chat` | POST `{peer_endpoint, prompt, model?}` | Proxy signé `invoke chat` vers le peer |
| `/api/v0/whoami` | GET | `{instance_id}` |
| `/api/v0/health` | GET | `{status: ok}` |
| `/api/v0/files` | GET/POST/DELETE | Panneau Files : upload + listing des blobs visibles (classes A/B/D) |

Couche blob réseau : `PUT/GET/HEAD/DELETE /n3ur0n/v0/blobs/*hash` autorisés par verbe signé `blob_ticket` ; GC périodique (`blob_gc.rs`) ; côté local `/api/v0/files`, `/api/v0/files/*hash`, `/api/v0/cap-jobs/blobs`. Spec : `n3ur0n-blob-protocol-v0.md`.

Smoke script (`bash docker/cluster-smoke.sh`) couvre : healthchecks, 6 pings croisés signés, describe_self, invoke chat signé a→c, bootstrap b←a, cascade depth-1 a→b→c sur capacité `chat`, **et le chemin browser** via POST `/api/v0/chat` sur node-a.

```bash
# Workspace Rust
cargo build --release -p n3ur0n-server
cargo test                                   # cible 80%+ sur core
cargo run -p n3ur0n-server -- serve

# Frontend — AUCUNE commande de build, aucun bundler, aucun package.json.
#   Les assets de crates/server/ui/ sont pris par rust-embed. En build DEBUG
#   rust-embed les relit sur disque à chaque requête : éditer un .js/.css puis
#   recharger la page suffit, pas de recompilation (vérifié 2026-07-28).
#   En build RELEASE ils sont embarqués dans le binaire → recompiler.

# Desktop Tauri (pas de package.json racine ; tout passe par cargo)
cargo run   -p n3ur0n-desktop                # dev
cargo build -p n3ur0n-desktop --release      # binaire nu
cargo install tauri-cli@^2 && cargo tauri build   # bundles .dmg/.msi/.AppImage/.deb
#   En CI c'est tauri-action avec projectPath: crates/desktop
#   (cf. .github/workflows/release.yml). Voir crates/desktop/README.md.

# CLI publisher
n3ur0n init                                  # paire de clés + config + SQLite
n3ur0n serve
n3ur0n peers list|add|remove|ping
n3ur0n invoke <peer> <capability> [--args ...]
```

## Test plan minimal (cible v0.1)

- Unit `cargo test` — focus `core` (signature, anti-replay, JCS, parsing).
- `proptest` — round-trip signature/vérif, idempotence JCS, déterminisme anti-replay.
- Intégration : compose 3 instances + 1 backend mock. Scénarios obligatoires :
  - Découverte par cascade profondeur 1.
  - Invocation libre / restreinte.
  - Replay rejeté.
  - Cert auto-signé TOFU.
  - Consumer (sans listener) → publisher : aller-retour propre.
- `cargo-fuzz` sur le parser de message, parser JCS, decoder Base32.
- Playwright (web) + tauri-driver (desktop) — smoke test sur chat / peers / config.
  En attendant, `bash scripts/ui-smoke.sh` pilote l'UI embarquée dans un vrai Chrome
  via CDP (aucune dépendance npm : Node 22 a `WebSocket`, Chrome parle le protocole),
  assert sur le DOM **et** capture des PNG dans `target/ui-smoke/`. Il démarre le
  binaire **debug** exprès : en release les assets sont figés au build, la page servie
  serait périmée.

## Questions ouvertes bloquantes (archi §11)

À trancher **avant lancement public**, pas avant le code :

- Granularité de la synapse (1:1 vs 1:lobe vs 1:capability).
- Mécanisme anti-free-riding pour lobes communautaires.
- Position juridique sur les noms de marques (`@google`, `@adobe`).
- Localisation du planner pour pipelines multi-étapes.
- Modèle économique du registre par défaut.
- Ancrage de l'appartenance à un lobe (§11.6) — une instance déclare ses lobes depuis le 2026-09-13, rien ne le vérifie.

Si une décision implémentation force la main sur l'une de ces questions, **ne pas trancher silencieusement** — remonter à l'utilisateur.

## Conventions de travail spécifiques au projet

- Tout choix de stack ou archi qui dévie des docs nécessite **note datée en tête du doc concerné** + changelog explicite. Pas de dérive silencieuse.
- Sections "décisions" (archi §3-9) vs sections "dette" (archi §10-11, stack §16) : ne pas mélanger. Glisser une dette dans une décision = mensonge à soi-même ; glisser une décision floue dans une dette = procrastination.
- Documents en français. Code, identifiants, commits : anglais standard.

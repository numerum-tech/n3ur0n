# OpenWolf

@.wolf/OPENWOLF.md

This project uses OpenWolf for context management. Read and follow .wolf/OPENWOLF.md every session. Check .wolf/cerebrum.md before generating code. Check .wolf/anatomy.md before reading files.


# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## État du dépôt (8 mai 2026)

Phase pré-implémentation. **Aucun code à ce jour** — uniquement deux documents de spec :

- [n3ur0n-architecture-v0.md](n3ur0n-architecture-v0.md) — architecture v0.1 (vision, vocabulaire, couches, identité, protocole, limites assumées, questions ouvertes).
- [project-tech-stack.md](project-tech-stack.md) — stack technique v0.1 (Rust workspace, Tauri 2, SvelteKit, axum, SQLite, profils consumer/publisher).

**Règle de précédence** : si un détail de stack contredit l'architecture, l'architecture prime ; le doc de stack est mis à jour pour refléter le compromis (cf. `project-tech-stack.md` §17).

Avant tout travail d'implémentation, **lire les deux docs en entier** — ils sont opiniâtres, les choix sont fermes (pas des suggestions). Les sections "Limites assumées" (§10 archi) et "Limites et reports" (§16 stack) listent les compromis explicites de v0.1 ; ne pas réintroduire de fonctionnalité sans note datée.

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
5. **Surface utilisateur** — CLI / API REST + UI Svelte (desktop Tauri ou web servie par le binaire serveur).

## Invariants protocolaires non négociables

- Identifiant canonique = `n3:` + Base32(SHA-256(clé publique Ed25519)). Auto-vérifiable, pas de registre requis.
- Tout message porte `sender_id, recipient_id, timestamp, nonce, verb, payload, sender_public_key, signature`. La **signature** couvre le **JCS** (RFC 8785, `serde_jcs`) de l'envelope (tout sauf `signature`). Le champ `sender_public_key` accompagne le message sur le fil ; le destinataire vérifie `hash(sender_public_key) == sender_id` puis utilise cette clé pour vérifier la signature. (Voir amendement 2026-05-08 dans `n3ur0n-architecture-v0.md`.)
- **Sans canonicalisation, les signatures divergent silencieusement** — ne jamais sérialiser à la main pour signer.
- Vérifications obligatoires côté destinataire : binding pk↔id, signature, `recipient_id`, fenêtre timestamp ±5 min, anti-replay nonce sur 1h.
- Trois verbes méta (`describe_self`, `get_known_peers`, `ping`) sont **toujours en mode libre**. Ne jamais restreindre.
- Quatrième verbe : `invoke`. **Aucun autre verbe v0.1**. Pas de session, pas de streaming protocolaire, pas de pipeline orchestré côté protocole.
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
│   │   ├── protocol.rs        # payloads typés des 4 verbes
│   │   ├── capability.rs      # CapabilityDecl, AccessMode
│   │   └── error.rs
│   ├── adapters/              # lib : trait Backend + EchoBackend (MCP/OpenAI/HTTP/Process à venir)
│   ├── storage/               # lib : SQLite + r2d2, repos peers + nonces
│   │   └── migrations/        # SQL versionné via schema_version table
│   ├── node/                  # lib : orchestration runtime
│   │   ├── identity_file.rs   # load/save keys.json (0600)
│   │   ├── registry.rs        # CapabilityRegistry
│   │   ├── node.rs            # Node (keypair + db + backend + registry + clock)
│   │   └── handler.rs         # handle_request : verify → anti-replay → dispatch
│   ├── server/                # lib + bin : axum + clap (publisher)
│   │   ├── lib.rs             # http::app(node), bootstrap
│   │   ├── http.rs            # /n3ur0n/v0/messages + /api/v0
│   │   ├── bootstrap.rs       # config dirs, load_node, create_identity
│   │   ├── cli.rs             # init / serve / keys
│   │   └── main.rs
│   └── desktop/               # placeholder Tauri (excluded jusqu'à init Tauri CLI)
├── frontend/                  # SvelteKit + adapter-static + Tailwind
└── .gitignore                 # ignore /target, frontend build artifacts, runtime files
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
| Frontend | SvelteKit + `adapter-static` + Tailwind + bits-ui | React/Next, Vue/Nuxt (SSR incompatible avec embed statique) |
| Shell desktop | Tauri 2.x | Electron, Wails, Neutralino |
| CLI | `clap` v4 derive | structopt, argh |
| Logs | `tracing` + `tracing-subscriber` | `log` direct, `slog` |

## Commandes (validées sur scaffold actuel)

Workspace Rust opérationnel : `cargo check --workspace`, `cargo test --workspace` passent.

### CLI publisher

```bash
n3ur0n init                            # genère keys.json (0600) + sqlite
n3ur0n serve --port 4242 --endpoint http://... [--bootstrap http://peer1:4242 --bootstrap http://peer2:4242] \
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

`--config-dir` lu via flag OU env `N3UR0N_CONFIG_DIR`. `--bootstrap` lu via flag répété OU env `N3UR0N_BOOTSTRAP_PEERS` (CSV). Backend args lus aussi via env `N3UR0N_BACKEND`, `N3UR0N_OPENAI_BASE_URL`, `N3UR0N_OPENAI_MODEL`, `N3UR0N_OPENAI_API_KEY`.

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
docker compose -f docker/compose.yml down -v
```

3 nodes (`node-a`/`node-b`/`node-c`) sur ports hôte 4242/4243/4244, réseau bridge interne `n3uronnet`. Volumes par nœud. Healthcheck via `/n3ur0n/v0/health` (renvoie `{status, instance_id, protocol_version}`).

- `node-a`, `node-b` : backend echo (default).
- `node-c` : backend Ollama via `host.docker.internal:11434` (host Ollama réutilisé via `extra_hosts: host-gateway`). Modèle override par env `OLLAMA_MODEL` (défaut `qwen2.5:0.5b`), base URL override par `OLLAMA_BASE_URL`.
- `node-b` bootstrappe automatiquement depuis `node-a` (env `N3UR0N_BOOTSTRAP_PEERS`).

### Capacity planner v0.1 (Mode A — local LLM)

Le user **dialogue uniquement avec son instance**. L'instance fait planner → tool_call → observe → reply. v0.1 livre une seule impl `LLMPlanner` (tool-calling natif OpenAI). Modèle recommandé : `llama3.1:8b` ou `qwen2.5:7b`.

Cf `n3ur0n-planner-brainstorm.md` pour le brainstorm complet (3 modes, 4 niveaux, limites assumées).

**Flow runtime** :
```
browser → /api/v0/conversations/:id/messages {message}
       → middleware client_id (cookie)
       → ownership check (404 sinon)
       → conv_lock[id] mutex (sérialise même conv)
       → planner_slots semaphore (limite parallèle LLM)
       → load ConversationState (cache LRU OR SQLite)
       → planner.dispatch (boucle plan → tool_call → observe)
       → persist chaque turn (transaction atomique)
       → return {reply, trace, model}
```

**Limites configurables** (env / CLI) :
- `N3UR0N_PLANNER_MODE=llm|none`
- `N3UR0N_PLANNER_LLM_BASE_URL`, `N3UR0N_PLANNER_LLM_MODEL`, `N3UR0N_PLANNER_LLM_API_KEY`
- `N3UR0N_MAX_CONCURRENT_PLANNERS=4`
- `N3UR0N_MAX_ACTIVE_CONVERSATIONS=50`
- `MAX_TOOL_TURNS=6`, `MAX_CONTEXT_TURNS=16` (constantes code)

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

Smoke script (`bash docker/cluster-smoke.sh`) couvre : healthchecks, 6 pings croisés signés, describe_self, invoke chat signé a→c, bootstrap b←a, cascade depth-1 a→b→c sur capacité `chat`, **et le chemin browser** via POST `/api/v0/chat` sur node-a.

```bash
# Workspace Rust
cargo build --release -p n3ur0n-server
cargo test                                   # cible 80%+ sur core
cargo run -p n3ur0n-server -- serve

# Frontend (codebase UI unique)
pnpm --filter frontend build                 # output dans frontend/build/
pnpm --filter frontend dev                   # proxy Vite vers /api du serveur

# Desktop Tauri
pnpm tauri dev                               # hot reload UI + recompile Rust
pnpm tauri build --target <triple>           # .dmg / .msi / .AppImage / .deb

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

## Questions ouvertes bloquantes (archi §11)

À trancher **avant lancement public**, pas avant le code :

- Granularité de la synapse (1:1 vs 1:lobe vs 1:capability).
- Mécanisme anti-free-riding pour lobes communautaires.
- Position juridique sur les noms de marques (`@google`, `@adobe`).
- Localisation du planner pour pipelines multi-étapes.
- Modèle économique du registre par défaut.

Si une décision implémentation force la main sur l'une de ces questions, **ne pas trancher silencieusement** — remonter à l'utilisateur.

## Conventions de travail spécifiques au projet

- Tout choix de stack ou archi qui dévie des docs nécessite **note datée en tête du doc concerné** + changelog explicite. Pas de dérive silencieuse.
- Sections "décisions" (archi §3-9) vs sections "dette" (archi §10-11, stack §16) : ne pas mélanger. Glisser une dette dans une décision = mensonge à soi-même ; glisser une décision floue dans une dette = procrastination.
- Documents en français. Code, identifiants, commits : anglais standard.

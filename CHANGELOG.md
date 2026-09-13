# Changelog

All notable changes to this project are documented here. The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once 1.0 ships.

## [Unreleased]

## [0.4.3] — 2026-09-13 — addressing, manifests out of the binary, activity

### Added
- **Readable addressing.** An instance can carry an `alias` (`--alias`, `N3UR0N_ALIAS`, `instance.toml`, or Settings → Identity), announced in `describe_self`. Chat mentions render it as `alias#idprefix` and still resolve **only** by `n3:` id: an alias is a claim a node makes about itself and would go to the first squatter. `@peer:self` names the current instance.
- **Lobe membership.** An instance declares the federations it belongs to (`--lobe`, `N3UR0N_LOBES`, `instance.toml`, `PUT /api/v0/settings/lobes`), 5 max, and `cap.lobe_ids ⊆ instance.lobe_ids` is enforced at registration. Membership is **declared, not verified** — see architecture §11.6.
- **`@` mention picker** in the composer scoping files, peers, capabilities and lobes; each entry shows the token it inserts, a partial prefix filters by type, and an unknown reference now **refuses the dispatch** (422 `unresolved_mentions`) instead of silently widening the scope the user had narrowed.
- **Activity section**: a live dashboard of what the node serves, runs and calls, fed by the `audit_log` table — which had existed since the first migration with no writer. Aggregates only, never a listing; SSE snapshot every two seconds. `GET /api/v0/activity`, `GET /api/v0/activity/stream`.
- **Continuation rounds** in the planner (`MAX_PLAN_ROUNDS=2`), driven by structural triggers — a failed step, or a plan that hit the per-round depth cap — never by asking the model to judge its own completeness.
- **Hybrid capability retrieval** (BM25 + embeddings) when an embeddings endpoint is configured.
- `rename_file` capability and a real blob round trip over the wire, covered by three cluster tests (`cargo test -p n3ur0n-node --test cluster_blob_transfer -- --ignored`).
- Blobs carry a human-readable `path`; capability outputs are attached to the user who requested them.
- `bash scripts/ui-smoke.sh` — drives the embedded UI in a real Chrome over CDP, asserts on the DOM and captures PNGs. No npm dependency: Node 22 has `WebSocket`, Chrome speaks the protocol.
- Reference doc [n3ur0n-blob-lifecycle-v0.md](n3ur0n-blob-lifecycle-v0.md): a file's full life between two nodes, with the known gaps between spec and code.

### Changed
- **Capabilities belong outside the gateway.** The five compiled utility functions (`time`, `random_int`, `reverse`, `string_length`, `rename_file`) moved to `docker/hermes/`, an HTTP service in its own container, reached through an `http_base` backend manifest and one cap manifest each (`docker/manifests/`). Every instance ships the same binary, so anything compiled into it is published identically by every node and the network has nothing to route — the cluster only looked federated because `N3UR0N_CAPS` hid part of the list on each node. These are also the repo's first worked example of manifest mode, which existed only in the parser's tests.
- **`--manifest-dir` has a default**: `<config>/manifests`. Manifest mode engages when that directory holds a `.toml`; a node whose directory is empty keeps its compile-time backend and still gets the manifest runtime, so the first capability saved through the UI is served without a restart.
- `{{args}}` in a binding template forwards the whole argument object. Without it a capability with *optional* arguments had no valid spelling: naming each field fails to render as soon as one is absent, and omitting `body_template` sends no body.
- Docker cluster: fixed subnet with the last octet matching the published port (a recreated container used to reshuffle the others and a cached name then pointed at a different node); capabilities spread so no node has everything, one node has none, and one advertises a capability it cannot serve.
- UI: instance identity grouped under **Identity**, About left to the project; dispatch statuses translated and Firstcapped; direct chat waits with an animated ellipsis instead of a plan panel it has no plan for.
- Default planner model `qwen2.5:7b` (measured; 7B is the practical floor — see [n3ur0n-planner-selection-v0.md](n3ur0n-planner-selection-v0.md) §6bis).
- Docs corrected to describe the UI that actually ships. The web UI is `crates/server/ui/`: hand-written vanilla JS (ES modules) + CSS, **no bundler, no package.json, no build step** — ~4.3k LOC across `app.js` (3500), `auth.js`, `i18n.js`, `icons.js`, `index.html`, `style.css`, `locales/{en,fr}.json`. It is embedded by rust-embed (`#[folder = "ui/"]`, `crates/server/src/http.rs`) and is also what the desktop shell loads (`"frontendDist": "../server/ui"`). In debug builds rust-embed reads the assets from disk, so editing a `.js`/`.css` and reloading the page needs no recompile; release builds embed them. Claims of SvelteKit / `adapter-static` / Tailwind / `bits-ui` / `pnpm --filter frontend` in `CLAUDE.md`, `project-tech-stack.md` (§8, §10.1, §12.4, §15) and the 0.1.0 entry below were never true; each is now corrected in place with a dated deviation note.

### Fixed
- **Accented text was mangled in every reply.** `resolve_value` and `substitute_inline` walked their input byte by byte and ended each iteration on `out.push(bytes[i] as char)`, reading one UTF-8 byte as a Latin-1 code point: `chaîne inversée` reached the browser as `chaÃ®ne inversÃ©e`. `resolve_value` runs over every composer reply.
- **A manifest-mode node could not execute its own capability.** The plan executor sent any step with no endpoint to `node.backend()` — the compile-time slot, which manifest mode fills with an inert Echo — so `time` answered `{}` while the same capability answered correctly to a signed invoke from a peer.
- **The settings API edited a directory the node never read.** It used `config_dir/{caps,backends}` while the node reloads from `--manifest-dir`; a capability saved on the cluster returned `{"ok": true, "registered": 2}`, the count coming from the other directory. A reload that can load none of the manifests present no longer swaps an empty registry in, and a capability saved but not registered answers 422 naming the missing backend.
- **The composer answered the same message twice.** The user turn is recorded before the plan compiles, so the conversation tail handed to reflect already ended on it and the re-statement appended a second copy.
- Blob layer: retention derives from the ticket's *purpose*, not its 300 s authorization window (uploads were swept by the GC minutes after landing); a file that has been sent stops classifying itself as local cache; an upload appears in the section it actually landed in, and sections that cannot receive one no longer offer the button.
- A tool call and its result are persisted atomically — a crash between the two left a dangling call.
- The Skills refresh button actually asks the peers (`POST /api/v0/peers/refresh`) instead of re-reading local caches; a peer's alias survives a reverse-announce that does not know it.
- A peer mention resolves through an indexed `GLOB` prefix rather than a full directory scan.

### Removed
- `UtilityBackend` and `--backend utility`. **Breaking** for any deployment selecting it; the equivalent capabilities now come from manifests (see Changed). `EchoBackend` and the OpenAI-compatible backend are unaffected.
- `frontend/` — dead SvelteKit scaffold from the initial commit (`1453c6b`, May 2026). 214 LOC of boilerplate whose own landing copy read *"Pre-implementation scaffold. UI surfaces … will land progressively"*; they never did. It was never built by CI (no `pnpm`/`npm` step in either workflow), never served, and had no root `package.json` to `--filter` from. Its only external reference was a `COPY frontend ./frontend` in `docker/Dockerfile`, which fed an image with no node toolchain — removed too.

## [0.4.2] — 2026-07-20 — planner accuracy + release plumbing

First properly-documented release since 0.4.0; also carries everything that shipped unversioned in 0.4.1 (direct chat, blob layer, id truncation, dependency bumps, `OpenAIBackend` hardening — listed below).

### Added
- Reusable planner-accuracy eval suite (`crates/node/tests/planner_eval.rs` + `fixtures/planner_cases.json`, `scripts/planner-eval.sh`): 22 cases across single/multi/chain/none/trap categories; grades valid/exact/precision/recall per category, JSON report, tool-valid gate ≥95%. Ignored by default (needs an LLM).
- `GET /n3ur0n/v0/health` now returns `version` (`env!("CARGO_PKG_VERSION")`) alongside `protocol_version`, so a deployed node's release is checkable directly.
- www: planner-model guidance (8B/14B, CPU hosting) folded into Start; GitHub links hub.
- Direct chat mode: `DirectChatPlanner`, `POST .../messages` `{ mode?: "auto"|"direct", model?: string }`, composer toggle + model override in UI (EN/FR). *(shipped unversioned in 0.4.1)*
- Blob protocol layer (spec `n3ur0n-blob-protocol-v0.md` + 2026-06-04 amendment): hash-addressed transfer on `PUT/GET/HEAD/DELETE /n3ur0n/v0/blobs/*hash` authorized by the new signed `blob_ticket` verb (never dispatched via `/messages`); A–D blob classes; periodic GC; local Files API (`/api/v0/files`, `/api/v0/cap-jobs/blobs`) and Files panel in the UI; message attachments threaded through planners via `UserInput`. *(shipped unversioned in 0.4.1)*

### Fixed
- Planner tool selection reliability: compile prompt now separates `peer:`/`capability:` (was a combined `peer::cap` header the model copied whole into `peer`) and lists each skill's `output_fields`; `validate_plan` gained a ref-path guard (step exists, no self-ref, field in `schema_out`). Measured tool-valid rate 12%→100% on the eval suite.
- Planner no longer fabricates temporal values on an empty plan (prompt counter-rule: the model does not know the current time — it must use a time skill, never invent one).
- Compile-prompt size guard (`COMPILE_PROMPT_TOKEN_BUDGET`, logs approx tokens, warns above budget).
- UI: live plan-step chips are clickable during streaming (stream `args`/`result` through `StepDone`), not only after reload.
- Release assets named `n3ur0n-server-*` / `n3ur0n-desktop-*`.

### Changed
- Instance id shortened: derived from the **first 20 bytes** of `SHA-256(pubkey)` instead of the full 32 (`n3:` + 32 Base32 chars, was 52). Truncation is on the hash bytes, not the Base32 string; `ID_HASH_BYTES` in `core/identity.rs`. Collision ~2^80, second-preimage ~2^160. No `protocol_version` bump — no deployed network at the time. **Breaking for any pre-existing `keys.json`: the same key now yields a different id** (a stale id in `keys.json` self-heals to the secret-derived one with a warning). *(shipped unversioned in 0.4.1)*
- Dependencies: rand 0.8→0.10, ed25519-dalek 2→3, sha2 0.10→0.11, serde_jcs 0.1→0.2 (canonical output verified byte-identical via golden test), axum 0.7→0.8 (route param syntax `:x`→`{x}`), tower-http 0.5→0.6, http-body-util 0.1.4, docker builder base rust 1.97. *(shipped unversioned in 0.4.1)*
- `OpenAIBackend`: caller-supplied `model` in invoke payloads is now ignored unless `allow_model_override` is set (network-facing backends lock to `default_model`); base URLs are normalized (strips `/v1`, `/api/generate`, `/v1/chat/completions` suffixes). *(shipped unversioned in 0.4.1)*
- UI: dark theme is the default; backend base-url example uses `localhost`; Firstcaps labels; cap-form back-nav returns to the template picker.

## [0.4.0] — open source, i18n, RBAC, settings UX

### Added
- Apache-2.0 license + open-source scaffolding (README, CONTRIBUTING, CODE_OF_CONDUCT, SECURITY).
- GitHub Actions: CI (Linux + macOS + Windows), release workflow for desktop + server artefacts, Pages workflow for the landing page.
- GitHub Pages landing page with download buttons that pull from the latest release.
- i18n EN + FR: catalogs at `crates/server/ui/locales/{en,fr}.json`, `/api/v0/locales`, `data-i18n` DOM attributes, locale picker in Settings → Interface.
- Interface settings: language picker + Dark / Light / System theme (`:root[data-theme]`).
- Capability composer: all three binding kinds (`prompt`, `mcp`, `http`) with per-kind backend filtering; template picker (blank, translator, summarizer, fact-extractor, weather HTTP, fs-read MCP).
- `AccessMode::Private`: excluded from `describe_self`, remote invoke returns `UnknownCapability`; Public / Restricted / Private badges in UI.
- Skills type filter (binding kind + access mode) on sidebar and Settings.
- Backend hot-reload: `Node.backends` is `Arc<ArcSwap<BackendsRegistry>>`; POST/DELETE `/api/v0/backends` reload without restart.
- Backend edit form: GET `/api/v0/backends/:name`, upsert with `api_key_keep` to preserve secrets on blank edit.
- RBAC phase 1: SQLite migration `0003_users_sessions.sql`, argon2id passwords, session cookies, roles (User / Operator / Admin), permission-gated API routes, Users admin page, `N3UR0N_AUTH_DISABLE=1` for loopback dev.

### Changed
- Workspace and desktop package version aligned to **0.4.0** (wire `protocol_version` remains `n3ur0n/0.3` — no envelope change).

## [0.3.0] — capability manifests + desktop client

### Added
- TOML capability manifest system (`backends/*.toml` + `caps/*.toml`) at `<config_dir>`.
- Three binding types: `prompt`, `mcp`, `http`.
- Hot-reload of capability registry via ArcSwap (no restart for skill CRUD).
- Master-detail Settings UI: Backends, Skills, Gateways, Identity, About.
- Capability composer form (prompt binding).
- Tauri 2 desktop shell with embedded loopback axum server.
- First-launch Ollama auto-detect + default backend scaffold.
- Modal dialog replacing native `window.confirm`/`alert` (Tauri WKWebView compatibility).
- Reverse-announce: peers can attach `sender_endpoint` to envelopes for symmetric discovery.
- Transitive bootstrap (depth-N peer crawl on `--bootstrap`).

### Changed
- Backend instantiation moved from compile-time `BackendKind` enum to runtime manifest scan.
- `Node.registry` is now `Arc<ArcSwap<CapabilityRegistry>>`.

## [0.2.0] — planner v2

### Added
- PlanExec planner: typed plan with parallel step execution and SSE streaming dispatch.
- BM25 retrieval over capability examples + descriptions.
- Constrained decoding via GBNF / JSON Schema (when the backend supports it).
- PlanCompiler cascade across known peers.

## [0.1.0] — initial protocol

### Added
- Workspace crates: `core`, `storage`, `adapters`, `node`, `server`.
- Ed25519 signed envelopes with JCS canonicalization.
- Four protocol verbs: `describe_self`, `get_known_peers`, `ping`, `invoke`.
- SQLite storage for peers + nonces (anti-replay).
- OpenAI-compatible backend (Ollama, llama.cpp, vLLM).
- Echo + utility backends for tests.
- Tower/axum HTTP server + clap CLI.
- Bundled static web UI (rust-embed). *(Listed as "Svelte" until 2026-07-28 — it never was; see Unreleased.)*
- Docker compose cluster + smoke test.

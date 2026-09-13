# anatomy.md

> Auto-maintained by OpenWolf. Last scanned: 2026-09-13T08:38:53.625Z
> Files: 157 tracked | Anatomy hits: 0 | Misses: 0

## ./

- `.dockerignore` — Docker ignore rules (~42 tok)
- `.gitignore` — Git ignore rules (~527 tok)
- `AGENTS.md` — OpenWolf (~75 tok)
- `Cargo.toml` — Rust package manifest (~948 tok)
- `CHANGELOG.md` — Changelog (~2177 tok)
- `CLAUDE.md` — OpenWolf (~5805 tok)
- `CODE_OF_CONDUCT.md` — Contributor Covenant Code of Conduct (~577 tok)
- `CONTRIBUTING.md` — Contributing to N3UR0N (~770 tok)
- `GEMINI.md` — OpenWolf (~75 tok)
- `LICENSE` — Project license (~3016 tok)
- `n3ur0n-architecture-v0.md` — N3UR0N — Document d'architecture (draft 0) (~8416 tok)
- `n3ur0n-blob-protocol-v0.md` — N3UR0N — Blob Protocol v0.1 (brouillon) (~9178 tok)
- `n3ur0n-capability-manifest-v0.md` — N3UR0N — Capability Manifest v0.1 (brouillon) (~5750 tok)
- `n3ur0n-direct-chat-v0.md` — N3UR0N — Mode chat direct (spec d'implémentation v0) (~2266 tok)
- `n3ur0n-planner-brainstorm.md` — N3UR0N — Brainstorm planner & multi-conversation (~3026 tok)
- `n3ur0n-planner-enhancements-v1.md` — Planner améliorations v1, spec P1–P6 pour agent de code (~2713 tok)
- `n3ur0n-planner-optimisation-v0.md` — Revue du moteur de planification — pertinence et rapidité (~7508 tok)
- `n3ur0n-planner-recommendations-v0.md` — Recommandations Planner — N3UR0N v0.1 → v0.2 (~7412 tok)
- `n3ur0n-planner-selection-v0.md` — N3UR0N — Sélection de capacité par le planner (brainstorm v0) (~5482 tok)
- `NOTICE` (~82 tok)
- `project-tech-stack.md` — N3UR0N — Stack technique (draft 1) (~8032 tok)
- `README.md` — Project documentation (~1877 tok)
- `ROADMAP.md` — Roadmap (~2370 tok)
- `SECURITY.md` — Security Policy (~374 tok)
- `USE_CASES.md` — N3UR0N use cases (samples) (~1715 tok)

## .github/

- `dependabot.yml` (~138 tok)
- `PULL_REQUEST_TEMPLATE.md` — Summary (~206 tok)

## .github/ISSUE_TEMPLATE/

- `bug.yml` (~463 tok)
- `config.yml` (~110 tok)
- `feature.yml` (~239 tok)

## .github/workflows/

- `ci.yml` — CI: CI (~601 tok)
- `release.yml` — CI: Release (~2186 tok)

## crates/adapters/

- `Cargo.toml` — Rust package manifest (~200 tok)

## crates/adapters/src/

- `echo.rs` — Echo backend: returns the args verbatim. For dev / smoke tests. (~716 tok)
- `embeddings.rs` — Embedding client for an OpenAI-compatible `/v1/embeddings` endpoint. (~2231 tok)
- `lib.rs` — n3ur0n-adapters (~296 tok)
- `openai.rs` — OpenAI-compatible backend. (~5229 tok)
- `utility.rs` — Utility backend: a deterministic backend exposing several small (~4179 tok)

## crates/adapters/tests/

- `openai_backend.rs` — Mock-server tests for `OpenAIBackend`. (~2599 tok)

## crates/core/

- `Cargo.toml` — Rust package manifest (~201 tok)

## crates/core/src/

- `blob.rs` — Blob protocol types: BlobRef, ticket payload, classification enums. (~4894 tok)
- `capability.rs` — Capability declaration as exposed in `describe_self`. (~1832 tok)
- `error.rs` — Core errors. Stable, exhaustive — every variant maps to a distinct rejection (~456 tok)
- `identity.rs` — Cryptographic identity for an n3ur0n instance. (~1938 tok)
- `lib.rs` — `n3ur0n-core`: pure protocol logic. (~491 tok)
- `message.rs` — Wire envelope, signed message, JCS-canonical signing helpers. (~2794 tok)
- `protocol.rs` — Typed payloads for the four v0.1 protocol verbs. (~1396 tok)
- `verify.rs` — Pure envelope verification. (~1784 tok)

## crates/desktop/

- `.gitignore` — Git ignore rules (~37 tok)
- `build.rs` (~12 tok)
- `Cargo.toml` — Rust package manifest (~320 tok)
- `README.md` — Project documentation (~669 tok)
- `tauri.conf.json` (~228 tok)

## crates/desktop/capabilities/

- `default.json` (~64 tok)

## crates/desktop/icons/

- `icon.icns` (~89228 tok)

## crates/desktop/src/

- `main.rs` — N3UR0N desktop shell (Tauri 2). (~3107 tok)

## crates/node/

- `Cargo.toml` — Rust package manifest (~299 tok)

## crates/node/src/

- `backends_registry.rs` — Name-addressed registry of live backend instances. (~526 tok)
- `blob_client.rs` — Outbound blob upload/download helpers. (~1819 tok)
- `blob_resolve.rs` — Resolve BlobRefs before remote invokes and fetch output blobs after. (~3092 tok)
- `client.rs` — Outbound peer client. (~1563 tok)
- `conversation.rs` — Conversation state: in-memory shape of a persisted thread. (~7465 tok)
- `discovery.rs` — Discovery: bootstrap initial peers + transitive cascade + on-demand (~3233 tok)
- `error.rs` — Errors produced by the node orchestration layer. (~492 tok)
- `handler.rs` — Verb dispatcher. (~2828 tok)
- `identity_file.rs` — On-disk identity file (`keys.json`). (~1538 tok)
- `lib.rs` — `n3ur0n-node`: runtime orchestration shared between the server and the (~296 tok)
- `mention.rs` — Explicit scoping mentions typed in the chat composer. (~4756 tok)
- `node.rs` — Runtime n3ur0n node. (~2883 tok)
- `registry.rs` — In-memory capability registry. (~1410 tok)
- `runtime.rs` — Runtime orchestration for multi-conversation, multi-client workloads. (~3269 tok)

## crates/node/src/bindings/

- `backend.rs` — Backend instances: live handles to upstream services. (~942 tok)
- `http.rs` — `http` binding — forward to an HTTP endpoint declared in the manifest. (~3090 tok)
- `mcp_client.rs` — Minimal MCP client over stdio. (~2333 tok)
- `mcp.rs` — `mcp` binding — call a tool on an MCP server. (~2423 tok)
- `mod.rs` — Capability bindings: the runtime side of cap manifests. (~871 tok)
- `prompt.rs` — `prompt` binding — LLM call configured by manifest data. (~2609 tok)
- `template.rs` — Tiny templating for `{{args.dotted.path}}` substitution. (~1892 tok)

## crates/node/src/manifest/

- `mod.rs` — Capability + backend manifest format (cap.toml / backend.toml). (~220 tok)
- `parser.rs` — TOML loader + validator for `backends/*.toml` and `caps/*.toml`. (~4398 tok)
- `tests.rs` — Parser tests for the v0.3 manifest format. (~3026 tok)
- `types.rs` — Types backing the v0.3 manifest format. (~1775 tok)

## crates/node/src/planner/

- `catalog.rs` — Aggregated capability catalog (self + peers) for the planner. (~7785 tok)
- `compiler.rs` — Pluggable plan compilation. (~4417 tok)
- `direct.rs` — Direct chat planner: single LLM call per message, no plan compilation. (~4429 tok)
- `grammar.rs` — Constrained-decoding grammars for the planner's compile step. (~7103 tok)
- `mod.rs` — Planner trait + concrete impls. (~1683 tok)
- `plan_exec.rs` — Plan-then-execute planner. (~24340 tok)
- `plan.rs` — Typed plan + reference resolver + executor for plan-then-execute mode. (~16060 tok)
- `planner_cap.rs` — Expose a [`PlanCompiler`] as the `plan` capability on the network. (~2415 tok)
- `retrieval.rs` — BM25 retrieval over the capability catalog. (~2515 tok)
- `retriever.rs` — Capability retrieval: BM25, optionally fused with embeddings. (~3939 tok)

## crates/node/tests/

- `continuation_rounds.rs` — A failed step must give the planner a second round. (~2832 tok)
- `handler.rs` — End-to-end tests for the verb dispatcher. (~2414 tok)
- `planner_eval.rs` — Reusable planner-accuracy suite. (~7538 tok)

## crates/node/tests/fixtures/

- `mock_mcp_server.py` — reply, error, handle, main (~919 tok)
- `planner_caps_large.json` — Declares strings (~47134 tok)
- `planner_caps.json` — Declares option (~4762 tok)
- `planner_cases.json` (~2585 tok)

## crates/server/

- `Cargo.toml` — Rust package manifest (~370 tok)

## crates/server/src/

- `auth.rs` — RBAC: session cookie auth + role/permission policy + auth routes. (~5473 tok)
- `blob_gc.rs` — Background garbage collection for expired blobs. (~350 tok)
- `blobs.rs` — Publisher blob HTTP handlers (`/n3ur0n/v0/blobs/:hash`). (~4292 tok)
- `bootstrap_config.rs` — Persisted bootstrap seed peers (`bootstrap.toml` in the config dir). (~2114 tok)
- `bootstrap.rs` — Wiring helpers: paths, node construction. (~4073 tok)
- `cli.rs` — CLI subcommands. (~4804 tok)
- `files_api.rs` — Local user files API (`/api/v0/files`). (~3235 tok)
- `http.rs` — Axum app: peer protocol + local API + embedded UI. (~13259 tok)
- `lib.rs` — Library surface of `n3ur0n-server`. Exposed so integration tests can mount (~137 tok)
- `main.rs` — N3UR0N publisher binary entry point. (~398 tok)
- `planner_config.rs` — User-facing planner model selection (`planner.toml` in the config dir). (~3176 tok)
- `settings.rs` — Settings REST surface: backend + capability manifest CRUD over the (~9948 tok)

## crates/server/tests/

- `blob_outputs.rs` — A capability output must land in the requesting user's Files panel. (~1415 tok)
- `blobs.rs` — Integration tests for blob PUT/GET/HEAD. (~845 tok)
- `direct_chat.rs` — Integration tests for conversation dispatch `mode: "direct"`. (~2092 tok)
- `discovery.rs` — Discovery integration test: spin up two real HTTP listeners on (~1127 tok)
- `http_protocol.rs` — Integration test: drive the axum router with `tower::ServiceExt::oneshot`, (~1578 tok)

## crates/server/ui/

- `app.js` — Per-message draft attachments (cleared on send or conversation switch). (~48965 tok)
- `auth.js` — Frontend auth glue. (~2881 tok)
- `i18n.js` — Tiny i18n runtime for the N3UR0N web UI. (~1464 tok)
- `icons.js` — Bundled SVG icons (Lucide-derived paths, ISC license). No CDN — works in Tauri WKWebView. (~2138 tok)
- `index.html` — N3UR0N (~5158 tok)
- `style.css` — Styles: 81 rules, 31 vars (~13369 tok)

## crates/server/ui/locales/

- `en.json` — Declares D (~4500 tok)
- `fr.json` — Declares n (~4865 tok)

## crates/storage/

- `Cargo.toml` — Rust package manifest (~201 tok)

## crates/storage/migrations/

- `0001_init.sql` — Initial N3UR0N schema. See project-tech-stack.md §6.2. (~496 tok)
- `0002_conversations.sql` — Conversations: first-class persistent threads, isolated per browser/Tauri (~328 tok)
- `0003_users_sessions.sql` — RBAC: users + sessions. (~323 tok)
- `0004_blobs.sql` — Blob index (publisher + consumer mirror for classes A/B/D). (~339 tok)
- `0005_plan_runs.sql` — plan_runs: durability journal for planner dispatches (phase 1, write-only). (~237 tok)
- `0006_blob_path.sql` — Human-readable path for a blob (local petname over the content hash). (~146 tok)

## crates/storage/src/

- `auth.rs` — Users + sessions: persistence + password / session-token primitives. (~3365 tok)
- `blobs.rs` — Blob index repository (SQLite). (~3350 tok)
- `conversations.rs` — Conversations + turns repo. (~2667 tok)
- `lib.rs` — n3ur0n-storage (~868 tok)
- `nonces.rs` — Insert a nonce. Returns `Ok(true)` if newly inserted, `Ok(false)` if already seen (replay). (~382 tok)
- `peers.rs` — [derive(Debug, Clone, Serialize, Deserialize)] (~1218 tok)
- `plan_runs.rs` — Plan runs repo — durability journal for planner dispatches (phase 1). (~1337 tok)

## deploy/hiawatha/

- `n3ur0n.net.conf` — N3UR0N — Hiawatha front for n3ur0n.net (alternative to nginx) (~884 tok)

## deploy/nginx/

- `n3ur0n.net.conf` — N3UR0N — nginx front for n3ur0n.net (HTTP only) (~991 tok)

## deploy/systemd/

- `install.sh` — Install / update the N3UR0N seed systemd unit on a Linux VPS. (~478 tok)
- `n3ur0n-seed.env.example` — /etc/n3ur0n/seed.env — sourced by n3ur0n-seed.service (~162 tok)
- `n3ur0n-seed.service` — N3UR0N seed / publisher — systemd unit (~467 tok)
- `uninstall.sh` — Tear down the N3UR0N seed systemd install (inverse of install.sh). (~425 tok)

## docker/

- `cluster-smoke.sh` — Cluster smoke test: (~2204 tok)
- `compose.yml` — 4-node N3UR0N test cluster. (~1298 tok)
- `Dockerfile` — Docker container definition (~786 tok)
- `entrypoint.sh` — Auto-init identity on first boot, then serve. (~161 tok)

## docs/superpowers/plans/

- `2026-06-04-direct-chat-mode.md` — Direct Chat Mode Implementation Plan (~1849 tok)

## docs/superpowers/specs/

- `2026-07-15-n3uron-www-design.md` — Design: n3uron.com one-pager (`www/`) (~260 tok)
- `2026-07-17-bootstrap-seeds-ui-design.md` — Design: Settings bootstrap seeds (~263 tok)

## scripts/

- `gen-planner-caps-large.py` — Generate the large adversarial capability corpus for the planner eval. (~6793 tok)
- `planner-eval.sh` — Planner accuracy suite — run often to see how the planner behaves. (~235 tok)

## www/

- `fonts.css` — Styles: 16 rules (~1765 tok)
- `index.html` — N3UR0N — peer-to-peer AI capability network (~4487 tok)
- `styles.css` — Styles: 73 rules, 15 vars (~4247 tok)

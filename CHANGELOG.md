# Changelog

All notable changes to this project are documented here. The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once 1.0 ships.

## [Unreleased]

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
- Bundled Svelte web UI (rust-embed).
- Docker compose cluster + smoke test.

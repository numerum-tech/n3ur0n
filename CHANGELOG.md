# Changelog

All notable changes to this project are documented here. The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once 1.0 ships.

## [Unreleased]

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

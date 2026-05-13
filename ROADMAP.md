# Roadmap

Public-facing milestone tracker. Internal session notes live outside this repo.

See [CHANGELOG.md](CHANGELOG.md) for what has shipped.

## Status legend

- ✅ shipped
- 🚧 in progress
- 🧭 planned, scope clear
- 💭 planned, scope still open
- ⛔ explicitly out of scope (for now)

---

## v0.1 — protocol skeleton ✅

- Workspace crates: `core`, `storage`, `adapters`, `node`, `server`
- Ed25519 signed envelopes with JCS canonicalization
- 4 protocol verbs: `describe_self`, `get_known_peers`, `ping`, `invoke`
- Anti-replay (nonce table, ±5 min timestamp window)
- OpenAI-compatible backend (Ollama, llama.cpp, vLLM)
- SQLite storage for peers + nonces
- Axum HTTP server + clap CLI
- Bundled web UI (rust-embed)
- Docker compose cluster + smoke test

## v0.2 — planner ✅

- PlanExec planner: typed plan, DAG validation, parallel step execution
- SSE streaming dispatch + horizontal step UI
- BM25 retrieval over capability examples/descriptions
- Constrained decoding (GBNF / JSON Schema) when backend supports it
- PlanCompiler cascade across known peers
- Backpressure (per-step semaphore) + 180s LLM timeout

## v0.3 — capability manifests ✅

- TOML manifests at runtime (`backends/*.toml` + `caps/*.toml`)
- Three binding types: `prompt`, `mcp`, `http`
- Hot-reload of capability registry (no restart for skill CRUD)
- Master-detail Settings UI (Backends, Skills, Gateways, Identity, About)
- Capability composer form (prompt binding)
- Tauri 2 desktop shell with embedded loopback axum server
- First-launch Ollama auto-detect + default backend scaffold
- Reverse-announce + transitive bootstrap

## v0.4 — open source 🚧

- ✅ Apache-2.0 + public docs + GitHub Pages download site
- ✅ Multi-arch CI + release workflow (server bin × 5 targets, Docker, desktop installers)
- 🧭 **i18n — EN + FR shipped by default.** Web UI, desktop shell menus,
  CLI `--help`, error messages. Locale resolved from `Accept-Language`
  (web) / OS locale (desktop) / `--lang` flag (CLI). Translation
  catalog under `crates/server/ui/locales/{en,fr}.json` and
  `crates/server/src/locales/`. Third-party locales accepted via PR.
- 🧭 MCP + HTTP binding forms in composer (currently prompt-only)
- 🧭 Backend hot-reload (currently restart-required)
- 🧭 OS keychain integration for backend `api_key` storage
- 🧭 Conversation summarization above N turns
- 🧭 SSE reconnect + stable step IDs across dispatches
- 💭 Wire format fuzz harness (`cargo-fuzz`)
- 💭 Public capability marketplace browser

## v0.5 — federation 💭

- Lobe membership protocol (subscribe / unsubscribe / list members)
- HITL approval gate before sensitive `invoke` execution
- Brand-handle resolution semantics (`@google`, `@adobe` — legal review first)
- Anti-free-riding signal for community lobes
- Public bootstrap registry (governance TBD)

## v1.0 — protocol freeze 💭

- Protocol invariants frozen: 4 verbs, envelope shape, signature path
- Stable wire compatibility commitment
- Mobile clients (iOS / Android)
- WASM / subprocess generic bindings
- Multi-modal capabilities (image-gen, audio)

---

## Out of scope (explicit ⛔)

These have been considered and rejected for the foreseeable future. Re-open only with a new use case the spec cannot already serve.

- Central registry of capabilities (peer gossip is the model)
- Streaming inside the protocol (use the local SSE bridge, not the wire)
- Pipeline orchestration as a protocol verb (planner composes locally)
- Session state at the protocol layer (every `invoke` is independent)
- Cross-conversation long-term memory in v0.x (per-conversation only)

---

## Open governance questions

To resolve before a public launch — not before v0.4 ships:

- Synapse granularity (1:1 vs 1:lobe vs 1:capability)
- Economic model of any default registry
- Localisation of planners for multi-step pipelines
- Trademark posture on brand handles

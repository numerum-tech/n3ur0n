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

## v0.4 — open source ✅ (release 0.4.0)

- ✅ Apache-2.0 + public docs + GitHub Pages download site
- ✅ Multi-arch CI + release workflow (server bin × 5 targets, Docker, desktop installers)
- ✅ i18n EN + FR (web UI catalogs, Settings → Interface locale picker, `data-i18n` DOM). Third-party locales via PR (`crates/server/ui/locales/*.json`).
- ✅ MCP + HTTP binding forms in composer + template picker
- ✅ Backend hot-reload (`ArcSwap` on `BackendsRegistry`; POST/DELETE backends without restart)
- ✅ RBAC phase 1 (users, sessions, roles, permission-gated `/api/v0`)
- ✅ `AccessMode::Private` + cap type filters in Settings / sidebar
- ✅ Interface theme (Dark / Light / System)
- 🧭 CLI `--help` + server error messages fully i18n (web UI done in 0.4.0)
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
- **Registry-as-capability** (see reflection below)

---

## Reflection — registry as a capability

**Premise.** The spec rejects a central registry. But "discover a capability"
is itself a useful operation, and the network needs *some* answer to
"where can I find a French→English translator I haven't seen before?".

**Proposal.** Don't bake a registry into the protocol. Expose registries
as ordinary capabilities, invoked through the same signed `invoke` verb
that any other capability uses. A registry is just a peer that publishes
caps like:

```
[descriptor]
name        = "registry.search"
description = "Search a capability index by name, tag, language, country."
schema_in   = { query: string, tags?: [string], languages?: [string], limit?: int }
schema_out  = { results: [{ instance_id, endpoint, cap_name, score }] }
```

```
[descriptor]
name        = "registry.announce"
description = "Submit this peer's describe_self for indexing."
schema_in   = { describe_self: object, signature: string }
schema_out  = { accepted: bool, ttl_seconds: int }
```

**What this buys.**
- Zero protocol delta. Four verbs, same envelope, same signature path.
  A "registry" is a peer with a few opinionated caps.
- Pluggable trust. Each user picks which registry peers they trust the
  way they pick which gateways to bootstrap from. Operate without one
  works fine — peer gossip remains the default.
- Competing registries are natural. A privacy-focused registry can
  expose `registry.search` over Tor; a curated lobe registry can refuse
  to index uncertified caps; a search engine can aggregate from
  multiple registry peers.
- Same signing + auditability story as everything else. No new
  authentication surface, no new attack class.

**Open questions before commit.**
- Reserved name prefix (`registry.*`?) or pure convention? Reserved is
  more discoverable; convention keeps the spec smaller.
- Index TTL — pull (registry refreshes by polling `describe_self`)
  vs push (peers re-announce). Pull is more honest; push scales better.
- Anti-spam: do we require the announcing peer to also respond to
  `ping` from the registry within the TTL? Probably yes.
- Does the planner get to call `registry.search` autonomously, or only
  when the user opts in? Privacy-relevant — registries can fingerprint
  queries.
- Default registry peers in the binary: zero, one curated, or a small
  bootstrap list under community governance? Decision blocks v1.0.

**Status.** Thinking-out-loud, not committed. Will graduate to a 🧭
item under v0.5 once the open questions narrow.

## v1.0 — protocol freeze 💭

- Protocol invariants frozen: 4 verbs, envelope shape, signature path
- Stable wire compatibility commitment
- Mobile clients (iOS / Android)
- WASM / subprocess generic bindings
- Multi-modal capabilities (image-gen, audio)

---

## Out of scope (explicit ⛔)

These have been considered and rejected for the foreseeable future. Re-open only with a new use case the spec cannot already serve.

- Central registry baked into the protocol (peer gossip is the default;
  optional registries are themselves capabilities — see reflection above)
- Streaming inside the protocol (use the local SSE bridge, not the wire)
- Pipeline orchestration as a protocol verb (planner composes locally)
- Session state at the protocol layer (every `invoke` is independent)
- Cross-conversation long-term memory in v0.x (per-conversation only)

---

## Open governance questions

To resolve before a public launch — not before v1.0:

- Synapse granularity (1:1 vs 1:lobe vs 1:capability)
- Economic model of any default registry
- Localisation of planners for multi-step pipelines
- Trademark posture on brand handles

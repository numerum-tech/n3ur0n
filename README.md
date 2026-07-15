<div align="center">

# N3UR0N

**A peer-to-peer network for publishing and invoking AI capabilities — no central authority, no vendor lock-in.**

[![CI](https://github.com/numerum-tech/n3ur0n/actions/workflows/ci.yml/badge.svg)](https://github.com/numerum-tech/n3ur0n/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org)

[Architecture](n3ur0n-architecture-v0.md) · [Use cases](USE_CASES.md) · [Capability manifests](n3ur0n-capability-manifest-v0.md) · [Blob protocol](n3ur0n-blob-protocol-v0.md) · [Roadmap](ROADMAP.md) · [Website](www/)

</div>

---

## What is N3UR0N?

N3UR0N is a distributed protocol where every participant runs a small **gateway instance** (a "neuron") that exposes one or more **AI capabilities** — a local LLM, an MCP server, an HTTP API, a prompted skill — to the network. Other neurons discover those capabilities by name, invoke them with a signed message, and route the result back. There is no broker, no registry, no central server.

Capabilities are not shipped with the binary. They are **TOML manifests** users author, store on their machine, and (optionally) advertise to peers. The runtime ships a small set of binding types (`prompt`, `mcp`, `http`); everything else is composition.

> **N3UR0N is not** a model, an inference engine, or a tool format. It is the connective tissue between models, tools, and the people running them.

Sample scenarios (public commons, company capability fabric, vendor↔partner edge) are sketched in [USE_CASES.md](USE_CASES.md).

### Why

- **No central authority.** Identity is `n3:` + Base32(SHA-256(Ed25519 pubkey)). Every message is signed. Anyone can verify; no one can revoke.
- **Capabilities are data, not code.** Add a skill by writing a `cap.toml`. No recompile, no plugin SDK.
- **Bring your own backend.** Anything that speaks **OpenAI-compatible** HTTP (Ollama, OpenAI, llama.cpp server, vLLM, …) or **MCP** works as a backend. Other vendors work when they expose one of those surfaces.
- **Two profiles, one core.** A desktop app (Tauri) for consumers; a headless server for publishers. Same Rust core.

## Quick start

Pre-built installers, GitHub Releases artefacts, GitHub Pages downloads, and a published `ghcr.io` image are **not available yet**. A release workflow exists (`.github/workflows/release.yml`); until the first `v*` tag ships artefacts, use **build from source** or **Docker Compose**.

### Server (from source)

```bash
cargo build --release -p n3ur0n-server
./target/release/n3ur0n init
./target/release/n3ur0n serve --port 4242 \
  --endpoint http://127.0.0.1:4242 \
  --backend ollama --openai-model llama3.1:8b
```

Open `http://localhost:4242/ui/` in a browser.

Manifest mode (hot-reload backends/caps under a config dir):

```bash
./target/release/n3ur0n serve --port 4242 \
  --endpoint http://127.0.0.1:4242 \
  --manifest-dir ~/.config/n3ur0n
```

### Desktop (from source)

Requires [Tauri 2 prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```bash
cargo run -p n3ur0n-desktop
# or:
cargo tauri dev --manifest-path crates/desktop/Cargo.toml
```

On first launch the app probes `http://localhost:11434` and, if Ollama answers, scaffolds a default `local_ollama` backend. Open the chat UI and type.

### Docker (Compose cluster)

```bash
docker compose -f docker/compose.yml up -d --build
# planner UI: http://localhost:4242/ui/
```

This builds the local `n3ur0n:dev` image from `docker/Dockerfile`. See `docker/compose.yml` for the multi-node layout and `docker/cluster-smoke.sh` for a smoke test.

## How it works

```
┌────────────┐   signed Ed25519 envelope   ┌────────────┐
│ neuron A   │ ──────────────────────────▶ │ neuron B   │
│ (user UI)  │     invoke(cap, args)       │ (backend)  │
└────────────┘ ◀────────────────────────── └────────────┘
                signed reply
```

Every message carries `sender_id`, `recipient_id`, `timestamp`, `nonce`, `verb`, `payload`, `sender_public_key`, `signature`. Signatures cover the JCS canonical form (RFC 8785). Verification is pure: `hash(public_key) == sender_id` binds key to identity without a registry.

Protocol verbs:

- `describe_self` — list capabilities + endpoint
- `get_known_peers` — share peer directory
- `ping` — liveness
- `invoke` — call a capability
- `blob_ticket` — authorize blob HTTP ops on `/n3ur0n/v0/blobs` (never dispatched via `/messages`)

Each instance also runs an optional **planner**. By default (**PlanExec**) a local LLM compiles a typed plan, a deterministic executor runs capability invokes (bounded parallelism), then a final LLM call reflects a user-facing reply. A **direct chat** mode skips planning for a single LLM call per message.

See [`n3ur0n-architecture-v0.md`](n3ur0n-architecture-v0.md) for the full protocol spec and [`n3ur0n-blob-protocol-v0.md`](n3ur0n-blob-protocol-v0.md) for blobs.

## Capability manifests

A capability is a TOML file. Example:

```toml
# caps/translator-fr-en.toml
[manifest]
version = "0.1"

[descriptor]
name = "translator-fr-en"
version = "1.2.0"
description = "Translate French to English"
mode = "free"
languages = ["fr", "en"]

[descriptor.schema_in]
type = "object"
required = ["text"]
properties = { text = { type = "string" } }

[descriptor.schema_out]
type = "object"
required = ["translation"]
properties = { translation = { type = "string" } }

[[descriptor.examples]]
user_intent = "Translate 'bonjour' to English"
args = { text = "bonjour" }
expected_output = { translation = "hello" }

[binding]
type = "prompt"
backend = "local_ollama"

[binding.prompt]
system_prompt = "Translate the user input from French to English. Return JSON: {\"translation\": \"...\"}."
user_template = "{{args.text}}"
parameters    = { temperature = 0.0 }
output_parser = "json"
```

In **manifest mode** (desktop, or server `--manifest-dir`), drop files under `<config>/backends/` and `<config>/caps/`. Capability CRUD hot-reloads without restart; backend CRUD reloads the backends registry and rebinds caps. See [`n3ur0n-capability-manifest-v0.md`](n3ur0n-capability-manifest-v0.md) for the conceptual spec (the on-disk layout shipped in 0.3+ diverges slightly — prefer this example and the Settings UI).

## Project status

**v0.4.0** (tagged release line) — open-source scaffolding, EN/FR UI, RBAC phase 1, full capability composer (`prompt` / `mcp` / `http`), backend hot-reload, `AccessMode::Private`. Wire protocol `n3ur0n/0.3` unchanged.

**On `main` beyond 0.4.0 (see [CHANGELOG](CHANGELOG.md) Unreleased)** — direct chat mode; hash-addressed blob layer + Files panel + message attachments; planner context control and durable plan-run journaling work in progress.

**Not yet** — published release binaries / GHCR image / Pages download site; public registry; lobe governance; marketplace; WASM bindings; OS keychain for API keys; mobile clients; CLI i18n.

See [ROADMAP.md](ROADMAP.md) for the milestone tracker.

## Contributing

We welcome contributions. Read [CONTRIBUTING.md](CONTRIBUTING.md) first — there are a few hard rules around protocol invariants (signed envelopes, JCS canonicalization) that must not regress.

By participating you agree to abide by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

To report a vulnerability, see [SECURITY.md](SECURITY.md). Do not file public issues for security bugs.

## License

Apache License 2.0 — see [LICENSE](LICENSE).

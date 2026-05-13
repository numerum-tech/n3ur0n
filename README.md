<div align="center">

# N3UR0N

**A peer-to-peer network for publishing and invoking AI capabilities — no central authority, no vendor lock-in.**

[![CI](https://github.com/n3ur0n/n3ur0n/actions/workflows/ci.yml/badge.svg)](https://github.com/n3ur0n/n3ur0n/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org)

[Download](https://n3ur0n.github.io/n3ur0n/) · [Documentation](docs/) · [Architecture](n3ur0n-architecture-v0.md) · [Capability manifests](n3ur0n-capability-manifest-v0.md)

</div>

---

## What is N3UR0N?

N3UR0N is a distributed protocol where every participant runs a small **gateway instance** (a "neuron") that exposes one or more **AI capabilities** — a local LLM, an MCP server, an HTTP API, a prompted skill — to the network. Other neurons discover those capabilities by name, invoke them with a signed message, and route the result back. There is no broker, no registry, no central server.

Capabilities are not shipped with the binary. They are **TOML manifests** users author, store on their machine, and (optionally) advertise to peers. The runtime ships a small set of binding types (`prompt`, `mcp`, `http`); everything else is composition.

> **N3UR0N is not** a model, an inference engine, or a tool format. It is the connective tissue between models, tools, and the people running them.

### Why

- **No central authority.** Identity is `n3:` + Base32(SHA-256(Ed25519 pubkey)). Every message is signed. Anyone can verify; no one can revoke.
- **Capabilities are data, not code.** Add a skill by writing a `cap.toml`. No recompile, no plugin SDK.
- **Bring your own backend.** Local Ollama, OpenAI, Anthropic, Mistral, llama.cpp, vLLM, your own MCP server — anything that speaks OpenAI-compatible HTTP or MCP works as a backend.
- **Two profiles, one core.** A desktop app (Tauri) for consumers; a headless server for publishers. Same Rust core.

## Quick start

### Desktop app

Download the latest installer for your OS from the [releases page](https://github.com/n3ur0n/n3ur0n/releases/latest) or the [project site](https://n3ur0n.github.io/n3ur0n/).

- **macOS** — `n3ur0n_<version>_universal.dmg`
- **Windows** — `n3ur0n_<version>_x64-setup.exe`
- **Linux** — `n3ur0n_<version>_amd64.AppImage` or `.deb`

On first launch the app auto-detects a local Ollama server and scaffolds a default backend. Open the chat tab and type — that is it.

### Server (Linux)

```bash
# Pre-built binary (replace VERSION + ARCH)
curl -L -o n3ur0n \
  https://github.com/n3ur0n/n3ur0n/releases/latest/download/n3ur0n-linux-x86_64
chmod +x n3ur0n

./n3ur0n init                                    # generate identity + db
./n3ur0n serve --port 4242 \
  --endpoint http://your.host:4242 \
  --backend ollama --openai-model llama3.1:8b
```

Open `http://localhost:4242/ui/` in a browser.

### Docker

```bash
docker run -d --name n3ur0n \
  -p 4242:4242 \
  -v n3ur0n_data:/var/lib/n3ur0n \
  ghcr.io/n3ur0n/n3ur0n:latest
```

### Build from source

```bash
# Workspace check
cargo check --workspace

# Server binary
cargo build --release -p n3ur0n-server
./target/release/n3ur0n --help

# Desktop (requires Tauri prerequisites for your OS:
# https://tauri.app/start/prerequisites/)
cargo tauri dev   --manifest-path crates/desktop/Cargo.toml
cargo tauri build --manifest-path crates/desktop/Cargo.toml
```

Rust 1.85+ (edition 2024) required.

## How it works

```
┌────────────┐   signed Ed25519 envelope   ┌────────────┐
│ neuron A   │ ──────────────────────────▶ │ neuron B   │
│ (user UI)  │     invoke(cap, args)       │ (backend)  │
└────────────┘ ◀────────────────────────── └────────────┘
                signed reply               
```

Every message carries `sender_id`, `recipient_id`, `timestamp`, `nonce`, `verb`, `payload`, `sender_public_key`, `signature`. Signatures cover the JCS canonical form (RFC 8785). Verification is pure: `hash(public_key) == sender_id` binds key to identity without a registry.

Four protocol verbs total:
- `describe_self` — list capabilities + endpoint
- `get_known_peers` — share peer directory
- `ping` — liveness
- `invoke` — call a capability

Each instance also runs an optional **planner** that turns user prompts into capability calls. By default it uses a local LLM with native tool-calling.

See [`n3ur0n-architecture-v0.md`](n3ur0n-architecture-v0.md) for the full protocol spec.

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

Drop it in `<config>/caps/`. The runtime picks it up; no restart needed. See [`n3ur0n-capability-manifest-v0.md`](n3ur0n-capability-manifest-v0.md) for the spec.

## Project status

**v0.3 (current)** — capability manifests, hot-reload, master-detail Settings UI, desktop app, Docker image. The protocol is stable enough to play with; cryptographic primitives and signing are non-negotiable.

**Not yet** — public registry, lobe governance, marketplace, WASM bindings, OS keychain integration, mobile clients.

See [WORK_PLAN.md](WORK_PLAN.md) for the roadmap.

## Contributing

We welcome contributions. Read [CONTRIBUTING.md](CONTRIBUTING.md) first — there are a few hard rules around protocol invariants (signed envelopes, JCS canonicalization) that must not regress.

By participating you agree to abide by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

To report a vulnerability, see [SECURITY.md](SECURITY.md). Do not file public issues for security bugs.

## License

Apache License 2.0 — see [LICENSE](LICENSE).

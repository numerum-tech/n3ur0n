# crates/desktop

Tauri 2 shell for the **consumer profile** of N3UR0N: a local-only client
app for connecting to local + remote LLMs, MCP servers, HTTP APIs, and
remote N3UR0N peer gateways under one signed protocol.

## Architecture

```
┌─ Tauri window ──────────────────────────────────────────┐
│  webview → http://127.0.0.1:<random_port>/ui/           │
└────────────────────┬────────────────────────────────────┘
                     │ (loopback only — never bound to 0.0.0.0)
┌────────────────────▼────────────────────────────────────┐
│  Embedded axum router (n3ur0n_server::http::app)        │
│  + manifest-mode Node (n3ur0n_node)                     │
│  + identity / SQLite / planner / bindings               │
└─────────────────────────────────────────────────────────┘
```

The desktop binary is a **strict consumer**: no public listener, no peer
endpoint advertised by default. Outbound calls to remote N3UR0N peers
(`peer_client::send_signed`) work because they don't require an inbound
listener. Calls back from those peers won't reach this node — by design.

## App config dir

| OS | Path |
|---|---|
| macOS | `~/Library/Application Support/n3ur0n/` |
| Linux | `~/.config/n3ur0n/` |
| Windows | `%APPDATA%\n3ur0n\` |

Contents:
- `keys.json` — Ed25519 keypair (0600 perms on Unix).
- `n3ur0n.sqlite` — peer directory + nonce table + conversations.
- `backends/*.toml` — backend manifests (LLM endpoints, MCP servers, HTTP bases).
- `caps/*.toml` — capability manifests (skills exposed by this client).

## First launch

1. Generates an identity if none exists.
2. Probes `http://localhost:11434/v1/models`. If Ollama answers, a
   default `backends/local_ollama.toml` manifest is scaffolded so the
   planner has something to use immediately.
3. Starts the embedded server on a random loopback port.
4. Opens the Tauri window pointing at `http://127.0.0.1:<port>/ui/`.

## Run (dev)

```bash
cargo run -p n3ur0n-desktop
```

## Build

```bash
cargo build -p n3ur0n-desktop --release
# binary at target/release/n3ur0n-desktop
```

Native bundles (`.dmg`, `.app`, `.msi`, `.AppImage`, `.deb`) require the
Tauri CLI:

```bash
cargo install tauri-cli@^2
cargo tauri build
```

## Status (v0.3 alpha)

- [x] Scaffold + loopback embed
- [x] Identity in OS config dir
- [x] Ollama auto-detect on first launch
- [ ] Settings tab (form-driven backend CRUD)
- [ ] Planner runtime auto-wired
- [ ] Cap composer (form → manifest)
- [ ] OS keychain for api_key storage
- [ ] Signed app bundles + auto-updater

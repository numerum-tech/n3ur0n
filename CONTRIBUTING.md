# Contributing to N3UR0N

Thanks for your interest. This project is in active development and we welcome patches, bug reports, capability manifests, and protocol feedback.

## Before you start

1. Read [`n3ur0n-architecture-v0.md`](n3ur0n-architecture-v0.md) and [`project-tech-stack.md`](project-tech-stack.md). The architecture is opinionated and the choices are firm, not suggestions.
2. Search existing [issues](https://github.com/n3ur0n/n3ur0n/issues) and [pull requests](https://github.com/n3ur0n/n3ur0n/pulls) before opening a new one.
3. For non-trivial changes, open an issue first so we can align on scope before you write code.

## Hard rules (cannot regress)

These are protocol invariants. PRs that break any of them will be rejected on sight.

- **Every message is signed.** Ed25519, signature covers the JCS canonical form (RFC 8785). Never serialise envelopes by hand.
- **Identity = `n3:` + Base32(SHA-256(pubkey)).** No registries, no aliases at the protocol layer.
- **Four verbs only.** `describe_self`, `get_known_peers`, `ping`, `invoke`. No streaming, no sessions, no pipeline orchestration in the protocol.
- **Three meta verbs are always free.** `describe_self`, `get_known_peers`, `ping` cannot be restricted.
- **Crate dependency hierarchy.** `core` knows no HTTP/SQL. `storage` knows no HTTP. `adapters` knows no SQL. `node` knows no axum. See `CLAUDE.md` for the full table.

## Development setup

```bash
# Rust 1.85+ stable, edition 2024
rustup install stable
rustup component add clippy rustfmt

# Build + test
cargo check --workspace
cargo test  --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

For desktop work see [Tauri prerequisites](https://tauri.app/start/prerequisites/).

For the cluster smoke test:

```bash
docker compose -f docker/compose.yml up -d --build
bash docker/cluster-smoke.sh
```

## Pull requests

- One concern per PR. Refactors live in their own PR.
- Tests required for new behaviour. Bug fixes ship with a regression test.
- Public APIs need rustdoc with a `//!` module-level comment explaining the layer.
- Commit messages: imperative mood, scoped prefix (`feat(planner):`, `fix(server):`, `docs:`), 70-char title.
- New dependencies: justify in the PR description. We prefer adding code to adding crates.
- Frontend changes: run the dev server and verify in a browser before claiming done. Type checks are not feature tests.

## Capability manifests

Adding a new built-in cap? Drop a TOML in `crates/server/seed/caps/` (if we ever have a seed dir) or document it in the PR. Users author their own under `<config>/caps/`.

## Reporting bugs

Use the bug template at [.github/ISSUE_TEMPLATE/bug.yml](.github/ISSUE_TEMPLATE/bug.yml). Include:

- N3UR0N version (`n3ur0n keys` shows it)
- OS + arch
- The exact command or UI action
- Logs at `RUST_LOG=debug` if possible

## Security

See [SECURITY.md](SECURITY.md). Never file a public issue for a vulnerability.

## License

By submitting a contribution you agree it is licensed under the [Apache License 2.0](LICENSE).

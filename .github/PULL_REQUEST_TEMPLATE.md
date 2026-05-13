<!-- Thanks for contributing! -->

## Summary

<!-- 1–3 bullet points describing the change and the motivation. -->

## What changed

- 

## Testing

<!-- How did you verify this? `cargo test`, manual UI run, cluster smoke, etc. -->

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --all -- --check` passes
- [ ] If UI/protocol: I ran it end-to-end and verified the user-visible behaviour
- [ ] If new public API: rustdoc added

## Protocol invariants

<!-- Tick the ones that apply. If your PR violates an invariant, justify it here. -->

- [ ] No change to the four verbs (`describe_self`, `get_known_peers`, `ping`, `invoke`)
- [ ] Envelope signing path untouched
- [ ] Identity scheme (`n3:` + Base32(SHA-256(pubkey))) untouched

## Related issues

Closes #

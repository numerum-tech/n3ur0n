# crates/desktop

Tauri 2 shell for the N3UR0N consumer / hybrid client.

**Not yet scaffolded.** Initialise it once `frontend/` builds and the Tauri CLI is
available:

```bash
# from the workspace root
pnpm --filter frontend build      # produces frontend/build/
pnpm dlx @tauri-apps/cli init     # writes tauri.conf.json + src-tauri layout
```

Then re-add `crates/desktop` to `members = [...]` in the root `Cargo.toml`
and remove it from `exclude`.

Spec reference: `project-tech-stack.md` §9.

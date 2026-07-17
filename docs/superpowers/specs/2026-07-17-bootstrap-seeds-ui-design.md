# Design: Settings bootstrap seeds

**Date:** 2026-07-17  
**Status:** approved — simplified  

## Goal

Wire public / configured bootstrap peers without a second “seed list” UI.

## Behaviour

- **One UI path:** Settings → Gateways → **Add gateway**.
- Button **Use seed.n3ur0n.net** fills the endpoint field before save.
- Prefill endpoint from saved `bootstrap.toml` or `N3UR0N_BOOTSTRAP_PEERS` when present.
- On successful add: peer enters the directory **and** the endpoint is merged into `bootstrap.toml` (startup rediscovery).
- Gateway **detail** shows whether the peer is a startup seed.
  - **Remove from seeds** — drop endpoint from `bootstrap.toml` only (peer stays).
  - **Remove gateway** — delete peer row only (seed list unchanged).
- **Startup** (desktop / `serve` when CLI bootstrap empty): load `bootstrap.toml`; CLI/env still override when set.

## API (unchanged)

`GET|PUT /api/v0/settings/bootstrap` — used by the add-gateway form and by headless startup.

## Out of scope

Separate Gateways “Bootstrap seeds” card (removed).

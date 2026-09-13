#!/usr/bin/env bash
# Web UI smoke — drives the embedded UI in a real browser and screenshots it.
# Usage: scripts/ui-smoke.sh [port]
# Env:   UI_SMOKE_OUT (default target/ui-smoke), CHROME (path to Chrome)
#
# Why not Playwright: the UI ships as plain ES modules with no package.json and
# no bundler (see CLAUDE.md). A screenshot harness is no reason to introduce
# one — Chrome speaks the DevTools Protocol and Node 22 has a global WebSocket,
# which is the whole dependency list.
#
# Uses the DEBUG binary on purpose: rust-embed re-reads crates/server/ui/ from
# disk in debug builds, so editing a .js/.css and re-running needs no rebuild.
# A release binary serves the assets frozen at build time and will show you a
# stale page.
set -euo pipefail

PORT="${1:-4399}"
CDP_PORT="${CDP_PORT:-9222}"
OUT="${UI_SMOKE_OUT:-target/ui-smoke}"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$ROOT/target/ui-smoke-run"

[ -x "$CHROME" ] || { echo "Chrome not found at: $CHROME (override with \$CHROME)" >&2; exit 1; }

cleanup() {
    [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null || true
    [ -n "${CHROME_PID:-}" ] && kill "$CHROME_PID" 2>/dev/null || true
}
trap cleanup EXIT

cd "$ROOT"
rm -rf "$WORK"; mkdir -p "$WORK" "$OUT"

cargo build -p n3ur0n-server
./target/debug/n3ur0n init --config-dir "$WORK" >/dev/null
RUST_LOG=warn ./target/debug/n3ur0n serve --config-dir "$WORK" --port "$PORT" \
    --endpoint "http://localhost:$PORT" > "$WORK/serve.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 40); do
    curl -sf "http://localhost:$PORT/api/v0/health" >/dev/null && break
    sleep 0.25
done

COOKIE=$(curl -s -i -X POST "http://localhost:$PORT/api/v0/auth/bootstrap" \
    -H 'content-type: application/json' \
    -d '{"username":"smoke","password":"smoke123"}' \
    | grep -i '^set-cookie: n3ur0n_session=' \
    | sed 's/.*n3ur0n_session=\([^;]*\).*/\1/' | tr -d '\r')
[ -n "$COOKIE" ] || { echo "could not bootstrap a session" >&2; exit 1; }

"$CHROME" --headless=new --disable-gpu --remote-debugging-port="$CDP_PORT" \
    --user-data-dir="$WORK/chrome" --window-size=1440,960 \
    --no-first-run --no-default-browser-check about:blank > "$WORK/chrome.log" 2>&1 &
CHROME_PID=$!

for _ in $(seq 1 40); do
    curl -sf "http://127.0.0.1:$CDP_PORT/json/version" >/dev/null && break
    sleep 0.25
done

printf 'n3ur0n ui smoke sample\n' > "$WORK/sample.txt"
CDP_PORT="$CDP_PORT" node "$ROOT/scripts/ui-smoke.mjs" "http://localhost:$PORT" "$COOKIE" "$OUT" "$WORK/sample.txt"
echo "screenshots in $OUT"

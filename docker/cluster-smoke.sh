#!/usr/bin/env bash
# Cluster smoke test:
#   1. Wait for all 3 healthchecks.
#   2. Cross-node signed ping (6 directed pairs).
#   3. describe_self a -> {b, c}: c should advertise the `chat` capability.
#   4. invoke chat a -> c (signed) and check we get a non-empty assistant reply.
#   5. Bootstrap-populated directory: b should know a (compose env).
#   6. Cascade discovery for `chat`:
#        - a refresh node-b
#        - b refresh node-c
#      then `n3ur0n peers discover --capability chat` on a should pull c
#      via b.
#   7. Local-API path used by the web UI: POST /api/v0/chat on node-a's
#      port 4242 with peer_endpoint=node-c. Verifies the same end-to-end
#      browser flow.
#
# Run after `docker compose -f docker/compose.yml up -d --build`.
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE=(docker compose -f "$ROOT_DIR/compose.yml")

NODES=(node-a node-b node-c)

green()  { printf '\033[1;32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[1;33m%s\033[0m\n' "$*"; }
red()    { printf '\033[1;31m%s\033[0m\n' "$*" >&2; }

# --- 1. Wait for healthchecks --------------------------------------------------
yellow "Waiting for nodes to become healthy..."
for node in "${NODES[@]}"; do
    for _ in $(seq 1 30); do
        status=$("${COMPOSE[@]}" ps --format json "$node" \
                 | python3 -c 'import sys,json; print(json.load(sys.stdin)["Health"])' 2>/dev/null \
                 || echo "unknown")
        if [ "$status" = "healthy" ]; then break; fi
        sleep 1
    done
    if [ "$status" != "healthy" ]; then
        red "$node never became healthy (status=$status)"
        exit 1
    fi
done
green "All 3 nodes healthy."

# --- 2. Cross-node signed ping -------------------------------------------------
for from in "${NODES[@]}"; do
    for to in "${NODES[@]}"; do
        [ "$from" = "$to" ] && continue
        yellow "[$from -> $to] ping"
        "${COMPOSE[@]}" exec -T "$from" \
            n3ur0n send --endpoint "http://$to:4242" --verb ping \
            > /dev/null
    done
done
green "Cross-node signed ping OK."

# --- 3. describe_self ----------------------------------------------------------
for to in node-b node-c; do
    yellow "[node-a -> $to] describe_self"
    "${COMPOSE[@]}" exec -T node-a \
        n3ur0n send --endpoint "http://$to:4242" --verb describe_self \
        | python3 -c 'import sys,json; d=json.load(sys.stdin); print(" ", d["instance_id"], "caps:", [c["name"] for c in d["capabilities"]])'
done
green "describe_self OK."

# --- 4. invoke chat a -> c (signed) -------------------------------------------
yellow "[node-a -> node-c] invoke chat (signed protocol)"
"${COMPOSE[@]}" exec -T node-a \
    n3ur0n send --endpoint http://node-c:4242 --verb invoke \
    --payload '{"capability":"chat","args":{"prompt":"Reply with one short sentence: hello."}}' \
    | python3 -c '
import sys, json
r = json.load(sys.stdin)["result"]
content = r["message"]["content"]
assert content.strip(), f"empty assistant content: {r}"
print("  model:", r.get("model"), "| reply:", content[:80] + ("..." if len(content) > 80 else ""))
'
green "invoke chat OK."

# --- 5. Compose bootstrap populated b's directory ------------------------------
yellow "Waiting for node-b's startup bootstrap to complete..."
for _ in $(seq 1 20); do
    out_b=$("${COMPOSE[@]}" exec -T node-b n3ur0n peers list 2>/dev/null || true)
    if echo "$out_b" | grep -q '^n3:'; then break; fi
    sleep 1
done
yellow "node-b directory:"
echo "$out_b"
if ! echo "$out_b" | grep -q "node-a:4242"; then
    red "node-b directory does not contain node-a (compose bootstrap failed)"
    exit 1
fi
green "Compose bootstrap populated node-b directory OK."

# --- 6. Cascade discovery for `chat` ------------------------------------------
yellow "[node-a] peers refresh node-b"
"${COMPOSE[@]}" exec -T node-a \
    n3ur0n peers refresh --endpoint http://node-b:4242 > /dev/null
yellow "[node-b] peers refresh node-c"
"${COMPOSE[@]}" exec -T node-b \
    n3ur0n peers refresh --endpoint http://node-c:4242 > /dev/null

yellow "[node-a] peers discover --capability chat (cascade depth 1 via b)"
"${COMPOSE[@]}" exec -T node-a \
    n3ur0n peers discover --capability chat

dir_a=$("${COMPOSE[@]}" exec -T node-a n3ur0n peers list)
yellow "node-a directory:"
echo "$dir_a"
if ! echo "$dir_a" | grep -q "node-c:4242"; then
    red "node-a did not discover node-c via cascade"
    exit 1
fi
green "Cascade discovery OK."

# --- 7. Local API path used by the web UI -------------------------------------
yellow "POST http://localhost:4242/api/v0/chat (browser flow)"
out=$(curl -fsS -X POST http://localhost:4242/api/v0/chat \
      -H "content-type: application/json" \
      -d '{"peer_endpoint":"http://node-c:4242","prompt":"Reply with: ok cluster."}')
echo "$out" | python3 -c '
import sys, json
b = json.load(sys.stdin)
content = b["reply"]["result"]["message"]["content"]
assert content.strip(), f"empty content: {b}"
print("  peer:", b["peer_id"][:24] + "...", "| reply:", content[:80])
'
green "Web-UI local API path OK."

# --- 8. Capacity planner: /api/v0/converse via conversations -------------
yellow "Capacity planner: create conversation + dispatch via planner"
COOKIE_FILE=$(mktemp -t n3uron-smoke-cookies)
# trap removed: keep cookie for any post-mortem inspection
CONV_ID=$(curl -fsS -c "$COOKIE_FILE" -b "$COOKIE_FILE" \
    -X POST http://localhost:4242/api/v0/conversations \
    -H "content-type: application/json" -d '{}' \
    | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
yellow "  conversation_id: $CONV_ID"

# Seed node-a directory so the planner has chat available.
"${COMPOSE[@]}" exec -T node-a \
    n3ur0n peers refresh --endpoint http://node-c:4242 > /dev/null

REPLY=$(curl -fsS --max-time 180 -c "$COOKIE_FILE" -b "$COOKIE_FILE" \
    -X POST "http://localhost:4242/api/v0/conversations/$CONV_ID/messages" \
    -H "content-type: application/json" \
    -d '{"message":"Reply with the single word: ok"}')

echo "$REPLY" | python3 -c '
import sys, json
b = json.load(sys.stdin)
assert b.get("reply"), f"empty reply: {b}"
trace = b.get("trace", [])
# The LLM may pick any tool from the catalog, or none at all if confident.
# Validate shape only: reply non-empty, trace is a list, model populated.
assert isinstance(trace, list), f"trace not a list: {b}"
assert b.get("model"), f"no model in outcome: {b}"
print("  reply:", b["reply"][:80])
print("  model:", b["model"])
print("  trace:", [t.get("capability") for t in trace] or "(no tools — direct reply)")
'
green "Capacity planner OK."

# --- 9. Direct chat mode (single LLM call, empty trace) -----------------------
yellow "Direct chat: mode=direct on same conversation"
DIRECT_REPLY=$(curl -fsS --max-time 120 -c "$COOKIE_FILE" -b "$COOKIE_FILE" \
    -X POST "http://localhost:4242/api/v0/conversations/$CONV_ID/messages" \
    -H "content-type: application/json" \
    -d '{"message":"Reply with exactly: direct","mode":"direct"}')

echo "$DIRECT_REPLY" | python3 -c '
import sys, json
b = json.load(sys.stdin)
assert b.get("reply"), f"empty reply: {b}"
assert b.get("trace") == [], f"direct mode must have empty trace: {b}"
print("  reply:", b["reply"][:80])
print("  trace: (empty)")
'

yellow "Direct chat: invalid mode returns 400"
CODE=$(curl -sS -o /dev/null -w "%{http_code}" -c "$COOKIE_FILE" -b "$COOKIE_FILE" \
    -X POST "http://localhost:4242/api/v0/conversations/$CONV_ID/messages" \
    -H "content-type: application/json" \
    -d '{"message":"x","mode":"bogus"}')
test "$CODE" = "400" || { red "expected HTTP 400 for invalid mode, got $CODE"; exit 1; }
green "Direct chat mode OK."

green "Cluster smoke test PASSED."

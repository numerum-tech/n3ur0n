#!/usr/bin/env bash
# Cluster smoke test:
#   1. Wait for all 3 healthchecks.
#   2. Cross-node signed ping (6 directed pairs).
#   3. describe_self a -> {b, c}.
#   4. invoke echo a -> c.
#   5. Compose-driven bootstrap: b should know a (compose sets
#      N3UR0N_BOOTSTRAP_PEERS=http://node-a:4242 on node-b).
#   6. Manual cascade discovery topology:
#        - a refresh node-b   (a learns b)
#        - b refresh node-c   (b's directory becomes {a, c})
#      then `n3ur0n peers discover --capability echo` on a
#      should reach c via b's directory.
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

# --- 4. invoke -----------------------------------------------------------------
yellow "[node-a -> node-c] invoke echo"
"${COMPOSE[@]}" exec -T node-a \
    n3ur0n send --endpoint http://node-c:4242 --verb invoke \
    --payload '{"capability":"echo","args":{"hello":"world"}}' \
    | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["result"]=={"hello":"world"}, r; print(" ", r)'
green "invoke OK."

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

# --- 6. Cascade discovery topology --------------------------------------------
# Topology required to exercise depth-1 cascade:
#   a knows {b}
#   b knows {a, c}
yellow "[node-a] peers refresh node-b"
"${COMPOSE[@]}" exec -T node-a \
    n3ur0n peers refresh --endpoint http://node-b:4242 > /dev/null
yellow "[node-b] peers refresh node-c"
"${COMPOSE[@]}" exec -T node-b \
    n3ur0n peers refresh --endpoint http://node-c:4242 > /dev/null

yellow "[node-a] peers discover --capability echo (cascade depth 1 via b)"
"${COMPOSE[@]}" exec -T node-a \
    n3ur0n peers discover --capability echo

dir_a=$("${COMPOSE[@]}" exec -T node-a n3ur0n peers list)
yellow "node-a directory:"
echo "$dir_a"
if ! echo "$dir_a" | grep -q "node-b:4242"; then
    red "node-a does not know node-b after refresh"
    exit 1
fi
if ! echo "$dir_a" | grep -q "node-c:4242"; then
    red "node-a did not discover node-c via cascade"
    exit 1
fi
green "Cascade discovery OK."

green "Cluster smoke test PASSED."

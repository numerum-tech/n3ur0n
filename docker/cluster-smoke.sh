#!/usr/bin/env bash
# Cluster smoke test: walks the 3-node mesh and exercises ping +
# describe_self via the signed protocol path.
#
# Run after `docker compose -f docker/compose.yml up -d --build` and once
# all healthchecks are green.
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE=(docker compose -f "$ROOT_DIR/compose.yml")

NODES=(node-a node-b node-c)

green()  { printf '\033[1;32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[1;33m%s\033[0m\n' "$*"; }
red()    { printf '\033[1;31m%s\033[0m\n' "$*" >&2; }

# 1. Wait for all healthchecks.
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

# 2. Each node pings the other two via the signed protocol.
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

# 3. describe_self from node-a -> {b, c}: dump capabilities.
for to in node-b node-c; do
    yellow "[node-a -> $to] describe_self"
    "${COMPOSE[@]}" exec -T node-a \
        n3ur0n send --endpoint "http://$to:4242" --verb describe_self \
        | python3 -c 'import sys,json; d=json.load(sys.stdin); print(" ", d["instance_id"], "caps:", [c["name"] for c in d["capabilities"]])'
done
green "describe_self OK."

# 4. invoke `echo` from node-a -> node-c.
yellow "[node-a -> node-c] invoke echo"
"${COMPOSE[@]}" exec -T node-a \
    n3ur0n send --endpoint http://node-c:4242 --verb invoke \
    --payload '{"capability":"echo","args":{"hello":"world"}}' \
    | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["result"]=={"hello":"world"}, r; print(" ", r)'
green "invoke OK."

green "Cluster smoke test PASSED."

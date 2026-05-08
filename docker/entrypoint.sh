#!/usr/bin/env sh
# Auto-init identity on first boot, then serve.
set -eu

CONFIG_DIR="${N3UR0N_CONFIG_DIR:-/var/lib/n3ur0n}"
PORT="${N3UR0N_PORT:-4242}"

if [ ! -f "$CONFIG_DIR/keys.json" ]; then
    echo "[entrypoint] no identity at $CONFIG_DIR/keys.json — initialising"
    /usr/local/bin/n3ur0n init --config-dir "$CONFIG_DIR"
fi

ENDPOINT_FLAG=""
if [ -n "${N3UR0N_ENDPOINT:-}" ]; then
    ENDPOINT_FLAG="--endpoint $N3UR0N_ENDPOINT"
fi

# shellcheck disable=SC2086
exec /usr/local/bin/n3ur0n serve --config-dir "$CONFIG_DIR" --port "$PORT" $ENDPOINT_FLAG

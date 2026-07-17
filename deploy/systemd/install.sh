#!/usr/bin/env bash
# Install / update the N3UR0N seed systemd unit on a Linux VPS.
#
# Prerequisites:
#   cargo build --release -p n3ur0n-server
# Run from anywhere; paths resolve relative to this script.
#
# Usage:
#   ./deploy/systemd/install.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/target/release/n3ur0n"
UNIT_SRC="$ROOT/deploy/systemd/n3ur0n-seed.service"
ENV_SRC="$ROOT/deploy/systemd/n3ur0n-seed.env.example"
ENV_DST="/etc/n3ur0n/seed.env"

if [[ ! -x "$BIN" ]]; then
  echo "error: missing binary: $BIN" >&2
  echo "build first: cargo build --release -p n3ur0n-server" >&2
  exit 1
fi

if [[ ! -f "$UNIT_SRC" || ! -f "$ENV_SRC" ]]; then
  echo "error: deploy files missing under $ROOT/deploy/systemd/" >&2
  exit 1
fi

# 1. User + dirs
if ! id -u n3ur0n &>/dev/null; then
  sudo useradd --system --home /var/lib/n3ur0n --shell /usr/sbin/nologin n3ur0n
fi
sudo mkdir -p /var/lib/n3ur0n /etc/n3ur0n
sudo chown -R n3ur0n:n3ur0n /var/lib/n3ur0n

# 2. Binary
sudo install -m 755 "$BIN" /usr/local/bin/n3ur0n

# 3. Unit + env (do not clobber a tuned seed.env)
sudo install -m 644 "$UNIT_SRC" /etc/systemd/system/n3ur0n-seed.service
if [[ ! -f "$ENV_DST" ]]; then
  sudo install -m 644 "$ENV_SRC" "$ENV_DST"
  echo "wrote $ENV_DST — edit N3UR0N_ENDPOINT before relying on describe_self ads"
else
  echo "keeping existing $ENV_DST"
fi

# 4. Enable + start
sudo systemctl daemon-reload
sudo systemctl enable --now n3ur0n-seed

echo
echo "status:"
sudo systemctl --no-pager --full status n3ur0n-seed || true
echo
echo "health (local): curl -fsS http://127.0.0.1:4242/n3ur0n/v0/health"
echo "logs:           journalctl -u n3ur0n-seed -f"

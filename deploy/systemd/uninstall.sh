#!/usr/bin/env bash
# Tear down the N3UR0N seed systemd install (inverse of install.sh).
#
# Default: stop service, remove unit + /usr/local/bin/n3ur0n.
# Keeps identity/data (/var/lib/n3ur0n) and /etc/n3ur0n/seed.env.
#
# Usage:
#   ./deploy/systemd/uninstall.sh           # service + binary only
#   ./deploy/systemd/uninstall.sh --purge   # also remove env, data, system user

set -euo pipefail

PURGE=0
for arg in "$@"; do
  case "$arg" in
    --purge) PURGE=1 ;;
    -h|--help)
      sed -n '2,12p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *)
      echo "error: unknown option: $arg (try --purge)" >&2
      exit 1
      ;;
  esac
done

echo "stopping n3ur0n-seed (if present)…"
sudo systemctl disable --now n3ur0n-seed 2>/dev/null || true
sudo rm -f /etc/systemd/system/n3ur0n-seed.service
sudo systemctl daemon-reload
sudo systemctl reset-failed n3ur0n-seed 2>/dev/null || true

if [[ -e /usr/local/bin/n3ur0n ]]; then
  sudo rm -f /usr/local/bin/n3ur0n
  echo "removed /usr/local/bin/n3ur0n"
fi

if [[ "$PURGE" -eq 1 ]]; then
  echo "purge: removing config, data, and system user…"
  sudo rm -f /etc/n3ur0n/seed.env
  sudo rmdir /etc/n3ur0n 2>/dev/null || true
  sudo rm -rf /var/lib/n3ur0n
  if id -u n3ur0n &>/dev/null; then
    sudo userdel n3ur0n 2>/dev/null || sudo userdel --remove n3ur0n 2>/dev/null || true
  fi
  echo "purge complete (keys/db gone)"
else
  echo "kept /var/lib/n3ur0n and /etc/n3ur0n/seed.env (pass --purge to delete)"
fi

echo "uninstall done"

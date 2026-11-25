#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$HOME/release" "$HOME/reports" "$SUPA_DEV_HOME"
cd "$HOME/release"
release="https://github.com/Supa-Doge/supa-dev/releases/download/v${SUPA_TEST_VERSION:-0.1.2}"
curl --fail --location --retry 3 "$release/supa-dev-linux-x86_64" -o supa-dev
curl --fail --location --retry 3 "$release/SHA256SUMS" -o SHA256SUMS
expected="$(awk '$2 == "supa-dev-linux-x86_64" {print $1}' SHA256SUMS)"
printf '%s  supa-dev\n' "$expected" | sha256sum --check
chmod +x supa-dev
./supa-dev --version
./supa-dev --diagnose > "$HOME/reports/initial-status.jsonl" 2> "$HOME/reports/bootstrap.log"
"$SUPA_DEV_HOME/runtime/python-3.13.18/python/bin/python3" /repo/scripts/docker-smoke/smoke.py

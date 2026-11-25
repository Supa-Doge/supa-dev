#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
(cd backend/components/lmstudio-client && npm ci --no-audit --no-fund && npm run bundle)
cargo test --locked
cargo build --release --locked
mkdir -p dist
cp target/release/supa-dev dist/supa-dev-linux-x86_64
chmod 755 dist/supa-dev-linux-x86_64
(cd dist && sha256sum supa-dev-linux-x86_64 > SHA256SUMS)

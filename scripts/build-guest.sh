#!/usr/bin/env bash
# Build the mwydyn guest modules for wasm32-unknown-unknown and stage
# them where the mwydyn-wasm tests look for them. Zero imports, no
# WASI (round 33: CBOR all the way).
#
#   rustup target add wasm32-unknown-unknown   # once
#   ./scripts/build-guest.sh
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -p mwydyn-guest-minimal --target wasm32-unknown-unknown --release

mkdir -p crates/mwydyn-wasm/tests/artifacts
cp target/wasm32-unknown-unknown/release/mwydyn_guest_minimal.wasm \
   crates/mwydyn-wasm/tests/artifacts/

ls -la crates/mwydyn-wasm/tests/artifacts/
echo "guest staged — run the mwydyn-wasm tests"

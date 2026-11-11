#!/usr/bin/env bash
# Build the JS/TS guest artifacts (round 43): engine wasm + tool,
# then compose example scripts into self-contained §9 guests.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -p mwydyn-guest-js --target wasm32-unknown-unknown --release
cargo build -p mwydyn-js --release

ART=crates/mwydyn-wasm/tests/artifacts
mkdir -p "$ART"
ENG=target/wasm32-unknown-unknown/release/mwydyn_guest_js.wasm
# the UNCOMPOSED engine — the round-47 ambient regression test composes
# its own script through the mwydyn-js library
cp "$ENG" "$ART/mwydyn_guest_js.wasm"
TOOL=target/release/mwydyn-js.exe
[ -f "$TOOL" ] || TOOL=target/release/mwydyn-js

for ex in echo; do
  "$TOOL" build --engine "$ENG" --script "crates/mwydyn-guest-js/examples/$ex.js" \
    -o "$ART/mwydyn_guest_js_$ex.wasm"
done
ls -la "$ART"/mwydyn_guest_js_*.wasm
echo "JS guests staged — run the mwydyn-wasm tests"

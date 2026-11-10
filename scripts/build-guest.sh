#!/usr/bin/env bash
# Build the mwydyn guests for wasm32-unknown-unknown and stage them
# where the mwydyn-wasm tests look for them (round 33: CBOR all the
# way; the SDK owns the exports since round 41).
#
#   rustup target add wasm32-unknown-unknown   # once
#   ./scripts/build-guest.sh
set -euo pipefail
cd "$(dirname "$0")/.."

ART=crates/mwydyn-wasm/tests/artifacts
mkdir -p "$ART"

build() { # build <package> [feature] <artifact-name>
    local pkg="$1" name="$2"; shift 2
    if [ "$#" -gt 0 ]; then
        cargo build -p "$pkg" --target wasm32-unknown-unknown --release --features "$1"
    else
        cargo build -p "$pkg" --target wasm32-unknown-unknown --release
    fi
    cp "target/wasm32-unknown-unknown/release/${pkg//-/_}.wasm" "$ART/$name.wasm"
}

# minimal: hand-rolled byzantine fixture (never on the SDK)
build mwydyn-guest-minimal mwydyn_guest_minimal
# counter: the durable example (durable_entry!)
build mwydyn-guest-counter mwydyn_guest_counter
# store: the stateless example (entry!)
build mwydyn-guest-store mwydyn_guest_store
# dedup: the stateful example (stateful_entry!) + its stateless
# twin (the round-37 dual proof — one source, two declarations)
build mwydyn-guest-dedup mwydyn_guest_dedup
build mwydyn-guest-dedup mwydyn_guest_dedup_stateless stateless-build

ls -la "$ART/"
echo "guests staged — run the mwydyn-wasm tests"

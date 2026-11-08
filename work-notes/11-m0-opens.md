# 11 — M0 opens: workspace, the data crate, the first green corpus

> Status: in progress (2026-11-08). Green light landed with three
> owner directives (compose-parity; non-wasmtime-specific interface;
> faces-not-implementations for journal/blob) — recorded in
> [docs/plan.md](../docs/plan.md). Amends nothing in the spec; the
> spec and skeleton are now growing in lockstep (charter law #10).

## 1. Toolchain

rustc/cargo **1.99.1** — A1's 1.95 pin is stale; the wasmtime version
for M1 is unconstrained by the toolchain. (A1's note: wasmtime 49
wanted rustc 1.96, hence its 48.0.2 pin — moot here.)

## 2. What exists

- Workspace: `mwydyn-data` (real), `mwydyn-core` / `mwydyn-schema`
  (stubs), `mwydyn-vectors` (the runner). `.gitignore`, resolver 2.
- **`mwydyn-data`**: the canonical CBOR profile — shortest-form
  integers; floats always emitted as f64 (**floatness preserved**:
  `Float(3.0)` encodes `0xfb`, never as an integer); f16/f32 accepted
  on decode, converted; NaN/Infinity/undefined rejected both ways;
  bytes as major-2; bignums tags 2/3 with minimal magnitudes; tag
  1001 timestamps (int64 micros, UTC); string-key-only maps sorted
  by (content-byte-length, bytes) with duplicate rejection; key
  hygiene (blacklist always + strict whitelist
  `^[A-Za-z_][A-Za-z0-9_-]*$`, overridable to blacklist-only);
  indefinite lengths forbidden; unknown tags rejected; depth limit
  64 (containers nest at most 64 levels). ~350 lines, zero deps.
- **`mwydyn-vectors`**: the corpus runner — JSON corpora, CBOR as
  hex, values as JSON projection or explicit `model` descriptors
  (bytes / timestamp / big / float / null); ops encode / decode /
  roundtrip; rejection codes stable
  (`empty|malformed|indefinite|too-deep|trailing|non-finite|unknown-tag|duplicate-key|bad-key|bad-bignum`).
- **`vectors/cbor-profile-v0.json`**: 59 cases, GREEN.

## 3. The lesson of the day: hand-typed hex is a bug farm

The first run failed 9 cases — every failure was in a hand-typed
`want` (a lost map entry count `a1` vs `a2`, a `-7` I encoded as
`-1`, one miscounted zero-run in a float), never in the encoder.
Corpus expectations are now **machine-patched** by the kept
`patch-corpus` example: expectations are derived from actual encoder
output by construction. Corollary for future corpora: hand-write
only the *rejections* and the tiny canonical pins; generate the rest.

Also fixed in passing: one clippy warning (OR-pattern → range).

## 4. Gates

`cargo test` — 1 corpus test, 59 cases, green. `cargo clippy` clean.

## 5. Next (still M0)

Envelope structs + `letter` in `mwydyn-data`; then `mwydyn-core`:
contexts, mailboxes/cursors, structural admission, the effects
engine, atomic in-proc commit, `fail-fast`/`restart`, the ticker,
the RAM blob store, the task pattern — driven by native Rust actors
implementing the `receive → effects` trait (compose-parity law).
Letter→verdict vectors alongside.

## Provenance

Owner green light + directives, 2026-11-08. Face pattern source: A1
work-notes/50 (the runtime crate's seam faces, CTR discipline).

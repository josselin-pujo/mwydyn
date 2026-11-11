# 46 — wasmtime 47 + the mwydyn-js library surface

> Status: implemented and green (2026-11-11) — full workspace suite
> 46 suites / all green on wasmtime 47.0.4, clippy clean. No design
> change: the §9 contract, the barrier ABI, and the SDK surfaces are
> byte-identical. Motivated by the FIRST EXTERNAL CONSUMER
> (platformc, the onepoint data-tools compiler) adopting mwydyn as
> its plugin substrate — see §4.

## 1. The wasmtime bump (26 → 47)

`mwydyn-wasm`, `mwydyn-js`, and `mwydyn-guest-js` (dev-dep) move from
wasmtime 26 to 47 — pinned to match the consumer's tree (platformc
already carries 47.0.4 through its vendored googlesql-rs), so one
wasmtime compiles into the combined dependency graph instead of two.

Two API facts surfaced, both trivial:

- **wasmtime 47 leaves the std error hierarchy** — `wasmtime::Error`
  deliberately does NOT implement `core::error::Error` (their
  error.rs documents the coherence conflict), so `anyhow::Context`'s
  blanket impl no longer applies to `Result<_, wasmtime::Error>`. The
  fix: wasmtime ships an `anyhow` cargo feature restoring
  `From<wasmtime::Error> for anyhow::Error` (hence `?` conversions),
  plus its own sealed `wasmtime::error::Context` for
  Result-level `.context()`. We took the feature (one attribute per
  manifest) and `map_err` for the one Result-context site (mwydyn-js
  probe).
- Everything else compiled untouched: epoch interruption, the barrier
  exchange (`TypedFunc`, `Memory`, `Global`), the statefulness probe,
  the ambient linker — the core-module API surface mwydyn uses is
  stable across the range. The container suite (byzantine proofs,
  rollback, durable reopen, JS echo through the container) passed
  WITHOUT modification.

## 2. mwydyn-js becomes lib + thin CLI

`crates/mwydyn-js` now carries `lib.rs` (the composition logic) and
`main.rs` (argument parsing + IO, unchanged CLI behavior and
messages):

- `pub struct Composed { pub wasm: Vec<u8>, pub slot: i32 }`
- `pub fn compose(engine_wasm: &[u8], script: &[u8]) ->
  Result<Composed>` — the slot's NUL terminator is the library's job;
  callers pass raw script bytes.
- `pub fn probe_slot(engine_wasm: &[u8]) -> Result<i32>` — the
  throwaway-instantiation probe, public for diagnostics.

Embedders consume the library directly instead of shelling out to the
binary (platformc's `compose-js` is the first). One lib test pins
compose→valid-module with the script bytes present in the data
section (skip-with-message when the engine artifact is absent —
the repo-wide convention).

## 3. Environment note

`mwydyn-py` interop fails in a bare shell with the Miniconda 3.8
default (`init_fs_encoding` — embedded init can't find its stdlib);
work-note 45's convention `PYO3_PYTHON=C:/Python312/python.exe` is
the fix, or `PYTHONHOME` pointed at the conda prefix. Environmental,
not code.

## 4. What this round is for

platformc (aspect-pass plugins) becomes the first non-Athena consumer
on the road to the 1.0 bar. Its integration keeps mwydyn at arm's
length: git dependency, rev-pinned; RAM contexts only; stateless
guests through `call_task`. The letter contract (`platformc.pass@1`)
lives entirely on the consumer side — the core stays schema-blind
and contract-blind.

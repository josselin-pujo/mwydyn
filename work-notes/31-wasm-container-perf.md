# 31 — WASM actor container: ping/pong through the full pipeline

> Status: implemented and green (2026-11-08). 18 walking + 5 face +
> 4 envelope units + 2 corpora (72 cases) + 2 schema + 1 throughput
> + 1 wasmtime perf + 1 wasmtime ping/pong. Clippy clean.

## 1. WASM actor container

`WasmActor` in `mwydyn-wasm/src/container.rs`:

- Wraps a wasmtime `Store<()>` + `Instance` + `TypedFunc<i64, i64>`
  (the guest's `process` export).
- Implements the `Actor` trait: `receive` calls the WASM function
  with the letter payload decoded as i64, posts the result to the
  target address. Compose-parity with native actors.
- `fork`: creates a fresh wasmtime Store + Instance from the same
  module (stateless approximation — §7.6 for stateful WASM actors).
- Guest compiled from WAT (direct text-rep, no separate guest
  compilation needed for tests).

## 2. Perf numbers

| Layer | What | Throughput |
|---|---|---|
| Raw wasmtime calls (no framework) | 20k decrements bouncing between two instances | **1.4M calls/s** (764 ns/call) |
| Context pump (fork/rollback + journal + effects) | 5000 bounces through 2 WASM actors | **3434 bounces/s** (291 µs/bounce) |

The WASM execution is NOT the bottleneck. The framework per-letter
cost (fork + journal commit + effects validation) dominates. The
ready-queue + worker-pool optimizations target exactly this gap.

## 3. Findings

- The O(n) round-robin scan becomes a ceiling at scale — the
  ready-queue (round 32) fixes this when the worker pool lands.
- WASM actors work through the Context pump alongside native actors
  (compose-parity proven by the test).
- The epoch deadline must be set per-exchange (§6): without it, the
  epoch ticker fires immediately and traps every WASM call.

## Provenance

Owner "next session is now!", 2026-11-08.

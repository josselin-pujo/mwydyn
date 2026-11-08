# 26 — Hydration: M2 closes

> Status: implemented and green (2026-11-08). **M2 IS CLOSED** — the
> durable-context story works end to end: work, die, reopen,
> hydrate, keep working. Gates: 14 walking + 5 face + 4 envelope
> units + 2 corpora (72 cases) + 2 schema, clippy clean.

## 1. What landed

- **Face additions** (both providers): `letters_for(context, actor,
  up_to)` — the hydration read, active-lineage-only (ghost letters
  excluded from replay by construction); `mints(context, actor)` —
  journaled Mint facts for grant-set reconstruction. InMemory now
  actually STORES mints (it was a no-op before — the provider loop's
  cross-check caught that too).
- **`Context::hydrate()`** — the round-26 contract, implemented:
  1. The embedding re-spawns the same composition on the reopened
     journal (composition is declarative at open; the journal is the
     event truth).
  2. Rows reset to spawn rows + journaled Mint facts (base_rows
     reset makes hydration idempotent).
  3. A fresh instance replays its DELIVERED letters (`1..=cursor`)
     through `receive` with effects DISCARDED and the host clock
     pinned to each letter's journaled commit time.
  4. Pending letters stay pending — they were never processed, so
     state-as-of-the-ack-boundary is exact and live delivery
     continues without duplication or loss.
- Replay panics are caught, counted, and warned (the letter remains
  a live-delivery problem); replay is skipped-safe.

## 2. The proof (both SQLite-backed)

- `hydration_rebuilds_rows_and_ticker_schedules`: mint a row + arm
  a timer at T=1000 (fires 1500); process death; reopen + hydrate;
  the grant set is reconstructed (`CAPS:role.write,role.read`) and
  the HYDRATED schedule fires the tick at T=1600 — the round-19
  ticker coherence, now demonstrated across a process boundary.
- `durable_context_survives_reopen`: deliver m1, post m2, die
  before delivery; reopen + hydrate; post m3; both m2 and m3
  delivered exactly once — **m1 is not re-delivered** (log owns
  acks, round 01, mechanical at last).

## 3. The design law this closes

Composition is declarative at open; the journal is the event truth;
hydration is deterministic replay of delivered letters with a
pinned clock. It works because receive is a PURE TRANSITION —
round 07's purity mandate was the prerequisite, and replay of a
ticker schedule requires no ticker-specific code (round 19's
coherence, now proven across a process boundary).

## 4. Known limits (honest ledger)

- Blob store not yet durable (M3): hydration replays that read
  pre-crash blobs see loud Missing.
- Replay panics skip the letter's state contribution (warned) —
  stateful-actor snapshots (§7.6) are the full answer.
- Mint-fact row reconstruction is additive-only (no revocation
  facts yet — revocation is a §3.3 future fact type).

## 5. Next

- **M3**: blob persistence + reference-aware GC above the watermark.
- **The executor** (round 21): parallel transitions on workers,
  serialized dispatch-order commits.
- **WASM guests (M1's world binding)**: the world draft is v0.2/v0.3;
  wit-bindgen against the frozen shape.
- Then: 1.0 approaches — "everything hard that isn't core is a
  pluggable implementation away."

## Provenance

Owner "onward to hydration!", 2026-11-08. Sources: round 19 (ticker
coherence), round 07 (purity), A1 work-notes 23 (replay discipline).

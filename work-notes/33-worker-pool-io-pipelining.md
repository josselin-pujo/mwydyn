# 33 — The worker pool + THE IO PIPELINING VERIFICATION

> Status: implemented and green (2026-11-09). All suites: 18 walking
> + 5 face + 1 throughput + 3 io-pipelining + 5 worker-pool + 4
> envelope/barrier units + 2 corpora + 2 schema + 6 wasm barrier +
> 2 wasm perf. Clippy clean workspace-wide.

## 1. The owner's bar, and what the audit found

Owner: "we must VERIFY that IO pipelining works and we don't have
any relationship between number of exchanges and number of journalIO
at the SQLite layer... buffering and pipelining has to be done with
no shortcuts there."

The audit found TWO real defects, both invisible on the RAM tier:

1. **Group commit was per-exchange.** `run_until_idle` flushed after
   every `deliver_one()` — which delivers exactly ONE message. One
   journal transaction per letter; on SQLite-direct that is one fsync
   per letter. The round-32 machinery existed but the cadence was
   exchange-driven.
2. **The durability overlay was a silent no-op.** `InMemoryJournal`
   implemented `unsynced_ops`/`take_ops` as INHERENT methods; the
   TRAIT defaults (empty) won through `Box<dyn JournalFace>` —
   `flush_durability` drained nothing, ever, since round 31. (Fixed:
   trait overrides delegating to the inherent methods.)

## 2. The architecture correction (the round's lesson)

My first fix buffered ops across deliveries — and produced the
perfect failure: the durable test saw **m2 re-delivered fifty
times**. THE MAILBOX IS A CURSOR OVER THE JOURNAL (§3.1): a buffered
ack never advances the cursor, so the scan re-offers the same letter
forever inside the window. Buffering inside the hot journal is
structurally wrong.

The correct layering was already in the codebase — round 31's
overlay, now actually wired:

- **Hot journal**: applies EVERY per-receive transaction instantly
  (posts + mints + ack = ONE transaction, §8.2's atomicity unit).
  The cursor stays current; no re-delivery. Memory ops, zero IO.
- **Durability boundary**: `flush_durability()` drains the hot op
  log to the DURABLE face as ONE transaction — on policy
  (`max_ops_per_flush`, default 256), at quiescence
  (policy-respecting), and on the embedding's explicit barrier call.
  **Durability IO is a function of the knob, never the exchange
  count.**
- Semantics: the undrained tail is the pipelining exposure —
  at-least-once redo from the durable point. Letters posted but not
  yet drained are LOST on death (they never existed durably): the
  embedding's durability barrier is `flush_durability()`.
- `hydrate()` now reads delivered history from the DURABLE face when
  the overlay is present (the hot journal starts empty on reopen).

## 3. The verification (SQLite behind the counting wrapper)

`tests/io_pipelining.rs` — every claim measured at the SQLite
boundary (commit count = fsync count under WAL+FULL):

- **`durability_io_tracks_policy_not_exchanges`**: 4,000 exchanges,
  K=64 vs K=512 → transactions track ≈ ops/K, ≥4× drop when the
  knob rises, ≥200 exchanges per durable transaction at K=512. The
  relationship is the POLICY's, not the exchange count's.
- **`one_explicit_flush_is_one_sqlite_transaction`**: drain held off
  (K=MAX), 2,000 exchanges advance the world with ZERO SQLite
  transactions; one barrier call lands the whole window (appends +
  acks, 4,002 ops) as exactly ONE transaction.
- **`crash_before_drain_redoes_from_the_durable_point`**: reopen on
  the same SQLite file — hydrated history restored, new work runs
  exactly-once, nothing lost, nothing duplicated.

## 4. The worker pool (§6, round 21's design landed)

`ctx.workers(n)` — wave dispatch: one letter per actor per wave
(single-cursor), transitions run on `min(n, wave)` scoped threads,
and the pump thread is the COMMITTER: `land_effects` (validate →
classify → buffer-the-receive-transaction) runs strictly in dispatch
order. The journal's total order = dispatch order; replay stays
deterministic given the log. Forks happen ON WORKERS (§3.5's
transactional receive — the rollback copy is the worker's; the
entry stands empty mid-wave because nothing scans between
composition and apply). Refusal → restore the rollback copy; crash
→ restore + `handle_crash` — serial semantics exactly.

Proven: fan-out lands every letter exactly-once; busy refusal rolls
back and heals under parallel; poison dead-letters through the real
strike path; **serial and pool agree on the outcome multiset**;
WASM guests ride the pool like natives (compose-parity + the
`Actor: Send` bound paying off).

## 5. The honest throughput report

| Workload | serial | pool(4) |
|---|---|---|
| 8 echo actors (cheap transitions) | 2,970/s | 2,973/s |
| 8 WASM guests (fork 120µs + receive 71µs each) | 2,518/s | 2,384/s |

Parity, not speedup — and the decomposition says why: the
parallelizable part (fork+receive ≈ 191µs) is offset by the
per-wave scoped-thread spawn (~80µs × wave on Windows) and the
serial committer framework (~200µs of scan/commit/queue that MUST
stay serial — it is the committer's law). Persistent worker threads
fix the spawn overhead; they require a §8 face-contract decision
(see §7 queue): the workers need `&dyn JournalFace` for host reads
while the committer needs `&mut` between waves — today solved with
scoped threads per wave, which cannot persist. The production
executor needs ONE of:
(a) `JournalFace::commit(&self)` with provider-internal locking;
(b) a read-face split (`JournalRead` for hosts / `JournalCommit` for
the committer) — matches the byzantine read/write posture;
(c) sanctioned scoped-pool plumbing (raw-pointer + Mutex, invariant:
no commit during a wave).

## 6. Bug story (recorded)

- Buffering acks inside the hot journal → m2×50 re-delivery. The
  cursor invariant beats batching instincts.
- The overlay's no-op: inherent-vs-trait method shadowing through a
  trait object — exactly why the face has conformance vectors coming
  (a provider test that drives flush_durability through the FACE
  would have caught this at round 31).
- Livelock candidate found and designed against: buffered posts
  filling a small mailbox bound with the flush pinned to a higher
  threshold — the window starves its own drain. The overlay design
  (hot applies immediately) makes the class impossible.

## Provenance

Owner: "Go ahead with worker pool, and we must VERIFY that IO
pipelining works..." 2026-11-09.

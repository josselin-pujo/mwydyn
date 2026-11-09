# 36 — Depth-N pipelining + the apply-cost hunt

> Status: implemented and green (2026-11-09). 22 suites ok, clippy
> clean.

## 1. The micro-profile (probe, deleted after reading)

| apply-path piece | ns/op |
|---|---|
| **journal.commit (1 append + 1 ack)** | **10,618** |
| payload wall (canonical decode) | 333 |
| letter.validate | 467 |
| pending_first() peek | ~2,200 |

The commit WAS the apply story — everything else was noise. Inside
it: the op_log recorded EVERY op into the hot journal (a full
envelope clone + a context string per op) for an overlay most runs
never open, and `Commit.appended` cloned every envelope although its
single consumer only ever looks at external-target ones.

## 2. The fixes

- **`RecordingJournal`** (round 38): the overlay's recording layer is
  a wrapper — `with_durability` wraps its hot journal in it; the
  plain hot journal records NOTHING. Same face methods
  (`unsynced_ops`/`take_ops`), zero cost for tiers with no durable
  face. (InMemoryJournal's inherent-vs-trait overlay methods — the
  round-34 no-op — die with the field.)
- **`Commit.appended` is external-only** — internal appends (the
  overwhelming majority) pay no extra envelope clone; the journal's
  own store is the truth.
- commit re-measured: 10.6 → 8.2µs at bench depth (the bench's ack
  path also drags an O(depth) front-shift — noted, not yet a lever).

## 3. Depth-N pipelining (round 38)

Depth-1 (round 36) left workers idle through the committer's apply.
Now `PIPELINE_DEPTH = 4`: up to four waves dispatched before the
oldest is collected and applied. Round 36's invariants hold
unchanged — compose skips dispatched indices (single-cursor; the
terminal drain never fires in-flight), results are globally
sequenced and buffered per wave (young fast results precede old slow
ones), transactions land strictly in dispatch order. The pass bound
still bites: it stops NEW dispatches; the outstanding pipeline
drains.

## 4. Numbers (8-actor workloads, workers=4)

| Workload | serial | pool(4) | wall/ex | committer/ex |
|---|---|---|---|---|
| Echo (cheap transitions) | 3,886/s | **30,764/s (7.9×)** | 23.8µs | 21.8µs |
| WASM guests (persistent instances) | **6,249/s** | 9,052/s | 97µs | 65.6µs |

- Echo's wall is now AT the committer bound: **the pipeline
  saturates the committer** — the Amdahl point, reached. From here
  echo throughput = cheaper apply/compose, full stop.
- WASM's two bounds are now BALANCED (committer ~15.2k/s, workers
  ~14k/s at receive ≈ 71µs/ex) — neither alone is the lever; the
  remaining gap to either bound is wave granularity (4 exchanges per
  wave here). Serial jumped too: 2,671 → 6,249/s (persistent
  instances + cheaper commit, serial path included).

Run-to-run variance on this box is ±30% (parallel cargo runs share
cores) — treat single readings as ranges.

## 5. The throughput ledger, after three rounds of measurement

| round | echo pool(4) | WASM pool(4) | lever pulled |
|---|---|---|---|
| 34 (parity) | ~3.0k/s | ~2.5k/s | — |
| 35 (persistent threads) | — | 3.9k/s | spawn-per-wave |
| 37 (persistent instances) | — | 9.8k/s | the 120µs fork |
| 38 (depth + commit) | **30.8k/s** | ~9.5k/s | op_log + appended clones, depth |

Next levers, if wanted: finer waves / cheaper stage (letter+rows
clone per job ≈ 5.8µs/ex), the ack front-shift, worker count. None
is urgent — the features board (§7.6 guest snapshot ABI, barrier
conformance vectors, mwydyn-stdlib) is the better spend.

## Provenance

Owner: "Let's add pipelining depth and do a round of profiling and
optimisation focused on apply costs, then we can continue on
features!" 2026-11-09.

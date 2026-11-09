# 34 — Wave pipelining + THE AMDAHL POINT

> Status: implemented and green (2026-11-09). 20 suites ok, clippy
> clean. Numbers measured on the dev box (variance noted — parallel
> cargo runs share it).

## 1. Wave pipelining (round 36)

The pool's phases were strictly sequential: compose → dispatch →
collect → apply, per wave. The committer's apply and its collect
window were fully exposed serial time. Now: **the next wave is
dispatched BEFORE the current wave is collected and applied** —
workers run wave k+1 through both of the committer's phases for
wave k.

Legality, spelled out:
- compose skips DISPATCHED indices (a `dispatched` set): the
  single-cursor invariant holds across the pipeline, and the
  terminal drain can never fire on an in-flight entry;
- wave k+1 composed before wave k applied simply doesn't see wave
  k's posts yet — they compose in wave k+2 (chains advance one wave
  per cycle, same as before, no reordering);
- transactions land strictly in dispatch order: waves are applied
  in dispatch order, and within a wave `land_effects` runs in seq
  order. The journal's total order = dispatch order — the round-21
  law is untouched.
- RESULTS ARE GLOBALLY SEQUENCED and buffered per wave
  (`BTreeMap<u64, WaveOutcome>`): with the pipeline open, wave k+1's
  fast results arrive before wave k's slow one — positional zipping
  would have silently mismatched actors to envelopes.

One lifecycle subtlety the first cut got wrong: after applying a
wave, the freshly-composed next wave may be non-empty ONLY NOW (the
actors were in flight during the earlier compose) — the loop must
RE-ARM composition after the world moves, or it quiesces early (the
2-of-200 lesson, caught by the fan-out test).

## 2. The compose tax the profile exposed

The first pipelined profile was unambiguous: `compose = 434,871µs`
of ~439ms wall — **99% of the committer's life was scanning**. The
scan called `pending()` per actor per wave, and `pending()` clones
the ENTIRE queue to take `.first()` — half a million envelope
clones across the run.

Fix: `JournalRead::pending_first()` — peek ONE envelope (default =
delegating; `InMemoryJournal` specializes to the front of the
pending index without cloning the seq vec). Compose dropped to
~11µs/ex; the whole workload transformed (§3).

Lesson: the profiler pointed at the line; the queue-clone was
invisible from the code's shape. MEASURE, THEN REORDER THE CODE.

## 3. Numbers (8-actor workloads, workers=4)

| Workload | serial | pool | speedup |
|---|---|---|---|
| Echo (cheap transitions) | 4,231–4,537/s | 18,231–27,046/s | **~4–6×** |
| WASM guests (fork 120µs + receive 71µs) | 2,671–2,908/s | 3,799–5,243/s | **~1.4–1.8×** |

(Ranges = run variance; parallel cargo runs share the box.)

## 4. THE AMDAHL POINT (the owner's ask: focus throughput work)

`PoolProfile` (in `PumpStats.profile`, pool mode) carries the
committer's phase ledger: waves, wall, compose, stage_dispatch,
collect (the overlapped window), apply, polls — plus
`committer_us_per_exchange()`.

Measured this run:
- **Echo: committer = 34.6µs/ex → hard bound ~29k/s.** The wall
  (42µs/ex) is approaching the bound: THE BOTTLENECK IS THE
  COMMITTER (apply ≈ 23µs/ex — the per-receive journal transaction
  — + compose ≈ 11µs/ex). Adding workers does NOTHING from here.
  The levers, in order: cheaper apply (hot-journal commit cost),
  cheaper compose (cache the pending-front), NOT parallelism.
- **WASM: wall = 248µs/ex vs committer = 148µs/ex** — the worker
  side is closing in: each exchange costs a worker 191µs serial
  (fork 120µs + receive 71µs). The levers, in order: **§7.6 WASM
  snapshots** (kills the 120µs re-instantiate-per-fork), then apply
  cost, then more workers (4 → 8) once the fork is gone.

Rule of thumb the profile teaches: cheap-transition workloads are
committer-bound (pool adds nothing past the bound); expensive-
transition workloads are worker-bound until the fork dies.

## 5. Provenance

Owner: "Go ahead with wave-pipelining, and let's profile afterwards
to actually measure the amdahl point to focus throughput work."
2026-11-09.

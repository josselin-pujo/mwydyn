# 21 — The execution model: parallel transitions, serialized commits

> Status: DECIDED and readiness-implemented (2026-11-08). Spec §6
> amended. The executor itself is post-M2, additive.

## 1. The discussion

The owner, before the context switch welds the pump's shape:
*"we serialize interactions (per context), but we don't actually
NEED to serialize processing since we are CSP shaped — shouldn't we
at least start looking at running actors on an executor and be
ready for multithread?"*

## 2. The contract

- **Transitions run in parallel** — different actors share nothing;
  the single-cursor invariant (§3.2) guarantees an actor never
  overlaps itself. The compiler consumer's fan-out is the real need.
- **Commits serialize per context** — the journal's total order
  (§8). The good trade: the transition is arbitrarily expensive, the
  commit is an append+ack.
- **Determinism is relative to the journal.** Owner's law, verbatim:
  *"non-deterministic race is fine so long as there IS a winner and
  it is reliably recorded, which is the case — journal is law,
  whatever happens that creates the stream of events that are
  journaled is what it is; in reality a lot of events will come
  from outside clocks anyways, so I'm good with non-deterministic
  but recorded, it's the best we can ask."*
- Replay follows the journal → deterministic given the recording.
  Rerunning a race without the journal is not reproducible — the
  answer to that want is the serial pump, not a cleverer parallel
  one.
- **The serial `run_until_idle` pump stays forever** as the
  deterministic mode: tests and replay verification.

## 3. The mechanism (post-M2 executor)

The four-phase engine already splits at the seam: phase 1 (the
transition) is the parallelizable part; phases 2–4 (validate,
classify, apply+commit) are the serialized committer. Workers run
transitions concurrently; the committer lands them in DISPATCH
order — parallelism with deterministic commit order. Backpressure =
bounded in-flight transitions; single-cursor = an actor's next
dispatch waits for its own commit.

## 4. Readiness bounds (implemented this round)

- `JournalFace: Send + Sync` — external providers are threaded;
  one that cannot cross threads is not a real provider.
- `Actor: Send` — actor state MOVES between scheduler and workers;
  forbids `Rc` in actor state by contract, while only toy actors
  exist to fix.
- `Spawn` factories + `Entry` field: `Send`.
- `ClockFace: Send + Sync`.
- `RamBlobStore`: `RefCell` → `Mutex` (held only for map
  operations — no user code inside; poisoning practically
  unreachable).

## Provenance

Owner question + ratification, 2026-11-08. CSP framing; the
journal-as-recording philosophy is A1's recovered whole.

# 30 — The actor contract stays sync, forever

> Status: DECIDED (2026-11-08, ratified by the owner). A law in
> AGENTS.md. No implementation this round — the deliverable IS the
> restraint, plus a tiered SDK-ergonomics plan for when it's needed.

## 1. The question

Stateful orchestration actors (request → third actor → enrich →
gather → reply; many in-flight "requests") are natural to write with
`async`/`await`. Should the core pay the toll — per-actor event
loops (frightening cost), a shared loop (conceptual leakage), or
contract support that breaks compose-parity — or does the
state-machine sugaring stay inside actors?

## 2. The answer

**Core stays sync, forever.** The chain of reasons:

1. **Await already exists in mwydyn — it's the runtime.** Posting
   effects and returning from receive IS suspension; the reply
   letter IS the wakeup. Adding Rust async would rebuild the pump's
   continuation mechanism inside the actor boundary, at higher cost.
2. **Compose-parity dies first** (round 11): `async fn receive` is
   unspeakable in WASM (no async in the frozen WIT), in C (a
   function pointer cannot suspend), and burdens Python (asyncio
   knowledge now required of every python-actor author).
3. **The core would own a second scheduling dimension**: futures
   capturing `&Host` across yields are not `Send` (round 21's
   bounds), so either faces become Arc-owned (tight-core bleeds) or
   transitions can't cross threads (the executor dies). Mid-await
   actors are an interior state the pump must understand.
4. **Real parallelism doesn't need async**: the round-21 executor
   gives multicore via workers + dispatch-order commits, bounds
   already bought. Async would buy concurrency-of-waiting — which
   the runtime already provides.

## 3. The ergonomics plan (SDK-side, tiered, deferred to first need)

- **Tier 0 (now)**: the multi-hop walking test is the reference
  pattern; correlation-scoped replies are already in the contract.
- **Tier 1 (first orchestration actor)**: peer-crate patterns —
  kind→handler Router; `RequestTable<Correlation, Partial>` for
  fan-out/gather; deadline-race helper (tick vs. reply, round 06).
  Sync, testable without a runtime, ~50 lines each.
- **Tier 2 (proven need only)**: pinned-future driver for native
  async-LOOKING actors — SDK-internal mini-executor, pinned future
  in actor state, effects buffered between polls. Local compiler
  trickery: zero core diff, zero contract diff. WASM/TS/python
  actors keep native handler shapes.

## 4. The OTP lessons, distilled (for the no-combat-experience file)

- **gen_statem returns actions** — a pure transition returning an
  action list. Our `receive → effects` IS gen_statem-shaped; the
  most battle-tested actor platform converged on our shape, and all
  its ergonomics sit on top, never inside OTP.
- **Selective receive on a shared mailbox is a pathology** —
  router-by-kind sugar is the cure, not a compromise.
- **Veterans don't write deep gen_servers — they add processes.**
  Deep orchestration chains = choreography: more small actors
  connected by correlations (task actors make it cheap), not deeper
  machines.
- **Deadlines**: `erlang:send_after` races → our ticker-vs-reply
  race (round 06), one-to-one.

## Provenance

Owner question + ratification, 2026-11-08 ("right call"; compiler
trickery deferred, "there is always another way to dispatch").
Companion to round 21's execution model; AGENTS law added.

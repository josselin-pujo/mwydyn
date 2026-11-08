# 07 — Effects as declarations; the transaction boundary

> Status: decided (round 2026-11-08, with the project owner) — the
> owner overruled the initial imperative-v0 recommendation. World
> revised to the effect-list shape: [docs/world-v0.wit](../docs/world-v0.wit)
> v0.1. Amends [spec-v0](../docs/spec-v0.md) §3 (gains §3.5), §7.8.

## 1. The decision (owner's argument, ratified)

- **Effect lists make transactions possible when we want them** —
  the banking nerve says yes.
- **Reads outside the inbox and internal state are limited to
  content-addressable blobs** — repeatable reads by nature, nearly a
  pure function of the parameters; the only failure is a swept ref,
  and that is a clear error, never changed data.
- The slight complexity increase for actor writers is worth having
  the purity **contractually mandated**.

## 2. What the runtime now does

`receive: (memory, letter, blob-reads) → list<effect>` — a pure
transition. The runtime:

1. **Validates before applying** (fail-before-compute): schema,
   sizes, authority rows, digest resolvability (a `put`'s digest is
   computed at validation — content addressing closes the data-flow
   gap for post-effects referencing fresh puts), capacity/caps.
2. **Applies in order, atomically** within the receiving context —
   state transition + effects = one journal transaction (log tier)
   or one in-proc commit (RAM tier).
3. **Handles refusals** in two classes: `busy` → re-queue the letter
   at its inbox tail, no attempt burned (overload self-heals: the
   actor re-receives and re-declares); `invalid`/`unauthorized` →
   attempt burns → N strikes → dead-letter (§3.1).
4. Applies `emit` effects **outside the transaction** — lossy by
   design.

## 3. What this buys (the dividends)

- **Atomic checkpoints** (§7.6 becomes nearly free): instance capture
  and effect application are separated by construction.
- **Fully vector-shaped replay testing**: `(letter, seeded memory,
  blob fixtures) → effects` is a pure function table.
- **A tiny frozen world**: the import surface is pure reads only —
  blob reads + `caps.list`. Everything else is data.
- **Self-healing overload**: temporary refusal re-queues the letter;
  actors re-receive without writing retry logic.
- **Interruptability for free**: imports are pure, so the §6 rules
  (no threads, no reentrant host calls) are satisfied structurally.

## 4. The boundary the banking nerve must respect [proposed]

**Single-context atomic, cross-context eventual.** State + all
effects commit atomically *within* the receiving context; posts
across contexts are at-least-once eventual — multi-context
consistency is saga territory. This is the honest limit of
"transactions": one journal, one commit. (Awaiting owner
ratification.)

## 5. Ripples absorbed

- §6 interruptability rules: trivially satisfied (pure imports).
- §7.5 telemetry: `emit` is an effect, outside the transaction.
- §7.6 snapshot/replay: checkpoints trivially atomic; replay =
  re-run the pure transition.
- The deferral idiom changes: no refuse-handling in actor code —
  the runtime re-queues; timers remain for genuine timeouts (races).

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 41 (fail-before-compute, refusal verdicts), 04 §5
(commitTs as audit), 51 (content-addressed transport).

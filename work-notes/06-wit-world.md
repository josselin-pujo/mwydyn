# 06 — Clock obliviousness and the WIT world draft

> Status: backpressure ratified (round 2026-11-08) with the owner's
> refinement — *how and why a refusal is minted is policy that doesn't
> concern the sender*. Clock obliviousness ratified. World draft v0
> in [docs/world-v0.wit](../docs/world-v0.wit), awaiting owner
> reaction. Amends [spec-v0](../docs/spec-v0.md) §7.8.

## 1. Backpressure refinement (owner)

`post → refuse` is the surface; **the minting of the refusal is
runtime policy, not sender-visible**. Concretized: the refusal reason
set is coarse and stable — `busy | unauthorized | unknown-target |
invalid` — with bounds/credits/overload all mapping to `busy`; fine
diagnostics go to telemetry, never to senders.

## 2. Clock obliviousness (owner, ratified)

**The clock IS the inbox tick; there is no other clock.**

- Guest subjective time = the cursor position. Guests never read a
  clock; all nondeterminism enters the system *as messages* (A1's
  event-carried-nondeterminism discipline [A1:04 §5], recovered as a
  theorem again).
- **The ticker** (owner's caveat, formalized): a well-known NAME —
  arming a timer is `post(ticker, timer.arm)` — the only wall-clock
  reader, runtime-owned. Arm requests carry **durations, never
  timestamps** (durations are deterministic data; the ticker anchors
  them at the arm message's journaled commit time). Ticks are
  journaled messages: rewind un-happens them, replay re-fires them
  from the log watermark — the round-02 rule ("clock reads must be
  journaled") mechanized.
- The timeout idiom: **race the tick against the expected letter**;
  first message wins. `after`-style receive is a pattern, not a
  primitive.
- Arc closure: this is the VMM idea from the first Athena2
  conversation — "all actions through an event store providing an
  external vector clock" — at actor scale. The inbox tick *is* that
  clock.
- Durations are data; timestamps are runtime-assigned audit facts
  (commitTs) — usable for audit, never for branching [A1:04 §5].

## 3. The world, and what got rejected

Shape: **four imports** (post, caps, blobs, telemetry), **two
exports** (receive, boot/init), **one well-known name** (ticker).
`receive` is called once per inbox message, sequentially — the ABI
itself is the single-cursor invariant made mechanical.

Rejected from v0, deliberately, each to keep the freeze small:

| Rejected | Why |
|---|---|
| `yield` | defer-and-retry = self-tick via ticker + return; long compute is fuel/epoch-bounded |
| `subscribe` | fan-out is a binding pattern (dispatch queue policy) or user-built |
| snapshot verb | runtime checkpoints happen at message boundaries (§7.6); guests need nothing |
| timer verb | the ticker is a well-known *address*; arming = post — no new verb |

Escape hatch: world version bumps (like the CBOR profile) are the
compatibility mechanism when a rejected verb eventually earns its
place.

## 4. OPEN within the draft

1. **Imperative effects vs returned-effect lists**: `receive` makes
   effects via imports (imperative — the A1 channel-seam shape) vs
   returns an effect list the runtime executes (declarative —
   trivially atomic checkpoints and replay-testable effects, at the
   cost of a mixed model for reads). Proposed: imperative for v0,
   declarative considered only as a future world version.
2. Envelope field set convergence (spec §2) — `letter` here vs §2's
   runtime envelope.

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 04 §5 (injected sources, event-carried nondeterminism,
commitTs-as-audit), 50 (the channel seam — one native, CTR-3 live
grants), 23 (boot kicks, `__athenaManifest`), 41 (refusal verdicts).

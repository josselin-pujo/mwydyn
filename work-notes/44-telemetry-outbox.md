# 44 — The telemetry outbox

> Status: DECIDED 2026-11-10 (owner, session — four forks taken one
> at a time). Spec §3.6. Implementation queued (staging in §7 below);
> vectors `telemetry-v0` are part of the definition of done.

## 1. Where we started (the drift surface)

The spec promised "an actor has an inbox, an outbox, and a telemetry
outbox — best-effort, lossy by design, never backpressures" while the
implementation had `Effect::Emit { level, message: String }` landing
in an UNBOUNDED `Vec<(Level, String)>` with a getter. "Never
backpressures" was implemented as "grows forever" — the exact failure
mode lossiness exists to prevent. Runtime facts (refusals, strikes,
crashes — §3.2 lifecycle facts, the refusal record) rode the same Vec
as actor logs, conflated into strings. §7 queue item 5 named the
missing contract: correlation propagation, span shape, drop
accounting.

## 2. The forks, as taken

1. **Span model → DERIVED-ONLY (owner-ratified).** Actors never
   declare spans. Three laws make actor spans unimplementable, not
   merely awkward: clock obliviousness (§6 — an actor cannot measure
   a duration), the sync contract (round 30), and atomic transitions
   (the runtime observes nothing mid-flight; start/end markers from
   one receive would get near-identical stamps). One span per
   exchange, derived from lineage. Cross-context linking queued
   (§6 below).
2. **Record type → stated as default, unchallenged.** ONE `Emit`
   carrying a record; CLOSED kinds (`log | metric | exchange |
   lifecycle | refusal | drop`) — telemetry is infrastructure data
   and gets barrier-verb discipline (exact spellings, unknown →
   refused, corpus-pinned), unlike letters whose `kind` is opaque
   (round 10 — application data). Metrics are points-not-aggregates,
   temporality always delta. `ts` runtime-minted at apply.
3. **Outbox mechanics → SHARED RING + DRAIN (owner-ratified).** One
   bounded ring per Context, drop-oldest, capacity knob;
   `drain_telemetry()` in the `drain_external()` pattern; drop
   accounting THROUGH THE SAME PIPE (a leading synthetic `drop`
   record per lossy drain). No push face — a foreign sink in the
   apply path is a blocking hazard; apply stays allocation-only.
   Accepted wart: one noisy actor can starve the ring (records carry
   `actor`; fairness quotas YAGNI until observed). Spec-literal
   per-actor rings and push faces are the rejected alternatives.
4. **Emit gating → UNGATED (owner-ratified).** Observation is not an
   effect on the world — consistent with posts being ungated today
   (per-target post caps are the queued default-deny fork). The ring
   bound is the only guard; floods become drop records, NEVER
   strikes (telemetry must not burn business machinery). Gating was
   observed to degenerate: a gated refusal could not burn a strike
   without violating lossy-by-design, so it would collapse into
   drop-with-accounting — the same outcome plus a row check.

## 3. Lineage: the core attaches it (owner amendment)

The initial position — "the sink derives spans by tailing
`JournalRead`" — was too glib, and the owner's amendment fixed two
defects at once:

- **The journal has commit times but NO dispatch wall time** —
  transition durations exist only in the core's real-time view.
  Telemetry is the only channel that can carry them (and durations
  are real-time observations, replay-exempt — consistent by
  construction).
- **Sink integration drops from (drain + journal tail) to (drain)**:
  correlation arrives pre-attached.

The mechanics were already in place: `ack` is a checksum-bound
envelope field since round 26/29 (runtime-stamped, host-decided,
exact, unforgeable — the snapshot `covers` machinery depends on it),
so the mailbox hands the pump `{seq, ack, from}` with every pending
letter. The core reads a field that is already there.

**The `exchange` record** — one per delivery attempt, always emitted
(busy included: late-because-busy-three-times is exactly what a
trace should show; owner: "lossyness is there to handle storms, no
need to be stringy on emission"), behind a builder flag defaulting
ON:

```
exchange: { seq, ack?, from, actor,
            ts_in: dispatch wall time, ts_out: commit/strike wall time,
            outcome: committed | busy | struck }
```

Sink-side derivation is a PURE FUNCTION of one drain stream: `seq` =
span id; `ack` = parent span id; `ack: None` = root (embedding/pump
and timer posts — trace id ≡ root seq of the ack chain); actor
logs/metrics join by `seq` → span events; an unresolvable `ack`
(= a foreign context's seq, today) → link-or-root. Orphan records
are legal lossy behavior (a log whose exchange record dropped maps
to a standalone OTel log).

**The law that falls out**: actors never see lineage; sinks never
touch the journal; the core attaches both halves. The journal stays
the DURABLE record of causality (unchanged law); telemetry is the
lossy real-time projection WITH timing.

## 4. The injection handle (bootstrap for roots)

Owner-raised: external sources that inject letters need the seq back
to map letters to external ids. Found: `Context::post` SWALLOWED the
seq (returned bare `Accepted`) — a host-API gap, not an envelope or
telemetry one. The outbound half already worked (`drain_external()`
yields stamped envelopes, `.seq` in hand). Owner-ratified fixes:

- **`Accepted` carries the seq** — the durable channel for external
  id ↔ seq mapping. The mapping itself lives in the EMBEDDER's store;
  telemetry deliberately stays out (lossy observation ≠ durable id
  mapping).
- **Injection accepts an optional parent** (`Option<u64>`), stamped
  into the envelope's `ack` under the EXISTING round-26/29 law (the
  host decides causality) — no envelope change, no ABI change, the
  checksum already covers it.
- **The core validates the parent is a known seq** (any lineage, one
  lookup) or refuses `Invalid` — loud beats a silently unresolvable
  parent; the embedder's typo dies at injection.

The prize: request/response across the external boundary closes
causal loops with zero protocol the external system must speak —
actor posts out (drained, seq known), external system works, the
embedding injects the reply with `parent = <drained seq>`; the trace
crosses the external hop with both ends anchored (the sink may
synthesize the external system as a linked span between). See
spec §3.4's amended bullet.

## 5. The contract (consolidated → spec §3.6)

**Producers**: actors declare `log` and `metric` (emit effects,
helpers `Effect::log` / `Effect::metric_*`); the runtime declares
`lifecycle` (§3.2 facts), `refusal` (admission + ambient refusals,
the ambient-v0 spellings), `drop`, and `exchange`. **Spans are
nobody's declaration.**

**The record**: an `mwydyn-data` type, canonical CBOR, closed kinds,
unknown kinds refused. Guests supply kind + payload ONLY; `ts`,
`actor`, `seq` are runtime-minted on apply (byzantine posture — no
forged attribution or timestamps, even accidentally). `Level` stays
5-valued → OTel SeverityNumber 1/5/9/13/17 at the mapping layer.

**Semantics**: outside the transaction, literally — a failed journal
commit does not retract emits. No double-counting by construction
(hydration discards effects; crashes eat theirs pre-apply; one real
exchange emits at most once). Ungated. Lossy: bounded ring,
drop-oldest, drops lead the drain as records.

**Barrier + SDK**: the response grows `{"emit": {…}}` as an additive
verb — NO ABI bump (old host + new guest → the documented
`UnsupportedVerb` slot, stable error; new host + old guest →
silence). `mwydyn-guest` gains `Effect::emit` + helpers. The record
shape lives in mwydyn-data beside `barrier.rs`; the codec law holds
(guest-side codec IS mwydyn-data).

**Vectors** (`telemetry-v0`, barrier-v0 pattern verbatim): canonical
forms per kind, the unknown-kind refusal, the drop record, the
exchange record, runtime-minted fields proven absent from the guest
side of the wire.

## 6. Forks, queued and resolved

- **Cross-context correlation — RESOLVED (same round, follow-up
  session, owner-ratified).** The owner's conflict ("unsolvable in
  the general case" vs "we can provide the mapping as best
  practice, but it adds core surface") dissolved on inspection into
  two cases:
  - **Foreign systems: unsolvable by the runtime** — a carrier that
    replays, fans out, or originates breaks causality only it can
    see; carriers must cooperate (OTel's own position). Mechanism:
    PAYLOAD convention echoing our outbound `seq` (the drained
    envelope already carries it; §9.5 replyTo precedent; a
    `replyTo`-style SDK blessing possible later) — the embedding
    can then parent-stamp the injection automatically. Never an
    envelope field.
  - **mwydyn↔mwydyn: real, knowable, and ALREADY BUILT by this
    round** — the bridging embedding is the only entity holding
    both namespaces, and it holds them by the ratified handles:
    drained seq out, injected seq back. The correlation table is
    two columns the bridge writes anyway; the sink joins (trivial
    today: contexts are in-proc, one drain point). ZERO core
    surface — and correctly so: the core's causal knowledge ends at
    its journal; a core registry would violate tight core
    (contract never reads it), duplicate bridge knowledge, and
    still miss foreign hops. Best practice = bridge pattern doc /
    example, later a `with_correlation_edges` convenience on the
    OTel LAYER if that crate ever exists.
  - **Envelope trace tokens: DORMANT, with trigger** — necessary
    only when mwydyn↔mwydyn hops cross process boundaries and both
    sides' spans land at one OTel backend joining by shared trace
    id (a join no remote bridge table can serve). Trigger = the
    existence of cross-process mwydyn transport; then a version
    bump, decided then.
  Interim sink rule unchanged: unresolvable `ack` → link-or-root.
- **Default-deny boundary** (queued, unrelated round) — if it ever
  gates posts, it may revisit emit gating. Until then: ungated
  stands.

## 7. Implementation staging (the plan hook)

1. `mwydyn-data`: `TelemetryRecord` + canonical forms + `telemetry`
   verb in the barrier response shape; unit tests.
2. `mwydyn-core`: `Effect::Emit { record }` (+ helpers), the bounded
   ring + `drain_telemetry()` + drop records, runtime fact records
   (lifecycle/refusal/exchange), the injection handle (`Accepted`
   seq + validated parent).
3. `mwydyn-wasm` + `mwydyn-guest`: the emit verb end to end;
   examples grow telemetry.
4. `mwydyn-vectors`: `telemetry-v0` generator + harness.
5. OUT OF CORE, later, maybe never urgent: the OTel mapping layer
   (crate or example — `opentelemetry` crates never touch the core's
   dependency graph; tight core law).

Owner's scope line on record: **OTel mapping is out of scope** for
this round and for the core permanently; the types merely project
cleanly (severity numbers, delta temporality, seq/ack span identity,
context name → resource).

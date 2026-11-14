# 50 — Tasks meet the ambient surface: `call_task_with`

> Status: implemented and green (2026-11-14) — 49 suites, clippy
> clean workspace-wide. Third and final round from the platformc
> handoff (ADR-0019 debt 2). Small by design: the task verbs
> (round 17) predate capability rows at the ambient face (round 40)
> and never met — this round is the meeting. Spec §3.4 amended
> (round-50 bullet): capability-quiet by default, supervision is
> the verb's.

## 1. The debt, closed

`call_task` spawned `Spawn::new(None, factory)` — no `.rows()` —
so any task needing the ambient surface (blob work) was dead on
arrival, and every embedder rebuilt the call pattern by hand:
platformc's bridge re-implemented spawn → post-under-correlation →
run → outbox-match just to add `blob.read`/`blob.write`, and
hardcoded its own correlation token (valid only because its context
was single-exchange — a trap for the second consumer, defused here:
the mint was already core-side all along, `call-N` off the context
counter; what platformc hardcoded was its copy's).

The reframe on record (from the handoff session): this was never a
platformc ask — it was an internal inconsistency in our own
surface. Tasks (round 17) and the ambient face (round 40) were both
core law; the verbs just couldn't carry grants between them.

## 2. The shape (default stated in the handoff session, uncontested)

- **`call_task_with(spec: Spawn, letter)`** — the one-call round
  trip over a FULL spec. Name, rows, init_spec are the caller's
  knobs. The correlation is minted here (`call-N`, unique per
  context); the reply is matched by it in the external outbox;
  non-matching entries survive the scan, as always.
- **`call_task` survives as the documented ZERO-GRANT convenience**
  — literally `call_task_with(Spawn::new(None, factory), request)`
  (behavior-identical: the old body is the new body, and the old
  `spawn_task(factory, &[])` path was already the default-empty
  spec). Capability-quiet by default is now NAMED in §3.4 — the
  ambient surface is granted, never ambient.
- **SUPERVISION IS THE VERB'S, NOT THE SPEC'S**: FailFast is forced
  whatever the spec says. A one-call round trip presumes a task
  that terminates; a long-lived worker is `spawn`, not a call.
  (The forcing is behaviorally pinned, §3.)
- `spawn_task` unchanged, its doc now pointing rows-bearing callers
  at `spawn`/`call_task_with`.

## 3. The pins (`mwydyn-core/tests/task_surface.rs`)

- **ZERO-GRANT DEFAULT** — `call_task`'s task does `blob_put` and
  reports the refusal IN ITS REPLY: `refused: "Unauthorized"`, by
  name, observed — never a fake success, never a swallowed strike.
- **THE ROWS-BEARING CALL** — the SAME actor (one source, two
  verdicts — the capability is the only delta) under
  `call_task_with` with `blob.read`/`blob.write`: staged=true,
  digest present, get-round-trip true, reply riding the minted
  correlation.
- **THE MINT + THE BYSTANDER** — back-to-back calls each match
  THEIR reply (unique `call-N`s); both tasks' uncorrelated
  bystander letters survive both scans — the match never drains
  what it did not match.
- **FAILFAST IS THE VERB'S** — a task that strikes on first touch
  and answers ever after, spawned with `Supervision::Restart` in
  the spec: no reply, dead letter. Under a respected Restart the
  respawned task (shared poison flag) would have answered attempt
  two — the pin's counterfactual is mechanical, not rhetorical.

## 4. Consumer mapping

platformc's bridge deletes its re-implementation and hands
`call_task_with(Spawn::new(None, factory).rows(...), letter)` —
its hardcoded correlation token dies with the copy. ADR-0019 debt 2
closes. All four handoff debts are now landed (48: guest artifacts;
49: prelude codec + forks; 50: task rows); the platformc
integration thread is consumer-side from here.

## 5. State

- `crates/mwydyn-core/src/context.rs` — `call_task_with` + `call_task`
  as the thin zero-grant wrapper (the old body verbatim under the
  new name, plus the forced-supervision line).
- `crates/mwydyn-core/tests/task_surface.rs` — the pins.
- `docs/spec-v0.md` — §3.4 amended: the task pattern as verbs,
  capability-quiet by default, supervision is the verb's.

# 01 — Delivery semantics and supervision

> Status: decided (round 2026-11-08, with the project owner); two
> [proposed] items pending ratification. Amends [spec-v0](../docs/spec-v0.md)
> §3 (gains §3.1, §3.2), §7 (forks 1–2 resolved).

## 1. Delivery — log owns acks

The framing question "does an ack rewind?" resolved: **log owns acks**.

- At-least-once delivery; dedup key = correlation id [A1:04 §4,
  recovered].
- The delivery cursor is part of the durable inbox object, never
  separate bookkeeping — and the owner's rule: **the cursor never
  carries a different durability than the mailbox itself**. RAM inbox
  = RAM cursor; journaled inbox = journaled cursor. Any delivery
  bookkeeping rides the mailbox tier.
- Crash recovery and time travel are therefore the same mechanism:
  rewind re-delivers everything after the rewind point; correlation
  dedup resolves the replays.
- Poison letters: the delivery-attempt count rides the mailbox;
  advancing past a poison message is a policy-authorized cursor
  advance, journaled as a dead-letter fact (rewind-consistent).
- Warning label: rewind re-executes side effects of non-idempotent
  messages already delivered. Correct semantics; stated, not guarded.

## 2. Supervision — policy, not mechanism

The owner's framing: dev/live composition wants fail-fast without
restart (immediate feedback for agent-originated actors); production
wants reliability, warm pools, blue/green deployment, A/B testing.
Forcing one model would be wrong — supervision is a flexible policy.

- Core: lifecycle facts (spawned, delivered, `crashed{trap, message,
  cursor}`, terminal) + a policy attachment at spawn.
- Built-ins: `fail-fast` (dev), `restart` (production: backoff +
  restart-intensity window → escalate / dead-letter).
- Restart state follows the actor's state choice (spec §6): fresh |
  snapshot | replay-from-log.
- Task actors: crash = error reply; the caller is the implicit
  fail-fast supervisor.
- **Single-actor, single-cursor invariant**: one message at a time per
  actor; scale-out is fan-out to more actors, never parallel consumers
  on one inbox. Load-bearing for snapshot/replay coherence.
- Warm pools & parking: instance state is a cache; mailbox+cursor are
  the truth — the A1 substrate shape [A1:21 §5], rediscovered at actor
  scale. Parking = the suspendable contract at rest; pooling = an
  instance cache bound to a name. The PaaS composes them; the CLI
  never pays for them.

**[proposed, pending ratification]**

- No mandatory supervision tree — escalation routes to the owner
  handle; trees are user-buildable (a native actor that supervises
  children).
- Blue/green + A/B ride a **name → implementation binding
  indirection**: swap or split the binding, never the actor.
  Mechanisms in the core API, policy at the deployment layer; lands
  under spec §7.4 (addressing).

## 3. Residue

- Bounded mailboxes & backpressure (credit flow vs block vs drop) —
  its own fork, queued behind the capability model.

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 04 §4 (correlation dedup), 21 §5 (scratch-is-cache shape).

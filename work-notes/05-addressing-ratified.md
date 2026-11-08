# 05 — Addressing ratified; backpressure opened

> Status: naming ratified (round 2026-11-08) — addressing lands as
> [spec-v0](../docs/spec-v0.md) §3.4. The §3.2 [proposed] items are
> both landed (binding indirection → §3.4; no-mandatory-tree by
> consequence). Backpressure proposal opened, reply pending.

## 1. Ratified

**Hybrid naming (owner: "actors that need to be known have stable
names, actors that must only be known by their creator that handles
dispatch/coordination don't"):**

- Naming is opt-in. Named actors: addressable within their context,
  refuse-on-collision. Unnamed actors: only the spawner's handle —
  the task actor.
- A "service" = a named, long-lived actor. No special class.
- Consequence: escalation routes to the owner handle, so the
  no-mandatory-supervision-tree proposal is ratified by consequence.
  The actor contract now carries no pending [proposed]s.

## 2. Opened — backpressure (proposal, reply pending)

- **Base mechanism: bounded mailboxes + explicit refusal.** `post` →
  `ok | refuse{mailbox-full}`; refusals are machine-readable data;
  senders compose their own policy (retry with backoff, reroute,
  dead-letter). No silent drops, no blocked senders — a sender never
  waits mid-message (single-cursor integrity).
- **Credits as a runtime-enforced policy mode, not a new verb**: the
  runtime can enforce per-target credit budgets *behind* the same
  `post` call, replenished as the target's cursor advances. Credits
  are derived from cursor position, so they rewind consistently — and
  the frozen WIT world stays `post` regardless of strategy.
- **Retry storms are self-limiting**: a retrying producer burns its
  own fuel (epoch/fuel budgets); timers (message-shaped) complete the
  composition — defer-and-retry without blocking primitives.
- **Tier defaults [proposed]**: RAM-tier = large-bounded (a full
  mailbox on a task actor is a bug — fail it); log-tier = bounded,
  bounds are runtime config per actor class.

Why this ordering matters for the freeze: backpressure is the last
*semantic* fork whose answer could touch the WIT world. With refusal
as the base surface, the verb set is safe to freeze next.

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 41 (operational refusals vs fatal verdicts — D-059's "a
lying spec never kills"), 04 §3 (journaled ids).

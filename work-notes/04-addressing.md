# 04 — Addressing: names, ids, bindings

> Status: the §3.3 default-deny boundary DECIDED this round; the
> addressing proposal is in discussion, one fork open (name
> allocation & collision policy). Amends [spec-v0](../docs/spec-v0.md)
> §3.3, §7.4.

## 1. Ratified this round

- **Trust-context hybrid** (spec §3.3): type-level authority inside a
  trust context, explicit target rows across contexts. Owner's law,
  verbatim: **"intra-context transparency by default, cross-context
  ceremony."** Cross-context minting is the ceremony that makes PaaS
  service-to-service wiring explicit and deliberate.

## 2. The addressing proposal [proposed]

Two-level addresses, because three different things were being cramped
into one identifier:

- **Names** — `(context, name)`, readable, what actors post to;
  names unique within a trust context. NOT security objects:
  authority is subject-bound rows (§3.3) — which is exactly why
  mwydyn can afford readable addresses.
- **Instance ids** — opaque, runtime-minted, **journaled at
  creation** (replay-stable; the A1 Q7 discipline: legitimately
  random because the id is a journaled injected fact). What lifecycle
  facts, supervision, and logs cite.
- **Bindings** — the name → implementation indirection
  (name → {impl digest, config}). Swap = blue/green; split = A/B;
  a dispatch queue = a binding whose backend is a set + a policy.
  (This formally lands the §3.2 [proposed] item. If the naming fork
  lands as proposed — unnamed actors hold only the spawner's handle —
  the owner-handle escalation route is implied, and the
  no-mandatory-tree proposal is ratified by consequence.)

Location transparency: addresses are data; resolution is a runtime
concern; in-proc vs network never changes message bytes.

## 3. OPEN — name allocation & collision policy

- (a) Explicit names at spawn, refuse-on-collision — deterministic
  and ceremony-friendly, but verbose for dynamic fan-out.
- (b) Minted ephemeral ids + a distinguished "service" class with
  names — two actor natures, more spec surface.
- **[proposed] (c) Hybrid**: naming is opt-in. Named actors are
  addressable within their context, refuse-on-collision. Unnamed
  actors hold only the spawner's handle — which is exactly the task
  actor. A "service" is then simply: a named, long-lived actor.

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 04 §3 (opaque ids, replay-stable minting, Q7), 45 (loop
swaps and reserved ids — the binding-swap precedent).

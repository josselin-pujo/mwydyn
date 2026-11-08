# 08 — Clock domains ratified; envelope field set proposed

> Status: transaction boundary DECIDED (round 2026-11-08); envelope
> field set [proposed, awaiting ratification]. Amends
> [spec-v0](../docs/spec-v0.md) §3.5, §2.

## 1. Ratified — the clock-domain law

**Single-context atomic, cross-context eventual.** Owner's law,
verbatim: *"can't have a single cross-context clock, and atomicity is
only valid inside a clock domain; contexts break clock domains,
because if they don't we have a single LOCK in the sky and that's a
disaster waiting for scale to hit."*

The unification this buys: **the wall is one wall with three names —
trust boundary, clock domain, transaction boundary.** A trust context
is where names are unique, where authority is type-level, where the
journal commits atomically, and where the inbox tick is a coherent
clock. Cross the wall: explicit rows, ceremony, eventual delivery,
sagas. Round 06's clock obliviousness and round 03's capability
hybrid turn out to be the same wall seen from different sides.

## 2. Proposed — the envelope field set

Producer-supplied (from the guest `letter`): `kind`, `payload`,
`cause?`, `correlation?`, `authority`.

Runtime-assigned: `seq` (identity = `(context, seq)` — the context
journal's contiguous position; **on log tier the inbox is a cursor
over the context journal**), `from` (sender instance id — replies
target it; no reply-to field), `to`, `commitTs` (audit only),
`producer` (sending actor's **implementation digest** — provenance
for blue/green and A/B forensics: which version of the code sent
this), `checksum`.

The mwydyn twists vs A1, worth recording:

- **Correlation is demoted.** In A1 it was THE dedup key (Q10); in
  mwydyn the transaction makes redelivery-impossible for committed
  work, so correlation's primary role is reply matching, with dedup
  as defense-in-depth. A1's most careful safety mechanism retires to
  an advisory role because the architecture made it redundant.
- **No `ttl`**: staleness belongs to dead-letter policy and timer
  races, not to the envelope.
- `producer` carries the implementation digest, not just identity —
  the binding indirection's forensic payoff.

## 3. Position

With envelope fields ratified and the schema registry (§7.9) specced,
the walking skeleton gets its shopping list: vectors first, then the
Rust runtime lib + toy actors.

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 04 §§2,3,5,8 (envelope, identity, timestamps, checksums).

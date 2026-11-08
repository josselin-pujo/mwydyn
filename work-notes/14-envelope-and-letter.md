# 14 — Envelope and letter land in mwydyn-data

> Status: implemented (2026-11-08), unit-tested, gates green.
> Amends nothing in the spec — §2's field set, byte-for-byte.

## 1. What exists

`crates/mwydyn-data/src/envelope.rs`:

- `Letter` — the producer-supplied half, structurally incapable of
  carrying runtime-assigned fields (the byzantine posture, made
  type-level). `Letter::new` validates: payload ≤ 1 MiB
  (`LETTER_PAYLOAD_CAP`), kind syntax, non-empty cause/correlation/
  authority entries.
- `valid_kind` — pins the layer-2 kind SYNTAX only:
  `<namespace>.<name>@<major>`, segments `[a-z0-9_-]`, one `@`,
  decimal major. Schema *existence* stays the registry's concern —
  the core remains schema-blind (round 10).
- `Envelope` — letter + seq + from + to + commitTs (Option; `None`
  on the RAM tier) + producer (implementation digest). `to_value()`
  gives the canonical data-model form (field names fixed, sorted by
  the profile's length-first rule: to, seq, from, kind, cause,
  payload, commitTs, producer, authority, correlation);
  `checksum()` = SHA-256 over that canonical encoding — assigned at
  commit (M2), recomputable over a copy to verify bytes.
- `Address` — Named{context, name} | Instance(id).

## 2. Tests (in-crate, not corpus — see note)

Letter validation boundary cases (at-cap passes, cap+1 fails, kind
syntax table), checksum determinism + field binding (seq and commitTs
changes move it), canonical-form roundtrip through the crate's own
profile. Grep-tested via corpus later if a second implementation
needs the wire form; for now these are Rust unit tests because
letters are API-level, not wire-level — the ENVELOPE's wire form
(to_value) is already corpus-compatible and pinned by roundtrip.

## 3. Next

`mwydyn-core`: contexts, mailboxes/cursors, structural admission,
the effects engine, atomic in-proc commit, `fail-fast`/`restart`,
the ticker, the RAM blob store, the task pattern — native Rust
actors over the `receive → effects` trait (compose-parity).

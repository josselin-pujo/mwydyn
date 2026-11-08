# 29 — §7.6 snapshots: boot record, the ack stamp, name-keyed restore

> Status: DECIDED + implemented (2026-11-08). World draft v0.5 (boot
> record typed), Envelope gains runtime-stamped `ack`, snapshots
> sink collocated, snapshot letters name-keyed, hydrate integrates
> covers-skip. Gates: 17 walking + 5 face + 4 envelope units + 2
> corpora + 2 schema, clippy clean.

## 1. The cursor question — DECIDED

*"Who decides the actor's read cursor?"* — **THE HOST, from the
journal.** The chain: round 01 (log owns acks) + §2 (byzantine
posture — the guest cannot write runtime fields) + the hazard
analysis (actor-chosen cursors can skip [loss] or rewind [double
effects — effects are journaled, re-processing duplicates them]).
The resolution mechanism makes it exact:

- **The `ack` stamp**: every journaled letter gains a runtime-stamped
  `ack = Option<seq>` — the position of the letter its sender was
  processing at send time. Minted in the SAME transaction as the
  sender's ack (round 08), so a snapshot letter stamped ack=P is
  PROVABLY covers-position-P: race-free (the serial commit is what
  makes the stamp exact), forgery-proof (runtime-minted).
- Restore: `cursor = snapshot_letter.ack`; blob ref → boot.

## 2. The boot record (world draft v0.5)

- WIT-typed: `boot { actor: actor-id, parent: address, snapshot:
  option<blob-ref>, config: list<u8> }` — the runtime-MINTED
  identity fields are the CONTRACT (evolving them = a world-version
  bump = deliberate); `config` stays opaque canonical CBOR with
  reserved-key discipline (layer-2 evolution, no WIT churn) — the
  round-10 two-layer doctrine, applied.
- `parent` REQUIRED (external for composition spawns): a fresh
  actor's only handle for initiating.
- Actor-init signature: `init(&mut self, host, boot)` — boot is the
  RESTORE point.

## 3. Name-keyed persistence ("legion but named")

Only NAMED actors get snapshot restore — persistence keys on the
STABLE name (ids are per-process mints). Unnamed (task) actors are
ephemeral by lifecycle. The owner's shard pattern: a coordinator
spawns legion children named `shard-N` — deterministic, collision-
free under §3.4's refuse-on-collision, each snapshotting under its
own key. "Compared to EJB, very low ceremony."

The snapshot letter: `mwydyn.snapshot.save@1` to the well-known
`snapshots` SINK (runtime-reserved, collocated, receives-and-ignores
— the journal IS the registry; boot-scanned directly, no hydration
dependency). Payload: `{name, ref}` — the blob is GC-immune via
round-27 reachability. Core schema added to `core_schemas()`.

## 4. Hydration integration

`hydrate()` re-initializes from the STORED boot (snapshot restore
included) and replays `letters_for(actor)` FILTERED to
`seq >= snapshot.covers` — the covered prefix is redundant (the
boot restored state-as-of-covers; replaying it would be identical
work). Both paths agree by determinism (§3.5 purity).

## 5. Ghost/rewind/GC interplay (all automatic)

- Snapshot letters on ghost branches: excluded from restore scan
  (active-lineage only) — a rewind restores the world INCLUDING
  which snapshots exist.
- Snapshot blobs: journal-referenced → round-27 reachability keeps
  them. Retention (old snapshots) is future policy, not a hole.

## 6. The HotSpot scar, honored

Actor-managed snapshots (state serialized by the actor's own code)
sidestep the JIT/memory-copy swamp entirely — §7.6 stays a
CONTRACT, not a wasmtime feature. The hypothetical WASM
memory+stack copy remains exactly that: hypothetical.

## 7. Gates

17 walking (snapshot_letter_survives_reopen...: adds 1+2,
snapshot, die, reopen+hydrate → COUNT:6 with no double-count —
covers exactness proven) + 5 face + 4 units + 2 corpora + 2 schema;
clippy clean (JournalOp size linter allowed with justification).

## Provenance

Owner design (snapshot-as-letter, well-known address) + cursor
question; host-decides answer ratified with boot-channel choice.
Sources: round 01 (log owns acks), §2 byzantine posture, round 08
(transaction atomicity = stamp exactness), round 19 (ghost
discipline), round 27 (GC reachability).

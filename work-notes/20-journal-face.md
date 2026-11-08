# 20 — M2 opens: the journal face and its first provider

> Status: implemented and green (2026-11-08). `journal.rs` in
> `mwydyn-core`: the §8 contract as a trait + `InMemoryJournal`.
> 4 face tests green; full suite green (11 walking + 4 envelope
> units + 4 face + 2 corpora + 2 schema); clippy clean.

## 1. What exists

- **`JournalFace`** (§8 as a trait): `commit(context, ops)` — the
  atomic transaction `{Append, Ack, DeadLetter, Fork, Mint}`;
  `read(lineage, from, to)` — inclusive, verified, works on ANY
  lineage (live or ghost); `pending(actor)` / `pending_count` (the
  mailbox IS a cursor over the journal); `cursor(lineage, actor)`;
  `active_lineage`; `lineages`.
- **`InMemoryJournal`** — the substrate-independence witness. Key
  mechanics: forked prefixes are NOT copied — child-lineage reads
  delegate to parents (storage dedup for free; a ghost branch costs
  nothing until it grows its own tail); forks seed child cursors
  CLAMPED to the fork point and carry unacked letters within the
  shared prefix; beyond-history is a loud error, never a silent gap.
- Face tests pin: identity stamping (seq/commitTs/checksum),
  pending/cursor flow, fork + shared-prefix delegation + THE GHOST
  READ (parent tail beyond the fork), fork carrying unacked prefix
  letters, cursor clamping.

## 2. Design notes for the record

- `Pending.to_actor` is the pending-index key ("" = no actor
  delivers it — the external outbox letter). The runtime fills it
  with the target entry's id.
- Dead-letter facts ride the same transaction as their paired Ack;
  the fact is the audit trail, the Ack is the cursor advance (§3.1).
- Mint facts journal for replay (§3.3 tier rule); row
  reconstruction is hydration (next slice).
- commitTs comes from an injected ClockFace — determinism for tests,
  wall clock for production.

## 3. Next (in order)

1. **Context switch**: mailboxes become journal-backed (the VecDeque
   goes away); busy checks read `pending_count`; strikes commit
   DeadLetter+Ack; `rewind(position)` becomes a Context method.
2. **The ghost gate**: `Host::ghost_read` behind the `ghost.read`
   cap — the "solid gate now" the owner required (granted → works;
   absent → Unauthorized; both tested).
3. **SQLiteJournal** behind a cargo feature, passing the SAME face
   tests (substrate independence).
4. Hydration (rebuild actor rows + schedules from the journal) —
   the M2b that makes crash recovery real.

## Provenance

Owner ratification (fork + ghosts + ghost.read default-off + double
implementation), 2026-11-08. Sources: spec §8; A1 work-notes 04 §8,
21 §5.

# 22 — M2 part 2: the context switch — mailboxes become cursors

> Status: implemented and green (2026-11-08). The `Mailbox` struct is
> GONE; the mailbox is a cursor over the journal (§3.1, literal).
> 12 walking tests — the WHOLE behavioral suite survived the
> substrate swap — plus the new rewind/ghost-gate test. Full gates
> green, clippy clean.

## 1. What changed

- **The pump's commit is now ONE journal transaction per receive**
  (round 08, literal): `{Append(effect posts), Mint facts,
  Ack(delivered seq)}` — log owns acks, mechanically.
- **Busy refusal = no commit at all.** The letter stays pending on
  the journal and is re-delivered on a later pass — the requeue
  machinery deleted itself; self-healing is now just "the letter is
  still there."
- **Strikes** are runtime bookkeeping keyed (actor, seq),
  deliberately NOT journaled (retry policy state; a crash resetting
  strikes just re-tries — at-least-once permits).
- **Terminal actors** drain pending letters via journaled
  DeadLetter+Ack pairs (fail-fast stays loud).
- **`Context::rewind(position)`** — commits the Fork, restarts all
  actors FRESH (state-as-of-position is §7.6's snapshot contract;
  fresh is the documented M2 approximation), clears strikes.
- **The ghost gate** (the owner's "solid gate now"):
  `Host::ghost_read(lineage, from, to)` behind `ghost.read` rows;
  per-lineage attenuation via the object grammar
  (`ghost.read:*` / `ghost.read:<lineage-id>`). Tested three ways:
  wide grant reads ghosts; per-lineage grant reads lineage 1 but is
  denied on the active lineage 2; grantless is denied outright and
  produced no reads (exactly-2 assertion).
- `Commit` now carries the stamped `appended` envelopes — the
  external outbox view feeds from journal truth, not a parallel
  path.
- Poll letters are JOURNALED now (they are letters; on the log tier
  they replay).

## 2. The proof

The 12-test behavioral suite — atomic rollback, busy self-heal,
restart/fail-fast supervision, ticker on the manual clock, call_task
round trip, external outbox — passed the substrate swap with only
assertion-string updates. That is the payoff of vectors-first and of
behavioral tests written against the CONTRACT, not the internals.
(One real bug was found and fixed: the rewritten phase-4 initially
lost the external-outbox feed; `Commit.appended` now carries stamped
envelopes for exactly such views.)

## 3. Next (M2 remainder)

1. `SQLiteJournal` behind a cargo feature, passing the SAME face
   tests (`journal_face.rs` parameterized over providers).
2. Hydration: rebuild actor rows + ticker schedules from the
   journal — crash recovery becomes real.
3. `Context::with_journal` is the durable door; a walking test with
   a SQLite-backed context closes M2.

## Provenance

Owner "Onward!", 2026-11-08. Sources: spec §3.1, §8; work-notes
19–21.

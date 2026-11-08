# 23 — M2 part 3: the SQLite provider — substrate independence proven

> Status: implemented and green (2026-11-08). `SQLiteJournal` behind
> the `sqlite` cargo feature (DEFAULT ON — the owner uses SQLite
> everywhere as a file format and persistence system). The face
> tests are PARAMETERIZED: in-memory and SQLite must pass the same
> four conformance suites, plus a reopen-persistence test only
> SQLite can run. Full gates green, clippy clean.

## 1. What exists

- `sqlite_journal.rs`: `SQLiteJournal` — WAL + `synchronous=FULL`
  (the durability point: a returned COMMIT is crash-consistent);
  entries stored as canonical CBOR with checksum columns and
  VERIFY-ON-READ (integrity failures are loud, §8.4); forked
  prefixes NOT copied — reads delegate to parent lineages through
  the same segment-partition logic as the in-memory provider
  (sorted ascending — segments are discovered parent-tail-first);
  cursors lazy-clamped through the fork chain (no cursor copying);
  facts (dead-letter, mint) journaled in their own table; contexts
  implicit (root lineage + active pointer created lazily).
- **Face tests parameterized over providers** — both must pass:
  identity stamping, pending/cursor flow, fork + ghost read +
  shared prefix, fork carry + cursor clamp. THE SUBSTRATE
  INDEPENDENCE PROOF: `5 passed` = 4 suites × 2 providers + the
  SQLite-only reopen test.
- `Envelope::from_value` in mwydyn-data — the inverse of
  `to_value`; the storage roundtrip of the digest law.
- `JournalError::Storage(String)` + `Integrity(&'static str)` for
  provider-side failures (mapped honestly, never swallowed).

## 2. Findings from the SQL round

- The provider loop in tests caught TWO real provider bugs the
  in-memory impl could never have: segments assembled parent-tail-
  first (wrong replay order — must sort ascending by `from`), and
  pending ranges passing `u64::MAX` as the history end (BeyondHistory
  on every call; must use the lineage's covered total with an
  early-empty for fully-acked cursors).
- `cursor_of` learned the SQL lesson: a missing cursor ROW is not a
  NULL column — `QueryReturnedNoRows` is the "nothing acked yet"
  case, and the fork-chain lazy clamp (min(parent cursor, fork
  point)) is where per-lineage cursors earn their keep.
- The bundled rusqlite build works on this box (MSVC present) —
  ~first-build cost only, default feature dependency accepted by
  the owner (SQLite everywhere, as a file format).

## 3. Next (M2 close-out)

Hydration: rebuild actor rows + ticker schedules from the journal
(Mint facts + arm letters replay) — crash recovery becomes real.
A durable-context walking test (with_journal over a file, drop,
reopen, keep working) closes the milestone.

## Provenance

Owner directive (default feature, double implementation for
substrate independence), 2026-11-08. Sources: spec §8; A1
work-notes 04 §8 (checksum discipline).

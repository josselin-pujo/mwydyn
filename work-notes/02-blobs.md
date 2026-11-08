# 02 — Blobs leave the actor model

> Status: decided (round 2026-11-08, with the project owner). Amends
> [spec-v0](../docs/spec-v0.md) §4 (rewritten), §3.2 (dispatch
> queues), §7.4. Supersedes round 00's "blob actor" decision —
> before any code existed; fix-forward note here, the spec is
> authoritative.

## 1. The argument (owner, ratified)

Every actor potentially reads blob bytes. If blob bytes never transit
messages, then blob load/store cannot be messages — a reference can
only be sent once the bytes are written anyway. So blob load/store is
host API: part of the frozen WIT world, with a CBOR extension type for
references. The Athena shape.

## 2. Why it holds (the stress test)

The actor model contributes two things — mutation management and
ordering. Blobs have neither: immutable, content-addressed,
idempotent. And the time-travel stress test passes *without*
journaling puts: `put` is a pure function of bytes → digest, so a
deterministic replay re-puts to the identical digest; a missing blob
is a loud integrity failure [A1:04 §10]; GC is reference-aware above
the live watermark.

## 3. The principle that fell out

**A host service may bypass the message layer only if it is
deterministic** (replayable without journaling). This is why blob is
ambient but clock is not — `clock.now()` under replay diverges; clock
reads must be journaled (fork §7.7). Randomness: mediated. Timers:
message-shaped (a timer event IS a message). Telemetry: lossy by
design, exempt from replay semantics entirely.

Consequence: round 00's "well-known service registry" dissolves —
ambient primitives live in the frozen WIT world; mediated services
ride capabilities and the log; query-shaped blob surface (search,
listing), if ever needed, is a native actor and never WIT growth.

## 4. Decisions

1. `blob.put / get / len / get_range` — synchronous host functions in
   the frozen WIT world (range/len so consumers stream without
   materializing; docgen's PDFs).
2. The blob reference is a first-class CBOR tagged type, shape
   `{ digest, mediaType, bytes }` [A1:05 §2].
3. Blob bytes never transit messages (1 MiB cap + by-reference law
   unchanged; writes precede any reference).
4. TTL is a `put` parameter; never ambient [proposed default:
   unreferenced bytes sweep by runtime GC configuration only].
5. Capability checks happen at the host-function boundary (fork §7.3
   places the grant rows).

## 5. Also this round

- Single-actor, single-cursor: ratified. Stateless actors may offer
  parallel implementations — each with its own mailbox. **Dispatch
  queues are routing primitives, not consumers**: no mailbox, no
  cursor, no actor semantics (spec §7.4).

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 04 §10 (replay-safe content addressing), 05 §2 (digest
reference shape), 51 (layering law), 41 (JournalBlobs face — the
precedent for ambient blob access).

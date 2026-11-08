# AGENTS.md — working notes for coding agents

mwydyn is a standalone WASM actor runtime. It is NOT Athena: Athena2 is
one consumer among five (see README's matrix). Design authority is the
consumer matrix — when consumers disagree, the intersection goes in the
core and the union goes behind layers.

## Documentation layout

- `docs/spec-v0.md` — the contract. Sections marked **DECIDED** are
  ratified; **OPEN** marks the discussion queue, taken one fork at a
  time with the project owner. The spec is authoritative:
  implementation drift is surfaced for arbitration, never silent.
- `work-notes/NN-*.md` — the numbered design journal (Athena
  convention). Append-only history: fix forward, never rewrite.
- `vectors/` — conformance vectors (planned). A spec section isn't
  done until its vectors exist.

## Working agreements

- The round cycle (user rule): brainstorm → docs → plan → implement.
- Questions to the user go one at a time, opinionated default first.
- No code before the spec's first sections carry DECIDED status.
- Spec text cites provenance: `[A1:NN]` = Athena1 `work-notes/NN`;
  recovered specs, not recovered code.

## State (2026-11-08)

Rounds done: charter (00) through schema registry (10) — spec §1–§6
all DECIDED. Owner's laws on record: clock obliviousness, "CBOR is
just CBOR", no reply-to, atomicity-only-inside-clock-domains,
refusal-minting-is-policy, compose-parity (native and WASM actors
compose identically), faces-not-implementations (journal/blob),
DISTRIBUTED JOURNALS ARE PLUGGABLE CRATES, NEVER CORE (round 25:
NATS/Kafka/consensus providers live outside the core's dependency
graph — tokio/TLS/client-SDK waves must never touch it; SQLite is
the owner-sanctioned in-core default feature),
NON-DETERMINISTIC BUT RECORDED (round 21: the journal is law — races
are real-time only, the recording is what happened; replay follows
the journal; transitions run in parallel, commits serialize),
APACHE-2.0 + LICENSE ADMISSION LIST (round 24: pure Apache-2.0
project license; linked copyleft GPL/AGPL/LGPL/SSPL refused —
external services don't propagate, only linked crates; every new
dependency passes `cargo deny check licenses`, policy in deny.toml,
snapshots in sbom/; prefer aws-lc-rs over ring when QUIC lands;
JSC's linked-LGPL is the named reason to keep JS on WASM/QuickJS-ng),
TIGHT CORE (round 18: if the contract never reads it, the core never
carries it — core deps = mwydyn-data only; mwydyn-schema is a peer
crate, the runtime is registry-blind as well as schema-blind).
**Implementation: M0 OPEN** (plan ACTIVE in [docs/plan.md](docs/plan.md)):
workspace live; `mwydyn-data` canonical CBOR profile **v0.1** built
(tag 1002 blob refs, hash-bound mediaTypes — round 12: content
identity includes type; digest = SHA-256(canonical CBOR of
{bytes, type})); **both corpora GREEN**: cbor-profile-v0 (59 cases) +
blob-refs-v0 (13 cases). Corpus wants are machine-generated
(`patch-corpus` example) — never hand-type hex; encode is a
serializer, not a validator (forged refs must be encodable for
rejection vectors). Toolchain rustc 1.99.1. **Envelope + letter
landed** ([envelope.rs](crates/mwydyn-data/src/envelope.rs) — byzantine
posture as types, kind syntax pinned, canonical form + checksum;
unit tests green). **`mwydyn-core` RUNS** (work-notes/15): native
actors over `receive(ctx, host, letter) → effects` (MsgCtx = runtime-
minted from, read-only); four-phase effects engine (transition →
validate-all → classify → journal-commit); **the mailbox IS a cursor
over the journal** (M2 part 2, work-notes/22: Mailbox struct GONE,
one journal transaction per receive, busy = no commit at all,
`Context::rewind` forks ghost branches, `Host::ghost_read` behind
`ghost.read` caps — solid gate, three-way tested); FailFast/Restart;
ticker on an injected ClockFace (durations never timestamps); RamBlobStore with verify-on-read (hash-bound
types); round-robin delivery (a busy actor must not starve
neighbors — rotating scan start, work-notes/15). **Round 16:
ALLOCATION IS NOT AN EFFECT** — blob.put is an eager ambient call
(`Host::blob_put`, ref in hand), not an effect; effects = post/
mint/emit only; world draft v0.3 (`ambient` interface). 10 walking
tests + 4 units + 2 corpora green, clippy clean. **Round 17:
`mwydyn-schema` lives** (registry: idempotent claims, conflict
refusal, digest-pinned projections; core schemas
`mwydyn.timer.{arm,tick,poll}@1` ship in every context — CDDL
carried verbatim, validation-by-CDDL is M1+). **The external outbox
landed** — posts to `Instance("external")` drain via
`drain_external()` (the embedding's handle, §3.4); `spawn_task` +
`call_task` are the task pattern as verbs. **M2: journal face +
TWO providers** (work-notes/20–23): `JournalFace` (§8 as a trait,
Send+Sync) with InMemory + SQLiteJournal (`sqlite` cargo feature,
DEFAULT ON — bundled rusqlite; WAL + synchronous=FULL = the
durability point; entries as canonical CBOR, verify-on-read; forks
delegate shared prefixes to parents — no copying). **Face tests are
PARAMETERIZED over providers** (journal_face.rs) — the substrate
independence proof. Ghost gate: `Host::ghost_read` behind
`ghost.read` caps, three-way tested. **Next:** hydration (rebuild
rows/schedules from journal) + durable-context walking test closes
M2; then the executor (round 21) becomes buildable.

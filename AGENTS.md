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
THE ACTOR CONTRACT STAYS SYNC FOREVER (round 30: Actor never grows
async — await is already implemented by the runtime [letters are the
resume]; async-LOOKING ergonomics live in SDKs as local lowerings
[Router / correlation tables / deadline-races / pinned-future
drivers], deferred to first need — compiler trickery crosses the
bridge only when no dispatch alternative exists),
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
unit tests green). **M3 CLOSED** (work-notes/27): `BlobFace` (put/get/len/retain/count,
Send+Sync, boxed passthrough) + RamBlobStore + SQLiteBlobStore
(`sqlite` feature, verify-on-read); `Context::gc()` — reachability =
tag-1002 digests across EVERY lineage (active AND ghost: ghosts are
readable, so ghost-referenced blobs survive GC); payload scanning is
data-model work (tag 1002), not schema — round 10 intact, sweep-only;
`Context::with_stores` = the durable combo constructor.
**ROUND 34: WORKER POOL + IO PIPELINING VERIFIED** (work-notes/33):
`ctx.workers(n)` wave-dispatch pool — transitions (forks included)
parallel on workers, the pump thread COMMITS in dispatch order
(journal order = dispatch order; serial pump stays as the
deterministic mode); serial/pool semantic equivalence + WASM
compose-parity under the pool PROVEN. **IO PIPELINING**: hot journal
applies per-receive transactions instantly (buffering acks in the
hot face = re-delivery — proven by the m2×50 lesson; the mailbox is
a cursor); the OVERLAY drains to the durable face on POLICY
(`max_ops_per_flush` default 256 + explicit `flush_durability()`
barrier) — VERIFIED at the SQLite boundary: 2,000 exchanges → ONE
transaction; knob drives the count (K=64 vs K=512, ≥4×);
crash-before-drain redoes from the durable point. Two latent defects
found and fixed: group commit was actually per-exchange; the
overlay was a silent no-op (inherent-vs-trait method shadowing —
face-level provider tests would have caught it; conformance vectors
queued). hydrate() reads durable history on the overlay tier.
ROUND 35: THE FACE IS CQRS (owner-ratified (b)) — `JournalRead`
(parallel readers: hosts/workers) split from `JournalCommit` (single
dedicated writer: the committer); providers implement both halves;
`Host::new` takes `&dyn JournalRead` (read-side host by type).
Mechanics: journal = `Arc<RwLock<Box<dyn JournalFace>>>`, blobs =
`Arc<dyn BlobFace>`; workers spawn ONCE per run (std::thread::spawn,
Arc handles, zero unsafe), take read guards; the committer alone
takes write guards between waves; guards are FIELD-scoped fns with
poison recovery (into_inner). Throughput re-measured: 8 WASM guests
serial 3,206/s → pool(4) 3,921/s (1.22×, first real speedup);
cheap-transitions stays serial-favored (pool = the
expensive-transitions tool; serial pump = deterministic + cheap
mode). ROUND 36: WAVE PIPELINING LANDED — next wave dispatched before the
current is collected+applied (compose skips dispatched indices;
results GLOBALLY sequenced, buffered per wave — pipeline interleaving
otherwise mismatches actors to envelopes; re-arm composition after
apply or the loop quiesces early). `JournalRead::pending_first()` —
the profile exposed compose cloning whole queues for `.first()`
(435ms of 439ms wall!); the peek transformed the numbers. MEASURED
AMDAHL POINT (work-notes/34): echo committer = 34.6µs/ex (bound
~29k/s — COMMITTER-BOUND: cheaper apply/compose is the lever, not
workers); WASM wall 248µs/ex vs committer 148µs/ex (worker-bound
approaching: fork 120µs + receive 71µs per worker per exchange —
§7.6 WASM snapshots kill the fork, then apply, then more workers).
`PumpStats.profile` = the phase ledger (PoolProfile) for future
profiling.
**M1 WASM container RUNS** (work-notes/31): `WasmActor` wrapping
wasmtime Store + Instance + TypedFunc; ping/pong through Context
pump at 3434 bounces/s (5000 bounces, 2 WASM actors, RAM tier);
WASM execution is NOT the bottleneck — the framework per-letter
cost dominates; WASM floor: 1.4M calls/s raw wasmtime.
**ROUND 33: CBOR ALL THE WAY — WIT IS DEAD** (work-notes/32):
canonical CBOR bytes ARE the WASM ABI; no component model, no WIT
(`wasmtime = "26"` lean, engine.rs deleted, Windows __fastfail
limitation died with it). The barrier contract lives in
[mwydyn-data/src/barrier.rs](crates/mwydyn-data/src/barrier.rs):
request `{"from","letter","v"}` / response
`[{"post":{"to","letter"}}]`, `alloc`/`dealloc`/
`process(ptr,len)→[u32 len][bytes]`; guests compile to plain
wasm32-unknown-unknown with ZERO imports (no WASI, nothing to
fake); guest-side codec IS mwydyn-data (one canon, both realms).
**THE PAYLOAD WALL**: non-canonical payload bytes refused at
`post` and in deliver Phase 2 — native and WASM identically; zero
bytes are NOT a payload (use Null/0xF6); the journal never stores
undecodable payloads. `mwydyn-guest-minimal` = the real guest
(work-order transform: totals items, stamps hops, reply-rides-from
after the first hop, reports OUT to the external outbox at budget);
rebuild via `scripts/build-guest.sh` (artifact gitignored, tests
skip-with-message if absent). Byzantine proof: garbage guest
responses are CRASHES through the real strike path, never silent
swallows. CBOR-tier throughput ~330 hops/s (growing tree +
re-instantiate-per-fork are the costs; §7.6 snapshots are the fix).
All suites green, clippy clean workspace-wide.
**`mwydyn-core` RUNS** (work-notes/15): native
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
`call_task` are the task pattern as verbs. **M2 CLOSED** (work-notes
20–26): journal face + InMemory + SQLiteJournal (parameterized face
tests = substrate independence); mailbox = cursor over the journal
(one journal transaction per receive); rewind = journaled fork,
ghost branches behind `ghost.read`; **hydration** (round 26) =
composition declarative at open, replay of DELIVERED letters
(1..=cursor) with effects discarded + clock pinned to journaled
commit times + Mint facts rebuilt into rows — idempotent via
base_rows reset; durable-context test proves work→die→reopen→keep
working with exactly-once delivery. **Next:** M3 blob persistence +
GC; the executor (round 21); WASM guests (world v0.3 binding);
§7.6 snapshots for state-as-of-position.

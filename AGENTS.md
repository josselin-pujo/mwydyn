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

## State (2026-11-10)

Rounds done: charter (00) through the Rust guest SDK (41) — spec
§1–§6 plus §8 journal and §9 WASM guest contract all DECIDED (§9
consolidates rounds 33/37/39/40/41). Owner's laws on record: clock
obliviousness, "CBOR is
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
**ROUND 37: DECLARED STATEFULNESS — the persistent WASM instance**
(work-notes/35): instances are LAZY + PERSISTENT (per-letter
instantiation GONE — the owner diagnosed it: 120µs was memory-init,
not call cost, and stateful guests were impossible by construction).
The guarantee follows the guest's DECLARATION: Stateful =
pre-receive linear-memory IMAGE rollback (exactly-once internal
state, doubled footprint, image KEPT through the strike chain —
apply a clone, never consume); Stateless = "just reused" (mutated
instance stays on refusal, no guarantee — rides to the core as
`Actor::fork_carries_state`, both pumps branch on it). Stateful
guests keep state in LINEAR MEMORY (byte-exact restore); mutable
globals revert on restore (fork(&self) cannot read them —
wasmtime mut-context). PROOF: one counter guest, both modes — 3
strikes then honest letter: stateful count=1, stateless count=4.
NUMBERS: serial 2,671→4,189/s (+57%), pool(4) 3,799→9,829/s
(2.35× serial), wall 248→86µs/ex — the fork lever is dead; next
levers are pipelining depth + apply cost. Guest lifecycle ABI for
snapshot-restore-from-boot queued (§7.6).
**ROUND 38: DEPTH-N PIPELINING + APPLY-COST HUNT** (work-notes/36):
PIPELINE_DEPTH=4 (fill-loop dispatch; round-36 invariants hold);
micro-profile showed journal.commit = THE apply cost (10.6µs vs
333ns payload wall) — fixed by `RecordingJournal` (the overlay's
recording is a WRAPPER; the hot journal records nothing — the
round-34 inherent-vs-trait no-op class dies with the field) and
`Commit.appended` now external-ONLY (internal appends pay no clone;
single consumer is the outbox check). Numbers: echo pool(4)
**30,764/s (7.9× serial, wall 23.8µs/ex ≈ committer bound 21.8µs —
THE PIPELINE SATURATES THE COMMITTER)**; WASM serial 6,249/s
(persistent instances help serial too), pool ~9.5k/s with committer
(~15.2k) and worker (~14k) bounds BALANCED — wave granularity is the
residual. Throughput ledger 34→38: echo 3.0k→30.8k, WASM 2.5k→9.5k.
Next levers (not urgent): cheaper stage (letter/rows clone per job
≈5.8µs/ex), ack front-shift, worker count — the features board is
the better spend.
**ROUND 39: §7.6 GUEST SNAPSHOT ABI + CONFORMANCE VECTORS**
(work-notes/39): guests MAY export `restore(ptr,len)` (boot
snapshot → blob_get host-side, CAP-GATED blob.read, loud if missing
with state present) and `snapshot()` (`Context::snapshot_actor` →
`Actor::snapshot_state` face → runtime blob_put → covers-stamped
snapshot letter, covers = actor's cursor; ref rides DIRECT BlobRef —
GC-VISIBLE). LATENT BUG FOUND+FIXED: the native counter's Bytes-
wrapped ref NEVER matched scan_snapshot — restore ran VACUOUSLY
(full replay reached the same count; the test couldn't tell) and the
snapshot blob was GC-INVISIBLE; test now hardened with a RESTORED
marker. Durable round-trip proof: 3 letters → snapshot → pending →
die → reopen(SQLite) → boot restore → hydrate skips covered prefix →
exactly-once (count=4). VECTORS: overlay-v0 (recording skin records/
drains through the FACE, durability face silent, plain hot silent —
the round-34 no-op regression gate) + barrier-v0 (canonical
request/response forms machine-generated + refusal table — any
second-realm decoder consumes it).
**ROUND 42: ambient-v0 CONFORMANCE VECTORS** (work-notes/42; spec
§9.4 cites the corpus): the ambient-import contract AS DATA, the
barrier-v0 pattern applied verbatim — machine generator
(`gen-ambient-v0`), committed corpus (23 cases), harness
(`mwydyn-vectors/tests/ambient.rs`). Pins: the FOUR canonical
request forms (blob.get/len with a REAL machine digest, blob.put
ttl-null + ttl-int, caps); the REFUSAL TABLE (unknown verb BY NAME,
ref missing/not-a-ref, bytes/mediaType/ttl wrong types, negative
ttl, non-map, verb-not-text, garbage); the RESPONSE ENVELOPE FORMS
— ok per result shape + `{"err"}` with every dispatch refusal's
EXACT SPELLING (Unauthorized/Missing/Integrity/Unavailable — a
drifting second-realm SDK fails the corpus). INVARIANT: every case
pins its canonical hex — malformed SHAPES are canonical bytes too
(ONE desugar, generator-side, for $hex/$ref). Corpus refs are
VALIDATED, never trusted (the runner's model law). Out of scope,
recorded: the -1 retry protocol (linear-memory mechanics) and SDK
reader diagnostics (SDK host tests pin them). 32 suites / 72
tests, clippy clean.
**ROUND 41: THE RUST GUEST SDK — THE GUEST DECLARES, THE RUNTIME
OBEYS** (work-notes/41; spec §9.6): `mwydyn-guest` is the
author-facing crate — one dependency, one trait, one entry line.
`Actor::receive(letter BY VALUE, &Cx) -> Vec<Effect>` (mirrors the
native contract — compose-parity of the API itself), `Durable`
(snapshot/restore), THE THREE WORDS: `entry!` / `stateful_entry!` /
`durable_entry!` (durable implies the stateful bit AND
compile-REQUIRES `Durable` — the vacuous-snapshot class dies at
build time). Typed `Cx` (blob get/put/len/caps — refusals BY NAME
as `Refusal`), `Effect::post/reply/reply_via`, `ValueExt`/`LetterExt`
payload getters, `fail!` = the documented strike path (panic =
strike, all of it in the crate doc — DOCS-AS-PRODUCT: the crate doc
IS the guide). **STATEFULNESS IS GUEST-DECLARED (§9.2,
owner-ratified in session)**: every entry macro emits
`mwydyn_decl() -> u32` (bit 0 = stateful; stateless = explicit
zero; ABSENT = Stateless by omission — hand-rolled guests, WAT
fixtures); the container probes it ONCE at composition (one
throwaway instantiation — the probe store needs its own
`set_epoch_deadline`, the default is EXPIRED: found the hard way),
`from_compiled_stateful` is REMOVED — **no embedder override
exists** (forcing a stateful guest stateless silently breaks
exactly-once; refused by removing the path). The `replyTo`
payload-field convention is RATIFIED (§9.5 DECIDED — owner, after
seeing it in force). THE PANIC WALL: generated exports catch + abort
INSIDE the extern boundary — host targets cannot unwind through
`extern "C"` (STATUS_STACK_BUFFER_OVERRUN, found the hard way); wasm
traps natively; loud either way. Examples: counter → `durable_entry!`
(round-39 durable proof unchanged), store → `entry!` (receipt shape
preserved, capRows always present), NEW `guest-dedup` = THE DUAL
PROOF alive under guest-declaration: ONE source, TWO artifacts
(`--features stateless-build`) — stateful rolls the strike-dirtied
seen-mark back ("b" still fresh), stateless keeps it; plus WAT
probe fixtures (absent / explicit-zero / bit-0). `build-guest.sh`
now builds ALL five artifacts (it had rotted to minimal-only).
DOC MAINTENANCE: spec §9 consolidates the WASM guest contract
(rounds 33+37+39+40+41) — the WIT draft is annotated SUPERSEDED in
the §7 queue, `world-v0.wit` is RETIRED (git history keeps it),
README/plan/spec swept of dead-interface references. All suites
green (31 suites, 71 tests), clippy clean workspace-wide.

**ROUND 40: THE AMBIENT HOST-FACE IMPORT SURFACE** (work-notes/40):
ONE import — `mwydyn.ambient(in_ptr, in_len, out_ptr, out_cap) ->
written` — canonical CBOR envelope in (`{"verb": ..., <fields>}`),
`{"ok"|"err"}` out, `-1` = response did not fit (SDK retries with
bigger scratch). Verbs mirror the native Host face WITH the same cap
checks (`row.holds`): blob.get/blob.len (blob.read), blob.put
(blob.write), caps (free) — NO clock verb (§6: the ticker is the
only wall-clock reader); unknown verbs refused by name. The pumps
hand each actor its `AmbientParts` (journal read Arc, blob Arc,
context name, ITS OWN rows) via `Actor::set_ambient` before receive/
init/fork — granted, never ambient. `mwydyn-guest` = THE GUEST SDK
(rlib: alloc/dealloc/respond/ambient_call; import gated by
`#[link(wasm_import_module = "mwydyn")]` — a bare extern lands in
env; host-target builds get a refusing stub). Proof: ONE store guest,
capped (put/get/len green, caps lists rows) vs uncapped
(`putErr: "Unauthorized"` OBSERVED BY THE GUEST, zero fake
successes). SDK lesson recorded: the scratch write that wasn't
there — alloc hands ZEROED memory.
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
working with exactly-once delivery. **ROUND 44: THE TELEMETRY
OUTBOX** (work-notes/44; spec §3.6, three commits, 37 suites):
closed-kind records (`log | metric | exchange | lifecycle |
refusal | drop`) in mwydyn-data — canonical CBOR, guest-declare
form = log|metric ONLY with minted fields refused (byzantine
posture at the telemetry boundary); `Effect::Emit { record }` +
log/metric helpers (Level lives in mwydyn-data now); the shared
BOUNDED RING (capacity knob, drop-oldest, per-kind drop ledger,
`drain_telemetry()` leads lossy drains with a `drop` record —
"never backpressures" is structural); ONE `exchange` RECORD PER
DELIVERY ATTEMPT (committed|busy|struck; ack = parent span id —
it already rode envelopes since round 26/29; ts_in/ts_out
wall-stamped, durations exist ONLY core-side; flag default on);
lifecycle + refusal records replace the stringly Vec<(Level,
String)>; THE INJECTION HANDLE (§3.4): `post` returns
`Accepted { seq }` + optional core-validated parent
(`JournalRead::seq_exists` default impl) — external id↔seq
mapping lives in the embedder's store, and request/response
across the external boundary closes causal loops (drain seq out,
inject parent in); UNGATED emission (floods = drop records,
never strikes; forged runtime kinds struck at Phase 2 = compose
parity with the barrier's `emit:<kind>` refusal). CROSS-CONTEXT
CORRELATION RESOLVED (owner, same round): foreign systems echo
ids by PAYLOAD convention (§9.5 precedent); mwydyn↔mwydyn = the
BRIDGE pattern (the ratified handles ARE the mechanism; edge
table with the bridge, join at the sink); envelope trace tokens
DORMANT until cross-process transport exists. telemetry-v0
corpus (19 cases) + generator + harness; OTel mapping = an
out-of-core layer, out of scope permanently.
**ROUND 43 COMPLETE** (work-notes/43): the JS/TS guest SDK —
engine (quickjs-ng v0.18.0 freestanding), wrapper+conversion+prelude
(three words with PROBE-TIME mwydyn_decl boot — the script's
registration IS the §9.2 declaration; Effect.log/metric from day
one), the `mwydyn-js` build tool (script-slot probe + data-segment
append), @mwydyn/guest types-only TS package, and the container
proof (echo + the §3.6 telemetry log in the drain). KNOWN ISSUE
RESOLVED (2026-11-11): the "opt-level crash" was LLVM LAWFULLY
zero-folding reads of the IMMUTABLE script-slot static (the build
tool's data-segment write is invisible to the optimizer) — static
mut + raw reads; pin removed; O3 green everywhere. THE LESSON:
anything a tool initializes post-link must be static mut.
**ROUND 45: THE PYTHON HOST EMBEDDING — THE INTEROP BENCH LIVES**
(work-notes/45): `mwydyn-py` (pyo3 0.23, host-side peer crate) —
the walking-test surface as a Python API (Context/spawn_python/
spawn_wasm/post→Accepted{seq}/run/drain_external/drain_telemetry/
snapshot_actor/dead_letters); Python actors are NATIVE-REALM
(stateless v0 default, unratified), effects RETURNED (Effect.post/
log/metric mirroring both SDKs), refusals raised as MwydynRefusal
BY NAME, a raised exception IS a panic IS a strike; Value⇄Python
with bignums riding Python ints. THE PROOF (owner's framing): one
python-driven context, a PYTHON actor + the round-43 JS echo guest
composing, ONE drain_telemetry showing logs from BOTH realms.
Lesson: effect addresses resolve AT THE SHIM (AmbientParts context),
never the builder. Tests: PYO3_PYTHON=C:/Python312/python.exe.
**ROUND 46: WASMTIME 47 + THE MWYDYN-JS LIBRARY SURFACE**
(work-notes/46): `mwydyn-wasm`/`mwydyn-js`/`mwydyn-guest-js` on
wasmtime 47.0.4 — pinned to match the FIRST EXTERNAL CONSUMER
(platformc's tree already carries 47 via vendored googlesql-rs; one
wasmtime, not two). wasmtime 47 left the std error hierarchy: take
the `anyhow` cargo feature (restores `?` into anyhow::Error), its
sealed `error::Context` for Result-context; everything else (epoch
interruption, barrier exchange, statefulness probe, ambient linker)
compiled untouched — the suite passed without modification, 46
suites green, clippy clean. `mwydyn-js` splits lib + thin CLI:
`compose(engine, script) -> Composed { wasm, slot }` (NUL is the
library's job), `probe_slot` public — embedders consume the library
(platformc's compose-js is the first). No design change: §9, the
barrier, and the SDK surfaces are byte-identical.
**ROUND 47: THE JS GUEST'S AMBIENT SURFACE — FOUND BY THE FIRST
CONSUMER** (work-notes/47): platformc's pass contract stages both
directions through blobs — exactly the surface round 43 never
exercised (the JS echo never called cx.blob*; the ambient suite
drives the RUST store guest) — and hit three wrapper defects on its
first blobPut: (1) the ambient trampoline freed its JSValueConst argv
(borrowed — quickjs frees it) = double free, the js_free_value_rt
crash; (2) JS_GetArrayBuffer's binding reshaped the real signature
(data pointer is the RETURN, size goes through psize) so the
typed-array branch of js_to_value NEVER ran — a Uint8Array fell to
plain-object conversion, indices as keys, refused BadKeyHygiene("0");
(3) the round-43 panic stash was unreadable through the container
(MWJS_PANIC_BUF exports as a CONST global — the Var-only collector
skips it) — GuestRt now captures it explicitly and the barrier's
process-trap panic appends "js panic stash: <message>", so every
future JS guest trap is debuggable from the host error alone. Plus
the branded-ref/byteLength property-value leaks. Regression pin:
js_guest_ambient_blob_round_trip (composed JS guest: put → get →
reply ref+bytes+len+caps; BlobRef round-trips as a real tag-1002);
build-guest-js.sh stages the uncomposed engine. Container suite green
incl. the pin. No design change — §9.4 was right in the spec and in
the Rust SDK.
**ROUND 48: THE GUEST ARTIFACT — THE MODULE/ACTOR SPLIT MADE PUBLIC**
(work-notes/48; first round from the platformc handoff,
docs/handoff-platformc-consumer.md, ADR-0019 debt 1): `GuestArtifact`
{engine, module, mode: PRIVATE} — COMPILE ONCE, PROBE ONCE, Clone at
Arc cost; `WasmActor::from_artifact(&artifact)` spawns
instantiation-only (lazy per round 37 — first exchange pays the
~120µs) for SERVE-MODE (fresh actor per request, the embedder's
determinism law) and SHARDING (N actors, one codebase — the real
mwydyn-side debt: construction was O(compile) per shard, 40ms Rust /
350ms quickjs). ARBITRATION (owner): a public GuestMode-TAKING
constructor is the round-41 override reborn (pub enum = forgeable —
carrying a probe result is indistinguishable from forging one, and
the probe it would save is 0–3ms of 350ms = noise); the artifact
CARRIES the probe's result in a private field — §9.2 amended: "the
artifact carries the probe, never the embedder", no public
constructor takes a mode anywhere. from_compiled/from_wat survive as
one-shot conveniences (artifact + from_artifact underneath);
probe_mode is a private free fn now. Pins (tests/artifact.rs): the
law (round-41 WAT table through from_module, clones carry the
declaration), the sharding shape (one compile, two counter shards,
independent counts), the cost boundary (20 spawns < ONE raw
Module::new; measured 170ns/spawn, pure assembly). LATENT DEFECT
FOUND+FIXED on the workspace run: the round-46 mwydyn-js lib pin was
BORN SKIPPED — `windows(5)` can never match the 6-byte literal
"mwydyn", so it read green only while the engine artifact was absent
and failed the moment round 47's script staged it; the pin now
asserts the FULL script lands VERBATIM (class named: a
skip-with-message pin's first full run after staging is its real
first run). 47 suites green, clippy clean workspace-wide.
**ROUND 49: THE JS SDK'S 1.0 SURFACE — PRELUDE CBOR CODEC + THE
THREE FORKS RATIFIED** (work-notes/49; spec §9.7 DECIDED, the
JS-SDK surface final as of this round): the prelude ships
`mwydyn.decode(bytes)`/`mwydyn.encode(value)` — ONE canon, both
realms, the second-realm decoder the corpora were written for.
DECODE NORMALIZES (owner fork, in session): liberal within the
restricted type set — non-shortest heads accepted, map keys sorted
on read (dups refused), f16/f32 widened to f64 — mirroring the Rust
decoder; refusals BY NAME (the corpus kebab codes ride .code).
ENCODE strict canonical. The JS value model mirrors the barrier
(number within ±2^53, BigInt beyond; Uint8Array; a decoded BlobRef
carries the SAME __mwRef brand — a decoded ref IS a host ref).
TWO EXPLICIT BOXES (JS-realm facts, not lossy surprises):
mwydyn.float(n) = floatness (bare integral numbers ARE ints —
3.0===3, the barrier's own rule), mwydyn.timestamp(µs) = the tag
(bare µs numbers lose it at the barrier, as always). Encode of a
branded ref = serializer-not-validator (round-12 law; validation is
the barrier's on crossing, decode's on reading). HARNESS
(mwydyn-wasm/tests/js_cbor.rs): the corpus driven through the REAL
COMPOSED ENGINE, one guest, every case a letter; uncrossable inputs
(bignums/timestamps/integral floats) ride an in-guest literal
table keyed by case name; 65/72 driven, 7 skips with reasons
(4 digest = host-side at put, JS SHA-256 out of codec scope; 3
enc-ref-reject = the VALIDATOR layer — JS validates at the barrier,
not codec-encode). THREE FORKS RATIFIED verbatim (§9.7): types-only
package, BYO-guard payload typing, number-when-safe. TWO LATENT
DEFECTS FOUND+FIXED in the round-43 conversion layer, both
luck-dependent, both broken by the engine rebuild's layout shift:
(1) the ref fields' property names were never NUL-terminated —
set_str passed raw &str as_ptr() to JS_SetPropertyStr and the read
side ditto; the names read into adjacent rodata ("digestmediaType-
bad-keycalled Result::unwrap()...") and write/read garbage AGREED
by sharing one rodata since round 43 — the prelude reads v.digest
by its CLEAN name and broke the luck; fix = c"..." literals both
sides (the class: host-side write/read FFI pairs validate each
other forever; only a SECOND consumer of the names catches it);
(2) i64::MIN crossed as a FLOAT — `i.abs() <= SAFE` overflows at
i64::MIN (wraps negative in release, "-2^63 ≤ 2^53" true) → the
float64 branch; fix = range compare. 48 suites green, clippy clean.
**Next:** round 50 = tasks meet the ambient surface
(`call_task_with(spec: Spawn, letter)`, zero-grant call_task kept
+ named, correlation mint already core-side, FailFast forced); then
platformc integration closes out (codec adoption deletes pass.js's
interim), mwydyn-stdlib (the merge actor), epoch/fuel budgets armed
per exchange.

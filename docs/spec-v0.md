# mwydyn — spec v0 (skeleton)

> Status: DRAFT. **DECIDED** = ratified in the 2026-11-08 charter
> round. **OPEN** = the discussion queue, one fork at a time.
> Provenance: `[A1:NN]` cites Athena1 `work-notes/NN` — recovered
> specifications, not recovered code.

## 0. Position [DECIDED]

- mwydyn is an **embeddable WASM actor runtime** — a single-node
  library first. Multi-node (the PaaS) is a later *distribution* of
  the same runtime, never a prerequisite of any consumer.
- Two actor kinds under ONE contract: **guest actors** (WASM
  components, sandboxed) and **native actors** (Rust, compiled in —
  the well-known services). Same contract, different trust domain.
- The runtime never depends on Athena. Athena2 embeds it; the PaaS
  distributes it.

## 1. Data model — messages are data [DECIDED]

- Encoding: Athena1's canonical CBOR profile [A1:04 §1], adopted as
  the starting profile — deterministic RFC 8949, always-float64 for
  non-integral numbers, no NaN/Inf/undefined, string-key-only maps
  sorted length-first-then-bytewise, no indefinite lengths, unknown
  tags rejected, depth/count/length limits, key hygiene. Extensions
  require a profile version bump (v0.1, round 12: tag 1002 — blob
  references).
- **Messages are data, not blob containers. Hard cap: 1 MiB per
  message.** Reality should sit far below; the cap exists to force the
  by-reference discipline.
- CBOR decode runs **host-side, at the WASM barrier** (after the
  inbox, before the guest; before the outbox) — guests never carry a
  codec. The validation boundary is the barrier.
- Routing and telemetry inspect **envelopes only**; payload decode
  happens at delivery.

## 2. Envelope [DECIDED in shape / OPEN in fields]

- Two disjoint field groups — producer-supplied (`type`, `payload`,
  `cause`, `correlation`, `authority`, …) and runtime-assigned
  (identity, position, timestamps, producer identity, checksum).
  **The runtime rejects any message arriving carrying runtime-assigned
  fields** — the byzantine posture [A1:04 §2].
- Ordering authority: per-actor inbox position, assigned at receive.
  No global order; cross-actor linkage via `cause` / `correlation`
  [A1:04 §§3–4].
- **[DECIDED, round 09]** the field set. *Producer-supplied* (from
  the guest `letter`): `kind` (layer-2 schema id), `payload`
  (canonical CBOR, ≤ 1 MiB), `cause?` (provenance ref),
  `correlation?` (exchange id — reply matching; dedup is
  defense-in-depth, the transaction is the safety mechanism),
  `authority` (row-cites). *Runtime-assigned* (rejected if present
  on arrival): `seq` — message identity = `(context, seq)`, the
  context journal's contiguous position (contiguous-or-refuse
  [A1:04 §8]); on log tier the inbox IS a cursor over the context
  journal; `from` (sender instance id — replies target it), `to`
  (target address), `commitTs` (tagged timestamp, audit only, never
  branching [A1:04 §5]), `producer` (sending actor's implementation
  digest — provenance for blue/green and A/B forensics), `checksum`
  (integrity over canonical CBOR). **No `ttl`** — staleness belongs
  to dead-letter policy and timer races. Additions are envelope
  version bumps. **No `reply-to` — owner's law:** an edge use-case
  that is easy to implement has no place as a first-order concept;
  reply routing is application-level (correlation matching), not
  envelope plumbing.

## 3. The actor contract [DECIDED shape / OPEN semantics]

- An actor has an **inbox**, an **outbox**, and a **telemetry
  outbox**. Telemetry is best-effort, lossy by design, never
  backpressures business messages, and lives **outside**
  snapshot/rollback semantics. The contract: §3.6.
- **Durability is runtime-owned and tiered**: inboxes and outboxes are
  always durable objects; the backing tier varies per actor (RAM for
  CLI/task actors → journaled disk for hosted). Same contract,
  different quality knob.
- **Task actors** are a first-class derived pattern: spawn-per-request,
  one message in, one reply out, RAM tier, supervised by the caller,
  die on reply. `invoke(actor, input) → output` is sugar over this —
  the function-shaped consumers never see mailboxes unless they want to.
### 3.1 Delivery semantics [DECIDED]

- **At-least-once delivery, dedup key = correlation id** [A1:04 §4].
  Non-idempotent actors use it to recognize redeliveries.
- **Log owns acks.** The delivery cursor is part of the durable inbox
  object — never separate bookkeeping — and **never carries a
  different durability than the mailbox itself**: a RAM inbox has a
  RAM cursor, a journaled inbox has a journaled cursor. "Processed" is
  always a log claim.
- Consequence: rewind re-delivers everything after the rewind point;
  correlation dedup resolves the replays. **Crash recovery and time
  travel are the same mechanism.**
- **Poison letters**: a delivery-attempt count rides the mailbox (same
  tier); advancing past a poison message is a *policy-authorized
  cursor advance*, journaled as a dead-letter fact — so
  dead-lettering rewinds consistently.
- Warning label (stated, not engineered away): rewinding re-executes
  side effects of non-idempotent messages already delivered.

### 3.2 Supervision & deployment [DECIDED in principle]

- **Supervision is policy, not mechanism.** The core provides
  lifecycle facts — spawned, delivered, `crashed{trap, message,
  cursor}`, terminal — and a policy attachment at spawn. Two named
  built-ins:
  - `fail-fast` — dev / live composition: a crash is terminal and
    surfaces loudly (immediate feedback for agent-originated actors);
  - `restart` — production: backoff, a restart-intensity window (max
    restarts in the window → escalate / dead-letter).
- Restart state follows the actor's state choice (§6): fresh
  re-instantiation | last snapshot | replay-from-log.
- Task actors map a crash to an error reply — the caller is the
  implicit fail-fast supervisor.
- **Single-actor, single-cursor invariant**: one actor processes one
  message at a time; scale-out is fan-out to more actors (task
  pattern, sharded workers), never parallel consumers on one inbox.
  This invariant is what makes snapshot/replay coherent. Stateless
  actors MAY offer parallel implementations — each with its own
  mailbox. **Dispatch queues are routing primitives, not consumers**:
  they distribute across bound backends, hold no mailbox and no
  cursor, carry no actor semantics (§7.4).
- **Warm pools & parking**: instance state is a cache; the
  mailbox+cursor are the truth (the A1 substrate shape [A1:21 §5]).
  Parking = the suspendable contract at rest; pooling = an instance
  cache bound to a name. The PaaS composes them; the CLI never pays.
- **No mandatory supervision tree [landed round 05]**: escalation
  routes to the owner handle; trees are user-buildable (a native
  actor that supervises children).
- **Binding indirection [landed → §3.4]**: swap or split the
  binding, never the actor.
- Residue: bounded mailboxes & backpressure — under discussion
  (round 05 proposal, reply pending).

### 3.3 Capabilities — grant rows, minted cites [DECIDED base, round 03]

- Rights are **grant rows** `{subject, verb, object}`, checked per-use
  at every boundary — message dispatch, host-function call — against
  the live grant set [A1 CTR-3 lesson: never infer authority from
  anything but the live set]. Envelopes present the `authority` refs
  they act under [A1:04 §2]. Refs are **row-cites, never bearer
  instruments**: rows are subject-bound, so a ref copied through a
  blob, log, or message is inert to anyone but its subject —
  capability pointers are *addresses of rights*, not rights, and
  moving them around is harmless by construction.
- Verbs are **namespaced by service**: `post` / `subscribe` (the
  message layer — the layer-2 hook), ambient service verbs
  (`blob.read`, `blob.write`, …), control-plane verbs (`spawn`,
  `supervise`, `bind`, `park`).
- **Minting is attenuation-only, holder-only, and journaled**: a mint
  cites its source row, names the grantee, and is verified by the
  service's **attenuation algebra** — per-service pure functions
  (`narrow(row, request) → row' | refuse`, `subsumes(a, b)`) defining
  when a minted row is strictly narrower. No mint may widen; the
  runtime verifies, never trusts. The algebra is where attenuation
  complexity is tamed: it is *local* to each service's object grammar
  and **conformance-vector tested** like everything else.
- **Revocation cascades via lineage**: minted rows cite their parent;
  revoking a row kills its descendants.
- Boot: root rows come from the spawn manifest (bundle-advertised
  caps + consent ceremonies for widening — the A1 posture, recovered
  [A1:21 §1]).
- Tier rule: grant-set mutations are log events for log-tier actors;
  RAM-tier actors hold in-memory grant sets seeded at spawn.
- **The default-deny boundary [DECIDED, round 04]**: the
  **trust-context hybrid** — type-level authority *inside* a trust
  context, explicit target rows *across* contexts. Owner's law,
  verbatim: **"intra-context transparency by default, cross-context
  ceremony."** Cross-context minting is the ceremony that makes PaaS
  service-to-service wiring deliberate.
- Named residual risk: the confused deputy (an actor tricked into
  exercising its own rows on another's behalf) is inherent to held
  authority; mitigation is attribution — every use names the row and
  the envelope's authority refs — not possession-based redesign.

### 3.4 Addressing & routing [DECIDED, rounds 04–05]

- **Names** `(context, name)` — readable, unique within a trust
  context; what actors post to. Not security objects (§3.3).
- **Instance ids** — opaque, runtime-minted, **journaled at
  creation** (replay-stable [A1:04 §3]); what lifecycle facts,
  supervision, and logs cite.
- **Bindings** — name → {implementation digest, config}. Swap =
  blue/green; split = A/B; a **dispatch queue** = a binding whose
  backend is a set + policy. Control-plane ops (`bind`, swap, split)
  are capability-gated.
- **Naming is opt-in [DECIDED, round 05]**: named actors are
  addressable within their context, refuse-on-collision; unnamed
  actors hold only the spawner's handle — the task actor. Escalation
  routes to the owner handle; supervision trees are user-buildable
  (the §3.2 proposal, ratified by consequence). A "service" = a
  named, long-lived actor.
- Location transparency: addresses are data; resolution is a runtime
  concern; in-proc vs network never changes message bytes.
- **The external boundary is a handle, and it closes causal loops
  [round 44]**: `drain_external()` yields stamped envelopes (seq in
  hand); injection (`Context::post`) RETURNS the seq and accepts an
  optional parent, stamped into the envelope's `ack` under the
  round-26/29 law (host-decided causality; core-validated against
  known seqs or refused `invalid`). External id ↔ seq mapping lives
  in the embedder's store — the seq return is the durable channel;
  telemetry never carries it (§3.6). Request/response across the
  boundary thus traces end to end: drain (seq out) → external work
  → inject (parent in) — no protocol the external system must speak.
  The same two handles ARE the cross-context bridge pattern (§3.6):
  drain seq out, injection seq in, edge table with the bridge, join
  at the sink.

### 3.5 Effects and transactions [DECIDED, rounds 07+16]

`receive` is a **pure transition**: `(instance memory, letter,
ambient reads) → effect list`. The runtime validates, then applies
effects in order. Effects are **post, mint, emit** — the *observable*
things. **[amended, round 16]** `blob.put` is NOT an effect: it is an
eager call on the ambient face (§4), returning the ref in hand. The
owner's law: **allocation is not an effect** — a put creates
content-addressed bytes and nothing else; if no committed reference
ever forms, it is unobservable garbage that reference-aware GC
collects (M3), so a rolled-back list and a never-executed one are
indistinguishable in observable state. (This amendment also resolved
a real spec contradiction: §4 had `put` as an ambient WIT function
since round 02; round 07's effect list contradicted that — the
owner caught it by asking how any actor ever gets a ref to post.)

- **The transaction boundary [DECIDED, round 08]**: within the
  receiving context, state transition + all effects commit as **one
  atomic unit** (one journal transaction on log tier, one in-proc
  commit on RAM tier). **Cross-context posts are NOT transactional**
  — at-least-once eventual; multi-context consistency is saga
  territory. Owner's law: **atomicity is only valid inside a clock
  domain — and a trust context IS a clock domain.** Contexts break
  clock domains deliberately: no shared clock, no single lock in the
  sky, nothing to fall over when scale hits. The wall is one wall
  with three names: trust boundary, clock domain, transaction
  boundary.
- **Puts are eager and non-observable** (round 16): `host.blob_put`
  returns the ref *during* receive — the actor embeds it in posts
  freely, saves it in state, or drops it (garbage). Replay re-puts
  identically (pure in `(bytes, type)`); GC collects unreferenced
  garbage above the live watermark (M3).
- **Validation happens before any application** (fail-before-compute):
  structural envelope laws, sizes under the cap, authority rows
  present, capacity/caps admissible. **The core is schema-blind**
  (§7.9, round 10): `kind` is an opaque string, payloads are opaque
  bytes — schema validity is an edge, embedding, or actor concern,
  never admission.
- **Two refusal classes at admission**: `busy` = temporary — the
  *letter* re-queues at its inbox tail, no delivery attempt burned
  (overload becomes self-healing: the actor re-receives and re-declares);
  `invalid`/`unauthorized` = permanent — the attempt burns, N strikes
  → the dead-letter path (§3.1).
- **Telemetry is outside the transaction**: `emit` effects apply
  best-effort regardless of commit or rollback — lossy by design,
  and structurally lossy at that (§3.6).
- **Reads are repeatable by contract**: imports are limited to
  content-addressed blob reads and `caps.list` — the answer is a
  function of the ref; a swept ref is a loud error, never changed
  data (§4 GC is reference-aware above the live watermark).
- Ripples: the interruptability rules (§6) are satisfied trivially —
  imports are pure reads; checkpoints (§7.6) become trivially atomic;
  replay-testing is vector-shaped: `(letter, seeded memory, blob
  fixtures) → effects`.

### 3.6 The telemetry outbox [DECIDED, round 44]

- **Three producers, one outbox.** Actors declare `log` and `metric`
  records (emit effects); the runtime declares `lifecycle` (§3.2
  facts), `refusal` (admission + ambient refusals), `drop` (loss
  accounting), and `exchange` (the span skeleton, below). **Spans
  are nobody's declaration** — derived at the sink from
  core-attached lineage. Actor spans are unimplementable by law,
  not merely undesired: clock obliviousness (§6) forbids measuring
  durations, and atomic transitions give mid-flight markers
  identical stamps (round 44, owner-ratified).
- **The record is canonical CBOR with CLOSED kinds** (`log | metric |
  exchange | lifecycle | refusal | drop`); unknown kinds refused by
  name — barrier-verb discipline, because telemetry is
  infrastructure data (contrast round 10's opaque letter `kind`,
  which is application data). Guests supply kind + payload ONLY;
  `ts`, `actor`, `seq` are RUNTIME-MINTED on apply — no forged
  attribution or timestamps, even accidentally. `Level` stays
  5-valued; metrics are points-not-aggregates, temporality always
  delta.
- **Semantics**: outside the transaction, literally — a failed
  journal commit does not retract emits; no double-counting by
  construction (hydration discards effects; crashes eat theirs
  pre-apply — one real exchange emits at most once). Emission is
  UNGATED: observation is not an effect on the world (posts are
  ungated today; the default-deny fork may revisit WITH routing,
  never telemetry-first), and floods become drop records — never
  strikes.
- **The outbox is one bounded ring per context** (drop-oldest,
  capacity knob): "never backpressures" is a structural guarantee,
  not a convention. `drain_telemetry()` is the embedder's handle
  (the `drain_external()` pattern); a drain that lost records leads
  with a synthetic `drop {dropped, by_kind}` record — the pipeline
  carries its own loss ledger. No push face: apply stays
  allocation-only.
- **Lineage attachment (spans)**: one `exchange` record per delivery
  attempt, always emitted (busy included — late-because-busy is
  trace-relevant), default-on behind a builder flag: `{seq, ack?,
  from, actor, ts_in, ts_out, outcome: committed | busy | struck}`.
  `seq` = span id; `ack` = parent span id (already the
  checksum-bound envelope field, round 26/29 — the mailbox hands it
  to the pump; the core adds a field read, not a law); `ack: None` =
  root (embedding/pump and timer posts; trace id ≡ root seq of the
  ack chain). Actor records join by `seq` → span events; orphan
  records (their exchange record dropped) are legal lossy behavior.
  Sink-side derivation is a pure function of one drain stream; an
  unresolvable `ack` (a foreign context's seq) links-or-roots.
  **Actors never see lineage; sinks never touch the journal; the
  core attaches both halves.** The journal remains the durable
  record of causality; telemetry is the lossy real-time projection
  WITH timing — dispatch wall time exists nowhere else.
- **Cross-context correlation [round 44, owner-ratified]**: the
  general case is unsolvable by the runtime — carriers must
  cooperate (OTel's own position). Foreign systems correlate by
  PAYLOAD convention, echoing our outbound `seq` (§9.5 precedent).
  mwydyn↔mwydyn closes at the bridge: the bridging embedding is the
  only entity holding both namespaces, and it holds them by these
  very handles (drained seq out, injected seq back); the edge table
  lives with the bridge, the join at the sink — zero core surface.
  Envelope trace tokens stay DORMANT until cross-process mwydyn
  transport exists.
- **The barrier carries `emit`** (round 44): additive response verb,
  NO ABI bump (an older host refuses it by name through the
  documented `UnsupportedVerb` slot); the record shape lives in
  mwydyn-data beside the barrier contract; conformance vectors
  `telemetry-v0`.
- **The OTel mapping is a LAYER, out of core** (tight core, round
  18): the types merely project cleanly — severity numbers, delta
  temporality, seq/ack span identity, context name → resource.

## 4. Blobs — an ambient host service [DECIDED, round 02]

Blobs do NOT go through the actor model. The actor model contributes
mutation management and ordering; blobs have neither — immutable,
content-addressed, idempotent. Mailbox round-trips on them are
ceremony without payoff.

- The store is an **ambient host service**: `blob.put(bytes,
  mediaType, ttl?) → ref`, `blob.get(ref) → bytes | loud-missing`,
  `blob.len(ref)`, `blob.get_range(ref, offset, len)` — synchronous
  host functions, hardcoded Rust, collocated in every runtime.
  Native actors hold the face directly; WASM guests reach the same
  verbs, same cap checks, through the ambient import (§9.4).
  (Range/len exist so consumers stream without materializing —
  docgen's PDFs.) **Round 16: put is eager-during-receive, not an
  effect (§3.5) — allocation is not an effect; the ref is in the
  actor's hand the moment the call returns.**
- **[amended round 28] Verification is PROVIDER POLICY.** The
  contracted invariant is content identity: reads return the bytes
  that hash to the digest. How the provider upholds that — eager
  per-read checks, a background scrub, or CAS-by-construction — is
  its own choice; corruption is reported loudly whenever detected.
  Rationale: per-read verification forces full fetch + re-hash
  before serving a RANGE of a large blob (§4's whole reason for
  range reads); range reads must SEEK, not materialize. The ref's
  `mediaType` is a HINT (a relabeled ref still returns true bytes);
  dispatchers that care consult provider-truth type or sniff bytes.
  Big blobs: atomic writes stay (the hash needs all bytes); slice-
  per-slice assembly is done by MERGE ACTORS — platform-specific
  actors that concat part-refs into a new ref, inheriting
  capabilities, GC reachability, and the executor; tree-of-merges =
  Merkle-shaped assembly without changing identity.
- The **blob reference is a first-class CBOR type: tag 1002**
  [DECIDED, round 12; profile v0 → v0.1] wrapping
  `{ digest, mediaType, size }` — digest = 64-char lowercase hex
  SHA-256, size = u64. Ref-ness is a data-model fact enforced at the
  barrier decode, not a convention. Routing and telemetry never see
  blob bytes, trivially.
- **The mediaType is hash-bound, not decorative [DECIDED, round 12]**.
  Owner's law: **content identity includes type** — same body under
  another type is *not* the same content, and cross-type coexistence
  would be a collision, not a desired state. `put(bytes, mediaType)`
  is a pure function of **(bytes, mediaType)** — digest = SHA-256
  over the canonical CBOR encoding of `{ bytes, type }`. The declared
  type is an *input*: no magic-byte detection inside `put`; purity
  and replay-safety unchanged (a deterministic replay re-puts to the
  identical digest). Consequences, ratified: no cross-type dedup; the
  type is frozen at put; **a ref re-typed in transit is detectable**
  at any verification point (the store on `get`; consumers before
  dispatch), because (type, bytes) no longer hashes to the digest.
  Sniff-validation of declared-vs-actual is an optional embedding
  face, never a purity dependency. The underlying observation is a
  law of its own: **anything claimed outside the hash is
  mutable-by-transit.**
- **Blob bytes never transit messages.** The 1 MiB cap (§1) and the
  by-reference layering law [A1:51] stay; writes happen before any
  reference can exist.
- **Replay-safe without journaling puts**: content addressing makes
  `put` a pure function of its inputs — bytes and declared media
  type (§4, round 12) — so a deterministic replay re-puts to the
  identical digest. The rules that buy this: writes are
  idempotent-by-content; **GC is reference-aware** (a digest cited by
  any reachable log position, actor state, or snapshot is collectable
  only above the live watermark); a missing blob is a loud integrity
  failure, never silent divergence [A1:04 §10].
- TTL is a `put` parameter, never an ambient default; unreferenced
  bytes sweep by runtime GC configuration [proposed default].
- The principle this draws: **a host service may bypass the message
  layer only if it is deterministic** (replayable without journaling).
  Blob is ambient; clock is not — `clock.now()` under replay diverges,
  so clock reads must be journaled (§7.7). Randomness: mediated.
  Timers: message-shaped (a timer event IS a message). Telemetry:
  lossy by design, exempt from replay semantics. Query-shaped blob
  surface (search, listing), if ever needed, is a native actor —
  never ambient-surface growth.

## 5. Type layers [DECIDED]

- **Layer 1 — the frozen guest ABI (§9)**: the barrier's effect
  verbs (post + emit [round 44]; mint when its effect lands) plus
  the ambient
  import's read verbs (§9.4). Frozen by rule: the first host-need
  evolution that adds an import or an ABI-level function breaks the
  freeze. Surface growth happens as DATA — ambient verbs (round 40)
  and barrier effect verbs evolve without touching the ABI.
- **Layer 2 — CBOR message schemas**: application contracts,
  versioned, registry-disciplined. **New capability = new message
  type + capability check, never an ABI change.** A1's evolution law
  carries over [A1:04 §7]: within a schema version, new fields are
  optional and consumers are tolerant readers; breaking changes bump
  the schema version.
- OPEN: registry mechanics beyond round 10 (§7.9).

## 6. Execution [DECIDED choices / OPEN mechanisms]

- **The execution model [DECIDED, round 21]**: CSP-shaped —
  **transitions run in parallel** (different actors share nothing;
  single-cursor means an actor never overlaps itself), **commits
  serialize per context** (the journal's total order, §8).
  Determinism is defined **relative to the journal**: races are
  real-time only, the recording is law — *"non-deterministic but
  recorded; the journal is law; whatever happens that creates the
  stream of events that are journaled is what it is"* (owner).
  Replay follows the journal and is deterministic given it. The
  mechanism (LANDED, round 34/35): `ctx.workers(n)` — wave dispatch,
  persistent workers run phase-1 transitions (forks included)
  concurrently; the pump thread commits in DISPATCH order —
  parallelism with deterministic commit order. **THE FACE IS
  CQRS [owner-ratified, round 35]**: `JournalRead` (parallel
  readers — hosts, workers) / `JournalCommit` (single dedicated
  writer — the committer) / `JournalFace` = both. It maps directly
  onto real journal implementations (read view vs write publisher);
  in-core, an `Arc<RwLock<...>>` enforces the law mechanically —
  no unsafe. **Wave pipelining [round 36]**: the next wave dispatches
  before the current applies — collect AND apply overlap with worker
  execution; the committer's per-exchange serial cost is the measured
  Amdahl bound (`PumpStats.profile`), and throughput work follows it:
  cheap-transition workloads are committer-bound (cheaper apply/
  compose is the lever), expensive-transition workloads are
  worker-bound (the fork cost is the lever — §7.6 snapshots).
  **Depth-N [round 38]**: `PIPELINE_DEPTH` waves in flight — apply
  AND collect overlap across the pipeline; echo-class workloads
  saturate the committer (wall = committer bound, measured). The
  overlay's recording layer is a WRAPPER (`RecordingJournal`) — the
  hot journal never records for tiers without a durable face, and
  `Commit.appended` carries external-target envelopes only.
- **The WASM guest lifecycle [DECIDED, round 37]**: instances are
  PERSISTENT — created lazily on first use, reused across letters;
  per-letter instantiation is gone. The transactional guarantee
  follows the guest's DECLARED statefulness: **Stateful** guests get
  pre-receive LINEAR-MEMORY IMAGE rollback (fork captures memory;
  refusal applies it to a fresh instance — exactly-once internal
  state, doubled footprint while the image lives); **Stateless**
  guests are just reused — the mutated instance stays on refusal,
  no once-and-only-once internal-state guarantee (the author's
  declaration is the contract; it rides as
  `Actor::fork_carries_state`). Stateful guests keep state in
  linear memory (restored byte-exact); mutable globals revert to
  initial values on a restore (documented). **The §7.6 snapshot ABI
  [DECIDED, round 39]**: guests MAY export `restore(ptr, len)`
  (boot snapshot → blob_get host-side, blob.read cap-gated, loud
  refusal to drop durable state silently) and `snapshot()`
  (`Context::snapshot_actor`: runtime blob_put + covers-stamped
  snapshot letter — covers = the actor's cursor; ref rides a DIRECT
  BlobRef, GC-visible). Snapshot refs in payload letters ride
  DIRECT — the Bytes-wrapped spelling never matched the scan (found
  round 39: the native restore path ran vacuously) and is
  GC-invisible.
- **The ambient host-face import surface [DECIDED, round 40]**: ONE
  import — `mwydyn.ambient(in, in_len, out, out_cap) → written` —
  canonical CBOR envelope in, `{"ok"} | {"err": code}` out (oversize
  → `-1`, the SDK retries). Verbs mirror the native Host face with
  the same cap checks: blob.get/put/len (granted), caps (free) — no
  clock verb (§6 stands: the ticker is the only wall-clock reader).
  The pumps hand each actor its `AmbientParts` (journal read side,
  blob face, context name, the actor's own rows) before receive —
  ambient verbs are GRANTED, never ambient; refusals are observed
  by the guest by name. The envelope shapes live in
  `mwydyn-data/src/ambient.rs`; the guest-side glue is the
  `mwydyn-guest` SDK crate. Guests WITHOUT the import stay
  zero-import.
- **Conformance vectors [round 39]**: overlay-v0 — the three
  overlay roles driven through the FACE (recording skin records;
  durability face silent; plain hot tier silent): the silent-no-op
  regression gate. barrier-v0 — canonical request/response forms
  (machine-generated hex) + the refusal table, consumable by any
  second-realm decoder. The serial `run_until_idle` pump stays
  forever as the deterministic mode (tests, replay verification).
  Readiness bounds bought now: `JournalFace: Send+Sync` (external
  providers are threaded — one that cannot cross threads is not a
  real provider), `Actor: Send` (no `Rc` in actor state, by
  contract), `ClockFace: Send+Sync`, blob store `Mutex`.
- **Durability IO pipelining [DECIDED + VERIFIED, round 34]**: the
  hot journal applies every per-receive transaction INSTANTLY (the
  mailbox is a cursor over the journal — buffering acks inside the
  hot face re-delivers letters, proven the hard way); the overlay
  drains the op log to the durable face as ONE transaction on
  POLICY (`max_ops_per_flush`, default 256; quiescence respects the
  policy; the embedding's `flush_durability()` is THE barrier).
  Durability IO is a function of the knob, never the exchange count
  — verified at the SQLite boundary with a counting face: 2,000
  exchanges → one transaction; the undrained tail is the
  at-least-once redo window; letters posted but undrained are lost
  on death (post-before-barrier). `hydrate()` reads delivered
  history from the durable face when the overlay is present.
- **The WASM barrier [DECIDED, round 33]**: canonical CBOR bytes
  are the WASM ABI — `Value`-in-memory never crosses a boundary,
  the same law the journal runs (journal ⇄ wire ⇄ barrier: one
  canon). No WIT, no component model: "enforced external CBOR
  envelope types" replace the interface layer — request
  `{"from", "letter", "v"}`, response `[{"post": {"to", "letter"}}]`,
  decoded through the canonical profile at the wall both ways
  (`mwydyn-data/src/barrier.rs` is the contract; conformance
  vectors apply at the barrier too). The guest-side codec IS
  mwydyn-data (one definition, both realms). ABI mechanics:
  `alloc`/`dealloc`/`process(ptr,len)→[u32 len][bytes]` over plain
  `wasm32-unknown-unknown` — guests have ZERO imports (no WASI,
  nothing to fake). Every violation is a guest CRASH (trap, garbage
  bytes, bad shape), classified by supervision — never a silent
  swallow. **THE PAYLOAD WALL**: non-canonical payload bytes are
  refused at admission (`post`) and in the effects phase — native
  and WASM identically; the journal never stores undecodable
  payloads. Payloads are canonical CBOR *always*; a letter carrying
  nothing uses `Null` (0xF6), not zero bytes. WHY (owner-ratified,
  round 33 arbitration): the runtime itself is a payload consumer —
  GC's tag-1002 reachability sweep is deep pointer inspection, and
  an undecodable payload's refs are invisible, so a referenced blob
  could be collected. The boundary line: **VALIDITY** (canonical
  CBOR or not) is the data model's, host-enforced; **SHAPE** (what
  fields/types the payload carries) is the kind's contract,
  registry/consumer-side — never the host's. A letter of any kind
  with payload `Null` passes the wall.
- Runtime: **wasmtime**. Budget knobs proven in A1's spike [A1:40]:
  epoch-interrupt wall clock, memory limiter, optional fuel.
- Stateful actors choose:
  - **(a) own state via the blob service** — load/save by digest;
    space/time-efficient, actor-complex;
  - **(b) the suspendable contract** — the restore unit is
    *instance state + inbox position*, under interruptability rules
    (no threads; no reentrant host calls mid-message — inbox/outbox
    IO is the IO).
- Mechanism note (verify): wasmtime *live* snapshots (memory + stack +
  PC) are not, to our knowledge, a shipped public API. (b) ships
  **replay-backed** — persist the inbox position, deterministic replay
  from a cold instance, hybrid checkpoint+replay later. The
  interruptability rules are exactly replay's determinism rules, so
  the contract is mechanism-independent. Watermark stamps at
  consistent points [A1:21 §5] are the recovery vocabulary.
- OPEN: checkpoint format (§7.6); the time-travel layer's API (§7.7).

## 7. OPEN forks — the queue

1. ~~Delivery semantics~~ — RESOLVED → §3.1 (log owns acks; cursor
   shares the mailbox's durability tier; poison letters as journaled
   cursor advances).
2. ~~Supervision & restart~~ — RESOLVED in principle → §3.2
   (supervision is policy; two built-ins; deployment rides the
   binding indirection). Residue: bounded mailboxes & backpressure —
   its own fork.
3. ~~Capability model~~ — RESOLVED in principle → §3.3 (grant rows +
   minted cites; per-service attenuation algebras; subject-bound
   refs). Residue: the **default-deny boundary** (per-target rows vs
   the proposed trust-context hybrid) — its own fork.
4. ~~Addressing & routing~~ — RESOLVED → §3.4 (names + ids +
   bindings; naming opt-in; dispatch queues as bindings). The
   no-well-known-registry note stands: ambient primitives live in
   the guest ABI (§9), mediated services ride capabilities + log.
5. ~~Telemetry contract~~ — RESOLVED → §3.6 (round 44: closed-kind
   records, the shared ring + drop ledger, core-attached lineage
   with the `exchange` record, the injection handle in §3.4).
   Residue: **cross-context correlation** — resolved in the same
   round (work-note 44 §6): foreign systems correlate by PAYLOAD
   convention (echoing our outbound seq — §9.5 precedent; the
   runtime cannot know edges only the carrier knows); mwydyn↔mwydyn
   traces close at the BRIDGE — the bridging embedding holds both
   namespaces by the ratified handles (drained seq out, injected
   seq in), keeps the edge table, the sink joins — zero core
   surface; envelope trace tokens stay DORMANT, reopening only when
   cross-process mwydyn transport exists (then: a version bump).
6. **Snapshot/replay contract** — checkpoint format, watermark
   stamps, GC interplay with time travel. (Guest boot-snapshot ABI
   landed round 39 → §9.3; the checkpoint-format fork remains.)
7. **Time-travel layer** — the optional clock/log spine for actors
   that opt in (snapshot-enabled ⇔ spine-riding).
8. ~~WIT verb set~~ — DRAFTED rounds 06–07 as `docs/world-v0.wit`
   (file retired round 41; git history keeps it), **SUPERSEDED
   round 33 → §9** (WIT is dead: canonical CBOR is the WASM ABI,
   zero-import guests). The ratified SHAPE survives the interface
   layer's death: effects returned never called, imports
   read-only-by-grant, one well-known name (ticker). Rejected from
   v0, with reasons in work-note 06: `yield`, `subscribe`, a
   snapshot verb, a timer verb — and, round 07, all imperative
   write imports. Residues still live: envelope field set
   convergence (§2); `field-value` variant contents.
9. **Schema registry** — [DECIDED, round 10]: ids
   `<namespace>.<name>@<major>`; the envelope `kind` carries it;
   namespaces claimed per context at composition, conflicting
   definitions for one id refuse. **Majors immutable once ratified**
   (a bug becomes a new major, never an edit); within a major,
   revisions additive-only, tolerant readers mandatory — non-breaking
   evolution for sources and sinks composed from **distinct delivery
   lifetimes** (owner). **Two representations: CDDL authoritative**
   (authoring), **canonical CBOR projection** (storage +
   orthogonality); deterministic mapping, roundtrip-vector-pinned —
   the registry practices the data model's own projection law
   [A1:04 §1]. **The core is schema-blind — "CBOR is just CBOR"
   (owner's law):** `kind` is an opaque string to the core; payloads
   are opaque bytes under structural envelope laws only; no admission
   schema checks. The registry is **queryable metadata** — a library
   API and external-tooling surface (compatibility checks, typed
   views) — never admission-enforced, never guest-visible in v0 (the
   frozen world is untouched). Consumed-kind declarations: optional
   metadata in spawn manifests, queryable, unenforced by the core.
   v0 = static per-context registry; dynamic install is PaaS-era.
   Vector format: JSON + CBOR hex/diag (validity / roundtrip /
   message-verdict); A1's `cbor-v1.json` ports as the first corpus.
10. **Multi-node boundary** — what single-node APIs must not preclude
    (placement, parking, cross-runtime dedup).

## 8. The journal contract [DECIDED, round 19]

The journal face is defined by **invariants + a provider admission
list**, not by function signatures. A provider is a thing satisfying
the invariants; everything else is not a provider — by name. (The
owner's law for this domain: a strict model beats a permissive API
with badly defined semantics.)

### 8.1 Identity and order

- Journal identity: `(context, lineage, seq)`. Lineages start at the
  context root; **`fork(r)` creates lineage L′ with parent (L, r)**
  — rewind is a journaled FACT, never a truncation [DECIDED, round
  19; owner: "we don't erase the future from the restored point"].
- `seq` contiguous per lineage, from 1, gap-free —
  contiguous-or-refuse.
- No global order across contexts.
- `fork` joins `spawn / supervise / bind / park` as a control-plane
  verb (§3.3) — capability-gated like the rest.

### 8.2 The atomic unit

- The journal's write unit is a **transaction**:
  `{appends (letters), acks (cursor advances), facts (dead-letter,
  mint, fork)}` — all-or-nothing at the provider's durability point.
  Commit returns only after durable; nothing is deliverable before
  its append is durable (write-before-delivery [A1:04 Q10]).
- External log providers map the transaction to ONE atomic record:
  JetStream and Kafka have no multi-message atomic publish, so the
  batch IS the record — which is our own commit granularity anyway
  (a receive's effects are a batch, §3.5).

### 8.3 Physical, logical, ghost

- Physically append-only; logically shaped by facts. No in-place
  mutation, ever.
- **Ghost branches** [owner's term, adopted as vocabulary]: an
  abandoned lineage tail is READ-ONLY history — *ghosts of an
  alternate reality*. Ghosts never deliver and never create effects;
  they are readable. Delivery follows the ACTIVE lineage only.
- `lineages(ctx)` lists lineages (id, parent, fork point, active);
  `read(lineage, range)` works on ANY lineage, live or ghost.
- Ticker coherence (no new mechanism needed): arms, polls, and ticks
  are all letters, so the ticker's schedule replays deterministically
  from the journal — after a rewind, the schedule rewinds itself.
- Ghost-read authorization: **[OPEN — proposed]** a capability
  (`ghost.read`), default-off, granted like any other row; A1
  precedent — the model had the tool to read its previous future.

### 8.4 Provider admission list

- Providers MUST provide: atomic batch commit at a durable point;
  contiguous per-lineage sequence; physically append-only with NO
  retention loss; ordered, byte-identical replay from any position;
  serialized writes per context (v0: single-writer — a distributed
  face brings its own serialization, outside this contract).
- **Admitted**: DBMS/SQLite (native fit); NATS JetStream (batch-as-
  record; consumer acks ARE cursors); Kafka (transactional producer
  + infinite retention + partition-per-context).
- **Not providers, by name**: at-most-once queues; object storage
  without atomic CAS; anything with retention-based loss.

### 8.5 First implementations

- `InMemoryJournal` — the RAM tier and the tests, proving the face
  is not SQLite-shaped.
- `SQLiteJournal` — the durable impl, behind the `sqlite` cargo
  feature (default ON): transactions give the atomic batch
  natively; envelopes stored as canonical CBOR, verified on read.
- **Placement law [round 25]**: distributed providers (NATS,
  Kafka, consensus-backed) are PLUGGABLE CRATES OUTSIDE the core —
  the face's `Send + Sync` bound and the §8.4 admission list are
  the entire integration surface; their dependency waves (tokio,
  TLS, client SDKs) never touch the core's graph.

## 9. The WASM guest contract [DECIDED, rounds 33+37+39+40+41]

Consolidated here from the rounds that built it; the barrier speaks
the journal's vocabulary (one canon, both realms) and adds nothing
the data model does not already define.

### 9.1 The barrier

Guests compile to plain `wasm32-unknown-unknown`. Exports:
`alloc(len)→ptr`, `dealloc(ptr,len)`, `process(req_ptr,req_len)→
resp_ptr`. The host writes the request at guest-provided scratch;
the response block is `[u32 le len][canonical CBOR bytes]`.

- Request: `{"from": <address>, "letter": <letter>, "v": 1}` —
  canonical CBOR, version-gated before interpretation.
- Response: an array of single-entry effect maps — `post` and
  `emit` (round 44): `{"post": {"to", "letter"}}` and
  `{"emit": {telemetry record}}` (kind + payload only; `ts`/
  `actor`/`seq` are runtime-minted, never guest-supplied — §3.6).
  Unknown verbs refused by name.
- Every failure mode — trap, garbage bytes, bad shape — is a crash,
  and crashes are supervision's problem (panic = strike).
- The payload wall applies at the barrier identically to native
  admission (compose-parity at the gate).

### 9.2 Declared statefulness — the guest declares [round 37, amended round 41]

The statefulness declaration belongs to the GUEST, not the
embedder, mirroring native `fork_carries_state` (compose-parity of
the declaration itself):

- A guest MAY export `mwydyn_decl() -> u32` — bit flags; bit 0 =
  STATEFUL. ABSENCE = Stateless by omission (test fixtures,
  hand-rolled guests).
- The runtime reads it as the lifecycle's source of truth:
  Stateful = pre-receive linear-memory image rollback (exactly-once
  internal state; mutable globals are NOT covered — stateful state
  lives in linear memory); Stateless = "just reused", no guarantee.
- **No embedder override exists.** Composing a stateful guest as
  stateless would silently break the author's exactly-once
  expectation; the mismatch is refused by removing the override
  path entirely (round 41).

### 9.3 Durable snapshots (§7.6 boot ABI) [round 39]

A guest MAY export `restore(ptr,len)` (apply a boot snapshot;
loud refusal if the boot carries state and the export is absent)
and `snapshot()→[u32 len][state bytes]` (the runtime stores it via
the blob face and lands a covers-stamped snapshot letter; the
state format is the guest's own). Export presence is the capability
probe. Snapshot semantics require statefulness — snapshotting a
torn state is a category error; SDK-level entry surfaces enforce
the implication (§9.6).

### 9.4 The ambient import [round 40]

ONE optional import, `mwydyn.ambient(in_ptr, in_len, out_ptr,
out_cap) -> written`: canonical CBOR `{"verb": ..., <fields>}` in,
`{"ok"} | {"err": "<code>"}` out, `-1` = response did not fit.
Verbs mirror the native host face with the same cap checks:
`blob.get`/`blob.len` (blob.read), `blob.put` (blob.write), `caps`
(free). No clock verb — the ticker is the only wall-clock reader
(§6). Powers are granted per-actor (capability rows), never
ambient; refusals are observed by name. **Conformance vectors:
`ambient-v0`** (round 42) — the four canonical request forms, the
refusal table with the offending verb named, and the response
envelope forms with every dispatch refusal's exact spelling.

### 9.5 Reply routing [DECIDED, round 41]

The envelope's `from` IS the return address (no reply-to field —
owner law). Payload-level routing to a THIRD place (e.g. the
embedding's external outbox) is a convention, not a model field.
RATIFIED: the SDK blesses `replyTo` — an Address-typed payload map
field — with helpers reading it by convention
(`Effect::reply_via`). Neutrality (every embedding its own
convention) and configurable field names are the rejected
alternatives on record.

### 9.6 Guest SDKs

SDKs wrap §9.1–§9.5 for plugin authors who know none of it; they
add surface for humans, never semantics for the runtime. The Rust
SDK (round 41) is the reference shape: an `Actor` trait mirroring
the native `receive → effects` contract, a `Durable` super-trait
compile-required for snapshot exports (the vacuous-snapshot bug
class dies at build time), and entry points making the declaration
choice explicit — stateless / stateful / durable. Additional
language SDKs (JS on QuickJS-ng) must expose the same three words
over the same ABI.

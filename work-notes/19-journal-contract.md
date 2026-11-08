# 19 — The journal contract: fork, ghosts, and the admission list

> Status: DECIDED (discussion round 2026-11-08, before any
> implementation — the owner asked to discuss this face first; they
> have been trying to abstract it "seemingly forever"). Spec gains
> §8. Implementation (InMemory + SQLite behind the face) follows
> once the last open point — ghost-read authorization — settles.

## 1. The owner's framing (the discussion's ground rule)

*"Ideally I'd like to plug external journal providers (kafka-like,
NATS JetStream, DBMS backends), but I'd rather exclude providers
that can't fit a strict model and HAVE a strict model, than have an
API with badly defined invariants and semantics."*

Resolution: **the contract is invariants + an admission list.** A
provider is a thing satisfying the invariants; everything else is
not a provider, by name. The strictness lives in the exclusion list.

## 2. Ratified

- **Fork-based rewind** [DECIDED]: `fork(r)` creates a lineage from
  parent position r; rewind is a journaled fact, never a truncation.
  Truncate was rejected BECAUSE it excludes Kafka/JetStream-class
  providers (you cannot truncate a replicated log mid-stream) — the
  provider criterion decided the rewind semantics.
- **Ghost branches** [owner's term, adopted]: the abandoned tail is
  read-only history — *"ghosts of an alternate reality; they don't
  exist as effect-creating anymore."* Ghosts never deliver; they are
  readable. **A1 hosted/multi precedent: there was a tool letting
  the model READ its previous future.** The mind truncates in a
  rollback; the ghost library keeps the alternate reality queryable.
- Ticker coherence: arms/polls/ticks are letters, so the ticker's
  schedule replays from the journal — rewinds need no ticker-specific
  logic.
- Batch-as-transaction: the write unit is
  `{appends, acks, facts}` all-or-nothing at a durability point;
  external providers map it to one atomic record.
- Admission list with named exclusions (§8.4): SQLite ✓, JetStream ✓
  (batch-as-record), Kafka ✓ (EOS + infinite retention);
  at-most-once queues, CAS-less object storage, retention-loss
  systems are NOT providers, by name.
- `fork` joins the control-plane verbs (§3.3), capability-gated.

## 3. Open — ghost-read authorization

Who may read ghosts? **[Proposed]** a capability — `ghost.read`,
default-off, granted like any other row (attenuation-only; per-
lineage objects like `ghost.read:<lineage-id>` are the natural
attenuation). A1 precedent favors actor-facing access (the model had
the tool). Alternative: embedding-only, no actor surface in v0.

## 4. Next

Implementation once §3 settles: `InMemoryJournal` + `SQLiteJournal`
behind the face; the log-tier context switches to journal-backed
cursors; rewind test = fork a ghost branch and read it back.

## Provenance

Owner discussion, 2026-11-08. Sources: A1 work-notes 04 §8 (journal
integrity, contiguous-or-refuse), 21 §5 (fork-on-loss, read access
to pruned branches — the ghost precedent), Q10 (write-before-
delivery, correlation dedup).

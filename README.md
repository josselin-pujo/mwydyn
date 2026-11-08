# mwydyn

> # ⚠️ WORK IN PROGRESS — DO NOT DEPEND ON THIS
>
> **Pre-1.0. APIs break without notice. Releases are snapshots of a
> design in motion, not promises. Nothing here is stable, tested for
> production, or supported in any way. If you build on this before
> 1.0, you built on sand — on purpose.**
>
> The 1.0 bar: the core contract (spec §1–§8) frozen, WASM guests
> live, a durable journal provider proven across reopen, and one
> real non-Athena consumer running on it. Everything hard that isn't
> core is a pluggable implementation away — which is why 1.0 should
> come fast.

An embeddable WASM-actor substrate: **messages as data**, actors as
the core primitive, and a journal with **time travel** — fork-based
rewind where the abandoned future stays readable as *ghost branches*.

Named in 2019, rebuilt in 2026 as the substrate under
[Athena](https://github.com/) v2 — extracted greenfield from a
design that learned the hard lessons so this one doesn't have to.

## The idea, in one paragraph

Actors are WASM components (or native Rust — both compose
identically) that receive **letters** and return **effects**:
`receive(ctx, letter) → effects`. Writing is declaring — there is no
post/put/mint import; the runtime validates the whole effect list,
then applies it atomically with the cursor advance (log owns acks).
Blob bytes never transit messages — they travel by content-hash
reference with the media type hash-bound. There is no clock: the
clock is the inbox tick, and timers fire as letters. Capabilities
are grant rows checked at every boundary. The context is a trust
boundary, a clock domain, and a transaction wall — one wall, three
names. The journal is append-only; rewind forks a new lineage and
the abandoned future becomes a readable ghost branch behind a
capability.

## What works today

- **Canonical CBOR data model** (`mwydyn-data`) — deterministic
  encoding, key hygiene, blob refs with hash-bound media types
  (tag 1002), envelopes with checksums. 59 + 13 conformance vectors.
- **The actor runtime** (`mwydyn-core`) — contexts, cursors over
  the journal, the four-phase effects engine (validate-all → apply
  atomically), fail-fast/restart supervision, capabilities with
  attenuated mints, the ticker (clock-oblivious; durations, never
  timestamps), the RAM blob store with verify-on-read, task actors,
  the external outbox, and `call_task` as a one-call round trip.
- **The journal, with two providers** — `JournalFace` (§8: the
  contract is invariants + a provider admission list) implemented by
  `InMemoryJournal` and `SQLiteJournal` (WAL + FULL sync, canonical
  CBOR storage, verify-on-read). **Fork-based rewind** and **ghost
  branch reads** pass on both. The face tests are parameterized —
  substrate independence is proven by the suite, not claimed.
- **12 walking tests** running the whole ratified contract end to
  end with native Rust actors.

## What does not exist yet

- WASM guests (the world is drafted in [`docs/world-v0.wit`](docs/world-v0.wit);
  binding is the next milestone)
- Hydration (rebuilding actor state from the journal) and a
  durable-context end-to-end test
- The parallel executor (the contract is ratified — parallel
  transitions, serialized commits; the serial pump is the
  deterministic mode)
- Telemetry sinks, multi-node anything, the PaaS
- Schema validation against CDDL (the registry carries and pins;
  parsing is future work)

## Layout

| Crate | Role |
|---|---|
| `mwydyn-data` | canonical CBOR profile, data model, envelope, checksums |
| `mwydyn-schema` | schema registry (peer crate — the core is schema-blind) |
| `mwydyn-core` | contexts, effects engine, journal face + providers, ticker, blob store |
| `mwydyn-vectors` | conformance-vector runner |

Distributed journal providers (NATS, Kafka, consensus-backed) are
**pluggable crates outside the core** — the face's admission list
(§8.4) is what they must satisfy, and its `Send + Sync` bound is
what makes them zero-core-change plug-ins.

## Reading the design

The design record is the point of this repository as much as the
code:

- [`docs/spec-v0.md`](docs/spec-v0.md) — the contract, §1–§8
- [`work-notes/`](work-notes/) — 25 numbered rounds of decisions,
  arguments, and the laws that fell out of them
- [`docs/plan.md`](docs/plan.md) — the milestone plan
- [`AGENTS.md`](AGENTS.md) — the working laws
- [`sbom/`](sbom/README.md) — dependency licensing policy

Laws worth the price of admission (there are more in AGENTS.md):
messages are data, not blob containers (1 MiB cap); *allocation is
not an effect*; *the clock is the inbox tick*; *the journal is law*
(non-deterministic but recorded); the context is a trust boundary,
a clock domain, and a transaction wall; if the contract never reads
it, the core never carries it.

## License

Apache-2.0. Linked dependencies are admission-gated: permissive
licenses only (see [`sbom/README.md`](sbom/README.md) for the
policy and the forecast for wasmtime/tokio/QUIC).

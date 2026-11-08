# 25 — GitHub-ready README; distributed journals are pluggable crates

> Status: DECIDED + implemented (2026-11-08). README rewritten to
> current state with the owner's WIP disclaimer verbatim in spirit:
> **do not depend on this until 1.0**. Amends §8.5 (placement law).

## 1. The placement law (owner question, settled)

*"The big thing I'm not sure about is distributed reliable journals
belonging in the core or an impl crate."*

**Pluggable crates, NEVER core.** Three ratified laws converge:

1. Tight core (round 18): the contract reads the FACE, not any
   durable implementation behind it.
2. §8.4 already said it: "a distributed face brings its own
   serialization with it, OUTSIDE this contract."
3. The dependency wave: a NATS/Kafka/consensus journal drags tokio,
   TLS stacks, and client SDKs into the graph — exactly what the
   round-24 license/admission gate exists to police. It must never
   touch the core's graph (core deps = mwydyn-data only).

The face being `Send + Sync` (round 21) is what makes this
zero-core-change: a distributed provider is a crate — ours or a
third party's — implementing `JournalFace` against §8's admission
list (atomic batch at a durability point; contiguous per-lineage
seqs; no retention loss; ordered byte-identical replay). SQLite
stays default-feature-in-core per the owner's explicit SQLite-
everywhere position; slimming it out later is the owner's option.

Future option, not now: if third-party providers ever materialize,
the face can be extracted to a tiny contract crate so providers
don't depend on the whole core.

## 2. The README

Rewritten to current state with the owner's big-fat-disclaimer at
the top: **WIP, do not depend on whatsoever until 1.0** — with the
1.0 bar stated (core contract frozen, WASM guests live, durable
journal proven, one real consumer). Sections: the idea in one
paragraph; what works today (with the proof numbers); what does not
exist yet; layout; the design-reading pointers; laws worth the
price of admission; license.

## 3. Next

Unchanged: hydration + durable-context walking test closes M2;
then the executor, then WASM guests (M1's world binding).

## Provenance

Owner directive + question, 2026-11-08. Sources: rounds 18 (tight
core), 21 (Send+Sync face), 24 (admission-list licensing).

# 18 — Tight core: the registry leaves the runtime

> Status: DECIDED and implemented (2026-11-08). `mwydyn-core` depends
> on `mwydyn-data` only. Amends §7.9's "registry" placement.

## 1. The owner's catch

*"I'm not sure about the schema registry — it doesn't belong in
core and promises something core doesn't do. We will eventually
need a schema registry somewhere, don't get me wrong; I'd just keep
it out of core, especially at such an early phase. I'd like a very
tight core."*

Round 17 had put a `Registry` inside `Context` with a `registry()`
accessor — API surface implying schema awareness that round 10
(schema-blindness) explicitly forbids. The accessor was never called
by the runtime. Dead weight with a misleading name.

## 2. The law this adds to AGENTS

**If the contract never reads it, the core never carries it.** The
core's dependency list IS its scope statement: `mwydyn-core` →
`mwydyn-data`, nothing else. The registry stays a peer crate
(`mwydyn-schema` → `mwydyn-data`), consumable by embeddings that
want typed views or compatibility checks — never by the runtime.

§7.9 amendment: "the runtime ships the core schemas" now means *the
schema crate publishes them* (`core_schemas()`); `Context` carries
neither the registry nor the schemas. A registry is an
EMBEDDING-side composition artifact — instantiate one when you want
one; the runtime is registry-blind as well as schema-blind.

## 3. Implemented

- `mwydyn-core` loses the `mwydyn-schema` dependency, the
  `registry` field, `registry()` and `claim_schema()`.
- `mwydyn-schema` unchanged (its tests already cover
  `core_schemas()` and digest stability); the walking-test that
  exercised `ctx.registry()` is gone — coverage lives where the
  crate lives.
- Dependency graph is now the scope statement:
  `core → data ← schema`.

## Provenance

Owner pushback, 2026-11-08. Continues round 10's schema-blindness
and round 16's "the observable things only" tightening.

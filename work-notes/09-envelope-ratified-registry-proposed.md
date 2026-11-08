# 09 — Envelope ratified; schema registry proposed

> Status: envelope field set DECIDED (round 2026-11-08). Schema
> registry (§7.9) proposed, in discussion — the last critical-path
> fork before the walking skeleton. Amends
> [spec-v0](../docs/spec-v0.md) §2 (DECIDED), §7.9.

## 1. Ratified — envelope field set

As proposed in [work-notes/08](08-clock-domains-and-envelope.md):
producer-supplied `kind` / `payload` / `cause?` / `correlation?` /
`authority`; runtime-assigned `seq` (identity = `(context, seq)`) /
`from` / `to` / `commitTs` / `producer` (implementation digest) /
`checksum`. No `ttl`.

**Owner's law on reply-to, verbatim:** *"I am always very bothered
when people present the actor model and their first ever interaction
has a reply-to; it's an edge use-case that is easy to implement but
it has no place whatsoever as a first-order concept."* Reply routing
is application-level (correlation matching), never envelope plumbing.
Written into §2.

## 2. Proposed — the schema registry (§7.9)

1. **Ids**: `<namespace>.<name>@<major>`; the envelope's `kind`
   carries the id. Namespaces are claimed per context at composition
   (no central authority; conflicting definitions for one id refuse
   at composition).
2. **Evolution**: majors are **immutable once ratified** — a schema
   bug becomes a new major, never an edit. Within a major, registry
   revisions are **additive-only** (new optional fields); consumers
   are tolerant readers (A1 Q21's law, now with mechanics). The
   envelope carries the major only — minors are invisible on the
   wire.
3. **Definition language: CDDL** (RFC 8610) — the schema language
   designed for CBOR, validates canonical CBOR directly, text and
   greppable (agent-friendly), known to LLMs. The registry pins each
   definition's digest.
4. **v0 registry = static per-context composition artifact.** The
   runtime ships `mwydyn.*` core schemas (at minimum
   `mwydyn.timer.arm@1` / `mwydyn.timer.tick@1`); embeddings compose
   theirs (Athena2: `athena.*`; the compiler: its own). Dynamic
   schema install is PaaS-era, deliberately not v0.
5. **Consumed-kind declaration**: actors MAY declare the kinds they
   consume at spawn; declared ⇒ enforced at admission (post refuses
   `invalid`, detail to telemetry). Optional but self-documenting
   when used.
6. **Vector format**: JSON files; CBOR as hex + diagnostic notation;
   three vector classes — validity (schema × value → verdict),
   roundtrip (decode→encode byte-identical under the canonical
   profile), and message-level (letter → runtime verdict). A1's
   `cbor-v1.json` ports as the first corpus.

## 3. Open bits for discussion

- CDDL vs a custom schema-in-CBOR DSL (CDDL proposed; custom would
  be more vector-native but reinvents a standard).
- Additive-within-major vs bump-for-any-change (additive proposed;
  A1 precedent).
- Consumed-kind declaration: optional (proposed) vs mandatory.

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 04 §7 (additive evolution, tolerant readers), 21 §7 (the
journal is the lockfile), test-vectors/cbor-v1.json (vector style).

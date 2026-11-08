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
compose identically), faces-not-implementations (journal/blob).
**Implementation: M0 OPEN** (plan ACTIVE in [docs/plan.md](docs/plan.md)):
workspace live; `mwydyn-data` canonical CBOR profile **v0.1** built
(tag 1002 blob refs, hash-bound mediaTypes — round 12: content
identity includes type; digest = SHA-256(canonical CBOR of
{bytes, type})); **both corpora GREEN**: cbor-profile-v0 (59 cases) +
blob-refs-v0 (13 cases). Corpus wants are machine-generated
(`patch-corpus` example) — never hand-type hex; encode is a
serializer, not a validator (forged refs must be encodable for
rejection vectors). Toolchain rustc 1.99.1. Next M0 slice: envelope +
letter structs, then `mwydyn-core` (contexts, cursors, effects
engine, admission, supervision, ticker, RAM blob store, task
pattern) driven by native Rust actors + letter→verdict vectors.

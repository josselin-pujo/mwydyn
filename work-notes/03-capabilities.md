# 03 — Capabilities: grant rows, minted cites

> Status: base decided (round 2026-11-08, with the project owner); the
> default-deny boundary is [proposed, OPEN]. Amends
> [spec-v0](../docs/spec-v0.md) §3 (gains §3.3), §7 (fork 3 resolved
> in principle).

## 1. The decision

Grant rows as the base, minted attenuated references as the delegation
mechanism (owner ratified). The owner's caution — "capability-enabled
pointers are awesome, but dangerous to move around, and minting
attenuated pointers is a complex issue" — is answered structurally:

- **Moving refs around is harmless by construction**: rows are
  subject-bound and refs are row-cites. A ref copied through a blob,
  a log, or a message is inert to anyone but its subject. Capability
  pointers are addresses of rights, not rights. (The object-cap
  danger — possession = authority — was the thing being avoided.)
- **Minting complexity is tamed by locality + vectors**: attenuation
  is per-service pure algebra (`narrow`, `subsumes`), verified by the
  runtime at mint time, conformance-vector tested per service. The
  complexity is real but local to each object grammar, and a wrong
  algebra fails loudly in vectors instead of silently in prod.

## 2. The contract (spec §3.3)

- Grant rows `{subject, verb, object}`, checked per-use at every
  boundary against the live grant set; envelopes present `authority`
  refs [A1:04 §2].
- Verbs namespaced by service: `post`/`subscribe` (message layer),
  ambient verbs (`blob.read/write`, …), control-plane verbs
  (`spawn`, `supervise`, `bind`, `park`).
- Mints: attenuation-only, holder-only, journaled (cite the source
  row, name the grantee); runtime verifies subsumption, never trusts.
- Revocation cascades via lineage.
- Boot rows from spawn manifests; widening via consent ceremonies
  [A1:21 §1 posture].
- Tier rule: grant mutations are log events for log-tier actors;
  in-memory sets seeded at spawn for RAM-tier.
- Confused-deputy: inherent to held authority; mitigated by
  attribution (every use names the row), not by redesign.

## 3. Open — the default-deny boundary

Is `post` per-target (`post:<type>:<target>`) or type-level?

- **Per-target everywhere**: maximal control, but bureaucratic — every
  actor-to-actor exchange needs pre-minted rows; composition dies
  under paperwork.
- **Type-level everywhere**: fluid, but tenant-leaky — any actor may
  post any type to any target.

**[proposed]** the **trust-context hybrid**: type-level authority
inside a trust context (a composition, tenant, or session), explicit
target rows across contexts; cross-context minting is the ceremony
that makes PaaS service-to-service wiring explicit. Precedent: A1's
`journal.read:<self>` self-scoping is exactly a trust context.
Introduces "trust context" as a concept — feeds fork §7.4
(addressing), where tenant/session scoping must land anyway.

## Provenance

Round discussion with the project owner, 2026-11-08. Sources: A1
work-notes 04 §2 (authority on envelopes), 19 §8 + 21 §1 (attenuation-
only posture, net.fetch grammar as an attenuation algebra instance),
50 (live grant sets, CTR-3).

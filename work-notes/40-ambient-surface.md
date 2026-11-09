# 40 — The ambient host-face import surface

> Status: implemented and green (2026-11-09). 28 suites ok, clippy
> clean.

## 1. The design (one import, verbs as data)

Guests keep their zero-import purity BY DEFAULT; the surface comes
into existence deliberately when a guest declares it. ONE import:

```
mwydyn.ambient(in_ptr, in_len, out_ptr, out_cap) -> written
```

- in: ONE canonical CBOR envelope `{"verb": ..., <verb fields>}`;
- out: ONE canonical CBOR envelope `{"ok": <value>} | {"err":
  "<code>"}` written into the caller's buffer (`-1` = did not fit;
  the SDK doubles its scratch and retries);
- verbs evolve WITHOUT ABI churn — unknown verbs are refused by
  name (`UnsupportedVerb`), the refusal wall's discipline.

**THE VERB SET** mirrors the native `Host` face — same powers, same
cap checks (`row.holds(domain, "*")`):
`blob.get` (blob.read) · `blob.put` (blob.write) · `blob.len`
(blob.read) · `caps` (free — self-inspection).
NO clock verb exists — the ticker remains the only wall-clock reader
(§6 clock obliviousness). `ghost.read` arrives with its face.

## 2. The trust posture

Ambient verbs are GRANTED, never ambient: the pumps hand each actor
its `AmbientParts` (journal read side, blob face, context name, THE
ACTOR'S OWN rows) via `Actor::set_ambient` right before receive —
serial, pool (the job's pieces), spawn, restart, rewind, hydrate.
The dispatch cap-checks every verb against those rows; the
uncapped-guest test sees `Unauthorized` BY NAME — the refusal is
observed by the guest, never swallowed. The negative proof: the same
guest artifact capped and uncapped behaves differently, and the
difference is entirely the Spawn's declaration.

## 3. The pieces

- `mwydyn-data/src/ambient.rs` — the envelope contract:
  `AmbientCall` (verbs + fields), `ok_response`/`err_response`,
  `response_payload` (the guest-side pull), named refusals. Unit
  tests pin canonical roundtrips + the refusal names.
- `mwydyn-core` — `AmbientParts` (Arc handles, cheap clones) +
  `Actor::set_ambient` (default no-op — native actors hold Host
  directly); every pump site wired (the parts ride BEFORE the fork,
  so placeholders inherit them).
- `mwydyn-wasm` — `Store<Option<AmbientParts>>`; a per-
  instantiation Linker registers the import; the dispatch reads
  guest memory through `Caller`, cap-checks, executes against the
  faces, writes the response. Guards: `None` parts → `Unavailable`;
  oversized responses → `-1` (retry protocol).
- `mwydyn-guest` — THE GUEST SDK (rlib): `alloc`/`dealloc`/
  `respond`/`read_block` + `ambient_call` (the import declared under
  `#[link(wasm_import_module = "mwydyn")]`, host-target builds get a
  refusing stub so SDK logic tests off-wasm).

## 4. The bugs the proofs caught

- **The write that wasn't there**: the SDK allocated scratch and
  handed it over WITHOUT copying the request in — the host read
  zeros → BadForm. The host side decoded perfectly standalone; the
  glue above missed the memcpy. (Probe-the-layer discipline again:
  isolate the layer before blaming the one you suspect.)
- `#[link(wasm_import_module = "mwydyn")]` is what names the wasm
  import module — a bare extern lands in `env` and the linker never
  matches. Host-target builds of guest crates need a gated stub or
  the workspace test link fails (undefined symbol).
- ABI exports are the guest's to declare even when the
  implementation rides the SDK.

## 5. The proofs

- capped guest: put → get → len all green, bytes match, caps lists
  exactly the two declared rows;
- uncapped guest: `putErr: "Unauthorized"` observed BY THE GUEST,
  no fake successes, caps lists zero rows.
- every pre-existing suite stays green — the zero-import guests
  (minimal, counter) instantiate unchanged through the same linker.

## Provenance

Owner: "ambient host-face import surface next, merge actor can
wait" 2026-11-09.

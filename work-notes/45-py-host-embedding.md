# 45 — The Python host embedding (mwydyn-py, PyO3)

> Status: implemented and green (2026-11-10) — the interop bench
> (§3) PASSES: python actor + JS echo guest, one drain, logs from
> both realms. Suite green, clippy clean. Tests need
> PYO3_PYTHON=C:/Python312/python.exe (the conda 3.8 default
> breaks embedded init — encodings). Role OWNER-RATIFIED: host-side
> embedding — Python embeds the runtime; actors are native-realm
> (trusted in-process code, GIL-serialized, compose-parity with WASM
> guests in the same context). The stateless-only v0 default remains
> UNRATIFIED-in-force (§2.2 of this note).

## 1. Why (owner's framing, 2026-11-10)

First users are Rust hosts — but (a) ACTOR-ISOLATED TEST CASES become
scripted instead of compiled ("so much easier if we don't need to
compile every single test harness in Rust"), and (b) the moment a
network transport exists, PYTHON NODES are invaluable (the bridge
pattern; envelope tokens stay dormant until then). The round ships
(a) as its proof; (b) shapes only the API surface (nothing may
preclude remote nodes).

## 2. The design

- **`crates/mwydyn-py`** — cdylib + rlib, `pyo3` (link-libpython v0;
  the build needs Python 3.8+ installed — this box has 3.8/3.11/3.12).
  PLACEMENT LAW: a peer host-side crate; pyo3 never touches the
  core's graph (same law as distributed journals).
- **The surface** (the walking-test surface as a Python API):
  `Context(name)`, `Context.with_sqlite(dir)` for durable tiers;
  `spawn_python(name, actor, supervision=..., rows=...)`;
  `spawn_wasm(name, artifact_path, ...)` (mwydyn-wasm under it);
  `post(to, kind, payload, correlation=..., parent=...)`;
  `run_until_idle(ms)`; `drain_external()`; `drain_telemetry()`;
  `snapshot_actor(name)`; `dead_letters()`.
- **Python actors** (native realm):
  `class Echo(mwydyn.Actor): def receive(self, letter, cx) -> list`
  — letter carries `kind/payload/cause/correlation/authority` with
  the payload DECODED (Value⇄Python: dict/list/str/int/float/bool/
  None/bytes; BlobRef as a frozen object; ints beyond ±2^53 ride
  through Python's arbitrary-precision as BigPos/BigNeg). Effects
  returned, never called: `mwydyn.Effect.post/reply/reply_via/log/
  metric` — the same vocabulary as the Rust and JS SDKs (§9.6's
  three-words law, host-side).
- **`cx`**: `sender()` + the ambient face via the native Host
  (blob_put/get/len, caps) — refusals raised as `MwydynRefusal`
  with the code BY NAME (Python exceptions are the natural strike
  path: a raised exception IS a panic IS a strike — same law).
- **Statelessness (default in force, unratified)**: v0 Python actors
  are all stateless-declared — the shim's fork reuses the SAME
  PyObject (the instance persists; no rollback promised). The
  `stateful` decorator stays reserved; deepcopy/pickle semantics
  would be a later round with vectors.

## 3. The proof (the interop bench, as the owner framed it)

One Python-driven test: a context with a PYTHON actor and the JS
echo GUEST (round 43's artifact) composing — the Python actor
kicks the JS guest, the guest's reply routes to Python's outbox,
and ONE `drain_telemetry()` shows logs from BOTH realms. The
contracts get their cross-realm stress test.

## 4. GIL and threads

v0 drives everything from the Python thread (the test-harness
shape); the shim takes `Python::with_gil` per receive. The worker
pool is not exposed; when it is, Python actors serialize on the GIL
(glue-speed, ~µs/call vs the ~22µs committer bound — fine, and now
measured, not assumed).

## 5. The lessons

- **Effect addresses resolve AT THE SHIM, not the builder**: the
  Python-visible `Effect.post` passes `to` through as-given (name
  string or {context, name}); the shim resolves with the actor's
  REAL context from AmbientParts. Resolving in the builder baked
  `context: ""` and every post struck ("unknown target").
- **Host<'a> is a view; the cx owns Arcs** (AmbientParts) — pyclass
  types cannot borrow the host view.
- **Verdict, not Result**: round 44's injection handle means `post`
  returns `Accepted { seq } | Refused` — the binding surfaces the
  seq (the external id↔seq mapping stays embedder-side, §3.4).
- pyo3 0.23 API discipline: Bound/Py::new/clone_ref, run wants
  &CString; the to_object→IntoPyObject migration wave is allowed at
  crate root for v0.

## Provenance

Owner, 2026-11-10: "first users will be rust hosts, but as soon as
a network transport happens having python nodes is going to be
invaluable… making actor isolated test cases is so much easier if
we don't need to compile every single test harness in rust."

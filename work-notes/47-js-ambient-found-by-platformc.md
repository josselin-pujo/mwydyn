# 47 — The JS guest's ambient surface, found by the first consumer

> Status: implemented and green (2026-11-11) — full container suite
> green incl. the new JS-ambient regression pin; platformc's JS pass
> (the first real JS plugin) runs end to end. Three wrapper defects
> fixed, one diagnostics gap closed. No design change: §9.4's contract
> was correct in the spec and in the RUST guest SDK — only the JS
> wrapper's FFI layer had never been exercised.

## 1. Why the bugs survived round 43

The JS story shipped with the echo example, which never calls
`cx.blob*` — the ambient trampoline had exactly zero test coverage on
the JS side (the ambient suite drives the RUST store guest). The
first real JS plugin (platformc's pass contract stages everything
through blobs) hit all three defects on its first `blobPut`.

## 2. The three defects

1. **The ambient trampoline freed its borrowed argument** — argv
   carries JSValueConst (quickjs owns and frees it at frame teardown);
   the trampoline's `JS_FreeValue(ctx, req)` double-freed. Crash
   shape: `js_free_value_rt` inside `JS_CallInternal`. Fix: drop the
   free.
2. **`JS_GetArrayBuffer`'s binding reshaped the real signature.**
   quickjs-ng: `uint8_t *JS_GetArrayBuffer(ctx, size_t *psize,
   JSValueConst obj)` — the data pointer is the RETURN, the size goes
   through psize. The binding declared `(ctx, pbuf*, obj) -> i32`, so
   every call misread its arguments, failed, and the typed-array
   branch of `js_to_value` NEVER ran — a Uint8Array fell through to
   plain-object conversion with its indices as map keys, refused as
   `BadKeyHygiene("0")` (the ledger: "receive threw" → panic → strike).
   Fix: binding corrected to the real shape; the call site reads the
   returned pointer; the non-buffer fall-through now frees the handle
   (a leak since round 43). The branded-ref read path leaked its
   property values the same way — also freed now.
3. **The panic stash was unreadable through the container.** The
   round-43 hook stashes panics into `MWJS_PANIC_BUF`, which exports
   as a CONST global (it holds the static's ADDRESS). The container's
   globals collector takes Var globals only (correctly — the state
   image law), so `js_panic_stash` diagnostics had nothing to read.
   Fix: `GuestRt` captures the stash global explicitly at
   instantiation, and the barrier's process-trap panic now appends
   `js panic stash: <message>` — every future JS guest trap is
   debuggable from the host error alone (probe.rs's read pattern,
   promoted into the container).

## 3. The regression pin

`js_guest.rs::js_guest_ambient_blob_round_trip` — a composed JS guest
puts a blob, gets it back, replies with ref + bytes + len + caps;
asserts the BlobRef round-trips as a real tag-1002 value, the bytes
survive Uint8Array → Bytes, blobLen agrees, and the granted rows are
visible. Runs through the same library the build tool uses (no second
committed artifact); `scripts/build-guest-js.sh` now also stages the
uncomposed engine so the pin runs. Found-by attribution: platformc
(the first consumer's pass contract stages both directions through
blobs — exactly the surface round 43 never exercised).

## 4. Environment

mwydyn-py interop needs `PYO3_PYTHON=C:/Python312/python.exe` in a
bare shell (work-note 45's convention; the conda 3.8 default breaks
embedded init).

//! mwydyn-guest — THE GUEST SDK (round 40): the wasm-side barrier
//! glue for Rust guests. mwydyn-data IS the codec (canonical CBOR
//! both realms); this crate adds the linear-memory mechanics —
//! scratch management, the `[u32 len][bytes]` response block, and
//! the ambient-call wrapper over the `mwydyn.ambient` import.
//!
//! A guest exposes (see mwydyn-guest-minimal / -counter / -store):
//! `alloc(len) -> ptr`, `dealloc(ptr, len)`, `process(req_ptr,
//! req_len) -> resp_ptr` — plus optional `restore`/`snapshot` (§7.6)
//! and whatever imports it declares (`mwydyn.ambient`).

use mwydyn_data::{decode, encode, DecodeOptions, Value};

/// Hand `len` bytes of scratch to the host (the barrier's inbound
/// buffer). Backed by a leaked boxed slice — exact capacity.
pub fn alloc(len: usize) -> *mut u8 {
    assert!(len > 0, "guest sdk: zero-length alloc");
    let buf = vec![0u8; len].into_boxed_slice();
    Box::into_raw(buf) as *mut u8
}

/// Return scratch previously handed out by [`alloc`].
///
/// # Safety
/// `ptr` must be a pointer previously returned by [`alloc`] with the
/// same `len`, not yet deallocated.
pub unsafe fn dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

/// Wrap response bytes in the ABI block `[u32 le len][bytes]`.
pub fn respond(bytes: &[u8]) -> *mut u8 {
    let mut buf = Vec::with_capacity(4 + bytes.len());
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    Box::into_raw(buf.into_boxed_slice()) as *mut u8
}

/// Read a response block handed back by the host.
///
/// # Safety
/// `ptr` must point to a block the host wrote: `[u32 le len][bytes]`.
pub unsafe fn read_block(ptr: *const u8) -> Vec<u8> {
    let prefix = unsafe { std::slice::from_raw_parts(ptr, 4) };
    let len = u32::from_le_bytes(prefix.try_into().expect("4 bytes")) as usize;
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr.add(4), len) });
    out
}

// The import lives under its own module name; on HOST builds the
// symbol does not exist — host-target compiles (SDK unit tests) get
// a stub that refuses, wasm builds get the real import.
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "mwydyn")]
extern "C" {
    // the wasm import is ("mwydyn", "ambient") — the linker's name
    fn ambient(in_ptr: *const u8, in_len: usize, out_ptr: *mut u8, out_cap: usize) -> i32;
}

#[cfg(target_arch = "wasm32")]
unsafe fn ambient_raw(
    in_ptr: *const u8,
    in_len: usize,
    out_ptr: *mut u8,
    out_cap: usize,
) -> i32 {
    unsafe { ambient(in_ptr, in_len, out_ptr, out_cap) }
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn ambient_raw(
    _in_ptr: *const u8,
    _in_len: usize,
    _out_ptr: *mut u8,
    _out_cap: usize,
) -> i32 {
    -1
}

/// The ambient call: canonical CBOR request in, `{"ok"} | {"err"}`
/// response out. `Err(code)` carries the host's refusal code
/// (`Unauthorized`, `Missing`, `UnsupportedVerb`, ...). The guest
/// keeps ONE response scratch that grows on demand (the host refuses
/// oversized writes with -1 and the call retries).
pub fn ambient_call(call: Value) -> Result<Value, String> {
    let request = encode(&call);
    let in_ptr = alloc(request.len());
    // THE WRITE the first cut forgot: scratch arrives zeroed — the
    // request must land in it or the host reads zeros (BadForm).
    unsafe {
        std::slice::from_raw_parts_mut(in_ptr, request.len()).copy_from_slice(&request);
    }
    // SAFETY: in_ptr is a live alloc of exactly request.len().
    let mut out_cap = 256usize;
    loop {
        let out_ptr = alloc(out_cap);
        // SAFETY: both pointers are live scratch with the declared
        // lengths; the host never retains them.
        let written = unsafe {
            ambient_raw(in_ptr, request.len(), out_ptr, out_cap)
        };
        if written >= 0 {
            // SAFETY: the host wrote `written` bytes at out_ptr.
            let resp_bytes = unsafe { read_block_from(out_ptr, written as usize) };
            unsafe { dealloc(in_ptr, request.len()) };
            unsafe { dealloc(out_ptr, out_cap) };
            let v = decode(&resp_bytes, &DecodeOptions::default())
                .map_err(|e| format!("BadForm: {e:?}"))?;
            return match &v {
                Value::Map(pairs) => {
                    for (k, val) in pairs {
                        match k.as_str() {
                            "ok" => return Ok(val.clone()),
                            "err" => {
                                return match val {
                                    Value::Text(code) => Err(code.clone()),
                                    _ => Err("err without a code".into()),
                                }
                            }
                            _ => {}
                        }
                    }
                    Err("response carries neither ok nor err".into())
                }
                _ => Err("response not a map".into()),
            };
        }
        unsafe { dealloc(out_ptr, out_cap) };
        out_cap *= 2;
        if out_cap > (1 << 22) {
            return Err("OutCap".into());
        }
    }
}

/// Read `len` bytes at `ptr` (the host-written response body).
/// # Safety
/// `ptr` must point to `len` initialized bytes.
unsafe fn read_block_from(ptr: *mut u8, len: usize) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
}

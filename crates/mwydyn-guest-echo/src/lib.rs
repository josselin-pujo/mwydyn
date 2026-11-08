//! mwydyn-guest-echo — a minimal WASM guest actor for testing.
//! Implements the receiver export: takes a letter, returns effects.
//! Compiled to wasm32-wasip2.

// Use wit-bindgen to generate the guest bindings from the WIT.
// For now, use the raw wasmtime ABI (simplified — no component model)
// to prove the wasmtime plumbing. The full component-model binding
// comes with the WIT types stabilization.

// Export a function that echoes a u64 back (the simplest possible
// letter processing — proves instantiation and calling work).
#[no_mangle]
pub extern "C" fn echo(value: u64) -> u64 {
    value.wrapping_add(1)
}

#[no_mangle]
pub extern "C" fn alloc(size: u32) -> u32 {
    // Minimal bump allocator for linear memory (for testing host→guest
    // memory sharing patterns). Returns a pointer to `size` bytes.
    let mut buf: Vec<u8> = Vec::with_capacity(size as usize);
    let ptr = buf.as_mut_ptr() as u32;
    std::mem::forget(buf);
    ptr
}

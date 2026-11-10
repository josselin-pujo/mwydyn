//! The wasm-side heap and libm (round 43): quickjs-ng's C code
//! calls malloc/libm; on the freestanding target those symbols come
//! from HERE — one unified heap (Rust's allocator owns linear
//! memory) and exact math via the pure-Rust `libm` crate (wasm has
//! floor/ceil/sqrt instructions but NO pow/log2/fmod — and LLVM
//! lowers those intrinsics to calls to the very C symbols this
//! module defines, so they must not call back into intrinsics).
//! Compiled only for the wasm target; native links the real libc.

#![cfg(target_arch = "wasm32")]

use std::alloc::{alloc, dealloc, realloc as rust_realloc, Layout};

/// Every block carries a 16-byte header with its size — the C
/// contract (free/realloc without sizes) becomes exact `Layout`s,
/// which the global allocator requires for soundness.
const HDR: usize = 16;
const ALIGN: usize = 16;

fn user_to_block(ptr: *mut u8) -> (*mut u8, Layout) {
    debug_assert!(!ptr.is_null(), "free/realloc of null handled by callers");
    let block = unsafe { ptr.sub(HDR) };
    let size = unsafe { (block as *const usize).read() };
    (block, Layout::from_size_align(size, ALIGN).expect("stored layout"))
}

fn block_ptr(size: usize) -> *mut u8 {
    let total = size + HDR;
    let block = unsafe { alloc(Layout::from_size_align(total, ALIGN).expect("size")) };
    if block.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { (block as *mut usize).write(total) };
    unsafe { block.add(HDR) }
}

#[no_mangle]
pub extern "C" fn malloc(size: usize) -> *mut u8 {
    block_ptr(size)
}

#[no_mangle]
pub extern "C" fn calloc(nmemb: usize, size: usize) -> *mut u8 {
    let total = nmemb.checked_mul(size).expect("calloc overflow");
    let ptr = block_ptr(total);
    if !ptr.is_null() {
        unsafe { std::ptr::write_bytes(ptr, 0, total) };
    }
    ptr
}

#[no_mangle]
pub extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    if ptr.is_null() {
        return block_ptr(size);
    }
    let (block, old) = user_to_block(ptr);
    let new = block_ptr(size);
    if new.is_null() {
        return std::ptr::null_mut();
    }
    // copy USER bytes only — starting at the block start would drag
    // the OLD size header over the new one and poison every later
    // free (found the hard way: dlmalloc traps on layout mismatch)
    let old_user = old.size() - HDR;
    let copy = old_user.min(size);
    unsafe { std::ptr::copy_nonoverlapping(ptr, new, copy) };
    unsafe { dealloc(block, old) };
    new
}

#[no_mangle]
pub extern "C" fn free(ptr: *mut u8) {
    if !ptr.is_null() {
        let (block, layout) = user_to_block(ptr);
        unsafe { dealloc(block, layout) };
    }
}

#[no_mangle]
pub extern "C" fn malloc_usable_size(ptr: *const u8) -> usize {
    if ptr.is_null() {
        return 0;
    }
    user_to_block(ptr as *mut u8).1.size() - HDR
}

// ---- libm (pure Rust — never the intrinsics, see module doc) ----

macro_rules! math1 {
    ($name:ident, $rust:ident) => {
        #[no_mangle]
        pub extern "C" fn $name(x: f64) -> f64 {
            libm::$rust(x)
        }
    };
}

math1!(floor, floor);
math1!(ceil, ceil);
math1!(round, round);
math1!(trunc, trunc);
math1!(log2, log2);
math1!(fabs, fabs);
math1!(sqrt, sqrt);
math1!(acos, acos);
math1!(asin, asin);
math1!(atan, atan);
math1!(cos, cos);
math1!(sin, sin);
math1!(tan, tan);
math1!(cosh, cosh);
math1!(sinh, sinh);
math1!(tanh, tanh);
math1!(exp, exp);
math1!(log, log);
math1!(log10, log10);
math1!(log1p, log1p);
math1!(expm1, expm1);
math1!(cbrt, cbrt);
math1!(acosh, acosh);
math1!(asinh, asinh);
math1!(atanh, atanh);

#[no_mangle]
pub extern "C" fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

#[no_mangle]
pub extern "C" fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

#[no_mangle]
pub extern "C" fn scalbn(x: f64, exp: i32) -> f64 {
    libm::scalbn(x, exp)
}

#[no_mangle]
pub extern "C" fn copysign(x: f64, y: f64) -> f64 {
    libm::copysign(x, y)
}

#[no_mangle]
pub extern "C" fn fmod(x: f64, y: f64) -> f64 {
    libm::fmod(x, y)
}

#[no_mangle]
pub extern "C" fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

#[no_mangle]
pub extern "C" fn modf(x: f64, iptr: *mut f64) -> f64 {
    let (frac, int) = libm::modf(x);
    unsafe {
        *iptr = int;
    }
    frac
}

#[no_mangle]
pub extern "C" fn ldexp(x: f64, exp: i32) -> f64 {
    libm::ldexp(x, exp)
}

#[no_mangle]
pub extern "C" fn frexp(x: f64, exp: *mut i32) -> f64 {
    let (mant, e) = libm::frexp(x);
    unsafe {
        *exp = e;
    }
    mant
}

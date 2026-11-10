//! The quickjs-ng C API surface the wrapper uses (v0.18.0,
//! JS_NAN_BOXING=0 — the 16-byte JSValue everywhere). Ownership
//! follows the header's law: a function returning JSValue transfers
//! ownership (caller frees); JSValueConst parameters borrow.

#![allow(non_camel_case_types)]

use std::ffi::c_void;

pub type JSRuntime = c_void;
pub type JSContext = c_void;

#[repr(C)]
#[derive(Copy, Clone)]
pub union JSValueUnion {
    pub int32: i32,
    pub float64: f64,
    pub ptr: *mut c_void,
    pub uint64: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct JSValue {
    pub u: JSValueUnion,
    pub tag: i64,
}

impl JSValue {
    pub const fn mkval(tag: i64, v: i32) -> JSValue {
        JSValue {
            u: JSValueUnion { int32: v },
            tag,
        }
    }
    pub fn ptr(tag: i64, p: *mut c_void) -> JSValue {
        JSValue {
            u: JSValueUnion { ptr: p },
            tag,
        }
    }
    pub fn float(v: f64) -> JSValue {
        JSValue {
            u: JSValueUnion { float64: v },
            tag: JS_TAG_FLOAT64,
        }
    }
    pub fn tag(&self) -> i64 {
        self.tag
    }
    pub fn is_exception(&self) -> bool {
        self.tag == JS_TAG_EXCEPTION
    }
}

pub type JSValueConst = JSValue;

// tags (quickjs.h, non-nan-boxing values)
pub const JS_TAG_BIG_INT: i64 = -9;
pub const JS_TAG_SYMBOL: i64 = -8;
pub const JS_TAG_STRING: i64 = -7;
pub const JS_TAG_OBJECT: i64 = -1;
pub const JS_TAG_INT: i64 = 0;
pub const JS_TAG_BOOL: i64 = 1;
pub const JS_TAG_NULL: i64 = 2;
pub const JS_TAG_UNDEFINED: i64 = 3;
pub const JS_TAG_EXCEPTION: i64 = 6;
pub const JS_TAG_FLOAT64: i64 = 8;

pub const JS_EVAL_TYPE_GLOBAL: i32 = 0;

extern "C" {
    pub fn JS_NewRuntime() -> *mut JSRuntime;
    pub fn JS_FreeRuntime(rt: *mut JSRuntime);
    pub fn JS_SetMemoryLimit(rt: *mut JSRuntime, limit: usize);
    pub fn JS_NewContext(rt: *mut JSRuntime) -> *mut JSContext;
    pub fn JS_FreeContext(ctx: *mut JSContext);
    pub fn JS_Eval(
        ctx: *mut JSContext,
        input: *const u8,
        input_len: usize,
        filename: *const u8,
        flags: i32,
    ) -> JSValue;
    pub fn JS_GetException(ctx: *mut JSContext) -> JSValue;
    pub fn JS_FreeValue(ctx: *mut JSContext, v: JSValue);
    pub fn JS_ToCStringLen2(
        ctx: *mut JSContext,
        plen: *mut usize,
        val: JSValueConst,
        cesu8: bool,
    ) -> *const u8;
    pub fn JS_FreeCString(ctx: *mut JSContext, ptr: *const u8);
    pub fn JS_GetGlobalObject(ctx: *mut JSContext) -> JSValue;
    pub fn JS_GetPropertyStr(
        ctx: *mut JSContext,
        this_obj: JSValueConst,
        prop: *const u8,
    ) -> JSValue;
    pub fn JS_Call(
        ctx: *mut JSContext,
        func_obj: JSValueConst,
        this_obj: JSValueConst,
        argc: i32,
        argv: *const JSValue,
    ) -> JSValue;
    pub fn JS_NewStringLen(ctx: *mut JSContext, s: *const u8, len: usize) -> JSValue;
    pub fn JS_NewObject(ctx: *mut JSContext) -> JSValue;
    pub fn JS_NewBigInt64(ctx: *mut JSContext, v: i64) -> JSValue;
    pub fn JS_SetPropertyStr(
        ctx: *mut JSContext,
        this_obj: JSValue,
        prop: *const u8,
        val: JSValue,
    ) -> i32;
    pub fn JS_SetInterruptHandler(
        rt: *mut JSRuntime,
        cb: Option<unsafe extern "C" fn(rt: *mut JSRuntime, opaque: *mut c_void) -> bool>,
        opaque: *mut c_void,
    );
    pub fn JS_ToInt64(ctx: *mut JSContext, pres: *mut i64, val: JSValueConst) -> i32;
    pub fn JS_ToFloat64(ctx: *mut JSContext, pres: *mut f64, val: JSValueConst) -> i32;
    pub fn JS_GetTypedArrayBuffer(
        ctx: *mut JSContext,
        obj: JSValueConst,
        pbyte_offset: *mut usize,
        pbyte_length: *mut usize,
        pbytes_per_element: *mut usize,
    ) -> JSValue;
    pub fn JS_GetPropertyUint32(
        ctx: *mut JSContext,
        this_obj: JSValueConst,
        idx: u32,
    ) -> JSValue;
    pub fn JS_IsArray(val: JSValueConst) -> bool;
    pub fn JS_IsError(val: JSValueConst) -> bool;
}

// ---- conversion-layer additions (round 43 wrapper) ----
pub type JSPropertyEnum = c_void;
pub const JS_GPN_STRING_MASK: i32 = 1; // (1 << 0): include string-keyed properties
pub const JS_PROP_C_W_E: i32 = 0x01 | 0x02 | 0x04; // configurable|writable|enumerable

extern "C" {
    pub fn JS_NewArray(ctx: *mut JSContext) -> JSValue;
    pub fn JS_SetPropertyUint32(ctx: *mut JSContext, this_obj: JSValue, idx: u32, val: JSValue) -> i32;
    pub fn JS_GetOwnPropertyNames(
        ctx: *mut JSContext,
        ptab: *mut *mut JSPropertyEnum,
        plen: *mut u32,
        obj: JSValueConst,
        flags: i32,
    ) -> i32;
    pub fn JS_FreePropertyEnum(ctx: *mut JSContext, tab: *mut JSPropertyEnum, len: u32);
    pub fn JS_DefinePropertyValueStr(
        ctx: *mut JSContext,
        this_obj: JSValue,
        prop: *const u8,
        val: JSValue,
        flags: i32,
    ) -> i32;
    pub fn JS_NewUint8ArrayCopy(ctx: *mut JSContext, buf: *const u8, len: usize) -> JSValue;
    pub fn JS_ToInt32(ctx: *mut JSContext, pres: *mut i32, val: JSValueConst) -> i32;
    pub fn JS_GetArrayBuffer(ctx: *mut JSContext, pbuf: *mut *mut u8, obj: JSValueConst) -> i32;
    // JS_NewCFunction is a static inline in the header — the real
    // entry point is JS_NewCFunction2 (generic calling convention).
    pub fn JS_NewCFunction2(
        ctx: *mut JSContext,
        func: Option<unsafe extern "C" fn(*mut JSContext, JSValue, i32, *mut JSValue) -> JSValue>,
        name: *const u8,
        length: i32,
        cproto: i32, // JSCFunctionEnum; 0 = JS_CFUNC_generic
        magic: i32,
    ) -> JSValue;
    pub fn JS_Throw(ctx: *mut JSContext, obj: JSValue) -> JSValue;
    pub fn JS_NewInt32(ctx: *mut JSContext, v: i32) -> JSValue;
    pub fn JS_AtomToCStringLen(ctx: *mut JSContext, plen: *mut usize, atom: u32) -> *const u8;
    pub fn JS_GetProperty(ctx: *mut JSContext, this_obj: JSValueConst, prop: u32) -> JSValue;
}

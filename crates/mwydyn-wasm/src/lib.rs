//! mwydyn-wasm — the WASM actor container (M1, world v0.5 binding).
//!
//! Instantiates WASM components as actors. The hot tier is always
//! InMemory; this crate provides the WASM execution realm.

pub mod container;
pub mod engine;

pub use container::WasmEngine;

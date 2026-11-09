//! mwydyn-wasm — the WASM actor container (M1, round 33).
//!
//! Instantiates plain `wasm32-unknown-unknown` modules as actors
//! behind the barrier ABI: canonical CBOR bytes over linear memory,
//! zero guest imports, no WASI. The hot tier is always InMemory;
//! this crate provides the WASM execution realm.

pub mod container;

pub use container::{external_address, WasmActor, WasmEngine};

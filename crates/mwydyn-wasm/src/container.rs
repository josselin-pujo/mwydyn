//! The WASM actor container — instantiates WASM components as actors
//! and drives `receive` per letter, returning effects. Compose-parity
//! with native actors: same `receive(ctx, letter) → effects` shape,
//! WASM execution realm.

use mwydyn_core::{Actor, Effect, Host, MsgCtx};
use mwydyn_data::Letter;
use wasmtime::{Config, Engine, Instance, Module, Store, TypedFunc};

/// The wasmtime engine — shared across instances, epoch-based
/// deadline enforcement, component model.
pub struct WasmEngine {
    engine: Engine,
    _epoch_thread: std::thread::JoinHandle<()>,
}

impl WasmEngine {
    /// Create with epoch interruption (§6: wall-clock budget per
    /// receive, not per-instruction fuel).
    pub fn new() -> Result<WasmEngine, anyhow::Error> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;

        let engine_handle = engine.clone();
        let ticker = std::thread::Builder::new()
            .name("mwydyn-epoch".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(1));
                engine_handle.increment_epoch();
            })
            .expect("epoch ticker spawns");

        Ok(WasmEngine {
            engine,
            _epoch_thread: ticker,
        })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

impl std::ops::Deref for WasmEngine {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        &self.engine
    }
}

/// A WASM actor: wraps a wasmtime instance as a native `Actor`.
/// The guest module exports a `process(count: i64) → i64` function;
/// the container marshals letter payloads to/from WASM memory.
pub struct WasmActor {
    store: Store<()>,
    process_fn: TypedFunc<i64, i64>,
    target: mwydyn_data::Address,
}

impl WasmActor {
    /// Create a WASM actor from a WAT module (direct text-rep
    /// compilation — no separate guest compilation needed for tests).
    /// The module must export a `process(param: i64) → result: i64`.
    pub fn from_wat(
        engine: &WasmEngine,
        wat: &str,
        target: mwydyn_data::Address,
    ) -> Result<WasmActor, anyhow::Error> {
        let module = Module::new(engine.engine(), wat)?;
        let mut store = Store::new(engine.engine(), ());
        let instance = Instance::new(&mut store, &module, &[])?;
        let process_fn = instance
            .get_typed_func::<i64, i64>(&mut store, "process")
            .map_err(|e| anyhow::anyhow!("no `process(i64)→i64` export: {e}"))?;
        Ok(WasmActor {
            store,
            process_fn,
            target,
        })
    }
}

impl Actor for WasmActor {
    fn fork(&self) -> Box<dyn Actor> {
        // WASM state cloning (linear memory snapshot) is §7.6 —
        // stateless actors (pure functions) don't need it.
        unimplemented!("WASM fork — §7.6 snapshots for stateful WASM actors")
    }

    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        // Decode the letter payload as an i64 counter value.
        let payload = &letter.payload;
        if payload.len() < 8 {
            return vec![];
        }
        let count = i64::from_le_bytes(payload[..8].try_into().unwrap());

        // Call the WASM guest function. The result is the transformed
        // value — the container wraps it as a Post effect.
        let result = self.process_fn.call(&mut self.store, count).unwrap_or(0);

        // Post the result to the target.
        vec![Effect::Post {
            to: self.target.clone(),
            letter: Letter::new(
                "test.wasm.processed@1",
                result.to_le_bytes().to_vec(),
                None,
                letter.correlation.clone(),
                vec![],
            )
            .unwrap(),
        }]
    }
}

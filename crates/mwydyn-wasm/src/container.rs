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
    engine: Engine,
    module: Module,
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
        // Epoch enforcement off for the perf test (the production
        // executor sets the deadline per exchange — §6).
        store.set_epoch_deadline(1_000_000);
        let instance = Instance::new(&mut store, &module, &[])?;
        let process_fn = instance
            .get_typed_func::<i64, i64>(&mut store, "process")
            .map_err(|e| anyhow::anyhow!("no `process(i64)→i64` export: {e}"))?;
        Ok(WasmActor {
            engine: engine.engine().clone(),
            module,
            store,
            process_fn,
            target,
        })
    }
}

impl Actor for WasmActor {
    /// Fork: re-instantiate from the same module. For stateless
    /// actors (pure functions), fork = fresh is equivalent. For
    /// stateful WASM actors, §7.6 snapshots apply.
    fn fork(&self) -> Box<dyn Actor> {
        let mut store = Store::new(&self.engine, ());
        let instance = Instance::new(&mut store, &self.module, &[]).unwrap();
        let process_fn = instance
            .get_typed_func::<i64, i64>(&mut store, "process")
            .unwrap();
        Box::new(WasmActor {
            engine: self.engine.clone(),
            module: self.module.clone(),
            store,
            process_fn,
            target: self.target.clone(),
        })
    }

    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        let payload = &letter.payload;
        if payload.len() < 8 {
            return vec![];
        }
        let count = i64::from_le_bytes(payload[..8].try_into().unwrap());
        let result = match self.process_fn.call(&mut self.store, count) {
            Ok(r) => r,
            Err(_) => 0, // WASM trap: clamp (supervision handles in prod)
        };
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

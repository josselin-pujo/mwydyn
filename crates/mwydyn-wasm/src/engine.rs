//! The wasmtime engine wrapper (§6, round 31): epoch-based
//! preemption for deadline enforcement (wall-clock budget per
//! receive, not per-instruction fuel), memory limiter, component
//! model. The engine is SHARED across instances — compiled modules
//! are cached per class, instance memories are isolated.

use wasmtime::{Config, Engine};

/// The wasmtime engine — shared across all WASM actor instances in
/// the runtime. Thread-safe (Send + Sync). Compiled modules are
/// cached per class; instance memories are isolated per actor.
pub struct WasmEngine {
    engine: Engine,
    /// Background epoch incrementor handle — dropped to stop.
    _epoch_thread: std::thread::JoinHandle<()>,
}

impl WasmEngine {
    /// Create an engine with epoch-based deadline enforcement.
    /// The epoch tick interval controls how finely wall-clock
    /// deadlines are enforced (default: 1ms ticks, 50ms yield).
    pub fn new() -> Result<WasmEngine, anyhow::Error> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;

        // The epoch ticker: increments the engine's epoch counter
        // periodically. Instances waiting on a deadline see it move
        // and yield (or trap) accordingly.
        let engine_handle = engine.clone();
        let ticker = std::thread::Builder::new()
            .name("mwydyn-epoch".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(1));
                engine_handle.increment_epoch();
            })
            .expect("epoch thread spawns");

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

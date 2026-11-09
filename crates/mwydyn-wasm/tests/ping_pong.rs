//! M1 WASM perf: ping/pong with two wasmtime instances running a
//! decrement function through the Context pump. Measures the
//! framework overhead per WASM letter (fork + journal commit +
//! effects marshaling) vs. the raw wasmtime floor.

use mwydyn_core::{Actor, Context, Effect, Host, MsgCtx, Spawn};
use mwydyn_data::{decode, encode, Address, DecodeOptions, Letter, Value};
use wasmtime::{Engine, Instance, Module, Store, TypedFunc};

fn cbor_int(n: i64) -> Vec<u8> {
    encode(&Value::Int(n))
}

const DECREMENT_WAT: &str = r#"
    (module
        (func (export "process") (param i64) (result i64)
            local.get 0
            i64.const 1
            i64.sub
        )
    )
"#;

fn addr(context: &str, name: &str) -> Address {
    Address::Named { context: context.into(), name: name.into() }
}

/// A WASM bouncer: wraps a wasmtime Store + Instance + TypedFunc.
/// Stateless — `fork` creates a fresh instance from the same module
/// (the function is pure, so fork = fresh is equivalent).
struct WasmBouncer {
    engine: Engine,
    module: Module,
    store: Store<()>,
    process_fn: TypedFunc<i64, i64>,
    target: Address,
}

impl Clone for WasmBouncer {
    fn clone(&self) -> Self {
        let mut store = Store::new(&self.engine, ());
        let instance = Instance::new(&mut store, &self.module, &[]).unwrap();
        let process_fn = instance
            .get_typed_func::<i64, i64>(&mut store, "process")
            .unwrap();
        WasmBouncer {
            engine: self.engine.clone(),
            module: self.module.clone(),
            store,
            process_fn,
            target: self.target.clone(),
        }
    }
}

impl Actor for WasmBouncer {
    fn fork(&self) -> Box<dyn Actor> {
        Box::new(self.clone())
    }

    fn receive(&mut self, _ctx: &MsgCtx, _host: &Host, letter: &Letter) -> Vec<Effect> {
        // Round 33: the payload wall — payloads are canonical CBOR.
        let count = match decode(&letter.payload, &DecodeOptions::default()) {
            Ok(Value::Int(n)) => n,
            _ => return vec![],
        };
        let new_count = self
            .process_fn
            .call(&mut self.store, count)
            .unwrap_or_else(|e| panic!("WASM trap in process_fn: {e}"));
        vec![Effect::Post {
            to: self.target.clone(),
            letter: Letter::new("test.bounce@1", cbor_int(new_count), None, None, vec![])
                .unwrap(),
        }]
    }
}

#[test]
fn wasm_ping_pong_pipeline_throughput() {
    let engine = Engine::default();
    let module = Module::new(&engine, DECREMENT_WAT).unwrap();

    let mut ctx = Context::new("pp");
    let n = 5_000u64;
    let ping_addr = addr("pp", "ping");
    let pong_addr = addr("pp", "pong");

    // Spawn two WASM bouncers with the same decrement module.
    // They bounce the counter back and forth through the pump.
    let engine_for_ping = engine.clone();
    let module_for_ping = module.clone();
    let pong_for_ping = pong_addr.clone();

    ctx.spawn(
        Spawn::new(
            Some("ping"),
            Box::new(move || {
                let mut store = Store::new(&engine_for_ping, ());
                let instance = Instance::new(&mut store, &module_for_ping, &[]).unwrap();
                let func = instance
                    .get_typed_func::<i64, i64>(&mut store, "process")
                    .unwrap();
                Box::new(WasmBouncer {
                    engine: engine_for_ping.clone(),
                    module: module_for_ping.clone(),
                    store,
                    process_fn: func,
                    target: pong_for_ping.clone(),
                }) as Box<dyn Actor>
            }),
        )
        .supervision(mwydyn_core::Supervision::Restart),
    )
    .unwrap();

    let engine_for_pong = engine.clone();
    let module_for_pong = module.clone();
    let ping_for_pong = ping_addr.clone();

    ctx.spawn(
        Spawn::new(
            Some("pong"),
            Box::new(move || {
                let mut store = Store::new(&engine_for_pong, ());
                let instance = Instance::new(&mut store, &module_for_pong, &[]).unwrap();
                let func = instance
                    .get_typed_func::<i64, i64>(&mut store, "process")
                    .unwrap();
                Box::new(WasmBouncer {
                    engine: engine_for_pong.clone(),
                    module: module_for_pong.clone(),
                    store,
                    process_fn: func,
                    target: ping_for_pong.clone(),
                }) as Box<dyn Actor>
            }),
        )
        .supervision(mwydyn_core::Supervision::Restart),
    )
    .unwrap();

    // Post the initial letter to ping with the starting count.
    let kick = Letter::new(
        "test.bounce@1",
        cbor_int(n as i64),
        None,
        None,
        vec![],
    )
    .unwrap();
    ctx.post(&ping_addr, kick, vec![]);

    // Run the pump: the letters bounce ping → pong → ping → pong
    // until the counter reaches 0. Each bounce is one WASM call
    // + one journal commit. Measure the throughput.
    let start = std::time::Instant::now();
    ctx.run_until_idle(20_000);
    let elapsed = start.elapsed();

    let throughput = n as f64 / elapsed.as_secs_f64();
    eprintln!(
        "WASM ping/pong: {} bounces in {:?} → {:.0} bounces/s",
        n, elapsed, throughput
    );
    assert!(
        throughput > 1000.0,
        "WASM bouncing >1k/s on the RAM tier (got {:.0})",
        throughput
    );
}

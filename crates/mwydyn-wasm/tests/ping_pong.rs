//! WASM ping/pong perf: two WASM actors bouncing a counter,
//! measuring wasmtime call throughput.

use wasmtime::{Engine as WtEngine, Store as WtStore, Module as WtModule,
               Instance as WtInstance, TypedFunc};

#[test]
fn wasm_ping_pong_throughput() {
    use std::time::Instant;

    let engine = WtEngine::default();
    let wat = r#"
        (module
            (func (export "process") (param i64) (result i64)
                local.get 0
                i64.const 1
                i64.sub
            )
        )
    "#;
    let module = WtModule::new(&engine, wat).unwrap();

    let mut store_ping = WtStore::new(&engine, ());
    let inst_ping = WtInstance::new(&mut store_ping, &module, &[]).unwrap();
    let ping_fn: TypedFunc<i64, i64> = inst_ping.get_typed_func(&mut store_ping, "process").unwrap();

    let mut store_pong = WtStore::new(&engine, ());
    let inst_pong = WtInstance::new(&mut store_pong, &module, &[]).unwrap();
    let pong_fn: TypedFunc<i64, i64> = inst_pong.get_typed_func(&mut store_pong, "process").unwrap();

    let rounds = 10_000i64;
    let mut count = rounds;
    let start = Instant::now();
    while count > 0 {
        count = ping_fn.call(&mut store_ping, count).unwrap();
        count = pong_fn.call(&mut store_pong, count).unwrap();
    }
    let elapsed = start.elapsed();
    let total_calls = rounds * 2;
    let calls_per_sec = total_calls as f64 / elapsed.as_secs_f64();

    eprintln!(
        "WASM ping/pong: {} round trips in {:?} → {:.0} calls/s ({:.0} ns/call)",
        rounds, elapsed, calls_per_sec,
        elapsed.as_nanos() as f64 / total_calls as f64
    );
    assert_eq!(count, 0);
    assert!(calls_per_sec > 100_000.0, ">100k/s");
}

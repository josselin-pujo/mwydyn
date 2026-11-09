//! M1 perf baseline: raw wasmtime calls. The floor for WASM letters/s.
//! Everything the framework adds shows up as a delta against this.

use wasmtime::{Engine as WtEngine, Store as WtStore, Module as WtModule,
               Instance as WtInstance, TypedFunc};

#[test]
fn wasm_ping_pong_raw_floor() {
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
    let mut store_a = WtStore::new(&engine, ());
    let ia = WtInstance::new(&mut store_a, &module, &[]).unwrap();
    let pa: TypedFunc<i64, i64> = ia.get_typed_func(&mut store_a, "process").unwrap();
    let mut store_b = WtStore::new(&engine, ());
    let ib = WtInstance::new(&mut store_b, &module, &[]).unwrap();
    let pb: TypedFunc<i64, i64> = ib.get_typed_func(&mut store_b, "process").unwrap();

    let n = 10_000i64;
    let mut count = n;
    let start = std::time::Instant::now();
    while count > 0 {
        count = pa.call(&mut store_a, count).unwrap();
        count = pb.call(&mut store_b, count).unwrap();
    }
    let elapsed = start.elapsed();
    let calls_per_sec = (n * 2) as f64 / elapsed.as_secs_f64();
    eprintln!("WASM floor: {} round trips in {:?} → {:.0} calls/s ({:.0} ns/call)",
        n, elapsed, calls_per_sec, elapsed.as_nanos() as f64 / (n * 2) as f64);
    assert_eq!(count, 0);
    assert!(calls_per_sec > 100_000.0, "wasmtime calls should be >100k/s");
}

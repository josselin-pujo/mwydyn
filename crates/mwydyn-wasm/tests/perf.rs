//! M1 perf baseline: raw wasmtime calls. The floor for WASM letters/s.

use wasmtime::{Engine as WtEngine, Store as WtStore, Module as WtModule, Instance as WtInstance, TypedFunc};

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

/// The component-model test: load the compiled WASM guest component
/// and call its process function through wasmtime's component API.
/// This proves the WASM execution realm works end to end.
#[test]
fn component_model_instantiates_and_calls() {
    use wasmtime::component::{Component, Linker};
    use wasmtime::{Config, Engine as WtEngine, Store as WtStore};

    // Build the minimal guest component.
    let guest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-wasip2/release/mwydyn_guest_minimal.wasm");
    // Compile from WAT if the binary doesn't exist (first build).
    if !guest_path.exists() {
        panic!(
            "guest WASM not found at {}. Run: cargo build -p mwydyn-guest-minimal --target wasm32-wasip2 --release",
            guest_path.display()
        );
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = WtEngine::new(&config).unwrap();
    let component = Component::from_file(&engine, &guest_path).unwrap();

    let mut store = WtStore::new(&engine, ());
    let mut linker = Linker::new(&engine);
    // No imports needed for the minimal world.
    let instance = linker.instantiate(&mut store, &component).unwrap();

    // Get the process function: WIT s64 maps to Rust i64.
    let process = instance
        .get_func(&mut store, "process")
        .expect("process export");
    let process = process.typed::<(i64,), (i64,)>(&mut store).unwrap();

    // Call: process(42) → 41 (decrement).
    let (result,) = process.call(&mut store, (42,)).unwrap();
    assert_eq!(result, 41, "process(42) should return 41 (decrement)");

    // Call: process(0) → -1.
    let (result,) = process.call(&mut store, (0,)).unwrap();
    assert_eq!(result, -1);

    eprintln!("component-model: instantiation + calling works ✓");
}

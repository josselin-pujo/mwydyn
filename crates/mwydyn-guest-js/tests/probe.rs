//! UB-hunt probe (round 43 follow-up): drive the CURRENT release
//! artifact through decl + one process exchange; on trap, read the
//! exported panic buffer. Skip-if-absent.
use wasmtime::{Engine as WEngine, Instance, Linker, Memory, Module, Store};

fn find_artifact() -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/mwydyn-wasm/tests/artifacts/mwydyn_guest_js_echo.wasm");
    p.exists().then_some(p)
}

#[test]
fn probe_decl_and_exchange() {
    let Some(path) = find_artifact() else {
        eprintln!("SKIP: artifact missing — run scripts/build-guest-js.sh");
        return;
    };
    let wasm = std::fs::read(&path).unwrap();
    let engine = WEngine::default();
    let module = Module::new(&engine, &wasm).unwrap();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .func_wrap("mwydyn", "ambient", |_a: i32, _b: i32, _c: i32, _d: i32| -> i32 { -1 })
        .unwrap();
    let mut store = Store::new(&engine, ());
    store.set_epoch_deadline(1_000_000);
    let instance: Instance = linker.instantiate(&mut store, &module).unwrap();
    let memory = instance.get_memory(&mut store, "memory").unwrap();

    // dump the slot: address + first bytes
    let slot = instance.get_typed_func::<(), i32>(&mut store, "mwydyn_script_slot").unwrap()
        .call(&mut store, ()).unwrap();
    let m = memory.data(&store);
    println!("slot @ {slot}, bytes: {:?}", &m[slot as usize..slot as usize + 24]);

    let decl = instance.get_typed_func::<(), i32>(&mut store, "mwydyn_decl").unwrap();
    match decl.call(&mut store, ()) {
        Ok(v) => println!("decl = {v}"),
        Err(e) => {
            dump_frames(&e);
            read_panic(&mut store, &instance, memory);
            return;
        }
    }

    use mwydyn_data::{barrier::BarrierRequest, address_to_value, encode, Address, Letter, Value};
    let letter = Letter::new(
        "demo.kick@1",
        encode(&Value::map(vec![
            ("replyTo".into(), address_to_value(&Address::Instance("external".into()))),
            ("n".into(), Value::Int(7)),
        ]).unwrap()),
        None, None, vec![],
    ).unwrap();
    let req = encode(&BarrierRequest { from: Address::Instance("external".into()), letter }.to_value());
    let p = instance.get_typed_func::<i32, i32>(&mut store, "alloc").unwrap()
        .call(&mut store, req.len() as i32).unwrap();
    memory.write(&mut store, p as usize, &req).unwrap();
    let proc = instance.get_typed_func::<(i32, i32), i32>(&mut store, "process").unwrap();
    match proc.call(&mut store, (p, req.len() as i32)) {
        Ok(_) => println!("process ok"),
        Err(e) => {
            dump_frames(&e);
            read_panic(&mut store, &instance, memory);
        }
    }
}

fn dump_frames(e: &wasmtime::Error) {
    for line in format!("{e}").lines() {
        if line.contains("wasm!") {
            println!("{line}");
        }
    }
}

fn read_panic(store: &mut Store<()>, instance: &Instance, memory: Memory) {
    let Some(gl) = instance.get_global(&mut *store, "MWJS_PANIC_BUF") else {
        println!("(no panic global)");
        return;
    };
    let addr = gl.get(&mut *store).i32().unwrap() as usize;
    let mem = memory.data(&*store);
    if addr + 4 > mem.len() {
        println!("(panic buf out of range)");
        return;
    }
    let len = u32::from_le_bytes([mem[addr], mem[addr + 1], mem[addr + 2], mem[addr + 3]]) as usize;
    println!("PANIC: {}", String::from_utf8_lossy(&mem[addr + 4..addr + 4 + len]));
}

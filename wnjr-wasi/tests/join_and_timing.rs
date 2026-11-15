//! wasi-threads join_all and spawn timing tests.

use std::sync::{Arc, Mutex};
use std::time::Instant;
use wnjr_core::{Config, Engine, Instance, Value};
use wnjr_wasi::{ThreadRegistry, ThreadsConfig, WasiContextBuilder};

fn engine() -> Engine {
    Engine::new(Config::new()).unwrap()
}

/// Make a shared memory owned by a bootstrap module, return its handle.
fn make_shared_memory(min: u32, max: u32) -> wnjr_core::SharedMemHandle {
    let wat = format!(
        "(module (import \"test\" \"get\" (func $$get)) (memory (export \"mem\") {min} {max} shared) (func (export \"init\") (call $$get)))"
    );
    let slot: Arc<Mutex<Option<wnjr_core::SharedMemHandle>>> = Arc::new(Mutex::new(None));
    let slot_clone = Arc::clone(&slot);
    let get = wnjr_core::Import::func("test", "get", move |caller, _args, _results| {
        *slot_clone.lock().unwrap() = caller.shared_memory();
        Ok(())
    });
    let wasm = wat::parse_str(wat).expect("wat encodes");
    let mut inst = Instance::new(&engine(), &wasm, &[get]).unwrap();
    inst.invoke("init", &[]).unwrap();
    let handle = slot.lock().unwrap().take().expect("shared memory handle");
    drop(inst);
    handle
}

#[test]
fn join_all_waits_for_normal_returns() {
    let wat = r#"(module
        (import "env" "memory" (memory 1 1 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32) (result i32)))
        (func (export "spawn") (param i32) (result i32) (call $spawn (local.get 0)))
        (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
            ;; Mark this tid as alive at offset tid*4
            local.get $tid
            i32.const 2
            i32.shl
            i32.const 42
            i32.atomic.store)
        (func (export "check") (param i32) (result i32)
            local.get 0
            i32.const 2
            i32.shl
            i32.atomic.load))"#;
    let bytes = Arc::new(wat::parse_str(wat).expect("wat encodes"));
    let handle = make_shared_memory(1, 1);
    let registry = Arc::new(ThreadRegistry::with_exit_hook(|_| {}));
    let cfg = ThreadsConfig::new(engine(), Arc::clone(&bytes), handle.clone())
        .unwrap()
        .with_context(WasiContextBuilder::new())
        .with_registry(Arc::clone(&registry));
    let mem = wnjr_core::Import::shared_memory("env", "memory", handle);
    let mut main = Instance::new(
        &engine(),
        &bytes,
        &[mem, wnjr_wasi::thread_spawn_import(&cfg)],
    )
    .unwrap();

    for tid in 1..=4 {
        let res = main.invoke("spawn", &[Value::I32(tid)]).unwrap();
        assert!(
            matches!(res[0], Value::I32(t) if t > 0),
            "spawn returned {res:?}"
        );
    }

    let errors = registry.join_all();
    assert_eq!(errors, Vec::<String>::new(), "workers failed");

    for tid in 1..=4 {
        let val = main.invoke("check", &[Value::I32(tid)]).unwrap()[0];
        assert_eq!(val, Value::I32(42), "tid {tid} did not mark itself");
    }
}

#[test]
fn spawn_timing_with_module_cache() {
    let wat = r#"(module
        (import "env" "memory" (memory 1 1 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32) (result i32)))
        (func (export "spawn") (param i32) (result i32) (call $spawn (local.get 0)))
        (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
            ;; Mark arrival at offset 4
            i32.const 4
            i32.const 1
            i32.atomic.store)
        (func (export "wait_arrival") (result i32)
            (block $done
                (loop $again
                    i32.const 4
                    i32.atomic.load
                    br_if $done
                    br $again))
            i32.const 4
            i32.atomic.load))"#;
    let bytes = Arc::new(wat::parse_str(wat).expect("wat encodes"));
    let handle = make_shared_memory(1, 1);
    let cfg = ThreadsConfig::new(engine(), Arc::clone(&bytes), handle.clone()).unwrap();
    let mem = wnjr_core::Import::shared_memory("env", "memory", handle);
    let mut main = Instance::new(
        &engine(),
        &bytes,
        &[mem, wnjr_wasi::thread_spawn_import(&cfg)],
    )
    .unwrap();

    let start = Instant::now();
    main.invoke("spawn", &[Value::I32(1)]).unwrap();
    main.invoke("wait_arrival", &[]).unwrap();
    let elapsed = start.elapsed();
    eprintln!("Spawn to wasi_thread_start first instruction: {elapsed:?}");
    // Measured <1ms on arm64/x86; the 5s ceiling catches structural hangs only.
    assert!(
        elapsed.as_secs() < 5,
        "spawn took {elapsed:?}, ceiling 5s (measured ~600µs)"
    );
}

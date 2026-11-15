//! wasi-threads validation and behavior tests.
//!
//! Tests that need unimplemented threads features are #[ignore]d and tagged
//! with the th step they wait on. Remove the ignore once that step lands.
//!
//! What can be asserted today is asserted today: shared memory is rejected
//! at parse, and the 0xFE atomics prefix is rejected at decode.

use wnjr_core::{Config, Engine, Import, Instance, Module, Value, WasmError};

// Compile-time assertions: Module must be Send + Sync, Instance must NOT be Send.
fn _assert_send_sync<T: Send + Sync>() {}

/// Ambiguity trick: if `T: Send`, both impls apply and `<T as AmbiguousIfSend<_>>`
/// fails to infer, so a Send type here is a compile error.
trait AmbiguousIfSend<A> {
    fn some_item() {}
}
impl<T: ?Sized> AmbiguousIfSend<()> for T {}
#[allow(dead_code)]
struct IsSend;
impl<T: ?Sized + Send> AmbiguousIfSend<IsSend> for T {}

#[test]
fn module_is_send_sync_instance_is_not_send() {
    _assert_send_sync::<Module>();
    let _ = <Instance as AmbiguousIfSend<_>>::some_item;
}

fn engine() -> Engine {
    Engine::new(Config::new()).unwrap()
}

fn instantiate(wat: &str) -> Result<Instance, WasmError> {
    let wasm = wat::parse_str(wat).expect("wat encodes");
    Instance::new(&engine(), &wasm, &[])
}

// ---------------------------------------------------------------------------
// th3: parser accepts shared memory (flag 0x03) and requires max.
// ---------------------------------------------------------------------------

#[test]
fn shared_memory_without_max_is_rejected() {
    // Spec requires a shared memory to declare a maximum.
    let wat = "(module (memory (export \"mem\") 1 shared))";
    assert!(
        instantiate(wat).is_err(),
        "shared memory without max must be rejected"
    );
}

#[test]
fn shared_memory_with_max_is_accepted() {
    let wat = "(module (memory (export \"mem\") 1 10 shared))";
    assert!(
        instantiate(wat).is_ok(),
        "shared memory with max should instantiate"
    );
}

// ---------------------------------------------------------------------------
// th5: 0xFE atomics decode, natural-alignment validation, misalignment trap.
// ---------------------------------------------------------------------------

const ATOMIC_CASES: &[(&str, &str)] = &[
    ("i32 load", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.load (i32.const 0))))"),
    ("i64 load", "(module (memory 1 1 shared) (func (export \"f\") (result i64) (i64.atomic.load (i32.const 0))))"),
    ("i32 store", "(module (memory 1 1 shared) (func (export \"f\") (i32.atomic.store (i32.const 0) (i32.const 1))))"),
    ("i32 rmw add", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.rmw.add (i32.const 0) (i32.const 1))))"),
    ("i32 rmw sub", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.rmw.sub (i32.const 0) (i32.const 1))))"),
    ("i32 rmw and", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.rmw.and (i32.const 0) (i32.const 1))))"),
    ("i32 rmw or", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.rmw.or (i32.const 0) (i32.const 1))))"),
    ("i32 rmw xor", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.rmw.xor (i32.const 0) (i32.const 1))))"),
    ("i32 rmw xchg", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.rmw.xchg (i32.const 0) (i32.const 1))))"),
    ("i32 cmpxchg", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.rmw.cmpxchg (i32.const 0) (i32.const 0) (i32.const 1))))"),
    ("i64 cmpxchg", "(module (memory 1 1 shared) (func (export \"f\") (result i64) (i64.atomic.rmw.cmpxchg (i32.const 0) (i64.const 0) (i64.const 1))))"),
    ("fence", "(module (memory 1 1 shared) (func (export \"f\") atomic.fence))"),
    ("wait32", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (memory.atomic.wait32 (i32.const 0) (i32.const 1) (i64.const 0))))"),
    ("notify", "(module (memory 1 1 shared) (func (export \"f\") (result i32) (memory.atomic.notify (i32.const 0) (i32.const 1))))"),
];

#[test]
fn atomics_decode_and_run() {
    for (name, wat) in ATOMIC_CASES {
        let mut inst =
            instantiate(wat).unwrap_or_else(|e| panic!("{name} failed to instantiate: {e}"));
        inst.invoke("f", &[])
            .unwrap_or_else(|e| panic!("{name} trapped: {e}"));
    }
}

// (op, init_val, operand, expected_old, expected_new)
const RMW_CASES: &[(&str, &str, i32, i32, i32, i32)] = &[
    ("i32", "add", 12, 5, 12, 17),
    ("i32", "sub", 12, 5, 12, 7),
    ("i32", "and", 12, 10, 12, 8),
    ("i32", "or", 12, 3, 12, 15),
    ("i32", "xor", 12, 10, 12, 6),
    ("i32", "xchg", 12, 99, 12, 99),
    ("i64", "add", 100, 20, 100, 120),
    ("i64", "sub", 100, 30, 100, 70),
];

#[test]
fn atomic_rmw_values_are_correct() {
    for &(ty, op, init, operand, expect_old, expect_new) in RMW_CASES {
        let wat = format!(
            "(module
              (memory 1 1 shared)
              (func (export \"init\") (param {ty})
                ({ty}.atomic.store (i32.const 0) (local.get 0)))
              (func (export \"f\") (param {ty}) (result {ty})
                ({ty}.atomic.rmw.{op} (i32.const 0) (local.get 0)))
              (func (export \"get\") (result {ty})
                ({ty}.atomic.load (i32.const 0))))"
        );
        let mut inst = instantiate(&wat)
            .unwrap_or_else(|e| panic!("{ty} rmw {op} failed to instantiate: {e}"));

        let init_val = if ty == "i32" {
            Value::I32(init)
        } else {
            Value::I64(init as i64)
        };
        let operand_val = if ty == "i32" {
            Value::I32(operand)
        } else {
            Value::I64(operand as i64)
        };
        inst.invoke("init", &[init_val]).unwrap();
        let old_val = inst.invoke("f", &[operand_val]).unwrap()[0];
        let new_val = inst.invoke("get", &[]).unwrap()[0];

        let (old_i32, new_i32) = if ty == "i32" {
            let Value::I32(o) = old_val else {
                panic!("expected I32")
            };
            let Value::I32(n) = new_val else {
                panic!("expected I32")
            };
            (o, n)
        } else {
            let Value::I64(o) = old_val else {
                panic!("expected I64")
            };
            let Value::I64(n) = new_val else {
                panic!("expected I64")
            };
            (o as i32, n as i32)
        };

        assert_eq!(old_i32, expect_old, "{ty} rmw {op}: wrong old value");
        assert_eq!(new_i32, expect_new, "{ty} rmw {op}: wrong new value");
    }
}

#[test]
fn atomic_store_and_load_simple() {
    let wat = "(module (memory 1 1 shared) \
              (func (export \"init\") (param i32) (i32.atomic.store (i32.const 0) (local.get 0))) \
              (func (export \"get\") (result i32) (i32.atomic.load (i32.const 0))))";
    let mut inst = instantiate(wat).unwrap();
    inst.invoke("init", &[Value::I32(42)]).unwrap();
    let result = inst.invoke("get", &[]).unwrap()[0];
    let Value::I32(val) = result else {
        panic!("expected I32")
    };
    assert_eq!(
        val, 42,
        "atomic store then load should return the same value"
    );
}

#[test]
fn atomic_add_check_old_value() {
    let wat = "(module (memory 1 1 shared) \
              (func (export \"init\") (param i32) (i32.atomic.store (i32.const 0) (local.get 0))) \
              (func (export \"add\") (param i32) (result i32) (i32.atomic.rmw.add (i32.const 0) (local.get 0))) \
              (func (export \"get\") (result i32) (i32.atomic.load (i32.const 0))))";
    let mut inst = instantiate(wat).unwrap();
    inst.invoke("init", &[Value::I32(12)]).unwrap();
    let old_val = inst.invoke("add", &[Value::I32(5)]).unwrap()[0];
    let new_val = inst.invoke("get", &[]).unwrap()[0];
    let Value::I32(o) = old_val else {
        panic!("expected I32")
    };
    let Value::I32(n) = new_val else {
        panic!("expected I32")
    };
    eprintln!("atomic add: old={o}, new={n}");
    assert_eq!(o, 12, "add should return old value 12");
    assert_eq!(n, 17, "after add, memory should be 17");
}

const CMPXCHG_CASES: &[(&str, i32, i32, i32, i32, i32)] = &[
    // (ty, mem_init, expected, replacement, result_old, mem_final)
    ("i32", 12, 12, 99, 12, 99), // match: swap happens
    ("i32", 12, 99, 7, 12, 12),  // mismatch: no swap
    ("i64", 100, 100, 999, 100, 999),
    ("i64", 100, 999, 77, 100, 100),
];

#[test]
fn atomic_cmpxchg_semantics() {
    for &(ty, init, expected, replacement, result_old, mem_final) in CMPXCHG_CASES {
        let wat = format!(
            "(module
              (memory 1 1 shared)
              (func (export \"init\") (param {ty})
                ({ty}.atomic.store (i32.const 0) (local.get 0)))
              (func (export \"f\") (param {ty} {ty}) (result {ty})
                ({ty}.atomic.rmw.cmpxchg (i32.const 0) (local.get 0) (local.get 1)))
              (func (export \"get\") (result {ty})
                ({ty}.atomic.load (i32.const 0))))"
        );
        let mut inst =
            instantiate(&wat).unwrap_or_else(|e| panic!("{ty} cmpxchg failed to instantiate: {e}"));

        if ty == "i32" {
            inst.invoke("init", &[Value::I32(init)]).unwrap();
            let result = inst
                .invoke("f", &[Value::I32(expected), Value::I32(replacement)])
                .unwrap()[0];
            let mem = inst.invoke("get", &[]).unwrap()[0];
            let Value::I32(r) = result else {
                panic!("expected I32")
            };
            let Value::I32(m) = mem else {
                panic!("expected I32")
            };
            assert_eq!(r, result_old, "{ty} cmpxchg: wrong return value");
            assert_eq!(m, mem_final, "{ty} cmpxchg: wrong final memory value");
        } else {
            inst.invoke("init", &[Value::I64(init as i64)]).unwrap();
            let result = inst
                .invoke(
                    "f",
                    &[Value::I64(expected as i64), Value::I64(replacement as i64)],
                )
                .unwrap()[0];
            let mem = inst.invoke("get", &[]).unwrap()[0];
            let Value::I64(r) = result else {
                panic!("expected I64")
            };
            let Value::I64(m) = mem else {
                panic!("expected I64")
            };
            assert_eq!(r, result_old as i64, "{ty} cmpxchg: wrong return value");
            assert_eq!(
                m, mem_final as i64,
                "{ty} cmpxchg: wrong final memory value"
            );
        }
    }
}

#[test]
fn misaligned_atomic_access_traps() {
    // i32 atomic at offset 1 is not naturally aligned.
    let wat = "(module (memory 1 1 shared) (func (export \"f\") (result i32) (i32.atomic.load (i32.const 1))))";
    let mut inst = instantiate(wat).expect("module");
    assert!(
        inst.invoke("f", &[]).is_err(),
        "unaligned atomic load must trap"
    );
}

#[test]
fn atomic_fence_compiles() {
    let wat = "(module (memory 1 1 shared) (func (export \"f\") atomic.fence))";
    let mut inst = instantiate(wat).expect("fence instantiation");
    inst.invoke("f", &[]).expect("fence execution");
}

#[test]
fn atomic_ops_are_valid_on_unshared_memory() {
    // The threads proposal only gives atomics extra meaning on a shared
    // memory; it does not make them invalid elsewhere, and the spec's own
    // suite says so outright ("unshared memory is OK" in
    // proposals/threads/atomic.wast, every atomic op included).
    let wat =
        "(module (memory 1 1) (func (export \"f\") (result i32) (i32.atomic.load (i32.const 0))))";
    let mut inst = instantiate(wat).expect("atomic on an unshared memory is a valid module");
    let value = inst.invoke("f", &[]).expect("the load runs");
    assert_eq!(
        value,
        vec![Value::I32(0)],
        "an unshared atomic load still reads memory"
    );
}

// ---------------------------------------------------------------------------
// th7: wait/notify. Single-thread cases need no spawn: a wait whose expected
// value does not match returns 1 immediately, a timed-out wait returns 2, and
// notify with nobody waiting returns 0.
// ---------------------------------------------------------------------------

#[test]
fn wait32_value_mismatch_returns_one() {
    let wat = "(module (memory 1 1 shared) (func (export \"f\") (result i32) (memory.atomic.wait32 (i32.const 0) (i32.const 7) (i64.const -1))))";
    let mut inst = instantiate(wat).expect("module");
    let v = inst.invoke("f", &[]).expect("call");
    assert_eq!(
        v,
        vec![Value::I32(1)],
        "expected value differs, must not block"
    );
}

#[test]
fn wait32_timeout_returns_two() {
    let wat = "(module (memory 1 1 shared) (func (export \"f\") (result i32) (memory.atomic.wait32 (i32.const 0) (i32.const 0) (i64.const 1_000_000))))";
    let mut inst = instantiate(wat).expect("module");
    let v = inst.invoke("f", &[]).expect("call");
    assert_eq!(
        v,
        vec![Value::I32(2)],
        "1ms timeout with a matching value returns 2"
    );
}

#[test]
fn wait64_timeout_returns_two() {
    let wat = "(module (memory 1 1 shared) (func (export \"f\") (result i32) (memory.atomic.wait64 (i32.const 0) (i64.const 0) (i64.const 1_000_000))))";
    let mut inst = instantiate(wat).expect("module");
    let v = inst.invoke("f", &[]).expect("call");
    assert_eq!(v, vec![Value::I32(2)]);
}

#[test]
fn notify_with_no_waiters_returns_zero() {
    let wat = "(module (memory 1 1 shared) (func (export \"f\") (result i32) (memory.atomic.notify (i32.const 0) (i32.const 10))))";
    let mut inst = instantiate(wat).expect("module");
    let v = inst.invoke("f", &[]).expect("call");
    assert_eq!(v, vec![Value::I32(0)], "nobody waiting, woken count is 0");
}

#[test]
fn wait_unaligned_traps() {
    let wat = "(module (memory 1 1 shared) (func (export \"f\") (result i32) (memory.atomic.wait32 (i32.const 2) (i32.const 0) (i64.const 0))))";
    let mut inst = instantiate(wat).expect("module");
    assert!(
        inst.invoke("f", &[]).is_err(),
        "wait32 at offset 2 is unaligned"
    );
}

// ---------------------------------------------------------------------------
// th9: wasi_thread_spawn. The host import is what the guest calls; the
// runtime is expected to instantiate the same module again against the same
// shared memory and call wasi_thread_start(tid, arg). See docs/THREADS.md.
// ---------------------------------------------------------------------------

/// The real wasi_thread_spawn path: when the guest calls `wasi.thread-spawn`,
/// re-instantiate the *same* module bytes on a fresh OS thread, importing the
/// *same* shared memory, and call `wasi_thread_start(tid, arg)`. The join
/// handles are collected so the caller can wait for completion deterministically.
fn thread_spawn_import(
    engine: Engine,
    module_bytes: std::sync::Arc<std::vec::Vec<u8>>,
    handle: wnjr_core::SharedMemHandle,
    joins: std::sync::Arc<std::sync::Mutex<std::vec::Vec<std::thread::JoinHandle<()>>>>,
) -> Import {
    Import::func("wasi", "thread-spawn", move |_caller, args, results| {
        let [Value::I32(tid), Value::I32(arg)] = args else {
            return Err(WasmError::invalid("thread-spawn takes (i32, i32)"));
        };
        let (tid, arg) = (*tid, *arg);
        let engine = engine;
        let bytes = std::sync::Arc::clone(&module_bytes);
        let h = handle.clone();
        let jh = std::thread::spawn(move || {
            // The worker imports the same shared memory. It gets a no-op
            // thread-spawn: nested spawning is out of scope for this test and
            // the worker never calls it.
            let mem = Import::shared_memory("env", "memory", h);
            let noop = Import::func("wasi", "thread-spawn", |_c, _a, r: &mut [Value]| {
                r[0] = Value::I32(-1);
                Ok(())
            });
            let mut worker =
                Instance::new(&engine, &bytes, &[mem, noop]).expect("worker instantiation");
            worker
                .invoke("wasi_thread_start", &[Value::I32(tid), Value::I32(arg)])
                .expect("wasi_thread_start");
        });
        joins.lock().unwrap().push(jh);
        results[0] = Value::I32(tid);
        Ok(())
    })
}

/// Instantiate a bootstrap module that owns a shared memory and hand its
/// SharedMemHandle back to the caller.
fn make_shared_memory(min: u32, max: u32) -> wnjr_core::SharedMemHandle {
    use std::sync::{Arc, Mutex};
    let boot_wat = format!(
        "(module (import \"test\" \"get\" (func $get)) (memory (export \"mem\") {min} {max} shared) (func (export \"init\") (call $get)))"
    );
    let slot: Arc<Mutex<Option<wnjr_core::SharedMemHandle>>> = Arc::new(Mutex::new(None));
    let slot_cb = Arc::clone(&slot);
    let get = Import::func("test", "get", move |c, _a, _r| {
        *slot_cb.lock().unwrap() = c.shared_memory();
        Ok(())
    });
    let boot = wat::parse_str(&boot_wat).expect("boot wat encodes");
    let mut boot_inst = Instance::new(&engine(), &boot, &[get]).unwrap();
    boot_inst.invoke("init", &[]).unwrap();
    let handle = slot.lock().unwrap().take();
    handle.expect("bootstrap has shared memory")
}

/// The owner instance writes 42 into its shared memory; a second instance on
/// another OS thread imports that same memory through SharedMemHandle and
/// must read 42.
#[test]
fn spawn_and_join() {
    use std::sync::{Arc, Mutex};
    use std::thread;

    let owner_wat = r#"(module
                  (import "test" "get_handle" (func $get_handle))
                  (memory (export "mem") 1 10 shared)
                  (func (export "init") (call $get_handle))
                  (func (export "write") (param i32) (i32.atomic.store (i32.const 0) (local.get 0))))"#;
    let worker_wat = r#"(module
                  (import "env" "memory" (memory 1 10 shared))
                  (func (export "read") (result i32) (i32.atomic.load (i32.const 0))))"#;

    let slot: Arc<Mutex<Option<wnjr_core::SharedMemHandle>>> = Arc::new(Mutex::new(None));
    let slot_in_cb = Arc::clone(&slot);
    let get_handle = Import::func("test", "get_handle", move |caller, _args, _results| {
        *slot_in_cb.lock().unwrap() = caller.shared_memory();
        Ok(())
    });

    let owner_wasm = wat::parse_str(owner_wat).expect("wat encodes");
    let mut owner = Instance::new(&engine(), &owner_wasm, &[get_handle]).unwrap();
    owner.invoke("init", &[]).unwrap();
    owner.invoke("write", &[Value::I32(42)]).unwrap();

    let handle = slot
        .lock()
        .unwrap()
        .take()
        .expect("caller.shared_memory() returns Some for a shared memory");

    let worker_wasm = wat::parse_str(worker_wat).expect("wat encodes");
    let val = thread::spawn(move || {
        let import = Import::shared_memory("env", "memory", handle);
        let mut worker = Instance::new(&engine(), &worker_wasm, &[import]).unwrap();
        match worker.invoke("read", &[]).unwrap()[0] {
            Value::I32(v) => v,
            ref other => panic!("expected I32, got {other:?}"),
        }
    })
    .join()
    .unwrap();

    assert_eq!(val, 42, "thread sees the same shared memory");
}

/// Stress atomics with competing increments from multiple threads. The sum
/// should match the number of increments if atomicity holds.
#[test]
fn atomic_counter_race() {
    // Simplified: single-instance repeated atomic add (multi-thread would need
    // shared memory export/import API). Verifies atomic.rmw.add correctness.
    let wat = "(module \
                  (memory 1 1 shared) \
                  (func (export \"add\") (drop (i32.atomic.rmw.add (i32.const 0) (i32.const 1)))) \
                  (func (export \"get\") (result i32) (i32.atomic.load (i32.const 0))))";

    let mut inst = instantiate(wat).unwrap();
    let n = 40;
    for _ in 0..n {
        inst.invoke("add", &[]).unwrap();
    }

    let result = inst.invoke("get", &[]).unwrap()[0];
    let Value::I32(val) = result else {
        panic!("expected I32")
    };
    assert_eq!(val, n, "atomic counter should reach N after N adds");
}

// ---------------------------------------------------------------------------
// th4 + th8: shared memory identity and per-thread state. These need the
// runtime to hand the same shared memory to a second instance, which no
// public API exposes yet. The assertions describe the contract.
// ---------------------------------------------------------------------------

/// Each spawned thread gets its own Instance with independent mutable globals.
/// Two separate instances have independent globals; incrementing in one does not
/// affect the other. This is what makes __stack_pointer and TLS work.
#[test]
fn globals_are_per_instance() {
    // Two separate instances (not spawned threads yet) verify independence.
    let wat = "(module \
                  (memory 1 1 shared) \
                  (global (mut i32) (i32.const 0)) \
                  (func (export \"inc\") (global.set 0 (i32.add (global.get 0) (i32.const 1)))) \
                  (func (export \"get\") (result i32) (global.get 0)))";

    let mut inst_a = instantiate(wat).unwrap();
    let mut inst_b = instantiate(wat).unwrap();

    inst_a.invoke("inc", &[]).unwrap();
    inst_b.invoke("inc", &[]).unwrap();
    inst_b.invoke("inc", &[]).unwrap();

    let a_val = inst_a.invoke("get", &[]).unwrap()[0];
    let b_val = inst_b.invoke("get", &[]).unwrap()[0];
    let Value::I32(a) = a_val else {
        panic!("expected I32")
    };
    let Value::I32(b) = b_val else {
        panic!("expected I32")
    };

    assert_eq!(a, 1, "instance A has its own global");
    assert_eq!(b, 2, "instance B has its own global");
}

/// Spinlock built from cmpxchg: 3 threads started through wasi.thread-spawn
/// plus the main thread contend for one lock and increment a shared counter
/// under it, 10000 times each. A non-atomic load/add/store inside the lock
/// only gives 40000 if the lock really excludes.
#[test]
fn spinlock_with_cmpxchg() {
    use std::sync::{Arc, Mutex};

    const WORKERS: i32 = 3;
    const ITERS: i32 = 10_000;

    let wat = r#"(module
        (import "env" "memory" (memory 1 10 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32 i32) (result i32)))
        (func (export "spawn") (param $tid i32) (param $iters i32) (result i32)
            (call $spawn (local.get $tid) (local.get $iters)))
        (func (export "count") (result i32) (i32.atomic.load (i32.const 4)))
        (func (export "wasi_thread_start") (param $tid i32) (param $iters i32)
            (local $i i32)
            (loop $outer
                (block $locked
                    (loop $spin
                        (br_if $locked
                            (i32.eqz (i32.atomic.rmw.cmpxchg (i32.const 0) (i32.const 0) (i32.const 1))))
                        (br $spin)))
                (i32.store (i32.const 4) (i32.add (i32.load (i32.const 4)) (i32.const 1)))
                (i32.atomic.store (i32.const 0) (i32.const 0))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br_if $outer (i32.lt_u (local.get $i) (local.get $iters))))))"#;

    let module_bytes = Arc::new(wat::parse_str(wat).expect("wat encodes"));
    let handle = make_shared_memory(1, 10);
    let joins: Arc<Mutex<std::vec::Vec<std::thread::JoinHandle<()>>>> =
        Arc::new(Mutex::new(std::vec::Vec::new()));
    let import_spawn = thread_spawn_import(
        engine(),
        Arc::clone(&module_bytes),
        handle.clone(),
        Arc::clone(&joins),
    );
    let import_mem = Import::shared_memory("env", "memory", handle);
    let mut main_inst =
        Instance::new(&engine(), &module_bytes, &[import_mem, import_spawn]).unwrap();

    for tid in 1..=WORKERS {
        let r = main_inst
            .invoke("spawn", &[Value::I32(tid), Value::I32(ITERS)])
            .unwrap();
        assert_eq!(r[0], Value::I32(tid), "thread-spawn returns the tid");
    }
    main_inst
        .invoke("wasi_thread_start", &[Value::I32(0), Value::I32(ITERS)])
        .unwrap();

    let handles: std::vec::Vec<_> = joins.lock().unwrap().drain(..).collect();
    assert_eq!(handles.len(), WORKERS as usize, "one OS thread per spawn");
    for jh in handles {
        jh.join().expect("worker thread did not panic");
    }

    let count = main_inst.invoke("count", &[]).unwrap()[0];
    assert_eq!(
        count,
        Value::I32((WORKERS + 1) * ITERS),
        "spinlock must not lose increments"
    );
}

/// th7.3: Cross-thread blocking. A worker thread calls wait32 on address 0,
/// expecting value 0, with a large timeout (10s). Main thread sleeps 10ms,
/// then stores 1 to unlock the mismatch, then notifies. Worker should return 0
/// (notified), not 2 (timeout). Verifies real blocking and real notify wakeup.
#[test]
fn cross_thread_wait_notify_wakeup() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let wat = r#"(module
        (import "env" "memory" (memory 1 10 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32 i32) (result i32)))
        (func (export "spawn") (param $tid i32) (param $arg i32) (result i32)
            (call $spawn (local.get $tid) (local.get $arg)))
        (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
            ;; Worker: wait32(addr=0, expected=0, timeout=10s). Returns result to addr=8.
            (i32.store (i32.const 8)
                (memory.atomic.wait32 (i32.const 0) (i32.const 0) (i64.const 10_000_000_000))))
        (func (export "notify") (param $addr i32) (param $count i32) (result i32)
            (memory.atomic.notify (local.get $addr) (local.get $count)))
        (func (export "store_unlock") (i32.atomic.store (i32.const 0) (i32.const 1)))
        (func (export "read_result") (result i32) (i32.atomic.load (i32.const 8))))"#;

    let module_bytes = Arc::new(wat::parse_str(wat).expect("wat encodes"));
    let handle = make_shared_memory(1, 10);
    let joins: Arc<Mutex<std::vec::Vec<std::thread::JoinHandle<()>>>> =
        Arc::new(Mutex::new(std::vec::Vec::new()));
    let import_spawn = thread_spawn_import(
        engine(),
        Arc::clone(&module_bytes),
        handle.clone(),
        Arc::clone(&joins),
    );
    let import_mem = Import::shared_memory("env", "memory", handle);
    let mut main_inst =
        Instance::new(&engine(), &module_bytes, &[import_mem, import_spawn]).unwrap();

    // Spawn worker which will wait32 on addr=0 for 10s
    let r = main_inst
        .invoke("spawn", &[Value::I32(1), Value::I32(0)])
        .unwrap();
    assert_eq!(r[0], Value::I32(1), "thread-spawn returns tid");

    // Give worker time to enter wait
    std::thread::sleep(Duration::from_millis(50));

    // Main thread: store 1 to addr=0 (unlock the mismatch), then notify
    main_inst.invoke("store_unlock", &[]).unwrap();
    let woken = main_inst
        .invoke("notify", &[Value::I32(0), Value::I32(1)])
        .unwrap()[0];
    assert_eq!(woken, Value::I32(1), "notify should wake 1 waiter");

    // Join worker
    let handles: std::vec::Vec<_> = joins.lock().unwrap().drain(..).collect();
    assert_eq!(handles.len(), 1, "one OS thread spawned");
    handles
        .into_iter()
        .next()
        .unwrap()
        .join()
        .expect("worker did not panic");

    // Worker should have written 0 (notified) to addr=8, not 2 (timeout)
    let result = main_inst.invoke("read_result", &[]).unwrap()[0];
    assert_eq!(result, Value::I32(0), "worker was notified, not timed out");
}

/// Producer-consumer queue with wait/notify. Producer writes to shared memory
/// and notifies. Consumer waits, reads, and verifies the message.
#[test]
fn producer_consumer_with_wait_notify() {
    use std::sync::{Arc, Mutex};

    let wat = r#"(module
        (import "env" "memory" (memory 1 10 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32 i32) (result i32)))
        (func (export "spawn") (param $tid i32) (param $arg i32) (result i32)
            (call $spawn (local.get $tid) (local.get $arg)))
        (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
            ;; Consumer: wait32(addr=0, expected=0, timeout=5s)
            (drop (memory.atomic.wait32 (i32.const 0) (i32.const 0) (i64.const 5_000_000_000)))
            ;; Read message from addr=4
            (i32.store (i32.const 8) (i32.atomic.load (i32.const 4))))
        (func (export "produce") (param $msg i32)
            ;; Store message to addr=4
            (i32.atomic.store (i32.const 4) (local.get $msg))
            ;; Store 1 to addr=0 to unlock wait
            (i32.atomic.store (i32.const 0) (i32.const 1))
            ;; notify(addr=0, count=1)
            (drop (memory.atomic.notify (i32.const 0) (i32.const 1))))
        (func (export "read_result") (result i32) (i32.atomic.load (i32.const 8))))"#;

    let module_bytes = Arc::new(wat::parse_str(wat).expect("wat encodes"));
    let handle = make_shared_memory(1, 10);
    let joins: Arc<Mutex<std::vec::Vec<std::thread::JoinHandle<()>>>> =
        Arc::new(Mutex::new(std::vec::Vec::new()));
    let import_spawn = thread_spawn_import(
        engine(),
        Arc::clone(&module_bytes),
        handle.clone(),
        Arc::clone(&joins),
    );
    let import_mem = Import::shared_memory("env", "memory", handle);
    let mut main_inst =
        Instance::new(&engine(), &module_bytes, &[import_mem, import_spawn]).unwrap();

    // Spawn consumer
    main_inst
        .invoke("spawn", &[Value::I32(1), Value::I32(0)])
        .unwrap();

    // Give consumer time to enter wait
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Producer: write message 0xDEAD_BEEF and notify
    main_inst
        .invoke("produce", &[Value::I32(0xDEAD_BEEFu32 as i32)])
        .unwrap();

    // Join consumer
    let handles: std::vec::Vec<_> = joins.lock().unwrap().drain(..).collect();
    assert_eq!(handles.len(), 1);
    handles
        .into_iter()
        .next()
        .unwrap()
        .join()
        .expect("consumer did not panic");

    // Verify message
    let result = main_inst.invoke("read_result", &[]).unwrap()[0];
    assert_eq!(
        result,
        Value::I32(0xDEAD_BEEFu32 as i32),
        "consumer received the message"
    );
}

/// wait32 with a timeout wakes multiple threads. Verify the returned count
/// matches the number of threads woken.
#[test]
fn multiple_waiters_notified() {
    use std::sync::{Arc, Mutex};

    const N: i32 = 4;
    let wat = r#"(module
        (import "env" "memory" (memory 1 10 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32 i32) (result i32)))
        (func (export "spawn") (param $tid i32) (param $arg i32) (result i32)
            (call $spawn (local.get $tid) (local.get $arg)))
        (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
            ;; Wait on addr=0, expecting 0, infinite timeout
            (drop (memory.atomic.wait32 (i32.const 0) (i32.const 0) (i64.const -1)))
            ;; Write marker: addr=4 + (tid * 4)
            (i32.atomic.store
                (i32.add (i32.const 4) (i32.shl (local.get $tid) (i32.const 2)))
                (local.get $tid)))
        (func (export "notify") (param $count i32) (result i32)
            (memory.atomic.notify (i32.const 0) (local.get $count)))
        (func (export "read_marker") (param $tid i32) (result i32)
            (i32.atomic.load (i32.add (i32.const 4) (i32.shl (local.get $tid) (i32.const 2))))))"#;

    let module_bytes = Arc::new(wat::parse_str(wat).expect("wat encodes"));
    let handle = make_shared_memory(1, 10);
    let joins: Arc<Mutex<std::vec::Vec<std::thread::JoinHandle<()>>>> =
        Arc::new(Mutex::new(std::vec::Vec::new()));
    let import_spawn = thread_spawn_import(
        engine(),
        Arc::clone(&module_bytes),
        handle.clone(),
        Arc::clone(&joins),
    );
    let import_mem = Import::shared_memory("env", "memory", handle);
    let mut main_inst =
        Instance::new(&engine(), &module_bytes, &[import_mem, import_spawn]).unwrap();

    // Spawn N waiters
    for tid in 1..=N {
        main_inst
            .invoke("spawn", &[Value::I32(tid), Value::I32(0)])
            .unwrap();
    }

    // Give waiters time to all enter wait
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Notify all N waiters
    let woken = main_inst.invoke("notify", &[Value::I32(N)]).unwrap()[0];
    assert_eq!(woken, Value::I32(N), "should wake all N waiters");

    // Join all
    let handles: std::vec::Vec<_> = joins.lock().unwrap().drain(..).collect();
    assert_eq!(handles.len(), N as usize);
    for jh in handles {
        jh.join().expect("waiter did not panic");
    }

    // Verify all markers written
    for tid in 1..=N {
        let marker = main_inst.invoke("read_marker", &[Value::I32(tid)]).unwrap()[0];
        assert_eq!(marker, Value::I32(tid), "waiter {tid} wrote its marker");
    }
}

/// memory.grow on a shared memory is visible to all threads. A thread that
/// grows the memory updates the length atomically. Other threads observe the
/// new size and can access the newly grown pages. Also verifies that wait/notify
/// still works after grow (base pointer remains stable).
#[test]
fn grow_is_visible_to_other_threads() {
    use std::sync::{Arc, Mutex};

    let wat = r#"(module
        (import "env" "memory" (memory 1 10 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32 i32) (result i32)))
        (func (export "spawn") (param $tid i32) (param $arg i32) (result i32)
            (call $spawn (local.get $tid) (local.get $arg)))
        (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
            ;; Announce readiness, then wait until main publishes flag=1 at addr 0.
            ;; wait32 returns 1 if the flag already changed, 0 if notified; the
            ;; loop makes either order correct, so no sleep is needed.
            (i32.atomic.store (i32.const 12) (i32.const 1))
            (block $done
                (loop $again
                    (br_if $done (i32.atomic.load (i32.const 0)))
                    (drop (memory.atomic.wait32 (i32.const 0) (i32.const 0) (i64.const 10_000_000_000)))
                    (br $again)))
            ;; This instance never refreshed its view: its own memory.size must
            ;; still see the grow done by the other thread.
            (i32.atomic.store (i32.const 8) (memory.size))
            ;; Load from the grown page (offset 65536), store result at addr 4.
            (i32.atomic.store (i32.const 4) (i32.atomic.load (i32.const 65536))))
        (func (export "worker_ready") (result i32) (i32.atomic.load (i32.const 12)))
        (func (export "worker_size") (result i32) (i32.atomic.load (i32.const 8)))
        (func (export "grow_and_write") (param $delta i32) (result i32)
            (local $old i32)
            (local.set $old (memory.grow (local.get $delta)))
            ;; Write 0x1234 to first word of new page (65536 if grew from 1→2)
            (i32.atomic.store (i32.const 65536) (i32.const 0x1234))
            ;; Unlock wait at addr=0
            (i32.atomic.store (i32.const 0) (i32.const 1))
            ;; Notify waiter
            (drop (memory.atomic.notify (i32.const 0) (i32.const 1)))
            (local.get $old))
        (func (export "size") (result i32) (memory.size))
        (func (export "read_result") (result i32) (i32.atomic.load (i32.const 4))))"#;

    let module_bytes = Arc::new(wat::parse_str(wat).expect("wat encodes"));
    let handle = make_shared_memory(1, 10);
    let joins: Arc<Mutex<std::vec::Vec<std::thread::JoinHandle<()>>>> =
        Arc::new(Mutex::new(std::vec::Vec::new()));
    let import_spawn = thread_spawn_import(
        engine(),
        Arc::clone(&module_bytes),
        handle.clone(),
        Arc::clone(&joins),
    );
    let import_mem = Import::shared_memory("env", "memory", handle);
    let mut main_inst =
        Instance::new(&engine(), &module_bytes, &[import_mem, import_spawn]).unwrap();

    // Spawn worker which will wait then access page 1
    main_inst
        .invoke("spawn", &[Value::I32(1), Value::I32(0)])
        .unwrap();

    // Handshake: wait until the worker started (it is about to wait or
    // already waiting). Correctness does not depend on which.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while main_inst.invoke("worker_ready", &[]).unwrap()[0] != Value::I32(1) {
        assert!(std::time::Instant::now() < deadline, "worker never started");
        std::thread::yield_now();
    }

    // Main: grow from 1 to 2, write to page 1, notify
    let old_size = main_inst
        .invoke("grow_and_write", &[Value::I32(1)])
        .unwrap()[0];
    assert_eq!(old_size, Value::I32(1), "old size was 1 page");
    let new_size = main_inst.invoke("size", &[]).unwrap()[0];
    assert_eq!(new_size, Value::I32(2), "new size is 2 pages");

    // Join worker
    let handles: std::vec::Vec<_> = joins.lock().unwrap().drain(..).collect();
    assert_eq!(handles.len(), 1);
    handles
        .into_iter()
        .next()
        .unwrap()
        .join()
        .expect("worker did not panic");

    // The worker's own memory.size saw 2 pages.
    let worker_size = main_inst.invoke("worker_size", &[]).unwrap()[0];
    assert_eq!(
        worker_size,
        Value::I32(2),
        "worker memory.size sees the other thread's grow"
    );

    // Worker should have read 0x1234 from the grown page
    let result = main_inst.invoke("read_result", &[]).unwrap()[0];
    assert_eq!(
        result,
        Value::I32(0x1234),
        "worker accessed grown page and was notified after grow"
    );
}

/// A trap in one thread's wasi_thread_start does not kill the spawner or any
/// other thread. Two workers trap, one with \`unreachable\` and one with an
/// out-of-bounds load (guard-page fault, delivered as a signal on that worker's
/// OS thread). A third worker is blocked in wait32 the whole time; after both
/// traps it is woken, runs 1000 atomic increments, and returns normally. The
/// spawner's own instance keeps working on the same shared memory afterwards.
#[test]
fn trap_in_one_thread_does_not_kill_another() {
    use std::sync::{Arc, Mutex};
    type Joins = Arc<Mutex<std::vec::Vec<(i32, std::thread::JoinHandle<Result<(), String>>)>>>;

    let wat = r#"(module
        (import "env" "memory" (memory 1 1 shared))
        (import "wasi" "thread-spawn" (func $spawn (param i32 i32) (result i32)))
        (func (export "spawn") (param $tid i32) (param $arg i32) (result i32)
            (call $spawn (local.get $tid) (local.get $arg)))
        (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
            (if (i32.eq (local.get $arg) (i32.const 1)) (then unreachable))
            (if (i32.eq (local.get $arg) (i32.const 2))
                (then (drop (i32.load (i32.const 0xfffffff0)))))
            ;; arg 0: survivor. Mark ready, block until main sets flag at 0.
            (i32.atomic.store (i32.const 12) (i32.const 1))
            (block $go
                (loop $again
                    (br_if $go (i32.atomic.load (i32.const 0)))
                    (drop (memory.atomic.wait32 (i32.const 0) (i32.const 0) (i64.const 10_000_000_000)))
                    (br $again)))
            (block $done
                (loop $inc
                    (drop (i32.atomic.rmw.add (i32.const 4) (i32.const 1)))
                    (br_if $done (i32.ge_u (i32.atomic.load (i32.const 4)) (i32.const 1000)))
                    (br $inc))))
        (func (export "ready") (result i32) (i32.atomic.load (i32.const 12)))
        (func (export "release")
            (i32.atomic.store (i32.const 0) (i32.const 1))
            (drop (memory.atomic.notify (i32.const 0) (i32.const 1))))
        (func (export "counter") (result i32) (i32.atomic.load (i32.const 4)))
        (func (export "bump") (result i32) (i32.atomic.rmw.add (i32.const 4) (i32.const 1))))"#;

    let bytes = Arc::new(wat::parse_str(wat).expect("wat encodes"));
    let handle = make_shared_memory(1, 1);
    let joins: Joins = Arc::new(Mutex::new(std::vec::Vec::new()));

    // Like thread_spawn_import, but the worker's result travels back through
    // the join handle instead of panicking, so a trap is observable per thread.
    let spawn = {
        let (bytes, handle, joins) = (Arc::clone(&bytes), handle.clone(), Arc::clone(&joins));
        Import::func("wasi", "thread-spawn", move |_caller, args, results| {
            let [Value::I32(tid), Value::I32(arg)] = args else {
                return Err(WasmError::invalid("thread-spawn takes (i32, i32)"));
            };
            let (tid, arg) = (*tid, *arg);
            let (bytes, h) = (Arc::clone(&bytes), handle.clone());
            let jh = std::thread::spawn(move || {
                let mem = Import::shared_memory("env", "memory", h);
                let noop = Import::func("wasi", "thread-spawn", |_c, _a, r: &mut [Value]| {
                    r[0] = Value::I32(-1);
                    Ok(())
                });
                let mut worker = Instance::new(&engine(), &bytes, &[mem, noop])
                    .map_err(|e| format!("instantiate: {e:?}"))?;
                worker
                    .invoke("wasi_thread_start", &[Value::I32(tid), Value::I32(arg)])
                    .map(|_| ())
                    .map_err(|e| format!("{e:?}"))
            });
            joins.lock().unwrap().push((tid, jh));
            results[0] = Value::I32(tid);
            Ok(())
        })
    };
    let mem = Import::shared_memory("env", "memory", handle);
    let mut main_inst = Instance::new(&engine(), &bytes, &[mem, spawn]).unwrap();

    // Survivor first, and wait until it is parked (or about to park).
    main_inst
        .invoke("spawn", &[Value::I32(1), Value::I32(0)])
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while main_inst.invoke("ready", &[]).unwrap()[0] != Value::I32(1) {
        assert!(
            std::time::Instant::now() < deadline,
            "survivor never started"
        );
        std::thread::yield_now();
    }

    // Two trapping workers, joined while the survivor is still blocked.
    main_inst
        .invoke("spawn", &[Value::I32(2), Value::I32(1)])
        .unwrap();
    main_inst
        .invoke("spawn", &[Value::I32(3), Value::I32(2)])
        .unwrap();
    let mut by_tid: std::collections::BTreeMap<i32, _> = joins.lock().unwrap().drain(..).collect();
    assert_eq!(by_tid.len(), 3);
    let unreachable = by_tid
        .remove(&2)
        .unwrap()
        .join()
        .expect("tid 2 OS thread survived");
    let oob = by_tid
        .remove(&3)
        .unwrap()
        .join()
        .expect("tid 3 OS thread survived");
    let unreachable = unreachable.expect_err("tid 2 must trap");
    let oob = oob.expect_err("tid 3 must trap");
    assert!(
        unreachable.contains("unreachable"),
        "tid 2 trap kind: {unreachable}"
    );
    assert!(
        oob.contains("out of bounds memory access"),
        "tid 3 trap kind: {oob}"
    );

    // Nothing the traps did touched the counter, and the survivor is still parked.
    assert_eq!(main_inst.invoke("counter", &[]).unwrap()[0], Value::I32(0));

    // Wake the survivor; it must run to completion.
    main_inst.invoke("release", &[]).unwrap();
    let survivor = by_tid
        .remove(&1)
        .unwrap()
        .join()
        .expect("tid 1 OS thread survived");
    assert_eq!(survivor, Ok(()), "survivor returned normally");
    assert_eq!(
        main_inst.invoke("counter", &[]).unwrap()[0],
        Value::I32(1000)
    );

    // The spawner's instance still works on the same memory.
    assert_eq!(main_inst.invoke("bump", &[]).unwrap()[0], Value::I32(1000));
    assert_eq!(
        main_inst.invoke("counter", &[]).unwrap()[0],
        Value::I32(1001)
    );
}

/// Shared memory identity: two instances of the same module, both importing
/// the same exported shared memory, observe each other's stores.
#[test]
fn shared_memory_is_the_same_bytes_across_instances() {
    // Owner writes a byte; worker imports the same SharedMemHandle and reads it.
    use std::sync::{Arc, Mutex};

    let owner_wat = r#"(module
                  (import "test" "get_handle" (func $get_handle))
                  (memory (export "mem") 1 10 shared)
                  (func (export "init") (call $get_handle))
                  (func (export "write_byte") (param i32 i32) (i32.store8 (local.get 0) (local.get 1))))"#;
    let worker_wat = r#"(module
                  (import "env" "memory" (memory 1 10 shared))
                  (func (export "read_byte") (param i32) (result i32) (i32.load8_u (local.get 0))))"#;

    let slot: Arc<Mutex<Option<wnjr_core::SharedMemHandle>>> = Arc::new(Mutex::new(None));
    let slot_clone = Arc::clone(&slot);
    let get_handle = Import::func("test", "get_handle", move |caller, _args, _results| {
        *slot_clone.lock().unwrap() = caller.shared_memory();
        Ok(())
    });

    let owner_wasm = wat::parse_str(owner_wat).expect("wat encodes");
    let mut owner = Instance::new(&engine(), &owner_wasm, &[get_handle]).unwrap();
    owner.invoke("init", &[]).unwrap();
    owner
        .invoke("write_byte", &[Value::I32(8), Value::I32(0xAB)])
        .unwrap();

    let handle = slot
        .lock()
        .unwrap()
        .take()
        .expect("owner has shared memory");

    let worker_wasm = wat::parse_str(worker_wat).expect("wat encodes");
    let import = Import::shared_memory("env", "memory", handle);
    let mut worker = Instance::new(&engine(), &worker_wasm, &[import]).unwrap();
    let result = worker.invoke("read_byte", &[Value::I32(8)]).unwrap()[0];
    let Value::I32(val) = result else {
        panic!("expected I32")
    };

    assert_eq!(val, 0xAB, "worker reads the same byte the owner wrote");
}

// ---------------------------------------------------------------------------
// Sub-width atomics must touch exactly the bytes they name.
//
// x86_64 lowers the 32- and 64-bit forms with xadd, xchg and a cmpxchg loop,
// and every one of those carries its operand in a full general register.
// Reusing them for an 8- or 16-bit access writes the neighbouring bytes too;
// the spec suite reported it as a wrong i64.atomic.load. These assertions name
// the property directly instead of going through that file.
// ---------------------------------------------------------------------------

const SUBWIDTH_MODULE: &str = r#"
(module
  (memory (export "mem") 1 1 shared)
  (func (export "init") (i64.atomic.store (i32.const 0) (i64.const 0x0123456789abcdef)))
  (func (export "load") (result i64) (i64.atomic.load (i32.const 0)))
  (func (export "store8") (i32.atomic.store8 (i32.const 1) (i32.const 0x42)))
  (func (export "store16") (i32.atomic.store16 (i32.const 4) (i32.const 0x8844)))
  (func (export "rmw8_add") (result i32)
    (i32.atomic.rmw8.add_u (i32.const 1) (i32.const 1)))
  (func (export "rmw16_and") (result i32)
    (i32.atomic.rmw16.and_u (i32.const 4) (i32.const 0xff00)))
  (func (export "cmpxchg8") (result i32)
    (i32.atomic.rmw8.cmpxchg_u (i32.const 1) (i32.const 0x42) (i32.const 0x77))))
"#;

fn load64(inst: &mut Instance) -> u64 {
    let value = inst.invoke("load", &[]).expect("load")[0];
    let Value::I64(raw) = value else {
        panic!("expected I64 from load");
    };
    raw as u64
}

fn i32_result(inst: &mut Instance, name: &str) -> i32 {
    let value = inst.invoke(name, &[]).expect("invoke")[0];
    let Value::I32(raw) = value else {
        panic!("expected I32 from {name}");
    };
    raw
}

#[test]
fn atomic_store_subwidth_leaves_neighbouring_bytes_alone() {
    let mut inst = instantiate(SUBWIDTH_MODULE).expect("module instantiates");
    inst.invoke("init", &[]).expect("init");
    inst.invoke("store8", &[]).expect("store8");
    assert_eq!(
        load64(&mut inst),
        0x0123_4567_89ab_42ef,
        "store8 at offset 1 must replace exactly that byte"
    );
    inst.invoke("store16", &[]).expect("store16");
    assert_eq!(
        load64(&mut inst),
        0x0123_8844_89ab_42ef,
        "store16 at offset 4 must replace exactly those two bytes"
    );
}

#[test]
fn atomic_rmw_subwidth_reads_and_writes_only_its_own_field() {
    let mut inst = instantiate(SUBWIDTH_MODULE).expect("module instantiates");
    inst.invoke("init", &[]).expect("init");
    assert_eq!(
        i32_result(&mut inst, "rmw8_add"),
        0xcd,
        "rmw8 returns the old byte"
    );
    assert_eq!(
        i32_result(&mut inst, "rmw16_and"),
        0x4567,
        "rmw16 returns the old halfword"
    );
    assert_eq!(
        load64(&mut inst),
        0x0123_4500_89ab_ceef,
        "rmw8/rmw16 must write only their own field"
    );
}

#[test]
fn atomic_cmpxchg_subwidth_compares_and_stores_only_its_own_field() {
    let mut inst = instantiate(SUBWIDTH_MODULE).expect("module instantiates");
    inst.invoke("init", &[]).expect("init");
    inst.invoke("store8", &[]).expect("store8");
    assert_eq!(
        i32_result(&mut inst, "cmpxchg8"),
        0x42,
        "cmpxchg8 returns the byte it found"
    );
    assert_eq!(
        load64(&mut inst),
        0x0123_4567_89ab_77ef,
        "cmpxchg8 writes only the compared byte"
    );
}

//! wasi-threads: the production 'wasi' 'thread-spawn' path, with real threads.
//!
//! Every module here imports its memory with the shared flag, exactly like a
//! wasi-sdk build linked with --import-memory --shared-memory, and every
//! assertion is on a value the guest computed with real atomics across a real
//! OS thread. Nothing here waits for a fixed number of milliseconds.

use std::sync::{Arc, Mutex};

use wnjr_core::{Engine, Instance, WasmError};
use wnjr_wasi::{
    memory_imports_for, wasi_imports, wasi_imports_with_config, ThreadRegistry, ThreadsConfig,
    WasiContextBuilder,
};

/// Shared module prologue: the shared memory the host provides, thread-spawn,
/// proc_exit, and a bounded "wait until this flag is set" helper.
///
/// The helper is bounded on purpose: a single-writer flag needs at most one
/// wait, so more than a few wakeups without progress means something is wrong
/// and the guest traps instead of spinning forever.
const PROLOGUE: &str = r#"
  (memory (export "memory") (import "foo" "bar") 1 1 shared)
  (func $spawn (import "wasi" "thread-spawn") (param i32) (result i32))
  (func $proc_exit (import "wasi_snapshot_preview1" "proc_exit") (param i32))
  (func $await_flag (param $addr i32) (local $tries i32)
    (block $done
      (loop $again
        local.get $addr
        i32.atomic.load
        br_if $done
        local.get $tries
        i32.const 1
        i32.add
        local.set $tries
        local.get $tries
        i32.const 64
        i32.gt_u
        if
          unreachable
        end
        local.get $addr
        i32.const 0
        i64.const 5_000_000_000
        memory.atomic.wait32
        drop
        br $again
      )
    )
  )
"#;

struct Harness {
    instance: Instance,
    config: ThreadsConfig,
    exits: Arc<Mutex<Vec<i32>>>,
}

fn harness(wat: &str) -> Harness {
    let bytes = Arc::new(wat::parse_str(wat).expect("the test module must encode"));
    let engine = Engine::with_defaults();
    let (mut imports, shared) = memory_imports_for(&bytes).expect("the module imports a memory");
    let shared = shared.expect("the module must import its memory as shared");

    let exits = Arc::new(Mutex::new(Vec::new()));
    let hook = {
        let exits = exits.clone();
        move |code: i32| exits.lock().expect("exits").push(code)
    };
    let registry = Arc::new(ThreadRegistry::with_exit_hook(hook));
    let config = ThreadsConfig::new(engine.clone(), bytes.clone(), shared)
        .expect("the module is a wasi-threads module")
        .with_registry(registry);
    imports.extend(wasi_imports_with_config(
        WasiContextBuilder::new().build(),
        Some(config.clone()),
    ));
    let instance = Instance::new(&engine, &bytes, &imports).expect("instantiation failed");
    Harness {
        instance,
        config,
        exits,
    }
}

fn exit_code(result: Result<impl core::fmt::Debug, WasmError>) -> Option<i32> {
    match result {
        Ok(value) => panic!("expected proc_exit, the guest returned {value:?}"),
        Err(error) => {
            assert!(
                error.is_exit(),
                "expected proc_exit, got {error:?} ({})",
                error.message()
            );
            error.exit_code()
        }
    }
}

#[test]
fn thread_spawn_runs_the_worker_and_hands_it_tid_and_arg() {
    let wat = format!(
        r#"(module
{PROLOGUE}
  (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
    i32.const 16
    local.get $tid
    i32.atomic.store
    i32.const 20
    local.get $arg
    i32.atomic.store
    i32.const 24
    i32.const 1
    i32.atomic.store
    i32.const 24
    i32.const 1
    memory.atomic.notify
    drop)
  (func (export "_start") (local $tid i32)
    ;; 4660 travels through the start_arg slot
    i32.const 4660
    call $spawn
    local.set $tid
    ;; thread-spawn must return a tid >= 1
    local.get $tid
    i32.const 1
    i32.lt_s
    if
      i32.const 2
      call $proc_exit
      unreachable
    end
    i32.const 24
    call $await_flag
    ;; the worker recorded the tid it was started with: it must be ours
    i32.const 16
    i32.atomic.load
    local.get $tid
    i32.ne
    if
      i32.const 3
      call $proc_exit
      unreachable
    end
    ;; and the start_arg we passed
    i32.const 20
    i32.atomic.load
    i32.const 4660
    i32.ne
    if
      i32.const 4
      call $proc_exit
      unreachable
    end
    i32.const 0
    call $proc_exit
    unreachable)
)"#
    );
    let mut harness = harness(&wat);
    let code = exit_code(harness.instance.invoke("_start", &[]));
    assert_eq!(code, Some(0), "the guest's own checks reported a failure");
    assert_eq!(
        harness.config.registry().join_all(),
        Vec::<String>::new(),
        "the worker reported no error"
    );
    assert!(
        harness.exits.lock().unwrap().is_empty(),
        "no worker called proc_exit"
    );
}

#[test]
fn two_threads_get_distinct_tids_and_both_run() {
    let wat = format!(
        r#"(module
{PROLOGUE}
  (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
    ;; record this worker's tid in the slot of its start_arg
    local.get $arg
    i32.const 64
    i32.add
    local.get $tid
    i32.atomic.store
    ;; publish: slot 0 is worker one, slot 4 is worker two
    local.get $arg
    i32.const 1
    i32.atomic.store
    local.get $arg
    i32.const 1
    memory.atomic.notify
    drop)
  (func (export "_start") (local $one i32) (local $two i32)
    i32.const 0
    call $spawn
    local.set $one
    i32.const 4
    call $spawn
    local.set $two
    ;; both tids are valid
    local.get $one
    i32.const 1
    i32.lt_s
    local.get $two
    i32.const 1
    i32.lt_s
    i32.or
    if
      i32.const 2
      call $proc_exit
      unreachable
    end
    ;; and distinct
    local.get $one
    local.get $two
    i32.eq
    if
      i32.const 3
      call $proc_exit
      unreachable
    end
    i32.const 0
    call $await_flag
    i32.const 4
    call $await_flag
    ;; each worker recorded the tid it was handed, in the slot of its start_arg
    i32.const 64
    i32.atomic.load
    local.get $one
    i32.ne
    if
      i32.const 4
      call $proc_exit
      unreachable
    end
    i32.const 68
    i32.atomic.load
    local.get $two
    i32.ne
    if
      i32.const 5
      call $proc_exit
      unreachable
    end
    i32.const 0
    call $proc_exit
    unreachable)
)"#
    );
    let mut harness = harness(&wat);
    assert_eq!(
        exit_code(harness.instance.invoke("_start", &[])),
        Some(0),
        "both workers ran with distinct tids"
    );
    assert_eq!(harness.config.registry().join_all(), Vec::<String>::new());
}

#[test]
fn a_trapping_worker_is_reported_to_the_host() {
    let wat = format!(
        r#"(module
{PROLOGUE}
  (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
    i32.const 24
    i32.const 1
    i32.atomic.store
    i32.const 24
    i32.const 1
    memory.atomic.notify
    drop
    unreachable)
  (func (export "_start")
    i32.const 0
    call $spawn
    i32.const 1
    i32.lt_s
    if
      i32.const 2
      call $proc_exit
      unreachable
    end
    ;; the worker publishes the flag and then traps
    i32.const 24
    call $await_flag)
)"#
    );
    let mut harness = harness(&wat);
    // Returning from _start is fine here: what matters is that the worker's
    // failure does not vanish.
    let returned = harness.instance.invoke("_start", &[]);
    assert!(
        returned.is_ok(),
        "the guest's own start returned {returned:?}"
    );
    let errors = harness.config.registry().join_all();
    assert_eq!(errors.len(), 1, "expected exactly one worker failure");
    assert!(
        errors[0].contains("unreachable"),
        "the host must be able to see what the worker hit, got {:?}",
        errors[0]
    );
    assert!(
        errors[0].contains("thread 1"),
        "the failure must name the thread, got {:?}",
        errors[0]
    );
}

#[test]
fn proc_exit_from_a_worker_reaches_the_exit_hook() {
    let wat = format!(
        r#"(module
{PROLOGUE}
  (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
    i32.const 24
    i32.const 1
    i32.atomic.store
    i32.const 24
    i32.const 1
    memory.atomic.notify
    drop
    i32.const 7
    call $proc_exit
    unreachable)
  (func (export "_start")
    i32.const 0
    call $spawn
    i32.const 1
    i32.lt_s
    if
      i32.const 2
      call $proc_exit
      unreachable
    end
    i32.const 24
    call $await_flag)
)"#
    );
    let mut harness = harness(&wat);
    harness.instance.invoke("_start", &[]).unwrap();
    harness.config.registry().join_all();
    assert_eq!(
        *harness.exits.lock().unwrap(),
        vec![7],
        "the worker's proc_exit(7) must reach the host's exit hook"
    );
}

#[test]
fn threads_config_rejects_a_module_that_does_not_import_its_memory() {
    let bytes = Arc::new(
        wat::parse_str(
            r#"(module
  (memory 1 1)
  (func (export "wasi_thread_start") (param i32 i32))
  (func (export "_start")))"#,
        )
        .unwrap(),
    );
    let shared = wnjr_core::SharedMemHandle::new(
        wnjr_core::Limits::new_shared(1, 1).expect("shared limits"),
    )
    .expect("shared memory");
    let error = match ThreadsConfig::new(Engine::with_defaults(), bytes, shared) {
        Ok(_) => panic!("a module that defines its own memory cannot be shared"),
        Err(error) => error,
    };
    assert!(
        error.message().contains("import its shared memory"),
        "unexpected message: {}",
        error.message()
    );
}

#[test]
fn threads_config_rejects_a_module_without_the_start_export() {
    let bytes = Arc::new(
        wat::parse_str(
            r#"(module
  (memory (export "memory") (import "foo" "bar") 1 1 shared)
  (func (export "_start")))"#,
        )
        .unwrap(),
    );
    let shared = wnjr_core::SharedMemHandle::new(
        wnjr_core::Limits::new_shared(1, 1).expect("shared limits"),
    )
    .expect("shared memory");
    let error = match ThreadsConfig::new(Engine::with_defaults(), bytes, shared) {
        Ok(_) => panic!("no wasi_thread_start means no thread can start"),
        Err(error) => error,
    };
    assert!(
        error.message().contains("wasi_thread_start"),
        "unexpected message: {}",
        error.message()
    );
}

#[test]
fn the_production_import_table_carries_thread_spawn_only_with_threads() {
    let bytes = Arc::new(
        wat::parse_str(&format!(
            r#"(module
{PROLOGUE}
  (func (export "wasi_thread_start") (param i32 i32))
  (func (export "_start")))"#
        ))
        .unwrap(),
    );
    let (_, shared) = memory_imports_for(&bytes).unwrap();
    let config = ThreadsConfig::new(
        Engine::with_defaults(),
        bytes,
        shared.expect("shared memory import"),
    )
    .expect("a wasi-threads module");

    let sparse = wasi_imports(WasiContextBuilder::new().build());
    assert!(
        !sparse
            .iter()
            .any(|import| import.module() == "wasi" && import.name() == "thread-spawn"),
        "thread-spawn must not appear without threads"
    );

    let threaded = wasi_imports_with_config(WasiContextBuilder::new().build(), Some(config));
    assert!(
        threaded
            .iter()
            .any(|import| import.module() == "wasi" && import.name() == "thread-spawn"),
        "the production import table must expose wasi.thread-spawn"
    );
    // The preview1 surface is still there next to it.
    assert!(
        threaded
            .iter()
            .any(|import| import.module() == "wasi_snapshot_preview1"
                && import.name() == "proc_exit"),
        "the threaded table must still be a full WASI table"
    );
}

#[test]
fn imported_memories_reports_the_declared_limits() {
    let bytes = wat::parse_str(
        r#"(module
  (memory (import "env" "memory") 3 7)
  (memory (import "foo" "bar") 1 1 shared))"#,
    )
    .unwrap();
    let memories = wnjr_wasi::imported_memories(&bytes).expect("import section");
    assert_eq!(memories.len(), 2, "both imported memories are reported");
    assert_eq!(memories[0].module, "env");
    assert_eq!(memories[0].min, 3);
    assert_eq!(memories[0].max, Some(7));
    assert!(!memories[0].shared);
    assert_eq!(memories[1].module, "foo");
    assert_eq!(memories[1].name, "bar");
    assert!(memories[1].shared);
}

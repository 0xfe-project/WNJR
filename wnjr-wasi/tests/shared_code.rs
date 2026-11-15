//! wasi-threads workers reuse the code the main instance compiled.
//!
//! The main instance and ThreadsConfig hold one Arc<Module>, as the CLI does.
//! Every round spawns four workers, joins them, and checks the shared
//! counter. The compile counter is process-wide, so this file has one test.

use std::sync::Arc;

use wnjr_core::{jit_stats_snapshot, Engine, Instance, Module, Value};
use wnjr_wasi::{memory_imports_for, wasi_imports_with_config, ThreadsConfig, WasiContextBuilder};

const WAT: &str = r#"(module
  (memory (export "memory") (import "foo" "bar") 1 1 shared)
  (func $spawn (import "wasi" "thread-spawn") (param i32) (result i32))
  (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
    i32.const 0
    i32.const 1
    i32.atomic.rmw.add
    drop)
  (func (export "spawn4") (result i32)
    (local $i i32) (local $failed i32)
    (loop $again
      i32.const 0
      call $spawn
      i32.const 1
      i32.lt_s
      local.get $failed
      i32.or
      local.set $failed
      local.get $i
      i32.const 1
      i32.add
      local.tee $i
      i32.const 4
      i32.lt_u
      br_if $again)
    local.get $failed)
  (func (export "count") (result i32) i32.const 0 i32.atomic.load)
)"#;

#[test]
fn workers_compile_nothing_across_twenty_spawn_rounds() {
    let bytes = wat::parse_str(WAT).unwrap();
    let engine = Engine::with_defaults();
    let module = Arc::new(Module::new("workers", &bytes).unwrap());
    let (mut imports, shared) = memory_imports_for(&bytes).unwrap();
    let config =
        ThreadsConfig::from_module(engine.clone(), Arc::clone(&module), &bytes, shared.unwrap())
            .unwrap();
    imports.extend(wasi_imports_with_config(
        WasiContextBuilder::new().build(),
        Some(config.clone()),
    ));

    let before_main = jit_stats_snapshot().compiles;
    let mut instance = Instance::from_module(&engine, (*module).clone(), &imports).unwrap();
    assert_eq!(
        jit_stats_snapshot().compiles - before_main,
        1,
        "the main instance compiles once"
    );

    let before_workers = jit_stats_snapshot().compiles;
    for round in 1..=20 {
        assert_eq!(
            instance.invoke("spawn4", &[]).unwrap(),
            [Value::I32(0)],
            "round {round}: every spawn returns a tid >= 1"
        );
        let errors = config.registry().join_all();
        assert!(errors.is_empty(), "round {round}: {errors:?}");
        assert_eq!(
            instance.invoke("count", &[]).unwrap(),
            [Value::I32(4 * round)],
            "round {round}"
        );
    }
    assert_eq!(
        jit_stats_snapshot().compiles - before_workers,
        0,
        "80 worker instantiations must not compile"
    );
}

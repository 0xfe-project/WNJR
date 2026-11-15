//! Shared native code across instances of one parsed module.
//!
//! Module clones share one code slot. An instance whose compile made no
//! import-dependent decision publishes its code there, and a later instance
//! with the same memory facts installs it without compiling. The counter is
//! process-wide, so every test here holds LOCK while it measures.

use std::sync::Mutex;

use wnjr_core::{jit_stats_snapshot, Config, Engine, Import, Module, RuntimeWorld, Tier, Value};

static LOCK: Mutex<()> = Mutex::new(());

fn compiles() -> usize {
    jit_stats_snapshot().compiles
}

/// Host-only imports, like the wasi path: nothing to inline, no tags.
const HOST_ONLY: &str = r#"(module
  (import "env" "add" (func $add (param i32 i32) (result i32)))
  (memory 1)
  (global $g (mut i32) (i32.const 0))
  (func $sq (param i32) (result i32) local.get 0 local.get 0 i32.mul)
  (func (export "run") (param i32) (result i32)
    global.get $g
    i32.const 1
    i32.add
    global.set $g
    local.get 0
    call $sq
    global.get $g
    call $add)
)"#;

fn host_add() -> Import {
    Import::func("env", "add", |_caller, args, results| {
        let (Value::I32(a), Value::I32(b)) = (args[0], args[1]) else {
            panic!("add takes two i32");
        };
        results[0] = Value::I32(a + b);
        Ok(())
    })
}

#[test]
fn second_instance_of_a_host_only_module_compiles_nothing() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let wasm = wat::parse_str(HOST_ONLY).unwrap();
    for &tier in Tier::ALL {
        let engine = Engine::new(Config::new().tier(tier)).unwrap();
        let module = Module::new("host-only", &wasm).unwrap();
        let second = module.clone();
        let imports = [host_add()];
        let mut world = RuntimeWorld::new();

        let before_first = compiles();
        let a = world.instantiate(&engine, module, &imports).unwrap();
        let first_compiles = compiles() - before_first;

        let before_second = compiles();
        let b = world.instantiate(&engine, second, &imports).unwrap();
        let second_compiles = compiles() - before_second;

        assert_eq!(
            first_compiles, 1,
            "{tier:?}: the first instance compiles once"
        );
        assert_eq!(
            second_compiles, 0,
            "{tier:?}: the second instance must reuse the code"
        );

        // Globals stay per instance even though the code is shared.
        assert_eq!(
            world.invoke(a, "run", &[Value::I32(5)]).unwrap(),
            [Value::I32(26)]
        );
        assert_eq!(
            world.invoke(a, "run", &[Value::I32(5)]).unwrap(),
            [Value::I32(27)]
        );
        assert_eq!(
            world.invoke(b, "run", &[Value::I32(3)]).unwrap(),
            [Value::I32(10)]
        );
    }
}

/// The provider's export is a throw wrapper the decoder inlines into a
/// caller in another instance: cross_instance_inlined.
const PROVIDER: &str = r#"(module
  (tag $t (export "t") (param i32))
  (func (export "thrower") (param i32) local.get 0 throw $t)
)"#;

const CONSUMER: &str = r#"(module
  (import "p" "t" (tag $t (param i32)))
  (import "p" "thrower" (func $thrower (param i32)))
  (func (export "run") (param i32) (result i32)
    (block $caught (result i32)
      (try_table (catch $t $caught)
        local.get 0
        call $thrower)
      i32.const -1))
)"#;

#[test]
fn a_module_with_a_cross_instance_inline_recompiles_per_instance() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let provider = wat::parse_str(PROVIDER).unwrap();
    let consumer = wat::parse_str(CONSUMER).unwrap();
    for &tier in Tier::ALL {
        let engine = Engine::new(Config::new().tier(tier)).unwrap();
        let mut world = RuntimeWorld::new();
        let src = world
            .instantiate(&engine, Module::new("provider", &provider).unwrap(), &[])
            .unwrap();
        let imports: Vec<_> = world
            .instance(src)
            .unwrap()
            .exports()
            .unwrap()
            .into_iter()
            .map(|(name, value)| Import::new("p", &name, value))
            .collect();

        let module = Module::new("consumer", &consumer).unwrap();
        let second = module.clone();

        let before_first = compiles();
        let a = world.instantiate(&engine, module, &imports).unwrap();
        let first_compiles = compiles() - before_first;
        let before_second = compiles();
        let b = world.instantiate(&engine, second, &imports).unwrap();
        let second_compiles = compiles() - before_second;

        assert_eq!(first_compiles, 1, "{tier:?}: the first consumer compiles");
        assert!(
            second_compiles > 0,
            "{tier:?}: code with an inlined import body must not be reused"
        );
        assert_eq!(
            world.invoke(a, "run", &[Value::I32(41)]).unwrap(),
            [Value::I32(41)]
        );
        assert_eq!(
            world.invoke(b, "run", &[Value::I32(7)]).unwrap(),
            [Value::I32(7)]
        );
    }
}

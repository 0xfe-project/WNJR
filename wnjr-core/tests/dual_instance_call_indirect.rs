//! Dual-instance call_indirect through the generic dispatch path.
//!
//! The table is exported, so active elements are stored in absolute form, and
//! the module contains a `table.set`, so `compute_static_table_dispatch_modes`
//! picks `TableDispatchMode::Generic` (not FixedLocalOnly, not Cached). Every
//! call through an own element therefore goes through the self-absolute range
//! block, which reads base/count from the instance's NativeContext.
//!
//! Two instances of the same module live in one world, so their funcaddr
//! ranges differ (asserted below via the exported funcref values). Each must
//! resolve its own elements to its own functions. A foreign funcref written
//! into a table lies outside the self range and must fall back to the runtime
//! call path and still run the right function.

use wnjr_core::{Config, Engine, Module, RuntimeWorld, Tier, Value};

fn module_bytes() -> Vec<u8> {
    wat::parse_str(
        r#"(module
        (type $t0 (func (param i32) (result i32)))
        (table (export "t") 3 funcref)
        (func $add_one (export "add_one") (type $t0)
            local.get 0 i32.const 1 i32.add)
        (func $add_two (export "add_two") (type $t0)
            local.get 0 i32.const 2 i32.add)
        (elem (i32.const 0) $add_one $add_two)
        (func (export "dispatch") (param i32 i32) (result i32)
            local.get 1
            local.get 0
            call_indirect (type $t0))
        (func (export "set") (param i32 funcref)
            local.get 0 local.get 1 table.set))"#,
    )
    .expect("wat encodes")
}

fn one_i32(values: Vec<Value>) -> i32 {
    match values.as_slice() {
        [Value::I32(v)] => *v,
        other => panic!("expected one i32, got {other:?}"),
    }
}

#[test]
fn dual_instance_call_indirect_generic_path_hits_own_functions() {
    for &tier in Tier::ALL {
        let engine = Engine::new(Config::new().tier(tier)).unwrap();
        let mut world = RuntimeWorld::new();
        let a = world
            .instantiate(&engine, Module::new("m", &module_bytes()).unwrap(), &[])
            .unwrap();
        let b = world
            .instantiate(&engine, Module::new("m", &module_bytes()).unwrap(), &[])
            .unwrap();

        // The two instances hold different absolute identities for the same
        // module function, i.e. their funcaddr ranges differ.
        let a_add_one = world
            .instance(a)
            .unwrap()
            .get_func("add_one")
            .unwrap()
            .to_value();
        let b_add_one = world
            .instance(b)
            .unwrap()
            .get_func("add_one")
            .unwrap()
            .to_value();
        let b_add_two = world
            .instance(b)
            .unwrap()
            .get_func("add_two")
            .unwrap()
            .to_value();
        assert_ne!(
            a_add_one, b_add_one,
            "{tier:?}: instances must not share funcaddrs"
        );

        let call = |world: &mut RuntimeWorld, inst, slot: i32, arg: i32| {
            one_i32(
                world
                    .invoke(inst, "dispatch", &[Value::I32(slot), Value::I32(arg)])
                    .unwrap_or_else(|e| panic!("{tier:?} dispatch({slot}, {arg}): {e:?}")),
            )
        };

        // Own elements, both instances, both slots: each hits its own function.
        assert_eq!(call(&mut world, a, 0, 10), 11, "{tier:?} a[0]");
        assert_eq!(call(&mut world, a, 1, 10), 12, "{tier:?} a[1]");
        assert_eq!(call(&mut world, b, 0, 20), 21, "{tier:?} b[0]");
        assert_eq!(call(&mut world, b, 1, 20), 22, "{tier:?} b[1]");

        // A foreign funcref is outside a's self range: it must take the
        // runtime fallback and still run b's add_two, not one of a's functions.
        world.invoke(a, "set", &[Value::I32(2), b_add_two]).unwrap();
        assert_eq!(call(&mut world, a, 2, 30), 32, "{tier:?} a[2] = b.add_two");
        // And a's own add_one written back through table.set is a self hit.
        world.invoke(b, "set", &[Value::I32(2), a_add_one]).unwrap();
        assert_eq!(call(&mut world, b, 2, 40), 41, "{tier:?} b[2] = a.add_one");

        // Own elements still resolve after the foreign writes.
        assert_eq!(call(&mut world, a, 0, 50), 51, "{tier:?} a[0] after writes");
        assert_eq!(call(&mut world, b, 1, 60), 62, "{tier:?} b[1] after writes");
    }
}

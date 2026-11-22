//! Turning a funcref back into something callable: a host that reads an element
//! out of a table or a global gets a reference, and a reference has to name a
//! function of the instance it came from.

use wnjr_core::{Config, Engine, Instance, Value};

fn engine() -> Engine {
    Engine::new(Config::new()).expect("engine")
}

/// A module with two functions in a table and a global that holds one.
fn table_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (type $unary (func (param i32) (result i32)))
            (table (export "t") 4 funcref)
            (global (export "g") funcref (ref.func $double))
            (func $double (type $unary) local.get 0 i32.const 2 i32.mul)
            (func $increment (type $unary) local.get 0 i32.const 1 i32.add)
            (elem (i32.const 0) $double $increment)
            (func (export "slot") (param i32) (result funcref)
                local.get 0 table.get 0))"#,
    )
    .expect("wat")
}

fn instantiate() -> Instance {
    let engine = engine();
    Instance::new(&engine, &table_module(), &[]).expect("instance")
}

#[test]
fn a_table_element_can_be_read_and_called() {
    let mut instance = instantiate();
    let table = instance.get_export("t").expect("export").expect("table");
    assert_eq!(table.table_size(), Some(4));

    let double = table.table_get(0).expect("table").expect("element");
    let mut results = [Value::Unknown];
    instance
        .call_value(&double, &[Value::I32(21)], &mut results)
        .expect("call");
    assert_eq!(results[0], Value::I32(42));

    let increment = table.table_get(1).expect("table").expect("element");
    let mut results = [Value::Unknown];
    instance
        .call_value(&increment, &[Value::I32(41)], &mut results)
        .expect("call");
    assert_eq!(results[0], Value::I32(42));
}

#[test]
fn a_null_element_is_not_callable() {
    let mut instance = instantiate();
    let table = instance.get_export("t").expect("export").expect("table");
    let empty = table.table_get(3).expect("table").expect("element");
    assert!(instance.func_from_value(&empty).is_none());
    assert!(instance
        .call_value(&empty, &[Value::I32(0)], &mut [Value::Unknown])
        .is_err());
}

#[test]
fn a_reference_out_of_a_global_is_callable() {
    let mut instance = instantiate();
    let global = instance.get_export("g").expect("export").expect("global");
    let value = global.global_value().expect("value");
    let mut results = [Value::Unknown];
    instance
        .call_value(&value, &[Value::I32(8)], &mut results)
        .expect("call");
    assert_eq!(results[0], Value::I32(16));
}

#[test]
fn a_reference_made_by_wasm_resolves_the_same_way() {
    let mut instance = instantiate();
    let mut results = [Value::Unknown];
    instance
        .invoke("slot", &[Value::I32(1)])
        .expect("slot")
        .into_iter()
        .for_each(|value| results[0] = value);
    let mut called = [Value::Unknown];
    instance
        .call_value(&results[0], &[Value::I32(9)], &mut called)
        .expect("call");
    assert_eq!(called[0], Value::I32(10));
}

#[test]
fn a_reference_from_another_world_does_not_resolve() {
    let instance = instantiate();
    let mut other = instantiate();
    let table = instance.get_export("t").expect("export").expect("table");
    let element = table.table_get(0).expect("table").expect("element");
    assert!(other.func_from_value(&element).is_none());
}

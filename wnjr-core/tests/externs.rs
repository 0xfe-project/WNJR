//! The host can inspect and write what a module exports.
#![cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]

use wnjr_core::{Config, Engine, ExternKind, Instance, Tier, Value};

fn engine() -> Engine {
    Engine::new(Config::new().tier(Tier::Jit)).expect("engine")
}

#[test]
fn exports_are_classified_and_readable() {
    let wasm = wat::parse_str(
        r#"(module
            (memory (export "mem") 1 4)
            (global (export "counter") (mut i32) (i32.const 7))
            (global (export "fixed") i32 (i32.const 9))
            (table (export "tab") 2 8 funcref)
            (func (export "noop")))"#,
    )
    .expect("valid module");
    let instance = Instance::new(&engine(), &wasm, &[]).expect("instance");

    let exports = instance.exports().expect("exports");
    let by_name = |name: &str| {
        exports
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, e)| e.clone())
            .expect("named export")
    };

    assert_eq!(by_name("mem").kind(), ExternKind::Memory);
    assert_eq!(by_name("tab").kind(), ExternKind::Table);
    assert_eq!(by_name("noop").kind(), ExternKind::Func);
    let counter = by_name("counter");
    assert_eq!(counter.kind(), ExternKind::Global);
    assert_eq!(counter.is_mutable(), Some(true));

    assert_eq!(counter.global_value(), Some(Value::I32(7)));
    counter
        .set_global_value(Value::I32(41))
        .expect("mutable global accepts a write");
    assert_eq!(
        instance
            .get_export("counter")
            .unwrap()
            .unwrap()
            .global_value(),
        Some(Value::I32(41)),
        "the write reached the instance"
    );

    let fixed = by_name("fixed");
    assert_eq!(fixed.is_mutable(), Some(false));
    assert!(
        fixed.set_global_value(Value::I32(1)).is_err(),
        "an immutable global rejects a write"
    );
    assert!(
        counter.set_global_value(Value::I64(1)).is_err(),
        "a value of the wrong type is refused"
    );
}

#[test]
fn a_table_can_be_read_written_and_grown() {
    let wasm = wat::parse_str(
        r#"(module
            (table (export "tab") 2 5 funcref)
            (func $f) (elem (i32.const 0) $f)
            (func (export "size") (result i32) table.size))"#,
    )
    .expect("valid module");
    let mut instance = Instance::new(&engine(), &wasm, &[]).expect("instance");
    let table = instance.get_export("tab").unwrap().unwrap();

    assert_eq!(table.table_size(), Some(2));
    assert!(table.table_get(0).unwrap().is_ok());
    assert!(table.table_get(9).unwrap().is_err(), "out of bounds");

    let null = Value::Ref(wnjr_core::RefValue::null(), wnjr_core::value_type::RefType::funcref());
    table.table_set(1, null).expect("write a null reference");
    assert_eq!(table.table_grow(2, null), Ok(2), "grow returns the old size");
    assert_eq!(
        instance.invoke("size", &[]).unwrap(),
        vec![Value::I32(4)],
        "the instance sees the grown table"
    );
    assert!(
        table.table_grow(5, null).is_err(),
        "growing past the declared maximum fails"
    );
}

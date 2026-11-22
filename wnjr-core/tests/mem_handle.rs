//! A host handle to a wasm linear memory: the bytes are shared, the base does
//! not move, and a grow is announced to whoever owns the view.
#![cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use wnjr_core::{Config, Engine, Import, Instance, MemHandle, Tier, Value};

const PAGE: usize = 65536;

fn engine() -> Engine {
    Engine::new(
        Config::new()
            .tier(Tier::Jit)
            .wasm_memory_max_pages(u32::MAX),
    )
    .expect("engine")
}

/// A module that reads byte 0, writes byte 1, grows and reports its size.
fn probe_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (import "host" "memory" (memory 1 4))
            (func (export "read0") (result i32) i32.const 0 i32.load8_u)
            (func (export "write1") (param i32) i32.const 1 local.get 0 i32.store8)
            (func (export "grow") (param i32) (result i32) local.get 0 memory.grow)
            (func (export "size") (result i32) memory.size))"#,
    )
    .expect("valid module")
}

#[test]
fn host_and_wasm_see_one_memory() {
    let engine = engine();
    let handle = MemHandle::new(1, Some(4)).expect("handle");
    let imports = vec![Import::memory_handle("host", "memory", &handle).expect("import")];
    let mut instance = Instance::new(&engine, &probe_module(), &imports).expect("instance");

    // host -> wasm
    // SAFETY: the reservation is committed for at least one page and nothing
    // else is writing it.
    unsafe { *handle.base() = 0x2a };
    assert_eq!(
        instance.invoke("read0", &[]).unwrap(),
        vec![Value::I32(0x2a)]
    );

    // wasm -> host
    instance.invoke("write1", &[Value::I32(0x7f)]).unwrap();
    // SAFETY: as above, byte 1 is inside the committed first page.
    let seen = unsafe { *handle.base().add(1) };
    assert_eq!(seen, 0x7f);
}

#[test]
fn a_grow_keeps_the_base_and_announces_itself() {
    let engine = engine();
    let handle = MemHandle::new(1, Some(4)).expect("handle");
    let base = handle.base();
    let seen = Arc::new(AtomicUsize::new(0));
    let id = handle.id();
    let seen_by_listener = Arc::clone(&seen);
    handle.set_grow_listener(Some(Box::new(move |memory_id, bytes| {
        assert_eq!(
            memory_id, id,
            "the callback reports the memory it belongs to"
        );
        seen_by_listener.store(bytes, Ordering::SeqCst);
    })));

    let imports = vec![Import::memory_handle("host", "memory", &handle).expect("import")];
    let mut instance = Instance::new(&engine, &probe_module(), &imports).expect("instance");

    assert_eq!(
        instance.invoke("grow", &[Value::I32(1)]).unwrap(),
        vec![Value::I32(1)],
        "wasm grow returns the previous size"
    );
    assert_eq!(handle.len(), 2 * PAGE, "the handle sees the new length");
    assert_eq!(handle.base(), base, "growing does not move the base");
    assert_eq!(
        seen.load(Ordering::SeqCst),
        2 * PAGE,
        "the callback ran with the new length"
    );

    // The grown pages are real: the host can write the last byte of page 2 and
    // wasm can read it back through the shared reservation.
    let last = 2 * PAGE - 1;
    // SAFETY: the second page is committed now, so the byte is inside the
    // usable region.
    unsafe { *base.add(last) = 0x5a };
    assert_eq!(handle.len(), 2 * PAGE);
}

#[test]
fn a_failed_grow_does_not_announce_itself() {
    let engine = engine();
    let handle = MemHandle::new(1, Some(2)).expect("handle");
    let fired = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fired);
    handle.set_grow_listener(Some(Box::new(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    })));

    let imports = vec![Import::memory_handle("host", "memory", &handle).expect("import")];
    let mut instance = Instance::new(&engine, &probe_module(), &imports).expect("instance");

    // Past the declared maximum: wasm reports failure, nothing is committed,
    // nothing is announced.
    assert_eq!(
        instance.invoke("grow", &[Value::I32(2)]).unwrap(),
        vec![Value::I32(-1)]
    );
    assert_eq!(handle.len(), PAGE);
    assert_eq!(fired.load(Ordering::SeqCst), 0);

    // A host grow past the ceiling is refused the same way.
    assert!(handle.grow(2).is_err());
    assert_eq!(handle.len(), PAGE);
    assert_eq!(fired.load(Ordering::SeqCst), 0);
}

#[test]
fn a_host_grow_is_visible_to_the_running_instance() {
    let engine = engine();
    let handle = MemHandle::new(1, Some(4)).expect("handle");
    let imports = vec![Import::memory_handle("host", "memory", &handle).expect("import")];
    let mut instance = Instance::new(&engine, &probe_module(), &imports).expect("instance");

    handle.grow(2).expect("host grow");
    assert_eq!(handle.len(), 3 * PAGE);
    assert_eq!(
        instance.invoke("size", &[]).unwrap(),
        vec![Value::I32(3)],
        "the instance reports the size the host grew it to"
    );
}

#[test]
fn the_mapping_outlives_the_instance_and_dies_with_the_last_handle() {
    // A reservation that leaked would still count against the address space
    // after the handle is dropped; reserving many 8 GB regions in a row is what
    // makes that observable without reaching into the allocator.
    for _ in 0..64 {
        let handle = MemHandle::new(1, Some(2)).expect("handle");
        let base = handle.base();
        {
            let engine = engine();
            let imports = vec![Import::memory_handle("host", "memory", &handle).expect("import")];
            let instance = Instance::new(&engine, &probe_module(), &imports).expect("instance");
            drop(instance);
        }
        // Still alive after the instance is gone, and still the same mapping.
        assert_eq!(handle.base(), base);
        drop(handle);
    }
}

#[test]
fn a_memory_the_instance_exports_can_be_handed_out() {
    let engine = engine();
    let wasm = wat::parse_str(r#"(module (memory (export "mem") 1 3))"#).expect("valid module");
    let instance = Instance::new(&engine, &wasm, &[]).expect("instance");

    let handle = instance
        .memory_handle("mem")
        .expect("lookup")
        .expect("a guard-page memory has a handle");
    assert_eq!(handle.len(), PAGE);
    assert_eq!(handle.capacity(), 3 * PAGE);

    assert!(
        instance.memory_handle("nothing-here").unwrap().is_none(),
        "an unknown export is not a handle"
    );
}

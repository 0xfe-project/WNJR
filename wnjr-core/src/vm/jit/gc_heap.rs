//! Deferred reference counting for Wasm GC objects.
//!
//! This is not a tracing collector and not a copying collector. Each object
//! carries a reference count. `alloc_*` starts the count at one. [`GcHeap::retain`]
//! and [`GcHeap::release`] are the only operations that move it. [`GcHeap::release`]
//! drops the payload at zero and leaves the slot in place, so a [`GcRef`] index
//! is never reused.
//!
//! `Value` is `Copy`. The JIT copies GC references as plain words and does not
//! yet emit retain or release, so objects created by compiled code are not
//! reclaimed until that lowering exists. The count is real; the missing piece
//! is the compiler calling it.

use core::cell::Cell;

use crate::{collections, vm::value::Value, WasmError};

/// Reference to one arena-allocated GC object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GcRef(usize);

impl GcRef {
    #[inline]
    pub(crate) const fn new(index: usize) -> Self {
        Self(index)
    }

    #[inline]
    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone)]
enum GcObject {
    Struct(GcStruct),
    Array(GcArray),
}

#[derive(Debug, Clone)]
pub(crate) struct GcStruct {
    pub(crate) type_idx: u32,
    pub(crate) fields: collections::Vec<Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct GcArray {
    pub(crate) type_idx: u32,
    pub(crate) elements: collections::Vec<Value>,
}

#[derive(Debug, Clone)]
struct Slot {
    rc: Cell<u32>,
    object: Option<GcObject>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GcHeap {
    slots: collections::Vec<Slot>,
}

impl GcHeap {
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            slots: collections::Vec::new(),
        }
    }

    fn push(&mut self, object: GcObject) -> GcRef {
        let index = self.slots.len();
        self.slots.push(Slot {
            rc: Cell::new(1),
            object: Some(object),
        });
        GcRef::new(index)
    }

    pub(crate) fn alloc_struct(&mut self, type_idx: u32, fields: collections::Vec<Value>) -> GcRef {
        self.push(GcObject::Struct(GcStruct { type_idx, fields }))
    }

    pub(crate) fn alloc_array(
        &mut self,
        type_idx: u32,
        elements: collections::Vec<Value>,
    ) -> GcRef {
        self.push(GcObject::Array(GcArray { type_idx, elements }))
    }

    /// Bump the count. A freed or out-of-range reference is left alone.
    pub(crate) fn retain(&self, gc_ref: GcRef) {
        if let Some(slot) = self.slots.get(gc_ref.index()) {
            if slot.object.is_some() {
                slot.rc.set(slot.rc.get().saturating_add(1));
            }
        }
    }

    /// Drop the count. At zero the payload is dropped and the slot stays,
    /// so later lookups fail instead of aliasing a new object.
    pub(crate) fn release(&mut self, gc_ref: GcRef) {
        let Some(slot) = self.slots.get_mut(gc_ref.index()) else {
            return;
        };
        if slot.object.is_none() {
            return;
        }
        let next = slot.rc.get().saturating_sub(1);
        slot.rc.set(next);
        if next == 0 {
            slot.object = None;
        }
    }

    fn get(&self, gc_ref: GcRef) -> Result<&GcObject, WasmError> {
        self.slots
            .get(gc_ref.index())
            .and_then(|slot| slot.object.as_ref())
            .ok_or_else(|| WasmError::invalid("invalid GC reference"))
    }

    fn get_mut(&mut self, gc_ref: GcRef) -> Result<&mut GcObject, WasmError> {
        self.slots
            .get_mut(gc_ref.index())
            .and_then(|slot| slot.object.as_mut())
            .ok_or_else(|| WasmError::invalid("invalid GC reference"))
    }

    #[inline]
    pub(crate) fn get_struct(&self, gc_ref: GcRef) -> Result<&GcStruct, WasmError> {
        match self.get(gc_ref)? {
            GcObject::Struct(obj) => Ok(obj),
            GcObject::Array(_) => Err(WasmError::invalid("GC reference is not a struct")),
        }
    }

    #[inline]
    pub(crate) fn get_struct_mut(&mut self, gc_ref: GcRef) -> Result<&mut GcStruct, WasmError> {
        match self.get_mut(gc_ref)? {
            GcObject::Struct(obj) => Ok(obj),
            GcObject::Array(_) => Err(WasmError::invalid("GC reference is not a struct")),
        }
    }

    #[inline]
    pub(crate) fn get_array(&self, gc_ref: GcRef) -> Result<&GcArray, WasmError> {
        match self.get(gc_ref)? {
            GcObject::Array(obj) => Ok(obj),
            GcObject::Struct(_) => Err(WasmError::invalid("GC reference is not an array")),
        }
    }

    #[inline]
    pub(crate) fn get_array_mut(&mut self, gc_ref: GcRef) -> Result<&mut GcArray, WasmError> {
        match self.get_mut(gc_ref)? {
            GcObject::Array(obj) => Ok(obj),
            GcObject::Struct(_) => Err(WasmError::invalid("GC reference is not an array")),
        }
    }

    #[inline]
    pub(crate) fn type_idx(&self, gc_ref: GcRef) -> Result<u32, WasmError> {
        match self.get(gc_ref)? {
            GcObject::Struct(obj) => Ok(obj.type_idx),
            GcObject::Array(obj) => Ok(obj.type_idx),
        }
    }

    #[inline]
    pub(crate) fn is_struct(&self, gc_ref: GcRef) -> bool {
        matches!(
            self.slots
                .get(gc_ref.index())
                .and_then(|slot| slot.object.as_ref()),
            Some(GcObject::Struct(_))
        )
    }

    #[inline]
    pub(crate) fn is_array(&self, gc_ref: GcRef) -> bool {
        matches!(
            self.slots
                .get(gc_ref.index())
                .and_then(|slot| slot.object.as_ref()),
            Some(GcObject::Array(_))
        )
    }
}

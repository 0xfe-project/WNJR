//! WebAssembly 3.0 type system — thin re-export shim over njvm_ir types.
//!
//! The core types (ValueType, RefType, HeapType, etc.) now live in njvm-ir.
//! This module re-exports them for compatibility and adds frontend-only parsing/encoding functions.

pub use njvm_ir::{AbstractHeapType, HeapType, RefType, ValueType};

use crate::{collections, error::WasmError, utils::payload::Payload};

// ============================================================================
// Frontend-only parsing and encoding functions (not in njvm-ir, which is pure data).
// ============================================================================

pub(crate) fn parse_heap_type(payload: &mut Payload) -> Result<HeapType, WasmError> {
    let first_byte = payload.peek_u8().map_err(WasmError::from)?;

    if let Ok(aht) = AbstractHeapType::try_from(first_byte) {
        payload.read_u8().map_err(WasmError::from)?;
        return Ok(HeapType::Abstract(aht));
    }

    let type_idx = payload.read_leb128_i32().map_err(WasmError::from)?;
    if type_idx < 0 {
        return Err(WasmError::invalid("Type index cannot be negative"));
    }

    Ok(HeapType::Concrete(type_idx as u32))
}

pub fn encode_heap_type(heap_type: &HeapType) -> collections::Vec<u8> {
    match heap_type {
        HeapType::Abstract(aht) => collections::vec![*aht as u8],
        HeapType::Concrete(idx) => crate::utils::leb128::write_leb128_i32(*idx as i32),
    }
}

pub(crate) fn parse_value_type(payload: &mut Payload) -> Result<ValueType, WasmError> {
    let first_byte = payload.read_u8().map_err(WasmError::from)?;

    match first_byte {
        0x7F => Ok(ValueType::I32),
        0x7E => Ok(ValueType::I64),
        0x7D => Ok(ValueType::F32),
        0x7C => Ok(ValueType::F64),
        0x7B => Ok(ValueType::V128),
        0x63 => {
            let heap_type = parse_heap_type(payload)?;
            Ok(ValueType::Ref(RefType::new(true, heap_type)))
        }
        0x64 => {
            let heap_type = parse_heap_type(payload)?;
            Ok(ValueType::Ref(RefType::new(false, heap_type)))
        }
        _ => {
            if let Ok(aht) = AbstractHeapType::try_from(first_byte) {
                Ok(ValueType::Ref(RefType::nullable_abstract(aht)))
            } else {
                Err(WasmError::invalid("Invalid value type byte"))
            }
        }
    }
}

pub fn encode_value_type(value_type: &ValueType) -> collections::Vec<u8> {
    match value_type {
        ValueType::I32 => collections::vec![0x7F],
        ValueType::I64 => collections::vec![0x7E],
        ValueType::F32 => collections::vec![0x7D],
        ValueType::F64 => collections::vec![0x7C],
        ValueType::V128 => collections::vec![0x7B],
        ValueType::Ref(rt) => {
            if rt.nullable {
                if let HeapType::Abstract(aht) = rt.heap_type {
                    return collections::vec![aht as u8];
                }
            }

            let mut bytes = collections::vec![if rt.nullable { 0x63 } else { 0x64 }];
            bytes.extend(encode_heap_type(&rt.heap_type));
            bytes
        }
        ValueType::Unknown => panic!("Cannot encode Unknown value type"),
    }
}

#[inline]
pub fn ensure_value_type_enabled_in_build(value_type: &ValueType) -> Result<(), WasmError> {
    // Only read on backends without SIMD (the cfg block below).
    let _ = value_type;
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    if matches!(value_type, ValueType::V128) {
        return Err(WasmError::invalid("SIMD is not supported on this CPU"));
    }
    Ok(())
}


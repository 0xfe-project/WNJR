//! WebAssembly 3.0 Type Definitions
//!
//! The composite type definitions now live in `njvm_ir::types::type_defs`.
//! This module re-exports them and keeps the type-section reference checks,
//! which return `WasmError`, as free functions.

pub use njvm_ir::types::type_defs::*;

use crate::{
    error::WasmError,
    value_type::{HeapType, ValueType},
};
use tracked_alloc::sync::Arc;

pub(crate) fn validate_type_references(
    def_type: &DefType,
    current_idx: usize,
    type_count: usize,
    all_types: &[Arc<DefType>],
) -> Result<(), WasmError> {
    for &supertype_idx in &def_type.supertypes {
        if supertype_idx as usize >= type_count {
            return Err(WasmError::invalid("unknown supertype"));
        }
    }

    match &def_type.composite {
        CompositeType::Func(func_type) => {
            for param in func_type.params() {
                validate_valtype_references(
                    param,
                    current_idx,
                    def_type.rec_group,
                    type_count,
                    all_types,
                )?;
            }
            for result in func_type.results() {
                validate_valtype_references(
                    result,
                    current_idx,
                    def_type.rec_group,
                    type_count,
                    all_types,
                )?;
            }
        }
        CompositeType::Struct(struct_type) => {
            for field in &struct_type.fields {
                validate_storage_type_references(
                    &field.storage,
                    current_idx,
                    def_type.rec_group,
                    type_count,
                    all_types,
                )?;
            }
        }
        CompositeType::Array(array_type) => {
            validate_storage_type_references(
                &array_type.element.storage,
                current_idx,
                def_type.rec_group,
                type_count,
                all_types,
            )?;
        }
    }

    Ok(())
}

pub(crate) fn validate_valtype_references(
    vt: &ValueType,
    current_idx: usize,
    rec_group: Option<u32>,
    type_count: usize,
    all_types: &[Arc<DefType>],
) -> Result<(), WasmError> {
    if let ValueType::Ref(ref_type) = vt {
        if let HeapType::Concrete(idx) = ref_type.heap_type {
            let idx_usize = idx as usize;
            if idx_usize >= type_count {
                return Err(WasmError::invalid("unknown type"));
            }

            if idx_usize > current_idx {
                if let Some(my_rec_group) = rec_group {
                    if let Some(referenced_type) = all_types.get(idx_usize) {
                        if referenced_type.rec_group != Some(my_rec_group) {
                            return Err(WasmError::invalid(
                                "forward reference crosses recursion group boundaries",
                            ));
                        }
                    }
                } else {
                    return Err(WasmError::invalid(
                        "forward reference outside recursion group",
                    ));
                }
            }
        }
    }

    Ok(())
}

fn validate_storage_type_references(
    st: &StorageType,
    current_idx: usize,
    rec_group: Option<u32>,
    type_count: usize,
    all_types: &[Arc<DefType>],
) -> Result<(), WasmError> {
    if let StorageType::Val(vt) = st {
        validate_valtype_references(vt, current_idx, rec_group, type_count, all_types)?;
    }
    Ok(())
}


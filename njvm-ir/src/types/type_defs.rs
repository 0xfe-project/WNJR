//! WebAssembly 3.0 Type Definitions
//!
//! This module contains all composite type definitions used in the unified
//! type section:
//! - Function types (0x60)
//! - Struct types (0x5F)
//! - Array types (0x5E)

use crate::collections;
use crate::types::ValueType;
use core::fmt;
use tracked_alloc::string::String;
use tracked_alloc::sync::Arc;

/// A complete type definition from the type section.
#[derive(Debug, Clone, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DefType {
    /// The composite type (function, struct, or array).
    pub composite: CompositeType,
    /// Indices of declared supertypes.
    pub supertypes: collections::Vec<u32>,
    /// Whether this type is final.
    pub is_final: bool,
    /// Explicit recursion-group id, if any.
    pub rec_group: Option<u32>,
}


// Validation functions (validate_type_references, validate_valtype_references, validate_storage_type_references)
// moved to wnjr-wasm-njvm frontend crate — they return WasmError which carries runtime payloads.

#[derive(Debug, Clone, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum CompositeType {
    Func(Arc<FunctionType>),
    Struct(StructType),
    Array(ArrayType),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FunctionType {
    pub params: collections::Vec<ValueType>,
    pub results: collections::Vec<ValueType>,
}

impl FunctionType {
    pub fn new(params: collections::Vec<ValueType>, results: collections::Vec<ValueType>) -> Self {
        Self { params, results }
    }

    pub fn params(&self) -> &[ValueType] {
        &self.params
    }

    pub fn results(&self) -> &[ValueType] {
        &self.results
    }
}

impl fmt::Display for FunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let params: collections::Vec<String> = self
            .params
            .iter()
            .map(|v| {
                let mut buf = String::new();
                core::fmt::Write::write_fmt(&mut buf, format_args!("{}", v)).unwrap();
                buf
            })
            .collect();
        let results: collections::Vec<String> = self
            .results
            .iter()
            .map(|v| {
                let mut buf = String::new();
                core::fmt::Write::write_fmt(&mut buf, format_args!("{}", v)).unwrap();
                buf
            })
            .collect();
        write!(f, "({}) -> ({})", params.join(", "), results.join(", "))
    }
}

#[derive(Debug, Clone, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StructType {
    pub fields: collections::Vec<FieldType>,
}

impl StructType {
    pub fn new(fields: collections::Vec<FieldType>) -> Self {
        Self { fields }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ArrayType {
    pub element: FieldType,
}

impl ArrayType {
    pub fn new(element: FieldType) -> Self {
        Self { element }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FieldType {
    pub storage: StorageType,
    pub mutable: bool,
}

impl FieldType {
    pub fn new(storage: StorageType, mutable: bool) -> Self {
        Self { storage, mutable }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum StorageType {
    Val(ValueType),
    Packed(PackedType),
}

impl StorageType {
    pub fn to_valtype(&self) -> ValueType {
        match self {
            StorageType::Val(vt) => *vt,
            StorageType::Packed(PackedType::I8 | PackedType::I16) => ValueType::I32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum PackedType {
    I8,
    I16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_type() {
        let ft = FunctionType::new(
            collections::vec![ValueType::I32, ValueType::I64],
            collections::vec![ValueType::F32],
        );

        assert_eq!(ft.params().len(), 2);
        assert_eq!(ft.results().len(), 1);
    }
}

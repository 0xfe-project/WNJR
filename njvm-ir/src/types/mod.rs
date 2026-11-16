//! WebAssembly 3.0 type system.
//!
//! This module contains the complete type vocabulary plus subtyping logic.
//! It does NOT contain parsing (leb128, module sections) — those stay in the frontend.

pub mod value_type;
pub mod type_defs;
pub mod type_context;

pub use value_type::{ValueType, RefType, HeapType, AbstractHeapType};
pub use type_defs::{
    FunctionType, StructType, ArrayType, FieldType, StorageType, PackedType,
    CompositeType, DefType,
};
pub use type_context::TypeContext;

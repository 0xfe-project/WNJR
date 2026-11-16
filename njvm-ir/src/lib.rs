#![no_std]
#![doc = r"
NJVM Intermediate Representation

This crate defines the contract between WASM frontends (parser/validator/decoder)
and the NJVM backend (middle/lower/codegen). It contains:

- The complete WebAssembly 3.0 type system (value types, heap types, ref types, composite types)
- Semantic IR (SemanticProgram, SemanticOpKind, PrimitiveOpKind)
- Type-checking logic (subtyping, initialization, top/bottom types)

This crate is **pure data structures and logic**. It does NOT contain:
- Byte-level parsing (leb128 decoding, module parsing)
- Runtime value encoding (RefValue, Value, TagIdentity)
- Error types with runtime payloads (WasmError)
- Execution machinery (JIT, interpreter, runtime)
"]

// njvm-ir is the shared IR contract. Its consumers — wnjr-wasm-njvm (frontend)
// and njvm-core (backend) — are wired in from S0.2 onward. Until then many
// items have no in-crate caller, which is expected for a contract crate.
#![allow(dead_code)]

extern crate alloc;

// collections façade matches wnjr-core's pattern.
pub(crate) mod collections {
    pub(crate) use tracked_alloc::{into_alloc_vec, vec, Vec};
}

pub mod types;
pub mod sir;

// Re-exports for ergonomic frontend use
pub use types::{ValueType, RefType, HeapType, AbstractHeapType};
pub use sir::{
    SemanticProgram, SemanticOp, SemanticOpKind,
    PrimitiveOpKind, SemanticTarget, SemanticIndex, BrTableEntry, SemanticCatchClause,
};

// TryFrom<u8> for ValueType, implemented here to avoid orphan-rule conflict.
// The frontend calls parse_value_type directly; this is for legacy decode sites.
use core::convert::TryFrom;
impl TryFrom<u8> for ValueType {
    type Error = ();  // njvm-ir is no_std and has no WasmError; frontend wraps this.

    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        match byte {
            0x7F => Ok(ValueType::I32),
            0x7E => Ok(ValueType::I64),
            0x7D => Ok(ValueType::F32),
            0x7C => Ok(ValueType::F64),
            0x7B => Ok(ValueType::V128),
            _ => {
                if let Ok(aht) = AbstractHeapType::try_from(byte) {
                    Ok(ValueType::Ref(RefType::nullable_abstract(aht)))
                } else {
                    Err(())
                }
            }
        }
    }
}

#[cfg(test)]
mod roundtrip_serde {
    extern crate std;
    use super::*;
    use alloc::vec;

    #[test]
    fn roundtrip_semantic_program() {
        let prog = SemanticProgram {
            params: 1,
            results: 1,
            local_count: 2,
            max_stack_height: 3,
            ops: vec![
                SemanticOp {
                    kind: SemanticOpKind::LocalGet { idx: 0 },
                },
                SemanticOp {
                    kind: SemanticOpKind::Primitive(PrimitiveOpKind::I32Const { value: 42 }),
                },
                SemanticOp {
                    kind: SemanticOpKind::ReturnOne,
                },
            ],
            local_types: vec![ValueType::I32, ValueType::I64],
            result_types: vec![ValueType::I32],
            op_result_types: Default::default(),
        };

        let bytes = rmp_serde::to_vec(&prog).expect("serialize failed");
        let decoded: SemanticProgram = rmp_serde::from_slice(&bytes).expect("deserialize failed");
        assert_eq!(prog, decoded);
    }

    #[test]
    fn roundtrip_value_type() {
        let types = vec![
            ValueType::I32,
            ValueType::I64,
            ValueType::F32,
            ValueType::F64,
            ValueType::V128,
            ValueType::Ref(RefType::funcref()),
            ValueType::Ref(RefType::externref()),
        ];

        for vt in types {
            let bytes = rmp_serde::to_vec(&vt).expect("serialize failed");
            let decoded: ValueType = rmp_serde::from_slice(&bytes).expect("deserialize failed");
            assert_eq!(vt, decoded);
        }
    }
}

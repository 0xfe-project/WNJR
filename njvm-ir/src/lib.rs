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

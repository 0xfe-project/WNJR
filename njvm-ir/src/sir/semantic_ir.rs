//! Decoded semantic function-body model.
//!
//! `PrimitiveOpKind` carries the shared leaf-op vocabulary. `SemanticOpKind` is the
//! larger per-function IR that embeds those leaf ops alongside locals, calls,
//! returns, structured control markers, and branch targets.
//!
//! Important:
//! - no frame-slot layout
//! - no spill/fill preparation artifacts
//! - no local-cache decisions
//! - no prepared block shaping or machine placement

use crate::collections;

use tracked_alloc::collections::BTreeMap;

use crate::types::ValueType;

use super::common::{BrTableEntry, SemanticTarget};
use super::primitive_op::PrimitiveOpKind;

/// Catch clause of a `try_table`, already resolved against the enclosing
/// control stack. `tag_idx = None` marks a `catch_all[_ref]` wildcard;
/// `forwards_exn = true` marks the `_ref` forms.
#[derive(Clone, Debug, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SemanticCatchClause {
    pub tag_idx: Option<u32>,
    pub payload_arity: u16,
    pub forwards_exn: bool,
    pub stack_drop: u32,
    pub target: SemanticTarget,
}

/// One semantic Wasm operation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SemanticOp {
    pub kind: SemanticOpKind,
}

/// Semantic function-body op kind.
///
/// This owns the parts of Wasm that are not just reusable leaf ops: locals,
/// calls, returns, structured control markers, and branch metadata. Ordinary
/// non-structural ops are represented as `Primitive(PrimitiveOpKind)`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum SemanticOpKind {
    Primitive(PrimitiveOpKind),
    LocalGet {
        idx: u16,
    },
    LocalSet {
        idx: u16,
    },
    LocalTee {
        idx: u16,
    },
    Block {
        params: u16,
        results: u16,
    },
    Loop {
        params: u16,
        results: u16,
    },
    If {
        params: u16,
        results: u16,
        else_target: SemanticTarget,
    },
    Else {
        end_target: SemanticTarget,
    },
    End,
    Br {
        stack_drop: u32,
        arity: u16,
        target: SemanticTarget,
    },
    BrIf {
        stack_drop: u32,
        arity: u16,
        target: SemanticTarget,
    },
    BrOnNull {
        stack_drop: u32,
        arity: u16,
        target: SemanticTarget,
        ref_type: ValueType,
    },
    BrOnNonNull {
        stack_drop: u32,
        arity: u16,
        target: SemanticTarget,
        ref_type: ValueType,
    },
    BrOnCast {
        stack_drop: u32,
        arity: u16,
        target: SemanticTarget,
        fail_type: ValueType,
        cast_type: ValueType,
    },
    BrOnCastFail {
        stack_drop: u32,
        arity: u16,
        target: SemanticTarget,
        fail_type: ValueType,
        cast_type: ValueType,
    },
    BrTable {
        entries: collections::Vec<BrTableEntry>,
    },
    CallDirect {
        callee: u32,
        params: u16,
        results: u16,
    },
    CallIndirect {
        type_idx: u32,
        table_idx: u32,
        /// The table is 64-bit, so its index operand is an i64.
        index64: bool,
        params: u16,
        results: u16,
    },
    CallRef {
        type_idx: u32,
        params: u16,
        results: u16,
    },
    ReturnCallDirect {
        callee: u32,
        params: u16,
        results: u16,
    },
    ReturnCallIndirect {
        type_idx: u32,
        table_idx: u32,
        /// The table is 64-bit, so its index operand is an i64.
        index64: bool,
        params: u16,
        results: u16,
    },
    ReturnCallRef {
        type_idx: u32,
        params: u16,
        results: u16,
    },
    ReturnVoid,
    ReturnOne,
    Return {
        arity: u16,
    },
    TryTable {
        params: u16,
        results: u16,
        catches: collections::Vec<SemanticCatchClause>,
    },
    Throw {
        tag_idx: u32,
        arity: u16,
    },
    AllocExnRef {
        tag_idx: u32,
    },
    ThrowRef,
}

/// Semantic program for one function body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SemanticProgram {
    pub params: u16,
    pub results: u16,
    pub local_count: u16,
    pub max_stack_height: u16,
    pub ops: collections::Vec<SemanticOp>,
    /// Per-local value types (params ++ non-param locals).
    ///
    /// When non-empty, `local_types.len() == local_count`.
    /// When empty, downstream consumers should treat all values as untyped.
    pub local_types: collections::Vec<ValueType>,
    /// Function result types in signature order.
    ///
    /// When non-empty, `result_types.len() == results`.
    pub result_types: collections::Vec<ValueType>,
    /// Per-op result types for dynamic producers and structured control signatures.
    ///
    /// Maps semantic op index to result value types for ops that push
    /// results whose types are not deterministic from the opcode alone
    /// (calls, blocks with multi-value signatures, etc.).
    pub op_result_types: BTreeMap<usize, collections::Vec<ValueType>>,
}

// NOTE: validate() and ensure_prepare_supported() moved to njvm-core
// (they return WasmError, which carries runtime payloads).

impl From<PrimitiveOpKind> for SemanticOpKind {
    #[inline]
    fn from(kind: PrimitiveOpKind) -> Self {
        Self::Primitive(kind)
    }
}

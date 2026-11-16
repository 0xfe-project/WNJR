//! Semantic IR definitions.
//!
//! This is the contract between the WASM frontend and the NJVM middle layer.
//! It defines the decoded semantic function model: primitive ops, structured
//! control flow, locals, calls, and branch targets.

pub mod common;
pub mod primitive_op;
pub mod semantic_ir;

pub use common::{SemanticIndex, SemanticTarget, BrTableEntry};
pub use primitive_op::PrimitiveOpKind;
pub use semantic_ir::{SemanticProgram, SemanticOp, SemanticOpKind, SemanticCatchClause};

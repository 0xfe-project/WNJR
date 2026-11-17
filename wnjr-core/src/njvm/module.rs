//! NJVM-IR function and module structures.

use crate::collections::Vec;

use super::op::Op;
use super::types::Type;

/// A function in NJVM-IR.
///
/// N1 (this task) defines the function body as a flat `Vec<Op>` with labels
/// and jumps. N3 (verifier) will check that the CFG is reducible and that
/// labels are declared before use. For now, the builder just appends ops.
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    /// Parameter types.
    pub params: Vec<Type>,
    /// Return types (multiple results are allowed).
    pub results: Vec<Type>,
    /// Local variable types (beyond parameters).
    pub locals: Vec<Type>,
    /// Function body: a sequence of ops.
    pub body: Vec<Op>,
}

impl Function {
    /// Create a new function with the given signature and locals. Body starts empty.
    pub fn new(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>) -> Self {
        Function {
            params,
            results,
            locals,
            body: Vec::new(),
        }
    }
}

/// An NJVM-IR module.
///
/// N1 defines the skeleton. N2 (frontend producer) will populate the function
/// table, type table, globals, data, tags, and memory descriptors. For now,
/// these are placeholder `Vec`s.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    /// Type signatures (function types). N2 will fill this.
    pub types: Vec<FunctionType>,
    /// Function definitions.
    pub functions: Vec<Function>,
    /// Global variable types and initializers. N2 will fill this.
    pub globals: Vec<Global>,
    /// Data segments. N2 will fill this.
    pub data: Vec<DataSegment>,
    /// Exception tags. N2 will fill this.
    pub tags: Vec<Tag>,
    /// Memory descriptors (min/max pages). N2 will fill this.
    pub memories: Vec<Memory>,
}

impl Module {
    /// Create an empty module.
    pub fn new() -> Self {
        Module {
            types: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
            data: Vec::new(),
            tags: Vec::new(),
            memories: Vec::new(),
        }
    }
}

impl Default for Module {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Placeholder types for module metadata (N2 will define full shapes)
// ─────────────────────────────────────────────────────────────────────────────

/// A function type signature.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunctionType {
    pub params: Vec<Type>,
    pub results: Vec<Type>,
}

/// A global variable descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct Global {
    pub ty: Type,
    pub mutable: bool,
    /// Initializer expression (N2 will define shape; for N1 it's an opaque placeholder).
    pub init: Vec<u8>,
}

/// A data segment.
#[derive(Clone, Debug, PartialEq)]
pub struct DataSegment {
    /// Offset expression (N2 will define shape; for N1 it's opaque).
    pub offset: Vec<u8>,
    /// Data bytes.
    pub data: Vec<u8>,
}

/// An exception tag.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tag {
    pub params: Vec<Type>,
}

/// A memory descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Memory {
    pub min: u32,
    pub max: Option<u32>,
}

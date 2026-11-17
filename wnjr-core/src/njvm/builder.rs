//! NJVM-IR builder: construct functions and modules in memory.

use crate::collections::Vec;

use super::expr::Expr;
use super::module::{Function, Module};
use super::op::{LabelId, Op};
use super::types::Type;

/// A builder for constructing an NJVM-IR function incrementally.
///
/// Usage:
/// 1. `FunctionBuilder::new(params, results, locals)`
/// 2. Call `push_op(...)` to append ops to the body.
/// 3. Use `next_label()` to allocate a fresh label id (or pick your own).
/// 4. Call `build()` to finalize and return the `Function`.
///
/// No verifier is run in N1; that happens in N3. For now, the builder just
/// tracks the body as a `Vec<Op>`.
pub struct FunctionBuilder {
    params: Vec<Type>,
    results: Vec<Type>,
    locals: Vec<Type>,
    body: Vec<Op>,
    next_label_id: LabelId,
}

impl FunctionBuilder {
    /// Create a new function builder with the given signature and locals.
    pub fn new(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>) -> Self {
        FunctionBuilder {
            params,
            results,
            locals,
            body: Vec::new(),
            next_label_id: 0,
        }
    }

    /// Allocate a fresh label id. You can call this before emitting the
    /// `Label` op, then use the returned id in `Jmp`/`JmpIf` targets.
    pub fn next_label(&mut self) -> LabelId {
        let id = self.next_label_id;
        self.next_label_id += 1;
        id
    }

    /// Append an operation to the function body.
    pub fn push_op(&mut self, op: Op) {
        self.body.push(op);
    }

    /// Convenience: push a `Label` op with the given id and params.
    pub fn label(&mut self, id: LabelId, params: Vec<Type>) {
        self.push_op(Op::Label { id, params });
    }

    /// Convenience: push an unconditional jump.
    pub fn jmp(&mut self, target: LabelId) {
        self.push_op(Op::Jmp { target });
    }

    /// Convenience: push a conditional jump.
    pub fn jmp_if(&mut self, cond: Expr, target: LabelId) {
        self.push_op(Op::JmpIf { cond, target });
    }

    /// Finalize and return the constructed function.
    pub fn build(self) -> Function {
        Function {
            params: self.params,
            results: self.results,
            locals: self.locals,
            body: self.body,
        }
    }
}

/// A builder for constructing an NJVM-IR module incrementally.
pub struct ModuleBuilder {
    module: Module,
}

impl ModuleBuilder {
    /// Create a new empty module builder.
    pub fn new() -> Self {
        ModuleBuilder {
            module: Module::new(),
        }
    }

    /// Add a function to the module.
    pub fn add_function(&mut self, func: Function) {
        self.module.functions.push(func);
    }

    /// Finalize and return the constructed module.
    pub fn build(self) -> Module {
        self.module
    }
}

impl Default for ModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

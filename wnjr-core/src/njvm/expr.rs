//! NJVM-IR expressions (group B): tree-form, typed, value-producing nodes.
//!
//! An `Expr` computes a single value. Binary/Unary operands are themselves
//! `Expr` nodes (boxed), so the representation is a tree rather than a stack.

use crate::collections::Vec;
use alloc::boxed::Box;

use super::op::LabelId;
use super::types::{BinaryOp, ConstVal, MemType, Type, UnaryOp};

/// A tree-form expression that produces one value.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A constant literal (group H `Const`).
    Const { ty: Type, val: ConstVal },

    /// Binary operation. `ty` fixes signedness and int-vs-float.
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        ty: Type,
    },

    /// Unary operation.
    Unary {
        op: UnaryOp,
        val: Box<Expr>,
        ty: Type,
    },

    /// Numeric conversion from one type to another.
    Convert {
        from: Type,
        to: Type,
        val: Box<Expr>,
    },

    /// Ternary select: `then_val` if `cond` is nonzero, else `else_val`.
    Select {
        cond: Box<Expr>,
        then_val: Box<Expr>,
        else_val: Box<Expr>,
    },

    /// Memory load. `ptr` is the address expression.
    Load {
        ty: MemType,
        ptr: Box<Expr>,
        offset: u32,
        align: u32,
    },

    /// Read a local variable (group G `LocalGet`).
    LocalGet(u32),

    /// Read a global variable (group G `GlobalGet`).
    GlobalGet(u32),

    /// `LocalTee`: write `val` into local `idx` and yield the value (group G).
    LocalTee { idx: u32, val: Box<Expr> },

    /// Direct call used as an expression yielding its (single) result value.
    Call { func_idx: u32, args: Vec<Expr> },

    /// Indirect call used as an expression yielding its result value.
    CallIndirect {
        type_idx: u32,
        table_idx: u32,
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    /// GNU `&&label` (chibicc `ND_LABEL_VAL`): the address of a label in the
    /// current function, typed `I32`. Pre-lowering marker only:
    /// `computed_goto::lower_function` replaces it with an `I32` constant (the
    /// label's number). Code handed to later passes must not contain it.
    LabelAddr(LabelId),
}

impl Expr {
    /// Convenience: wrap this expression in a `Box`.
    pub fn boxed(self) -> Box<Expr> {
        Box::new(self)
    }

    /// Build a binary expression, boxing the operands.
    pub fn binary(op: BinaryOp, lhs: Expr, rhs: Expr, ty: Type) -> Expr {
        Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            ty,
        }
    }

    /// Build a unary expression, boxing the operand.
    pub fn unary(op: UnaryOp, val: Expr, ty: Type) -> Expr {
        Expr::Unary {
            op,
            val: Box::new(val),
            ty,
        }
    }
}

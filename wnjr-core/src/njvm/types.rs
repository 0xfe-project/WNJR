//! NJVM-IR scalar types and operator kinds.

use serde::{Deserialize, Serialize};
/// NJVM-IR type system: integer widths, floats, and Ptr (equivalent to I32).
///
/// Deliberately a subset: no i128, v128, reference types, or function
/// references. jsval is not represented here. The frontend (N2) splits a jsval
/// into two locals (one F64, one I64) before producing NJVM-IR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Type {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    /// Pointer type, equivalent to I32 in code generation.
    Ptr,
}

/// Memory access width/type for Load and Store (group B).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemType {
    I8,
    U8,
    I16,
    U16,
    I32,
    I64,
    F32,
    F64,
}

/// Binary operators (group B).
///
/// Integer signedness and float-vs-int selection is carried by the `ty` field
/// of `Expr::Binary`, not by the operator itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    // Bitwise / shift
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Rotl,
    Rotr,
    // Comparison
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    // Float-specific
    Min,
    Max,
    Copysign,
}

/// Unary operators (group B).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
    Abs,
    Sqrt,
    Ceil,
    Floor,
    Trunc,
    Nearest,
    Clz,
    Ctz,
    Popcnt,
}

/// Atomic operation kind for `Op::Atomic` (group G).
///
/// The full operand shape (pointer, value, memory ordering) is defined in a
/// later task; N1 keeps the operator, width, and offset per the IR sketch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AtomicOp {
    Load,
    Store,
    Add,
    Sub,
    And,
    Or,
    Xor,
    Xchg,
    Cmpxchg,
    Wait,
    Notify,
    Fence,
}

/// A constant literal (group H `Const`).
///
/// Floats are stored as their raw bit patterns so the enum can derive `Eq`
/// and `Hash` and so NaN payloads round-trip exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConstVal {
    I32(i32),
    I64(i64),
    /// F32 value as raw IEEE-754 bits.
    F32(u32),
    /// F64 value as raw IEEE-754 bits.
    F64(u64),
}

impl ConstVal {
    /// Build an `F32` constant from a float value.
    pub fn f32(v: f32) -> Self {
        ConstVal::F32(v.to_bits())
    }

    /// Build an `F64` constant from a float value.
    pub fn f64(v: f64) -> Self {
        ConstVal::F64(v.to_bits())
    }
}

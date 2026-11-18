//! N5 type erasure: tree-form NJVM-IR `Expr` -> wasm stack instructions.
//!
//! [`lower_function`] walks a function body and emits wasm instructions in
//! post-order (operands before the operator). The NJVM `Type` on each node
//! picks the wasm instruction:
//!
//! - I8/U8/I16/U16/I32/U32/Ptr -> the i32 family, I64/U64 -> i64, F32/F64 -> f32/f64.
//! - Div/Rem/Shr/Lt/Le/Gt/Ge and Convert pick the signed or unsigned form
//!   from the type: U8/U16/U32/U64/Ptr are unsigned, I8/I16/I32/I64 signed.
//! - Min/Max/Copysign and Sqrt/Ceil/Floor/Trunc/Nearest only exist for floats;
//!   Rem, bit ops, shifts, rotates, Not, Clz/Ctz/Popcnt only for integers.
//!
//! # Narrow types
//!
//! A narrow value (I8/U8/I16/U16) can only be produced by `Load` (load8_s,
//! load8_u, load16_s, load16_u) or `Convert`, and only be consumed by
//! `Convert`, `Store`, `Drop`, or as a condition. On the wasm stack it is an
//! i32 already normalized to its width (sign-extended for I8/I16,
//! zero-extended for U8/U16). Anywhere else (Binary, Unary, Select operands,
//! Const, locals, signatures) a narrow type is an error.
//!
//! # Scope
//!
//! Only straight-line bodies are lowered. A body containing `Jmp`, `JmpIf` or
//! `BrTable` is rejected with [`EraseErrorKind::NeedsStackify`] (N6's job).
//! A `Label` with no jumps in the function (the builder's entry label) emits
//! nothing. Structured control, EH (except `Throw`), coroutines and atomics
//! are rejected with [`EraseErrorKind::Unsupported`].
//!
//! # Choices this pass makes
//!
//! - `Not` is bitwise complement (`x ^ -1`), since it keeps the operand type.
//!   C `!x` is `Eq(x, 0)`.
//! - Integer `Neg` is `0 - x`. Signed integer `Abs` is `(x ^ (x >> w-1)) - (x >> w-1)`
//!   through a scratch local appended after the declared locals; unsigned `Abs` is
//!   the identity.
//! - Float -> int `Convert` uses the trapping `trunc` (out of range is UB in C).
//!   A narrow target truncates to i32 signed, then normalizes.
//! - `Select` evaluates then, else, cond (wasm operand order), not field order.
//!   `CallIndirect` evaluates args before the callee.
//! - A function with results always gets a trailing `unreachable` before `end`,
//!   so a body that falls off the end traps instead of failing validation.
//!
//! The pass checks types itself and returns an error rather than panicking
//! on input the N3 verifier would have rejected.

use wasm_encoder::{Ieee32, Ieee64, InstructionSink, MemArg, ValType};

use crate::collections::Vec;

use super::expr::Expr;
use super::module::{Function, ImportKind, Module};
use super::op::Op;
use super::types::{BinaryOp, ConstVal, MemType, Type, UnaryOp};

/// Why a function body could not be lowered. `at` is the index of the
/// top-level op in `Function::body` (for `NarrowLocal`, `None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EraseError {
    pub at: Option<usize>,
    pub kind: EraseErrorKind,
}

/// What went wrong in [`lower_function`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EraseErrorKind {
    /// The body has `Jmp`/`JmpIf`/`BrTable`; it needs the stackify pass (N6).
    NeedsStackify,
    /// An op this straight-line pass does not lower (structured control etc.).
    Unsupported { op: &'static str },
    /// `Expr::LabelAddr` or `Op::GotoIndirect` is still present.
    UnloweredComputedGoto,
    /// A local declared with a narrow type; wasm has no such local type.
    NarrowLocal { idx: u32, ty: Type },
    /// A narrow type where a full wasm value is required (Binary/Unary/Select
    /// operand, Const, call argument, ...).
    NarrowValue { ty: Type },
    /// The operator is not defined for this type (e.g. Sqrt on I32).
    IllegalBinary { op: BinaryOp, ty: Type },
    IllegalUnary { op: UnaryOp, ty: Type },
    /// An operand has type `got` where `expected` was needed.
    TypeMismatch { expected: Type, got: Type },
    /// A store/address/condition operand of the wrong wasm class.
    ClassMismatch { expected: ValType, got: Type },
    /// A constant whose value does not fit its declared type.
    ConstMismatch { ty: Type, val: ConstVal },
    LocalOutOfRange { idx: u32 },
    GlobalOutOfRange { idx: u32 },
    FuncOutOfRange { idx: u32 },
    TypeOutOfRange { idx: u32 },
    TagOutOfRange { idx: u32 },
    /// Call/throw/return values do not match the signature.
    ArityMismatch { expected: usize, got: usize },
    /// A call used as an expression must have exactly one result.
    CallResultArity { results: usize },
}

impl core::fmt::Display for EraseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.at {
            Some(i) => write!(f, "op {i}: {:?}", self.kind),
            None => write!(f, "{:?}", self.kind),
        }
    }
}

/// Module-level index spaces the pass needs (imports first).
pub struct Env<'m> {
    funcs: Vec<(&'m [Type], &'m [Type])>,
    module: &'m Module,
    globals: Vec<Type>,
    tags: Vec<&'m [Type]>,
}

impl<'m> Env<'m> {
    pub fn new(module: &'m Module) -> Self {
        let mut funcs = Vec::new();
        let mut globals = Vec::new();
        let mut tags = Vec::new();
        for imp in &module.imports {
            match &imp.kind {
                ImportKind::Function(t) => match module.types.get(*t as usize) {
                    Some(ft) => funcs.push((ft.params.as_slice(), ft.results.as_slice())),
                    // An import of a missing type: keep the index space aligned
                    // with an empty signature; calls to it fail on arity.
                    None => funcs.push((&[][..], &[][..])),
                },
                ImportKind::Global(g) => globals.push(g.ty),
                ImportKind::Tag(t) => tags.push(t.params.as_slice()),
                _ => {}
            }
        }
        for f in &module.functions {
            funcs.push((f.params.as_slice(), f.results.as_slice()));
        }
        globals.extend(module.globals.iter().map(|g| g.ty.ty));
        tags.extend(module.tags.iter().map(|t| t.params.as_slice()));
        Env { funcs, module, globals, tags }
    }
}

/// A lowered function: its extra locals (declared locals, then scratch) and
/// the instruction bytes including the final `end`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lowered {
    pub locals: Vec<ValType>,
    pub code: Vec<u8>,
}

fn is_narrow(t: Type) -> bool {
    matches!(t, Type::I8 | Type::U8 | Type::I16 | Type::U16)
}

fn is_float(t: Type) -> bool {
    matches!(t, Type::F32 | Type::F64)
}

fn is_unsigned(t: Type) -> bool {
    matches!(t, Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::Ptr)
}

/// The wasm value class of any NJVM type (narrow types are i32 on the stack).
fn class(t: Type) -> ValType {
    match t {
        Type::I64 | Type::U64 => ValType::I64,
        Type::F32 => ValType::F32,
        Type::F64 => ValType::F64,
        _ => ValType::I32,
    }
}

/// The wasm type of a non-narrow NJVM type.
fn wide(t: Type) -> Result<ValType, EraseErrorKind> {
    if is_narrow(t) {
        Err(EraseErrorKind::NarrowValue { ty: t })
    } else {
        Ok(class(t))
    }
}

fn memarg(offset: u32, align: u32) -> MemArg {
    MemArg { offset: offset as u64, align, memory_index: 0 }
}

/// Does any op (at any depth) jump?
fn has_jumps(body: &[Op]) -> bool {
    body.iter().any(|op| match op {
        Op::Jmp { .. } | Op::JmpIf { .. } | Op::BrTable { .. } => true,
        Op::Block { body } | Op::Loop { body } => has_jumps(body),
        Op::If { then_body, else_body, .. } => has_jumps(then_body) || has_jumps(else_body),
        Op::Switch { cases, default, .. } => {
            cases.iter().any(|(_, b)| has_jumps(b)) || has_jumps(default)
        }
        Op::TypeSwitch { cases, .. } => cases.iter().any(|(_, b)| has_jumps(b)),
        Op::TryTable { body, catches } => {
            has_jumps(body) || catches.iter().any(|(_, b)| has_jumps(b))
        }
        Op::Await { fallback, .. } => has_jumps(fallback),
        _ => false,
    })
}

struct Cx<'a, 'm> {
    env: &'a Env<'m>,
    func: &'a Function,
    /// params.len() + locals.len(): the first scratch index.
    declared: u32,
    scratch_i32: Option<u32>,
    scratch_i64: Option<u32>,
    extra: Vec<ValType>,
}

/// Lower one function body. `env` is built once per module with [`Env::new`].
pub fn lower_function(env: &Env<'_>, func: &Function) -> Result<Lowered, EraseError> {
    let mut locals = Vec::with_capacity(func.locals.len());
    for (i, &t) in func.locals.iter().enumerate() {
        let idx = (func.params.len() + i) as u32;
        locals.push(wide(t).map_err(|_| EraseError {
            at: None,
            kind: EraseErrorKind::NarrowLocal { idx, ty: t },
        })?);
    }
    if has_jumps(&func.body) {
        return Err(EraseError { at: None, kind: EraseErrorKind::NeedsStackify });
    }
    let mut cx = Cx {
        env,
        func,
        declared: (func.params.len() + func.locals.len()) as u32,
        scratch_i32: None,
        scratch_i64: None,
        extra: Vec::new(),
    };
    let mut code: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    {
        let mut sink = InstructionSink::new(&mut code);
        for (i, op) in func.body.iter().enumerate() {
            cx.op(&mut sink, op).map_err(|kind| EraseError { at: Some(i), kind })?;
        }
        if !func.results.is_empty() {
            sink.unreachable();
        }
        sink.end();
    }
    locals.extend(cx.extra.iter().copied());
    Ok(Lowered { locals, code })
}

impl Cx<'_, '_> {
    fn scratch(&mut self, vt: ValType) -> u32 {
        let slot = if vt == ValType::I64 { &mut self.scratch_i64 } else { &mut self.scratch_i32 };
        if let Some(i) = *slot {
            return i;
        }
        let i = self.declared + self.extra.len() as u32;
        *slot = Some(i);
        self.extra.push(vt);
        i
    }

    fn local(&self, idx: u32) -> Result<Type, EraseErrorKind> {
        let p = self.func.params.len();
        let i = idx as usize;
        if i < p {
            Ok(self.func.params[i])
        } else {
            self.func.locals.get(i - p).copied().ok_or(EraseErrorKind::LocalOutOfRange { idx })
        }
    }

    fn global(&self, idx: u32) -> Result<Type, EraseErrorKind> {
        self.env.globals.get(idx as usize).copied().ok_or(EraseErrorKind::GlobalOutOfRange { idx })
    }

    /// Lower `e` and require exactly type `t`.
    fn expect(&mut self, s: &mut InstructionSink<'_>, e: &Expr, t: Type) -> Result<(), EraseErrorKind> {
        let got = self.expr(s, e)?;
        if got != t {
            return Err(EraseErrorKind::TypeMismatch { expected: t, got });
        }
        Ok(())
    }

    /// Lower `e` and require its wasm class to be `vt` (narrow allowed for i32).
    fn expect_class(&mut self, s: &mut InstructionSink<'_>, e: &Expr, vt: ValType) -> Result<Type, EraseErrorKind> {
        let got = self.expr(s, e)?;
        if class(got) != vt {
            return Err(EraseErrorKind::ClassMismatch { expected: vt, got });
        }
        Ok(got)
    }

    /// Lower a condition: any integer type, left on the stack as an i32 truth value.
    fn cond(&mut self, s: &mut InstructionSink<'_>, e: &Expr) -> Result<(), EraseErrorKind> {
        let t = self.expr(s, e)?;
        match class(t) {
            ValType::I32 => Ok(()),
            ValType::I64 => {
                s.i64_const(0).i64_ne();
                Ok(())
            }
            _ => Err(EraseErrorKind::ClassMismatch { expected: ValType::I32, got: t }),
        }
    }

    fn args(&mut self, s: &mut InstructionSink<'_>, args: &[Expr], params: &[Type]) -> Result<(), EraseErrorKind> {
        if args.len() != params.len() {
            return Err(EraseErrorKind::ArityMismatch { expected: params.len(), got: args.len() });
        }
        for (a, &p) in args.iter().zip(params) {
            self.expect(s, a, p)?;
        }
        Ok(())
    }

    fn func_sig(&self, idx: u32) -> Result<(&[Type], &[Type]), EraseErrorKind> {
        self.env.funcs.get(idx as usize).copied().ok_or(EraseErrorKind::FuncOutOfRange { idx })
    }

    fn type_sig(&self, idx: u32) -> Result<(&[Type], &[Type]), EraseErrorKind> {
        self.env
            .module
            .types
            .get(idx as usize)
            .map(|ft| (ft.params.as_slice(), ft.results.as_slice()))
            .ok_or(EraseErrorKind::TypeOutOfRange { idx })
    }

    /// Emit a direct call; returns the result types.
    fn call(&mut self, s: &mut InstructionSink<'_>, func_idx: u32, args: &[Expr]) -> Result<Vec<Type>, EraseErrorKind> {
        let (params, results) = self.func_sig(func_idx)?;
        let (params, results) = (params.to_vec(), results.to_vec());
        self.args(s, args, &params)?;
        s.call(func_idx);
        Ok(results)
    }

    fn call_indirect(
        &mut self,
        s: &mut InstructionSink<'_>,
        type_idx: u32,
        table_idx: u32,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<Vec<Type>, EraseErrorKind> {
        let (params, results) = self.type_sig(type_idx)?;
        let (params, results) = (params.to_vec(), results.to_vec());
        self.args(s, args, &params)?;
        let t = self.expr(s, callee)?;
        if is_narrow(t) || class(t) != ValType::I32 {
            return Err(EraseErrorKind::ClassMismatch { expected: ValType::I32, got: t });
        }
        s.call_indirect(table_idx, type_idx);
        Ok(results)
    }

    /// Lower an expression in post-order; returns its NJVM type.
    fn expr(&mut self, s: &mut InstructionSink<'_>, e: &Expr) -> Result<Type, EraseErrorKind> {
        match e {
            Expr::Const { ty, val } => {
                wide(*ty)?;
                match (class(*ty), *val) {
                    (ValType::I32, ConstVal::I32(v)) => s.i32_const(v),
                    (ValType::I64, ConstVal::I64(v)) => s.i64_const(v),
                    (ValType::F32, ConstVal::F32(b)) => s.f32_const(Ieee32::from(f32::from_bits(b))),
                    (ValType::F64, ConstVal::F64(b)) => s.f64_const(Ieee64::from(f64::from_bits(b))),
                    _ => return Err(EraseErrorKind::ConstMismatch { ty: *ty, val: *val }),
                };
                Ok(*ty)
            }
            Expr::Binary { op, lhs, rhs, ty } => self.binary(s, *op, lhs, rhs, *ty),
            Expr::Unary { op, val, ty } => self.unary(s, *op, val, *ty),
            Expr::Convert { from, to, val } => {
                self.expect(s, val, *from)?;
                convert(s, *from, *to);
                Ok(*to)
            }
            Expr::Select { cond, then_val, else_val } => {
                let t = self.expr(s, then_val)?;
                wide(t)?;
                self.expect(s, else_val, t)?;
                self.cond(s, cond)?;
                s.select();
                Ok(t)
            }
            Expr::Load { ty, ptr, offset, align } => {
                let pt = self.expr(s, ptr)?;
                if !matches!(pt, Type::Ptr | Type::I32 | Type::U32) {
                    return Err(EraseErrorKind::ClassMismatch { expected: ValType::I32, got: pt });
                }
                let m = memarg(*offset, *align);
                Ok(match ty {
                    MemType::I8 => { s.i32_load8_s(m); Type::I8 }
                    MemType::U8 => { s.i32_load8_u(m); Type::U8 }
                    MemType::I16 => { s.i32_load16_s(m); Type::I16 }
                    MemType::U16 => { s.i32_load16_u(m); Type::U16 }
                    MemType::I32 => { s.i32_load(m); Type::I32 }
                    MemType::I64 => { s.i64_load(m); Type::I64 }
                    MemType::F32 => { s.f32_load(m); Type::F32 }
                    MemType::F64 => { s.f64_load(m); Type::F64 }
                })
            }
            Expr::LocalGet(idx) => {
                let t = self.local(*idx)?;
                s.local_get(*idx);
                Ok(t)
            }
            Expr::GlobalGet(idx) => {
                let t = self.global(*idx)?;
                s.global_get(*idx);
                Ok(t)
            }
            Expr::LocalTee { idx, val } => {
                let t = self.local(*idx)?;
                self.expect(s, val, t)?;
                s.local_tee(*idx);
                Ok(t)
            }
            Expr::Call { func_idx, args } => {
                let r = self.call(s, *func_idx, args)?;
                single(&r)
            }
            Expr::CallIndirect { type_idx, table_idx, callee, args } => {
                let r = self.call_indirect(s, *type_idx, *table_idx, callee, args)?;
                single(&r)
            }
            Expr::LabelAddr(_) => Err(EraseErrorKind::UnloweredComputedGoto),
        }
    }

    fn binary(
        &mut self,
        s: &mut InstructionSink<'_>,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        ty: Type,
    ) -> Result<Type, EraseErrorKind> {
        use BinaryOp::*;
        let vt = wide(ty)?;
        let legal = match op {
            Add | Sub | Mul | Div | Lt | Le | Gt | Ge | Eq | Ne => true,
            Rem | And | Or | Xor | Shl | Shr | Rotl | Rotr => !is_float(ty),
            Min | Max | Copysign => is_float(ty),
        };
        if !legal {
            return Err(EraseErrorKind::IllegalBinary { op, ty });
        }
        self.expect(s, lhs, ty)?;
        self.expect(s, rhs, ty)?;
        let u = is_unsigned(ty);
        match vt {
            ValType::I32 => match op {
                Add => s.i32_add(),
                Sub => s.i32_sub(),
                Mul => s.i32_mul(),
                Div => if u { s.i32_div_u() } else { s.i32_div_s() },
                Rem => if u { s.i32_rem_u() } else { s.i32_rem_s() },
                And => s.i32_and(),
                Or => s.i32_or(),
                Xor => s.i32_xor(),
                Shl => s.i32_shl(),
                Shr => if u { s.i32_shr_u() } else { s.i32_shr_s() },
                Rotl => s.i32_rotl(),
                Rotr => s.i32_rotr(),
                Lt => if u { s.i32_lt_u() } else { s.i32_lt_s() },
                Le => if u { s.i32_le_u() } else { s.i32_le_s() },
                Gt => if u { s.i32_gt_u() } else { s.i32_gt_s() },
                Ge => if u { s.i32_ge_u() } else { s.i32_ge_s() },
                Eq => s.i32_eq(),
                Ne => s.i32_ne(),
                Min | Max | Copysign => unreachable!("checked above"),
            },
            ValType::I64 => match op {
                Add => s.i64_add(),
                Sub => s.i64_sub(),
                Mul => s.i64_mul(),
                Div => if u { s.i64_div_u() } else { s.i64_div_s() },
                Rem => if u { s.i64_rem_u() } else { s.i64_rem_s() },
                And => s.i64_and(),
                Or => s.i64_or(),
                Xor => s.i64_xor(),
                Shl => s.i64_shl(),
                Shr => if u { s.i64_shr_u() } else { s.i64_shr_s() },
                Rotl => s.i64_rotl(),
                Rotr => s.i64_rotr(),
                Lt => if u { s.i64_lt_u() } else { s.i64_lt_s() },
                Le => if u { s.i64_le_u() } else { s.i64_le_s() },
                Gt => if u { s.i64_gt_u() } else { s.i64_gt_s() },
                Ge => if u { s.i64_ge_u() } else { s.i64_ge_s() },
                Eq => s.i64_eq(),
                Ne => s.i64_ne(),
                Min | Max | Copysign => unreachable!("checked above"),
            },
            ValType::F32 => match op {
                Add => s.f32_add(),
                Sub => s.f32_sub(),
                Mul => s.f32_mul(),
                Div => s.f32_div(),
                Min => s.f32_min(),
                Max => s.f32_max(),
                Copysign => s.f32_copysign(),
                Lt => s.f32_lt(),
                Le => s.f32_le(),
                Gt => s.f32_gt(),
                Ge => s.f32_ge(),
                Eq => s.f32_eq(),
                Ne => s.f32_ne(),
                _ => unreachable!("checked above"),
            },
            _ => match op {
                Add => s.f64_add(),
                Sub => s.f64_sub(),
                Mul => s.f64_mul(),
                Div => s.f64_div(),
                Min => s.f64_min(),
                Max => s.f64_max(),
                Copysign => s.f64_copysign(),
                Lt => s.f64_lt(),
                Le => s.f64_le(),
                Gt => s.f64_gt(),
                Ge => s.f64_ge(),
                Eq => s.f64_eq(),
                Ne => s.f64_ne(),
                _ => unreachable!("checked above"),
            },
        };
        Ok(if matches!(op, Lt | Le | Gt | Ge | Eq | Ne) { Type::I32 } else { ty })
    }

    fn unary(&mut self, s: &mut InstructionSink<'_>, op: UnaryOp, val: &Expr, ty: Type) -> Result<Type, EraseErrorKind> {
        use UnaryOp::*;
        let vt = wide(ty)?;
        let legal = match op {
            Neg | Abs => true,
            Not | Clz | Ctz | Popcnt => !is_float(ty),
            Sqrt | Ceil | Floor | Trunc | Nearest => is_float(ty),
        };
        if !legal {
            return Err(EraseErrorKind::IllegalUnary { op, ty });
        }
        match (vt, op) {
            (ValType::I32, Neg) => {
                s.i32_const(0);
                self.expect(s, val, ty)?;
                s.i32_sub();
            }
            (ValType::I64, Neg) => {
                s.i64_const(0);
                self.expect(s, val, ty)?;
                s.i64_sub();
            }
            (ValType::I32 | ValType::I64, Abs) if is_unsigned(ty) => {
                self.expect(s, val, ty)?;
            }
            (ValType::I32, Abs) => {
                // (x ^ m) - m, m = x >> 31
                self.expect(s, val, ty)?;
                let x = self.scratch(ValType::I32);
                s.local_tee(x).local_get(x).i32_const(31).i32_shr_s().i32_xor();
                s.local_get(x).i32_const(31).i32_shr_s().i32_sub();
            }
            (ValType::I64, Abs) => {
                self.expect(s, val, ty)?;
                let x = self.scratch(ValType::I64);
                s.local_tee(x).local_get(x).i64_const(63).i64_shr_s().i64_xor();
                s.local_get(x).i64_const(63).i64_shr_s().i64_sub();
            }
            (ValType::I32, _) => {
                self.expect(s, val, ty)?;
                match op {
                    Not => s.i32_const(-1).i32_xor(),
                    Clz => s.i32_clz(),
                    Ctz => s.i32_ctz(),
                    Popcnt => s.i32_popcnt(),
                    _ => unreachable!("checked above"),
                };
            }
            (ValType::I64, _) => {
                self.expect(s, val, ty)?;
                match op {
                    Not => s.i64_const(-1).i64_xor(),
                    Clz => s.i64_clz(),
                    Ctz => s.i64_ctz(),
                    Popcnt => s.i64_popcnt(),
                    _ => unreachable!("checked above"),
                };
            }
            (ValType::F32, _) => {
                self.expect(s, val, ty)?;
                match op {
                    Neg => s.f32_neg(),
                    Abs => s.f32_abs(),
                    Sqrt => s.f32_sqrt(),
                    Ceil => s.f32_ceil(),
                    Floor => s.f32_floor(),
                    Trunc => s.f32_trunc(),
                    Nearest => s.f32_nearest(),
                    _ => unreachable!("checked above"),
                };
            }
            (_, _) => {
                self.expect(s, val, ty)?;
                match op {
                    Neg => s.f64_neg(),
                    Abs => s.f64_abs(),
                    Sqrt => s.f64_sqrt(),
                    Ceil => s.f64_ceil(),
                    Floor => s.f64_floor(),
                    Trunc => s.f64_trunc(),
                    Nearest => s.f64_nearest(),
                    _ => unreachable!("checked above"),
                };
            }
        }
        Ok(ty)
    }

    fn op(&mut self, s: &mut InstructionSink<'_>, op: &Op) -> Result<(), EraseErrorKind> {
        match op {
            // Only reached when the function has no jumps: an unused label.
            Op::Label { .. } | Op::Nop => {}
            Op::Jmp { .. } | Op::JmpIf { .. } | Op::BrTable { .. } => {
                return Err(EraseErrorKind::NeedsStackify)
            }
            Op::GotoIndirect { .. } => return Err(EraseErrorKind::UnloweredComputedGoto),
            Op::Store { ty, ptr, val, offset, align } => {
                let pt = self.expr(s, ptr)?;
                if !matches!(pt, Type::Ptr | Type::I32 | Type::U32) {
                    return Err(EraseErrorKind::ClassMismatch { expected: ValType::I32, got: pt });
                }
                let m = memarg(*offset, *align);
                match ty {
                    MemType::I8 | MemType::U8 => {
                        self.expect_class(s, val, ValType::I32)?;
                        s.i32_store8(m);
                    }
                    MemType::I16 | MemType::U16 => {
                        self.expect_class(s, val, ValType::I32)?;
                        s.i32_store16(m);
                    }
                    MemType::I32 => {
                        self.expect_class(s, val, ValType::I32)?;
                        s.i32_store(m);
                    }
                    MemType::I64 => {
                        self.expect_class(s, val, ValType::I64)?;
                        s.i64_store(m);
                    }
                    MemType::F32 => {
                        self.expect_class(s, val, ValType::F32)?;
                        s.f32_store(m);
                    }
                    MemType::F64 => {
                        self.expect_class(s, val, ValType::F64)?;
                        s.f64_store(m);
                    }
                }
            }
            Op::Return { vals } => {
                let results = self.func.results.clone();
                self.args(s, vals, &results)?;
                s.return_();
            }
            Op::Unreachable => {
                s.unreachable();
            }
            Op::Call { func_idx, args } => {
                let r = self.call(s, *func_idx, args)?;
                for _ in &r {
                    s.drop();
                }
            }
            Op::CallIndirect { type_idx, table_idx, callee, args } => {
                let r = self.call_indirect(s, *type_idx, *table_idx, callee, args)?;
                for _ in &r {
                    s.drop();
                }
            }
            Op::Throw { tag_idx, args } => {
                let params = self
                    .env
                    .tags
                    .get(*tag_idx as usize)
                    .ok_or(EraseErrorKind::TagOutOfRange { idx: *tag_idx })?
                    .to_vec();
                self.args(s, args, &params)?;
                s.throw(*tag_idx);
            }
            Op::LocalSet { idx, val } => {
                let t = self.local(*idx)?;
                self.expect(s, val, t)?;
                s.local_set(*idx);
            }
            Op::GlobalSet { idx, val } => {
                let t = self.global(*idx)?;
                self.expect(s, val, t)?;
                s.global_set(*idx);
            }
            Op::MemoryCopy { dest, src, len } => {
                self.expect_class(s, dest, ValType::I32)?;
                self.expect_class(s, src, ValType::I32)?;
                self.expect_class(s, len, ValType::I32)?;
                s.memory_copy(0, 0);
            }
            Op::MemoryFill { dest, val, len } => {
                self.expect_class(s, dest, ValType::I32)?;
                self.expect_class(s, val, ValType::I32)?;
                self.expect_class(s, len, ValType::I32)?;
                s.memory_fill(0);
            }
            Op::MemorySize => {
                s.memory_size(0).drop();
            }
            Op::MemoryGrow { delta } => {
                self.expect_class(s, delta, ValType::I32)?;
                s.memory_grow(0).drop();
            }
            Op::Drop { val } => {
                self.expr(s, val)?;
                s.drop();
            }
            Op::Const { .. } => return Err(EraseErrorKind::Unsupported { op: "Const" }),
            Op::Block { .. } => return Err(EraseErrorKind::Unsupported { op: "Block" }),
            Op::Loop { .. } => return Err(EraseErrorKind::Unsupported { op: "Loop" }),
            Op::If { .. } => return Err(EraseErrorKind::Unsupported { op: "If" }),
            Op::Switch { .. } => return Err(EraseErrorKind::Unsupported { op: "Switch" }),
            Op::TypeSwitch { .. } => return Err(EraseErrorKind::Unsupported { op: "TypeSwitch" }),
            Op::TryTable { .. } => return Err(EraseErrorKind::Unsupported { op: "TryTable" }),
            Op::Setjmp { .. } => return Err(EraseErrorKind::Unsupported { op: "Setjmp" }),
            Op::Longjmp { .. } => return Err(EraseErrorKind::Unsupported { op: "Longjmp" }),
            Op::Await { .. } => return Err(EraseErrorKind::Unsupported { op: "Await" }),
            Op::Yield { .. } => return Err(EraseErrorKind::Unsupported { op: "Yield" }),
            Op::Atomic { .. } => return Err(EraseErrorKind::Unsupported { op: "Atomic" }),
        }
        Ok(())
    }
}

fn single(results: &[Type]) -> Result<Type, EraseErrorKind> {
    match results {
        [t] => Ok(*t),
        _ => Err(EraseErrorKind::CallResultArity { results: results.len() }),
    }
}

/// Bring an i32 holding any value down to the normalized form of narrow `to`.
fn normalize(s: &mut InstructionSink<'_>, to: Type) {
    match to {
        Type::I8 => { s.i32_extend8_s(); }
        Type::I16 => { s.i32_extend16_s(); }
        Type::U8 => { s.i32_const(0xff).i32_and(); }
        Type::U16 => { s.i32_const(0xffff).i32_and(); }
        _ => {}
    }
}

/// Emit the conversion `from` -> `to` for a value already on the stack.
/// Every (from, to) pair of the 11 types is defined.
fn convert(s: &mut InstructionSink<'_>, from: Type, to: Type) {
    let (cf, ct) = (class(from), class(to));
    match (cf, ct) {
        (ValType::I32, ValType::I32) => {
            // Same representation. Only narrowing to a smaller (or
            // differently signed) narrow type changes bits.
            if is_narrow(to) && from != to {
                normalize(s, to);
            }
        }
        (ValType::I64, ValType::I32) => {
            s.i32_wrap_i64();
            normalize(s, to);
        }
        (ValType::I32, ValType::I64) => {
            // Narrow sources are already normalized, so extending the i32
            // by the source's signedness gives the right value.
            if is_unsigned(from) { s.i64_extend_i32_u(); } else { s.i64_extend_i32_s(); }
        }
        (ValType::I64, ValType::I64) => {}
        (ValType::I32, ValType::F32) => {
            if is_unsigned(from) { s.f32_convert_i32_u(); } else { s.f32_convert_i32_s(); }
        }
        (ValType::I32, ValType::F64) => {
            if is_unsigned(from) { s.f64_convert_i32_u(); } else { s.f64_convert_i32_s(); }
        }
        (ValType::I64, ValType::F32) => {
            if is_unsigned(from) { s.f32_convert_i64_u(); } else { s.f32_convert_i64_s(); }
        }
        (ValType::I64, ValType::F64) => {
            if is_unsigned(from) { s.f64_convert_i64_u(); } else { s.f64_convert_i64_s(); }
        }
        (ValType::F32, ValType::I32) => {
            if is_narrow(to) {
                s.i32_trunc_f32_s();
                normalize(s, to);
            } else if is_unsigned(to) {
                s.i32_trunc_f32_u();
            } else {
                s.i32_trunc_f32_s();
            }
        }
        (ValType::F64, ValType::I32) => {
            if is_narrow(to) {
                s.i32_trunc_f64_s();
                normalize(s, to);
            } else if is_unsigned(to) {
                s.i32_trunc_f64_u();
            } else {
                s.i32_trunc_f64_s();
            }
        }
        (ValType::F32, ValType::I64) => {
            if is_unsigned(to) { s.i64_trunc_f32_u(); } else { s.i64_trunc_f32_s(); }
        }
        (ValType::F64, ValType::I64) => {
            if is_unsigned(to) { s.i64_trunc_f64_u(); } else { s.i64_trunc_f64_s(); }
        }
        (ValType::F32, ValType::F64) => { s.f64_promote_f32(); }
        (ValType::F64, ValType::F32) => { s.f32_demote_f64(); }
        _ => {}
    }
}

#[cfg(test)]
mod tests;

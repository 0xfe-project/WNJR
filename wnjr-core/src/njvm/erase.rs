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
//! # N7 lowerings
//!
//! - `Switch` -> `br_table` when dense, `eq`/`br_if` chain when sparse (see
//!   `Cx::switch` for the layout and the threshold). `TypeSwitch` is
//!   `NotImplemented` (no reference types in NJVM-IR, so nothing consumes it).
//! - Memory addresses (Load/Store/MemoryCopy/MemoryFill): an `I64`/`U64`
//!   address is a gc-tagged LP64 pointer (#634) and gets `i32.wrap_i64`
//!   right before the access; `offset`/`align` go into the memarg unchanged.
//! - Call/CallIndirect: args left to right, then (indirect) the callee;
//!   `call_indirect` uses the op's `table_idx` (0 for C) and `type_idx`.
//! - `Atomic { op: Fence }` -> `atomic.fence`; other atomics are
//!   `NotImplemented` until `Op::Atomic` carries operands.
//!
//! The pass checks types itself and returns an error rather than panicking
//! on input the N3 verifier would have rejected.

use wasm_encoder::{
    BlockType, Catch as WasmCatch, Ieee32, Ieee64, InstructionSink, MemArg, ValType,
};

use crate::collections::Vec;

use super::expr::Expr;
use super::module::{Function, ImportKind, Module};
use super::op::{Catch, Op};
use super::stackify::{stackify, Node, StackifyError};
use super::types::{AtomicOp, BinaryOp, ConstVal, MemType, Type, UnaryOp};

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
    /// Stackify (N6) could not restructure the body's flat CFG into
    /// structured control flow.
    Stackify(StackifyError),
    /// An op this straight-line pass does not lower (structured control etc.).
    Unsupported {
        op: &'static str,
    },
    /// An op whose lowering is deliberately not written in this version;
    /// `reason` says why (no consumer yet, operand shape undefined, ...).
    NotImplemented {
        op: &'static str,
        reason: &'static str,
    },
    /// `Expr::LabelAddr` or `Op::GotoIndirect` is still present.
    UnloweredComputedGoto,
    /// A local declared with a narrow type; wasm has no such local type.
    NarrowLocal {
        idx: u32,
        ty: Type,
    },
    /// A narrow type where a full wasm value is required (Binary/Unary/Select
    /// operand, Const, call argument, ...).
    NarrowValue {
        ty: Type,
    },
    /// The operator is not defined for this type (e.g. Sqrt on I32).
    IllegalBinary {
        op: BinaryOp,
        ty: Type,
    },
    IllegalUnary {
        op: UnaryOp,
        ty: Type,
    },
    /// An operand has type `got` where `expected` was needed.
    TypeMismatch {
        expected: Type,
        got: Type,
    },
    /// A store/address/condition operand of the wrong wasm class.
    ClassMismatch {
        expected: ValType,
        got: Type,
    },
    /// A constant whose value does not fit its declared type.
    ConstMismatch {
        ty: Type,
        val: ConstVal,
    },
    LocalOutOfRange {
        idx: u32,
    },
    GlobalOutOfRange {
        idx: u32,
    },
    FuncOutOfRange {
        idx: u32,
    },
    TypeOutOfRange {
        idx: u32,
    },
    TagOutOfRange {
        idx: u32,
    },
    /// Call/throw/return values do not match the signature.
    ArityMismatch {
        expected: usize,
        got: usize,
    },
    /// A call used as an expression must have exactly one result.
    CallResultArity {
        results: usize,
    },
    /// An atomic op missing an operand it needs, or carrying one it does not.
    AtomicShape {
        op: AtomicOp,
    },
    /// An access width wasm has no atomic instruction for.
    AtomicWidth {
        width: u8,
    },
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
    /// Per tag (imports first): the wasm type index of `[] -> params`, needed
    /// as the block type of a catch target when the tag has 2+ params.
    tag_blocks: Vec<Option<u32>>,
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
        Env {
            funcs,
            module,
            globals,
            tags,
            tag_blocks: Vec::new(),
        }
    }

    /// Supply the block type indices of multi-value tags (see `tag_blocks`).
    /// `encode` interns them; without this a catch of a 2+ param tag is
    /// `NotImplemented`.
    pub fn with_tag_block_types(mut self, tag_blocks: Vec<Option<u32>>) -> Self {
        self.tag_blocks = tag_blocks;
        self
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
    MemArg {
        offset: offset as u64,
        align,
        memory_index: 0,
    }
}

/// Does any op (at any depth) jump?
fn has_jumps(body: &[Op]) -> bool {
    body.iter().any(|op| match op {
        Op::Jmp { .. } | Op::JmpIf { .. } | Op::BrTable { .. } => true,
        Op::Block { body } | Op::Loop { body } => has_jumps(body),
        Op::If {
            then_body,
            else_body,
            ..
        } => has_jumps(then_body) || has_jumps(else_body),
        Op::Switch { cases, default, .. } => {
            cases.iter().any(|(_, b)| has_jumps(b)) || has_jumps(default)
        }
        Op::TypeSwitch { cases, .. } => cases.iter().any(|(_, b)| has_jumps(b)),
        // Regions of their own; erased (and stackified) separately.
        Op::TryTable { .. } => false,
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
    let mut cx = Cx {
        env,
        func,
        declared: (func.params.len() + func.locals.len()) as u32,
        scratch_i32: None,
        scratch_i64: None,
        extra: Vec::new(),
    };
    let mut code: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if has_jumps(&func.body) {
        // N6: restructure the flat CFG into a tree, then emit it.
        let tree = stackify(&func.body).map_err(|e| EraseError {
            at: None,
            kind: EraseErrorKind::Stackify(e),
        })?;
        let mut sink = InstructionSink::new(&mut code);
        cx.emit_tree(&mut sink, &func.body, &tree, None)?;
        // A result-returning function whose structured body never falls off the
        // end (every path returns or loops) still needs a terminator so the
        // implicit function end does not underflow the value stack.
        if !func.results.is_empty() {
            sink.unreachable();
        }
        sink.end();
    } else {
        let mut sink = InstructionSink::new(&mut code);
        for (i, op) in func.body.iter().enumerate() {
            cx.op(&mut sink, op)
                .map_err(|kind| EraseError { at: Some(i), kind })?;
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
        let slot = if vt == ValType::I64 {
            &mut self.scratch_i64
        } else {
            &mut self.scratch_i32
        };
        if let Some(i) = *slot {
            return i;
        }
        let i = self.declared + self.extra.len() as u32;
        *slot = Some(i);
        self.extra.push(vt);
        i
    }

    /// A new local of class `vt`, never shared (unlike [`Cx::scratch`]).
    fn fresh(&mut self, vt: ValType) -> u32 {
        let i = self.declared + self.extra.len() as u32;
        self.extra.push(vt);
        i
    }

    /// One atomic op: operands in wasm's order, result into `dest`.
    ///
    /// The width picks both the instruction and the value class: up to four
    /// bytes is an i32 access, eight is an i64 one. wasm has no signed narrow
    /// atomic, only the `_u` forms, so a narrow access yields an unsigned value;
    /// the frontend normalizes it the way it does for a `Load`.
    #[allow(clippy::too_many_arguments)]
    fn atomic(
        &mut self,
        s: &mut InstructionSink<'_>,
        op: AtomicOp,
        width: u8,
        offset: u32,
        ptr: Option<&Expr>,
        val: Option<&Expr>,
        replacement: Option<&Expr>,
        dest: Option<u32>,
    ) -> Result<(), EraseErrorKind> {
        if op == AtomicOp::Fence {
            if ptr.is_some() || val.is_some() || replacement.is_some() || dest.is_some() {
                return Err(EraseErrorKind::AtomicShape { op });
            }
            s.atomic_fence();
            return Ok(());
        }

        let ptr = ptr.ok_or(EraseErrorKind::AtomicShape { op })?;
        // align is the exponent: a 4-byte access aligns to 4.
        let m = memarg(offset, (width as u32).trailing_zeros());
        let vt = match width {
            1 | 2 | 4 => ValType::I32,
            8 => ValType::I64,
            _ => return Err(EraseErrorKind::AtomicWidth { width }),
        };

        // Address first, then the op's own operand, because wasm pops the last
        // pushed operand first.
        self.addr(s, ptr)?;
        match (op, width) {
            // A load takes only the address; a store also takes the value.
            (AtomicOp::Load, _) => {}
            (AtomicOp::Store, _) => {
                let v = val.ok_or(EraseErrorKind::AtomicShape { op })?;
                self.expect_class(s, v, vt)?;
            }
            (AtomicOp::Add, _)
            | (AtomicOp::Sub, _)
            | (AtomicOp::And, _)
            | (AtomicOp::Or, _)
            | (AtomicOp::Xor, _)
            | (AtomicOp::Xchg, _)
            | (AtomicOp::Notify, _) => {
                let v = val.ok_or(EraseErrorKind::AtomicShape { op })?;
                self.expect_class(s, v, vt)?;
            }
            (AtomicOp::Cmpxchg, _) => {
                let e = val.ok_or(EraseErrorKind::AtomicShape { op })?;
                self.expect_class(s, e, vt)?;
                let r = replacement.ok_or(EraseErrorKind::AtomicShape { op })?;
                self.expect_class(s, r, vt)?;
            }
            (AtomicOp::Wait, _) => {
                let e = val.ok_or(EraseErrorKind::AtomicShape { op })?;
                self.expect_class(s, e, vt)?;
                // The timeout is an i64 nanosecond count in both waits.
                let t = replacement.ok_or(EraseErrorKind::AtomicShape { op })?;
                self.expect_class(s, t, ValType::I64)?;
            }
            (AtomicOp::Fence, _) => unreachable!("fence returned above"),
        }

        match (op, width) {
            (AtomicOp::Load, 1) => {
                s.i32_atomic_load8_u(m);
            }
            (AtomicOp::Load, 2) => {
                s.i32_atomic_load16_u(m);
            }
            (AtomicOp::Load, 4) => {
                s.i32_atomic_load(m);
            }
            (AtomicOp::Load, 8) => {
                s.i64_atomic_load(m);
            }
            (AtomicOp::Store, 1) => {
                s.i32_atomic_store8(m);
            }
            (AtomicOp::Store, 2) => {
                s.i32_atomic_store16(m);
            }
            (AtomicOp::Store, 4) => {
                s.i32_atomic_store(m);
            }
            (AtomicOp::Store, 8) => {
                s.i64_atomic_store(m);
            }
            (AtomicOp::Add, 1) => {
                s.i32_atomic_rmw8_add_u(m);
            }
            (AtomicOp::Add, 2) => {
                s.i32_atomic_rmw16_add_u(m);
            }
            (AtomicOp::Add, 4) => {
                s.i32_atomic_rmw_add(m);
            }
            (AtomicOp::Add, 8) => {
                s.i64_atomic_rmw_add(m);
            }
            (AtomicOp::Sub, 1) => {
                s.i32_atomic_rmw8_sub_u(m);
            }
            (AtomicOp::Sub, 2) => {
                s.i32_atomic_rmw16_sub_u(m);
            }
            (AtomicOp::Sub, 4) => {
                s.i32_atomic_rmw_sub(m);
            }
            (AtomicOp::Sub, 8) => {
                s.i64_atomic_rmw_sub(m);
            }
            (AtomicOp::And, 1) => {
                s.i32_atomic_rmw8_and_u(m);
            }
            (AtomicOp::And, 2) => {
                s.i32_atomic_rmw16_and_u(m);
            }
            (AtomicOp::And, 4) => {
                s.i32_atomic_rmw_and(m);
            }
            (AtomicOp::And, 8) => {
                s.i64_atomic_rmw_and(m);
            }
            (AtomicOp::Or, 1) => {
                s.i32_atomic_rmw8_or_u(m);
            }
            (AtomicOp::Or, 2) => {
                s.i32_atomic_rmw16_or_u(m);
            }
            (AtomicOp::Or, 4) => {
                s.i32_atomic_rmw_or(m);
            }
            (AtomicOp::Or, 8) => {
                s.i64_atomic_rmw_or(m);
            }
            (AtomicOp::Xor, 1) => {
                s.i32_atomic_rmw8_xor_u(m);
            }
            (AtomicOp::Xor, 2) => {
                s.i32_atomic_rmw16_xor_u(m);
            }
            (AtomicOp::Xor, 4) => {
                s.i32_atomic_rmw_xor(m);
            }
            (AtomicOp::Xor, 8) => {
                s.i64_atomic_rmw_xor(m);
            }
            (AtomicOp::Xchg, 1) => {
                s.i32_atomic_rmw8_xchg_u(m);
            }
            (AtomicOp::Xchg, 2) => {
                s.i32_atomic_rmw16_xchg_u(m);
            }
            (AtomicOp::Xchg, 4) => {
                s.i32_atomic_rmw_xchg(m);
            }
            (AtomicOp::Xchg, 8) => {
                s.i64_atomic_rmw_xchg(m);
            }
            (AtomicOp::Cmpxchg, 1) => {
                s.i32_atomic_rmw8_cmpxchg_u(m);
            }
            (AtomicOp::Cmpxchg, 2) => {
                s.i32_atomic_rmw16_cmpxchg_u(m);
            }
            (AtomicOp::Cmpxchg, 4) => {
                s.i32_atomic_rmw_cmpxchg(m);
            }
            (AtomicOp::Cmpxchg, 8) => {
                s.i64_atomic_rmw_cmpxchg(m);
            }
            (AtomicOp::Wait, 4) => {
                s.memory_atomic_wait32(m);
            }
            (AtomicOp::Wait, 8) => {
                s.memory_atomic_wait64(m);
            }
            (AtomicOp::Notify, 4) => {
                s.memory_atomic_notify(m);
            }
            (AtomicOp::Notify, 8) => return Err(EraseErrorKind::AtomicWidth { width }),
            (AtomicOp::Fence, _) => unreachable!("fence returned above"),
            // The combinations wasm does not have: a 64-bit operand with a
            // narrower access (i64.atomic.rmw8.*.u). NJVM-IR keeps width and
            // value class separate, so these are expressible but have no
            // frontend and no instruction here yet.
            (op, width) => {
                let _ = op;
                return Err(EraseErrorKind::AtomicWidth { width });
            }
        }

        if op == AtomicOp::Store {
            return Ok(());
        }
        match dest {
            Some(d) => {
                let have = self.local(d)?;
                if class(have) != vt {
                    return Err(EraseErrorKind::ClassMismatch {
                        expected: vt,
                        got: have,
                    });
                }
                s.local_set(d);
            }
            None => {
                s.drop();
            }
        }
        Ok(())
    }

    /// Lower a memory address and leave a memory0 i32 offset on the stack.
    ///
    /// gc pointer tagging (#634): chibicc is LP64, so a pointer the frontend
    /// keeps as a full 8-byte value is an `I64`/`U64` whose high 32 bits are
    /// the gc tag and whose low 32 bits are the memory0 address. Every access
    /// strips the tag with `i32.wrap_i64` right before the instruction; the
    /// `offset`/`align` of the access go into the memarg unchanged. A
    /// `Ptr`/`I32`/`U32` address is already an untagged memory0 offset.
    fn addr(&mut self, s: &mut InstructionSink<'_>, e: &Expr) -> Result<(), EraseErrorKind> {
        let t = self.expr(s, e)?;
        match t {
            Type::Ptr | Type::I32 | Type::U32 => Ok(()),
            Type::I64 | Type::U64 => {
                s.i32_wrap_i64();
                Ok(())
            }
            _ => Err(EraseErrorKind::ClassMismatch {
                expected: ValType::I32,
                got: t,
            }),
        }
    }

    fn local(&self, idx: u32) -> Result<Type, EraseErrorKind> {
        let p = self.func.params.len();
        let i = idx as usize;
        if i < p {
            Ok(self.func.params[i])
        } else {
            self.func
                .locals
                .get(i - p)
                .copied()
                .ok_or(EraseErrorKind::LocalOutOfRange { idx })
        }
    }

    fn global(&self, idx: u32) -> Result<Type, EraseErrorKind> {
        self.env
            .globals
            .get(idx as usize)
            .copied()
            .ok_or(EraseErrorKind::GlobalOutOfRange { idx })
    }

    /// Lower `e` and require exactly type `t`.
    fn expect(
        &mut self,
        s: &mut InstructionSink<'_>,
        e: &Expr,
        t: Type,
    ) -> Result<(), EraseErrorKind> {
        let got = self.expr(s, e)?;
        if got != t {
            return Err(EraseErrorKind::TypeMismatch { expected: t, got });
        }
        Ok(())
    }

    /// Lower `e` and require its wasm class to be `vt` (narrow allowed for i32).
    fn expect_class(
        &mut self,
        s: &mut InstructionSink<'_>,
        e: &Expr,
        vt: ValType,
    ) -> Result<Type, EraseErrorKind> {
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
            _ => Err(EraseErrorKind::ClassMismatch {
                expected: ValType::I32,
                got: t,
            }),
        }
    }

    fn args(
        &mut self,
        s: &mut InstructionSink<'_>,
        args: &[Expr],
        params: &[Type],
    ) -> Result<(), EraseErrorKind> {
        if args.len() != params.len() {
            return Err(EraseErrorKind::ArityMismatch {
                expected: params.len(),
                got: args.len(),
            });
        }
        for (a, &p) in args.iter().zip(params) {
            self.expect(s, a, p)?;
        }
        Ok(())
    }

    fn func_sig(&self, idx: u32) -> Result<(&[Type], &[Type]), EraseErrorKind> {
        self.env
            .funcs
            .get(idx as usize)
            .copied()
            .ok_or(EraseErrorKind::FuncOutOfRange { idx })
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
    fn call(
        &mut self,
        s: &mut InstructionSink<'_>,
        func_idx: u32,
        args: &[Expr],
    ) -> Result<Vec<Type>, EraseErrorKind> {
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
            return Err(EraseErrorKind::ClassMismatch {
                expected: ValType::I32,
                got: t,
            });
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
                    (ValType::F32, ConstVal::F32(b)) => {
                        s.f32_const(Ieee32::from(f32::from_bits(b)))
                    }
                    (ValType::F64, ConstVal::F64(b)) => {
                        s.f64_const(Ieee64::from(f64::from_bits(b)))
                    }
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
            Expr::Select {
                cond,
                then_val,
                else_val,
            } => {
                let t = self.expr(s, then_val)?;
                wide(t)?;
                self.expect(s, else_val, t)?;
                self.cond(s, cond)?;
                s.select();
                Ok(t)
            }
            Expr::Load {
                ty,
                ptr,
                offset,
                align,
            } => {
                self.addr(s, ptr)?;
                let m = memarg(*offset, *align);
                Ok(match ty {
                    MemType::I8 => {
                        s.i32_load8_s(m);
                        Type::I8
                    }
                    MemType::U8 => {
                        s.i32_load8_u(m);
                        Type::U8
                    }
                    MemType::I16 => {
                        s.i32_load16_s(m);
                        Type::I16
                    }
                    MemType::U16 => {
                        s.i32_load16_u(m);
                        Type::U16
                    }
                    MemType::I32 => {
                        s.i32_load(m);
                        Type::I32
                    }
                    MemType::I64 => {
                        s.i64_load(m);
                        Type::I64
                    }
                    MemType::F32 => {
                        s.f32_load(m);
                        Type::F32
                    }
                    MemType::F64 => {
                        s.f64_load(m);
                        Type::F64
                    }
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
            Expr::MemorySize => {
                s.memory_size(0);
                Ok(Type::I32)
            }
            Expr::MemoryGrow { delta } => {
                self.expect_class(s, delta, ValType::I32)?;
                s.memory_grow(0);
                Ok(Type::I32)
            }
            Expr::Call { func_idx, args } => {
                let r = self.call(s, *func_idx, args)?;
                single(&r)
            }
            Expr::CallIndirect {
                type_idx,
                table_idx,
                callee,
                args,
            } => {
                let r = self.call_indirect(s, *type_idx, *table_idx, callee, args)?;
                single(&r)
            }
            Expr::LabelAddr(_) => Err(EraseErrorKind::UnloweredComputedGoto),
        }
    }

    /// Lower `Op::Switch`. Both strategies share one wasm layout:
    ///
    /// ```text
    /// block                 ;; exit
    ///   block               ;; default
    ///     block             ;; arm k-1
    ///       ...
    ///         block         ;; arm 0
    ///           <dispatch>  ;; br j for arm j, br k for default
    ///         end
    ///         <arm 0> br (k-0)
    ///       ...
    ///     end
    ///     <arm k-1> br 1
    ///   end
    ///   <default>
    /// end
    /// ```
    ///
    /// Arm j is at depth j from the dispatch, the default at depth k, and after
    /// arm j the exit is at depth k - j. Arms do not fall through into each
    /// other (NJVM `Switch` is a multi-way `If`, not a C switch; C fallthrough
    /// is the frontend's job, with jumps). Duplicate case values: the first
    /// wins, later arms with the same value are dead and are not emitted.
    ///
    /// The scrutinee is evaluated exactly once. Dense dispatch is
    /// `br_table` on `scrutinee - min`; sparse dispatch is a chain of
    /// `eq`/`br_if` against a fresh local, in case order.
    fn switch(
        &mut self,
        s: &mut InstructionSink<'_>,
        scrutinee: &Expr,
        cases: &[(ConstVal, Vec<Op>)],
        default: &[Op],
    ) -> Result<(), EraseErrorKind> {
        /// Dense threshold. `br_table` costs one bounds check and one
        /// indirect jump whatever the case count, but its table has `span`
        /// entries; a compare chain costs one compare per case before the hit
        /// and no table. So: at least 4 distinct cases (below that the chain
        /// is as fast and smaller; same minimum as LLVM's
        /// `-min-jump-table-entries=4`), the table at least 25% full (span <= 4
        /// * cases, bounding the bytes wasted on default slots), and at most
        /// 4096 slots so one sparse outlier cannot blow up the code.
        const DENSE_MIN_CASES: usize = 4;
        const DENSE_MAX_SPAN_PER_CASE: i128 = 4;
        const DENSE_MAX_SPAN: i128 = 4096;

        let t = self.expr(s, scrutinee)?;
        let vt = class(t);
        if !matches!(vt, ValType::I32 | ValType::I64) {
            return Err(EraseErrorKind::ClassMismatch {
                expected: ValType::I32,
                got: t,
            });
        }
        // Distinct case values (first wins), as integers in the order of the
        // scrutinee's signedness, with the index of the arm they select.
        let mut arms: alloc::vec::Vec<(i128, &[Op])> = alloc::vec::Vec::new();
        for (v, body) in cases {
            let n: i128 = match (vt, *v, is_unsigned(t)) {
                (ValType::I32, ConstVal::I32(x), false) => x as i128,
                (ValType::I32, ConstVal::I32(x), true) => x as u32 as i128,
                (ValType::I64, ConstVal::I64(x), false) => x as i128,
                (ValType::I64, ConstVal::I64(x), true) => x as u64 as i128,
                _ => return Err(EraseErrorKind::ConstMismatch { ty: t, val: *v }),
            };
            if !arms.iter().any(|(m, _)| *m == n) {
                arms.push((n, body.as_slice()));
            }
        }
        let k = arms.len() as u32;
        let (min, max) = arms
            .iter()
            .fold((i128::MAX, i128::MIN), |(lo, hi), (n, _)| {
                (lo.min(*n), hi.max(*n))
            });
        let span = if arms.is_empty() { 0 } else { max - min + 1 };
        let dense = arms.len() >= DENSE_MIN_CASES
            && span <= DENSE_MAX_SPAN_PER_CASE * arms.len() as i128
            && span <= DENSE_MAX_SPAN;

        // The scrutinee is on the stack. Park it in a fresh local: arms may
        // contain switches of their own, so no shared scratch slot.
        let tmp = self.fresh(vt);
        s.local_set(tmp);
        s.block(BlockType::Empty); // exit
        s.block(BlockType::Empty); // default
        for _ in 0..k {
            s.block(BlockType::Empty);
        }
        if dense {
            // slot -> arm depth; holes go to the default (depth k).
            let mut table = alloc::vec![k; span as usize];
            for (j, (n, _)) in arms.iter().enumerate() {
                table[(n - min) as usize] = j as u32;
            }
            s.local_get(tmp);
            if vt == ValType::I32 {
                // Wrapping subtract: a value below `min` wraps to a large
                // unsigned index, which br_table sends to the default.
                s.i32_const(min as i32).i32_sub();
            } else {
                // i64: range-check before narrowing, or a value 2^32 away
                // from a case would alias it after i32.wrap_i64.
                s.i64_const(min as i64).i64_sub().local_tee(tmp);
                s.i64_const(span as i64).i64_ge_u().br_if(k);
                s.local_get(tmp).i32_wrap_i64();
            }
            s.br_table(table, k);
        } else {
            for (j, (n, _)) in arms.iter().enumerate() {
                s.local_get(tmp);
                if vt == ValType::I32 {
                    s.i32_const(*n as i32).i32_eq();
                } else {
                    s.i64_const(*n as i64).i64_eq();
                }
                s.br_if(j as u32);
            }
            s.br(k);
        }
        for (j, (_, body)) in arms.iter().enumerate() {
            s.end(); // arm j's block: its branch lands here
            for op in body.iter() {
                self.op(s, op)?;
            }
            s.br(k - j as u32);
        }
        s.end(); // default block
        for op in default {
            self.op(s, op)?;
        }
        s.end(); // exit
        Ok(())
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
                Div => {
                    if u {
                        s.i32_div_u()
                    } else {
                        s.i32_div_s()
                    }
                }
                Rem => {
                    if u {
                        s.i32_rem_u()
                    } else {
                        s.i32_rem_s()
                    }
                }
                And => s.i32_and(),
                Or => s.i32_or(),
                Xor => s.i32_xor(),
                Shl => s.i32_shl(),
                Shr => {
                    if u {
                        s.i32_shr_u()
                    } else {
                        s.i32_shr_s()
                    }
                }
                Rotl => s.i32_rotl(),
                Rotr => s.i32_rotr(),
                Lt => {
                    if u {
                        s.i32_lt_u()
                    } else {
                        s.i32_lt_s()
                    }
                }
                Le => {
                    if u {
                        s.i32_le_u()
                    } else {
                        s.i32_le_s()
                    }
                }
                Gt => {
                    if u {
                        s.i32_gt_u()
                    } else {
                        s.i32_gt_s()
                    }
                }
                Ge => {
                    if u {
                        s.i32_ge_u()
                    } else {
                        s.i32_ge_s()
                    }
                }
                Eq => s.i32_eq(),
                Ne => s.i32_ne(),
                Min | Max | Copysign => unreachable!("checked above"),
            },
            ValType::I64 => match op {
                Add => s.i64_add(),
                Sub => s.i64_sub(),
                Mul => s.i64_mul(),
                Div => {
                    if u {
                        s.i64_div_u()
                    } else {
                        s.i64_div_s()
                    }
                }
                Rem => {
                    if u {
                        s.i64_rem_u()
                    } else {
                        s.i64_rem_s()
                    }
                }
                And => s.i64_and(),
                Or => s.i64_or(),
                Xor => s.i64_xor(),
                Shl => s.i64_shl(),
                Shr => {
                    if u {
                        s.i64_shr_u()
                    } else {
                        s.i64_shr_s()
                    }
                }
                Rotl => s.i64_rotl(),
                Rotr => s.i64_rotr(),
                Lt => {
                    if u {
                        s.i64_lt_u()
                    } else {
                        s.i64_lt_s()
                    }
                }
                Le => {
                    if u {
                        s.i64_le_u()
                    } else {
                        s.i64_le_s()
                    }
                }
                Gt => {
                    if u {
                        s.i64_gt_u()
                    } else {
                        s.i64_gt_s()
                    }
                }
                Ge => {
                    if u {
                        s.i64_ge_u()
                    } else {
                        s.i64_ge_s()
                    }
                }
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
        Ok(if matches!(op, Lt | Le | Gt | Ge | Eq | Ne) {
            Type::I32
        } else {
            ty
        })
    }

    fn unary(
        &mut self,
        s: &mut InstructionSink<'_>,
        op: UnaryOp,
        val: &Expr,
        ty: Type,
    ) -> Result<Type, EraseErrorKind> {
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
                s.local_tee(x)
                    .local_get(x)
                    .i32_const(31)
                    .i32_shr_s()
                    .i32_xor();
                s.local_get(x).i32_const(31).i32_shr_s().i32_sub();
            }
            (ValType::I64, Abs) => {
                self.expect(s, val, ty)?;
                let x = self.scratch(ValType::I64);
                s.local_tee(x)
                    .local_get(x)
                    .i64_const(63)
                    .i64_shr_s()
                    .i64_xor();
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

    /// Emit one TryTable region (its body or a handler). A region with jumps
    /// is stackified on its own; `exit` is the `br` depth that leaves it.
    /// An error inside the region is reported with its kind only; its `at`
    /// is the enclosing top-level op.
    fn region(
        &mut self,
        s: &mut InstructionSink<'_>,
        ops: &[Op],
        exit: u32,
    ) -> Result<(), EraseErrorKind> {
        if has_jumps(ops) {
            let tree = stackify(ops).map_err(EraseErrorKind::Stackify)?;
            self.emit_tree(s, ops, &tree, Some(exit))
                .map_err(|e| e.kind)
        } else {
            for op in ops {
                self.op(s, op)?;
            }
            Ok(())
        }
    }

    /// Lower `Op::TryTable` with `n` catches:
    ///
    /// ```text
    /// block                       ;; exit
    ///   block (result p_{n-1})    ;; h_{n-1}
    ///     ...
    ///       block (result p_0)    ;; h_0
    ///         try_table (catch t_0 0) ... (catch t_{n-1} n-1)
    ///           <body>            ;; falling off: br 0 (try_table end)
    ///         end
    ///         br n                ;; body done -> exit
    ///       end                   ;; h_0: payload of t_0 on the stack
    ///       local.set bind_0 (reversed)
    ///       <handler 0> br n-1
    ///     ...
    ///   end                       ;; h_{n-1}
    ///   <handler n-1> br 0
    /// end
    /// ```
    ///
    /// Catch labels are resolved outside the try_table, so catch k targets
    /// h_k at depth k. A handler block's result type is its tag's params.
    fn try_table(
        &mut self,
        s: &mut InstructionSink<'_>,
        body: &[Op],
        catches: &[Catch],
    ) -> Result<(), EraseErrorKind> {
        let n = catches.len() as u32;
        let mut bts: Vec<BlockType> = Vec::with_capacity(catches.len());
        for c in catches {
            let params: Vec<Type> = match c.tag {
                Some(t) => self
                    .env
                    .tags
                    .get(t as usize)
                    .ok_or(EraseErrorKind::TagOutOfRange { idx: t })?
                    .to_vec(),
                None => Vec::new(),
            };
            if c.bind.len() != params.len() {
                return Err(EraseErrorKind::ArityMismatch {
                    expected: params.len(),
                    got: c.bind.len(),
                });
            }
            for (&l, &p) in c.bind.iter().zip(params.iter()) {
                let lt = self.local(l)?;
                if lt != p {
                    return Err(EraseErrorKind::TypeMismatch {
                        expected: p,
                        got: lt,
                    });
                }
            }
            let bt = match params.len() {
                0 => BlockType::Empty,
                1 => BlockType::Result(wide(params[0])?),
                _ => {
                    let t = c.tag.unwrap_or(0);
                    let idx = self.env.tag_blocks.get(t as usize).copied().flatten().ok_or(
                        EraseErrorKind::NotImplemented {
                            op: "TryTable",
                            reason: "a catch of a multi-param tag needs its block type; build Env with with_tag_block_types (encode does)",
                        },
                    )?;
                    BlockType::FunctionType(idx)
                }
            };
            bts.push(bt);
        }
        s.block(BlockType::Empty); // exit
        for k in (0..catches.len()).rev() {
            s.block(bts[k]);
        }
        let clauses: alloc::vec::Vec<WasmCatch> = catches
            .iter()
            .enumerate()
            .map(|(k, c)| match c.tag {
                Some(tag) => WasmCatch::One {
                    tag,
                    label: k as u32,
                },
                None => WasmCatch::All { label: k as u32 },
            })
            .collect();
        s.try_table(BlockType::Empty, clauses);
        self.region(s, body, 0)?;
        s.end();
        s.br(n);
        for (k, c) in catches.iter().enumerate() {
            s.end(); // h_k
            for &l in c.bind.iter().rev() {
                s.local_set(l);
            }
            let d = n - 1 - k as u32;
            self.region(s, &c.body, d)?;
            s.br(d);
        }
        s.end(); // exit
        Ok(())
    }

    /// Emit a stackified [`Node`] tree. Blocks and loops carry no stack values
    /// (the IR keeps the operand stack empty at labels), so every construct has
    /// an empty block type.
    ///
    /// `ops` is the region the tree was built from (the function body, or a
    /// `TryTable` body/handler). `exit` is the `br` depth, at the tree's top
    /// level, that leaves the region; `None` for the function body, where
    /// falling off the end returns or traps.
    fn emit_tree(
        &mut self,
        s: &mut InstructionSink<'_>,
        ops: &[Op],
        nodes: &[Node],
        exit: Option<u32>,
    ) -> Result<(), EraseError> {
        let inner = exit.map(|d| d + 1);
        for node in nodes {
            match node {
                Node::Block { body } => {
                    s.block(BlockType::Empty);
                    self.emit_tree(s, ops, body, inner)?;
                    s.end();
                }
                Node::Loop { body } => {
                    s.loop_(BlockType::Empty);
                    self.emit_tree(s, ops, body, inner)?;
                    s.end();
                }
                Node::If {
                    cond,
                    then_body,
                    else_body,
                } => {
                    let c = jmp_cond(ops, *cond);
                    self.cond(s, c).map_err(|kind| EraseError {
                        at: Some(*cond),
                        kind,
                    })?;
                    s.if_(BlockType::Empty);
                    self.emit_tree(s, ops, then_body, inner)?;
                    if !else_body.is_empty() {
                        s.else_();
                        self.emit_tree(s, ops, else_body, inner)?;
                    }
                    s.end();
                }
                Node::Br { depth } => {
                    s.br(*depth);
                }
                Node::BrIf { cond, depth } => {
                    let c = jmp_cond(ops, *cond);
                    self.cond(s, c).map_err(|kind| EraseError {
                        at: Some(*cond),
                        kind,
                    })?;
                    s.br_if(*depth);
                }
                Node::BrTable {
                    index,
                    targets,
                    default,
                } => {
                    let e = brtable_index(ops, *index);
                    // The index selects a target; wasm br_table wants an i32.
                    self.expect_class(s, e, ValType::I32)
                        .map_err(|kind| EraseError {
                            at: Some(*index),
                            kind,
                        })?;
                    s.br_table(targets.iter().copied(), *default);
                }
                Node::Stmt { idx } => {
                    self.op(s, &ops[*idx]).map_err(|kind| EraseError {
                        at: Some(*idx),
                        kind,
                    })?;
                }
                Node::FallOffEnd if exit.is_some() => {
                    // End of a TryTable region: continue after the TryTable.
                    s.br(exit.unwrap());
                }
                Node::FallOffEnd => {
                    // Reaching the body end: return for a void function, trap
                    // otherwise (a non-void function must return a value first).
                    if self.func.results.is_empty() {
                        s.return_();
                    } else {
                        s.unreachable();
                    }
                }
            }
        }
        Ok(())
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
                self.addr(s, ptr)?;
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
                self.addr(s, dest)?;
                self.addr(s, src)?;
                self.expect_class(s, len, ValType::I32)?;
                s.memory_copy(0, 0);
            }
            Op::MemoryFill { dest, val, len } => {
                self.addr(s, dest)?;
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
            Op::Switch { scrutinee, cases, default } => self.switch(s, scrutinee, cases, default)?,
            Op::TypeSwitch { .. } => {
                return Err(EraseErrorKind::NotImplemented {
                    op: "TypeSwitch",
                    reason: "no consumer in this version: NJVM-IR has no reference types, so there is no type tag to dispatch on",
                })
            }
            Op::TryTable { body, catches } => self.try_table(s, body, catches)?,
            Op::Setjmp { .. } => return Err(EraseErrorKind::Unsupported { op: "Setjmp" }),
            Op::Longjmp { .. } => return Err(EraseErrorKind::Unsupported { op: "Longjmp" }),
            Op::Await { .. } => return Err(EraseErrorKind::Unsupported { op: "Await" }),
            Op::Yield { .. } => return Err(EraseErrorKind::Unsupported { op: "Yield" }),
            Op::Atomic { op, width, offset, ptr, val, replacement, dest } => self.atomic(
                s,
                *op,
                *width,
                *offset,
                ptr.as_ref(),
                val.as_ref(),
                replacement.as_ref(),
                *dest,
            )?,
        }
        Ok(())
    }
}

/// The condition of the `JmpIf` at region index `idx` (stackify only points there).
fn jmp_cond(ops: &[Op], idx: usize) -> &Expr {
    match &ops[idx] {
        Op::JmpIf { cond, .. } => cond,
        _ => unreachable!("stackify points If/BrIf cond at a JmpIf"),
    }
}

/// The index of the `BrTable` at region index `idx`.
fn brtable_index(ops: &[Op], idx: usize) -> &Expr {
    match &ops[idx] {
        Op::BrTable { index, .. } => index,
        _ => unreachable!("stackify points BrTable index at a BrTable"),
    }
}

fn single(results: &[Type]) -> Result<Type, EraseErrorKind> {
    match results {
        [t] => Ok(*t),
        _ => Err(EraseErrorKind::CallResultArity {
            results: results.len(),
        }),
    }
}

/// Bring an i32 holding any value down to the normalized form of narrow `to`.
fn normalize(s: &mut InstructionSink<'_>, to: Type) {
    match to {
        Type::I8 => {
            s.i32_extend8_s();
        }
        Type::I16 => {
            s.i32_extend16_s();
        }
        Type::U8 => {
            s.i32_const(0xff).i32_and();
        }
        Type::U16 => {
            s.i32_const(0xffff).i32_and();
        }
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
            if is_unsigned(from) {
                s.i64_extend_i32_u();
            } else {
                s.i64_extend_i32_s();
            }
        }
        (ValType::I64, ValType::I64) => {}
        (ValType::I32, ValType::F32) => {
            if is_unsigned(from) {
                s.f32_convert_i32_u();
            } else {
                s.f32_convert_i32_s();
            }
        }
        (ValType::I32, ValType::F64) => {
            if is_unsigned(from) {
                s.f64_convert_i32_u();
            } else {
                s.f64_convert_i32_s();
            }
        }
        (ValType::I64, ValType::F32) => {
            if is_unsigned(from) {
                s.f32_convert_i64_u();
            } else {
                s.f32_convert_i64_s();
            }
        }
        (ValType::I64, ValType::F64) => {
            if is_unsigned(from) {
                s.f64_convert_i64_u();
            } else {
                s.f64_convert_i64_s();
            }
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
            if is_unsigned(to) {
                s.i64_trunc_f32_u();
            } else {
                s.i64_trunc_f32_s();
            }
        }
        (ValType::F64, ValType::I64) => {
            if is_unsigned(to) {
                s.i64_trunc_f64_u();
            } else {
                s.i64_trunc_f64_s();
            }
        }
        (ValType::F32, ValType::F64) => {
            s.f64_promote_f32();
        }
        (ValType::F64, ValType::F32) => {
            s.f32_demote_f64();
        }
        _ => {}
    }
}

#[cfg(test)]
mod lower_tests;
#[cfg(test)]
mod tests;

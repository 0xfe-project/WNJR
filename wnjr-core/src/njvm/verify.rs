//! NJVM-IR verifier (task N3).
//!
//! [`verify_module`] must accept a module before any transform runs on it. It
//! never panics on bad input; every failure is a [`VerifyError`] naming the
//! function, the op position and the reason.
//!
//! # Positions
//!
//! `VerifyError::func` is the function's index in the unified function index
//! space (imported functions first), as in `encode`.
//!
//! `VerifyError::at` is a path into the body. It alternates op index and child-body
//! index: `[3]` is `body[3]`; `[3, 1, 0]` is op 0 of child body 1 of
//! `body[3]`. Child bodies are numbered per op:
//! - `Block`, `Loop`: 0 = body
//! - `If`: 0 = then, 1 = else
//! - `Switch`: 0..n = cases, n = default
//! - `TypeSwitch`: 0..n = cases
//! - `TryTable`: 0 = body, 1.. = catch handlers in order
//! - `Await`: 0 = fallback
//!
//! An error inside an expression is reported at the op that holds it.
//!
//! # Control-flow model
//!
//! The CFG has one node per op. An op falls through to the next op of its
//! body; the last op of a child body falls through to the op after its parent
//! (wasm `block`/`loop`/`if` exit semantics; `Loop` has no implicit back edge). A
//! structured op has edges to the first op of each child body; `If` with an
//! empty arm, `TypeSwitch` (no default) and `Await` also edge to the next op.
//! `TryTable` edges to its body and to every handler (an exception can only
//! enter a handler through its try). `Jmp`/`JmpIf`/`BrTable` edge to their label.
//! `Return`, `Unreachable`, `Throw`, `Longjmp` have no successors.
//!
//! Reducibility (Hecht–Ullman): the CFG restricted to nodes reachable from
//! the entry is reducible iff, for a depth-first search from the entry, the
//! target of every retreating edge dominates its source. Unreachable ops are
//! not checked for reducibility.

use alloc::collections::BTreeMap;

use crate::collections::{vec, Vec};

use super::expr::Expr;
use super::module::{FunctionType, Function, ImportKind, Module};
use super::op::{LabelId, Op};
use super::types::{AtomicOp, BinaryOp, ConstVal, MemType, Type, UnaryOp};

/// A position in a function body; see the module docs.
pub type OpPath = Vec<usize>;

/// A verification failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyError {
    /// Function index (unified index space, imports first).
    pub func: u32,
    /// Path of the offending op.
    pub at: OpPath,
    pub kind: VerifyErrorKind,
}

/// The operator named by [`VerifyErrorKind::IllegalOp`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpName {
    Binary(BinaryOp),
    Unary(UnaryOp),
}

/// Why verification failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyErrorKind {
    /// An expression has type `got`; one of `expected` was required.
    TypeMismatch { expected: Vec<Type>, got: Type },
    /// A constant's value does not fit its declared type
    /// (I8..U32/Ptr need `ConstVal::I32`, I64/U64 `I64`, F32 `F32`, F64 `F64`).
    ConstMismatch { ty: Type, val: ConstVal },
    /// The operator is not defined for this type (e.g. `Sqrt` on I32, `Clz` on F64).
    IllegalOp { op: OpName, ty: Type },
    /// Local index `idx` is not below `count` (params + locals).
    LocalOutOfRange { idx: u32, count: u32 },
    GlobalOutOfRange { idx: u32, count: u32 },
    /// `GlobalSet` on an immutable global.
    ImmutableGlobal { idx: u32 },
    FuncOutOfRange { idx: u32, count: u32 },
    /// A type index (`CallIndirect`, or the type of an imported function) is
    /// not below `Module::types.len()`.
    TypeOutOfRange { idx: u32, count: u32 },
    TableOutOfRange { idx: u32, count: u32 },
    TagOutOfRange { idx: u32, count: u32 },
    /// Call or throw arguments do not match the callee's or tag's params.
    ArgMismatch { expected: Vec<Type>, got: Vec<Type> },
    /// A call used as an expression must have exactly one result.
    CallResultArity { results: Vec<Type> },
    /// `Return` values do not match the function's results.
    ReturnMismatch { expected: Vec<Type>, got: Vec<Type> },
    /// Control reaches the end of a function that has results.
    MissingReturn,
    /// A jump names a label not declared in this function.
    UndeclaredLabel { label: LabelId },
    /// `label` was already declared at `first`.
    DuplicateLabel { label: LabelId, first: OpPath },
    /// A `Label` declares params; the operand stack must be empty at labels.
    LabelParams { label: LabelId },
    /// The CFG is irreducible. `VerifyError::at` is the source op of a retreating
    /// edge whose target (`target`, the label `label` when it is one) does not
    /// dominate it.
    Irreducible { label: Option<LabelId>, target: OpPath },
    /// A jump from outside a `TryTable` body to a label inside it.
    JumpIntoTry { label: LabelId },
    /// A catch clause's `bind` locals do not have the types of its tag's
    /// params (catch_all binds nothing).
    CatchBind { expected: Vec<Type>, got: Vec<Type> },
    /// A pre-lowering computed-goto marker (`Expr::LabelAddr` or
    /// `Op::GotoIndirect`) is still present; run `computed_goto::lower_module` first.
    UnloweredComputedGoto,
    /// An atomic op is missing an operand it needs, or carries one it does not.
    AtomicShape { op: AtomicOp },
    /// An atomic access width no wasm instruction has (or does not match the
    /// width of the value operand).
    AtomicWidth { width: u8 },
}

impl core::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "function {} at {:?}: {:?}", self.func, self.at, self.kind)
    }
}

/// Verify every defined function of `module`. Returns the first error found
/// (functions in order; within a function, labels, then ops in order, then
/// the CFG).
pub fn verify_module(module: &Module) -> Result<(), VerifyError> {
    // A data fixup is a pre-lowering marker like `Expr::LabelAddr`: it says a
    // word still waits for a computed-goto label number, which only
    // `computed_goto::lower_module` assigns.
    if let Some(fixup) = module.data_label_fixups.first() {
        return Err(VerifyError {
            func: fixup.function,
            at: Vec::new(),
            kind: VerifyErrorKind::UnloweredComputedGoto,
        });
    }
    let env = Env::new(module);
    for (i, func) in module.functions.iter().enumerate() {
        let idx = env.imported_funcs + i as u32;
        verify_function(&env, idx, func).map_err(|(at, kind)| VerifyError { func: idx, at, kind })?;
    }
    Ok(())
}

type Fail = (OpPath, VerifyErrorKind);

/// Module-level index spaces.
struct Env<'m> {
    module: &'m Module,
    imported_funcs: u32,
    /// Imported function type indices, in order.
    import_funcs: Vec<u32>,
    /// (type, mutable) for every global, imports first.
    globals: Vec<(Type, bool)>,
    tables: u32,
    /// Params of every tag, imports first.
    tags: Vec<&'m [Type]>,
}

impl<'m> Env<'m> {
    fn new(module: &'m Module) -> Self {
        let mut import_funcs = Vec::new();
        let mut globals = Vec::new();
        let mut tables = 0u32;
        let mut tags: Vec<&'m [Type]> = Vec::new();
        for imp in &module.imports {
            match &imp.kind {
                ImportKind::Function(t) => import_funcs.push(*t),
                ImportKind::Global(g) => globals.push((g.ty, g.mutable)),
                ImportKind::Table(_) => tables += 1,
                ImportKind::Tag(t) => tags.push(&t.params),
                ImportKind::Memory(_) => {}
            }
        }
        globals.extend(module.globals.iter().map(|g| (g.ty.ty, g.ty.mutable)));
        tables += module.tables.len() as u32;
        tags.extend(module.tags.iter().map(|t| t.params.as_slice()));
        Env { module, imported_funcs: import_funcs.len() as u32, import_funcs, globals, tables, tags }
    }

    fn func_count(&self) -> u32 {
        self.imported_funcs + self.module.functions.len() as u32
    }

    fn func_sig(&self, idx: u32) -> Result<FunctionType, VerifyErrorKind> {
        if let Some(&t) = self.import_funcs.get(idx as usize) {
            return self.type_at(t).cloned();
        }
        match self.module.functions.get((idx - self.imported_funcs) as usize) {
            Some(f) => Ok(f.signature()),
            None => Err(VerifyErrorKind::FuncOutOfRange { idx, count: self.func_count() }),
        }
    }

    fn type_at(&self, idx: u32) -> Result<&'m FunctionType, VerifyErrorKind> {
        self.module
            .types
            .get(idx as usize)
            .ok_or(VerifyErrorKind::TypeOutOfRange { idx, count: self.module.types.len() as u32 })
    }
}

fn is_int(t: Type) -> bool {
    !is_float(t)
}

/// Whether a type is one wasm i64: the value class of an 8-byte atomic.
fn is_wide_int(t: Type) -> bool {
    matches!(t, Type::I64 | Type::U64)
}

fn is_float(t: Type) -> bool {
    matches!(t, Type::F32 | Type::F64)
}

/// Types that are one wasm i32: valid as an address, a length, a condition.
const I32_LIKE: [Type; 3] = [Type::I32, Type::U32, Type::Ptr];
const ADDR: [Type; 2] = [Type::Ptr, Type::I32];
/// Addresses of memory accesses (Load/Store/MemoryCopy/MemoryFill). Adds the
/// 8-byte gc-tagged pointer (#634): an I64/U64 whose low 32 bits are the
/// memory0 address; N7 strips the tag with i32.wrap_i64 before the access.
const MEM_ADDR: [Type; 4] = [Type::Ptr, Type::I32, Type::I64, Type::U64];
const INTS: [Type; 9] =
    [Type::I8, Type::U8, Type::I16, Type::U16, Type::I32, Type::U32, Type::I64, Type::U64, Type::Ptr];

fn const_fits(ty: Type, val: ConstVal) -> bool {
    match val {
        ConstVal::I32(_) => matches!(
            ty,
            Type::I8 | Type::U8 | Type::I16 | Type::U16 | Type::I32 | Type::U32 | Type::Ptr
        ),
        ConstVal::I64(_) => matches!(ty, Type::I64 | Type::U64),
        ConstVal::F32(_) => ty == Type::F32,
        ConstVal::F64(_) => ty == Type::F64,
    }
}

fn load_type(m: MemType) -> Type {
    match m {
        MemType::I8 => Type::I8,
        MemType::U8 => Type::U8,
        MemType::I16 => Type::I16,
        MemType::U16 => Type::U16,
        MemType::I32 => Type::I32,
        MemType::I64 => Type::I64,
        MemType::F32 => Type::F32,
        MemType::F64 => Type::F64,
    }
}

/// Value types a store of width `m` accepts. Narrow stores take the narrow
/// types of that width or a full i32 (wasm `i32.store8/16` truncates).
fn store_types(m: MemType) -> &'static [Type] {
    match m {
        MemType::I8 | MemType::U8 => &[Type::I8, Type::U8, Type::I32, Type::U32],
        MemType::I16 | MemType::U16 => &[Type::I16, Type::U16, Type::I32, Type::U32],
        MemType::I32 => &I32_LIKE,
        MemType::I64 => &[Type::I64, Type::U64],
        MemType::F32 => &[Type::F32],
        MemType::F64 => &[Type::F64],
    }
}

fn binary_ok(op: BinaryOp, ty: Type) -> bool {
    use BinaryOp::*;
    match op {
        Add | Sub | Mul | Div | Lt | Le | Gt | Ge | Eq | Ne => true,
        Rem | And | Or | Xor | Shl | Shr | Rotl | Rotr => is_int(ty),
        Min | Max | Copysign => is_float(ty),
    }
}

fn is_comparison(op: BinaryOp) -> bool {
    use BinaryOp::*;
    matches!(op, Lt | Le | Gt | Ge | Eq | Ne)
}

fn unary_ok(op: UnaryOp, ty: Type) -> bool {
    use UnaryOp::*;
    match op {
        Neg | Abs => true,
        Not | Clz | Ctz | Popcnt => is_int(ty),
        Sqrt | Ceil | Floor | Trunc | Nearest => is_float(ty),
    }
}

/// The child bodies of an op, in the numbering of the module docs.
fn child_bodies(op: &Op) -> Vec<&[Op]> {
    let mut v: Vec<&[Op]> = Vec::new();
    match op {
        Op::Block { body } | Op::Loop { body } => v.push(body),
        Op::If { then_body, else_body, .. } => {
            v.push(then_body);
            v.push(else_body);
        }
        Op::Switch { cases, default, .. } => {
            v.extend(cases.iter().map(|(_, b)| b.as_slice()));
            v.push(default);
        }
        Op::TypeSwitch { cases, .. } => v.extend(cases.iter().map(|(_, b)| b.as_slice())),
        Op::TryTable { body, catches } => {
            v.push(body);
            v.extend(catches.iter().map(|c| c.body.as_slice()));
        }
        Op::Await { fallback, .. } => v.push(fallback),
        _ => {}
    }
    v
}

/// A declared label: where, and inside which try bodies (outermost first).
struct LabelInfo {
    at: OpPath,
    tries: Vec<u32>,
}

struct FnCx<'a, 'm> {
    env: &'a Env<'m>,
    func: &'a Function,
    labels: BTreeMap<LabelId, LabelInfo>,
    next_try: u32,
}

fn verify_function(env: &Env<'_>, _idx: u32, func: &Function) -> Result<(), Fail> {
    let mut cx = FnCx { env, func, labels: BTreeMap::new(), next_try: 0 };
    let mut tries = Vec::new();
    cx.collect_labels(&func.body, &Vec::new(), &mut tries)?;
    cx.next_try = 0;
    cx.check_body(&func.body, &Vec::new(), &mut tries)?;
    Cfg::build(&cx).check(func)
}

impl FnCx<'_, '_> {
    fn local_count(&self) -> u32 {
        (self.func.params.len() + self.func.locals.len()) as u32
    }

    fn local(&self, idx: u32) -> Result<Type, VerifyErrorKind> {
        let p = self.func.params.len();
        let i = idx as usize;
        if i < p {
            Ok(self.func.params[i])
        } else if let Some(&t) = self.func.locals.get(i - p) {
            Ok(t)
        } else {
            Err(VerifyErrorKind::LocalOutOfRange { idx, count: self.local_count() })
        }
    }

    fn global(&self, idx: u32) -> Result<(Type, bool), VerifyErrorKind> {
        self.env
            .globals
            .get(idx as usize)
            .copied()
            .ok_or(VerifyErrorKind::GlobalOutOfRange { idx, count: self.env.globals.len() as u32 })
    }

    /// Visit every op in the same order as [`Self::check_body`]; try bodies get
    /// ids in that order.
    fn collect_labels(&mut self, body: &[Op], base: &OpPath, tries: &mut Vec<u32>) -> Result<(), Fail> {
        for (i, op) in body.iter().enumerate() {
            let at = { let mut p = base.clone(); p.push(i); p };
            if let Op::Label { id, params } = op {
                if let Some(first) = self.labels.get(id) {
                    return Err((at, VerifyErrorKind::DuplicateLabel { label: *id, first: first.at.clone() }));
                }
                if !params.is_empty() {
                    return Err((at, VerifyErrorKind::LabelParams { label: *id }));
                }
                self.labels.insert(*id, LabelInfo { at: at.clone(), tries: tries.clone() });
            }
            self.for_children(op, &at, tries, |cx, b, p, t| cx.collect_labels(b, p, t))?;
        }
        Ok(())
    }

    /// Recurse into the children of `op` with the right try-context: a
    /// `TryTable` body is a new try region, its handlers are not.
    fn for_children(
        &mut self,
        op: &Op,
        at: &OpPath,
        tries: &mut Vec<u32>,
        mut f: impl FnMut(&mut Self, &[Op], &OpPath, &mut Vec<u32>) -> Result<(), Fail>,
    ) -> Result<(), Fail> {
        for (c, b) in child_bodies(op).into_iter().enumerate() {
            let mut p = at.clone();
            p.push(c);
            let is_try_body = matches!(op, Op::TryTable { .. }) && c == 0;
            if is_try_body {
                tries.push(self.next_try);
                self.next_try += 1;
            }
            let r = f(self, b, &p, tries);
            if is_try_body {
                tries.pop();
            }
            r?;
        }
        Ok(())
    }

    fn check_body(&mut self, body: &[Op], base: &OpPath, tries: &mut Vec<u32>) -> Result<(), Fail> {
        for (i, op) in body.iter().enumerate() {
            let at = { let mut p = base.clone(); p.push(i); p };
            self.check_op(op, tries).map_err(|k| (at.clone(), k))?;
            self.for_children(op, &at, tries, |cx, b, p, t| cx.check_body(b, p, t))?;
        }
        Ok(())
    }

    fn target(&self, label: LabelId, tries: &[u32]) -> Result<(), VerifyErrorKind> {
        let info = self.labels.get(&label).ok_or(VerifyErrorKind::UndeclaredLabel { label })?;
        // Allowed only from inside every try body that encloses the label.
        if !tries.starts_with(&info.tries) {
            return Err(VerifyErrorKind::JumpIntoTry { label });
        }
        Ok(())
    }

    fn expect(&self, e: &Expr, allowed: &[Type]) -> Result<Type, VerifyErrorKind> {
        let t = self.expr(e)?;
        if allowed.contains(&t) {
            Ok(t)
        } else {
            Err(VerifyErrorKind::TypeMismatch { expected: allowed.to_vec(), got: t })
        }
    }

    fn args(&self, args: &[Expr], params: &[Type]) -> Result<(), VerifyErrorKind> {
        let got = args.iter().map(|a| self.expr(a)).collect::<Result<Vec<_>, _>>()?;
        if got != params {
            return Err(VerifyErrorKind::ArgMismatch { expected: params.to_vec(), got });
        }
        Ok(())
    }

    fn call(&self, func_idx: u32, args: &[Expr]) -> Result<FunctionType, VerifyErrorKind> {
        let sig = self.env.func_sig(func_idx)?;
        self.args(args, &sig.params)?;
        Ok(sig)
    }

    fn call_indirect(&self, type_idx: u32, table_idx: u32, callee: &Expr, args: &[Expr]) -> Result<FunctionType, VerifyErrorKind> {
        if table_idx >= self.env.tables {
            return Err(VerifyErrorKind::TableOutOfRange { idx: table_idx, count: self.env.tables });
        }
        let sig = self.env.type_at(type_idx)?;
        self.expect(callee, &I32_LIKE)?;
        self.args(args, &sig.params)?;
        Ok(sig.clone())
    }

    fn tag(&self, tag_idx: u32) -> Result<&[Type], VerifyErrorKind> {
        self.env
            .tags
            .get(tag_idx as usize)
            .copied()
            .ok_or(VerifyErrorKind::TagOutOfRange { idx: tag_idx, count: self.env.tags.len() as u32 })
    }

    fn check_op(&self, op: &Op, tries: &[u32]) -> Result<(), VerifyErrorKind> {
        match op {
            Op::Label { .. } | Op::Unreachable | Op::Nop | Op::MemorySize | Op::Block { .. } | Op::Loop { .. } => {}
            Op::Jmp { target } => self.target(*target, tries)?,
            Op::JmpIf { cond, target } => {
                self.expect(cond, &INTS)?;
                self.target(*target, tries)?;
            }
            Op::BrTable { index, targets, default } => {
                self.expect(index, &I32_LIKE)?;
                for t in targets {
                    self.target(*t, tries)?;
                }
                self.target(*default, tries)?;
            }
            Op::GotoIndirect { .. } => return Err(VerifyErrorKind::UnloweredComputedGoto),
            Op::Store { ty, ptr, val, .. } => {
                self.expect(ptr, &MEM_ADDR)?;
                self.expect(val, store_types(*ty))?;
            }
            Op::If { cond, .. } => {
                self.expect(cond, &INTS)?;
            }
            Op::Return { vals } => {
                let got = vals.iter().map(|v| self.expr(v)).collect::<Result<Vec<_>, _>>()?;
                if got != self.func.results {
                    return Err(VerifyErrorKind::ReturnMismatch { expected: self.func.results.clone(), got });
                }
            }
            Op::Switch { scrutinee, cases, .. } => {
                let t = self.expect(scrutinee, &INTS)?;
                for (v, _) in cases {
                    if !const_fits(t, *v) {
                        return Err(VerifyErrorKind::ConstMismatch { ty: t, val: *v });
                    }
                }
            }
            Op::TypeSwitch { scrutinee, .. } => {
                self.expr(scrutinee)?;
            }
            Op::Call { func_idx, args } => {
                self.call(*func_idx, args)?;
            }
            Op::CallIndirect { type_idx, table_idx, callee, args } => {
                self.call_indirect(*type_idx, *table_idx, callee, args)?;
            }
            Op::TryTable { catches, .. } => {
                for c in catches {
                    let params: Vec<Type> = match c.tag {
                        Some(t) => self.tag(t)?.to_vec(),
                        None => Vec::new(),
                    };
                    let got = c.bind.iter().map(|&l| self.local(l)).collect::<Result<Vec<_>, _>>()?;
                    if got != params {
                        return Err(VerifyErrorKind::CatchBind { expected: params, got });
                    }
                }
            }
            Op::Throw { tag_idx, args } => {
                let params = self.tag(*tag_idx)?;
                self.args(args, params)?;
            }
            Op::Setjmp { buf, dest } => {
                self.expect(buf, &ADDR)?;
                let t = self.local(*dest)?;
                if t != Type::I32 {
                    return Err(VerifyErrorKind::TypeMismatch { expected: vec![Type::I32], got: t });
                }
            }
            Op::Longjmp { buf, val } => {
                self.expect(buf, &ADDR)?;
                self.expect(val, &[Type::I32])?;
            }
            Op::Await { channel, .. } => {
                self.expr(channel)?;
            }
            Op::Yield { val } | Op::Drop { val } => {
                self.expr(val)?;
            }
            Op::LocalSet { idx, val } => {
                let t = self.local(*idx)?;
                self.expect(val, &[t])?;
            }
            Op::GlobalSet { idx, val } => {
                let (t, mutable) = self.global(*idx)?;
                if !mutable {
                    return Err(VerifyErrorKind::ImmutableGlobal { idx: *idx });
                }
                self.expect(val, &[t])?;
            }
            Op::MemoryCopy { dest, src, len } => {
                self.expect(dest, &MEM_ADDR)?;
                self.expect(src, &MEM_ADDR)?;
                self.expect(len, &I32_LIKE)?;
            }
            Op::MemoryFill { dest, val, len } => {
                self.expect(dest, &MEM_ADDR)?;
                self.expect(val, &I32_LIKE)?;
                self.expect(len, &I32_LIKE)?;
            }
            Op::MemoryGrow { delta } => {
                self.expect(delta, &I32_LIKE)?;
            }
            Op::Atomic { op, width, ptr, val, replacement, dest, .. } => {
                // The shape table: which operands each atomic op takes. A load
                // and a read-modify-write yield a value, a store does not, a
                // fence takes nothing at all.
                // A store is the one op with no result at all, so a
                // destination on it is meaningless rather than merely discarded.
                if *op == AtomicOp::Store && dest.is_some() {
                    return Err(VerifyErrorKind::AtomicShape { op: *op });
                }
                let want_val = !matches!(op, AtomicOp::Load | AtomicOp::Fence);
                // A compare-exchange carries the replacement there; a wait
                // carries its timeout, which is an i64 nanosecond count.
                let want_repl = matches!(op, AtomicOp::Cmpxchg | AtomicOp::Wait);
                // Whether the result is kept is the op's own business: a
                // discard is what dest None means. What has to match is the
                // inputs. A store additionally has no result to give.
                let has = [ptr.is_some(), val.is_some(), replacement.is_some()];
                let want = [*op != AtomicOp::Fence, want_val, want_repl];
                if has != want {
                    return Err(VerifyErrorKind::AtomicShape { op: *op });
                }
                if let Some(p) = ptr {
                    self.expect(p, &MEM_ADDR)?;
                }
                // 1, 2 and 4 bytes are an i32 access; 8 is an i64 one.
                let want_ty = if *width == 8 { Type::I64 } else { Type::I32 };
                if *op != AtomicOp::Fence && !matches!(width, 1 | 2 | 4 | 8) {
                    return Err(VerifyErrorKind::AtomicWidth { width: *width });
                }
                // The value operand has the access width's class; a
                // compare-exchange's replacement matches it, while a wait's
                // timeout is eight bytes whatever the access width is.
                for e in [val].into_iter().flatten() {
                    let got = self.expect(e, &INTS)?;
                    if is_wide_int(got) != is_wide_int(want_ty) {
                        return Err(VerifyErrorKind::AtomicWidth { width: *width });
                    }
                }
                if let Some(r) = replacement {
                    let got = self.expect(r, &INTS)?;
                    let want_wide = *op != AtomicOp::Wait && is_wide_int(want_ty) || *op == AtomicOp::Wait;
                    if is_wide_int(got) != want_wide {
                        return Err(VerifyErrorKind::AtomicWidth { width: *width });
                    }
                }
                if let Some(d) = dest {
                    let t = self.local(*d)?;
                    // A wait yields 0/1/2 and a notify yields the number of
                    // waiters woken: both are i32 whatever the access width.
                    if *op == AtomicOp::Wait || *op == AtomicOp::Notify {
                        if !is_int(t) {
                            return Err(VerifyErrorKind::AtomicShape { op: *op });
                        }
                    } else if is_wide_int(t) != is_wide_int(want_ty) {
                        return Err(VerifyErrorKind::AtomicWidth { width: *width });
                    }
                }
            }
            Op::Const { ty, val } => {
                if !const_fits(*ty, *val) {
                    return Err(VerifyErrorKind::ConstMismatch { ty: *ty, val: *val });
                }
            }
        }
        Ok(())
    }

    /// The type of `e`, checking every node below it.
    fn expr(&self, e: &Expr) -> Result<Type, VerifyErrorKind> {
        match e {
            Expr::Const { ty, val } => {
                if const_fits(*ty, *val) {
                    Ok(*ty)
                } else {
                    Err(VerifyErrorKind::ConstMismatch { ty: *ty, val: *val })
                }
            }
            Expr::Binary { op, lhs, rhs, ty } => {
                self.expect(lhs, &[*ty])?;
                self.expect(rhs, &[*ty])?;
                if !binary_ok(*op, *ty) {
                    return Err(VerifyErrorKind::IllegalOp { op: OpName::Binary(*op), ty: *ty });
                }
                // `ty` is the operand type; a comparison yields an i32 boolean.
                Ok(if is_comparison(*op) { Type::I32 } else { *ty })
            }
            Expr::Unary { op, val, ty } => {
                self.expect(val, &[*ty])?;
                if !unary_ok(*op, *ty) {
                    return Err(VerifyErrorKind::IllegalOp { op: OpName::Unary(*op), ty: *ty });
                }
                Ok(*ty)
            }
            Expr::Convert { from, to, val } => {
                self.expect(val, &[*from])?;
                Ok(*to)
            }
            Expr::Select { cond, then_val, else_val } => {
                self.expect(cond, &INTS)?;
                let t = self.expr(then_val)?;
                self.expect(else_val, &[t])?;
                Ok(t)
            }
            Expr::Load { ty, ptr, .. } => {
                self.expect(ptr, &MEM_ADDR)?;
                Ok(load_type(*ty))
            }
            Expr::LocalGet(idx) => self.local(*idx),
            Expr::GlobalGet(idx) => Ok(self.global(*idx)?.0),
            Expr::LocalTee { idx, val } => {
                let t = self.local(*idx)?;
                self.expect(val, &[t])
            }
            Expr::MemorySize => Ok(Type::I32),
            Expr::MemoryGrow { delta } => {
                self.expect(delta, &INTS)?;
                Ok(Type::I32)
            }
            Expr::Call { func_idx, args } => single(self.call(*func_idx, args)?),
            Expr::CallIndirect { type_idx, table_idx, callee, args } => {
                single(self.call_indirect(*type_idx, *table_idx, callee, args)?)
            }
            Expr::LabelAddr(_) => Err(VerifyErrorKind::UnloweredComputedGoto),
        }
    }
}

fn single(sig: FunctionType) -> Result<Type, VerifyErrorKind> {
    match sig.results.as_slice() {
        [t] => Ok(*t),
        _ => Err(VerifyErrorKind::CallResultArity { results: sig.results }),
    }
}

/// Where an edge goes, before labels are resolved to nodes.
#[derive(Clone, Copy)]
enum To {
    Node(usize),
    Label(LabelId),
}

/// The op-level CFG of one function. Node ids: the ops of one body are
/// numbered contiguously before their children.
struct Cfg {
    paths: Vec<OpPath>,
    label_of: Vec<Option<LabelId>>,
    succs: Vec<Vec<usize>>,
    /// Nodes whose fallthrough leaves the function body.
    falls_off: Vec<usize>,
}

impl Cfg {
    fn build(cx: &FnCx<'_, '_>) -> Cfg {
        let mut b = CfgBuilder { paths: Vec::new(), label_of: Vec::new(), edges: Vec::new(), falls_off: Vec::new() };
        b.body(&cx.func.body, &Vec::new(), None);
        let mut node_of = BTreeMap::new();
        for (n, l) in b.label_of.iter().enumerate() {
            if let Some(l) = l {
                node_of.insert(*l, n);
            }
        }
        let mut succs = vec![Vec::new(); b.paths.len()];
        for (from, to) in b.edges {
            // Every label was checked to be declared before the CFG is built.
            let t = match to {
                To::Node(n) => Some(n),
                To::Label(l) => node_of.get(&l).copied(),
            };
            if let Some(t) = t {
                succs[from].push(t);
            }
        }
        Cfg { paths: b.paths, label_of: b.label_of, succs, falls_off: b.falls_off }
    }

    fn check(&self, func: &Function) -> Result<(), Fail> {
        let n = self.paths.len();
        if n == 0 {
            return if func.results.is_empty() { Ok(()) } else { Err((Vec::new(), VerifyErrorKind::MissingReturn)) };
        }
        let dfs = Dfs::run(&self.succs);
        if !func.results.is_empty() {
            if let Some(&f) = self.falls_off.iter().find(|&&f| dfs.pre[f] != UNSEEN) {
                return Err((self.paths[f].clone(), VerifyErrorKind::MissingReturn));
            }
        }
        check_reducible(self, &dfs)
    }
}

/// The reducibility test. Kept as its own function so it can be switched off
/// in isolation when checking that the irreducible fixtures depend on it.
fn check_reducible(cfg: &Cfg, dfs: &Dfs) -> Result<(), Fail> {
    let idom = dominators(&cfg.succs, dfs);
    for &(src, tgt) in &dfs.retreating {
        if !dominates(&idom, tgt, src) {
            return Err((
                cfg.paths[src].clone(),
                VerifyErrorKind::Irreducible { label: cfg.label_of[tgt], target: cfg.paths[tgt].clone() },
            ));
        }
    }
    Ok(())
}

struct CfgBuilder {
    paths: Vec<OpPath>,
    label_of: Vec<Option<LabelId>>,
    edges: Vec<(usize, To)>,
    falls_off: Vec<usize>,
}

impl CfgBuilder {
    fn edge(&mut self, from: usize, to: Option<usize>) {
        match to {
            Some(t) => self.edges.push((from, To::Node(t))),
            None => self.falls_off.push(from),
        }
    }

    /// Number the ops of `body`, add their edges, and return the first op's
    /// node (or `exit` when the body is empty).
    fn body(&mut self, body: &[Op], base: &OpPath, exit: Option<usize>) -> Option<usize> {
        let first = self.paths.len();
        for (i, op) in body.iter().enumerate() {
            let mut p = base.clone();
            p.push(i);
            self.paths.push(p);
            self.label_of.push(if let Op::Label { id, .. } = op { Some(*id) } else { None });
        }
        for (i, op) in body.iter().enumerate() {
            let n = first + i;
            let next = if i + 1 < body.len() { Some(n + 1) } else { exit };
            let at = self.paths[n].clone();
            let child = |b: &mut Self, c: usize, ops: &[Op]| {
                let mut p = at.clone();
                p.push(c);
                let entry = b.body(ops, &p, next);
                b.edge(n, entry);
            };
            match op {
                Op::Jmp { target } => self.edges.push((n, To::Label(*target))),
                Op::JmpIf { target, .. } => {
                    self.edges.push((n, To::Label(*target)));
                    self.edge(n, next);
                }
                Op::BrTable { targets, default, .. } => {
                    for t in targets {
                        self.edges.push((n, To::Label(*t)));
                    }
                    self.edges.push((n, To::Label(*default)));
                }
                Op::Return { .. } | Op::Unreachable | Op::Throw { .. } | Op::Longjmp { .. } | Op::GotoIndirect { .. } => {}
                Op::TypeSwitch { .. } | Op::Await { .. } => {
                    for (c, b) in child_bodies(op).into_iter().enumerate() {
                        child(self, c, b);
                    }
                    self.edge(n, next);
                }
                Op::Block { .. } | Op::Loop { .. } | Op::If { .. } | Op::Switch { .. } | Op::TryTable { .. } => {
                    for (c, b) in child_bodies(op).into_iter().enumerate() {
                        child(self, c, b);
                    }
                }
                _ => self.edge(n, next),
            }
        }
        if body.is_empty() { exit } else { Some(first) }
    }
}

const UNSEEN: usize = usize::MAX;

/// Iterative depth-first search from node 0. Successors are explored in
/// edge order.
struct Dfs {
    /// Preorder number, `UNSEEN` for unreachable nodes.
    pre: Vec<usize>,
    /// Reachable nodes in reverse postorder.
    rpo: Vec<usize>,
    /// Edges to a node still on the DFS stack, in discovery order.
    retreating: Vec<(usize, usize)>,
}

impl Dfs {
    fn run(succs: &[Vec<usize>]) -> Dfs {
        let n = succs.len();
        let mut pre = vec![UNSEEN; n];
        let mut on_stack = vec![false; n];
        let mut post = Vec::with_capacity(n);
        let mut retreating = Vec::new();
        let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
        pre[0] = 0;
        on_stack[0] = true;
        let mut count = 1;
        while let Some(&mut (v, ref mut k)) = stack.last_mut() {
            if let Some(&w) = succs[v].get(*k) {
                *k += 1;
                if pre[w] == UNSEEN {
                    pre[w] = count;
                    count += 1;
                    on_stack[w] = true;
                    stack.push((w, 0));
                } else if on_stack[w] {
                    retreating.push((v, w));
                }
            } else {
                on_stack[v] = false;
                post.push(v);
                stack.pop();
            }
        }
        post.reverse();
        Dfs { pre, rpo: post, retreating }
    }
}

/// Immediate dominators (Cooper, Harvey, Kennedy: "A Simple, Fast Dominance
/// Algorithm"). `idom[0] == 0`; unreachable nodes get `UNSEEN`.
fn dominators(succs: &[Vec<usize>], dfs: &Dfs) -> Vec<usize> {
    let n = succs.len();
    let mut order = vec![UNSEEN; n];
    for (i, &v) in dfs.rpo.iter().enumerate() {
        order[v] = i;
    }
    let mut preds = vec![Vec::new(); n];
    for &v in &dfs.rpo {
        for &w in &succs[v] {
            preds[w].push(v);
        }
    }
    let mut idom = vec![UNSEEN; n];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &v in dfs.rpo.iter().skip(1) {
            let mut new = UNSEEN;
            for &p in &preds[v] {
                if idom[p] == UNSEEN {
                    continue;
                }
                new = if new == UNSEEN { p } else { intersect(&idom, &order, p, new) };
            }
            if new != idom[v] {
                idom[v] = new;
                changed = true;
            }
        }
    }
    idom
}

fn intersect(idom: &[usize], order: &[usize], mut a: usize, mut b: usize) -> usize {
    while a != b {
        while order[a] > order[b] {
            a = idom[a];
        }
        while order[b] > order[a] {
            b = idom[b];
        }
    }
    a
}

/// Whether `a` dominates `b` (both reachable).
fn dominates(idom: &[usize], a: usize, mut b: usize) -> bool {
    loop {
        if b == a {
            return true;
        }
        if b == 0 {
            return false;
        }
        b = idom[b];
    }
}


#[cfg(test)]
mod tests;

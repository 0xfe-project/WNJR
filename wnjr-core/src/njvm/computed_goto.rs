//! Computed goto lowering (task N1.5).
//!
//! GNU C computed goto (chibicc `ND_LABEL_VAL` = `&&L`, `ND_GOTO_EXPR` =
//! `goto *p`) enters NJVM-IR as two pre-lowering markers, `Expr::LabelAddr` and
//! `Op::GotoIndirect`. [`lower_function`] removes both and produces only ops that
//! already exist (`Label`, `LocalSet`, `Jmp`, `BrTable`, `Unreachable`).
//!
//! # Scheme
//!
//! 1. **Numbering.** Every label whose address is taken (appears in some
//!    `LabelAddr`) gets a number, `base + 1, base + 2, ...` in order of first
//!    appearance. Numbers start at `base + 1` so a zero (null) label value is
//!    never valid. A label value is a plain 32-bit integer (`I32`) and carries no
//!    gc tag.
//! 2. **`&&L`** becomes `Const { I32, number(L) }`.
//! 3. **Dispatch block.** The function gets a fresh `I32` local `sel` and a
//!    dispatch block at its head:
//!
//!    ```text
//!        Jmp entry            ; normal entry skips the dispatch machinery
//!    D:  Label
//!        BrTable (sel - base - 1) [L_1, ..., L_n] default T
//!    T:  Label
//!        Unreachable          ; trap: value not a label of this function
//!        <trampolines>        ; see 5.
//!    entry: Label
//!        <original body, rewritten>
//!    ```
//! 4. **`goto *p`** becomes `LocalSet { sel, p }` + `Jmp D`.
//! 5. **Every other edge into an address-taken label also goes through D.**
//!    `Jmp L` becomes `LocalSet { sel, n }; Jmp D`; `JmpIf c, L` becomes
//!    `LocalSet { sel, n }; JmpIf c, D` (`sel` is dead unless D is reached, so the
//!    early write is harmless); a `BrTable` target `L` is redirected to a
//!    trampoline `Label; LocalSet { sel, n }; Jmp D`; and a fallthrough into
//!    `L` gets `LocalSet { sel, n }; Jmp D` inserted before `L`.
//!
//!    After this, D is the only predecessor of each address-taken label, so D
//!    dominates all of them. Without step 5 a `goto *p` into a loop whose header
//!    is also entered by an ordinary edge makes a cycle with two entries, which is
//!    irreducible and would be rejected by the N3 verifier. With it, the
//!    computed-goto edges add no irreducibility (the CFG is reducible exactly when
//!    the input CFG without the computed-goto edges was).
//!
//! # Scope of label values
//!
//! A label value is only meaningful inside the function that took it. The
//! dispatch index is `sel - base - 1`, compared unsigned against the table
//! length, so any number outside this function's range (including 0 and every
//! other function's numbers) falls to the default and traps. [`lower_module`]
//! gives each function a disjoint range, so `goto *p` with a label value from a
//! different function always traps instead of landing on an unrelated label.

use crate::collections::Vec;
use alloc::collections::BTreeMap;

use super::expr::Expr;
use super::module::{Function, Module};
use super::op::{LabelId, Op};
use super::types::{BinaryOp, ConstVal, Type};

/// Why the lowering refused a function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComputedGotoError {
    /// `&&L` names a label that is not defined in this function.
    UnknownLabel(LabelId),
    /// An address-taken label declares stack params. `BrTable` cannot pass
    /// values; cross-edge values must travel through locals.
    LabelWithParams(LabelId),
    /// Label numbers do not fit in 32 bits (`base` too large).
    NumberOverflow,
}

/// The dispatch machinery that [`lower_function`] added to a function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispatch {
    /// The dispatch block's label (target of every lowered `goto *p`).
    pub label: LabelId,
    /// The trap block's label (the `BrTable` default).
    pub trap: LabelId,
    /// The real entry label the function now jumps to first.
    pub entry: LabelId,
    /// Local index of the selector `sel` (an `I32` appended to `locals`).
    pub selector: u32,
    /// `(label, number)` for each address-taken label, in table order.
    pub numbers: Vec<(LabelId, i32)>,
}

/// Result of lowering one function.
#[derive(Clone, Debug, PartialEq)]
pub struct Lowered {
    pub function: Function,
    /// `None` when the function used no computed goto (body unchanged).
    pub dispatch: Option<Dispatch>,
}

/// Lower computed goto in one function. Label numbers are `base + 1 ..=
/// base + n`; use `base = 0` for a standalone function, or [`lower_module`] to
/// give each function a disjoint range.
pub fn lower_function(func: &Function, base: u32) -> Result<Lowered, ComputedGotoError> {
    // Pass 1: collect defined labels, address-taken labels, max label id.
    let mut defined: BTreeMap<LabelId, bool> = BTreeMap::new(); // id -> has params
    let mut taken: Vec<LabelId> = Vec::new();
    let mut max_label: Option<LabelId> = None;
    let mut has_indirect = false;
    scan(&func.body, &mut |op| {
        let mut bump = |l: LabelId| max_label = Some(max_label.map_or(l, |m| m.max(l)));
        match op {
            Op::Label { id, params } => {
                defined.insert(*id, !params.is_empty());
                bump(*id);
            }
            Op::Jmp { target } | Op::JmpIf { target, .. } => bump(*target),
            Op::BrTable { targets, default, .. } => {
                targets.iter().for_each(|t| bump(*t));
                bump(*default);
            }
            Op::GotoIndirect { .. } => has_indirect = true,
            _ => {}
        }
        for_each_expr(op, &mut |e| {
            walk_expr(e, &mut |e| {
                if let Expr::LabelAddr(l) = e {
                    if !taken.contains(l) {
                        taken.push(*l);
                    }
                }
            })
        });
    });

    if taken.is_empty() && !has_indirect {
        return Ok(Lowered { function: func.clone(), dispatch: None });
    }

    let mut numbers: BTreeMap<LabelId, i32> = BTreeMap::new();
    let mut table: Vec<(LabelId, i32)> = Vec::new();
    for (i, &l) in taken.iter().enumerate() {
        match defined.get(&l) {
            None => return Err(ComputedGotoError::UnknownLabel(l)),
            Some(true) => return Err(ComputedGotoError::LabelWithParams(l)),
            Some(false) => {}
        }
        let n = (i as u32)
            .checked_add(1)
            .and_then(|k| k.checked_add(base))
            .filter(|&n| n <= i32::MAX as u32)
            .ok_or(ComputedGotoError::NumberOverflow)? as i32;
        numbers.insert(l, n);
        table.push((l, n));
    }

    let mut next = max_label.map_or(0, |m| m + 1);
    let mut fresh = || {
        let l = next;
        next += 1;
        l
    };
    let d = fresh();
    let trap = fresh();
    let entry = fresh();
    let sel = (func.params.len() + func.locals.len()) as u32;

    let mut rw = Rewriter { numbers: &numbers, d, sel, tramp: BTreeMap::new(), tramp_ops: Vec::new(), fresh: &mut fresh };
    let body = rw.seq(func.body.clone());
    let tramp_ops = core::mem::take(&mut rw.tramp_ops);

    // BrTable index: sel - (base + 1), so numbers base+1.. map to slots 0..
    let index = Expr::binary(
        BinaryOp::Sub,
        Expr::LocalGet(sel),
        Expr::Const { ty: Type::I32, val: ConstVal::I32(base.wrapping_add(1) as i32) },
        Type::I32,
    );

    let mut out = Vec::new();
    out.push(Op::Jmp { target: entry });
    out.push(Op::Label { id: d, params: Vec::new() });
    out.push(Op::BrTable { index, targets: table.iter().map(|(l, _)| *l).collect(), default: trap });
    out.push(Op::Label { id: trap, params: Vec::new() });
    out.push(Op::Unreachable);
    out.extend(tramp_ops);
    out.push(Op::Label { id: entry, params: Vec::new() });
    out.extend(body);

    let mut locals = func.locals.clone();
    locals.push(Type::I32);
    Ok(Lowered {
        function: Function { params: func.params.clone(), results: func.results.clone(), locals, body: out },
        dispatch: Some(Dispatch { label: d, trap, entry, selector: sel, numbers: table }),
    })
}

/// Lower every function of a module, giving each a disjoint range of label
/// numbers so a label value used in the wrong function traps.
pub fn lower_module(module: &Module) -> Result<Module, ComputedGotoError> {
    let mut out = module.clone();
    let mut base: u32 = 0;
    for f in out.functions.iter_mut() {
        let l = lower_function(f, base)?;
        if let Some(d) = &l.dispatch {
            base = base.checked_add(d.numbers.len() as u32).ok_or(ComputedGotoError::NumberOverflow)?;
        }
        *f = l.function;
    }
    Ok(out)
}

struct Rewriter<'a, F: FnMut() -> LabelId> {
    numbers: &'a BTreeMap<LabelId, i32>,
    d: LabelId,
    sel: u32,
    tramp: BTreeMap<LabelId, LabelId>,
    tramp_ops: Vec<Op>,
    fresh: &'a mut F,
}

impl<F: FnMut() -> LabelId> Rewriter<'_, F> {
    fn set_sel(&self, n: i32) -> Op {
        Op::LocalSet { idx: self.sel, val: Expr::Const { ty: Type::I32, val: ConstVal::I32(n) } }
    }

    fn trampoline(&mut self, l: LabelId) -> LabelId {
        if let Some(&t) = self.tramp.get(&l) {
            return t;
        }
        let t = (self.fresh)();
        let n = self.numbers[&l];
        self.tramp_ops.push(Op::Label { id: t, params: Vec::new() });
        self.tramp_ops.push(self.set_sel(n));
        self.tramp_ops.push(Op::Jmp { target: self.d });
        self.tramp.insert(l, t);
        t
    }

    /// Rewrite one op sequence. The start of a sequence counts as a
    /// fallthrough entry (it is reached from the enclosing op).
    fn seq(&mut self, ops: Vec<Op>) -> Vec<Op> {
        let mut out: Vec<Op> = Vec::new();
        for mut op in ops {
            // &&L -> Const number(L), in this op's own expressions.
            let numbers = self.numbers;
            for_each_expr_mut(&mut op, &mut |e| {
                walk_expr_mut(e, &mut |e| {
                    if let Expr::LabelAddr(l) = e {
                        *e = Expr::Const { ty: Type::I32, val: ConstVal::I32(numbers[l]) };
                    }
                })
            });
            // Nested bodies.
            for b in child_bodies_mut(&mut op) {
                let taken = core::mem::take(b);
                *b = self.seq(taken);
            }
            match op {
                Op::Label { id, .. } if self.numbers.contains_key(&id) => {
                    if out.last().map_or(true, falls_through) {
                        out.push(self.set_sel(self.numbers[&id]));
                        out.push(Op::Jmp { target: self.d });
                    }
                    out.push(op);
                }
                Op::GotoIndirect { target } => {
                    out.push(Op::LocalSet { idx: self.sel, val: target });
                    out.push(Op::Jmp { target: self.d });
                }
                Op::Jmp { target } if self.numbers.contains_key(&target) => {
                    out.push(self.set_sel(self.numbers[&target]));
                    out.push(Op::Jmp { target: self.d });
                }
                Op::JmpIf { cond, target } if self.numbers.contains_key(&target) => {
                    out.push(self.set_sel(self.numbers[&target]));
                    out.push(Op::JmpIf { cond, target: self.d });
                }
                Op::BrTable { index, targets, default } => {
                    let redirect = |l: LabelId, me: &mut Self| if me.numbers.contains_key(&l) { me.trampoline(l) } else { l };
                    let targets = targets.into_iter().map(|t| redirect(t, self)).collect();
                    let default = redirect(default, self);
                    out.push(Op::BrTable { index, targets, default });
                }
                other => out.push(other),
            }
        }
        out
    }
}

/// Whether control can fall from `op` into the next op.
fn falls_through(op: &Op) -> bool {
    !matches!(
        op,
        Op::Jmp { .. }
            | Op::BrTable { .. }
            | Op::Return { .. }
            | Op::Unreachable
            | Op::Throw { .. }
            | Op::Longjmp { .. }
            | Op::GotoIndirect { .. }
    )
}

/// Visit every op, including ops in nested bodies, pre-order.
fn scan(ops: &[Op], f: &mut dyn FnMut(&Op)) {
    for op in ops {
        f(op);
        for b in child_bodies(op) {
            scan(b, f);
        }
    }
}

fn child_bodies(op: &Op) -> Vec<&Vec<Op>> {
    let mut v = Vec::new();
    match op {
        Op::Block { body } | Op::Loop { body } => v.push(body),
        Op::If { then_body, else_body, .. } => {
            v.push(then_body);
            v.push(else_body);
        }
        Op::Switch { cases, default, .. } => {
            v.extend(cases.iter().map(|(_, b)| b));
            v.push(default);
        }
        Op::TypeSwitch { cases, .. } => v.extend(cases.iter().map(|(_, b)| b)),
        Op::TryTable { body, catches } => {
            v.push(body);
            v.extend(catches.iter().map(|(_, b)| b));
        }
        Op::Await { fallback, .. } => v.push(fallback),
        _ => {}
    }
    v
}

fn child_bodies_mut(op: &mut Op) -> Vec<&mut Vec<Op>> {
    let mut v = Vec::new();
    match op {
        Op::Block { body } | Op::Loop { body } => v.push(body),
        Op::If { then_body, else_body, .. } => {
            v.push(then_body);
            v.push(else_body);
        }
        Op::Switch { cases, default, .. } => {
            v.extend(cases.iter_mut().map(|(_, b)| b));
            v.push(default);
        }
        Op::TypeSwitch { cases, .. } => v.extend(cases.iter_mut().map(|(_, b)| b)),
        Op::TryTable { body, catches } => {
            v.push(body);
            v.extend(catches.iter_mut().map(|(_, b)| b));
        }
        Op::Await { fallback, .. } => v.push(fallback),
        _ => {}
    }
    v
}

/// The op's own top-level expressions (not those inside nested bodies).
macro_rules! op_exprs {
    ($op:expr, $f:expr, $iter:ident) => {
        match $op {
            Op::JmpIf { cond, .. } => $f(cond),
            Op::BrTable { index, .. } => $f(index),
            Op::GotoIndirect { target } => $f(target),
            Op::Store { ptr, val, .. } => {
                $f(ptr);
                $f(val);
            }
            Op::If { cond, .. } => $f(cond),
            Op::Return { vals } => vals.$iter().for_each(|e| $f(e)),
            Op::Switch { scrutinee, .. } | Op::TypeSwitch { scrutinee, .. } => $f(scrutinee),
            Op::Call { args, .. } => args.$iter().for_each(|e| $f(e)),
            Op::CallIndirect { callee, args, .. } => {
                $f(callee);
                args.$iter().for_each(|e| $f(e));
            }
            Op::Throw { args, .. } => args.$iter().for_each(|e| $f(e)),
            Op::Setjmp { buf } => $f(buf),
            Op::Longjmp { buf, val } => {
                $f(buf);
                $f(val);
            }
            Op::Await { channel, .. } => $f(channel),
            Op::Yield { val } | Op::LocalSet { val, .. } | Op::GlobalSet { val, .. } | Op::Drop { val } => $f(val),
            Op::MemoryCopy { dest, src, len } => {
                $f(dest);
                $f(src);
                $f(len);
            }
            Op::MemoryFill { dest, val, len } => {
                $f(dest);
                $f(val);
                $f(len);
            }
            Op::MemoryGrow { delta } => $f(delta),
            Op::Label { .. }
            | Op::Jmp { .. }
            | Op::Block { .. }
            | Op::Loop { .. }
            | Op::Unreachable
            | Op::TryTable { .. }
            | Op::MemorySize
            | Op::Atomic { .. }
            | Op::Const { .. }
            | Op::Nop => {}
        }
    };
}

fn for_each_expr(op: &Op, f: &mut dyn FnMut(&Expr)) {
    op_exprs!(op, f, iter)
}

fn for_each_expr_mut(op: &mut Op, f: &mut dyn FnMut(&mut Expr)) {
    op_exprs!(op, f, iter_mut)
}

fn walk_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match e {
        Expr::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, f);
            walk_expr(rhs, f);
        }
        Expr::Unary { val, .. } | Expr::Convert { val, .. } | Expr::LocalTee { val, .. } => walk_expr(val, f),
        Expr::Select { cond, then_val, else_val } => {
            walk_expr(cond, f);
            walk_expr(then_val, f);
            walk_expr(else_val, f);
        }
        Expr::Load { ptr, .. } => walk_expr(ptr, f),
        Expr::Call { args, .. } => args.iter().for_each(|a| walk_expr(a, f)),
        Expr::CallIndirect { callee, args, .. } => {
            walk_expr(callee, f);
            args.iter().for_each(|a| walk_expr(a, f));
        }
        Expr::Const { .. } | Expr::LocalGet(_) | Expr::GlobalGet(_) | Expr::LabelAddr(_) => {}
    }
}

fn walk_expr_mut(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    f(e);
    match e {
        Expr::Binary { lhs, rhs, .. } => {
            walk_expr_mut(lhs, f);
            walk_expr_mut(rhs, f);
        }
        Expr::Unary { val, .. } | Expr::Convert { val, .. } | Expr::LocalTee { val, .. } => walk_expr_mut(val, f),
        Expr::Select { cond, then_val, else_val } => {
            walk_expr_mut(cond, f);
            walk_expr_mut(then_val, f);
            walk_expr_mut(else_val, f);
        }
        Expr::Load { ptr, .. } => walk_expr_mut(ptr, f),
        Expr::Call { args, .. } => args.iter_mut().for_each(|a| walk_expr_mut(a, f)),
        Expr::CallIndirect { callee, args, .. } => {
            walk_expr_mut(callee, f);
            args.iter_mut().for_each(|a| walk_expr_mut(a, f));
        }
        Expr::Const { .. } | Expr::LocalGet(_) | Expr::GlobalGet(_) | Expr::LabelAddr(_) => {}
    }
}

#[cfg(test)]
mod tests;

//! Generic rewriting of NJVM-IR expressions, for IR-to-IR passes written
//! against this harness (including the deliberately wrong pass the self-test uses).

use wnjr_core::njvm::{BinaryOp, Expr, Module, Op};

/// Apply 'f' to every expression node of every function body, children
/// before parents.
pub fn rewrite_exprs(module: &Module, f: &mut dyn FnMut(&mut Expr)) -> Module {
    let mut out = module.clone();
    for func in &mut out.functions {
        for op in &mut func.body {
            walk_op(op, f);
        }
    }
    out
}

fn walk_ops(ops: &mut [Op], f: &mut dyn FnMut(&mut Expr)) {
    for op in ops {
        walk_op(op, f);
    }
}

fn walk_all(es: &mut [Expr], f: &mut dyn FnMut(&mut Expr)) {
    for e in es {
        walk_expr(e, f);
    }
}

pub fn walk_expr(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match e {
        Expr::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, f);
            walk_expr(rhs, f);
        }
        Expr::Unary { val, .. } | Expr::Convert { val, .. } | Expr::LocalTee { val, .. } => {
            walk_expr(val, f)
        }
        Expr::Select {
            cond,
            then_val,
            else_val,
        } => {
            walk_expr(cond, f);
            walk_expr(then_val, f);
            walk_expr(else_val, f);
        }
        Expr::Load { ptr, .. } => walk_expr(ptr, f),
        Expr::MemoryGrow { delta } => walk_expr(delta, f),
        Expr::MemorySize => {}
        Expr::Call { args, .. } => walk_all(args, f),
        Expr::CallIndirect { callee, args, .. } => {
            walk_expr(callee, f);
            walk_all(args, f);
        }
        Expr::Const { .. } | Expr::LocalGet(_) | Expr::GlobalGet(_) | Expr::LabelAddr(_) => {}
    }
    f(e);
}

pub fn walk_op(op: &mut Op, f: &mut dyn FnMut(&mut Expr)) {
    match op {
        Op::JmpIf { cond, .. } => walk_expr(cond, f),
        Op::BrTable { index, .. } => walk_expr(index, f),
        Op::GotoIndirect { target } => walk_expr(target, f),
        Op::Store { ptr, val, .. } => {
            walk_expr(ptr, f);
            walk_expr(val, f);
        }
        Op::Block { body } | Op::Loop { body } => walk_ops(body, f),
        Op::If {
            cond,
            then_body,
            else_body,
        } => {
            walk_expr(cond, f);
            walk_ops(then_body, f);
            walk_ops(else_body, f);
        }
        Op::Return { vals } => walk_all(vals, f),
        Op::Switch {
            scrutinee,
            cases,
            default,
        } => {
            walk_expr(scrutinee, f);
            for (_, body) in cases {
                walk_ops(body, f);
            }
            walk_ops(default, f);
        }
        Op::TypeSwitch { scrutinee, cases } => {
            walk_expr(scrutinee, f);
            for (_, body) in cases {
                walk_ops(body, f);
            }
        }
        Op::Call { args, .. } => walk_all(args, f),
        Op::CallIndirect { callee, args, .. } => {
            walk_expr(callee, f);
            walk_all(args, f);
        }
        Op::TryTable { body, catches } => {
            walk_ops(body, f);
            for c in catches {
                walk_ops(&mut c.body, f);
            }
        }
        Op::Throw { args, .. } => walk_all(args, f),
        Op::Setjmp { buf, .. } => walk_expr(buf, f),
        Op::Longjmp { buf, val } => {
            walk_expr(buf, f);
            walk_expr(val, f);
        }
        Op::Await { channel, fallback } => {
            walk_expr(channel, f);
            walk_ops(fallback, f);
        }
        Op::Yield { val }
        | Op::LocalSet { val, .. }
        | Op::GlobalSet { val, .. }
        | Op::Drop { val } => walk_expr(val, f),
        Op::MemoryCopy { dest, src, len } => {
            walk_expr(dest, f);
            walk_expr(src, f);
            walk_expr(len, f);
        }
        Op::MemoryFill { dest, val, len } => {
            walk_expr(dest, f);
            walk_expr(val, f);
            walk_expr(len, f);
        }
        Op::MemoryGrow { delta } => walk_expr(delta, f),
        Op::Label { .. }
        | Op::Jmp { .. }
        | Op::Unreachable
        | Op::MemorySize
        | Op::Atomic { .. }
        | Op::Const { .. }
        | Op::Nop => {}
    }
}

/// The deliberately wrong pass used by the harness self-test: every Add
/// becomes a Sub. The result still verifies and encodes; only the
/// differential run can tell.
pub fn swap_add_for_sub(module: &Module) -> Module {
    rewrite_exprs(module, &mut |e| {
        if let Expr::Binary { op, .. } = e {
            if *op == BinaryOp::Add {
                *op = BinaryOp::Sub;
            }
        }
    })
}

/// A correct pass with an effect: swap the operands of commutative integer
/// and float `Add`/`Mul` (and the bitwise ops) when both sides are free of
/// calls and local writes, so evaluation order cannot be observed.
pub fn commute_pure_operands(module: &Module) -> Module {
    fn pure(e: &Expr) -> bool {
        match e {
            Expr::Const { .. } | Expr::LocalGet(_) | Expr::GlobalGet(_) => true,
            Expr::Binary { lhs, rhs, .. } => pure(lhs) && pure(rhs),
            Expr::Unary { val, .. } | Expr::Convert { val, .. } => pure(val),
            Expr::Select {
                cond,
                then_val,
                else_val,
            } => pure(cond) && pure(then_val) && pure(else_val),
            Expr::Load { ptr, .. } => pure(ptr),
            // Both read or write the memory size, so neither is safe to
            // reorder against another access.
            Expr::MemorySize => true,
            Expr::MemoryGrow { .. } => false,
            Expr::LocalTee { .. }
            | Expr::Call { .. }
            | Expr::CallIndirect { .. }
            | Expr::LabelAddr(_) => false,
        }
    }
    rewrite_exprs(module, &mut |e| {
        if let Expr::Binary { op, lhs, rhs, .. } = e {
            if matches!(
                op,
                BinaryOp::Add | BinaryOp::Mul | BinaryOp::And | BinaryOp::Or | BinaryOp::Xor
            ) && pure(lhs)
                && pure(rhs)
            {
                core::mem::swap(lhs, rhs);
            }
        }
    })
}

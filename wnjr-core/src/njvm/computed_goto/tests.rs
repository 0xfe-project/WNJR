//! Tests for computed goto lowering.

use super::*;
use crate::collections::{vec, Vec};
use crate::njvm::builder::FunctionBuilder;
use alloc::collections::BTreeSet;

fn c32(n: i32) -> Expr {
    Expr::Const { ty: Type::I32, val: ConstVal::I32(n) }
}

fn label(id: LabelId) -> Op {
    Op::Label { id, params: vec![] }
}

// ── A small CFG + reducibility checker over flat bodies ─────────────────────

/// Basic blocks of a flat body. Block 0 is the implicit entry (ops before the
/// first Label); every Label starts a new block. Returns (block labels, succs).
fn cfg(body: &[Op]) -> (Vec<Option<LabelId>>, Vec<Vec<usize>>) {
    let mut labels: Vec<Option<LabelId>> = vec![None];
    let mut blocks: Vec<Vec<&Op>> = vec![Vec::new()];
    for op in body {
        if let Op::Label { id, .. } = op {
            labels.push(Some(*id));
            blocks.push(Vec::new());
        } else {
            blocks.last_mut().unwrap().push(op);
        }
    }
    let idx = |l: LabelId| labels.iter().position(|x| *x == Some(l)).expect("jump to undefined label");
    let mut succs = Vec::new();
    for (b, ops) in blocks.iter().enumerate() {
        let mut s = Vec::new();
        let mut falls = true;
        for op in ops {
            match op {
                Op::Jmp { target } => { s.push(idx(*target)); falls = false; }
                Op::JmpIf { target, .. } => s.push(idx(*target)),
                Op::BrTable { targets, default, .. } => {
                    targets.iter().for_each(|t| s.push(idx(*t)));
                    s.push(idx(*default));
                    falls = false;
                }
                Op::Return { .. } | Op::Unreachable => falls = false,
                Op::GotoIndirect { .. } => panic!("GotoIndirect survived lowering"),
                _ => {}
            }
            if !falls { break; }
        }
        if falls && b + 1 < blocks.len() { s.push(b + 1); }
        s.sort();
        s.dedup();
        succs.push(s);
    }
    (labels, succs)
}

/// Reducible iff every retreating edge of a DFS from the entry goes to a
/// dominator of its source (equivalently: the back-edge-free graph is acyclic).
fn is_reducible(succs: &[Vec<usize>]) -> bool {
    let n = succs.len();
    // Reachable set.
    let mut reach = vec![false; n];
    let mut st = vec![0usize];
    while let Some(b) = st.pop() {
        if !reach[b] { reach[b] = true; st.extend(succs[b].iter().copied()); }
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for b in 0..n { if reach[b] { for &s in &succs[b] { preds[s].push(b); } } }
    // Iterative dominators.
    let all: BTreeSet<usize> = (0..n).filter(|&b| reach[b]).collect();
    let mut dom: Vec<BTreeSet<usize>> = (0..n).map(|b| if b == 0 { [0].into_iter().collect() } else { all.clone() }).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for b in 1..n {
            if !reach[b] { continue; }
            let mut d: Option<BTreeSet<usize>> = None;
            for &p in &preds[b] {
                d = Some(match d { None => dom[p].clone(), Some(d) => d.intersection(&dom[p]).copied().collect() });
            }
            let mut d = d.unwrap_or_default();
            d.insert(b);
            if d != dom[b] { dom[b] = d; changed = true; }
        }
    }
    // DFS with on-stack marks; a retreating edge u->v must have v dom u.
    fn dfs(u: usize, succs: &[Vec<usize>], dom: &[BTreeSet<usize>], state: &mut [u8]) -> bool {
        state[u] = 1;
        for &v in &succs[u] {
            if state[v] == 1 && !dom[u].contains(&v) { return false; }
            if state[v] == 0 && !dfs(v, succs, dom, state) { return false; }
        }
        state[u] = 2;
        true
    }
    let mut state = vec![0u8; n];
    dfs(0, succs, &dom, &mut state)
}

fn preds_of(labels: &[Option<LabelId>], succs: &[Vec<usize>], l: LabelId) -> Vec<Option<LabelId>> {
    let t = labels.iter().position(|x| *x == Some(l)).unwrap();
    (0..succs.len()).filter(|&b| succs[b].contains(&t)).map(|b| labels[b]).collect()
}

// ── Acceptance ───────────────────────────────────────────────────────────────

/// The acceptance-criteria function: labels L0/L1/L2, `&&L1` and `&&L2`
/// taken, one `goto *p`. Locals: 0 = cond, 1 = which (params), 2 = p.
///
/// ```text
/// L0: if cond goto L2      ; ordinary edge into L2
///     p = &&L1
///     goto L1              ; ordinary edge into L1
/// L1: cond = 99
///     goto *p              ; computed edge into L1 or L2
/// L2: p = &&L2
///     if which goto L1     ; ordinary edge into L1
///     return
/// ```
fn acceptance_input() -> Function {
    let (l0, l1, l2, p) = (0, 1, 2, 2);
    let mut fb = FunctionBuilder::new(vec![Type::I32, Type::I32], vec![], vec![Type::I32]);
    fb.label(l0, vec![]);
    fb.jmp_if(Expr::LocalGet(0), l2);
    fb.push_op(Op::LocalSet { idx: p, val: Expr::LabelAddr(l1) });
    fb.jmp(l1);
    fb.label(l1, vec![]);
    fb.push_op(Op::LocalSet { idx: 0, val: c32(99) });
    fb.push_op(Op::GotoIndirect { target: Expr::LocalGet(p) });
    fb.label(l2, vec![]);
    fb.push_op(Op::LocalSet { idx: p, val: Expr::LabelAddr(l2) });
    fb.jmp_if(Expr::LocalGet(1), l1);
    fb.push_op(Op::Return { vals: vec![] });
    fb.build()
}

#[test]
fn acceptance_three_labels_two_taken() {
    let (l0, l1, l2, p) = (0, 1, 2, 2);
    let f = acceptance_input();
    let Lowered { function: out, dispatch } = lower_function(&f, 0).unwrap();
    let d = dispatch.expect("dispatch block");
    // Fresh labels after max id 2; selector appended after params+locals.
    assert_eq!((d.label, d.trap, d.entry, d.selector), (3, 4, 5, 3));
    assert_eq!(out.locals, vec![Type::I32, Type::I32]);
    // (b) numbering: L1 -> 1, L2 -> 2.
    assert_eq!(d.numbers, vec![(l1, 1), (l2, 2)]);

    let sel = d.selector;
    let set_sel = |n| Op::LocalSet { idx: sel, val: c32(n) };
    let jd = Op::Jmp { target: d.label };
    let expected = vec![
        // (a) dispatch block at the head.
        Op::Jmp { target: d.entry },
        label(d.label),
        Op::BrTable {
            index: Expr::binary(BinaryOp::Sub, Expr::LocalGet(sel), c32(1), Type::I32),
            targets: vec![l1, l2],
            default: d.trap,
        },
        label(d.trap),
        Op::Unreachable,
        label(d.entry),
        // L0
        label(l0),
        set_sel(2),                                             // (d) if cond goto L2 ...
        Op::JmpIf { cond: Expr::LocalGet(0), target: d.label }, //     ... via dispatch
        Op::LocalSet { idx: p, val: c32(1) },                   // (b) p = &&L1
        set_sel(1),                                             // (d) goto L1 via dispatch
        jd.clone(),
        // L1
        label(l1),
        Op::LocalSet { idx: 0, val: c32(99) },
        Op::LocalSet { idx: sel, val: Expr::LocalGet(p) },      // (c) goto *p
        jd.clone(),
        // L2
        label(l2),
        Op::LocalSet { idx: p, val: c32(2) },                   // (b) p = &&L2
        set_sel(1),                                             // (d) if which goto L1 via dispatch
        Op::JmpIf { cond: Expr::LocalGet(1), target: d.label },
        Op::Return { vals: vec![] },
    ];
    assert_eq!(out.body, expected);

    // (d) D is the only predecessor of L1 and L2, and the CFG is reducible.
    let (labels, succs) = cfg(&out.body);
    assert_eq!(preds_of(&labels, &succs, l1), vec![Some(d.label)]);
    assert_eq!(preds_of(&labels, &succs, l2), vec![Some(d.label)]);
    assert!(is_reducible(&succs));
}

/// Control for the checker: lowering `goto *p` to a BrTable in place, without
/// routing the other edges through one dispatch block, leaves the cycle
/// {L1, L2} with two entries from L0. That CFG is irreducible.
#[test]
fn naive_lowering_is_irreducible() {
    let mut f = acceptance_input();
    for op in f.body.iter_mut() {
        if let Op::GotoIndirect { target } = op {
            *op = Op::BrTable {
                index: Expr::binary(BinaryOp::Sub, target.clone(), c32(1), Type::I32),
                targets: vec![1, 2],
                default: 2,
            };
        }
    }
    let (_, succs) = cfg(&f.body);
    assert!(!is_reducible(&succs));
}

// ── Edge cases ───────────────────────────────────────────────────────────────

#[test]
fn no_computed_goto_is_identity() {
    let mut fb = FunctionBuilder::new(vec![Type::I32], vec![Type::I32], vec![]);
    fb.label(0, vec![]);
    fb.push_op(Op::Return { vals: vec![Expr::LocalGet(0)] });
    let f = fb.build();
    let l = lower_function(&f, 0).unwrap();
    assert!(l.dispatch.is_none());
    assert_eq!(l.function, f);
}

/// Fallthrough into a taken label gets `sel = n; Jmp D` inserted before it.
#[test]
fn fallthrough_is_redirected() {
    let mut fb = FunctionBuilder::new(vec![], vec![], vec![Type::I32]);
    fb.label(0, vec![]);
    fb.push_op(Op::LocalSet { idx: 0, val: Expr::LabelAddr(1) });
    fb.label(1, vec![]); // reached by falling through from L0
    fb.push_op(Op::Return { vals: vec![] });
    let Lowered { function: out, dispatch } = lower_function(&fb.build(), 0).unwrap();
    let d = dispatch.unwrap();
    let at = out.body.iter().position(|o| *o == label(1)).unwrap();
    assert_eq!(out.body[at - 2], Op::LocalSet { idx: d.selector, val: c32(1) });
    assert_eq!(out.body[at - 1], Op::Jmp { target: d.label });
    let (labels, succs) = cfg(&out.body);
    assert_eq!(preds_of(&labels, &succs, 1), vec![Some(d.label)]);
    assert!(is_reducible(&succs));
}

/// A BrTable target that is a taken label goes through a trampoline.
#[test]
fn br_table_target_uses_trampoline() {
    let mut fb = FunctionBuilder::new(vec![Type::I32], vec![], vec![Type::I32]);
    fb.label(0, vec![]);
    fb.push_op(Op::LocalSet { idx: 1, val: Expr::LabelAddr(1) });
    fb.push_op(Op::BrTable { index: Expr::LocalGet(0), targets: vec![1, 2, 1], default: 2 });
    fb.label(1, vec![]);
    fb.push_op(Op::GotoIndirect { target: Expr::LocalGet(1) });
    fb.label(2, vec![]);
    fb.push_op(Op::Return { vals: vec![] });
    let Lowered { function: out, dispatch } = lower_function(&fb.build(), 0).unwrap();
    let d = dispatch.unwrap();
    let bt = out.body.iter().rev().find_map(|o| match o {
        Op::BrTable { targets, default, .. } if *default == 2 => Some(targets.clone()),
        _ => None,
    }).unwrap();
    let t = bt[0];
    assert_eq!(bt, vec![t, 2, t], "one shared trampoline for L1, L2 untouched");
    let at = out.body.iter().position(|o| *o == label(t)).unwrap();
    assert_eq!(out.body[at + 1], Op::LocalSet { idx: d.selector, val: c32(1) });
    assert_eq!(out.body[at + 2], Op::Jmp { target: d.label });
    let (labels, succs) = cfg(&out.body);
    assert_eq!(preds_of(&labels, &succs, 1), vec![Some(d.label)]);
    assert!(is_reducible(&succs));
}

#[test]
fn label_addr_inside_nested_body() {
    let mut fb = FunctionBuilder::new(vec![], vec![], vec![Type::I32]);
    fb.label(0, vec![]);
    fb.push_op(Op::Block {
        body: vec![
            Op::LocalSet { idx: 0, val: Expr::LabelAddr(1) },
            Op::GotoIndirect { target: Expr::LocalGet(0) },
        ],
    });
    fb.label(1, vec![]);
    fb.push_op(Op::Return { vals: vec![] });
    let Lowered { function: out, dispatch } = lower_function(&fb.build(), 0).unwrap();
    let d = dispatch.unwrap();
    let inner = out.body.iter().find_map(|o| match o { Op::Block { body } => Some(body.clone()), _ => None }).unwrap();
    assert_eq!(inner, vec![
        Op::LocalSet { idx: 0, val: c32(1) },
        Op::LocalSet { idx: d.selector, val: Expr::LocalGet(0) },
        Op::Jmp { target: d.label },
    ]);
}

#[test]
fn unknown_label_is_rejected() {
    let mut fb = FunctionBuilder::new(vec![], vec![], vec![Type::I32]);
    fb.label(0, vec![]);
    fb.push_op(Op::LocalSet { idx: 0, val: Expr::LabelAddr(99) });
    fb.push_op(Op::Return { vals: vec![] });
    assert_eq!(lower_function(&fb.build(), 0).unwrap_err(), ComputedGotoError::UnknownLabel(99));
}

#[test]
fn taken_label_with_params_is_rejected() {
    let mut fb = FunctionBuilder::new(vec![], vec![], vec![Type::I32]);
    fb.label(0, vec![]);
    fb.push_op(Op::LocalSet { idx: 0, val: Expr::LabelAddr(1) });
    fb.jmp(1);
    fb.label(1, vec![Type::I32]);
    fb.push_op(Op::Return { vals: vec![] });
    assert_eq!(lower_function(&fb.build(), 0).unwrap_err(), ComputedGotoError::LabelWithParams(1));
}

/// `goto *p` with no taken label: dispatch has an empty table, always traps.
#[test]
fn goto_indirect_without_labels_traps() {
    let mut fb = FunctionBuilder::new(vec![Type::I32], vec![], vec![]);
    fb.label(0, vec![]);
    fb.push_op(Op::GotoIndirect { target: Expr::LocalGet(0) });
    let Lowered { function: out, dispatch } = lower_function(&fb.build(), 0).unwrap();
    let d = dispatch.unwrap();
    assert!(d.numbers.is_empty());
    assert!(matches!(&out.body[2], Op::BrTable { targets, default, .. } if targets.is_empty() && *default == d.trap));
}

/// Module lowering gives functions disjoint number ranges, and each
/// function's BrTable index subtracts its own base, so a foreign number is
/// out of range and reaches the trap default.
#[test]
fn module_ranges_are_disjoint() {
    let one = |taken: &[LabelId]| {
        let mut fb = FunctionBuilder::new(vec![], vec![], vec![Type::I32]);
        fb.label(0, vec![]);
        for &l in taken { fb.push_op(Op::LocalSet { idx: 0, val: Expr::LabelAddr(l) }); }
        fb.push_op(Op::GotoIndirect { target: Expr::LocalGet(0) });
        for &l in taken { fb.label(l, vec![]); fb.push_op(Op::Return { vals: vec![] }); }
        fb.build()
    };
    let mut m = Module::new();
    m.functions.push(one(&[1, 2]));
    m.functions.push(Function::new(vec![], vec![], vec![])); // no computed goto
    m.functions.push(one(&[1]));
    let out = lower_module(&m).unwrap();
    assert_eq!(lower_function(&m.functions[0], 0).unwrap().dispatch.unwrap().numbers, vec![(1, 1), (2, 2)]);
    assert_eq!(out.functions[1], m.functions[1]);
    let third = lower_function(&m.functions[2], 2).unwrap();
    assert_eq!(third.dispatch.as_ref().unwrap().numbers, vec![(1, 3)]);
    assert_eq!(out.functions[2], third.function);
    // Third function's table index is sel - 3: numbers 1 and 2 (function 0's)
    // wrap to huge unsigned values and hit the default trap.
    assert!(matches!(&out.functions[2].body[2],
        Op::BrTable { index: Expr::Binary { rhs, .. }, .. } if **rhs == c32(3)));
}

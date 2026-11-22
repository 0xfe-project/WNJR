//! N9 setjmp/longjmp lowering (#602): `Op::Setjmp` / `Op::Longjmp` ->
//! `TryTable` / `Throw` of one module-level tag.
//!
//! # Scheme
//!
//! - **Tag.** [`lower_module`] appends one tag `(label: i32, val: i32)` when
//!   any function uses setjmp or longjmp. setjmp and longjmp share it.
//! - **Site numbers.** Each `Setjmp` gets a number, `base + 1 ..= base + n`
//!   per function, disjoint across the module (as in `computed_goto`). The
//!   jmp_buf holds that number as an `i32` at offset 0; nothing else.
//! - **`dest = setjmp(buf)`** becomes
//!   ```text
//!       store.i32 buf, number(i)
//!       dest = 0                 ; direct return
//!       jmp C_i
//!   R_i:                          ; resume after a longjmp
//!       dest = cv; sel = 0
//!   C_i:
//!   ```
//! - **`longjmp(buf, val)`** becomes `throw tag (load.i32 buf, v + (v == 0))`
//!   with `v` teed into a temp, so `longjmp(buf, 0)` makes setjmp return 1.
//! - **Function with setjmp.** The body is wrapped:
//!   ```text
//!       sel = 0
//!   H:  try_table
//!         br_table sel [E, T_1 .. T_n] default TRAP
//!   TRAP: unreachable
//!   E:    <body, rewritten>; return (or unreachable)
//!       catch tag bind (cl, cv)
//!         if (u32)(cl - base - 1) >= n: rethrow (throw tag cl cv)
//!         sel = cl - base
//!       end
//!       jmp H
//!   ```
//!   A longjmp to a site of this function lands in the catch, which sets
//!   `sel` and re-enters the try at the dispatch `br_table`. A number from
//!   another function (or 0, an unset jmp_buf) is rethrown to the callers;
//!   uncaught it traps.
//! - **Reducibility.** The dispatch is a second entry into the body. A
//!   resume point R_i inside loops L_1 ⊃ ... ⊃ L_m (headers h_1..h_m) is not
//!   reached directly: T_i = h_1, and right after each header's label a
//!   `jmp_if sel == i` goes on to h_{k+1}, the last one to R_i. Every loop is
//!   then still entered only through its header, so the body stays reducible
//!   when it was. `sel` is reset at R_i so later normal passes through the
//!   headers do not redirect.
//!
//! # Scope and known limits
//!
//! - `Setjmp` must be in the function's top-level (flat CFG) body; nested in a
//!   structured op it is rejected ([`SetjmpError::NotTopLevel`]).
//! - The jmp_buf identifies the site, not the activation. A longjmp into an
//!   outer activation of a function while an inner activation of the same
//!   function is on the stack resumes in the inner one (recursion through a
//!   setjmp function). C programs that do this are rare; not handled.
//! - Locals keep their latest values across a longjmp (the frame is the
//!   same wasm frame), which is a valid refinement of C's "indeterminate".

use alloc::collections::{BTreeMap, BTreeSet};

use crate::collections::{vec, Vec};

use super::expr::Expr;
use super::module::{Function, Module, Tag};
use super::op::{Catch, LabelId, Op};
use super::types::{BinaryOp, ConstVal, MemType, Type};

/// Why the lowering refused a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetjmpError {
    /// A `Setjmp` nested inside a structured op of function `func` (unified
    /// index space).
    NotTopLevel { func: u32 },
    /// Site numbers do not fit in a positive `i32`.
    NumberOverflow,
}

/// One lowered function.
#[derive(Clone, Debug, PartialEq)]
pub struct Lowered {
    pub function: Function,
    /// Number of setjmp sites (numbers `base + 1 ..= base + sites`).
    pub sites: u32,
}

/// The params of the setjmp/longjmp tag: (site number, value).
pub const TAG_PARAMS: [Type; 2] = [Type::I32, Type::I32];

/// Lower every function. Adds the tag (at index `imported_tags + tags.len()`)
/// only if some function uses setjmp or longjmp; otherwise returns the module
/// unchanged.
pub fn lower_module(module: &Module) -> Result<Module, SetjmpError> {
    if !module.functions.iter().any(|f| {
        any_op(&f.body, &|o| {
            matches!(o, Op::Setjmp { .. } | Op::Longjmp { .. })
        })
    }) {
        return Ok(module.clone());
    }
    let mut out = module.clone();
    let tag = out.imported_tags() + out.tags.len() as u32;
    out.tags.push(Tag {
        params: TAG_PARAMS.to_vec(),
    });
    let imported = out.imported_functions();
    let mut base: u32 = 0;
    for (i, f) in out.functions.iter_mut().enumerate() {
        let l = lower_function(f, imported + i as u32, base, tag)?;
        base = base
            .checked_add(l.sites)
            .ok_or(SetjmpError::NumberOverflow)?;
        *f = l.function;
    }
    Ok(out)
}

fn i32c(v: i32) -> Expr {
    Expr::Const {
        ty: Type::I32,
        val: ConstVal::I32(v),
    }
}

fn bin(op: BinaryOp, ty: Type, a: Expr, b: Expr) -> Expr {
    Expr::binary(op, a, b, ty)
}

/// Lower one function. `func_idx` is only used in errors; `base` is the
/// last site number used by earlier functions; `tag` the setjmp tag index.
pub fn lower_function(
    func: &Function,
    func_idx: u32,
    base: u32,
    tag: u32,
) -> Result<Lowered, SetjmpError> {
    let is_setjmp = |o: &Op| matches!(o, Op::Setjmp { .. });
    for op in &func.body {
        if child_bodies(op).into_iter().any(|b| any_op(b, &is_setjmp)) {
            return Err(SetjmpError::NotTopLevel { func: func_idx });
        }
    }
    let sites: Vec<usize> = func
        .body
        .iter()
        .enumerate()
        .filter(|(_, o)| is_setjmp(o))
        .map(|(i, _)| i)
        .collect();
    let has_longjmp = any_op(&func.body, &|o| matches!(o, Op::Longjmp { .. }));
    if sites.is_empty() && !has_longjmp {
        return Ok(Lowered {
            function: func.clone(),
            sites: 0,
        });
    }
    let n = sites.len() as u32;
    if base.checked_add(n).map_or(true, |m| m > i32::MAX as u32) {
        return Err(SetjmpError::NumberOverflow);
    }

    let mut locals = func.locals.clone();
    let first = (func.params.len() + func.locals.len()) as u32;
    let alloc_i32 = |locals: &mut Vec<Type>| {
        let i = first + (locals.len() - func.locals.len()) as u32;
        locals.push(Type::I32);
        i
    };

    // longjmp -> throw, everywhere. One op for one op: indices are kept.
    let mut body = func.body.clone();
    if has_longjmp {
        let tv = alloc_i32(&mut locals);
        rewrite_longjmp(&mut body, tv, tag);
    }
    if sites.is_empty() {
        return Ok(Lowered {
            function: Function {
                params: func.params.clone(),
                results: func.results.clone(),
                locals,
                body,
            },
            sites: 0,
        });
    }
    let sel = alloc_i32(&mut locals);
    let cl = alloc_i32(&mut locals);
    let cv = alloc_i32(&mut locals);

    let mut next = max_label(&func.body).map_or(0, |m| m + 1);
    let mut fresh = || {
        let l = next;
        next += 1;
        l
    };
    let h = fresh();
    let entry = fresh();
    let trap = fresh();
    let rethrow = fresh();
    let handler_end = fresh();
    let resume: Vec<LabelId> = sites.iter().map(|_| fresh()).collect();
    let cont: Vec<LabelId> = sites.iter().map(|_| fresh()).collect();

    // Dispatch chain through enclosing loop headers (see module docs).
    let cfg = Cfg::build(&body);
    let mut first_hop: Vec<LabelId> = Vec::with_capacity(sites.len());
    // header op index -> (site number i (1-based), next hop)
    let mut checks: BTreeMap<usize, Vec<(i32, LabelId)>> = BTreeMap::new();
    for (k, &p) in sites.iter().enumerate() {
        let i = k as i32 + 1;
        let headers = cfg.enclosing_headers(cfg.block_of[p]);
        let hop_labels: Vec<(usize, LabelId)> = headers
            .iter()
            .filter_map(|&hb| match &body[cfg.starts[hb]] {
                Op::Label { id, .. } => Some((cfg.starts[hb], *id)),
                _ => None,
            })
            .collect();
        if hop_labels.is_empty() {
            first_hop.push(resume[k]);
        } else {
            first_hop.push(hop_labels[0].1);
            for (j, &(at, _)) in hop_labels.iter().enumerate() {
                let to = hop_labels.get(j + 1).map_or(resume[k], |x| x.1);
                checks.entry(at).or_default().push((i, to));
            }
        }
    }

    let mut targets = vec![entry];
    targets.extend(first_hop);
    let mut tb: Vec<Op> = vec![
        Op::BrTable {
            index: Expr::LocalGet(sel),
            targets,
            default: trap,
        },
        Op::Label {
            id: trap,
            params: Vec::new(),
        },
        Op::Unreachable,
        Op::Label {
            id: entry,
            params: Vec::new(),
        },
    ];
    let mut site_k = 0usize;
    for (p, op) in body.into_iter().enumerate() {
        match op {
            Op::Setjmp { buf, dest } => {
                let k = site_k;
                site_k += 1;
                let number = (base + k as u32 + 1) as i32;
                tb.push(Op::Store {
                    ty: MemType::I32,
                    ptr: buf,
                    val: i32c(number),
                    offset: 0,
                    align: 2,
                });
                tb.push(Op::LocalSet {
                    idx: dest,
                    val: i32c(0),
                });
                tb.push(Op::Jmp { target: cont[k] });
                tb.push(Op::Label {
                    id: resume[k],
                    params: Vec::new(),
                });
                tb.push(Op::LocalSet {
                    idx: dest,
                    val: Expr::LocalGet(cv),
                });
                tb.push(Op::LocalSet {
                    idx: sel,
                    val: i32c(0),
                });
                tb.push(Op::Label {
                    id: cont[k],
                    params: Vec::new(),
                });
            }
            op @ Op::Label { .. } => {
                tb.push(op);
                for &(i, to) in checks.get(&p).map(|v| v.as_slice()).unwrap_or(&[]) {
                    tb.push(Op::JmpIf {
                        cond: bin(BinaryOp::Eq, Type::I32, Expr::LocalGet(sel), i32c(i)),
                        target: to,
                    });
                }
            }
            op => tb.push(op),
        }
    }
    // Falling off the original body ends the function; it must not reach the
    // `jmp H` after the try.
    tb.push(if func.results.is_empty() {
        Op::Return { vals: Vec::new() }
    } else {
        Op::Unreachable
    });

    let lo = base as i32 + 1;
    let out_of_range = bin(
        BinaryOp::Ge,
        Type::U32,
        Expr::Convert {
            from: Type::I32,
            to: Type::U32,
            val: bin(BinaryOp::Sub, Type::I32, Expr::LocalGet(cl), i32c(lo)).boxed(),
        },
        Expr::Const {
            ty: Type::U32,
            val: ConstVal::I32(n as i32),
        },
    );
    let handler = vec![
        Op::JmpIf {
            cond: out_of_range,
            target: rethrow,
        },
        Op::LocalSet {
            idx: sel,
            val: bin(
                BinaryOp::Sub,
                Type::I32,
                Expr::LocalGet(cl),
                i32c(base as i32),
            ),
        },
        Op::Jmp {
            target: handler_end,
        },
        Op::Label {
            id: rethrow,
            params: Vec::new(),
        },
        Op::Throw {
            tag_idx: tag,
            args: vec![Expr::LocalGet(cl), Expr::LocalGet(cv)],
        },
        Op::Label {
            id: handler_end,
            params: Vec::new(),
        },
    ];

    let out = vec![
        Op::LocalSet {
            idx: sel,
            val: i32c(0),
        },
        Op::Label {
            id: h,
            params: Vec::new(),
        },
        Op::TryTable {
            body: tb,
            catches: vec![Catch {
                tag: Some(tag),
                bind: vec![cl, cv],
                body: handler,
            }],
        },
        Op::Jmp { target: h },
    ];
    Ok(Lowered {
        function: Function {
            params: func.params.clone(),
            results: func.results.clone(),
            locals,
            body: out,
        },
        sites: n,
    })
}

/// `Longjmp { buf, val }` -> `Throw tag [load.i32 buf, (tv = val) + (tv == 0)]`.
fn rewrite_longjmp(body: &mut [Op], tv: u32, tag: u32) {
    for op in body.iter_mut() {
        for b in child_bodies_mut(op) {
            rewrite_longjmp(b, tv, tag);
        }
        if let Op::Longjmp { buf, val } = op {
            let buf = core::mem::replace(buf, i32c(0));
            let val = core::mem::replace(val, i32c(0));
            let v = bin(
                BinaryOp::Add,
                Type::I32,
                Expr::LocalTee {
                    idx: tv,
                    val: val.boxed(),
                },
                bin(BinaryOp::Eq, Type::I32, Expr::LocalGet(tv), i32c(0)),
            );
            let label = Expr::Load {
                ty: MemType::I32,
                ptr: buf.boxed(),
                offset: 0,
                align: 2,
            };
            *op = Op::Throw {
                tag_idx: tag,
                args: vec![label, v],
            };
        }
    }
}

fn any_op(body: &[Op], f: &dyn Fn(&Op) -> bool) -> bool {
    body.iter()
        .any(|op| f(op) || child_bodies(op).into_iter().any(|b| any_op(b, f)))
}

fn max_label(body: &[Op]) -> Option<LabelId> {
    let mut m: Option<LabelId> = None;
    let mut bump = |l: LabelId| m = Some(m.map_or(l, |x| x.max(l)));
    fn walk(body: &[Op], bump: &mut dyn FnMut(LabelId)) {
        for op in body {
            match op {
                Op::Label { id, .. } => bump(*id),
                Op::Jmp { target } | Op::JmpIf { target, .. } => bump(*target),
                Op::BrTable {
                    targets, default, ..
                } => {
                    targets.iter().for_each(|t| bump(*t));
                    bump(*default);
                }
                _ => {}
            }
            for b in child_bodies(op) {
                walk(b, bump);
            }
        }
    }
    walk(body, &mut bump);
    m
}

fn child_bodies(op: &Op) -> Vec<&Vec<Op>> {
    let mut v = Vec::new();
    match op {
        Op::Block { body } | Op::Loop { body } => v.push(body),
        Op::If {
            then_body,
            else_body,
            ..
        } => {
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
            v.extend(catches.iter().map(|c| &c.body));
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
        Op::If {
            then_body,
            else_body,
            ..
        } => {
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
            v.extend(catches.iter_mut().map(|c| &mut c.body));
        }
        Op::Await { fallback, .. } => v.push(fallback),
        _ => {}
    }
    v
}

// ── loops of the top-level flat CFG ─────────────────────────────────────────

const UNSEEN: usize = usize::MAX;

struct Cfg {
    /// First op index of each block.
    starts: Vec<usize>,
    /// Op index -> block.
    block_of: Vec<usize>,
    preds: Vec<Vec<usize>>,
    /// Immediate dominator (entry: itself; unreachable: UNSEEN).
    idom: Vec<usize>,
    /// Loop header block -> blocks of its natural loop (merged over back edges).
    loops: BTreeMap<usize, BTreeSet<usize>>,
}

fn is_terminator(op: &Op) -> bool {
    matches!(
        op,
        Op::Jmp { .. }
            | Op::JmpIf { .. }
            | Op::BrTable { .. }
            | Op::Return { .. }
            | Op::Unreachable
            | Op::Throw { .. }
            | Op::Longjmp { .. }
            | Op::GotoIndirect { .. }
    )
}

impl Cfg {
    fn build(body: &[Op]) -> Cfg {
        let n = body.len();
        let mut starts: Vec<usize> = Vec::new();
        for i in 0..n {
            let leader =
                i == 0 || matches!(body[i], Op::Label { .. }) || is_terminator(&body[i - 1]);
            if leader {
                starts.push(i);
            }
        }
        let nb = starts.len();
        let mut block_of = vec![0usize; n];
        for (b, &s) in starts.iter().enumerate() {
            let e = starts.get(b + 1).copied().unwrap_or(n);
            for i in s..e {
                block_of[i] = b;
            }
        }
        let mut label_block: BTreeMap<LabelId, usize> = BTreeMap::new();
        for (b, &s) in starts.iter().enumerate() {
            if let Op::Label { id, .. } = &body[s] {
                label_block.insert(*id, b);
            }
        }
        let mut succs: Vec<Vec<usize>> = vec![Vec::new(); nb];
        for b in 0..nb {
            let e = starts.get(b + 1).copied().unwrap_or(n);
            let fall = if b + 1 < nb { Some(b + 1) } else { None };
            let add = |l: &LabelId, s: &mut Vec<usize>| {
                if let Some(&t) = label_block.get(l) {
                    s.push(t);
                }
            };
            let s = &mut succs[b];
            match &body[e - 1] {
                Op::Jmp { target } => add(target, s),
                Op::JmpIf { target, .. } => {
                    add(target, s);
                    s.extend(fall);
                }
                Op::BrTable {
                    targets, default, ..
                } => {
                    targets.iter().for_each(|t| add(t, s));
                    add(default, s);
                }
                o if is_terminator(o) => {}
                _ => s.extend(fall),
            }
        }
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); nb];
        for (b, ss) in succs.iter().enumerate() {
            for &t in ss {
                preds[t].push(b);
            }
        }
        // Reverse postorder from block 0 (iterative DFS).
        let mut rpo_num = vec![UNSEEN; nb];
        let mut post: Vec<usize> = Vec::new();
        if nb > 0 {
            let mut seen = vec![false; nb];
            let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
            seen[0] = true;
            while let Some(&mut (b, ref mut k)) = stack.last_mut() {
                if *k < succs[b].len() {
                    let t = succs[b][*k];
                    *k += 1;
                    if !seen[t] {
                        seen[t] = true;
                        stack.push((t, 0));
                    }
                } else {
                    post.push(b);
                    stack.pop();
                }
            }
        }
        let rpo: Vec<usize> = post.iter().rev().copied().collect();
        for (i, &b) in rpo.iter().enumerate() {
            rpo_num[b] = i;
        }
        // Cooper-Harvey-Kennedy.
        let mut idom = vec![UNSEEN; nb];
        if nb > 0 {
            idom[0] = 0;
            let mut changed = true;
            while changed {
                changed = false;
                for &b in rpo.iter().skip(1) {
                    let mut new: Option<usize> = None;
                    for &p in &preds[b] {
                        if idom[p] == UNSEEN {
                            continue;
                        }
                        new = Some(match new {
                            None => p,
                            Some(mut a) => {
                                let mut c = p;
                                while a != c {
                                    while rpo_num[a] > rpo_num[c] {
                                        a = idom[a];
                                    }
                                    while rpo_num[c] > rpo_num[a] {
                                        c = idom[c];
                                    }
                                }
                                a
                            }
                        });
                    }
                    if let Some(d) = new {
                        if idom[b] != d {
                            idom[b] = d;
                            changed = true;
                        }
                    }
                }
            }
        }
        let mut cfg = Cfg {
            starts,
            block_of,
            preds,
            idom,
            loops: BTreeMap::new(),
        };
        for &b in &rpo {
            for &h in &succs[b] {
                if cfg.dominates(h, b) {
                    let mut body: BTreeSet<usize> = BTreeSet::new();
                    body.insert(h);
                    let mut work = vec![b];
                    while let Some(x) = work.pop() {
                        if cfg.idom[x] != UNSEEN && body.insert(x) {
                            work.extend(cfg.preds[x].iter().copied());
                        }
                    }
                    cfg.loops.entry(h).or_default().extend(body);
                }
            }
        }
        cfg
    }

    fn dominates(&self, a: usize, mut b: usize) -> bool {
        if self.idom[b] == UNSEEN || self.idom[a] == UNSEEN {
            return false;
        }
        loop {
            if a == b {
                return true;
            }
            if b == 0 {
                return false;
            }
            b = self.idom[b];
        }
    }

    /// Headers of the loops containing `b`, outermost first.
    fn enclosing_headers(&self, b: usize) -> Vec<usize> {
        let mut hs: Vec<(usize, usize)> = self
            .loops
            .iter()
            .filter(|(_, body)| body.contains(&b))
            .map(|(&h, body)| (body.len(), h))
            .collect();
        hs.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)));
        hs.into_iter().map(|(_, h)| h).collect()
    }
}

#[cfg(test)]
mod tests;

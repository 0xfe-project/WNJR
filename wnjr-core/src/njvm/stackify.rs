//! N6 stackify: flat CFG -> structured control flow (Block, Loop, br).
//!
//! [`stackify`] takes a function body whose top level is a flat CFG of
//! `Label`/`Jmp`/`JmpIf`/`BrTable` plus straight-line statements (already
//! validated by the N3 verifier: reducible, operand stack empty at every
//! label) and rebuilds it as a structured [`Node`] tree that maps directly to
//! wasm `block`/`loop`/`br`/`br_if`/`br_table`. N5 type erasure emits the tree.
//!
//! # Algorithm (dominators + reverse postorder; Ramsey, "Beyond Relooper")
//!
//! 1. Cut the body into basic blocks. A block starts at index 0, at every
//!    `Label`, and right after every `Jmp`/`JmpIf`/`BrTable`/`Return`/`Unreachable`.
//!    Each block is straight-line statements then one terminator (an edge).
//! 2. Build the CFG, find the blocks reachable from the entry, number them in
//!    reverse postorder and compute the dominator tree (Cooper-Harvey-Kennedy).
//! 3. Classify each block:
//!    - **loop head**: target of a back edge (edge whose target dominates its
//!      source). Gets a `loop`; branches to it (back edges and forward
//!      entries) are `br` to that loop.
//!    - **needs a block**: has >= 2 forward predecessors, or is the forward
//!      target/default of a `BrTable`. Gets a `block` so forward branches can
//!      `br` out to it.
//!    A block that is neither is emitted inline at its single forward edge.
//! 4. Walk the dominator tree. At a node, its dominator children that need a
//!    block are wrapped in nested `block`s, opened in reverse-RPO order so the
//!    later (higher-RPO) child is the outer block, and the node's own body sits
//!    innermost. A loop head's `loop` encloses all of these. Each edge becomes `br`/
//!    `br_if`/`br_table` whose depth counts the enclosing constructs to the
//!    target, or is inlined when the target needs no label.
//!
//! Unreachable blocks are dropped.
//!
//! # Scope
//!
//! Only a top-level flat CFG is handled. A `Jmp`/`JmpIf`/`BrTable` nested
//! inside a structured op (`Block`, `Loop`, `If`, `Switch`, `TryTable`, ...`) is
//! rejected with [`StackifyError::NestedJump`]. Non-empty `Label` params are
//! rejected (cross-edge values travel through locals, per the IR contract).

use alloc::collections::{BTreeMap, BTreeSet};

use crate::collections::{vec, Vec};

use super::op::{LabelId, Op};

/// Why stackify refused a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackifyError {
    /// A `Jmp`/`JmpIf`/`BrTable` is nested inside a structured op. N6 only
    /// restructures a top-level flat CFG.
    NestedJump,
    /// A `Label` declares stack params; cross-edge values must go through locals.
    LabelWithParams(LabelId),
    /// A jump names a label with no matching `Label` op.
    UnknownLabel(LabelId),
    /// A branch target's construct was not found in scope while computing its
    /// depth. On a CFG the N3 verifier accepted this cannot happen; it means a
    /// bug in this pass (kept as an error rather than a panic).
    LabelNotInScope(usize),
    /// A block would be emitted twice. As above, only reachable on a pass bug.
    DoubleEmit(usize),
}

/// A structured control-flow node, isomorphic to a wasm instruction.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// A wasm `block`: a forward `br` to it lands just after `body`.
    Block { body: Vec<Node> },
    /// A wasm `loop`: a `br` to it lands back at the top of `body`.
    Loop { body: Vec<Node> },
    /// A wasm `if`. `cond` is the body op holding the condition expression.
    If { cond: usize, then_body: Vec<Node>, else_body: Vec<Node> },
    /// `br depth` (0 = innermost enclosing construct).
    Br { depth: u32 },
    /// `br_if depth`; `cond` is the body op holding the condition.
    BrIf { cond: usize, depth: u32 },
    /// `br_table`; `index` is the body op holding the index expression.
    BrTable { index: usize, targets: Vec<u32>, default: u32 },
    /// A straight-line statement: body op `idx` (anything but a label or jump).
    Stmt { idx: usize },
    /// Control reaches the end of the function body. N5 emits `return` for a
    /// function without results and `unreachable` otherwise, so it never falls
    /// into the code that follows an enclosing construct.
    FallOffEnd,
}

/// The maximum nesting depth of `Block`/`Loop`/`If` in the tree (for tests that
/// guard against wrapping every label).
pub fn max_depth(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            Node::Block { body } | Node::Loop { body } => 1 + max_depth(body),
            Node::If { then_body, else_body, .. } => {
                1 + max_depth(then_body).max(max_depth(else_body))
            }
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

// ── basic blocks ────────────────────────────────────────────────────────────

/// One block's terminator: the control edge(s) leaving it. `END` means the
/// edge falls off the function end.
const END: usize = usize::MAX;

#[derive(Clone, Debug)]
enum Term {
    /// No successor: the block ends in `Return`/`Unreachable`, or the body ends.
    Stop,
    /// One edge (`Jmp`, or fallthrough into the next label).
    Uncond(usize),
    /// `JmpIf`: `cond` taken -> `taken`, else -> `not_taken`.
    Cond { cond: usize, taken: usize, not_taken: usize },
    /// `BrTable`.
    Switch { index: usize, targets: Vec<usize>, default: usize },
}

#[derive(Clone, Debug)]
struct Block {
    stmts: Vec<usize>,
    term: Term,
}

/// True if any `Jmp`/`JmpIf`/`BrTable` appears anywhere inside these bodies.
fn body_has_jump(body: &[Op]) -> bool {
    body.iter().any(op_has_jump)
}

fn op_has_jump(op: &Op) -> bool {
    match op {
        Op::Jmp { .. } | Op::JmpIf { .. } | Op::BrTable { .. } => true,
        Op::Block { body } | Op::Loop { body } => body_has_jump(body),
        Op::If { then_body, else_body, .. } => body_has_jump(then_body) || body_has_jump(else_body),
        Op::Switch { cases, default, .. } => {
            cases.iter().any(|(_, b)| body_has_jump(b)) || body_has_jump(default)
        }
        Op::TypeSwitch { cases, .. } => cases.iter().any(|(_, b)| body_has_jump(b)),
        Op::TryTable { body, catches } => {
            body_has_jump(body) || catches.iter().any(|(_, b)| body_has_jump(b))
        }
        Op::Await { fallback, .. } => body_has_jump(fallback),
        _ => false,
    }
}

/// Does this top-level body contain a flat CFG op at its top level?
pub fn needs_stackify(body: &[Op]) -> bool {
    body.iter().any(|op| matches!(op, Op::Label { .. } | Op::Jmp { .. } | Op::JmpIf { .. } | Op::BrTable { .. }))
}

/// Cut `body` into basic blocks. Returns the blocks (block 0 is the entry) and
/// the map from label id to block index.
fn build_blocks(body: &[Op]) -> Result<Vec<Block>, StackifyError> {
    let n = body.len();
    // Leaders: start of each basic block.
    let mut leader = vec![false; n];
    if n > 0 {
        leader[0] = true;
    }
    for (i, op) in body.iter().enumerate() {
        match op {
            Op::Label { params, id } => {
                if !params.is_empty() {
                    return Err(StackifyError::LabelWithParams(*id));
                }
                leader[i] = true;
            }
            Op::Jmp { .. } | Op::JmpIf { .. } | Op::BrTable { .. } => {
                if i + 1 < n {
                    leader[i + 1] = true;
                }
            }
            Op::Return { .. } | Op::Unreachable | Op::Throw { .. } => {
                if i + 1 < n {
                    leader[i + 1] = true;
                }
            }
            other => {
                if op_has_jump(other) {
                    return Err(StackifyError::NestedJump);
                }
            }
        }
    }

    // Leader indices in order, and the block index of each leader.
    let mut starts: Vec<usize> = Vec::new();
    let mut block_of_index: BTreeMap<usize, usize> = BTreeMap::new();
    for i in 0..n {
        if leader[i] {
            block_of_index.insert(i, starts.len());
            starts.push(i);
        }
    }

    // Label id -> block index.
    let mut label_block: BTreeMap<LabelId, usize> = BTreeMap::new();
    for (bi, &s) in starts.iter().enumerate() {
        if let Op::Label { id, .. } = &body[s] {
            label_block.insert(*id, bi);
        }
    }
    let resolve = |l: LabelId| label_block.get(&l).copied().ok_or(StackifyError::UnknownLabel(l));

    // Build each block from its span.
    let mut blocks: Vec<Block> = Vec::with_capacity(starts.len());
    for (bi, &s) in starts.iter().enumerate() {
        let end = starts.get(bi + 1).copied().unwrap_or(n);
        let mut stmts: Vec<usize> = Vec::new();
        let mut term = Term::Stop;
        let mut ended = false;
        for i in s..end {
            match &body[i] {
                Op::Label { .. } => {} // the label marker emits nothing
                Op::Jmp { target } => {
                    term = Term::Uncond(resolve(*target)?);
                    ended = true;
                }
                Op::JmpIf { target, .. } => {
                    let taken = resolve(*target)?;
                    let not_taken = block_of_index.get(&(i + 1)).copied().unwrap_or(END);
                    term = Term::Cond { cond: i, taken, not_taken };
                    ended = true;
                }
                Op::BrTable { targets, default, .. } => {
                    let mut ts = Vec::with_capacity(targets.len());
                    for t in targets {
                        ts.push(resolve(*t)?);
                    }
                    term = Term::Switch { index: i, targets: ts, default: resolve(*default)? };
                    ended = true;
                }
                Op::Return { .. } | Op::Unreachable | Op::Throw { .. } => {
                    stmts.push(i);
                    term = Term::Stop;
                    ended = true;
                }
                _ => stmts.push(i),
            }
        }
        // Block ended by hitting the next label (no explicit terminator op):
        // fall through to that block.
        if !ended {
            if let Some(&next) = block_of_index.get(&end) {
                term = Term::Uncond(next);
            } else {
                term = Term::Uncond(END); // falls off the function end
            }
        }
        blocks.push(Block { stmts, term });
    }

    Ok(blocks)
}

// ── CFG analysis ─────────────────────────────────────────────────────────────

fn succs(t: &Term) -> Vec<usize> {
    match t {
        Term::Stop => Vec::new(),
        Term::Uncond(x) => {
            if *x == END { Vec::new() } else { vec![*x] }
        }
        Term::Cond { taken, not_taken, .. } => {
            let mut v = Vec::new();
            if *taken != END {
                v.push(*taken);
            }
            if *not_taken != END && *not_taken != *taken {
                v.push(*not_taken);
            }
            v
        }
        Term::Switch { targets, default, .. } => {
            let mut v: Vec<usize> = Vec::new();
            for &x in targets {
                if x != END && !v.contains(&x) {
                    v.push(x);
                }
            }
            if *default != END && !v.contains(default) {
                v.push(*default);
            }
            v
        }
    }
}

struct Cfg {
    blocks: Vec<Block>,
    /// Reverse-postorder position of each block; `u32::MAX` if unreachable.
    rpo_num: Vec<u32>,
    reach: Vec<bool>,
    idom: Vec<usize>,
    dom_children: Vec<Vec<usize>>,
    is_loop: Vec<bool>,
    needs_block: Vec<bool>,
}

impl Cfg {
    fn analyze(blocks: Vec<Block>) -> Cfg {
        let n = blocks.len();
        // DFS postorder from entry (block 0), iterative.
        let mut rpo_num = vec![u32::MAX; n];
        let mut postorder: Vec<usize> = Vec::new();
        if n > 0 {
            let mut visited = vec![false; n];
            let mut stack: Vec<(usize, Vec<usize>, usize)> = vec![(0, succs(&blocks[0].term), 0)];
            visited[0] = true;
            while !stack.is_empty() {
                let top = stack.len() - 1;
                let (node, i) = (stack[top].0, stack[top].2);
                if i < stack[top].1.len() {
                    let next = stack[top].1[i];
                    stack[top].2 += 1;
                    if !visited[next] {
                        visited[next] = true;
                        stack.push((next, succs(&blocks[next].term), 0));
                    }
                } else {
                    postorder.push(node);
                    stack.pop();
                }
            }
        }
        // Reverse postorder numbering.
        let mut order: Vec<usize> = postorder.clone();
        order.reverse();
        for (pos, &b) in order.iter().enumerate() {
            rpo_num[b] = pos as u32;
        }

        // Predecessors over reachable blocks.
        let reach: Vec<bool> = (0..n).map(|b| rpo_num[b] != u32::MAX).collect();
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        for b in 0..n {
            if reach[b] {
                for s in succs(&blocks[b].term) {
                    if reach[s] {
                        preds[s].push(b);
                    }
                }
            }
        }

        // Dominators (Cooper-Harvey-Kennedy), entry = 0.
        let mut idom = vec![usize::MAX; n];
        if n > 0 {
            idom[0] = 0;
            let rpo_order: Vec<usize> = order.iter().copied().filter(|&b| b != 0).collect();
            let intersect = |mut a: usize, mut b: usize, idom: &[usize]| -> usize {
                while a != b {
                    while rpo_num[a] > rpo_num[b] {
                        a = idom[a];
                    }
                    while rpo_num[b] > rpo_num[a] {
                        b = idom[b];
                    }
                }
                a
            };
            let mut changed = true;
            while changed {
                changed = false;
                for &b in &rpo_order {
                    let mut new_idom = usize::MAX;
                    for &p in &preds[b] {
                        if idom[p] != usize::MAX {
                            new_idom = if new_idom == usize::MAX {
                                p
                            } else {
                                intersect(p, new_idom, &idom)
                            };
                        }
                    }
                    if new_idom != usize::MAX && idom[b] != new_idom {
                        idom[b] = new_idom;
                        changed = true;
                    }
                }
            }
        }

        let mut dom_children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for b in 0..n {
            if b != 0 && reach[b] && idom[b] != usize::MAX {
                dom_children[idom[b]].push(b);
            }
        }

        let mut cfg = Cfg {
            blocks,
            rpo_num,
            reach,
            idom,
            dom_children,
            is_loop: vec![false; n],
            needs_block: vec![false; n],
        };

        // Classify: back edges -> loop heads; forward preds; brtable forced blocks.
        let mut forward_preds: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
        let mut force_block = vec![false; n];
        for b in 0..n {
            if !cfg.reach[b] {
                continue;
            }
            for s in succs(&cfg.blocks[b].term) {
                if !cfg.reach[s] {
                    continue;
                }
                if cfg.dominates(s, b) {
                    cfg.is_loop[s] = true; // back edge
                } else {
                    forward_preds[s].insert(b);
                }
            }
            // BrTable forward targets must be branchable (get a block).
            if let Term::Switch { targets, default, .. } = &cfg.blocks[b].term {
                for &t in targets.iter().chain(core::iter::once(default)) {
                    if t != END && cfg.reach[t] && !cfg.dominates(t, b) {
                        force_block[t] = true;
                    }
                }
            }
        }
        for b in 0..n {
            if !cfg.reach[b] {
                continue;
            }
            let fp = forward_preds[b].len();
            cfg.needs_block[b] = fp >= 2 || force_block[b];
        }
        cfg
    }

    /// Does `a` dominate `b`? Walks the idom chain from `b` to the entry.
    fn dominates(&self, a: usize, b: usize) -> bool {
        let mut x = b;
        loop {
            if x == a {
                return true;
            }
            let p = self.idom[x];
            if p == x || p == usize::MAX {
                return false;
            }
            x = p;
        }
    }

    /// Is the edge `from -> to` emitted as a branch (rather than inlined)? A back
    /// edge branches to its loop; a forward edge branches iff the target has a
    /// block. A loop head with one forward predecessor is inlined there.
    fn branches(&self, from: usize, to: usize) -> bool {
        if self.dominates(to, from) {
            self.is_loop[to]
        } else {
            self.needs_block[to]
        }
    }
}

// ── emission ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Ctx {
    Loop(usize),
    Block(usize),
    If,
}

struct Emit<'c> {
    cfg: &'c Cfg,
    emitted: BTreeSet<usize>,
}

impl Emit<'_> {
    /// Ramsey's `doTree`: emit `v` and the dominator children that need a
    /// block. A loop head's `loop` encloses those blocks, so code inside the
    /// loop that branches back to the head sees the `loop` in scope.
    fn do_tree(&mut self, v: usize, ctx: &mut Vec<Ctx>) -> Result<Vec<Node>, StackifyError> {
        if !self.emitted.insert(v) {
            return Err(StackifyError::DoubleEmit(v));
        }
        let mut merges: Vec<usize> = self.cfg.dom_children[v]
            .iter()
            .copied()
            .filter(|&c| self.cfg.needs_block[c])
            .collect();
        // Highest RPO first: it becomes the outermost block, emitted last.
        merges.sort_by(|&a, &b| self.cfg.rpo_num[b].cmp(&self.cfg.rpo_num[a]));
        if self.cfg.is_loop[v] {
            ctx.push(Ctx::Loop(v));
            let body = self.node_within(v, &merges, ctx)?;
            ctx.pop();
            Ok(vec![Node::Loop { body }])
        } else {
            self.node_within(v, &merges, ctx)
        }
    }

    fn node_within(
        &mut self,
        v: usize,
        merges: &[usize],
        ctx: &mut Vec<Ctx>,
    ) -> Result<Vec<Node>, StackifyError> {
        let Some((&y, rest)) = merges.split_first() else {
            return self.node_body(v, ctx);
        };
        ctx.push(Ctx::Block(y));
        let inner = self.node_within(v, rest, ctx)?;
        ctx.pop();
        let mut out = vec![Node::Block { body: inner }];
        out.extend(self.do_tree(y, ctx)?);
        Ok(out)
    }

    fn node_body(&mut self, v: usize, ctx: &mut Vec<Ctx>) -> Result<Vec<Node>, StackifyError> {
        let mut out: Vec<Node> =
            self.cfg.blocks[v].stmts.iter().map(|&idx| Node::Stmt { idx }).collect();
        match self.cfg.blocks[v].term.clone() {
            Term::Stop => {}
            Term::Uncond(t) => {
                let tail = self.edge(v, t, ctx)?;
                out.extend(tail);
            }
            Term::Cond { cond, taken, not_taken } => {
                if taken == not_taken {
                    let tail = self.edge(v, taken, ctx)?;
                    out.extend(tail);
                } else if taken != END && self.cfg.branches(v, taken) {
                    out.push(Node::BrIf { cond, depth: self.depth(v, taken, ctx)? });
                    let tail = self.edge(v, not_taken, ctx)?;
                    out.extend(tail);
                } else {
                    ctx.push(Ctx::If);
                    let then_body = self.edge(v, taken, ctx)?;
                    let else_body = self.edge(v, not_taken, ctx)?;
                    ctx.pop();
                    out.push(Node::If { cond, then_body, else_body });
                }
            }
            Term::Switch { index, targets, default } => {
                let mut ts = Vec::with_capacity(targets.len());
                for t in targets {
                    ts.push(self.depth(v, t, ctx)?);
                }
                let d = self.depth(v, default, ctx)?;
                out.push(Node::BrTable { index, targets: ts, default: d });
            }
        }
        Ok(out)
    }

    /// Emit an unconditional edge `v -> t`: a `br` when `t` has a label, the
    /// inlined subtree when it does not, and [`Node::FallOffEnd`] at the body end.
    fn edge(&mut self, v: usize, t: usize, ctx: &mut Vec<Ctx>) -> Result<Vec<Node>, StackifyError> {
        if t == END {
            return Ok(vec![Node::FallOffEnd]);
        }
        if self.cfg.branches(v, t) {
            Ok(vec![Node::Br { depth: self.depth(v, t, ctx)? }])
        } else {
            self.do_tree(t, ctx)
        }
    }

    /// Depth (enclosing-construct count) of the branch `from -> to`.
    fn depth(&self, from: usize, to: usize, ctx: &[Ctx]) -> Result<u32, StackifyError> {
        let back = self.cfg.dominates(to, from);
        for i in (0..ctx.len()).rev() {
            let hit = match ctx[i] {
                Ctx::Loop(x) => back && x == to,
                Ctx::Block(x) => !back && x == to,
                Ctx::If => false,
            };
            if hit {
                return Ok((ctx.len() - 1 - i) as u32);
            }
        }
        Err(StackifyError::LabelNotInScope(to))
    }
}

/// Restructure a top-level flat CFG into a structured [`Node`] tree. Each
/// `Node::Stmt`/`If`/`BrIf`/`BrTable` carries the index of the originating op in
/// `body`, so N5 can emit the statement/condition/index from the same body.
pub fn stackify(body: &[Op]) -> Result<Vec<Node>, StackifyError> {
    let blocks = build_blocks(body)?;
    if blocks.is_empty() {
        return Ok(Vec::new());
    }
    let cfg = Cfg::analyze(blocks);
    let mut emit = Emit { cfg: &cfg, emitted: BTreeSet::new() };
    let mut ctx: Vec<Ctx> = Vec::new();
    emit.do_tree(0, &mut ctx)
}

#[cfg(test)]
mod tests;

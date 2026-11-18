//! NJVM-IR operation (statement/control flow) enum, groups A–H.

use serde::{Deserialize, Serialize};
use crate::collections::Vec;

use super::expr::Expr;
use super::types::{AtomicOp, ConstVal, MemType, Type};

/// A label id for Jmp/JmpIf/BrTable targets (group A).
pub type LabelId = u32;

/// One catch clause of `Op::TryTable`.
///
/// `tag` is the tag caught, `None` = catch_all. On entry to `body` the
/// exception payload is stored into the locals `bind` (one per tag param, in
/// order; empty for catch_all), so the operand stack is empty as at a label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Catch {
    pub tag: Option<u32>,
    pub bind: Vec<u32>,
    pub body: Vec<Op>,
}

/// An NJVM-IR operation. This is the top-level instruction/statement form, not
/// the tree-form expressions (which live in `Expr`).
///
/// Eight groups:
/// - A: flat CFG (Label, Jmp, JmpIf, BrTable)
/// - B: tree expressions (covered by `Expr` and Store)
/// - C: structured control (Block, Loop, If, Return, Unreachable, Switch, TypeSwitch)
/// - D: function calls (Call, CallIndirect; also represented in `Expr` for value context)
/// - E: exception handling (TryTable, Throw, Setjmp, Longjmp)
/// - F: coroutines (Await, Yield)
/// - G: memory and locals (LocalSet, GlobalSet, MemoryCopy, MemoryFill, MemorySize, MemoryGrow, Atomic)
/// - H: other (Const as statement, Drop, Nop; Const as expression is in `Expr`)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    // ─────────────────────────────────────────────────────────────────────────
    // Group A: flat CFG
    // ─────────────────────────────────────────────────────────────────────────
    /// Block entry with a unique label. `params` describes the expected stack
    /// shape on entry. Constraint (verified in N3): operand stack must be empty
    /// at label entry; cross-edge values travel through locals.
    Label { id: LabelId, params: Vec<Type> },

    /// Unconditional jump to the target label.
    Jmp { target: LabelId },

    /// Conditional jump: jump to `target` if `cond` is nonzero.
    JmpIf { cond: Expr, target: LabelId },

    /// Indirect jump (switch on `index`). Falls through to `default` if
    /// `index >= targets.len()`.
    BrTable {
        index: Expr,
        targets: Vec<LabelId>,
        default: LabelId,
    },

    /// GNU `goto *target` (chibicc `ND_GOTO_EXPR`). `target` is a label value
    /// produced by `Expr::LabelAddr`. Pre-lowering marker only:
    /// `computed_goto::lower_function` turns it into `LocalSet` + `Jmp` to the
    /// function's dispatch block. Code handed to later passes must not contain it.
    GotoIndirect { target: Expr },

    // ─────────────────────────────────────────────────────────────────────────
    // Group B: expression statements
    // ─────────────────────────────────────────────────────────────────────────
    /// Memory store. `ptr` and `val` are tree expressions.
    Store {
        ty: MemType,
        ptr: Expr,
        val: Expr,
        offset: u32,
        align: u32,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Group C: structured control flow
    // ─────────────────────────────────────────────────────────────────────────
    /// A basic block: sequence of ops (no explicit label/jump; used for
    /// structured nesting).
    Block { body: Vec<Op> },

    /// A loop: repeated execution. The body may jump back to the loop head.
    Loop { body: Vec<Op> },

    /// Conditional: execute `then_body` if `cond` is nonzero, else `else_body`.
    If {
        cond: Expr,
        then_body: Vec<Op>,
        else_body: Vec<Op>,
    },

    /// Return from the function, yielding the given values.
    Return { vals: Vec<Expr> },

    /// Trap: unreachable code marker.
    Unreachable,

    /// Multi-way integer switch (jump to the arm matching `scrutinee`).
    Switch {
        scrutinee: Expr,
        cases: Vec<(ConstVal, Vec<Op>)>,
        default: Vec<Op>,
    },

    /// Type-based multi-way switch (discriminate on type tag). Used for GC ref
    /// dispatch (future extension; N1 just reserves the variant).
    TypeSwitch {
        scrutinee: Expr,
        cases: Vec<(Type, Vec<Op>)>,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Group D: function calls (as statements)
    // ─────────────────────────────────────────────────────────────────────────
    /// Direct call (statement form, discarding or binding the result).
    Call { func_idx: u32, args: Vec<Expr> },

    /// Indirect call (statement form).
    CallIndirect {
        type_idx: u32,
        table_idx: u32,
        callee: Expr,
        args: Vec<Expr>,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Group E: exception handling
    // ─────────────────────────────────────────────────────────────────────────
    /// Try-catch table: execute `body`, catch exceptions per `catches`
    /// (tried in order). `body` and every handler body are regions of their
    /// own: a jump inside one may only target a label of the same region (N3
    /// rejects jumps into a try; stackify N6 restructures each region alone).
    /// Falling off the end of the body or a handler continues after the op.
    TryTable {
        body: Vec<Op>,
        catches: Vec<Catch>,
    },

    /// Throw an exception with the given tag and arguments.
    Throw { tag_idx: u32, args: Vec<Expr> },

    /// C `dest = setjmp(buf)`. `buf` is the jmp_buf address, `dest` an `I32`
    /// local that receives 0 on the direct return and the longjmp value on a
    /// resume. Pre-lowering marker: `setjmp::lower_module` replaces it (#602).
    /// Only allowed in a function's top-level body.
    Setjmp { buf: Expr, dest: u32 },

    /// C `longjmp(buf, val)`, `val` an `I32`. Pre-lowering marker, lowered to
    /// `Throw` of the module's setjmp tag by `setjmp::lower_module`.
    Longjmp { buf: Expr, val: Expr },

    // ─────────────────────────────────────────────────────────────────────────
    // Group F: coroutines / async
    // ─────────────────────────────────────────────────────────────────────────
    /// Await: suspend until `channel` is ready, execute `fallback` if canceled.
    Await {
        channel: Expr,
        fallback: Vec<Op>,
    },

    /// Yield: suspend and produce `val` to the caller.
    Yield { val: Expr },

    // ─────────────────────────────────────────────────────────────────────────
    // Group G: memory, locals, globals, atomics
    // ─────────────────────────────────────────────────────────────────────────
    /// Write to a local variable (statement form).
    LocalSet { idx: u32, val: Expr },

    /// Write to a global variable.
    GlobalSet { idx: u32, val: Expr },

    /// Bulk memory copy.
    MemoryCopy {
        dest: Expr,
        src: Expr,
        len: Expr,
    },

    /// Bulk memory fill.
    MemoryFill {
        dest: Expr,
        val: Expr,
        len: Expr,
    },

    /// Query current memory size in pages.
    MemorySize,

    /// Grow memory by `delta` pages, returning the old size or −1 on failure.
    MemoryGrow { delta: Expr },

    /// Atomic memory operation. Full operand shape (pointer, value, ordering)
    /// is defined in N2; N1 keeps op, width, and offset per the IR sketch.
    Atomic {
        op: AtomicOp,
        width: u8,
        offset: u32,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Group H: other
    // ─────────────────────────────────────────────────────────────────────────
    /// Const as a statement (group H). Const as an expression is `Expr::Const`.
    Const { ty: Type, val: ConstVal },

    /// Drop: discard a value (when an expression is evaluated but its result is unused).
    Drop { val: Expr },

    /// No-op.
    Nop,
}

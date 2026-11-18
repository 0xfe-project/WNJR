//! Fixtures for the NJVM-IR verifier. Every bad fixture asserts the error
//! kind and the exact op position, not just `is_err`.

use crate::collections::{vec, Vec};
use crate::njvm::{
    lower_function, verify_module, BinaryOp, ConstVal, Expr, Function, FunctionBuilder,
    FunctionType, Global, GlobalType, InitExpr, LabelId, MemType, Module, ModuleBuilder, Op,
    OpName, TableType, Tag, Type, UnaryOp, VerifyError, VerifyErrorKind,
};

fn c32(v: i32) -> Expr {
    Expr::Const { ty: Type::I32, val: ConstVal::I32(v) }
}

fn f32c(v: f32) -> Expr {
    Expr::Const { ty: Type::F32, val: ConstVal::f32(v) }
}

fn label(id: LabelId) -> Op {
    Op::Label { id, params: vec![] }
}

fn ret() -> Op {
    Op::Return { vals: vec![] }
}

fn func(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>, body: Vec<Op>) -> Function {
    Function { params, results, locals, body }
}

fn module(funcs: Vec<Function>) -> Module {
    let mut mb = ModuleBuilder::new();
    for f in funcs {
        mb.add_function(f);
    }
    mb.build()
}

fn verify_one(f: Function) -> Result<(), VerifyError> {
    verify_module(&module(vec![f]))
}

/// Run and expect an error at `at` in function 0.
fn err_at(f: Function, at: &[usize]) -> VerifyErrorKind {
    let e = verify_one(f).expect_err("verifier accepted a bad fixture");
    assert_eq!(e.func, 0, "{e}");
    assert_eq!(e.at, at, "{e}");
    e.kind
}

// ── Good inputs ─────────────────────────────────────────────────────────────

/// N1's `add`: Label; Return(a + b).
#[test]
fn ok_n1_add() {
    let mut fb = FunctionBuilder::new(vec![Type::I32, Type::I32], vec![Type::I32], vec![]);
    let entry = fb.next_label();
    fb.label(entry, vec![]);
    fb.push_op(Op::Return {
        vals: vec![Expr::binary(BinaryOp::Add, Expr::LocalGet(0), Expr::LocalGet(1), Type::I32)],
    });
    verify_one(fb.build()).unwrap();
}

/// N1's two-label counting loop.
#[test]
fn ok_n1_two_label_loop() {
    let mut fb = FunctionBuilder::new(vec![], vec![], vec![Type::I32]);
    fb.push_op(Op::LocalSet { idx: 0, val: c32(0) });
    let head = fb.next_label();
    let exit = fb.next_label();
    fb.label(head, vec![]);
    fb.jmp_if(Expr::binary(BinaryOp::Ge, Expr::LocalGet(0), c32(10), Type::I32), exit);
    fb.push_op(Op::LocalSet {
        idx: 0,
        val: Expr::binary(BinaryOp::Add, Expr::LocalGet(0), c32(1), Type::I32),
    });
    fb.jmp(head);
    fb.label(exit, vec![]);
    fb.push_op(ret());
    verify_one(fb.build()).unwrap();
}

fn inc(idx: u32) -> Op {
    Op::LocalSet { idx, val: Expr::binary(BinaryOp::Add, Expr::LocalGet(idx), c32(1), Type::I32) }
}

fn ge10(idx: u32) -> Expr {
    Expr::binary(BinaryOp::Ge, Expr::LocalGet(idx), c32(10), Type::I32)
}

/// Two nested flat loops: for i { for j { } }.
#[test]
fn ok_nested_loop() {
    let (oh, ox, ih, ix) = (0, 1, 2, 3);
    let body = vec![
        Op::LocalSet { idx: 0, val: c32(0) },
        label(oh),
        Op::JmpIf { cond: ge10(0), target: ox },
        Op::LocalSet { idx: 1, val: c32(0) },
        label(ih),
        Op::JmpIf { cond: ge10(1), target: ix },
        inc(1),
        Op::Jmp { target: ih },
        label(ix),
        inc(0),
        Op::Jmp { target: oh },
        label(ox),
        ret(),
    ];
    verify_one(func(vec![], vec![], vec![Type::I32, Type::I32], body)).unwrap();
}

/// A flat loop nested in structured control: labels inside `Block`/`If` bodies,
/// with a back edge from inside the `If` to a label in the enclosing `Block`.
#[test]
fn ok_loop_inside_structured() {
    let body = vec![
        Op::Block {
            body: vec![
                label(0),
                inc(0),
                Op::If { cond: ge10(0), then_body: vec![Op::Nop], else_body: vec![Op::Jmp { target: 0 }] },
            ],
        },
        ret(),
    ];
    verify_one(func(vec![], vec![], vec![Type::I32], body)).unwrap();
}

// Computed goto fixtures. Labels: D dispatch, T trap, L1 and L2 address-taken
// (numbers 1 and 2). Param 0 is a condition, local 1 is `sel`.
const D: LabelId = 10;
const T: LabelId = 11;
const L1: LabelId = 1;
const L2: LabelId = 2;
const SEL: u32 = 1;

fn set_sel(n: i32) -> Op {
    Op::LocalSet { idx: SEL, val: c32(n) }
}

/// `D: BrTable(sel - 1) [L1, L2] default T; T: Unreachable`
fn dispatch() -> Vec<Op> {
    vec![
        label(D),
        Op::BrTable {
            index: Expr::binary(BinaryOp::Sub, Expr::LocalGet(SEL), c32(1), Type::I32),
            targets: vec![L1, L2],
            default: T,
        },
        label(T),
        Op::Unreachable,
    ]
}

/// `fallthrough_via_dispatch` decides whether the entry falls straight into
/// L1 (the bad shape) or sets `sel = 1` and jumps to D (the lowered shape).
fn computed_goto(fallthrough_via_dispatch: bool) -> Function {
    let mut body = vec![
        set_sel(2),
        Op::JmpIf { cond: Expr::LocalGet(0), target: D }, // goto *&&L2 when p0
    ];
    if fallthrough_via_dispatch {
        body.push(set_sel(1));
        body.push(Op::Jmp { target: D });
    }
    body.extend([
        label(L1),
        set_sel(2),
        Op::Jmp { target: D }, // goto *p, p = &&L2
    ]);
    body.extend(dispatch());
    body.extend([
        label(L2),
        set_sel(1),
        Op::JmpIf { cond: Expr::LocalGet(0), target: D }, // goto *&&L1 when p0
        ret(),
    ]);
    func(vec![Type::I32], vec![], vec![Type::I32], body)
}

#[test]
fn ok_computed_goto() {
    verify_one(computed_goto(true)).unwrap();
}

/// The direct fallthrough into L1 is kept: the cycle L1 -> D -> L1 is entered
/// both at D (body[1]) and at L1 (body[1] falling through), so D no longer
/// dominates L1. The DFS from the entry reaches D first, then L1, and the
/// retreating edge is L1's `Jmp D` at body[4].
#[test]
fn bad_computed_goto_direct_fallthrough() {
    let f = computed_goto(false);
    assert_eq!(f.body[2], label(L1));
    assert_eq!(f.body[4], Op::Jmp { target: D });
    assert_eq!(f.body[5], label(D));
    let k = err_at(f, &[4]);
    assert_eq!(k, VerifyErrorKind::Irreducible { label: Some(D), target: vec![5] });
}

/// The real lowering of a source function with `&&L`/`goto *p` produces a body
/// the verifier accepts.
#[test]
fn ok_lowered_computed_goto() {
    let body = vec![
        Op::LocalSet { idx: 1, val: Expr::LabelAddr(L2) },
        label(L1),
        Op::JmpIf { cond: Expr::LocalGet(0), target: L2 },
        Op::GotoIndirect { target: Expr::LocalGet(1) },
        label(L2),
        Op::LocalSet { idx: 1, val: Expr::LabelAddr(L1) },
        Op::JmpIf { cond: Expr::LocalGet(0), target: L1 },
        Op::GotoIndirect { target: Expr::LocalGet(1) },
    ];
    let src = func(vec![Type::I32], vec![], vec![Type::I32], body);
    // Before lowering the markers are rejected.
    assert_eq!(err_at(src.clone(), &[0]), VerifyErrorKind::UnloweredComputedGoto);
    let lowered = lower_function(&src, 0).unwrap();
    verify_one(lowered.function).unwrap();
}

#[test]
fn ok_try_internal_and_outward_jumps() {
    let body = vec![
        Op::TryTable {
            body: vec![
                label(0),
                inc(0),
                Op::JmpIf { cond: ge10(0), target: 1 }, // leave the try
                Op::Jmp { target: 0 },                  // loop inside the try
            ],
            catches: vec![(0, vec![Op::Jmp { target: 1 }])],
        },
        label(1),
        ret(),
    ];
    let mut m = module(vec![func(vec![], vec![], vec![Type::I32], body)]);
    m.tags.push(Tag { params: vec![] });
    verify_module(&m).unwrap();
}

// ── Bad inputs ──────────────────────────────────────────────────────────────

#[test]
fn bad_binary_operand_types_differ() {
    let e = Expr::binary(BinaryOp::Add, c32(1), f32c(2.0), Type::I32);
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Nop, Op::Drop { val: e }]), &[1]);
    assert_eq!(k, VerifyErrorKind::TypeMismatch { expected: vec![Type::I32], got: Type::F32 });
}

#[test]
fn bad_convert_from_differs_from_operand() {
    let e = Expr::Convert { from: Type::I64, to: Type::F64, val: c32(1).boxed() };
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Drop { val: e }]), &[0]);
    assert_eq!(k, VerifyErrorKind::TypeMismatch { expected: vec![Type::I64], got: Type::I32 });
}

#[test]
fn bad_load_pointer_is_float() {
    let e = Expr::Load { ty: MemType::I32, ptr: f32c(0.0).boxed(), offset: 0, align: 2 };
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Drop { val: e }]), &[0]);
    assert!(matches!(k, VerifyErrorKind::TypeMismatch { got: Type::F32, .. }), "{k:?}");
}

#[test]
fn bad_const_value_mismatch() {
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Const { ty: Type::I32, val: ConstVal::F64(0) }]), &[0]);
    assert_eq!(k, VerifyErrorKind::ConstMismatch { ty: Type::I32, val: ConstVal::F64(0) });
}

#[test]
fn bad_sqrt_on_int() {
    let e = Expr::unary(UnaryOp::Sqrt, c32(9), Type::I32);
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Drop { val: e }]), &[0]);
    assert_eq!(k, VerifyErrorKind::IllegalOp { op: OpName::Unary(UnaryOp::Sqrt), ty: Type::I32 });
}

#[test]
fn bad_copysign_on_int() {
    let e = Expr::binary(BinaryOp::Copysign, c32(1), c32(2), Type::I32);
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Drop { val: e }]), &[0]);
    assert_eq!(k, VerifyErrorKind::IllegalOp { op: OpName::Binary(BinaryOp::Copysign), ty: Type::I32 });
}

#[test]
fn bad_clz_and_shl_on_float() {
    let e = Expr::unary(UnaryOp::Clz, f32c(1.0), Type::F32);
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Drop { val: e }]), &[0]);
    assert_eq!(k, VerifyErrorKind::IllegalOp { op: OpName::Unary(UnaryOp::Clz), ty: Type::F32 });
    let e = Expr::binary(BinaryOp::Shl, f32c(1.0), f32c(2.0), Type::F32);
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Drop { val: e }]), &[0]);
    assert_eq!(k, VerifyErrorKind::IllegalOp { op: OpName::Binary(BinaryOp::Shl), ty: Type::F32 });
}

/// The error is reported at the nested path: body[1] (If), child 1 (else), op 0.
#[test]
fn bad_type_in_nested_body_reports_path() {
    let body = vec![
        Op::Nop,
        Op::If {
            cond: c32(1),
            then_body: vec![Op::Nop],
            else_body: vec![Op::Drop { val: Expr::unary(UnaryOp::Popcnt, f32c(1.0), Type::F32) }],
        },
    ];
    let k = err_at(func(vec![], vec![], vec![], body), &[1, 1, 0]);
    assert!(matches!(k, VerifyErrorKind::IllegalOp { .. }));
}

#[test]
fn bad_local_out_of_range() {
    let body = vec![Op::Nop, Op::LocalSet { idx: 2, val: c32(0) }];
    let k = err_at(func(vec![Type::I32], vec![], vec![Type::I32], body), &[1]);
    assert_eq!(k, VerifyErrorKind::LocalOutOfRange { idx: 2, count: 2 });
    let body = vec![Op::Drop { val: Expr::LocalGet(5) }];
    let k = err_at(func(vec![], vec![], vec![], body), &[0]);
    assert_eq!(k, VerifyErrorKind::LocalOutOfRange { idx: 5, count: 0 });
}

#[test]
fn bad_local_set_type() {
    let body = vec![Op::LocalSet { idx: 0, val: Expr::Const { ty: Type::I64, val: ConstVal::I64(0) } }];
    let k = err_at(func(vec![], vec![], vec![Type::I32], body), &[0]);
    assert_eq!(k, VerifyErrorKind::TypeMismatch { expected: vec![Type::I32], got: Type::I64 });
}

#[test]
fn bad_write_immutable_global() {
    let mut m = module(vec![func(vec![], vec![], vec![], vec![Op::Nop, Op::GlobalSet { idx: 0, val: c32(1) }])]);
    m.globals.push(Global { ty: GlobalType { ty: Type::I32, mutable: false }, init: InitExpr::i32(0) });
    let e = verify_module(&m).unwrap_err();
    assert_eq!((e.func, e.at.as_slice()), (0, &[1usize][..]));
    assert_eq!(e.kind, VerifyErrorKind::ImmutableGlobal { idx: 0 });
}

#[test]
fn bad_call_signature() {
    // Imported function 0: (i32) -> (); the defined function is index 1.
    let mut mb = ModuleBuilder::new();
    let ty = mb.add_type(FunctionType { params: vec![Type::I32], results: vec![] });
    mb.import_function("env", "f", ty);
    mb.add_function(func(vec![], vec![], vec![], vec![Op::Call { func_idx: 0, args: vec![f32c(1.0)] }]));
    let e = verify_module(&mb.build()).unwrap_err();
    assert_eq!((e.func, e.at.as_slice()), (1, &[0usize][..]));
    assert_eq!(e.kind, VerifyErrorKind::ArgMismatch { expected: vec![Type::I32], got: vec![Type::F32] });
}

#[test]
fn bad_call_index_out_of_range() {
    let k = err_at(func(vec![], vec![], vec![], vec![Op::Call { func_idx: 7, args: vec![] }]), &[0]);
    assert_eq!(k, VerifyErrorKind::FuncOutOfRange { idx: 7, count: 1 });
}

#[test]
fn bad_call_indirect() {
    let call = |table_idx| Op::CallIndirect { type_idx: 0, table_idx, callee: c32(0), args: vec![] };
    let mut m = module(vec![func(vec![], vec![], vec![], vec![call(0)])]);
    m.types.push(FunctionType { params: vec![Type::I64], results: vec![] });
    // No table yet.
    assert_eq!(verify_module(&m).unwrap_err().kind, VerifyErrorKind::TableOutOfRange { idx: 0, count: 0 });
    m.tables.push(TableType { min: 1, max: None });
    let e = verify_module(&m).unwrap_err();
    assert_eq!(e.at, vec![0]);
    assert_eq!(e.kind, VerifyErrorKind::ArgMismatch { expected: vec![Type::I64], got: vec![] });
}

#[test]
fn bad_throw_args() {
    let mut m = module(vec![func(vec![], vec![], vec![], vec![Op::Throw { tag_idx: 0, args: vec![f32c(0.0)] }])]);
    assert_eq!(verify_module(&m).unwrap_err().kind, VerifyErrorKind::TagOutOfRange { idx: 0, count: 0 });
    m.tags.push(Tag { params: vec![Type::I32] });
    let e = verify_module(&m).unwrap_err();
    assert_eq!(e.at, vec![0]);
    assert_eq!(e.kind, VerifyErrorKind::ArgMismatch { expected: vec![Type::I32], got: vec![Type::F32] });
}

#[test]
fn bad_return_count() {
    let body = vec![Op::Return { vals: vec![c32(1), c32(2)] }];
    let k = err_at(func(vec![], vec![Type::I32], vec![], body), &[0]);
    assert_eq!(k, VerifyErrorKind::ReturnMismatch { expected: vec![Type::I32], got: vec![Type::I32, Type::I32] });
}

#[test]
fn bad_return_missing_value() {
    let k = err_at(func(vec![], vec![Type::I32], vec![], vec![ret()]), &[0]);
    assert_eq!(k, VerifyErrorKind::ReturnMismatch { expected: vec![Type::I32], got: vec![] });
}

#[test]
fn bad_falls_off_end_with_results() {
    let k = err_at(func(vec![], vec![Type::I32], vec![], vec![Op::Nop, Op::Nop]), &[1]);
    assert_eq!(k, VerifyErrorKind::MissingReturn);
}

#[test]
fn bad_undeclared_label() {
    let body = vec![label(0), Op::JmpIf { cond: c32(1), target: 9 }, ret()];
    let k = err_at(func(vec![], vec![], vec![], body), &[1]);
    assert_eq!(k, VerifyErrorKind::UndeclaredLabel { label: 9 });
}

#[test]
fn bad_brtable_undeclared_default() {
    let body = vec![label(0), Op::BrTable { index: c32(0), targets: vec![0], default: 4 }];
    let k = err_at(func(vec![], vec![], vec![], body), &[1]);
    assert_eq!(k, VerifyErrorKind::UndeclaredLabel { label: 4 });
}

#[test]
fn bad_duplicate_label() {
    let body = vec![label(3), Op::Block { body: vec![Op::Nop, label(3)] }, ret()];
    let k = err_at(func(vec![], vec![], vec![], body), &[1, 0, 1]);
    assert_eq!(k, VerifyErrorKind::DuplicateLabel { label: 3, first: vec![0] });
}

#[test]
fn bad_label_params() {
    let body = vec![Op::Nop, Op::Label { id: 0, params: vec![Type::I32] }, ret()];
    let k = err_at(func(vec![], vec![], vec![], body), &[1]);
    assert_eq!(k, VerifyErrorKind::LabelParams { label: 0 });
}

/// The classic two-entry loop: entry -> B and entry -> A, A <-> B.
///   0: JmpIf p0, B    1: A:    2: Jmp B    3: B:    4: JmpIf p0, A    5: Return
/// The DFS goes 0 -> 3 -> 4 -> 1 -> 2 and meets B on the stack; B does not
/// dominate body[2] (A is reachable from the entry without B).
pub(super) fn irreducible_fixture() -> Function {
    let (a, b) = (0, 1);
    let body = vec![
        Op::JmpIf { cond: Expr::LocalGet(0), target: b },
        label(a),
        Op::Jmp { target: b },
        label(b),
        Op::JmpIf { cond: Expr::LocalGet(0), target: a },
        ret(),
    ];
    func(vec![Type::I32], vec![], vec![], body)
}

#[test]
fn bad_irreducible_cfg() {
    let k = err_at(irreducible_fixture(), &[2]);
    assert_eq!(k, VerifyErrorKind::Irreducible { label: Some(1), target: vec![3] });
}

#[test]
fn bad_jump_into_try() {
    let body = vec![
        Op::Jmp { target: 0 },
        Op::TryTable { body: vec![label(0), ret()], catches: vec![] },
    ];
    let k = err_at(func(vec![], vec![], vec![], body), &[0]);
    assert_eq!(k, VerifyErrorKind::JumpIntoTry { label: 0 });
}

/// From one try body into a sibling try body, and from a catch handler back
/// into its own try body: both enter a try from outside.
#[test]
fn bad_jump_between_try_bodies() {
    let body = vec![
        Op::TryTable { body: vec![Op::Jmp { target: 1 }], catches: vec![] },
        Op::TryTable { body: vec![label(1), ret()], catches: vec![] },
    ];
    let k = err_at(func(vec![], vec![], vec![], body), &[0, 0, 0]);
    assert_eq!(k, VerifyErrorKind::JumpIntoTry { label: 1 });

    let body = vec![Op::TryTable { body: vec![label(0), ret()], catches: vec![(0, vec![Op::Jmp { target: 0 }])] }];
    let mut m = module(vec![func(vec![], vec![], vec![], body)]);
    m.tags.push(Tag { params: vec![] });
    let e = verify_module(&m).unwrap_err();
    assert_eq!(e.at, vec![0, 1, 0]);
    assert_eq!(e.kind, VerifyErrorKind::JumpIntoTry { label: 0 });
}

#[test]
fn bad_unlowered_markers() {
    let k = err_at(func(vec![], vec![], vec![], vec![Op::GotoIndirect { target: c32(0) }]), &[0]);
    assert_eq!(k, VerifyErrorKind::UnloweredComputedGoto);
    let k = err_at(func(vec![], vec![], vec![Type::I32], vec![label(0), Op::LocalSet { idx: 0, val: Expr::LabelAddr(0) }]), &[1]);
    assert_eq!(k, VerifyErrorKind::UnloweredComputedGoto);
}

/// Errors carry the function index in the unified space, and the first
/// function in order wins.
#[test]
fn error_names_the_failing_function() {
    let good = func(vec![], vec![], vec![], vec![ret()]);
    let bad = func(vec![], vec![], vec![], vec![Op::Nop, Op::Nop, Op::Jmp { target: 0 }]);
    let e = verify_module(&module(vec![good.clone(), good, bad])).unwrap_err();
    assert_eq!((e.func, e.at), (2, vec![2]));
    assert_eq!(e.kind, VerifyErrorKind::UndeclaredLabel { label: 0 });
}

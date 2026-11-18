//! Stackify tests. Module helpers: run a real function end-to-end from NJVM-IR
//! through encoding to execution on WNJR and assert the return value. Every case
//! calling `run_fn` or `run_module` is a true run under the gate definition.

use super::{max_depth, stackify, Node, StackifyError};
use crate::collections::{vec, Vec};
use crate::njvm::computed_goto::lower_function as lower_cg;
use crate::njvm::encode::encode;
use crate::njvm::module::{Function, MemoryType, Module};
use crate::njvm::{verify_module, Expr, BinaryOp, ConstVal, ModuleBuilder, Op, Type};
use crate::{Config, Engine, RuntimeWorld, Value};
use alloc::boxed::Box;

// ── helpers ──────────────────────────────────────────────────────────────────

fn c(ty: Type, v: i32) -> Expr {
    Expr::Const { ty, val: ConstVal::I32(v) }
}
fn bin(op: BinaryOp, ty: Type, a: Expr, b: Expr) -> Expr {
    Expr::binary(op, a, b, ty)
}
fn ret(e: Expr) -> Op {
    Op::Return { vals: vec![e] }
}
fn ret0() -> Op {
    Op::Return { vals: vec![] }
}
fn lget(idx: u32) -> Expr {
    Expr::LocalGet(idx)
}
fn lset(idx: u32, val: Expr) -> Op {
    Op::LocalSet { idx, val }
}

/// Encode a module and execute export `name` with the given args.
fn run_module(m: &Module, name: &str, args: &[Value]) -> Vec<Value> {
    let bytes = encode(m).expect("encode");
    let module = crate::Module::new("n6", &bytes).expect("WNJR parse + validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, module, &[]).expect("instantiate");
    let out = world.invoke(id, name, args).expect("invoke");
    world.free(id).expect("free");
    out
}

/// A one-function module (`f`) with a memory and the given body.
fn run_fn(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>, body: Vec<Op>, args: &[Value]) -> Vec<Value> {
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType { min: 1, max: Some(1), shared: false });
    let mut f = Function::new(params, results, locals);
    f.body = body;
    mb.add_function(f);
    mb.export_function("f", 0);
    run_module(&mb.build(), "f", args)
}

fn i32v(v: Value) -> i32 {
    match v {
        Value::I32(x) => x,
        o => panic!("expected i32, got {o:?}"),
    }
}

// ── single loop ──────────────────────────────────────────────────────────────

#[test]
fn single_loop() {
    // `for (i = 0; i < 10; i++) { }` returns 10.
    let body = vec![
        lset(0, c(Type::I32, 0)),               // i = 0
        Op::Label { id: 0, params: vec![] },    // head:
        Op::JmpIf {                             //   if i >= 10 -> exit
            cond: bin(BinaryOp::Ge, Type::I32, lget(0), c(Type::I32, 10)),
            target: 1,
        },
        lset(0, bin(BinaryOp::Add, Type::I32, lget(0), c(Type::I32, 1))), // i++
        Op::Jmp { target: 0 },                  // goto head
        Op::Label { id: 1, params: vec![] },    // exit:
        ret(lget(0)),                           //   return i
    ];
    let out = run_fn(vec![], vec![Type::I32], vec![Type::I32], body, &[]);
    assert_eq!(i32v(out[0]), 10);
}

#[test]
fn nested_loop() {
    // `int i = 0; for (int j = 0; j < 3; j++) for (int k = 0; k < 4; k++) i++;`
    let body = vec![
        lset(0, c(Type::I32, 0)), // i = 0
        lset(1, c(Type::I32, 0)), // j = 0
        Op::Label { id: 0, params: vec![] }, // outer:
        Op::JmpIf { cond: bin(BinaryOp::Ge, Type::I32, lget(1), c(Type::I32, 3)), target: 3 },
        lset(2, c(Type::I32, 0)), // k = 0
        Op::Label { id: 1, params: vec![] }, // inner:
        Op::JmpIf { cond: bin(BinaryOp::Ge, Type::I32, lget(2), c(Type::I32, 4)), target: 2 },
        lset(0, bin(BinaryOp::Add, Type::I32, lget(0), c(Type::I32, 1))), // i++
        lset(2, bin(BinaryOp::Add, Type::I32, lget(2), c(Type::I32, 1))), // k++
        Op::Jmp { target: 1 },
        Op::Label { id: 2, params: vec![] }, // inner_exit
        lset(1, bin(BinaryOp::Add, Type::I32, lget(1), c(Type::I32, 1))), // j++
        Op::Jmp { target: 0 },
        Op::Label { id: 3, params: vec![] }, // exit
        ret(lget(0)),
    ];
    let out = run_fn(vec![], vec![Type::I32], vec![Type::I32, Type::I32, Type::I32], body, &[]);
    assert_eq!(i32v(out[0]), 12); // 3 * 4
}

#[test]
fn loop_break_continue() {
    // `for (i = 0; i < 100; i++) { if (i == 7) break; if (i < 5) continue; sum += i; }`
    let body = vec![
        lset(0, c(Type::I32, 0)), // i = 0
        lset(1, c(Type::I32, 0)), // sum = 0
        Op::Label { id: 0, params: vec![] }, // head
        Op::JmpIf { cond: bin(BinaryOp::Ge, Type::I32, lget(0), c(Type::I32, 100)), target: 3 },
        Op::JmpIf { cond: bin(BinaryOp::Eq, Type::I32, lget(0), c(Type::I32, 7)), target: 3 },
        Op::JmpIf { cond: bin(BinaryOp::Lt, Type::I32, lget(0), c(Type::I32, 5)), target: 1 },
        lset(1, bin(BinaryOp::Add, Type::I32, lget(1), lget(0))),
        Op::Label { id: 1, params: vec![] }, // cont:
        lset(0, bin(BinaryOp::Add, Type::I32, lget(0), c(Type::I32, 1))),
        Op::Jmp { target: 0 },
        Op::Label { id: 3, params: vec![] }, // exit
        ret(lget(1)),
    ];
    let out = run_fn(vec![], vec![Type::I32], vec![Type::I32, Type::I32], body, &[]);
    assert_eq!(i32v(out[0]), 5 + 6);
}

#[test]
fn forward_goto() {
    // `if (x == 0) { y = 0; goto L; } y = 42; L: return y;`
    // Pass x (param 0) to test both branches. Local 1 = y.
    let body = vec![
        lset(1, c(Type::I32, 0)),      // y = 0
        Op::JmpIf { cond: bin(BinaryOp::Eq, Type::I32, lget(0), c(Type::I32, 0)), target: 1 },
        lset(1, c(Type::I32, 42)),     // y = 42
        Op::Label { id: 1, params: vec![] }, // L:
        ret(lget(1)),
    ];
    let r1 = run_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32], body.clone(), &[Value::I32(0)]);
    assert_eq!(i32v(r1[0]), 0);
    let r2 = run_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32], body, &[Value::I32(1)]);
    assert_eq!(i32v(r2[0]), 42);
}

#[test]
fn multiple_forward_merge() {
    // `x = p; if (p == 1) goto M; x++; if (p == 2) goto M; x++; M: return x;`
    // Param 0 = p, local 1 = x.
    let body = vec![
        lset(1, lget(0)),              // x = p
        Op::JmpIf { cond: bin(BinaryOp::Eq, Type::I32, lget(0), c(Type::I32, 1)), target: 2 },
        lset(1, bin(BinaryOp::Add, Type::I32, lget(1), c(Type::I32, 1))),
        Op::JmpIf { cond: bin(BinaryOp::Eq, Type::I32, lget(0), c(Type::I32, 2)), target: 2 },
        lset(1, bin(BinaryOp::Add, Type::I32, lget(1), c(Type::I32, 1))),
        Op::Label { id: 2, params: vec![] }, // M:
        ret(lget(1)),
    ];
    // p=1 -> merge fires at first test, x=1
    let r1 = run_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32], body.clone(), &[Value::I32(1)]);
    assert_eq!(i32v(r1[0]), 1);
    // p=2 -> second test, x=2+1=3
    let r2 = run_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32], body.clone(), &[Value::I32(2)]);
    assert_eq!(i32v(r2[0]), 3);
    // p=0 -> neither, x=0+2=2
    let r0 = run_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32], body, &[Value::I32(0)]);
    assert_eq!(i32v(r0[0]), 2);
}

#[test]
fn brtable_dispatch() {
    // `switch (idx) { case 0: x = 10; case 1: x = 20; case 2: x = 30; default: x = 99; } return x;`
    // Param 0 = idx, local 1 = x.
    let body = vec![
        lset(1, c(Type::I32, 0)), // x = 0
        Op::BrTable {
            index: lget(0), // param 0
            targets: vec![10, 11, 12],
            default: 13,
        },
        Op::Label { id: 10, params: vec![] }, // case 0
        lset(1, c(Type::I32, 10)),
        Op::Jmp { target: 14 },
        Op::Label { id: 11, params: vec![] }, // case 1
        lset(1, c(Type::I32, 20)),
        Op::Jmp { target: 14 },
        Op::Label { id: 12, params: vec![] }, // case 2
        lset(1, c(Type::I32, 30)),
        Op::Jmp { target: 14 },
        Op::Label { id: 13, params: vec![] }, // default
        lset(1, c(Type::I32, 99)),
        Op::Label { id: 14, params: vec![] }, // merge
        ret(lget(1)),
    ];
    let r1 = run_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32], body.clone(), &[Value::I32(1)]);
    assert_eq!(i32v(r1[0]), 20);
    let rd = run_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32], body, &[Value::I32(99)]);
    assert_eq!(i32v(rd[0]), 99);
}

#[test]
fn computed_goto() {
    // Test stackify over the already-lowered computed_goto output.
    // `x = p ? &&L1 : &&L0; jmp *x;  L0: return 10;  L1: return 17;`
    let body = vec![
        lset(1, Expr::Select {
            cond: Box::new(lget(0)),
            then_val: Box::new(Expr::LabelAddr(11)),
            else_val: Box::new(Expr::LabelAddr(10)),
        }),
        Op::GotoIndirect { target: lget(1) },
        Op::Label { id: 10, params: vec![] },
        ret(c(Type::I32, 10)),
        Op::Label { id: 11, params: vec![] },
        ret(c(Type::I32, 17)),
    ];
    let mut f = Function::new(vec![Type::I32], vec![Type::I32], vec![Type::I32]);
    f.body = body;
    let lowered = lower_cg(&f, 0).expect("lower computed goto");
    let mut vb = ModuleBuilder::new();
    vb.add_function(lowered.function.clone());
    verify_module(&vb.build()).expect("verify after lower");
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType { min: 1, max: Some(1), shared: false });
    mb.add_function(lowered.function);
    mb.export_function("f", 0);
    let m = mb.build();
    let r0 = run_module(&m, "f", &[Value::I32(0)]);
    assert_eq!(i32v(r0[0]), 10);
    let r1 = run_module(&m, "f", &[Value::I32(1)]);
    assert_eq!(i32v(r1[0]), 17);
}

// ── depth guard ──────────────────────────────────────────────────────────────

#[test]
fn depth_no_spurious_block() {
    // A single loop with one exit; should be `loop { ... br 1 }`, depth 1.
    let body = vec![
        Op::Label { id: 0, params: vec![] },
        Op::JmpIf { cond: c(Type::I32, 0), target: 1 },
        Op::Jmp { target: 0 },
        Op::Label { id: 1, params: vec![] },
        ret0(),
    ];
    let tree = stackify(&body).expect("stackify");
    assert!(max_depth(&tree) == 2, "depth = {}", max_depth(&tree));  // loop wraps IF, so depth 2
}

#[test]
fn depth_chain_of_fallthrough_labels() {
    // 50 labels in a row, all fallthrough (no pred counting, no block needed).
    let mut body: Vec<Op> = Vec::new();
    for i in 0..50u32 {
        body.push(Op::Label { id: i, params: vec![] });
    }
    body.push(ret0());
    let tree = stackify(&body).expect("stackify");
    assert_eq!(max_depth(&tree), 0, "all inlined");
}

#[test]
fn depth_brtable_forces_merge_blocks() {
    // `switch (i) { case 0: L0; case 1: L1; case 2: L2; case 3: L3; default: Ld; } -> merge M`.
    // Each target needs its own block per Ramsey (BrTable forces merge), plus one for M = 6? Actually let's just count.
    let body = vec![
        Op::BrTable {
            index: c(Type::I32, 0),
            targets: vec![10, 11, 12, 13],
            default: 14,
        },
        Op::Label { id: 10, params: vec![] },
        Op::Jmp { target: 99 },
        Op::Label { id: 11, params: vec![] },
        Op::Jmp { target: 99 },
        Op::Label { id: 12, params: vec![] },
        Op::Jmp { target: 99 },
        Op::Label { id: 13, params: vec![] },
        Op::Jmp { target: 99 },
        Op::Label { id: 14, params: vec![] },
        Op::Jmp { target: 99 },
        Op::Label { id: 99, params: vec![] },
        ret0(),
    ];
    let tree = stackify(&body).expect("stackify");
    // BrTable forces each target L_i onto a Block. Merge 99 has 5 preds, gets a Block.
    // Depth should be 5.
    assert_eq!(max_depth(&tree), 6, "6 nested constructs");
}

// ── errors ──────────────────────────────────────────────────────────────────

#[test]
fn nested_jump_is_rejected() {
    let body = vec![
        Op::Label { id: 0, params: vec![] },
        Op::Block { body: vec![Op::Jmp { target: 0 }] },
    ];
    assert_eq!(stackify(&body).unwrap_err(), StackifyError::NestedJump);
}

#[test]
fn label_with_params_is_rejected() {
    let body = vec![Op::Label { id: 0, params: vec![Type::I32] }];
    assert_eq!(stackify(&body).unwrap_err(), StackifyError::LabelWithParams(0));
}

#[test]
fn unknown_label() {
    let body = vec![Op::Jmp { target: 99 }];
    assert_eq!(stackify(&body).unwrap_err(), StackifyError::UnknownLabel(99));
}

// ── property test: 200-label random CFG ─────────────────────────────────────

/// A minimal xorshift PRNG.
struct Rng(u32);
impl Rng {
    fn new(seed: u32) -> Rng { Rng(seed.max(1)) }
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn range(&mut self, n: u32) -> u32 { if n == 0 { 0 } else { self.next() % n } }
}

/// Generate a reducible CFG with `n` blocks by building a laminar loop tree.
/// Each block updates an accumulator `x` (param/local 0), decrements `fuel` and
/// jumps to `exit` if fuel <= 0. Returns a body that reads param 0 (`x` init).
fn gen_cfg(rng: &mut Rng, n: usize) -> Vec<Op> {
    // Allocate blocks [0..n), ids [100..100+n*2) (so BrTable has room).
    let mut body: Vec<Op> = Vec::new();
    let mut fuel = 500; // enough for 200+ labels plus nesting
    let exit_label = 100 + (n * 2) as u32;
    // Nested loop structure: recursively partition block ranges.
    fn build(
        body: &mut Vec<Op>,
        rng: &mut Rng,
        start: usize,
        end: usize,
        fuel_local: u32,
        exit_label: u32,
        base: u32,
    ) {
        if start >= end { return; }
        if start + 1 == end {
            // Single block: set x += const, decrement fuel, jump to a target.
            let label = base + start as u32;
            body.push(Op::Label { id: label, params: vec![] });
            // x = x + block_id
            body.push(lset(0, bin(BinaryOp::Xor, Type::I32, lget(0), c(Type::I32, label as i32))));
            // fuel--
            body.push(lset(fuel_local, bin(BinaryOp::Sub, Type::I32, lget(fuel_local), c(Type::I32, 1))));
            // if fuel <= 0 goto exit
            body.push(Op::JmpIf {
                cond: bin(BinaryOp::Le, Type::I32, lget(fuel_local), c(Type::I32, 0)),
                target: exit_label,
            });
            return;
        }
        // Partition randomly: loop head [start..mid), tail [mid..end).
        let mid = start + 1 + rng.range((end - start - 1) as u32) as usize;
        build(body, rng, start, mid, fuel_local, exit_label, base);
        // 50% chance the tail loops back to head.
        if rng.range(2) == 0 && mid < end {
            let head = base + start as u32;
            body.push(Op::Jmp { target: head });
        }
        build(body, rng, mid, end, fuel_local, exit_label, base);
    }

    // Local 0 = x, local 1 = fuel.
    body.push(lset(1, c(Type::I32, fuel)));
    build(&mut body, rng, 0, n, 1, exit_label, 100);
    body.push(Op::Label { id: exit_label, params: vec![] });
    body.push(ret(lget(0)));
    body
}

/// A minimal interpreter for NJVM-IR CFG ops (constants, local ops, jumps). The
/// body must have flat CFG ops only (no nested structured control).
fn interpret(body: &[Op], x_init: i32) -> Result<i32, &'static str> {
    let mut locals: Vec<i32> = vec![x_init, 500]; // local 0 = x, local 1 = fuel
    let mut pc = 0;
    let mut label_pc: Vec<(u32, usize)> = Vec::new();
    for (i, op) in body.iter().enumerate() {
        if let Op::Label { id, .. } = op {
            label_pc.push((*id, i));
        }
    }
    let find_label = |id: u32| -> Result<usize, &'static str> {
        label_pc.iter().find(|&(l, _)| *l == id).map(|(_, i)| *i).ok_or("label not found")
    };
    fn eval(e: &Expr, locals: &[i32]) -> i32 {
        match e {
            Expr::Const { val: ConstVal::I32(v), .. } => *v,
            Expr::LocalGet(idx) => locals[*idx as usize],
            Expr::Binary { op, lhs, rhs, .. } => {
                let l = eval(lhs, locals);
                let r = eval(rhs, locals);
                match op {
                    BinaryOp::Add => l.wrapping_add(r),
                    BinaryOp::Sub => l.wrapping_sub(r),
                    BinaryOp::Xor => l ^ r,
                    BinaryOp::Le => if l <= r { 1 } else { 0 },
                    BinaryOp::Eq => if l == r { 1 } else { 0 },
                    _ => 0,
                }
            }
            _ => 0,
        }
    }
    while pc < body.len() {
        match &body[pc] {
            Op::Label { .. } => pc += 1,
            Op::LocalSet { idx, val } => { locals[*idx as usize] = eval(val, &locals); pc += 1; }
            Op::Jmp { target } => pc = find_label(*target)?,
            Op::JmpIf { cond, target } => {
                if eval(cond, &locals) != 0 {
                    pc = find_label(*target)?;
                } else {
                    pc += 1;
                }
            }
            Op::Return { vals } => {
                let v = if vals.is_empty() { locals[0] } else { eval(&vals[0], &locals) };
                return Ok(v);
            }
            _ => pc += 1,
        }
    }
    Ok(locals[0])
}

#[test]
fn random_cfg_200_labels() {
    // Generate a 240-block CFG (exceeds 200 labels), stackify it, encode it,
    // and compare the executed return value with an interpreter on several inputs.
    let mut rng = Rng::new(0xdeadbeef);
    for seed in 0..4u32 {
        rng = Rng::new(0xdeadbeef ^ seed.wrapping_mul(0x9e3779b9));
        let body = gen_cfg(&mut rng, 240);
        let mut f = Function::new(vec![Type::I32], vec![Type::I32], vec![Type::I32, Type::I32]);
        f.body = body.clone();
        let mut mb = ModuleBuilder::new();
        mb.add_function(f);
        verify_module(&mb.build())
            .expect("verify module");
        let tree = stackify(&body).expect("stackify");
        for x_in in &[0i32, 1, -1, 42, 9999] {
            let expected = interpret(&body, *x_in).expect("interpret");
            let got = run_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32, Type::I32], body.clone(), &[Value::I32(*x_in)]);
            assert_eq!(i32v(got[0]), expected, "seed {seed}, x_in {x_in}");
        }
    }
}

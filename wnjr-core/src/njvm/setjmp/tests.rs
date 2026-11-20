//! N9 tests. Every behaviour test lowers setjmp/longjmp, runs the N3
//! verifier on the result, encodes it, hands the bytes to WNJR `Module::new`,
//! instantiates on the JIT and asserts the returned value.

use super::*;
use crate::collections::{vec, Vec};
use crate::njvm::encode::encode;
use crate::njvm::module::MemoryType;
use crate::njvm::{verify_module, ModuleBuilder};
use crate::{Config, Engine, RuntimeWorld, Value};

fn c(v: i32) -> Expr {
    Expr::Const { ty: Type::I32, val: ConstVal::I32(v) }
}
fn get(i: u32) -> Expr {
    Expr::LocalGet(i)
}
fn label(id: LabelId) -> Op {
    Op::Label { id, params: vec![] }
}
fn ret(e: Expr) -> Op {
    Op::Return { vals: vec![e] }
}
fn setjmp(buf: i32, dest: u32) -> Op {
    Op::Setjmp { buf: c(buf), dest }
}
fn call(f: u32, args: Vec<Expr>) -> Op {
    Op::Call { func_idx: f, args }
}
fn add(a: Expr, b: Expr) -> Expr {
    Expr::binary(BinaryOp::Add, a, b, Type::I32)
}

fn func(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>, body: Vec<Op>) -> Function {
    let mut f = Function::new(params, results, locals);
    f.body = body;
    f
}

/// `g(buf, v) { longjmp(buf, v); }`
fn longjmper() -> Function {
    func(vec![Type::I32, Type::I32], vec![], vec![], vec![Op::Longjmp { buf: get(0), val: get(1) }])
}

/// Module of `funcs` (function 0 exported as "f"), one page of memory.
fn module(funcs: Vec<Function>) -> Module {
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType { min: 1, max: Some(1), shared: false });
    for f in funcs {
        mb.add_function(f);
    }
    mb.export_function("f", 0);
    mb.build()
}

/// Lower, verify, encode, instantiate, call "f" once per argument list.
fn run(m: &Module, calls: &[Vec<Value>]) -> Vec<Result<Vec<Value>, ()>> {
    let lowered = lower_module(m).expect("setjmp lowering");
    verify_module(&lowered).expect("N3 accepts the lowered module");
    let bytes = encode(&lowered).expect("encode");
    let module = crate::Module::new("n9", &bytes).expect("WNJR parse + validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, module, &[]).expect("instantiate");
    let out = calls
        .iter()
        .map(|a| {
            world.invoke(id, "f", a).map_err(|_| ())
        })
        .collect();
    world.free(id).expect("free");
    out
}

fn run_i32(m: &Module, args: Vec<Value>) -> i32 {
    match run(m, &[args]).remove(0) {
        Ok(v) => match v.as_slice() {
            [Value::I32(x)] => *x,
            o => panic!("expected one i32, got {o:?}"),
        },
        Err(()) => panic!("unexpected trap"),
    }
}

/// `r = setjmp(b); if (r) return r; longjmp(b, 7);` -> 7.
#[test]
fn single_level() {
    let body = vec![
        setjmp(16, 0),
        Op::JmpIf { cond: get(0), target: 1 },
        Op::Longjmp { buf: c(16), val: c(7) },
        label(1),
        ret(get(0)),
    ];
    let m = module(vec![func(vec![], vec![Type::I32], vec![Type::I32], body)]);
    assert_eq!(run_i32(&m, vec![]), 7);
}

/// main: `r = setjmp(b); if (r) return r + 100; a(); return 0;`
/// a: `b2(); return;`  b2: `longjmp(b, 5);` -> 105.
#[test]
fn across_two_calls() {
    let main = func(
        vec![],
        vec![Type::I32],
        vec![Type::I32],
        vec![
            setjmp(16, 0),
            Op::JmpIf { cond: get(0), target: 1 },
            call(1, vec![]),
            ret(c(0)),
            label(1),
            ret(add(get(0), c(100))),
        ],
    );
    let a = func(vec![], vec![], vec![], vec![call(2, vec![]), Op::Return { vals: vec![] }]);
    let b = func(vec![], vec![], vec![], vec![Op::Longjmp { buf: c(16), val: c(5) }]);
    let m = module(vec![main, a, b]);
    assert_eq!(run_i32(&m, vec![]), 105);
}

/// Two sites, each longjmp'd to once, the later longjmp to the earlier site:
/// `r1 = setjmp(b1); if (r1) return r1*100 + r2;
///  r2 = setjmp(b2); if (r2) g(b1, 11); g(b2, 22);` -> 1122.
#[test]
fn two_sites_in_one_function() {
    let main = func(
        vec![],
        vec![Type::I32],
        vec![Type::I32, Type::I32],
        vec![
            setjmp(16, 0),
            Op::JmpIf { cond: get(0), target: 1 },
            setjmp(32, 1),
            Op::JmpIf { cond: get(1), target: 2 },
            call(1, vec![c(32), c(22)]),
            Op::Unreachable,
            label(2),
            call(1, vec![c(16), c(11)]),
            Op::Unreachable,
            label(1),
            ret(add(Expr::binary(BinaryOp::Mul, get(0), c(100), Type::I32), get(1))),
        ],
    );
    let m = module(vec![main, longjmper()]);
    assert_eq!(run_i32(&m, vec![]), 1122);
}

/// `f(v) { r = setjmp(b); if (r) return r; g(b, v); }`: longjmp(b, 0)
/// makes setjmp return 1; any other value comes back unchanged.
#[test]
fn longjmp_value_zero_is_one() {
    let main = func(
        vec![Type::I32],
        vec![Type::I32],
        vec![Type::I32],
        vec![
            setjmp(16, 1),
            Op::JmpIf { cond: get(1), target: 1 },
            call(1, vec![c(16), get(0)]),
            Op::Unreachable,
            label(1),
            ret(get(1)),
        ],
    );
    let m = module(vec![main, longjmper()]);
    let xs = [0, 42, 1, -3];
    let out = run(&m, &xs.iter().map(|x| vec![Value::I32(*x)]).collect::<Vec<_>>());
    let got: Vec<i32> = out
        .into_iter()
        .map(|r| match r.expect("no trap").as_slice() {
            [Value::I32(x)] => *x,
            o => panic!("{o:?}"),
        })
        .collect();
    assert_eq!(got, vec![1, 42, 1, -3]);
}

/// setjmp inside a loop (the resume point is in the loop body, so the
/// dispatch goes through the loop header):
/// `for (i = 0; i < 3; ) { r = setjmp(b); sum += r; i++; if (!r) g(b, 10); }`
/// i=0: r=0 -> longjmp -> r=10, sum=10, i=2; i=2: r=0 -> longjmp -> sum=20, i=4.
#[test]
fn setjmp_in_loop() {
    let (i, r, sum) = (0, 1, 2);
    let inc = |l: u32, e: Expr| Op::LocalSet { idx: l, val: add(get(l), e) };
    let main = func(
        vec![],
        vec![Type::I32],
        vec![Type::I32, Type::I32, Type::I32],
        vec![
            Op::LocalSet { idx: i, val: c(0) },
            Op::LocalSet { idx: sum, val: c(0) },
            label(0),
            Op::JmpIf { cond: Expr::binary(BinaryOp::Ge, get(i), c(3), Type::I32), target: 9 },
            setjmp(16, r),
            inc(sum, get(r)),
            inc(i, c(1)),
            Op::JmpIf { cond: get(r), target: 0 },
            call(1, vec![c(16), c(10)]),
            Op::Jmp { target: 0 },
            label(9),
            ret(get(sum)),
        ],
    );
    let m = module(vec![main, longjmper()]);
    assert_eq!(run_i32(&m, vec![]), 20);
}

/// A longjmp to main's buffer passes through `inner`, which has a setjmp of
/// its own; inner must rethrow, not resume itself (its resume path traps).
#[test]
fn foreign_label_is_rethrown() {
    let main = func(
        vec![],
        vec![Type::I32],
        vec![Type::I32],
        vec![setjmp(16, 0), Op::JmpIf { cond: get(0), target: 1 }, call(1, vec![]), ret(c(0)), label(1), ret(get(0))],
    );
    let inner = func(
        vec![],
        vec![],
        vec![Type::I32],
        vec![
            setjmp(32, 0),
            Op::JmpIf { cond: get(0), target: 1 },
            call(2, vec![c(16), c(9)]),
            Op::Return { vals: vec![] },
            label(1),
            Op::Unreachable,
        ],
    );
    let m = module(vec![main, inner, longjmper()]);
    assert_eq!(run_i32(&m, vec![]), 9);
}

/// longjmp through a jmp_buf no setjmp filled (number 0): uncaught, traps.
#[test]
fn unset_jmp_buf_traps() {
    let m = module(vec![func(vec![], vec![Type::I32], vec![], vec![Op::Longjmp { buf: c(16), val: c(1) }])]);
    assert!(run(&m, &[vec![]])[0].is_err());
}

#[test]
fn module_without_setjmp_is_unchanged() {
    let m = module(vec![func(vec![], vec![Type::I32], vec![], vec![ret(c(3))])]);
    assert_eq!(lower_module(&m).unwrap(), m);
}

#[test]
fn nested_setjmp_is_rejected() {
    let body = vec![Op::Block { body: vec![setjmp(16, 0)] }, ret(c(0))];
    let m = module(vec![func(vec![], vec![Type::I32], vec![Type::I32], body)]);
    assert_eq!(lower_module(&m), Err(SetjmpError::NotTopLevel { func: 0 }));
}

/// One tag for the whole module, `(i32, i32)`; site numbers disjoint.
#[test]
fn one_tag_disjoint_numbers() {
    let f = |buf| func(vec![], vec![], vec![Type::I32], vec![setjmp(buf, 0), Op::Return { vals: vec![] }]);
    let m = module(vec![f(16), f(32)]);
    let l = lower_module(&m).unwrap();
    assert_eq!(l.tags, vec![Tag { params: TAG_PARAMS.to_vec() }]);
    let numbers: Vec<i32> = l
        .functions
        .iter()
        .map(|f| {
            let mut n = None;
            fn find(ops: &[Op], n: &mut Option<i32>) {
                for op in ops {
                    if let Op::Store { val: Expr::Const { val: ConstVal::I32(v), .. }, .. } = op {
                        *n = Some(*v);
                    }
                    for b in child_bodies(op) {
                        find(b, n);
                    }
                }
            }
            find(&f.body, &mut n);
            n.expect("store of the site number")
        })
        .collect();
    assert_eq!(numbers, vec![1, 2]);
}


// ── engine EH probe (not NJVM): catch of a throw from a callee ─────────────
//
// A callee small enough for the inliner (inline.rs, MAX_CALLEE_OPS = 32,
// straight-line or simple if) is caught inside the caller by decode's static
// branch (decode.rs, DecodeTryCatch). A real call gets a dispatcher on its
// thrown edge instead (machine/lower_module.rs, attach_eh_dispatch): status 2
// branches to a tag test per clause, then a catch block that delivers the
// payload, then a rethrow tail for clauses that do not match. Both paths have
// to reach the same handler with the same values.

fn catch_from_callee(thrower: &str) -> Result<Vec<Value>, crate::WasmError> {
    let src = std::format!(
        r#"(module (tag $t)
          (func $thr (param i32) (result i32) {thrower})
          (func (export "f") (result i32)
            (block $h
              (try_table (result i32) (catch $t $h) (call $thr (i32.const 1)))
              (return))
            (i32.const 23)))"#
    );
    let bytes = wat::parse_str(&src).expect("parse wat");
    let m = crate::Module::new("eh", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("instantiate");
    let r = world.invoke(id, "f", &[]);
    world.free(id).expect("free");
    r
}

const THROW_IF: &str = "(local.get 0) (i32.const 0) (if (i32.ne) (then (throw $t))) (i32.const 0)";

#[test]
fn engine_catches_throw_from_inlined_callee() {
    assert_eq!(catch_from_callee(THROW_IF).expect("caught"), vec![Value::I32(23)]);
}

/// Same callee padded past the inliner's size limit, so it stays a real call.
#[test]
fn engine_catches_throw_from_real_call() {
    let nops = ["nop"; 40].join(" ");
    let r = catch_from_callee(&std::format!("{nops} {THROW_IF}"));
    assert_eq!(r.expect("caught"), vec![Value::I32(23)]);
}

/// The dispatcher tests each clause's tag in order, so a throw that only the
/// second clause names still lands in that clause.
#[test]
fn engine_tests_each_clause_tag_in_order() {
    let nops = ["nop"; 40].join(" ");
    let src = std::format!(
        r#"(module (tag $a) (tag $b)
          (func $thr (param i32) (result i32) {nops} (throw $a))
          (func (export "f") (result i32)
            (block $h
              (try_table (result i32) (catch $b $h) (catch $a $h) (call $thr (i32.const 1)))
              (return))
            (i32.const 23)))"#
    );
    assert_eq!(invoke_wat(&src).expect("caught"), vec![Value::I32(23)]);
}

/// A throw that crosses a call and matches no clause keeps unwinding out of the
/// function: the host sees an uncaught exception, not a caught one and not a
/// trap.
#[test]
fn engine_rethrows_an_unmatched_throw_across_a_call() {
    let nops = ["nop"; 40].join(" ");
    let r = catch_from_callee_other_tag(&nops);
    let err = r.expect_err("no clause matches, so the throw escapes");
    assert!(err.is_exception(), "escaped as {err:?}");
    assert!(err.exception_tag().is_some(), "the tag survives the rethrow");
}

fn catch_from_callee_other_tag(nops: &str) -> Result<Vec<Value>, crate::WasmError> {
    let src = std::format!(
        r#"(module (tag $a) (tag $b)
          (func $thr (param i32) (result i32) {nops} (throw $b))
          (func (export "f") (result i32)
            (block $h
              (try_table (result i32) (catch $a $h) (call $thr (i32.const 1)))
              (return))
            (i32.const 23)))"#
    );
    invoke_wat(&src)
}

/// Run a whole wat module's export "f".
fn invoke_wat(src: &str) -> Result<Vec<Value>, crate::WasmError> {
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("probe", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("instantiate");
    let r = world.invoke(id, "f", &[]);
    world.free(id).expect("free");
    r
}

/// A live local survives a call, and survives a throw that call raised and the
/// caller caught. The second half is what the setjmp lowering depends on: the
/// resume point reads locals that were never passed in registers along the
/// exception edge.
#[test]
fn locals_survive_a_call_and_a_caught_throw() {
    let normal = r#"(module
      (func $id (param i32) (result i32) (local.get 0))
      (func (export "f") (result i32)
        (local $x i32) (local $y i32) (local $z i32)
        (local.set $x (i32.const 111))
        (local.set $y (i32.const 7))
        (local.set $z (call $id (i32.const 222)))
        (i32.add (i32.add (local.get $x) (local.get $y)) (local.get $z))))"#;
    let thrown = r#"(module (tag $t)
      (func $thr (throw $t))
      (func (export "f") (result i32)
        (local $x i32) (local $y i32) (local $z i32)
        (local.set $x (i32.const 111))
        (local.set $y (i32.const 7))
        (block $h
          (try_table (catch $t $h) (call $thr))
          (return (i32.const 0)))
        (i32.add (i32.add (local.get $x) (local.get $y)) (i32.const 3))))"#;
    // Same, but the thrower is a register-hungry body rather than a bare throw.
    let thrown_busy = r#"(module (tag $t)
      (func $thr (param i32 i32 i32) (result i32) (local i32) (local i32) (local i32) (local i32)
        (local.set 3 (i32.add (local.get 0) (local.get 1)))
        (local.set 4 (i32.mul (local.get 3) (local.get 2)))
        (local.set 5 (i32.sub (local.get 4) (local.get 0)))
        (local.set 6 (i32.add (local.get 5) (local.get 1)))
        (if (i32.eqz (local.get 6)) (then (return (local.get 6))))
        (throw $t))
      (func (export "f") (result i32)
        (local $x i32) (local $y i32) (local $z i32)
        (local.set $x (i32.const 111))
        (local.set $y (i32.const 7))
        (block $h
          (try_table (catch $t $h) (drop (call $thr (i32.const 3) (i32.const 5) (i32.const 9))))
          (return (i32.const 0)))
        (i32.add (i32.add (local.get $x) (local.get $y)) (i32.const 3))))"#;
    let c = invoke_wat(thrown_busy);
    let a = invoke_wat(normal);
    let b = invoke_wat(thrown);
    assert_eq!(a.expect("normal"), vec![Value::I32(340)]);
    assert_eq!(b.expect("thrown"), vec![Value::I32(121)]);
    assert_eq!(c.expect("busy"), vec![Value::I32(121)]);
}

/// One pass of the setjmp-in-a-loop shape, returning what the resume point
/// sees: 210 means i reached 2 and sum 10, i.e. both locals crossed the throw
/// with the values they had at the call.
#[test]
fn resume_sees_locals_from_before_the_call() {
    let (i, r, sum) = (0, 1, 2);
    let inc = |l: u32, e: Expr| Op::LocalSet { idx: l, val: add(get(l), e) };
    let main = func(
        vec![],
        vec![Type::I32],
        vec![Type::I32, Type::I32, Type::I32],
        vec![
            Op::LocalSet { idx: i, val: c(0) },
            Op::LocalSet { idx: sum, val: c(0) },
            label(0),
            Op::JmpIf { cond: Expr::binary(BinaryOp::Ge, get(i), c(3), Type::I32), target: 9 },
            setjmp(16, r),
            inc(sum, get(r)),
            inc(i, c(1)),
            Op::JmpIf { cond: get(r), target: 7 },
            call(1, vec![c(16), c(10)]),
            Op::Jmp { target: 0 },
            label(7),
            ret(add(Expr::binary(BinaryOp::Mul, get(i), c(100), Type::I32), get(sum))),
            label(9),
            ret(c(-1)),
        ],
    );
    let m = module(vec![main, longjmper()]);
    assert_eq!(run_i32(&m, vec![]), 210);
}

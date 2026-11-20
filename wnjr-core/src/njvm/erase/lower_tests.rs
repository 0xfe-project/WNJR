//! N7 tests: Switch, load/store gc tag stripping (#634), Call/CallIndirect,
//! trivial mappings. Every behaviour test encodes the module, hands the bytes
//! to WNJR `Module::new`, instantiates it on the JIT and asserts the result.

use super::*;
use crate::collections::{vec, Vec};
use crate::njvm::encode::encode;
use crate::njvm::module::{FunctionType, MemoryType, TableType};
use crate::njvm::{verify_module, ElementSegment, InitExpr, ModuleBuilder, SegmentMode};
use crate::{Config, Engine, RuntimeWorld, Value};
use alloc::boxed::Box;

fn c(ty: Type, v: i32) -> Expr {
    Expr::Const { ty, val: ConstVal::I32(v) }
}
fn c64(ty: Type, v: i64) -> Expr {
    Expr::Const { ty, val: ConstVal::I64(v) }
}
fn bin(op: BinaryOp, ty: Type, a: Expr, b: Expr) -> Expr {
    Expr::binary(op, a, b, ty)
}
fn ret(e: Expr) -> Op {
    Op::Return { vals: vec![e] }
}
fn lset(idx: u32, v: i32) -> Op {
    Op::LocalSet { idx, val: c(Type::I32, v) }
}

/// Instantiate `m` and call `name` once per argument list; Err = trap.
fn run_many(m: &Module, name: &str, calls: &[Vec<Value>]) -> Vec<Result<Vec<Value>, ()>> {
    let bytes = encode(m).expect("encode");
    let module = crate::Module::new("n7", &bytes).expect("WNJR parse + validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, module, &[]).expect("instantiate");
    let out = calls.iter().map(|a| world.invoke(id, name, a).map_err(|_| ())).collect();
    world.free(id).expect("free");
    out
}

fn one_fn(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>, body: Vec<Op>, mem_max: u32) -> Module {
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType { min: 1, max: Some(mem_max), shared: false });
    let mut f = Function::new(params, results, locals);
    f.body = body;
    mb.add_function(f);
    mb.export_function("f", 0);
    mb.build()
}

/// Count `br_table` and `br_if` operators in function 0's wasm body.
fn dispatch_ops(m: &Module) -> (usize, usize) {
    let bytes = encode(m).expect("encode");
    let (mut tables, mut brifs) = (0, 0);
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("parse") {
            let mut r = body.get_operators_reader().expect("ops");
            while !r.eof() {
                match r.read().expect("op") {
                    wasmparser::Operator::BrTable { .. } => tables += 1,
                    wasmparser::Operator::BrIf { .. } => brifs += 1,
                    _ => {}
                }
            }
        }
    }
    (tables, brifs)
}

// ── Switch ──────────────────────────────────────────────────────────────────

/// `fn f(x: ty) -> i32 { l = 99; switch x { v_j => l = r_j, _ => l = def } return l }`
/// The "l = 99" before the switch catches a dispatch that skips every arm.
fn switch_fn(ty: Type, cases: Vec<(ConstVal, i32)>, def: i32) -> Module {
    let body = vec![
        lset(1, 99),
        Op::Switch {
            scrutinee: Expr::LocalGet(0),
            cases: cases.into_iter().map(|(v, r)| (v, vec![lset(1, r)])).collect(),
            default: vec![lset(1, def)],
        },
        ret(Expr::LocalGet(1)),
    ];
    one_fn(vec![ty], vec![Type::I32], vec![Type::I32], body, 1)
}

fn ok_i32(r: &Result<Vec<Value>, ()>) -> i32 {
    match r {
        Ok(v) => match v.as_slice() {
            [Value::I32(x)] => *x,
            o => panic!("expected one i32, got {o:?}"),
        },
        Err(()) => panic!("unexpected trap"),
    }
}

#[test]
fn switch_dense_uses_br_table() {
    // 10..=13 and 15: 5 cases over span 6, dense. 14 is a hole -> default.
    let cases = vec![10, 11, 12, 13, 15].into_iter().map(|v| (ConstVal::I32(v), v * 100)).collect();
    let m = switch_fn(Type::I32, cases, -1);
    verify_module(&m).expect("N3 accepts the switch");
    assert_eq!(dispatch_ops(&m), (1, 0), "dense switch must be one br_table, no br_if");
    let xs = [10, 11, 12, 13, 14, 15, 16, 9, 0, i32::MIN, i32::MAX];
    let out = run_many(&m, "f", &xs.iter().map(|x| vec![Value::I32(*x)]).collect::<Vec<_>>());
    let got: Vec<i32> = out.iter().map(ok_i32).collect();
    assert_eq!(got, vec![1000, 1100, 1200, 1300, -1, 1500, -1, -1, -1, -1, -1]);
}

#[test]
fn switch_sparse_uses_compare_chain() {
    // 4 cases over a span of ~1M: sparse.
    let vals = [1, 1000, -5, 1 << 20];
    let cases = vals.iter().enumerate().map(|(j, v)| (ConstVal::I32(*v), j as i32 + 1)).collect();
    let m = switch_fn(Type::I32, cases, -1);
    assert_eq!(dispatch_ops(&m), (0, 4), "sparse switch must be a chain of 4 br_if");
    let xs = [1, 1000, -5, 1 << 20, 2, 0, -4];
    let out = run_many(&m, "f", &xs.iter().map(|x| vec![Value::I32(*x)]).collect::<Vec<_>>());
    let got: Vec<i32> = out.iter().map(ok_i32).collect();
    assert_eq!(got, vec![1, 2, 3, 4, -1, -1, -1]);
}

#[test]
fn switch_default_only_and_fallthrough_free() {
    // No cases at all: the default runs.
    let m = switch_fn(Type::I32, vec![], 7);
    assert_eq!(ok_i32(&run_many(&m, "f", &[vec![Value::I32(3)]])[0]), 7);
    // Empty default: a miss keeps the value set before the switch.
    let body = vec![
        lset(1, 99),
        Op::Switch {
            scrutinee: Expr::LocalGet(0),
            cases: vec![(ConstVal::I32(1), vec![lset(1, 1)]), (ConstVal::I32(2), vec![lset(1, 2)])],
            default: vec![],
        },
        ret(Expr::LocalGet(1)),
    ];
    let m = one_fn(vec![Type::I32], vec![Type::I32], vec![Type::I32], body, 1);
    let out = run_many(&m, "f", &[vec![Value::I32(1)], vec![Value::I32(2)], vec![Value::I32(5)]]);
    assert_eq!(out.iter().map(ok_i32).collect::<Vec<_>>(), vec![1, 2, 99]);
}

#[test]
fn switch_duplicate_case_first_wins() {
    let cases = vec![(ConstVal::I32(4), 1), (ConstVal::I32(4), 2), (ConstVal::I32(5), 3)];
    let m = switch_fn(Type::I32, cases, -1);
    let out = run_many(&m, "f", &[vec![Value::I32(4)], vec![Value::I32(5)]]);
    assert_eq!(out.iter().map(ok_i32).collect::<Vec<_>>(), vec![1, 3]);
}

#[test]
fn switch_unsigned_dense_spans_sign_boundary() {
    // U32 cases 0x7FFF_FFFE..=0x8000_0001: dense in unsigned order even
    // though the signed values are i32::MAX-1 .. i32::MIN+1.
    let vals: Vec<i32> = vec![0x7FFF_FFFE, 0x7FFF_FFFF, 0x8000_0000u32 as i32, 0x8000_0001u32 as i32];
    let cases = vals.iter().enumerate().map(|(j, v)| (ConstVal::I32(*v), j as i32 + 1)).collect();
    let m = switch_fn(Type::U32, cases, -1);
    assert_eq!(dispatch_ops(&m).0, 1);
    let mut xs = vals.clone();
    xs.extend([0x8000_0002u32 as i32, 0x7FFF_FFFD, 0]);
    let out = run_many(&m, "f", &xs.iter().map(|x| vec![Value::I32(*x)]).collect::<Vec<_>>());
    assert_eq!(out.iter().map(ok_i32).collect::<Vec<_>>(), vec![1, 2, 3, 4, -1, -1, -1]);
}

#[test]
fn switch_i64_dense_does_not_alias_mod_2_32() {
    // Cases 2^40 + 0..=3. 2^40 + 3 + 2^32 differs only above bit 31: an
    // i64 dispatch that narrows before the range check would hit case 3.
    let base: i64 = 1 << 40;
    let cases = (0..4).map(|j| (ConstVal::I64(base + j), j as i32 + 1)).collect();
    let m = switch_fn(Type::I64, cases, -1);
    assert_eq!(dispatch_ops(&m).0, 1);
    let xs = [base, base + 1, base + 2, base + 3, base + 3 + (1 << 32), base - 1, base + 4, 0];
    let out = run_many(&m, "f", &xs.iter().map(|x| vec![Value::I64(*x)]).collect::<Vec<_>>());
    assert_eq!(out.iter().map(ok_i32).collect::<Vec<_>>(), vec![1, 2, 3, 4, -1, -1, -1, -1]);
}

#[test]
fn switch_i64_sparse() {
    let vals: [i64; 3] = [-1, 1 << 50, 7];
    let cases = vals.iter().enumerate().map(|(j, v)| (ConstVal::I64(*v), j as i32 + 1)).collect();
    let m = switch_fn(Type::I64, cases, -1);
    assert_eq!(dispatch_ops(&m), (0, 3));
    let xs = [-1i64, 1 << 50, 7, (1 << 50) + 7, 0xFFFF_FFFF];
    let out = run_many(&m, "f", &xs.iter().map(|x| vec![Value::I64(*x)]).collect::<Vec<_>>());
    assert_eq!(out.iter().map(ok_i32).collect::<Vec<_>>(), vec![1, 2, 3, -1, -1]);
}

#[test]
fn switch_nested_and_returning_arms() {
    // switch a { 0 => switch b { 0 => return 10, 1 => l = 11 }, 1 => return 20 }
    // with an inner default of l = 12 and an outer default of l = 30.
    let inner = Op::Switch {
        scrutinee: Expr::LocalGet(1),
        cases: vec![(ConstVal::I32(0), vec![ret(c(Type::I32, 10))]), (ConstVal::I32(1), vec![lset(2, 11)])],
        default: vec![lset(2, 12)],
    };
    let body = vec![
        lset(2, 99),
        Op::Switch {
            scrutinee: Expr::LocalGet(0),
            cases: vec![(ConstVal::I32(0), vec![inner]), (ConstVal::I32(1), vec![ret(c(Type::I32, 20))])],
            default: vec![lset(2, 30)],
        },
        ret(Expr::LocalGet(2)),
    ];
    let m = one_fn(vec![Type::I32, Type::I32], vec![Type::I32], vec![Type::I32], body, 1);
    let args = [(0, 0), (0, 1), (0, 5), (1, 0), (2, 0)];
    let out = run_many(&m, "f", &args.iter().map(|(a, b)| vec![Value::I32(*a), Value::I32(*b)]).collect::<Vec<_>>());
    assert_eq!(out.iter().map(ok_i32).collect::<Vec<_>>(), vec![10, 11, 12, 20, 30]);
}

#[test]
fn type_switch_is_not_implemented() {
    let body = vec![Op::TypeSwitch { scrutinee: c(Type::I32, 0), cases: vec![] }];
    let m = one_fn(vec![], vec![], vec![], body, 1);
    let err = lower_function(&Env::new(&m), &m.functions[0]).unwrap_err();
    assert!(matches!(err.kind, EraseErrorKind::NotImplemented { op: "TypeSwitch", .. }), "{err:?}");
}

// ── load / store: gc tag stripping (#634) ───────────────────────────────────

/// `fn f(p: u64) -> i32`: p is a tagged pointer (tag in bits 32..63, memory0
/// address in bits 0..31).
///   *(i32*)(p + 4)  = 0x1234_5678       (Store, offset 4)
///   *(i64*)(p + 8)  = *(i32*)(p + 4)    (Load + Store, offsets 4/8, i64 via extend)
///   return *(i32*)(addr + 4) + *(u8*)(p + 8) + (i32)*(i64*)(p+8) - 0x1234_5678
/// where `addr` is the untagged low half as a plain `Ptr` (256 here), so the
/// access through the tagged pointer must have landed at 256 + offset.
fn tagged_fn() -> Module {
    let p = || Expr::LocalGet(0);
    let body = vec![
        Op::Store { ty: MemType::I32, ptr: p(), val: c(Type::I32, 0x1234_5678), offset: 4, align: 2 },
        Op::Store {
            ty: MemType::I64,
            ptr: p(),
            val: Expr::Convert {
                from: Type::I32,
                to: Type::I64,
                val: Box::new(Expr::Load { ty: MemType::I32, ptr: Box::new(p()), offset: 4, align: 2 }),
            },
            offset: 8,
            align: 3,
        },
        ret(bin(
            BinaryOp::Sub,
            Type::I32,
            bin(
                BinaryOp::Add,
                Type::I32,
                bin(
                    BinaryOp::Add,
                    Type::I32,
                    Expr::Load { ty: MemType::I32, ptr: Box::new(c(Type::Ptr, 256)), offset: 4, align: 2 },
                    Expr::Convert {
                        from: Type::U8,
                        to: Type::I32,
                        val: Box::new(Expr::Load { ty: MemType::U8, ptr: Box::new(p()), offset: 8, align: 0 }),
                    },
                ),
                Expr::Convert {
                    from: Type::I64,
                    to: Type::I32,
                    val: Box::new(Expr::Load { ty: MemType::I64, ptr: Box::new(p()), offset: 8, align: 3 }),
                },
            ),
            c(Type::I32, 0x1234_5678),
        )),
    ];
    one_fn(vec![Type::U64], vec![Type::I32], vec![], body, 1)
}

#[test]
fn load_store_strip_gc_tag() {
    let m = tagged_fn();
    verify_module(&m).expect("N3 accepts a U64 (tagged) address");
    // Tags: none, small, large, all ones. Address 256 in the low half.
    let tags: [u64; 4] = [0, 7, 0xABCD, 0xFFFF_FFFF];
    let calls: Vec<Vec<Value>> = tags.iter().map(|t| vec![Value::I64(((t << 32) | 256) as i64)]).collect();
    for (t, r) in tags.iter().zip(run_many(&m, "f", &calls)) {
        // 0x1234_5678 + 0x78 + 0x1234_5678 - 0x1234_5678
        assert_eq!(ok_i32(&r), 0x1234_5678 + 0x78, "tag {t:#x}");
    }
}

#[test]
fn memory_copy_fill_strip_gc_tag() {
    // fill(tagged 512, 0x5A, 4); copy(tagged 600, tagged 512, 4); return *(i32*)600
    let tagged = |a: i64| c64(Type::U64, (0x00C0_FFEEi64 << 32) | a);
    let body = vec![
        Op::MemoryFill { dest: tagged(512), val: c(Type::I32, 0x5A), len: c(Type::I32, 4) },
        Op::MemoryCopy { dest: tagged(600), src: tagged(512), len: c(Type::I32, 4) },
        ret(Expr::Load { ty: MemType::I32, ptr: Box::new(c(Type::Ptr, 600)), offset: 0, align: 2 }),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![], body, 1);
    verify_module(&m).expect("N3 accepts tagged copy/fill addresses");
    assert_eq!(ok_i32(&run_many(&m, "f", &[vec![]])[0]), 0x5A5A_5A5A);
}

// ── Call / CallIndirect ─────────────────────────────────────────────────────

/// f0 sub(a,b), f1 mul(a,b): type (i32,i32)->i32. f2 seven(): ()->i32.
/// table 0 = [f0, f1, f2] (size 4, slot 3 null). f3 main(i, a, b) =
/// call_indirect type(i32,i32)->i32 table 0 [i](a, b).
fn indirect_module() -> Module {
    let mut mb = ModuleBuilder::new();
    let sig = mb.add_type(FunctionType { params: vec![Type::I32, Type::I32], results: vec![Type::I32] });
    mb.add_table(TableType { min: 4, max: Some(4) });
    for op in [BinaryOp::Sub, BinaryOp::Mul] {
        let mut f = Function::new(vec![Type::I32, Type::I32], vec![Type::I32], vec![]);
        f.body = vec![ret(bin(op, Type::I32, Expr::LocalGet(0), Expr::LocalGet(1)))];
        mb.add_function(f);
    }
    let mut seven = Function::new(vec![], vec![Type::I32], vec![]);
    seven.body = vec![ret(c(Type::I32, 7))];
    mb.add_function(seven);
    let mut main = Function::new(vec![Type::I32, Type::I32, Type::I32], vec![Type::I32], vec![]);
    main.body = vec![ret(Expr::CallIndirect {
        type_idx: sig,
        table_idx: 0,
        callee: Box::new(Expr::LocalGet(0)),
        args: vec![Expr::LocalGet(1), Expr::LocalGet(2)],
    })];
    mb.add_function(main);
    mb.add_element(ElementSegment { mode: SegmentMode::Active { index: 0, offset: InitExpr::i32(0) }, funcs: vec![0, 1, 2] });
    mb.export_function("main", 3);
    mb.build()
}

#[test]
fn call_indirect_two_targets_and_signature_trap() {
    let m = indirect_module();
    verify_module(&m).expect("N3 accepts the call_indirect module");
    let v = |i: i32| vec![Value::I32(i), Value::I32(9), Value::I32(4)];
    let out = run_many(&m, "main", &[v(0), v(1), v(2), v(3), v(4)]);
    assert_eq!(ok_i32(&out[0]), 5, "table[0] = sub");
    assert_eq!(ok_i32(&out[1]), 36, "table[1] = mul");
    assert!(out[2].is_err(), "table[2] has type ()->i32: signature mismatch must trap");
    assert!(out[3].is_err(), "null slot must trap");
    assert!(out[4].is_err(), "out of table bounds must trap");
}

#[test]
fn call_args_evaluate_left_to_right() {
    // l = 0; return sub(tee l = l*10+1, tee l = l*10+2) * 1000 + l
    // left to right: sub(1, 12) = -11, l = 12 -> -10988.
    // right to left would give sub(21, 2) = 19, l = 21 -> 19021.
    let step = |d: i32| Expr::LocalTee {
        idx: 0,
        val: Box::new(bin(
            BinaryOp::Add,
            Type::I32,
            bin(BinaryOp::Mul, Type::I32, Expr::LocalGet(0), c(Type::I32, 10)),
            c(Type::I32, d),
        )),
    };
    let mut mb = ModuleBuilder::new();
    let sig = mb.add_type(FunctionType { params: vec![Type::I32, Type::I32], results: vec![Type::I32] });
    mb.add_table(TableType { min: 1, max: Some(1) });
    let mut sub = Function::new(vec![Type::I32, Type::I32], vec![Type::I32], vec![]);
    sub.body = vec![ret(bin(BinaryOp::Sub, Type::I32, Expr::LocalGet(0), Expr::LocalGet(1)))];
    mb.add_function(sub);
    let combine = |call: Expr| {
        bin(BinaryOp::Add, Type::I32, bin(BinaryOp::Mul, Type::I32, call, c(Type::I32, 1000)), Expr::LocalGet(0))
    };
    let mut direct = Function::new(vec![], vec![Type::I32], vec![Type::I32]);
    direct.body = vec![ret(combine(Expr::Call { func_idx: 0, args: vec![step(1), step(2)] }))];
    mb.add_function(direct);
    let mut indirect = Function::new(vec![], vec![Type::I32], vec![Type::I32]);
    indirect.body = vec![ret(combine(Expr::CallIndirect {
        type_idx: sig,
        table_idx: 0,
        callee: Box::new(c(Type::I32, 0)),
        args: vec![step(1), step(2)],
    }))];
    mb.add_function(indirect);
    mb.add_element(ElementSegment { mode: SegmentMode::Active { index: 0, offset: InitExpr::i32(0) }, funcs: vec![0] });
    mb.export_function("direct", 1);
    mb.export_function("indirect", 2);
    let m = mb.build();
    assert_eq!(ok_i32(&run_many(&m, "direct", &[vec![]])[0]), -10988);
    assert_eq!(ok_i32(&run_many(&m, "indirect", &[vec![]])[0]), -10988);
}

// ── trivial mappings ────────────────────────────────────────────────────────

#[test]
fn trivial_memory_grow_size_nop_drop_fence() {
    // grow by 1 page, then store/load across the old end (65536). Without
    // the grow the store at 70000 traps.
    let body = vec![
        Op::Nop,
        Op::MemoryGrow { delta: c(Type::I32, 1) },
        Op::MemorySize,
        Op::Atomic { op: AtomicOp::Fence, width: 0, offset: 0, ptr: None, val: None, replacement: None, dest: None },
        Op::Store { ty: MemType::I32, ptr: c(Type::Ptr, 70000), val: c(Type::I32, 4242), offset: 0, align: 2 },
        Op::Drop { val: c(Type::I32, 1) },
        ret(Expr::Load { ty: MemType::I32, ptr: Box::new(c(Type::Ptr, 70000)), offset: 0, align: 2 }),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![], body, 2);
    assert_eq!(ok_i32(&run_many(&m, "f", &[vec![]])[0]), 4242);
}

#[test]
fn trivial_return_unreachable() {
    // if-free: Return before Unreachable means the trap is never reached.
    let m = one_fn(vec![], vec![Type::I32], vec![], vec![ret(c(Type::I32, 3)), Op::Unreachable], 1);
    assert_eq!(ok_i32(&run_many(&m, "f", &[vec![]])[0]), 3);
    let m = one_fn(vec![], vec![Type::I32], vec![], vec![Op::Unreachable, ret(c(Type::I32, 3))], 1);
    assert!(run_many(&m, "f", &[vec![]])[0].is_err());
}

fn atomic(op: AtomicOp, width: u8, ptr: Option<Expr>, val: Option<Expr>, replacement: Option<Expr>, dest: Option<u32>) -> Op {
    Op::Atomic { op, width, offset: 0, ptr, val, replacement, dest }
}

fn lget(idx: u32) -> Expr {
    Expr::LocalGet(idx)
}

const SLOT: i32 = 1024;

#[test]
fn atomic_rmw_and_cmpxchg_yield_the_old_value() {
    // A store, then an add, then a compare-exchange that succeeds: each yields
    // what the slot held before it, and the slot itself is only reachable
    // through the atomics.
    let body = vec![
        atomic(AtomicOp::Store, 4, Some(c(Type::Ptr, SLOT)), Some(c(Type::I32, 10)), None, None),
        atomic(AtomicOp::Add, 4, Some(c(Type::Ptr, SLOT)), Some(c(Type::I32, 5)), None, Some(0)),
        atomic(
            AtomicOp::Cmpxchg,
            4,
            Some(c(Type::Ptr, SLOT)),
            Some(c(Type::I32, 15)),
            Some(c(Type::I32, 40)),
            Some(1),
        ),
        ret(bin(BinaryOp::Add, Type::I32, lget(0), lget(1))),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![Type::I32, Type::I32], body, 1);
    // 10 (the value before the add) + 15 (the value before the exchange).
    assert_eq!(ok_i32(&run_many(&m, "f", &[vec![]])[0]), 25);
}

#[test]
fn atomic_cmpxchg_that_does_not_match_leaves_the_slot() {
    // The same program with an expected value that is not there: the slot keeps
    // its value and the old value still comes back, so the sum differs.
    let body = vec![
        atomic(AtomicOp::Store, 4, Some(c(Type::Ptr, SLOT)), Some(c(Type::I32, 10)), None, None),
        atomic(AtomicOp::Add, 4, Some(c(Type::Ptr, SLOT)), Some(c(Type::I32, 5)), None, Some(0)),
        atomic(
            AtomicOp::Cmpxchg,
            4,
            Some(c(Type::Ptr, SLOT)),
            Some(c(Type::I32, 99)),
            Some(c(Type::I32, 40)),
            Some(1),
        ),
        ret(bin(BinaryOp::Add, Type::I32, lget(0), lget(1))),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![Type::I32, Type::I32], body, 1);
    // 10 + 15 again: the failed exchange returns what was there, not the desired
    // value.
    assert_eq!(ok_i32(&run_many(&m, "f", &[vec![]])[0]), 25);

    // And the slot is untouched by the failed exchange: exchange for real and
    // check the value the exchange replaced.
    let body = vec![
        atomic(AtomicOp::Store, 4, Some(c(Type::Ptr, SLOT)), Some(c(Type::I32, 10)), None, None),
        atomic(
            AtomicOp::Cmpxchg,
            4,
            Some(c(Type::Ptr, SLOT)),
            Some(c(Type::I32, 99)),
            Some(c(Type::I32, 40)),
            Some(0),
        ),
        atomic(AtomicOp::Load, 4, Some(c(Type::Ptr, SLOT)), None, None, Some(1)),
        ret(bin(BinaryOp::Mul, Type::I32, lget(0), lget(1))),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![Type::I32, Type::I32], body, 1);
    // The exchange failed (10 was there, 99 was expected) and the slot is still
    // 10: 10 * 10.
    assert_eq!(ok_i32(&run_many(&m, "f", &[vec![]])[0]), 100);
}

#[test]
fn atomic_narrow_width_touches_one_byte_only() {
    // A 1-byte exchange must not disturb the three bytes after it.
    let body = vec![
        atomic(AtomicOp::Store, 4, Some(c(Type::Ptr, SLOT)), Some(c(Type::I32, 0x11223344)), None, None),
        atomic(AtomicOp::Xchg, 1, Some(c(Type::Ptr, SLOT)), Some(c(Type::I32, 0xff)), None, Some(0)),
        atomic(AtomicOp::Load, 4, Some(c(Type::Ptr, SLOT)), None, None, Some(1)),
        // The byte exchanged away (0x44) plus the word that is left.
        ret(bin(
            BinaryOp::Add,
            Type::I32,
            lget(0),
            bin(BinaryOp::Shr, Type::I32, lget(1), c(Type::I32, 8)),
        )),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![Type::I32, Type::I32], body, 1);
    // 0x44 + 0x112233 = 0x112277.
    assert_eq!(ok_i32(&run_many(&m, "f", &[vec![]])[0]), 0x112277);
}

#[test]
fn atomic_operand_shape_is_checked() {
    // An add with no value to add is a shape error, not a silent zero.
    let body = vec![
        atomic(AtomicOp::Add, 4, Some(c(Type::Ptr, SLOT)), None, None, Some(0)),
        ret(c(Type::I32, 0)),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![Type::I32], body, 1);
    let err = lower_function(&Env::new(&m), &m.functions[0]).unwrap_err();
    assert!(matches!(err.kind, EraseErrorKind::AtomicShape { op: AtomicOp::Add }), "{err:?}");

    // A store with no value has nothing to write.
    let body = vec![
        atomic(AtomicOp::Store, 4, Some(c(Type::Ptr, SLOT)), None, None, None),
        ret(c(Type::I32, 0)),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![], body, 1);
    let err = lower_function(&Env::new(&m), &m.functions[0]).unwrap_err();
    assert!(matches!(err.kind, EraseErrorKind::AtomicShape { op: AtomicOp::Store }), "{err:?}");

    // A width wasm has no instruction for is its own error.
    let body = vec![
        atomic(AtomicOp::Add, 3, Some(c(Type::Ptr, SLOT)), Some(c(Type::I32, 1)), None, Some(0)),
        ret(c(Type::I32, 0)),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![Type::I32], body, 1);
    let err = lower_function(&Env::new(&m), &m.functions[0]).unwrap_err();
    assert!(matches!(err.kind, EraseErrorKind::AtomicWidth { width: 3 }), "{err:?}");
}

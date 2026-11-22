//! The case library: hand-built NJVM-IR modules with a native Rust oracle
//! for every call. Coverage: arithmetic of every type, flat-CFG control flow
//! (loops, nesting, goto, `BrTable`, computed goto), Switch, load/store
//! including gc-tagged pointers, call / call_indirect, globals, exceptions,
//! and setjmp/longjmp.

use wnjr_core::njvm::{
    BinaryOp, Catch, ConstVal, ElementSegment, Expr, Function, FunctionType, InitExpr, MemType,
    MemoryType, Module, ModuleBuilder, Op, SegmentMode, TableType, Tag, Type, UnaryOp,
};

use crate::{Arg, Case, Outcome};

// ───────────────────────────── IR shorthands ─────────────────────────────

fn k(ty: Type, v: i32) -> Expr {
    Expr::Const {
        ty,
        val: ConstVal::I32(v),
    }
}
fn ci(v: i32) -> Expr {
    k(Type::I32, v)
}
fn cl(ty: Type, v: i64) -> Expr {
    Expr::Const {
        ty,
        val: ConstVal::I64(v),
    }
}
fn get(i: u32) -> Expr {
    Expr::LocalGet(i)
}
fn bin(op: BinaryOp, ty: Type, a: Expr, b: Expr) -> Expr {
    Expr::binary(op, a, b, ty)
}
fn add(a: Expr, b: Expr) -> Expr {
    bin(BinaryOp::Add, Type::I32, a, b)
}
fn sub(a: Expr, b: Expr) -> Expr {
    bin(BinaryOp::Sub, Type::I32, a, b)
}
fn mul(a: Expr, b: Expr) -> Expr {
    bin(BinaryOp::Mul, Type::I32, a, b)
}
fn cmp(op: BinaryOp, a: Expr, b: Expr) -> Expr {
    bin(op, Type::I32, a, b)
}
fn conv(from: Type, to: Type, e: Expr) -> Expr {
    Expr::Convert {
        from,
        to,
        val: Box::new(e),
    }
}
fn load(ty: MemType, addr: Expr, offset: u32) -> Expr {
    Expr::Load {
        ty,
        ptr: Box::new(addr),
        offset,
        align: 0,
    }
}
fn store(ty: MemType, addr: Expr, val: Expr, offset: u32) -> Op {
    Op::Store {
        ty,
        ptr: addr,
        val,
        offset,
        align: 0,
    }
}
fn ret(e: Expr) -> Op {
    Op::Return { vals: vec![e] }
}
fn ret_void() -> Op {
    Op::Return { vals: vec![] }
}
fn set(i: u32, e: Expr) -> Op {
    Op::LocalSet { idx: i, val: e }
}
fn lbl(id: u32) -> Op {
    Op::Label { id, params: vec![] }
}
fn jmp(t: u32) -> Op {
    Op::Jmp { target: t }
}
fn jif(cond: Expr, t: u32) -> Op {
    Op::JmpIf { cond, target: t }
}
fn call(f: u32, args: Vec<Expr>) -> Op {
    Op::Call { func_idx: f, args }
}
fn callx(f: u32, args: Vec<Expr>) -> Expr {
    Expr::Call { func_idx: f, args }
}
fn func(params: &[Type], results: &[Type], locals: &[Type], body: Vec<Op>) -> Function {
    let mut f = Function::new(params.to_vec(), results.to_vec(), locals.to_vec());
    f.body = body;
    f
}

/// A module of `funcs` with one page of memory; function `export` is "f".
fn module(funcs: Vec<Function>, export: u32) -> Module {
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType {
        min: 1,
        max: Some(1),
        shared: false,
    });
    for f in funcs {
        mb.add_function(f);
    }
    mb.export_function("f", export);
    mb.build()
}

fn single(f: Function) -> Module {
    module(vec![f], 0)
}

fn case(
    name: impl Into<String>,
    module: Module,
    stateless: bool,
    calls: Vec<(Vec<Arg>, Outcome)>,
) -> Case {
    Case {
        name: name.into(),
        module,
        export: "f".into(),
        calls,
        stateless,
    }
}

const PAGE: u64 = 65536;

// ───────────────────────────── integer arithmetic ─────────────────────────────

#[derive(Clone, Copy)]
struct IntTy {
    ty: Type,
    bits: u64,
    signed: bool,
}

const INT_TYPES: [IntTy; 4] = [
    IntTy {
        ty: Type::I32,
        bits: 32,
        signed: true,
    },
    IntTy {
        ty: Type::U32,
        bits: 32,
        signed: false,
    },
    IntTy {
        ty: Type::I64,
        bits: 64,
        signed: true,
    },
    IntTy {
        ty: Type::U64,
        bits: 64,
        signed: false,
    },
];

fn int_arg(t: IntTy, v: i64) -> Arg {
    if t.bits == 32 {
        Arg::I32(v as i32)
    } else {
        Arg::I64(v)
    }
}

fn int_out(t: IntTy, v: i64) -> Outcome {
    if t.bits == 32 {
        Outcome::i32(v as i32)
    } else {
        Outcome::i64(v)
    }
}

fn int_vals(t: IntTy) -> Vec<i64> {
    let mut v: Vec<i64> = vec![
        0,
        1,
        -1,
        2,
        3,
        7,
        31,
        32,
        0x7fff_ffff,
        i32::MIN as i64,
        0x1234_5678,
        0x8000_0001u32 as i32 as i64,
    ];
    if t.bits == 64 {
        v.extend([63, 64, 0x1_0000_0001, i64::MIN, i64::MAX, -(1 << 40)]);
    }
    v
}

fn is_cmp(op: BinaryOp) -> bool {
    use BinaryOp::*;
    matches!(op, Lt | Le | Gt | Ge | Eq | Ne)
}

/// The wasm-level result of `a op b` on `t`, None when it traps.
fn int_oracle(t: IntTy, op: BinaryOp, a: i64, b: i64) -> Option<i64> {
    use BinaryOp::*;
    let w = |v: i64| if t.bits == 32 { v as i32 as i64 } else { v };
    let (sa, sb) = (w(a), w(b));
    let (ua, ub) = if t.bits == 32 {
        (a as u32 as u64, b as u32 as u64)
    } else {
        (a as u64, b as u64)
    };
    let min = if t.bits == 32 {
        i32::MIN as i64
    } else {
        i64::MIN
    };
    let sh = (ub % t.bits) as u32;
    let rot = |left: bool| -> i64 {
        if t.bits == 32 {
            let x = ua as u32;
            (if left {
                x.rotate_left(sh)
            } else {
                x.rotate_right(sh)
            }) as i32 as i64
        } else if left {
            ua.rotate_left(sh) as i64
        } else {
            ua.rotate_right(sh) as i64
        }
    };
    Some(match op {
        Add => w(sa.wrapping_add(sb)),
        Sub => w(sa.wrapping_sub(sb)),
        Mul => w(sa.wrapping_mul(sb)),
        Div if t.signed => {
            if sb == 0 || (sa == min && sb == -1) {
                return None;
            }
            w(sa.wrapping_div(sb))
        }
        Div => {
            if ub == 0 {
                return None;
            }
            w((ua / ub) as i64)
        }
        Rem if t.signed => {
            if sb == 0 {
                return None;
            }
            w(sa.wrapping_rem(sb))
        }
        Rem => {
            if ub == 0 {
                return None;
            }
            w((ua % ub) as i64)
        }
        And => w(sa & sb),
        Or => w(sa | sb),
        Xor => w(sa ^ sb),
        Shl => w(sa.wrapping_shl(sh)),
        Shr if t.signed => w(sa >> sh),
        Shr => w((ua >> sh) as i64),
        Rotl => rot(true),
        Rotr => rot(false),
        Lt => (if t.signed { sa < sb } else { ua < ub }) as i64,
        Le => (if t.signed { sa <= sb } else { ua <= ub }) as i64,
        Gt => (if t.signed { sa > sb } else { ua > ub }) as i64,
        Ge => (if t.signed { sa >= sb } else { ua >= ub }) as i64,
        Eq => (ua == ub) as i64,
        Ne => (ua != ub) as i64,
        Min | Max | Copysign => unreachable!("float-only operator"),
    })
}

const INT_OPS: [BinaryOp; 18] = {
    use BinaryOp::*;
    [
        Add, Sub, Mul, Div, Rem, And, Or, Xor, Shl, Shr, Rotl, Rotr, Lt, Le, Gt, Ge, Eq, Ne,
    ]
};

fn int_binop_cases(out: &mut Vec<Case>) {
    for t in INT_TYPES {
        for op in INT_OPS {
            let res = if is_cmp(op) { Type::I32 } else { t.ty };
            let m = single(func(
                &[t.ty, t.ty],
                &[res],
                &[],
                vec![ret(bin(op, t.ty, get(0), get(1)))],
            ));
            let vals = int_vals(t);
            let mut calls = Vec::new();
            for &a in &vals {
                for &b in &vals {
                    let want = match int_oracle(t, op, a, b) {
                        None => Outcome::Trap,
                        Some(r) if is_cmp(op) => Outcome::i32(r as i32),
                        Some(r) => int_out(t, r),
                    };
                    calls.push((vec![int_arg(t, a), int_arg(t, b)], want));
                }
            }
            out.push(case(
                format!("arith/{:?}/{:?}", t.ty, op).to_lowercase(),
                m,
                true,
                calls,
            ));
        }
    }
}

fn int_unary_cases(out: &mut Vec<Case>) {
    use UnaryOp::*;
    for t in [INT_TYPES[0], INT_TYPES[2]] {
        for op in [Neg, Not, Abs, Clz, Ctz, Popcnt] {
            let m = single(func(
                &[t.ty],
                &[t.ty],
                &[],
                vec![ret(Expr::unary(op, get(0), t.ty))],
            ));
            let calls = int_vals(t)
                .into_iter()
                .map(|a| {
                    let r: i64 = if t.bits == 32 {
                        let x = a as i32;
                        (match op {
                            Neg => x.wrapping_neg(),
                            Not => !x,
                            Abs => x.wrapping_abs(),
                            Clz => x.leading_zeros() as i32,
                            Ctz => x.trailing_zeros() as i32,
                            _ => x.count_ones() as i32,
                        }) as i64
                    } else {
                        match op {
                            Neg => a.wrapping_neg(),
                            Not => !a,
                            Abs => a.wrapping_abs(),
                            Clz => a.leading_zeros() as i64,
                            Ctz => a.trailing_zeros() as i64,
                            _ => a.count_ones() as i64,
                        }
                    };
                    (vec![int_arg(t, a)], int_out(t, r))
                })
                .collect();
            out.push(case(
                format!("arith/{:?}/{:?}", t.ty, op).to_lowercase(),
                m,
                true,
                calls,
            ));
        }
    }
}

// ───────────────────────────── floating point ─────────────────────────────

macro_rules! float_oracles {
    ($bin:ident, $un:ident, $f:ty, $out:path) => {
        fn $bin(op: BinaryOp, a: $f, b: $f) -> Outcome {
            use BinaryOp::*;
            match op {
                Add => $out(a + b),
                Sub => $out(a - b),
                Mul => $out(a * b),
                Div => $out(a / b),
                Min => $out(if a.is_nan() || b.is_nan() {
                    <$f>::NAN
                } else if a == b {
                    if a.is_sign_negative() {
                        a
                    } else {
                        b
                    }
                } else if a < b {
                    a
                } else {
                    b
                }),
                Max => $out(if a.is_nan() || b.is_nan() {
                    <$f>::NAN
                } else if a == b {
                    if a.is_sign_negative() {
                        b
                    } else {
                        a
                    }
                } else if a > b {
                    a
                } else {
                    b
                }),
                Copysign => $out(a.copysign(b)),
                Lt => Outcome::i32((a < b) as i32),
                Le => Outcome::i32((a <= b) as i32),
                Gt => Outcome::i32((a > b) as i32),
                Ge => Outcome::i32((a >= b) as i32),
                Eq => Outcome::i32((a == b) as i32),
                Ne => Outcome::i32((a != b) as i32),
                _ => unreachable!("integer-only operator"),
            }
        }
        fn $un(op: UnaryOp, a: $f) -> Outcome {
            use UnaryOp::*;
            $out(match op {
                Neg => -a,
                Abs => a.abs(),
                Sqrt => a.sqrt(),
                Ceil => a.ceil(),
                Floor => a.floor(),
                Trunc => a.trunc(),
                Nearest => a.round_ties_even(),
                _ => unreachable!("integer-only operator"),
            })
        }
    };
}
float_oracles!(f32_bin, f32_un, f32, Outcome::f32);
float_oracles!(f64_bin, f64_un, f64, Outcome::f64);

fn float_vals() -> Vec<f64> {
    vec![
        0.0,
        -0.0,
        1.0,
        -1.5,
        2.5,
        3.5,
        0.1,
        1e10,
        -1e-10,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        16777217.0,
        4.5,
    ]
}

fn float_cases(out: &mut Vec<Case>) {
    use BinaryOp::*;
    use UnaryOp::*;
    let bins = [
        Add, Sub, Mul, Div, Min, Max, Copysign, Lt, Le, Gt, Ge, Eq, Ne,
    ];
    let uns = [Neg, Abs, Sqrt, Ceil, Floor, Trunc, Nearest];
    for ty in [Type::F32, Type::F64] {
        let is32 = ty == Type::F32;
        let arg = |v: f64| {
            if is32 {
                Arg::f32(v as f32)
            } else {
                Arg::f64(v)
            }
        };
        for op in bins {
            let res = if is_cmp(op) { Type::I32 } else { ty };
            let m = single(func(
                &[ty, ty],
                &[res],
                &[],
                vec![ret(bin(op, ty, get(0), get(1)))],
            ));
            let mut calls = Vec::new();
            for &a in &float_vals() {
                for &b in &float_vals() {
                    let want = if is32 {
                        f32_bin(op, a as f32, b as f32)
                    } else {
                        f64_bin(op, a, b)
                    };
                    calls.push((vec![arg(a), arg(b)], want));
                }
            }
            out.push(case(
                format!("arith/{:?}/{:?}", ty, op).to_lowercase(),
                m,
                true,
                calls,
            ));
        }
        for op in uns {
            let m = single(func(
                &[ty],
                &[ty],
                &[],
                vec![ret(Expr::unary(op, get(0), ty))],
            ));
            let calls = float_vals()
                .into_iter()
                .map(|a| {
                    (
                        vec![arg(a)],
                        if is32 {
                            f32_un(op, a as f32)
                        } else {
                            f64_un(op, a)
                        },
                    )
                })
                .collect();
            out.push(case(
                format!("arith/{:?}/{:?}", ty, op).to_lowercase(),
                m,
                true,
                calls,
            ));
        }
    }
}

// ───────────────────────────── conversions, select ─────────────────────────────

fn conv_case(
    name: &str,
    from: Type,
    to: Type,
    inputs: Vec<Arg>,
    oracle: impl Fn(Arg) -> Outcome,
) -> Case {
    let m = single(func(&[from], &[to], &[], vec![ret(conv(from, to, get(0)))]));
    let calls = inputs.into_iter().map(|a| (vec![a], oracle(a))).collect();
    case(format!("convert/{name}"), m, true, calls)
}

fn i32s() -> Vec<Arg> {
    [
        0,
        1,
        -1,
        127,
        128,
        255,
        256,
        -128,
        -129,
        32767,
        32768,
        65535,
        65536,
        0x7fff_ffff,
        i32::MIN,
        0x1234_5678,
        0x89ab_cdefu32 as i32,
    ]
    .into_iter()
    .map(Arg::I32)
    .collect()
}
fn i64s() -> Vec<Arg> {
    let mut v = i32s();
    v.clear();
    [
        0i64,
        1,
        -1,
        0x7fff_ffff,
        0x8000_0000,
        -0x8000_0001,
        0x1_0000_0000,
        0x1234_5678_9abc_def0,
        i64::MAX,
        i64::MIN,
        16777217,
        (1 << 53) + 1,
    ]
    .into_iter()
    .map(Arg::I64)
    .collect()
}
fn f64s() -> Vec<Arg> {
    [
        0.0,
        -0.0,
        0.5,
        -0.5,
        1.9,
        -1.9,
        2147483647.0,
        2147483648.0,
        -2147483648.0,
        -2147483649.0,
        4294967295.0,
        4294967296.0,
        9.3e18,
        -9.3e18,
        1e30,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        16777217.0,
        1e-50,
    ]
    .into_iter()
    .map(Arg::f64)
    .collect()
}
fn x32(a: Arg) -> i32 {
    match a {
        Arg::I32(v) => v,
        _ => unreachable!(),
    }
}
fn x64(a: Arg) -> i64 {
    match a {
        Arg::I64(v) => v,
        _ => unreachable!(),
    }
}
fn xf64(a: Arg) -> f64 {
    match a {
        Arg::F64(b) => f64::from_bits(b),
        _ => unreachable!(),
    }
}
fn xf32(a: Arg) -> f32 {
    match a {
        Arg::F32(b) => f32::from_bits(b),
        _ => unreachable!(),
    }
}

/// trunc(x) must lie in [lo, hi) for the conversion not to trap.
fn trunc_in(x: f64, lo: f64, hi: f64) -> Option<f64> {
    let t = x.trunc();
    if x.is_nan() || t < lo || t >= hi {
        None
    } else {
        Some(t)
    }
}

fn convert_cases(out: &mut Vec<Case>) {
    use Type::*;
    out.push(conv_case("i32_i64", I32, I64, i32s(), |a| {
        Outcome::i64(x32(a) as i64)
    }));
    out.push(conv_case("u32_i64", U32, I64, i32s(), |a| {
        Outcome::i64(x32(a) as u32 as i64)
    }));
    out.push(conv_case("i64_i32", I64, I32, i64s(), |a| {
        Outcome::i32(x64(a) as i32)
    }));
    out.push(conv_case("i32_f64", I32, F64, i32s(), |a| {
        Outcome::f64(x32(a) as f64)
    }));
    out.push(conv_case("u32_f64", U32, F64, i32s(), |a| {
        Outcome::f64(x32(a) as u32 as f64)
    }));
    out.push(conv_case("i32_f32", I32, F32, i32s(), |a| {
        Outcome::f32(x32(a) as f32)
    }));
    out.push(conv_case("u32_f32", U32, F32, i32s(), |a| {
        Outcome::f32(x32(a) as u32 as f32)
    }));
    out.push(conv_case("i64_f32", I64, F32, i64s(), |a| {
        Outcome::f32(x64(a) as f32)
    }));
    out.push(conv_case("i64_f64", I64, F64, i64s(), |a| {
        Outcome::f64(x64(a) as f64)
    }));
    out.push(conv_case("u64_f64", U64, F64, i64s(), |a| {
        Outcome::f64(x64(a) as u64 as f64)
    }));
    out.push(conv_case("f64_f32", F64, F32, f64s(), |a| {
        Outcome::f32(xf64(a) as f32)
    }));
    out.push(conv_case(
        "f32_f64",
        F32,
        F64,
        f64s()
            .into_iter()
            .map(|a| Arg::f32(xf64(a) as f32))
            .collect(),
        |a| Outcome::f64(xf32(a) as f64),
    ));
    out.push(conv_case("f64_i32", F64, I32, f64s(), |a| {
        match trunc_in(xf64(a), -2147483648.0, 2147483648.0) {
            Some(t) => Outcome::i32(t as i32),
            None => Outcome::Trap,
        }
    }));
    out.push(conv_case("f64_u32", F64, U32, f64s(), |a| {
        match trunc_in(xf64(a), 0.0, 4294967296.0) {
            Some(t) => Outcome::i32(t as u32 as i32),
            None => Outcome::Trap,
        }
    }));
    out.push(conv_case("f64_i64", F64, I64, f64s(), |a| {
        match trunc_in(xf64(a), -9.223372036854775808e18, 9.223372036854775808e18) {
            Some(t) => Outcome::i64(t as i64),
            None => Outcome::Trap,
        }
    }));
    out.push(conv_case(
        "f32_i32",
        F32,
        I32,
        f64s()
            .into_iter()
            .map(|a| Arg::f32(xf64(a) as f32))
            .collect(),
        |a| match trunc_in(xf32(a) as f64, -2147483648.0, 2147483648.0) {
            Some(t) => Outcome::i32(t as i32),
            None => Outcome::Trap,
        },
    ));
    // Narrow types exist only between a Load/Convert and a Convert/Store: the
    // round trip through each of them normalises (sign- or zero-extends).
    for (name, narrow, norm) in [
        ("i8", I8, (|x: i32| x as i8 as i32) as fn(i32) -> i32),
        ("u8", U8, |x| x as u8 as i32),
        ("i16", I16, |x| x as i16 as i32),
        ("u16", U16, |x| x as u16 as i32),
    ] {
        let m = single(func(
            &[I32],
            &[I32],
            &[],
            vec![ret(conv(narrow, I32, conv(I32, narrow, get(0))))],
        ));
        let calls = i32s()
            .into_iter()
            .map(|a| (vec![a], Outcome::i32(norm(x32(a)))))
            .collect();
        out.push(case(format!("convert/narrow_{name}"), m, true, calls));
    }
}

fn select_cases(out: &mut Vec<Case>) {
    let m = single(func(
        &[Type::I32, Type::I32, Type::I32],
        &[Type::I32],
        &[],
        vec![ret(Expr::Select {
            cond: Box::new(get(0)),
            then_val: Box::new(get(1)),
            else_val: Box::new(get(2)),
        })],
    ));
    let mut calls = Vec::new();
    for c in [0, 1, -1, 256] {
        calls.push((
            vec![Arg::I32(c), Arg::I32(11), Arg::I32(22)],
            Outcome::i32(if c != 0 { 11 } else { 22 }),
        ));
    }
    out.push(case("select/i32", m, true, calls));
    let m = single(func(
        &[Type::I32, Type::F64, Type::F64],
        &[Type::F64],
        &[],
        vec![ret(Expr::Select {
            cond: Box::new(get(0)),
            then_val: Box::new(get(1)),
            else_val: Box::new(get(2)),
        })],
    ));
    let calls = [0, 5]
        .map(|c| {
            (
                vec![Arg::I32(c), Arg::f64(1.5), Arg::f64(-2.5)],
                Outcome::f64(if c != 0 { 1.5 } else { -2.5 }),
            )
        })
        .to_vec();
    out.push(case("select/f64", m, true, calls));
}

// ───────────────────────────── control flow ─────────────────────────────

fn ints(xs: &[i32]) -> Vec<i32> {
    xs.to_vec()
}

fn one_arg(xs: &[i32], f: impl Fn(i32) -> i32) -> Vec<(Vec<Arg>, Outcome)> {
    xs.iter()
        .map(|&x| (vec![Arg::I32(x)], Outcome::i32(f(x))))
        .collect()
}

fn control_cases(out: &mut Vec<Case>) {
    use BinaryOp::*;
    // for (i = 0; i < n; i++) acc += i;
    let (n, i, acc) = (0, 1, 2);
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32, Type::I32],
        vec![
            set(i, ci(0)),
            set(acc, ci(0)),
            lbl(0),
            jif(cmp(Ge, get(i), get(n)), 1),
            set(acc, add(get(acc), get(i))),
            set(i, add(get(i), ci(1))),
            jmp(0),
            lbl(1),
            ret(get(acc)),
        ],
    ));
    out.push(case(
        "control/loop_sum",
        m,
        true,
        one_arg(&[-3, 0, 1, 2, 10, 100, 1000], |n| {
            let (mut i, mut acc) = (0i32, 0i32);
            while i < n {
                acc = acc.wrapping_add(i);
                i += 1;
            }
            acc
        }),
    ));

    // for i < n { for j < i { acc += i*j + 1 } }
    let (n, i, j, acc) = (0, 1, 2, 3);
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32, Type::I32, Type::I32],
        vec![
            set(acc, ci(0)),
            set(i, ci(0)),
            lbl(0),
            jif(cmp(Ge, get(i), get(n)), 1),
            set(j, ci(0)),
            lbl(2),
            jif(cmp(Ge, get(j), get(i)), 3),
            set(acc, add(get(acc), add(mul(get(i), get(j)), ci(1)))),
            set(j, add(get(j), ci(1))),
            jmp(2),
            lbl(3),
            set(i, add(get(i), ci(1))),
            jmp(0),
            lbl(1),
            ret(get(acc)),
        ],
    ));
    out.push(case(
        "control/nested_loops",
        m,
        true,
        one_arg(&[0, 1, 2, 5, 17, 60], |n| {
            let mut acc = 0i32;
            for i in 0..n.max(0) {
                for j in 0..i {
                    acc = acc.wrapping_add(i.wrapping_mul(j).wrapping_add(1));
                }
            }
            acc
        }),
    ));

    // if (x < 0) r = -x else r = x * 2; return r
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![
            jif(cmp(Lt, get(0), ci(0)), 1),
            set(1, mul(get(0), ci(2))),
            jmp(2),
            lbl(1),
            set(1, Expr::unary(UnaryOp::Neg, get(0), Type::I32)),
            lbl(2),
            ret(get(1)),
        ],
    ));
    out.push(case(
        "control/diamond",
        m,
        true,
        one_arg(&[0, 5, -5, i32::MIN, i32::MAX], |x| {
            if x < 0 {
                x.wrapping_neg()
            } else {
                x.wrapping_mul(2)
            }
        }),
    ));

    // forward goto: r = 1; if (x == 0) goto skip; r = r*10 + x; skip: return r + 5
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![
            set(1, ci(1)),
            jif(cmp(Eq, get(0), ci(0)), 1),
            set(1, add(mul(get(1), ci(10)), get(0))),
            lbl(1),
            ret(add(get(1), ci(5))),
        ],
    ));
    out.push(case(
        "control/goto_forward",
        m,
        true,
        one_arg(&[0, 1, 7, -3], |x| {
            if x == 0 {
                6
            } else {
                (10i32.wrapping_add(x)).wrapping_add(5)
            }
        }),
    ));

    // backward goto (Collatz): steps = 0; top: if (n == 1) goto done; n = even ? n/2 : 3n+1; steps++; goto top
    let (n, steps) = (0, 1);
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![
            set(steps, ci(0)),
            lbl(0),
            jif(cmp(Eq, get(n), ci(1)), 3),
            jif(bin(And, Type::I32, get(n), ci(1)), 1),
            set(n, bin(Shr, Type::I32, get(n), ci(1))),
            jmp(2),
            lbl(1),
            set(n, add(mul(get(n), ci(3)), ci(1))),
            lbl(2),
            set(steps, add(get(steps), ci(1))),
            jmp(0),
            lbl(3),
            ret(get(steps)),
        ],
    ));
    out.push(case(
        "control/goto_backward_collatz",
        m,
        true,
        one_arg(&[1, 2, 3, 6, 7, 27, 97], |mut n| {
            let mut s = 0;
            while n != 1 {
                n = if n & 1 != 0 { n * 3 + 1 } else { n >> 1 };
                s += 1;
            }
            s
        }),
    ));

    // BrTable with fallthrough between targets:
    //   acc = 0; br_table x [l0, l1, l2] default ld; l0: acc += 1; l1: acc += 10; l2: acc += 100; ld: acc += 1000
    let acc = 1;
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![
            set(acc, ci(0)),
            Op::BrTable {
                index: get(0),
                targets: vec![0, 1, 2],
                default: 3,
            },
            lbl(0),
            set(acc, add(get(acc), ci(1))),
            lbl(1),
            set(acc, add(get(acc), ci(10))),
            lbl(2),
            set(acc, add(get(acc), ci(100))),
            lbl(3),
            set(acc, add(get(acc), ci(1000))),
            ret(get(acc)),
        ],
    ));
    out.push(case(
        "control/br_table_fallthrough",
        m,
        true,
        one_arg(&[0, 1, 2, 3, 100, -1], |x| match x {
            0 => 1111,
            1 => 1110,
            2 => 1100,
            _ => 1000,
        }),
    ));

    // BrTable inside a loop: for i in 0..n { br_table (i % 3) ... } with each arm adding differently.
    let (n, i, acc) = (0, 1, 2);
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32, Type::I32],
        vec![
            set(i, ci(0)),
            set(acc, ci(0)),
            lbl(10),
            jif(cmp(Ge, get(i), get(n)), 11),
            Op::BrTable {
                index: bin(Rem, Type::I32, get(i), ci(3)),
                targets: vec![0, 1],
                default: 2,
            },
            lbl(0),
            set(acc, add(get(acc), ci(1))),
            jmp(3),
            lbl(1),
            set(acc, add(get(acc), ci(20))),
            jmp(3),
            lbl(2),
            set(acc, add(get(acc), ci(300))),
            lbl(3),
            set(i, add(get(i), ci(1))),
            jmp(10),
            lbl(11),
            ret(get(acc)),
        ],
    ));
    out.push(case(
        "control/br_table_in_loop",
        m,
        true,
        one_arg(&[0, 1, 2, 3, 10], |n| {
            (0..n).map(|i| [1, 20, 300][(i % 3) as usize]).sum()
        }),
    ));

    // Switch, dense and sparse (arms carry no jumps).
    let sw = |ty: Type, cases: Vec<(i32, i32)>, def: i32| {
        let c: Vec<(ConstVal, Vec<Op>)> = cases
            .into_iter()
            .map(|(v, r)| (ConstVal::I32(v), vec![set(1, ci(r))]))
            .collect();
        single(func(
            &[ty],
            &[Type::I32],
            &[Type::I32],
            vec![
                set(1, ci(99)),
                Op::Switch {
                    scrutinee: get(0),
                    cases: c,
                    default: vec![set(1, ci(def))],
                },
                ret(get(1)),
            ],
        ))
    };
    out.push(case(
        "control/switch_dense",
        sw(
            Type::I32,
            vec![(0, 10), (1, 11), (2, 12), (3, 13), (4, 14)],
            -1,
        ),
        true,
        one_arg(&[-1, 0, 1, 2, 3, 4, 5, 1000], |x| match x {
            0..=4 => 10 + x,
            _ => -1,
        }),
    ));
    out.push(case(
        "control/switch_sparse",
        sw(
            Type::I32,
            vec![(-1000, 1), (7, 2), (100000, 3), (1 << 30, 4)],
            0,
        ),
        true,
        one_arg(&[-1000, 7, 100000, 1 << 30, 0, 8, i32::MIN], |x| match x {
            -1000 => 1,
            7 => 2,
            100000 => 3,
            v if v == 1 << 30 => 4,
            _ => 0,
        }),
    ));
    // U32 dense run that spans the signed boundary.
    let vals = [0x7fff_fffeu32, 0x7fff_ffff, 0x8000_0000, 0x8000_0001];
    out.push(case(
        "control/switch_u32_boundary",
        sw(
            Type::U32,
            vals.iter()
                .enumerate()
                .map(|(i, &v)| (v as i32, i as i32 + 1))
                .collect(),
            0,
        ),
        true,
        one_arg(
            &[
                0x7fff_fffe,
                0x7fff_ffff,
                0x8000_0000u32 as i32,
                0x8000_0001u32 as i32,
                0x8000_0002u32 as i32,
                0,
            ],
            |x| {
                vals.iter()
                    .position(|&v| v as i32 == x)
                    .map_or(0, |p| p as i32 + 1)
            },
        ),
    ));

    computed_goto_cases(out);
}

fn computed_goto_cases(out: &mut Vec<Case>) {
    use BinaryOp::*;
    let sel = |c: Expr, a: Expr, b: Expr| Expr::Select {
        cond: Box::new(c),
        then_val: Box::new(a),
        else_val: Box::new(b),
    };
    // p = x == 0 ? &&A : (x == 1 ? &&B : &&C); goto *p; A: return 10; B: return 20; C: return 30
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![
            set(
                1,
                sel(
                    cmp(Eq, get(0), ci(0)),
                    Expr::LabelAddr(1),
                    sel(
                        cmp(Eq, get(0), ci(1)),
                        Expr::LabelAddr(2),
                        Expr::LabelAddr(3),
                    ),
                ),
            ),
            Op::GotoIndirect { target: get(1) },
            lbl(1),
            ret(ci(10)),
            lbl(2),
            ret(ci(20)),
            lbl(3),
            ret(ci(30)),
        ],
    ));
    out.push(case(
        "control/computed_goto_select",
        m,
        true,
        one_arg(&[0, 1, 2, -7], |x| match x {
            0 => 10,
            1 => 20,
            _ => 30,
        }),
    ));

    // A loop whose header is also reached by computed goto:
    //   i = 0; acc = 0; L: if (i >= n) goto done; acc += i; i++; p = &&L; goto *p; done: return acc
    let (n, i, acc, p) = (0, 1, 2, 3);
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32, Type::I32, Type::I32],
        vec![
            set(i, ci(0)),
            set(acc, ci(0)),
            lbl(0),
            jif(cmp(Ge, get(i), get(n)), 1),
            set(acc, add(get(acc), get(i))),
            set(i, add(get(i), ci(1))),
            set(p, Expr::LabelAddr(0)),
            Op::GotoIndirect { target: get(p) },
            lbl(1),
            ret(get(acc)),
        ],
    ));
    out.push(case(
        "control/computed_goto_loop",
        m,
        true,
        one_arg(&[-1, 0, 1, 5, 100], |n| {
            (0..n.max(0)).fold(0i32, |a, i| a.wrapping_add(i))
        }),
    ));

    // Threaded dispatch: a tiny bytecode interpreter in memory-free form.
    // ops: acc = 1; for step in 0..n { goto *table[step % 2] } where label A doubles, B adds 3.
    let (n, step, acc, p) = (0, 1, 2, 3);
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32, Type::I32, Type::I32],
        vec![
            set(step, ci(0)),
            set(acc, ci(1)),
            lbl(0),
            jif(cmp(Ge, get(step), get(n)), 3),
            set(
                p,
                sel(
                    bin(And, Type::I32, get(step), ci(1)),
                    Expr::LabelAddr(2),
                    Expr::LabelAddr(1),
                ),
            ),
            Op::GotoIndirect { target: get(p) },
            lbl(1),
            set(acc, mul(get(acc), ci(2))),
            jmp(4),
            lbl(2),
            set(acc, add(get(acc), ci(3))),
            lbl(4),
            set(step, add(get(step), ci(1))),
            jmp(0),
            lbl(3),
            ret(get(acc)),
        ],
    ));
    out.push(case(
        "control/computed_goto_dispatch",
        m,
        true,
        one_arg(&[0, 1, 2, 3, 9], |n| {
            (0..n).fold(1i32, |a, s| {
                if s & 1 != 0 {
                    a.wrapping_add(3)
                } else {
                    a.wrapping_mul(2)
                }
            })
        }),
    ));
}

// ───────────────────────────── memory ─────────────────────────────

fn memory_cases(out: &mut Vec<Case>) {
    use MemType as M;
    // Store an i32, read it back through every narrow load.
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![
            store(M::I32, ci(64), get(0), 0),
            ret(add(
                add(
                    conv(Type::I8, Type::I32, load(M::I8, ci(64), 0)),
                    mul(conv(Type::U8, Type::I32, load(M::U8, ci(64), 1)), ci(3)),
                ),
                add(
                    mul(conv(Type::I16, Type::I32, load(M::I16, ci(64), 2)), ci(5)),
                    mul(conv(Type::U16, Type::I32, load(M::U16, ci(64), 0)), ci(7)),
                ),
            )),
        ],
    ));
    let calls = i32s()
        .into_iter()
        .map(|a| {
            let b = x32(a).to_le_bytes();
            let (a8, b8) = (b[0] as i8 as i32, b[1] as i32);
            let (c16, d16) = (
                i16::from_le_bytes([b[2], b[3]]) as i32,
                u16::from_le_bytes([b[0], b[1]]) as i32,
            );
            (
                vec![a],
                Outcome::i32(
                    a8.wrapping_add(b8.wrapping_mul(3))
                        .wrapping_add(c16.wrapping_mul(5).wrapping_add(d16.wrapping_mul(7))),
                ),
            )
        })
        .collect();
    out.push(case("memory/narrow_loads", m, false, calls));

    // Narrow stores truncate: store8 at 72, store16 at 74, read the i32 at 72 (byte 73 stays 0).
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![
            store(M::U8, ci(72), get(0), 0),
            store(M::U16, ci(74), get(0), 0),
            ret(load(M::I32, ci(72), 0)),
        ],
    ));
    let calls = i32s()
        .into_iter()
        .map(|a| {
            (
                vec![a],
                Outcome::i32((x32(a) & 0xff) | ((x32(a) & 0xffff) << 16)),
            )
        })
        .collect();
    out.push(case("memory/narrow_stores", m, false, calls));

    // Float/int reinterpretation through memory.
    let m = single(func(
        &[Type::F64],
        &[Type::I64],
        &[],
        vec![
            store(M::F64, ci(128), get(0), 0),
            ret(load(M::I64, ci(128), 0)),
        ],
    ));
    let calls = f64s()
        .into_iter()
        .map(|a| (vec![a], Outcome::i64(xf64(a).to_bits() as i64)))
        .collect();
    out.push(case("memory/f64_bits", m, false, calls));
    let m = single(func(
        &[Type::F32],
        &[Type::I32],
        &[],
        vec![
            store(M::F32, ci(132), get(0), 0),
            ret(load(M::I32, ci(132), 0)),
        ],
    ));
    let calls = f64s()
        .into_iter()
        .map(|a| Arg::f32(xf64(a) as f32))
        .map(|a| (vec![a], Outcome::i32(xf32(a).to_bits() as i32)))
        .collect();
    out.push(case("memory/f32_bits", m, false, calls));
    let m = single(func(
        &[Type::I64],
        &[Type::F64],
        &[],
        vec![
            store(M::I64, ci(136), get(0), 0),
            ret(load(M::F64, ci(136), 0)),
        ],
    ));
    let calls = i64s()
        .into_iter()
        .map(|a| (vec![a], Outcome::f64(f64::from_bits(x64(a) as u64))))
        .collect();
    out.push(case("memory/i64_as_f64", m, false, calls));

    // Out-of-bounds access traps; the last in-bounds word does not.
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![
            store(M::I32, get(0), ci(77), 0),
            ret(load(M::I32, get(0), 0)),
        ],
    ));
    let calls = [0, 65532, 65533, 65536, -4, 100000]
        .map(|a| {
            (
                vec![Arg::I32(a)],
                if (a as u32 as u64) + 4 <= PAGE {
                    Outcome::i32(77)
                } else {
                    Outcome::Trap
                },
            )
        })
        .to_vec();
    out.push(case("memory/bounds", m, false, calls));

    tagged_cases(out);
}

fn tagged(tag: u64, addr: u64) -> i64 {
    ((tag << 32) | addr) as i64
}

fn tagged_cases(out: &mut Vec<Case>) {
    use BinaryOp::*;
    use MemType as M;
    let p = || get(0);
    // f(p: u64): store 0x12345678 at p+4, return load32(p+4) + load_u8(p+5)
    let m = single(func(
        &[Type::U64],
        &[Type::I32],
        &[],
        vec![
            store(M::I32, p(), ci(0x1234_5678), 4),
            ret(add(
                load(M::I32, p(), 4),
                conv(Type::U8, Type::I32, load(M::U8, p(), 5)),
            )),
        ],
    ));
    let mut calls = Vec::new();
    for tag in [0u64, 7, 0xABCD, 0xFFFF_FFFF] {
        for addr in [256u64, 65528, 65529, 65535, 65536, 0xFFFF_FFFC] {
            let ok = addr + 4 + 4 <= PAGE;
            calls.push((
                vec![Arg::I64(tagged(tag, addr))],
                if ok {
                    Outcome::i32(0x1234_5678 + 0x56)
                } else {
                    Outcome::Trap
                },
            ));
        }
    }
    out.push(case("memory/tagged_load_store", m, false, calls));

    // Array through a tagged pointer: for i < n { p[i] = i*i + 3 }; sum p[0..n]
    let (n, i, sum) = (1, 2, 3);
    let addr = |idx: Expr| {
        bin(
            Add,
            Type::U64,
            get(0),
            conv(Type::I32, Type::U64, mul(idx, ci(4))),
        )
    };
    let m = single(func(
        &[Type::U64, Type::I32],
        &[Type::I32],
        &[Type::I32, Type::I32],
        vec![
            set(i, ci(0)),
            set(sum, ci(0)),
            lbl(0),
            jif(cmp(Ge, get(i), get(n)), 1),
            store(M::I32, addr(get(i)), add(mul(get(i), get(i)), ci(3)), 0),
            set(i, add(get(i), ci(1))),
            jmp(0),
            lbl(1),
            set(i, ci(0)),
            lbl(2),
            jif(cmp(Ge, get(i), get(n)), 3),
            set(sum, add(get(sum), load(M::I32, addr(get(i)), 0))),
            set(i, add(get(i), ci(1))),
            jmp(2),
            lbl(3),
            ret(get(sum)),
        ],
    ));
    let mut calls = Vec::new();
    for tag in [0u64, 0x1111, 0xFFFF_FFFF] {
        for (a, nn) in [
            (1024u64, 0i32),
            (1024, 1),
            (1024, 16),
            (65500, 9),
            (65500, 10),
            (0, 100),
        ] {
            let want = if nn >= 1 && a + 4 * nn as u64 > PAGE {
                Outcome::Trap
            } else {
                Outcome::i32((0..nn.max(0)).fold(0i32, |s, i| s.wrapping_add(i * i + 3)))
            };
            calls.push((vec![Arg::I64(tagged(tag, a)), Arg::I32(nn)], want));
        }
    }
    out.push(case("memory/tagged_array", m, false, calls));

    // memory.fill / memory.copy through tagged pointers.
    let tg = |t: u64, a: u64| cl(Type::U64, tagged(t, a));
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![
            Op::MemoryFill {
                dest: tg(0xC0FFEE, 512),
                val: get(0),
                len: ci(4),
            },
            Op::MemoryCopy {
                dest: tg(0x1234, 600),
                src: tg(0xBEEF, 512),
                len: ci(4),
            },
            ret(load(M::I32, k(Type::Ptr, 600), 0)),
        ],
    ));
    let calls = [0, 0x5a, 0x1ff, -1]
        .map(|v| {
            (
                vec![Arg::I32(v)],
                Outcome::i32(i32::from_le_bytes([v as u8; 4])),
            )
        })
        .to_vec();
    out.push(case("memory/tagged_fill_copy", m, false, calls));
}

// ───────────────────────────── calls, globals, exceptions ─────────────────────────────

fn fib(n: i32) -> i32 {
    let (mut a, mut b) = (0i32, 1i32);
    for _ in 0..n {
        (a, b) = (b, a.wrapping_add(b));
    }
    a
}

fn call_cases(out: &mut Vec<Case>) {
    use BinaryOp::*;
    // recursive fib
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![
            jif(cmp(Lt, get(0), ci(2)), 1),
            ret(add(
                callx(0, vec![sub(get(0), ci(1))]),
                callx(0, vec![sub(get(0), ci(2))]),
            )),
            lbl(1),
            ret(get(0)),
        ],
    ));
    out.push(case(
        "call/fib_recursive",
        m,
        true,
        one_arg(&[0, 1, 2, 10, 20], fib),
    ));

    // mutual recursion
    let is_even = func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![
            jif(cmp(Eq, get(0), ci(0)), 1),
            ret(callx(1, vec![sub(get(0), ci(1))])),
            lbl(1),
            ret(ci(1)),
        ],
    );
    let is_odd = func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![
            jif(cmp(Eq, get(0), ci(0)), 1),
            ret(callx(0, vec![sub(get(0), ci(1))])),
            lbl(1),
            ret(ci(0)),
        ],
    );
    out.push(case(
        "call/mutual_recursion",
        module(vec![is_even, is_odd], 0),
        true,
        one_arg(&[0, 1, 2, 7, 100], |n| (n % 2 == 0) as i32),
    ));

    // small helpers (the shape an inliner works on): f(x) = sq(x) + sq(x + 1)
    let f = func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![ret(add(
            callx(1, vec![get(0)]),
            callx(1, vec![add(get(0), ci(1))]),
        ))],
    );
    let sq = func(
        &[Type::I32],
        &[Type::I32],
        &[],
        vec![ret(mul(get(0), get(0)))],
    );
    out.push(case(
        "call/small_helpers",
        module(vec![f, sq], 0),
        true,
        one_arg(&[0, 3, -4, 46340, i32::MAX], |x| {
            x.wrapping_mul(x)
                .wrapping_add(x.wrapping_add(1).wrapping_mul(x.wrapping_add(1)))
        }),
    ));

    // mixed argument types, reordered
    let g = func(
        &[Type::I32, Type::I64, Type::F64],
        &[Type::F64],
        &[],
        vec![ret(bin(
            Add,
            Type::F64,
            conv(Type::I32, Type::F64, get(0)),
            bin(Mul, Type::F64, conv(Type::I64, Type::F64, get(1)), get(2)),
        ))],
    );
    let f = func(
        &[Type::F64, Type::I64, Type::I32],
        &[Type::F64],
        &[],
        vec![ret(callx(1, vec![get(2), get(1), get(0)]))],
    );
    let mut calls = Vec::new();
    for (c, b, a) in [(0.5, 3i64, 7i32), (-2.25, i64::MAX, -1), (1e10, -5, 0)] {
        calls.push((
            vec![Arg::f64(c), Arg::I64(b), Arg::I32(a)],
            Outcome::f64(a as f64 + b as f64 * c),
        ));
    }
    out.push(case("call/mixed_args", module(vec![f, g], 0), true, calls));

    // call_indirect: table = [sub, mul, seven]; slot 3 null; size 4.
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType {
        min: 1,
        max: Some(1),
        shared: false,
    });
    let sig = mb.add_type(FunctionType {
        params: vec![Type::I32, Type::I32],
        results: vec![Type::I32],
    });
    mb.add_table(TableType {
        min: 4,
        max: Some(4),
    });
    for op in [Sub, Mul] {
        mb.add_function(func(
            &[Type::I32, Type::I32],
            &[Type::I32],
            &[],
            vec![ret(bin(op, Type::I32, get(0), get(1)))],
        ));
    }
    mb.add_function(func(&[], &[Type::I32], &[], vec![ret(ci(7))]));
    mb.add_function(func(
        &[Type::I32, Type::I32, Type::I32],
        &[Type::I32],
        &[],
        vec![ret(Expr::CallIndirect {
            type_idx: sig,
            table_idx: 0,
            callee: Box::new(get(0)),
            args: vec![get(1), get(2)],
        })],
    ));
    mb.add_element(ElementSegment {
        mode: SegmentMode::Active {
            index: 0,
            offset: InitExpr::i32(0),
        },
        funcs: vec![0, 1, 2],
    });
    mb.export_function("f", 3);
    let calls = (0..6)
        .map(|i| {
            (
                vec![Arg::I32(i), Arg::I32(9), Arg::I32(4)],
                match i {
                    0 => Outcome::i32(5),
                    1 => Outcome::i32(36),
                    _ => Outcome::Trap, // wrong signature, null slot, out of table bounds
                },
            )
        })
        .collect();
    out.push(case("call/call_indirect", mb.build(), true, calls));

    // Statement calls and a mutable global: inc() n times.
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType {
        min: 1,
        max: Some(1),
        shared: false,
    });
    let g = mb.add_i32_global(100);
    mb.add_function(func(
        &[],
        &[],
        &[],
        vec![
            Op::GlobalSet {
                idx: g,
                val: add(Expr::GlobalGet(g), ci(1)),
            },
            ret_void(),
        ],
    ));
    mb.add_function(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![
            set(1, ci(0)),
            lbl(0),
            jif(cmp(Ge, get(1), get(0)), 1),
            call(0, vec![]),
            set(1, add(get(1), ci(1))),
            jmp(0),
            lbl(1),
            ret(Expr::GlobalGet(g)),
        ],
    ));
    mb.export_function("f", 1);
    out.push(case(
        "call/global_counter",
        mb.build(),
        false,
        one_arg(&[0, 1, 5, 50], |n| 100 + n),
    ));
}

fn exception_cases(out: &mut Vec<Case>) {
    use BinaryOp::*;
    // try { if (x > 5) throw tag0(x); r = 1 } catch tag0(v) { r = v * 2 }; return r
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType {
        min: 1,
        max: Some(1),
        shared: false,
    });
    mb.add_tag(Tag {
        params: vec![Type::I32],
    });
    mb.add_function(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32, Type::I32],
        vec![
            Op::TryTable {
                body: vec![Op::Switch {
                    scrutinee: cmp(Gt, get(0), ci(5)),
                    cases: vec![(
                        ConstVal::I32(1),
                        vec![Op::Throw {
                            tag_idx: 0,
                            args: vec![get(0)],
                        }],
                    )],
                    default: vec![set(1, ci(1))],
                }],
                catches: vec![Catch {
                    tag: Some(0),
                    bind: vec![2],
                    body: vec![set(1, mul(get(2), ci(2)))],
                }],
            },
            ret(get(1)),
        ],
    ));
    mb.export_function("f", 0);
    out.push(case(
        "exception/try_catch",
        mb.build(),
        true,
        one_arg(&[0, 5, 6, 100], |x| if x > 5 { x * 2 } else { 1 }),
    ));
}

// ───────────────────────────── setjmp / longjmp ─────────────────────────────

fn setjmp_cases(out: &mut Vec<Case>) {
    use BinaryOp::*;
    let sj = |buf: i32, dest: u32| Op::Setjmp { buf: ci(buf), dest };
    let longjmper = || {
        func(
            &[Type::I32, Type::I32],
            &[],
            &[],
            vec![Op::Longjmp {
                buf: get(0),
                val: get(1),
            }],
        )
    };
    let lj_val = |v: i32| if v == 0 { 1 } else { v };

    // r = setjmp(b); if (r) return r; longjmp(b, 7);
    let m = single(func(
        &[],
        &[Type::I32],
        &[Type::I32],
        vec![
            sj(16, 0),
            jif(get(0), 1),
            Op::Longjmp {
                buf: ci(16),
                val: ci(7),
            },
            lbl(1),
            ret(get(0)),
        ],
    ));
    out.push(case(
        "setjmp/single_level",
        m,
        false,
        vec![(vec![], Outcome::i32(7))],
    ));

    // never longjmp'd: setjmp returns 0 once
    let m = single(func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![sj(16, 1), ret(add(get(1), get(0)))],
    ));
    out.push(case(
        "setjmp/direct_return",
        m,
        false,
        one_arg(&[0, 9], |x| x),
    ));

    // main(v): r = setjmp(b); if (r) return r + 100; a(v)   a(v): b2(v)   b2(v): longjmp(b, v)
    let main = func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![
            sj(16, 1),
            jif(get(1), 1),
            call(1, vec![get(0)]),
            ret(ci(0)),
            lbl(1),
            ret(add(get(1), ci(100))),
        ],
    );
    let a = func(
        &[Type::I32],
        &[],
        &[],
        vec![call(2, vec![get(0)]), ret_void()],
    );
    let b = func(
        &[Type::I32],
        &[],
        &[],
        vec![Op::Longjmp {
            buf: ci(16),
            val: get(0),
        }],
    );
    out.push(case(
        "setjmp/two_levels",
        module(vec![main, a, b], 0),
        false,
        one_arg(&[0, 5, -3, i32::MAX], |v| lj_val(v).wrapping_add(100)),
    ));

    // two sites: r1 = setjmp(b1); if (r1) return r1*100 + r2; r2 = setjmp(b2); if (r2) g(b1, 11); g(b2, 22)
    let main = func(
        &[],
        &[Type::I32],
        &[Type::I32, Type::I32],
        vec![
            sj(16, 0),
            jif(get(0), 1),
            sj(32, 1),
            jif(get(1), 2),
            call(1, vec![ci(32), ci(22)]),
            Op::Unreachable,
            lbl(2),
            call(1, vec![ci(16), ci(11)]),
            Op::Unreachable,
            lbl(1),
            ret(add(mul(get(0), ci(100)), get(1))),
        ],
    );
    out.push(case(
        "setjmp/two_sites",
        module(vec![main, longjmper()], 0),
        false,
        vec![(vec![], Outcome::i32(1122))],
    ));

    // setjmp in a loop: for (i = 0; i < n; ) { r = setjmp(b); sum += r; i++; if (!r) g(b, 10) }
    let (n, i, r, sum) = (0, 1, 2, 3);
    let main = func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32, Type::I32, Type::I32],
        vec![
            set(i, ci(0)),
            set(sum, ci(0)),
            lbl(0),
            jif(cmp(Ge, get(i), get(n)), 9),
            sj(16, r),
            set(sum, add(get(sum), get(r))),
            set(i, add(get(i), ci(1))),
            jif(get(r), 0),
            call(1, vec![ci(16), ci(10)]),
            jmp(0),
            lbl(9),
            ret(get(sum)),
        ],
    );
    out.push(case(
        "setjmp/in_loop",
        module(vec![main, longjmper()], 0),
        false,
        one_arg(&[0, 1, 2, 3, 6], |n| {
            // The C program, run natively: a longjmp resumes after setjmp with r = 10.
            let (mut i, mut sum) = (0, 0);
            while i < n {
                let mut r = 0;
                loop {
                    sum += r;
                    i += 1;
                    if r != 0 {
                        break;
                    }
                    r = 10; // longjmp(b, 10)
                }
            }
            sum
        }),
    ));

    // A longjmp to main's buffer passes through inner, which has a setjmp of its own.
    let main = func(
        &[Type::I32],
        &[Type::I32],
        &[Type::I32],
        vec![
            sj(16, 1),
            jif(get(1), 1),
            call(1, vec![get(0)]),
            ret(ci(0)),
            lbl(1),
            ret(get(1)),
        ],
    );
    let inner = func(
        &[Type::I32],
        &[],
        &[Type::I32],
        vec![
            sj(32, 1),
            jif(get(1), 1),
            call(2, vec![ci(16), get(0)]),
            ret_void(),
            lbl(1),
            Op::Unreachable,
        ],
    );
    out.push(case(
        "setjmp/foreign_label_rethrown",
        module(vec![main, inner, longjmper()], 0),
        false,
        one_arg(&[0, 9, -2], lj_val),
    ));

    // longjmp through a jmp_buf nobody filled: uncaught, traps.
    let m = single(func(
        &[],
        &[Type::I32],
        &[],
        vec![Op::Longjmp {
            buf: ci(16),
            val: ci(1),
        }],
    ));
    out.push(case(
        "setjmp/unset_buf_traps",
        m,
        false,
        vec![(vec![], Outcome::Trap)],
    ));
}

/// Every case, in a stable order.
pub fn all() -> Vec<Case> {
    let mut v = Vec::new();
    int_binop_cases(&mut v);
    int_unary_cases(&mut v);
    float_cases(&mut v);
    convert_cases(&mut v);
    select_cases(&mut v);
    control_cases(&mut v);
    memory_cases(&mut v);
    call_cases(&mut v);
    exception_cases(&mut v);
    setjmp_cases(&mut v);
    let _ = ints;
    v
}

//! N5 tests. Every test that checks behaviour encodes a module, passes the
//! bytes to WNJR `Module::new` (parse + validate), instantiates it on the JIT
//! and asserts the value an exported function returns.

use super::*;
use crate::collections::{vec, Vec};
use crate::njvm::encode::{encode, EncodeError};
use crate::njvm::module::{FunctionType, Global, GlobalType, InitExpr, MemoryType, TableType, Tag};
use crate::njvm::{ElementSegment, ModuleBuilder, SegmentMode};
use crate::{Config, Engine, RuntimeWorld, Value};
use alloc::boxed::Box;

// ── helpers ─────────────────────────────────────────────────────────────────

fn c(ty: Type, v: i32) -> Expr {
    Expr::Const { ty, val: ConstVal::I32(v) }
}
fn c64(ty: Type, v: i64) -> Expr {
    Expr::Const { ty, val: ConstVal::I64(v) }
}
fn cf32(v: f32) -> Expr {
    Expr::Const { ty: Type::F32, val: ConstVal::f32(v) }
}
fn cf64(v: f64) -> Expr {
    Expr::Const { ty: Type::F64, val: ConstVal::f64(v) }
}
fn bin(op: BinaryOp, ty: Type, a: Expr, b: Expr) -> Expr {
    Expr::binary(op, a, b, ty)
}
fn un(op: UnaryOp, ty: Type, a: Expr) -> Expr {
    Expr::unary(op, a, ty)
}
fn conv(from: Type, to: Type, v: Expr) -> Expr {
    Expr::Convert { from, to, val: Box::new(v) }
}
fn ret(e: Expr) -> Op {
    Op::Return { vals: vec![e] }
}

/// Encode `m`, validate it with WNJR, instantiate, and call export `name`.
fn run_module(m: &Module, name: &str, args: &[Value]) -> Vec<Value> {
    let bytes = encode(m).expect("encode");
    let module = crate::Module::new("n5", &bytes).expect("WNJR parse + validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, module, &[]).expect("instantiate");
    let out = world.invoke(id, name, args).expect("invoke");
    world.free(id).expect("free");
    out
}

/// A module with one exported function `f` (and a 1-page memory).
fn one_fn(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>, body: Vec<Op>) -> Module {
    let mut mb = ModuleBuilder::new();
    mb.add_memory(MemoryType { min: 1, max: Some(1), shared: false });
    let mut f = Function::new(params, results, locals);
    f.body = body;
    mb.add_function(f);
    mb.export_function("f", 0);
    mb.build()
}

/// `fn f() -> ty { return e }`, run, return the single result.
fn eval(ty: Type, e: Expr) -> Value {
    let m = one_fn(vec![], vec![ty], vec![], vec![ret(e)]);
    let out = run_module(&m, "f", &[]);
    assert_eq!(out.len(), 1);
    out[0]
}

fn i32v(v: Value) -> i32 {
    match v { Value::I32(x) => x, o => panic!("expected i32, got {o:?}") }
}
fn i64v(v: Value) -> i64 {
    match v { Value::I64(x) => x, o => panic!("expected i64, got {o:?}") }
}
fn f32v(v: Value) -> f32 {
    match v { Value::F32(x) => x, o => panic!("expected f32, got {o:?}") }
}
fn f64v(v: Value) -> f64 {
    match v { Value::F64(x) => x, o => panic!("expected f64, got {o:?}") }
}

/// Lower without encoding the module (for error tests).
fn lower(m: &Module) -> Result<Lowered, EraseError> {
    let env = Env::new(m);
    lower_function(&env, &m.functions[0])
}

// ── Const, LocalGet, the entry label ────────────────────────────────────────

#[test]
fn const_every_wide_type() {
    assert_eq!(i32v(eval(Type::I32, c(Type::I32, -7))), -7);
    assert_eq!(i32v(eval(Type::U32, c(Type::U32, -1))), -1);
    assert_eq!(i32v(eval(Type::Ptr, c(Type::Ptr, 4096))), 4096);
    assert_eq!(i64v(eval(Type::I64, c64(Type::I64, -1 << 40))), -1 << 40);
    assert_eq!(i64v(eval(Type::U64, c64(Type::U64, 1 << 62))), 1 << 62);
    assert_eq!(f32v(eval(Type::F32, cf32(1.5))), 1.5);
    assert_eq!(f64v(eval(Type::F64, cf64(-2.25))), -2.25);
}

#[test]
fn entry_label_without_jumps_lowers() {
    // The builder's usual shape: an entry Label, then straight-line code.
    let m = one_fn(
        vec![Type::I32, Type::I32],
        vec![Type::I32],
        vec![],
        vec![
            Op::Label { id: 0, params: vec![] },
            ret(bin(BinaryOp::Add, Type::I32, Expr::LocalGet(0), Expr::LocalGet(1))),
        ],
    );
    let out = run_module(&m, "f", &[Value::I32(40), Value::I32(2)]);
    assert_eq!(out, vec![Value::I32(42)]);
}

// ── Binary: every op × every legal wide type ────────────────────────────────

/// Evaluate `op` on (a, b) at type `ty` given as i64 bit patterns /
/// floats, and return the result as i64 bits or f64.
fn int_bin(op: BinaryOp, ty: Type, a: i64, b: i64) -> i64 {
    let (ea, eb) = match class(ty) {
        ValType::I32 => (c(ty, a as i32), c(ty, b as i32)),
        _ => (c64(ty, a), c64(ty, b)),
    };
    let cmp = matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne);
    let rt = if cmp { Type::I32 } else { ty };
    match eval(rt, bin(op, ty, ea, eb)) {
        Value::I32(x) => x as i64,
        Value::I64(x) => x,
        o => panic!("{o:?}"),
    }
}

fn float_bin(op: BinaryOp, ty: Type, a: f64, b: f64) -> f64 {
    let (ea, eb) = if ty == Type::F32 { (cf32(a as f32), cf32(b as f32)) } else { (cf64(a), cf64(b)) };
    let cmp = matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne);
    let rt = if cmp { Type::I32 } else { ty };
    match eval(rt, bin(op, ty, ea, eb)) {
        Value::I32(x) => x as f64,
        Value::F32(x) => x as f64,
        Value::F64(x) => x,
        o => panic!("{o:?}"),
    }
}

#[test]
fn binary_int_ops_all_wide_int_types() {
    use BinaryOp::*;
    // Operands chosen so every op gives a value that also shows signedness
    // where it matters: a = -8 (0xFFFF_FFF8 / 0xFFFF_FFFF_FFFF_FFF8), b = 3.
    for ty in [Type::I32, Type::U32, Type::Ptr, Type::I64, Type::U64] {
        let w64 = class(ty) == ValType::I64;
        let u = is_unsigned(ty);
        let (a, b) = (-8i64, 3i64);
        // Reference results computed in Rust at the right width/signedness.
        let r = |op: BinaryOp| -> i64 {
            if w64 {
                let (x, y) = (a, b);
                let (ux, uy) = (x as u64, y as u64);
                match op {
                    Add => x.wrapping_add(y), Sub => x.wrapping_sub(y), Mul => x.wrapping_mul(y),
                    Div => if u { (ux / uy) as i64 } else { x / y },
                    Rem => if u { (ux % uy) as i64 } else { x % y },
                    And => x & y, Or => x | y, Xor => x ^ y,
                    Shl => x << y, Shr => if u { (ux >> uy) as i64 } else { x >> y },
                    Rotl => ux.rotate_left(y as u32) as i64, Rotr => ux.rotate_right(y as u32) as i64,
                    Lt => (if u { ux < uy } else { x < y }) as i64,
                    Le => (if u { ux <= uy } else { x <= y }) as i64,
                    Gt => (if u { ux > uy } else { x > y }) as i64,
                    Ge => (if u { ux >= uy } else { x >= y }) as i64,
                    Eq => (x == y) as i64, Ne => (x != y) as i64,
                    _ => unreachable!(),
                }
            } else {
                let (x, y) = (a as i32, b as i32);
                let (ux, uy) = (x as u32, y as u32);
                let cmp = |v: bool| v as i64;
                match op {
                    Add => x.wrapping_add(y) as i64, Sub => x.wrapping_sub(y) as i64, Mul => x.wrapping_mul(y) as i64,
                    Div => if u { (ux / uy) as i32 as i64 } else { (x / y) as i64 },
                    Rem => if u { (ux % uy) as i32 as i64 } else { (x % y) as i64 },
                    And => (x & y) as i64, Or => (x | y) as i64, Xor => (x ^ y) as i64,
                    Shl => (x << y) as i64, Shr => if u { (ux >> uy) as i32 as i64 } else { (x >> y) as i64 },
                    Rotl => ux.rotate_left(uy) as i32 as i64, Rotr => ux.rotate_right(uy) as i32 as i64,
                    Lt => cmp(if u { ux < uy } else { x < y }),
                    Le => cmp(if u { ux <= uy } else { x <= y }),
                    Gt => cmp(if u { ux > uy } else { x > y }),
                    Ge => cmp(if u { ux >= uy } else { x >= y }),
                    Eq => cmp(x == y), Ne => cmp(x != y),
                    _ => unreachable!(),
                }
            }
        };
        for op in [Add, Sub, Mul, Div, Rem, And, Or, Xor, Shl, Shr, Rotl, Rotr, Lt, Le, Gt, Ge, Eq, Ne] {
            assert_eq!(int_bin(op, ty, a, b), r(op), "{op:?} at {ty:?}");
        }
    }
}

#[test]
fn binary_float_ops_f32_f64() {
    use BinaryOp::*;
    for ty in [Type::F32, Type::F64] {
        let (a, b) = (-6.0f64, 4.0f64);
        let r = |op: BinaryOp| -> f64 {
            match op {
                Add => a + b, Sub => a - b, Mul => a * b, Div => a / b,
                Min => a.min(b), Max => a.max(b), Copysign => a.copysign(b),
                Lt => (a < b) as i32 as f64, Le => (a <= b) as i32 as f64,
                Gt => (a > b) as i32 as f64, Ge => (a >= b) as i32 as f64,
                Eq => (a == b) as i32 as f64, Ne => (a != b) as i32 as f64,
                _ => unreachable!(),
            }
        };
        for op in [Add, Sub, Mul, Div, Min, Max, Copysign, Lt, Le, Gt, Ge, Eq, Ne] {
            assert_eq!(float_bin(op, ty, a, b), r(op), "{op:?} at {ty:?}");
        }
    }
}

/// -1 / 2: I32 gives 0 (signed), U32 gives 0x7FFF_FFFF (unsigned). One case per
/// sign-selected op, so each selection is checked on its own.
#[test]
fn signed_unsigned_select_distinct_results() {
    use BinaryOp::*;
    // 32-bit
    assert_eq!(int_bin(Div, Type::I32, -1, 2), 0);
    assert_eq!(int_bin(Div, Type::U32, -1, 2), 0x7FFF_FFFF);
    assert_eq!(int_bin(Rem, Type::I32, -7, 2), -1);
    assert_eq!(int_bin(Rem, Type::U32, -7, 2), 1);
    assert_eq!(int_bin(Shr, Type::I32, -16, 2), -4);
    assert_eq!(int_bin(Shr, Type::U32, -16, 2), 0x3FFF_FFFC);
    assert_eq!(int_bin(Lt, Type::I32, -1, 0), 1);
    assert_eq!(int_bin(Lt, Type::U32, -1, 0), 0);
    assert_eq!(int_bin(Le, Type::I32, -1, 0), 1);
    assert_eq!(int_bin(Le, Type::U32, -1, 0), 0);
    assert_eq!(int_bin(Gt, Type::I32, -1, 0), 0);
    assert_eq!(int_bin(Gt, Type::U32, -1, 0), 1);
    assert_eq!(int_bin(Ge, Type::I32, -1, 0), 0);
    assert_eq!(int_bin(Ge, Type::U32, -1, 0), 1);
    // Ptr is unsigned: an address above 2 GiB compares above a low one.
    assert_eq!(int_bin(Gt, Type::Ptr, -0x1000, 0x1000), 1);
    // 64-bit
    assert_eq!(int_bin(Div, Type::I64, -1, 2), 0);
    assert_eq!(int_bin(Div, Type::U64, -1, 2), i64::MAX);
    assert_eq!(int_bin(Rem, Type::I64, -7, 2), -1);
    assert_eq!(int_bin(Rem, Type::U64, -7, 2), 1);
    assert_eq!(int_bin(Shr, Type::I64, -16, 2), -4);
    assert_eq!(int_bin(Shr, Type::U64, -16, 2), (u64::MAX - 15) as i64 >> 2 & (u64::MAX >> 2) as i64);
    assert_eq!(int_bin(Lt, Type::I64, -1, 0), 1);
    assert_eq!(int_bin(Lt, Type::U64, -1, 0), 0);
    assert_eq!(int_bin(Le, Type::I64, -1, 0), 1);
    assert_eq!(int_bin(Le, Type::U64, -1, 0), 0);
    assert_eq!(int_bin(Gt, Type::I64, -1, 0), 0);
    assert_eq!(int_bin(Gt, Type::U64, -1, 0), 1);
    assert_eq!(int_bin(Ge, Type::I64, -1, 0), 0);
    assert_eq!(int_bin(Ge, Type::U64, -1, 0), 1);
}

// ── Unary ───────────────────────────────────────────────────────────────────

#[test]
fn unary_int_ops() {
    use UnaryOp::*;
    // 32-bit
    assert_eq!(i32v(eval(Type::I32, un(Neg, Type::I32, c(Type::I32, 5)))), -5);
    assert_eq!(i32v(eval(Type::I32, un(Not, Type::I32, c(Type::I32, 0x0F)))), !0x0F);
    assert_eq!(i32v(eval(Type::I32, un(Abs, Type::I32, c(Type::I32, -9)))), 9);
    assert_eq!(i32v(eval(Type::I32, un(Abs, Type::I32, c(Type::I32, 9)))), 9);
    assert_eq!(i32v(eval(Type::I32, un(Abs, Type::I32, c(Type::I32, i32::MIN)))), i32::MIN);
    assert_eq!(i32v(eval(Type::U32, un(Abs, Type::U32, c(Type::U32, -9)))), -9, "unsigned abs is identity");
    assert_eq!(i32v(eval(Type::I32, un(Clz, Type::I32, c(Type::I32, 1)))), 31);
    assert_eq!(i32v(eval(Type::I32, un(Ctz, Type::I32, c(Type::I32, 8)))), 3);
    assert_eq!(i32v(eval(Type::U32, un(Popcnt, Type::U32, c(Type::U32, 0xFF)))), 8);
    assert_eq!(i32v(eval(Type::Ptr, un(Not, Type::Ptr, c(Type::Ptr, 0)))), -1);
    // 64-bit
    assert_eq!(i64v(eval(Type::I64, un(Neg, Type::I64, c64(Type::I64, 5)))), -5);
    assert_eq!(i64v(eval(Type::U64, un(Not, Type::U64, c64(Type::U64, 0)))), -1);
    assert_eq!(i64v(eval(Type::I64, un(Abs, Type::I64, c64(Type::I64, -1 << 40)))), 1 << 40);
    assert_eq!(i64v(eval(Type::U64, un(Abs, Type::U64, c64(Type::U64, -3)))), -3);
    assert_eq!(i64v(eval(Type::I64, un(Clz, Type::I64, c64(Type::I64, 1)))), 63);
    assert_eq!(i64v(eval(Type::I64, un(Ctz, Type::I64, c64(Type::I64, 1 << 33)))), 33);
    assert_eq!(i64v(eval(Type::U64, un(Popcnt, Type::U64, c64(Type::U64, -1)))), 64);
}

#[test]
fn unary_abs_twice_shares_scratch_and_keeps_params() {
    // abs(a) + abs(b): two signed Abs in one function; the scratch local comes
    // after the param and must not clobber it.
    let e = bin(
        BinaryOp::Add,
        Type::I32,
        un(UnaryOp::Abs, Type::I32, Expr::LocalGet(0)),
        un(UnaryOp::Abs, Type::I32, Expr::LocalGet(1)),
    );
    let m = one_fn(vec![Type::I32, Type::I32], vec![Type::I32], vec![Type::I64], vec![ret(e)]);
    let lowered = lower(&m).unwrap();
    assert_eq!(lowered.locals, vec![ValType::I64, ValType::I32], "declared local, then one i32 scratch");
    assert_eq!(run_module(&m, "f", &[Value::I32(-3), Value::I32(-4)]), vec![Value::I32(7)]);
}

#[test]
fn unary_float_ops() {
    use UnaryOp::*;
    let f = |op, v: f64| f32v(eval(Type::F32, un(op, Type::F32, cf32(v as f32))));
    let d = |op, v: f64| f64v(eval(Type::F64, un(op, Type::F64, cf64(v))));
    assert_eq!(f(Neg, 2.5), -2.5);
    assert_eq!(f(Abs, -2.5), 2.5);
    assert_eq!(f(Sqrt, 16.0), 4.0);
    assert_eq!(f(Ceil, 1.2), 2.0);
    assert_eq!(f(Floor, 1.8), 1.0);
    assert_eq!(f(Trunc, -1.8), -1.0);
    assert_eq!(f(Nearest, 2.5), 2.0);
    assert_eq!(d(Neg, 2.5), -2.5);
    assert_eq!(d(Abs, -2.5), 2.5);
    assert_eq!(d(Sqrt, 2.25), 1.5);
    assert_eq!(d(Ceil, -1.2), -1.0);
    assert_eq!(d(Floor, -1.2), -2.0);
    assert_eq!(d(Trunc, 1.8), 1.0);
    assert_eq!(d(Nearest, 3.5), 4.0);
}

// ── Convert: every (from, to) pair of the 11 types ──────────────────────────

const ALL: [Type; 11] = [
    Type::I8, Type::U8, Type::I16, Type::U16, Type::I32, Type::U32,
    Type::I64, Type::U64, Type::F32, Type::F64, Type::Ptr,
];

/// The source value for a Convert test, as an expression of type `from`.
/// Narrow sources are produced by an inner Convert from I32 (the only way to
/// make one besides Load). The value -3 (or -3.75 for floats) exposes sign
/// and truncation behaviour.
fn src(from: Type) -> Expr {
    match from {
        Type::I8 | Type::U8 | Type::I16 | Type::U16 => conv(Type::I32, from, c(Type::I32, -3)),
        Type::I32 | Type::U32 | Type::Ptr => c(from, -3),
        Type::I64 | Type::U64 => c64(from, -3),
        Type::F32 => cf32(-3.75),
        Type::F64 => cf64(-3.75),
    }
}

/// Reference for `(to)(from)(-3)` in C, as the result's wasm value.
/// A float source that is negative is out of range for unsigned targets
/// (UB in C, a trap in wasm), so the matrix uses +3.75 for those.
fn reference(from: Type, to: Type) -> Value {
    // The integer the source holds, after its own normalization.
    let src_int: i64 = match from {
        Type::I8 => -3i8 as i64,
        Type::U8 => (-3i32 as u8) as i64,
        Type::I16 => -3i16 as i64,
        Type::U16 => (-3i32 as u16) as i64,
        Type::I32 => -3,
        Type::U32 | Type::Ptr => (-3i32 as u32) as i64,
        Type::I64 => -3,
        Type::U64 => -3, // bit pattern; handled below for floats
        _ => 0,
    };
    let fsrc: f64 = if is_unsigned(to) { 3.75 } else { -3.75 };
    match to {
        Type::F32 => Value::F32(match from {
            Type::F32 | Type::F64 => fsrc_for(from, -3.75) as f32,
            Type::U64 => (u64::MAX - 2) as f32,
            _ => src_int as f32,
        }),
        Type::F64 => Value::F64(match from {
            Type::F32 | Type::F64 => fsrc_for(from, -3.75),
            Type::U64 => (u64::MAX - 2) as f64,
            _ => src_int as f64,
        }),
        Type::I64 | Type::U64 => Value::I64(match from {
            Type::F32 | Type::F64 => if is_unsigned(to) { fsrc as u64 as i64 } else { fsrc as i64 },
            _ => src_int,
        }),
        _ => {
            let v: i64 = match from {
                Type::F32 | Type::F64 => if is_unsigned(to) && !is_narrow(to) { fsrc as u32 as i64 } else { fsrc as i64 },
                _ => src_int,
            };
            Value::I32(match to {
                Type::I8 => v as i8 as i32,
                Type::U8 => v as u8 as i32,
                Type::I16 => v as i16 as i32,
                Type::U16 => v as u16 as i32,
                _ => v as i32,
            })
        }
    }
}

fn fsrc_for(from: Type, v: f64) -> f64 {
    if from == Type::F32 { v as f32 as f64 } else { v }
}

/// The source for a float -> unsigned conversion must be in range.
fn src_for(from: Type, to: Type) -> Expr {
    if is_float(from) && is_unsigned(to) {
        if from == Type::F32 { cf32(3.75) } else { cf64(3.75) }
    } else {
        src(from)
    }
}

#[test]
fn convert_full_matrix() {
    let mut checked = 0;
    for from in ALL {
        for to in ALL {
            // A narrow result is returned through Convert to I32 (narrow values
            // cannot be returned); the outer Convert is the identity on an
            // already normalized i32, so it shows the inner one's bits.
            let inner = conv(from, to, src_for(from, to));
            let (e, rt) = if is_narrow(to) { (conv(to, Type::I32, inner), Type::I32) } else { (inner, to) };
            let got = eval(rt, e);
            let want = reference(from, to);
            // Compare as bits so -0.0/NaN cannot hide a difference.
            let bits = |v: Value| match v {
                Value::I32(x) => x as i64,
                Value::I64(x) => x,
                Value::F32(x) => x.to_bits() as i64,
                Value::F64(x) => x.to_bits() as i64,
                o => panic!("{o:?}"),
            };
            assert_eq!(bits(got), bits(want), "Convert {from:?} -> {to:?}: got {got:?}, want {want:?}");
            checked += 1;
        }
    }
    assert_eq!(checked, 121);
}

#[test]
fn convert_signedness_distinct_results() {
    // Widening: I32 -1 -> I64 is -1, U32 -1 -> I64 is 0xFFFF_FFFF.
    assert_eq!(i64v(eval(Type::I64, conv(Type::I32, Type::I64, c(Type::I32, -1)))), -1);
    assert_eq!(i64v(eval(Type::I64, conv(Type::U32, Type::I64, c(Type::U32, -1)))), 0xFFFF_FFFF);
    // Int -> float: U32 -1 is 4294967295.0.
    assert_eq!(f64v(eval(Type::F64, conv(Type::I32, Type::F64, c(Type::I32, -1)))), -1.0);
    assert_eq!(f64v(eval(Type::F64, conv(Type::U32, Type::F64, c(Type::U32, -1)))), 4294967295.0);
    // Float -> int: 3e9 fits U32 but not I32 (I32 would trap).
    assert_eq!(i32v(eval(Type::U32, conv(Type::F64, Type::U32, cf64(3e9)))) as u32, 3_000_000_000);
    // Narrowing: 0x1FF -> I8 is -1, -> U8 is 255.
    assert_eq!(i32v(eval(Type::I32, conv(Type::I8, Type::I32, conv(Type::I32, Type::I8, c(Type::I32, 0x1FF))))), -1);
    assert_eq!(i32v(eval(Type::I32, conv(Type::U8, Type::I32, conv(Type::I32, Type::U8, c(Type::I32, 0x1FF))))), 255);
}

// ── Select ──────────────────────────────────────────────────────────────────

#[test]
fn select_i32_and_i64_condition_and_float_values() {
    let sel = |cond: Expr, t: Expr, e: Expr| Expr::Select { cond: Box::new(cond), then_val: Box::new(t), else_val: Box::new(e) };
    assert_eq!(i32v(eval(Type::I32, sel(c(Type::I32, 1), c(Type::I32, 10), c(Type::I32, 20)))), 10);
    assert_eq!(i32v(eval(Type::I32, sel(c(Type::I32, 0), c(Type::I32, 10), c(Type::I32, 20)))), 20);
    // An i64 condition whose low 32 bits are zero must still be true.
    assert_eq!(i32v(eval(Type::I32, sel(c64(Type::I64, 1 << 32), c(Type::I32, 10), c(Type::I32, 20)))), 10);
    assert_eq!(f64v(eval(Type::F64, sel(c(Type::I32, 0), cf64(1.0), cf64(2.0)))), 2.0);
    assert_eq!(i64v(eval(Type::U64, sel(c(Type::I32, 7), c64(Type::U64, 3), c64(Type::U64, 4)))), 3);
}

// ── Load / Store: every MemType ─────────────────────────────────────────────

/// Store `val` with `st` at address 16, then load it back with `ld` and
/// return it (narrow loads go through Convert to I32).
fn store_load(st: MemType, val: Expr, ld: MemType) -> Value {
    let load = Expr::Load { ty: ld, ptr: Box::new(c(Type::Ptr, 8)), offset: 8, align: 0 };
    let (e, rt) = match ld {
        MemType::I8 => (conv(Type::I8, Type::I32, load), Type::I32),
        MemType::U8 => (conv(Type::U8, Type::I32, load), Type::I32),
        MemType::I16 => (conv(Type::I16, Type::I32, load), Type::I32),
        MemType::U16 => (conv(Type::U16, Type::I32, load), Type::I32),
        MemType::I32 => (load, Type::I32),
        MemType::I64 => (load, Type::I64),
        MemType::F32 => (load, Type::F32),
        MemType::F64 => (load, Type::F64),
    };
    let body = vec![
        Op::Store { ty: st, ptr: c(Type::Ptr, 16), val, offset: 0, align: 0 },
        ret(e),
    ];
    let m = one_fn(vec![], vec![rt], vec![], body);
    run_module(&m, "f", &[])[0]
}

#[test]
fn load_store_every_memtype() {
    // 0xFF80 stored as 16 bits: load8_s = -128, load8_u = 128,
    // load16_s = -128, load16_u = 0xFF80.
    let v = c(Type::I32, 0xFF80);
    assert_eq!(i32v(store_load(MemType::I16, v.clone(), MemType::I8)), -128);
    assert_eq!(i32v(store_load(MemType::I16, v.clone(), MemType::U8)), 128);
    assert_eq!(i32v(store_load(MemType::I16, v.clone(), MemType::I16)), -128);
    assert_eq!(i32v(store_load(MemType::U16, v.clone(), MemType::U16)), 0xFF80);
    // store8 truncates: 0x1234 -> 0x34.
    assert_eq!(i32v(store_load(MemType::I8, c(Type::I32, 0x1234), MemType::U8)), 0x34);
    assert_eq!(i32v(store_load(MemType::U8, c(Type::I32, 0x1281), MemType::I8)), -127);
    assert_eq!(i32v(store_load(MemType::I32, c(Type::I32, -5), MemType::I32)), -5);
    assert_eq!(i64v(store_load(MemType::I64, c64(Type::I64, -5 << 40), MemType::I64)), -5 << 40);
    assert_eq!(f32v(store_load(MemType::F32, cf32(1.25), MemType::F32)), 1.25);
    assert_eq!(f64v(store_load(MemType::F64, cf64(-1.25), MemType::F64)), -1.25);
}

#[test]
fn narrow_load_stores_back_without_convert() {
    // *(u8*)17 = *(i8*)16: a narrow value goes straight into a store.
    let body = vec![
        Op::Store { ty: MemType::I8, ptr: c(Type::Ptr, 16), val: c(Type::I32, 0xF0), offset: 0, align: 0 },
        Op::Store {
            ty: MemType::U8,
            ptr: c(Type::Ptr, 17),
            val: Expr::Load { ty: MemType::I8, ptr: Box::new(c(Type::Ptr, 16)), offset: 0, align: 0 },
            offset: 0,
            align: 0,
        },
        ret(conv(Type::U8, Type::I32, Expr::Load { ty: MemType::U8, ptr: Box::new(c(Type::Ptr, 17)), offset: 0, align: 0 })),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![], body);
    assert_eq!(run_module(&m, "f", &[]), vec![Value::I32(0xF0)]);
}

// ── locals, globals, tee ────────────────────────────────────────────────────

#[test]
fn local_set_get_tee_and_global() {
    let mut mb = ModuleBuilder::new();
    mb.add_global(Global { ty: GlobalType { ty: Type::I64, mutable: true }, init: InitExpr::Const(ConstVal::I64(100)) });
    let mut f = Function::new(vec![Type::I32], vec![Type::I64], vec![Type::I32]);
    f.body = vec![
        // l1 = p0 * 2
        Op::LocalSet { idx: 1, val: bin(BinaryOp::Mul, Type::I32, Expr::LocalGet(0), c(Type::I32, 2)) },
        // g0 = g0 + (i64)(tee l1 = l1 + 1)
        Op::GlobalSet {
            idx: 0,
            val: bin(
                BinaryOp::Add,
                Type::I64,
                Expr::GlobalGet(0),
                conv(Type::I32, Type::I64, Expr::LocalTee {
                    idx: 1,
                    val: Box::new(bin(BinaryOp::Add, Type::I32, Expr::LocalGet(1), c(Type::I32, 1))),
                }),
            ),
        },
        // return g0 + (i64)l1
        ret(bin(BinaryOp::Add, Type::I64, Expr::GlobalGet(0), conv(Type::I32, Type::I64, Expr::LocalGet(1)))),
    ];
    mb.add_function(f);
    mb.export_function("f", 0);
    let m = mb.build();
    // p0 = 5: l1 = 10, tee l1 = 11, g0 = 111, result 111 + 11 = 122.
    assert_eq!(run_module(&m, "f", &[Value::I32(5)]), vec![Value::I64(122)]);
}

// ── calls ───────────────────────────────────────────────────────────────────

#[test]
fn call_expr_call_stmt_call_indirect() {
    let mut mb = ModuleBuilder::new();
    let sig = mb.add_type(FunctionType { params: vec![Type::I32, Type::I32], results: vec![Type::I32] });
    mb.add_table(TableType { min: 1, max: Some(1) });
    mb.add_i32_global(0);
    // f0: sub(a, b) = a - b
    let mut sub = Function::new(vec![Type::I32, Type::I32], vec![Type::I32], vec![]);
    sub.body = vec![ret(bin(BinaryOp::Sub, Type::I32, Expr::LocalGet(0), Expr::LocalGet(1)))];
    mb.add_function(sub);
    // f1: bump(): g0 += 1, returns nothing
    let mut bump = Function::new(vec![], vec![], vec![]);
    bump.body = vec![Op::GlobalSet { idx: 0, val: bin(BinaryOp::Add, Type::I32, Expr::GlobalGet(0), c(Type::I32, 1)) }];
    mb.add_function(bump);
    // f2: main() = sub(10, 3) * 100 + call_indirect[0](50, 8) + g0, after calling bump
    // twice and sub once as a statement (result dropped).
    let mut main = Function::new(vec![], vec![Type::I32], vec![]);
    main.body = vec![
        Op::Call { func_idx: 1, args: vec![] },
        Op::Call { func_idx: 1, args: vec![] },
        Op::Call { func_idx: 0, args: vec![c(Type::I32, 1), c(Type::I32, 2)] },
        Op::CallIndirect { type_idx: sig, table_idx: 0, callee: c(Type::I32, 0), args: vec![c(Type::I32, 1), c(Type::I32, 1)] },
        ret(bin(
            BinaryOp::Add,
            Type::I32,
            bin(
                BinaryOp::Add,
                Type::I32,
                bin(BinaryOp::Mul, Type::I32, Expr::Call { func_idx: 0, args: vec![c(Type::I32, 10), c(Type::I32, 3)] }, c(Type::I32, 100)),
                Expr::CallIndirect { type_idx: sig, table_idx: 0, callee: Box::new(c(Type::I32, 0)), args: vec![c(Type::I32, 50), c(Type::I32, 8)] },
            ),
            Expr::GlobalGet(0),
        )),
    ];
    mb.add_function(main);
    mb.add_element(ElementSegment { mode: SegmentMode::Active { index: 0, offset: InitExpr::i32(0) }, funcs: vec![0] });
    mb.export_function("main", 2);
    let m = mb.build();
    // 7*100 + 42 + 2
    assert_eq!(run_module(&m, "main", &[]), vec![Value::I32(744)]);
}

// ── other statements ────────────────────────────────────────────────────────

#[test]
fn memory_fill_copy_drop_nop() {
    let body = vec![
        Op::Nop,
        Op::MemoryFill { dest: c(Type::Ptr, 32), val: c(Type::I32, 0xAB), len: c(Type::I32, 4) },
        Op::MemoryCopy { dest: c(Type::Ptr, 64), src: c(Type::Ptr, 32), len: c(Type::I32, 4) },
        Op::Drop { val: Expr::Load { ty: MemType::U8, ptr: Box::new(c(Type::Ptr, 0)), offset: 0, align: 0 } },
        Op::MemorySize,
        ret(Expr::Load { ty: MemType::I32, ptr: Box::new(c(Type::Ptr, 64)), offset: 0, align: 2 }),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![], body);
    assert_eq!(run_module(&m, "f", &[]), vec![Value::I32(0xABABABABu32 as i32)]);
}

#[test]
fn falls_off_end_traps_and_unreachable_traps() {
    // A body with results but no Return gets a trailing unreachable: it
    // validates, and calling it traps.
    let m = one_fn(vec![], vec![Type::I32], vec![], vec![Op::Nop]);
    let bytes = encode(&m).unwrap();
    let module = crate::Module::new("n5", &bytes).expect("validates");
    let engine = Engine::new(Config::new()).unwrap();
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, module, &[]).unwrap();
    assert!(world.invoke(id, "f", &[]).is_err(), "falling off the end must trap");

    let m = one_fn(vec![], vec![], vec![], vec![Op::Unreachable]);
    let bytes = encode(&m).unwrap();
    let module = crate::Module::new("n5", &bytes).expect("validates");
    let id = world.instantiate(&engine, module, &[]).unwrap();
    assert!(world.invoke(id, "f", &[]).is_err(), "unreachable must trap");
}

#[test]
fn throw_is_lowered() {
    let mut m = one_fn(vec![], vec![], vec![], vec![Op::Throw { tag_idx: 0, args: vec![c(Type::I32, 9)] }]);
    m.tags.push(Tag { params: vec![Type::I32] });
    let bytes = encode(&m).unwrap();
    let module = crate::Module::new("n5", &bytes).expect("validates");
    let engine = Engine::new(Config::new()).unwrap();
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, module, &[]).unwrap();
    assert!(world.invoke(id, "f", &[]).is_err(), "uncaught throw surfaces as an error");
}

// ── errors ──────────────────────────────────────────────────────────────────

#[test]
fn label_with_jump_needs_stackify() {
    let body = vec![
        Op::Label { id: 0, params: vec![] },
        Op::JmpIf { cond: c(Type::I32, 0), target: 0 },
        ret(c(Type::I32, 1)),
    ];
    let m = one_fn(vec![], vec![Type::I32], vec![], body);
    assert_eq!(lower(&m).unwrap_err().kind, EraseErrorKind::NeedsStackify);
    match encode(&m) {
        Err(EncodeError::Body { index: 0, error }) => assert_eq!(error.kind, EraseErrorKind::NeedsStackify),
        other => panic!("{other:?}"),
    }
    // A Jmp nested in a structured body is found too.
    let m = one_fn(vec![], vec![], vec![], vec![
        Op::Label { id: 0, params: vec![] },
        Op::Block { body: vec![Op::Jmp { target: 0 }] },
    ]);
    assert_eq!(lower(&m).unwrap_err().kind, EraseErrorKind::NeedsStackify);
    // And BrTable.
    let m = one_fn(vec![], vec![], vec![], vec![
        Op::Label { id: 0, params: vec![] },
        Op::BrTable { index: c(Type::I32, 0), targets: vec![0], default: 0 },
    ]);
    assert_eq!(lower(&m).unwrap_err().kind, EraseErrorKind::NeedsStackify);
}

#[test]
fn verifier_rejects_illegal_ops_before_n5() {
    use crate::njvm::{verify_module, OpName, VerifyErrorKind};
    let m = one_fn(vec![], vec![Type::I32], vec![], vec![ret(un(UnaryOp::Sqrt, Type::I32, c(Type::I32, 4)))]);
    let err = verify_module(&m).unwrap_err();
    assert_eq!(err.kind, VerifyErrorKind::IllegalOp { op: OpName::Unary(UnaryOp::Sqrt), ty: Type::I32 });
    let m = one_fn(vec![], vec![Type::F64], vec![], vec![ret(bin(BinaryOp::Rem, Type::F64, cf64(1.0), cf64(2.0)))]);
    assert!(matches!(verify_module(&m).unwrap_err().kind, VerifyErrorKind::IllegalOp { .. }));
}

/// N5 does not panic on input the verifier would reject.
#[test]
fn illegal_input_returns_errors_not_panics() {
    let cases: Vec<(Vec<Type>, Vec<Op>, EraseErrorKind)> = vec![
        (vec![Type::I32], vec![ret(un(UnaryOp::Sqrt, Type::I32, c(Type::I32, 4)))],
            EraseErrorKind::IllegalUnary { op: UnaryOp::Sqrt, ty: Type::I32 }),
        (vec![Type::F32], vec![ret(un(UnaryOp::Clz, Type::F32, cf32(1.0)))],
            EraseErrorKind::IllegalUnary { op: UnaryOp::Clz, ty: Type::F32 }),
        (vec![Type::F64], vec![ret(bin(BinaryOp::Rem, Type::F64, cf64(1.0), cf64(1.0)))],
            EraseErrorKind::IllegalBinary { op: BinaryOp::Rem, ty: Type::F64 }),
        (vec![Type::I32], vec![ret(bin(BinaryOp::Min, Type::I32, c(Type::I32, 1), c(Type::I32, 2)))],
            EraseErrorKind::IllegalBinary { op: BinaryOp::Min, ty: Type::I32 }),
        (vec![Type::I64], vec![ret(bin(BinaryOp::Copysign, Type::I64, c64(Type::I64, 1), c64(Type::I64, 2)))],
            EraseErrorKind::IllegalBinary { op: BinaryOp::Copysign, ty: Type::I64 }),
        // narrow types outside Load/Store/Convert
        (vec![Type::I32], vec![ret(bin(BinaryOp::Add, Type::I8, c(Type::I8, 1), c(Type::I8, 2)))],
            EraseErrorKind::NarrowValue { ty: Type::I8 }),
        (vec![], vec![Op::Drop { val: c(Type::U16, 1) }], EraseErrorKind::NarrowValue { ty: Type::U16 }),
        // operand type mismatch
        (vec![Type::I32], vec![ret(bin(BinaryOp::Add, Type::I32, c(Type::I32, 1), cf32(2.0)))],
            EraseErrorKind::TypeMismatch { expected: Type::I32, got: Type::F32 }),
        (vec![Type::I32], vec![ret(c64(Type::I64, 1))],
            EraseErrorKind::TypeMismatch { expected: Type::I32, got: Type::I64 }),
        (vec![Type::I32], vec![ret(Expr::Const { ty: Type::I32, val: ConstVal::I64(1) })],
            EraseErrorKind::ConstMismatch { ty: Type::I32, val: ConstVal::I64(1) }),
        (vec![Type::I32], vec![ret(Expr::LocalGet(9))], EraseErrorKind::LocalOutOfRange { idx: 9 }),
        (vec![Type::I32], vec![ret(Expr::GlobalGet(0))], EraseErrorKind::GlobalOutOfRange { idx: 0 }),
        (vec![Type::I32], vec![ret(Expr::Call { func_idx: 5, args: vec![] })], EraseErrorKind::FuncOutOfRange { idx: 5 }),
        (vec![], vec![Op::Store { ty: MemType::I32, ptr: cf32(0.0), val: c(Type::I32, 0), offset: 0, align: 0 }],
            EraseErrorKind::ClassMismatch { expected: ValType::I32, got: Type::F32 }),
        (vec![], vec![Op::Store { ty: MemType::F64, ptr: c(Type::Ptr, 0), val: c(Type::I32, 0), offset: 0, align: 0 }],
            EraseErrorKind::ClassMismatch { expected: ValType::F64, got: Type::I32 }),
        (vec![], vec![Op::Return { vals: vec![c(Type::I32, 1)] }], EraseErrorKind::ArityMismatch { expected: 0, got: 1 }),
        (vec![Type::I32], vec![ret(Expr::LabelAddr(0))], EraseErrorKind::UnloweredComputedGoto),
        (vec![], vec![Op::GotoIndirect { target: c(Type::I32, 0) }], EraseErrorKind::UnloweredComputedGoto),
        (vec![], vec![Op::If { cond: c(Type::I32, 1), then_body: vec![], else_body: vec![] }],
            EraseErrorKind::Unsupported { op: "If" }),
        (vec![], vec![Op::Const { ty: Type::I32, val: ConstVal::I32(0) }], EraseErrorKind::Unsupported { op: "Const" }),
    ];
    for (results, body, want) in cases {
        let m = one_fn(vec![], results, vec![], body.clone());
        let got = lower(&m).map(|_| ()).unwrap_err();
        assert_eq!(got.kind, want, "body {body:?}");
        assert_eq!(got.at, Some(0));
    }
    // A narrow local.
    let m = one_fn(vec![], vec![], vec![Type::U8], vec![]);
    assert_eq!(lower(&m).map(|_| ()).unwrap_err().kind, EraseErrorKind::NarrowLocal { idx: 0, ty: Type::U8 });
    // Calling a void function in expression position.
    let mut mb = ModuleBuilder::new();
    mb.add_function(Function::new(vec![], vec![], vec![]));
    let mut f = Function::new(vec![], vec![], vec![]);
    f.body = vec![Op::Drop { val: Expr::Call { func_idx: 0, args: vec![] } }];
    mb.add_function(f);
    let m = mb.build();
    let env = Env::new(&m);
    assert_eq!(lower_function(&env, &m.functions[1]).map(|_| ()).unwrap_err().kind, EraseErrorKind::CallResultArity { results: 0 });
}

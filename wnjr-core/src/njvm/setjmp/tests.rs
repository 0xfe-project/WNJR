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
    let out = calls.iter().map(|a| world.invoke(id, "f", a).map_err(|e| { std::eprintln!("trap: {e:?}"); })).collect();
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

#[test]
fn dbg_dump_across() {
    let main = func(vec![], vec![Type::I32], vec![Type::I32], vec![
        setjmp(16, 0), Op::JmpIf { cond: get(0), target: 1 }, call(1, vec![]), ret(c(0)), label(1), ret(add(get(0), c(100))),
    ]);
    let b = func(vec![], vec![], vec![], vec![Op::Longjmp { buf: c(16), val: c(5) }]);
    let l = lower_module(&module(vec![main, b])).unwrap();
    let bytes = encode(&l).unwrap();
    std::fs::write("/tmp/n9_across.wasm", &bytes).unwrap();
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.unwrap() {
            std::eprintln!("--- func");
            let mut r = body.get_operators_reader().unwrap();
            while !r.eof() { std::eprintln!("{:?}", r.read().unwrap()); }
        }
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

#[test]
fn eh_two_callee() {
    let src = r#"(module
      (tag $t (param i32 i32))
      (func $thrower (throw $t (i32.const 7) (i32.const 9)))
      (func (export "catcher") (result i32)
        (local $a i32) (local $b i32)
        (block $exit
          (block $h (result i32 i32)
            (try_table (catch $t $h)
              (call $thrower))
            (return (i32.const 999)))
          local.set $b local.set $a
          local.get $a local.get $b i32.add return)
        unreachable))
    "#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("minimal", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "catcher", &[]);
    world.free(id).unwrap();
    std::eprintln!("eh_two_callee: {r:?}");
    assert!(r.is_ok(), "eh_two_callee trapped");
}

#[test]
fn eh_two_inline() {
    let src = r#"(module
      (tag $t (param i32 i32))
      (func $thrower (throw $t (i32.const 7) (i32.const 9)))
      (func (export "catcher") (result i32)
        (local $a i32) (local $b i32)
        (block $exit
          (block $h (result i32 i32)
            (try_table (catch $t $h)
              (throw $t (i32.const 7) (i32.const 9)))
            (return (i32.const 999)))
          local.set $b local.set $a
          local.get $a local.get $b i32.add return)
        unreachable))
    "#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("minimal", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "catcher", &[]);
    world.free(id).unwrap();
    std::eprintln!("eh_two_inline: {r:?}");
    assert!(r.is_ok(), "eh_two_inline trapped");
}

#[test]
fn eh_one_callee() {
    let src = r#"(module
      (tag $t (param i32))
      (func $thrower (throw $t (i32.const 7)))
      (func (export "catcher") (result i32)
        (local $a i32) (local $b i32)
        (block $exit
          (block $h (result i32)
            (try_table (catch $t $h)
              (call $thrower))
            (return (i32.const 999)))
          local.set $a
          local.get $a return)
        unreachable))
    "#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("minimal", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "catcher", &[]);
    world.free(id).unwrap();
    std::eprintln!("eh_one_callee: {r:?}");
    assert!(r.is_ok(), "eh_one_callee trapped");
}

#[test]
fn eh_one_inline() {
    let src = r#"(module
      (tag $t (param i32))
      (func $thrower (throw $t (i32.const 7)))
      (func (export "catcher") (result i32)
        (local $a i32) (local $b i32)
        (block $exit
          (block $h (result i32)
            (try_table (catch $t $h)
              (throw $t (i32.const 7)))
            (return (i32.const 999)))
          local.set $a
          local.get $a return)
        unreachable))
    "#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("minimal", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "catcher", &[]);
    world.free(id).unwrap();
    std::eprintln!("eh_one_inline: {r:?}");
    assert!(r.is_ok(), "eh_one_inline trapped");
}

#[test]
fn eh_zero_callee() {
    let src = r#"(module
      (tag $t )
      (func $thrower (throw $t ))
      (func (export "catcher") (result i32)
        (local $a i32) (local $b i32)
        (block $exit
          (block $h 
            (try_table (catch $t $h)
              (call $thrower))
            (return (i32.const 999)))
          
          i32.const 5 return)
        unreachable))
    "#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("minimal", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "catcher", &[]);
    world.free(id).unwrap();
    std::eprintln!("eh_zero_callee: {r:?}");
    assert!(r.is_ok(), "eh_zero_callee trapped");
}

#[test]
fn eh_zero_inline() {
    let src = r#"(module
      (tag $t )
      (func $thrower (throw $t ))
      (func (export "catcher") (result i32)
        (local $a i32) (local $b i32)
        (block $exit
          (block $h 
            (try_table (catch $t $h)
              (throw $t ))
            (return (i32.const 999)))
          
          i32.const 5 return)
        unreachable))
    "#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("minimal", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "catcher", &[]);
    world.free(id).unwrap();
    std::eprintln!("eh_zero_inline: {r:?}");
    assert!(r.is_ok(), "eh_zero_inline trapped");
}

#[test]
fn bis_full() {
    let src = r#"(module
    (type (func (result i32 i32)))
    (memory 1 1)
    (tag $t (param i32 i32))
    (func (export "f") (result i32) (local i32 i32 i32 i32)
      i32.const 0 local.set 1
      loop
      block
        block (type 0)
          try_table (catch $t 0)
            
          block block block block
            local.get 1
            br_table 2 1 0
          end
          unreachable
          end
          local.get 3 local.set 0 i32.const 0 local.set 1 br 1
          end
          i32.const 16 i32.const 1 i32.store
          i32.const 0 local.set 0
          br 0
          end
          local.get 0
          if
            local.get 0 i32.const 100 i32.add return
          else
            call 1 i32.const 0 return
          end
          end
          br 1
        end
        local.set 3 local.set 2
        local.get 2 i32.const 1 i32.sub i32.const 1 i32.ge_u
        if
          local.get 2 local.get 3 throw $t
        else
          local.get 2 i32.const 0 i32.sub local.set 1 br 1
        end
        br 0
      end
      br 0
      end
      unreachable)
    (func (local i32)
      i32.const 16 i32.load
      i32.const 5 local.tee 0 local.get 0 i32.const 0 i32.eq i32.add
      throw $t)
  )"#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bis_full: {r:?}");
}

#[test]
fn bis_noloop() {
    let src = r#"(module
    (type (func (result i32 i32)))
    (memory 1 1)
    (tag $t (param i32 i32))
    (func (export "f") (result i32) (local i32 i32 i32 i32)
      i32.const 0 local.set 1
      
      block
        block (type 0)
          try_table (catch $t 0)
            
          block block block block
            local.get 1
            br_table 2 1 0
          end
          unreachable
          end
          local.get 3 local.set 0 i32.const 0 local.set 1 br 1
          end
          i32.const 16 i32.const 1 i32.store
          i32.const 0 local.set 0
          br 0
          end
          local.get 0
          if
            local.get 0 i32.const 100 i32.add return
          else
            call 1 i32.const 0 return
          end
          end
          br 1
        end
        local.set 3 local.set 2
        local.get 2 i32.const 1 i32.sub i32.const 1 i32.ge_u
        if
          local.get 2 local.get 3 throw $t
        else
          local.get 2 i32.const 0 i32.sub local.set 1 local.get 3 i32.const 100 i32.add return
        end
        br 0
      end
      
      
      unreachable)
    (func (local i32)
      i32.const 16 i32.load
      i32.const 5 local.tee 0 local.get 0 i32.const 0 i32.eq i32.add
      throw $t)
  )"#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bis_noloop: {r:?}");
}

#[test]
fn bis_nodispatch() {
    let src = r#"(module
    (type (func (result i32 i32)))
    (memory 1 1)
    (tag $t (param i32 i32))
    (func (export "f") (result i32) (local i32 i32 i32 i32)
      i32.const 0 local.set 1
      loop
      block
        block (type 0)
          try_table (catch $t 0)
            
          i32.const 16 i32.const 1 i32.store
          i32.const 0 local.set 0
          local.get 0
          if
            local.get 0 i32.const 100 i32.add return
          else
            call 1 i32.const 0 return
          end
          end
          br 1
        end
        local.set 3 local.set 2
        local.get 2 i32.const 1 i32.sub i32.const 1 i32.ge_u
        if
          local.get 2 local.get 3 throw $t
        else
          local.get 2 i32.const 0 i32.sub local.set 1 br 1
        end
        br 0
      end
      br 0
      end
      unreachable)
    (func (local i32)
      i32.const 16 i32.load
      i32.const 5 local.tee 0 local.get 0 i32.const 0 i32.eq i32.add
      throw $t)
  )"#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bis_nodispatch: {r:?}");
}

#[test]
fn bis_neither() {
    let src = r#"(module
    (type (func (result i32 i32)))
    (memory 1 1)
    (tag $t (param i32 i32))
    (func (export "f") (result i32) (local i32 i32 i32 i32)
      i32.const 0 local.set 1
      
      block
        block (type 0)
          try_table (catch $t 0)
            
          i32.const 16 i32.const 1 i32.store
          i32.const 0 local.set 0
          local.get 0
          if
            local.get 0 i32.const 100 i32.add return
          else
            call 1 i32.const 0 return
          end
          end
          br 1
        end
        local.set 3 local.set 2
        local.get 2 i32.const 1 i32.sub i32.const 1 i32.ge_u
        if
          local.get 2 local.get 3 throw $t
        else
          local.get 2 i32.const 0 i32.sub local.set 1 local.get 3 i32.const 100 i32.add return
        end
        br 0
      end
      
      
      unreachable)
    (func (local i32)
      i32.const 16 i32.load
      i32.const 5 local.tee 0 local.get 0 i32.const 0 i32.eq i32.add
      throw $t)
  )"#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bis_neither: {r:?}");
}

#[test]
fn bis_norethrow() {
    // Remove the rethrow: always br to return instead
    let src = r#"(module
      (memory 1 1) (tag $t (param i32 i32))
      (func (export "f") (result i32) (local i32 i32 i32 i32)
        i32.const 0 local.set 1
        block
          block (result i32 i32)
            try_table (catch $t 0)
              call 1 i32.const 999 return
            end
            br 1
          end
          local.set 3 local.set 2
          local.get 3 i32.const 100 i32.add return
        end
        unreachable)
      (func (local i32)
        i32.const 16 i32.load
        i32.const 5 local.tee 0 local.get 0 i32.const 0 i32.eq i32.add
        throw $t)
    )"#;
    let bytes = wat::parse_str(src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bis_norethrow: {r:?}");
}

#[test]
fn bx_plain_after() {
    let thrower = r#"(func $thr i32.const 1 i32.const 5 throw $t)"#;
    let catcher = r#"(func (export "f") (result i32) (local i32 i32 i32 i32)
        block
          block (result i32 i32)
            try_table (catch $t 0)
              call $thr i32.const 999 return
            end
            br 1
          end
          local.set 3 local.set 2
          local.get 3 i32.const 100 i32.add return
        end
        unreachable)"#;
    let src = std::format!("(module (memory 1 1) (tag $t (param i32 i32)) {} {})", catcher, thrower);
    let bytes = wat::parse_str(&src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bx_plain_after: {r:?}");
}

#[test]
fn bx_plain_before() {
    let thrower = r#"(func $thr i32.const 1 i32.const 5 throw $t)"#;
    let catcher = r#"(func (export "f") (result i32) (local i32 i32 i32 i32)
        block
          block (result i32 i32)
            try_table (catch $t 0)
              call $thr i32.const 999 return
            end
            br 1
          end
          local.set 3 local.set 2
          local.get 3 i32.const 100 i32.add return
        end
        unreachable)"#;
    let src = std::format!("(module (memory 1 1) (tag $t (param i32 i32)) {} {})", thrower, catcher);
    let bytes = wat::parse_str(&src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bx_plain_before: {r:?}");
}

#[test]
fn bx_local_after() {
    let thrower = r#"(func $thr (local i32) i32.const 1 i32.const 5 throw $t)"#;
    let catcher = r#"(func (export "f") (result i32) (local i32 i32 i32 i32)
        block
          block (result i32 i32)
            try_table (catch $t 0)
              call $thr i32.const 999 return
            end
            br 1
          end
          local.set 3 local.set 2
          local.get 3 i32.const 100 i32.add return
        end
        unreachable)"#;
    let src = std::format!("(module (memory 1 1) (tag $t (param i32 i32)) {} {})", catcher, thrower);
    let bytes = wat::parse_str(&src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bx_local_after: {r:?}");
}

#[test]
fn bx_load_after() {
    let thrower = r#"(func $thr i32.const 16 i32.load i32.const 5 throw $t)"#;
    let catcher = r#"(func (export "f") (result i32) (local i32 i32 i32 i32)
        block
          block (result i32 i32)
            try_table (catch $t 0)
              call $thr i32.const 999 return
            end
            br 1
          end
          local.set 3 local.set 2
          local.get 3 i32.const 100 i32.add return
        end
        unreachable)"#;
    let src = std::format!("(module (memory 1 1) (tag $t (param i32 i32)) {} {})", catcher, thrower);
    let bytes = wat::parse_str(&src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bx_load_after: {r:?}");
}

#[test]
fn bx_full_after() {
    let thrower = r#"(func $thr (local i32) i32.const 16 i32.load i32.const 5 local.tee 0 local.get 0 i32.const 0 i32.eq i32.add throw $t)"#;
    let catcher = r#"(func (export "f") (result i32) (local i32 i32 i32 i32)
        block
          block (result i32 i32)
            try_table (catch $t 0)
              call $thr i32.const 999 return
            end
            br 1
          end
          local.set 3 local.set 2
          local.get 3 i32.const 100 i32.add return
        end
        unreachable)"#;
    let src = std::format!("(module (memory 1 1) (tag $t (param i32 i32)) {} {})", catcher, thrower);
    let bytes = wat::parse_str(&src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bx_full_after: {r:?}");
}

#[test]
fn bx_full_before() {
    let thrower = r#"(func $thr (local i32) i32.const 16 i32.load i32.const 5 local.tee 0 local.get 0 i32.const 0 i32.eq i32.add throw $t)"#;
    let catcher = r#"(func (export "f") (result i32) (local i32 i32 i32 i32)
        block
          block (result i32 i32)
            try_table (catch $t 0)
              call $thr i32.const 999 return
            end
            br 1
          end
          local.set 3 local.set 2
          local.get 3 i32.const 100 i32.add return
        end
        unreachable)"#;
    let src = std::format!("(module (memory 1 1) (tag $t (param i32 i32)) {} {})", thrower, catcher);
    let bytes = wat::parse_str(&src).expect("parse wat");
    let m = crate::Module::new("bis", &bytes).expect("validate");
    let engine = Engine::new(Config::new()).expect("engine");
    let mut world = RuntimeWorld::new();
    let id = world.instantiate(&engine, m, &[]).expect("inst");
    let r = world.invoke(id, "f", &[]);
    world.free(id).unwrap();
    std::eprintln!("bx_full_before: {r:?}");
}

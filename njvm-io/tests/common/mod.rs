//! Shared NJVM-IR fixtures for the io crate's tests.
//!
//! These mirror the good inputs the N3 verifier accepts: N1's `add`, the
//! two-label counting loop, two nested flat loops, the lowered computed-goto
//! dispatch shape, and N2's comprehensive module (imports, globals, data, tag,
//! table, element, exports). Every one must verify.

#![allow(dead_code)]

use wnjr_core::njvm::{
    BinaryOp, ConstVal, DataSegment, ElementSegment, Expr, Function, FunctionBuilder, FunctionType,
    InitExpr, LabelId, Module, ModuleBuilder, Op, SegmentMode, TableType, Tag, Type,
};

fn c32(v: i32) -> Expr {
    Expr::Const {
        ty: Type::I32,
        val: ConstVal::I32(v),
    }
}

fn label(id: LabelId) -> Op {
    Op::Label {
        id,
        params: Vec::new(),
    }
}

fn ret() -> Op {
    Op::Return { vals: Vec::new() }
}

fn func(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>, body: Vec<Op>) -> Function {
    Function {
        params,
        results,
        locals,
        body,
    }
}

fn one(f: Function) -> Module {
    let mut mb = ModuleBuilder::new();
    mb.add_function(f);
    mb.build()
}

/// `fn add(a: i32, b: i32) -> i32 { return a + b; }`
pub fn add() -> Module {
    let mut fb = FunctionBuilder::new(vec![Type::I32, Type::I32], vec![Type::I32], vec![]);
    let entry = fb.next_label();
    fb.label(entry, vec![]);
    fb.push_op(Op::Return {
        vals: vec![Expr::binary(
            BinaryOp::Add,
            Expr::LocalGet(0),
            Expr::LocalGet(1),
            Type::I32,
        )],
    });
    one(fb.build())
}

/// Two-label counting loop: for (i = 0; i < 10; i++) {}
pub fn two_label_loop() -> Module {
    let mut fb = FunctionBuilder::new(vec![], vec![], vec![Type::I32]);
    fb.push_op(Op::LocalSet {
        idx: 0,
        val: c32(0),
    });
    let head = fb.next_label();
    let exit = fb.next_label();
    fb.label(head, vec![]);
    fb.jmp_if(
        Expr::binary(BinaryOp::Ge, Expr::LocalGet(0), c32(10), Type::I32),
        exit,
    );
    fb.push_op(Op::LocalSet {
        idx: 0,
        val: Expr::binary(BinaryOp::Add, Expr::LocalGet(0), c32(1), Type::I32),
    });
    fb.jmp(head);
    fb.label(exit, vec![]);
    fb.push_op(ret());
    one(fb.build())
}

fn inc(idx: u32) -> Op {
    Op::LocalSet {
        idx,
        val: Expr::binary(BinaryOp::Add, Expr::LocalGet(idx), c32(1), Type::I32),
    }
}

fn ge10(idx: u32) -> Expr {
    Expr::binary(BinaryOp::Ge, Expr::LocalGet(idx), c32(10), Type::I32)
}

/// Two nested flat loops: for i { for j {} }.
pub fn nested_loop() -> Module {
    let (oh, ox, ih, ix) = (0, 1, 2, 3);
    let body = vec![
        Op::LocalSet {
            idx: 0,
            val: c32(0),
        },
        label(oh),
        Op::JmpIf {
            cond: ge10(0),
            target: ox,
        },
        Op::LocalSet {
            idx: 1,
            val: c32(0),
        },
        label(ih),
        Op::JmpIf {
            cond: ge10(1),
            target: ix,
        },
        inc(1),
        Op::Jmp { target: ih },
        label(ix),
        inc(0),
        Op::Jmp { target: oh },
        label(ox),
        ret(),
    ];
    one(func(vec![], vec![], vec![Type::I32, Type::I32], body))
}

/// A computed-goto dispatch block: `BrTable(sel - 1) [L1, L2] default T`.
/// This is the shape the computed-goto lowering produces.
pub fn computed_goto_dispatch() -> Module {
    const D: LabelId = 10;
    const T: LabelId = 11;
    const L1: LabelId = 1;
    const L2: LabelId = 2;
    const SEL: u32 = 1;
    let set_sel = |n: i32| Op::LocalSet {
        idx: SEL,
        val: c32(n),
    };
    let dispatch = || {
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
    };
    let mut body = vec![
        set_sel(2),
        Op::JmpIf {
            cond: Expr::LocalGet(0),
            target: D,
        },
        set_sel(1),
        Op::Jmp { target: D },
        label(L1),
        set_sel(2),
        Op::Jmp { target: D },
    ];
    body.extend(dispatch());
    body.extend([
        label(L2),
        set_sel(1),
        Op::JmpIf {
            cond: Expr::LocalGet(0),
            target: D,
        },
        ret(),
    ]);
    one(func(vec![Type::I32], vec![], vec![Type::I32], body))
}

/// N2's comprehensive module: an imported function, an imported shared memory,
/// stack_pointer and tls_base globals, a passive data segment, a tag, a table,
/// two defined functions, an active element segment, and two exports.
pub fn comprehensive() -> Module {
    let mut mb = ModuleBuilder::new();
    let type_idx = mb.add_type(FunctionType {
        params: vec![Type::I32],
        results: vec![Type::I32],
    });
    mb.import_function("env", "host_add", type_idx);
    mb.import_memory("env", "memory", 1, Some(16));
    mb.add_stack_pointer_and_tls_base(65536, 0);
    mb.add_data(DataSegment {
        mode: SegmentMode::Passive,
        bytes: vec![0x42, 0x43],
    });
    mb.add_tag(Tag {
        params: vec![Type::I32],
    });
    mb.add_table(TableType {
        min: 2,
        max: Some(2),
    });
    mb.add_function(Function::new(vec![], vec![], vec![]));
    mb.add_function(Function::new(vec![], vec![], vec![]));
    mb.add_element(ElementSegment {
        mode: SegmentMode::Active {
            index: 0,
            offset: InitExpr::i32(0),
        },
        funcs: vec![1, 2],
    });
    mb.export_function("f1", 2);
    mb.export_memory("memory", 0);
    mb.build()
}

/// The snapshot fixture: N2's comprehensive module with the bodies of `add`,
/// `nested_loop` and `computed_goto_dispatch` appended as functions 3, 4, 5,
/// plus one function exercising If/Block/Store/Load/Select/Convert/F64 consts.
pub fn showcase() -> Module {
    let mut m = comprehensive();
    for f in [add(), nested_loop(), computed_goto_dispatch()] {
        m.functions.extend(f.functions);
    }
    let f64c = |v: f64| Expr::Const {
        ty: Type::F64,
        val: ConstVal::f64(v),
    };
    let body = vec![
        Op::Block {
            body: vec![Op::Store {
                ty: wnjr_core::njvm::MemType::F64,
                ptr: Expr::LocalGet(0),
                val: Expr::binary(BinaryOp::Mul, f64c(1.5), f64c(-0.0), Type::F64),
                offset: 8,
                align: 3,
            }],
        },
        Op::If {
            cond: Expr::LocalGet(0),
            then_body: vec![Op::Drop {
                val: Expr::Select {
                    cond: Expr::LocalGet(0).boxed(),
                    then_val: c32(1).boxed(),
                    else_val: c32(2).boxed(),
                },
            }],
            else_body: vec![],
        },
        Op::Return {
            vals: vec![Expr::Convert {
                from: Type::F64,
                to: Type::I32,
                val: Expr::Load {
                    ty: wnjr_core::njvm::MemType::F64,
                    ptr: Expr::LocalGet(0).boxed(),
                    offset: 8,
                    align: 3,
                }
                .boxed(),
            }],
        },
    ];
    m.functions
        .push(func(vec![Type::Ptr], vec![Type::I32], vec![], body));
    m
}

/// Every good fixture, with a stable name for snapshotting.
pub fn all() -> Vec<(&'static str, Module)> {
    vec![
        ("add", add()),
        ("two_label_loop", two_label_loop()),
        ("nested_loop", nested_loop()),
        ("computed_goto_dispatch", computed_goto_dispatch()),
        ("comprehensive", comprehensive()),
        ("showcase", showcase()),
    ]
}

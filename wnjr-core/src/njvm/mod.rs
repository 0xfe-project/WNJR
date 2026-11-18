//! NJVM-IR: typed, structured intermediate representation.
//!
//! First step of the NJVM transform path (task N1). This module only defines
//! the IR data structures and an in-memory builder. No lowering, no
//! verification, no wasm/SIR emission (those are N2–N4). The engine is
//! unchanged.
//!
//! Op groups (see `op::Op`): A flat CFG, B expressions, C structured control,
//! D calls, E exceptions, F coroutines, G memory/locals, H other.
//!
//! Constraints enforced by the N3 verifier (`verify::verify_module`):
//! - operand stack is empty at every `Label`; cross-edge values go through locals;
//! - only reducible CFGs are accepted.

pub mod builder;
pub mod computed_goto;
pub mod expr;
pub mod module;
pub mod op;
pub mod encode;
pub mod stackify;
pub mod erase;
pub mod types;
pub mod verify;

pub use builder::{FunctionBuilder, ModuleBuilder};
pub use computed_goto::{lower_function, lower_module, ComputedGotoError, Dispatch, Lowered};
pub use expr::Expr;
pub use module::{
    DataSegment, ElementSegment, Export, ExportKind, Function, FunctionType, Global, GlobalType,
    Import, ImportKind, InitExpr, MemoryType, Module, SegmentMode, TableType, Tag,
};
pub use op::{Catch, LabelId, Op};
pub use types::{AtomicOp, BinaryOp, ConstVal, MemType, Type, UnaryOp};
pub use stackify::{stackify, max_depth, Node, StackifyError};
pub use verify::{verify_module, OpName, OpPath, VerifyError, VerifyErrorKind};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::vec;

    /// Build `fn add(a: i32, b: i32) -> i32 { return a + b; }` as NJVM-IR:
    /// one entry Label, then Return(Binary{Add, LocalGet(0), LocalGet(1)}).
    #[test]
    fn build_add_function() {
        let mut fb = FunctionBuilder::new(
            vec![Type::I32, Type::I32], // params a, b
            vec![Type::I32],            // result
            vec![],                     // no extra locals
        );

        let entry = fb.next_label();
        fb.label(entry, vec![]);

        let sum = Expr::binary(
            BinaryOp::Add,
            Expr::LocalGet(0),
            Expr::LocalGet(1),
            Type::I32,
        );
        fb.push_op(Op::Return { vals: vec![sum] });

        let func = fb.build();

        // Signature is correct.
        assert_eq!(func.params, vec![Type::I32, Type::I32]);
        assert_eq!(func.results, vec![Type::I32]);
        assert!(func.locals.is_empty());

        // Body: Label then Return.
        assert_eq!(func.body.len(), 2);
        assert_eq!(func.body[0], Op::Label { id: 0, params: vec![] });

        match &func.body[1] {
            Op::Return { vals } => {
                assert_eq!(vals.len(), 1);
                assert_eq!(
                    vals[0],
                    Expr::Binary {
                        op: BinaryOp::Add,
                        lhs: alloc::boxed::Box::new(Expr::LocalGet(0)),
                        rhs: alloc::boxed::Box::new(Expr::LocalGet(1)),
                        ty: Type::I32,
                    }
                );
            }
            other => panic!("expected Return, got {:?}", other),
        }

        // Every op and type must be Debug-printable.
        let printed = alloc::format!("{:?}", func);
        assert!(printed.contains("Add"));
        assert!(printed.contains("Return"));
    }

    /// Build a simple counting loop with two labels and Jmp/JmpIf:
    ///
    ///   local i = 0
    /// head:                       (label 0)
    ///   if i >= 10 jmp exit        (JmpIf on the comparison, to label 1)
    ///   i = i + 1
    ///   jmp head                   (back-edge to label 0)
    /// exit:                        (label 1)
    ///   return
    #[test]
    fn build_loop_with_two_labels() {
        // one i32 local: the counter i (index 0, no params).
        let mut fb = FunctionBuilder::new(vec![], vec![], vec![Type::I32]);

        // i = 0
        fb.push_op(Op::LocalSet {
            idx: 0,
            val: Expr::Const {
                ty: Type::I32,
                val: ConstVal::I32(0),
            },
        });

        let head = fb.next_label();
        let exit = fb.next_label();

        // head:
        fb.label(head, vec![]);

        // if i >= 10 jmp exit
        let cond = Expr::binary(
            BinaryOp::Ge,
            Expr::LocalGet(0),
            Expr::Const {
                ty: Type::I32,
                val: ConstVal::I32(10),
            },
            Type::I32,
        );
        fb.jmp_if(cond, exit);

        // i = i + 1
        fb.push_op(Op::LocalSet {
            idx: 0,
            val: Expr::binary(
                BinaryOp::Add,
                Expr::LocalGet(0),
                Expr::Const {
                    ty: Type::I32,
                    val: ConstVal::I32(1),
                },
                Type::I32,
            ),
        });

        // jmp head (back-edge)
        fb.jmp(head);

        // exit:
        fb.label(exit, vec![]);
        fb.push_op(Op::Return { vals: vec![] });

        let func = fb.build();

        // Two distinct labels were allocated.
        assert_eq!(head, 0);
        assert_eq!(exit, 1);

        // Body shape: LocalSet, Label(head), JmpIf, LocalSet, Jmp(head), Label(exit), Return.
        assert_eq!(func.body.len(), 7);
        assert_eq!(func.body[1], Op::Label { id: head, params: vec![] });
        assert_eq!(func.body[5], Op::Label { id: exit, params: vec![] });

        // The JmpIf targets exit.
        match &func.body[2] {
            Op::JmpIf { target, .. } => assert_eq!(*target, exit),
            other => panic!("expected JmpIf, got {:?}", other),
        }
        // The back-edge Jmp targets head.
        assert_eq!(func.body[4], Op::Jmp { target: head });

        // Debug-printable.
        let printed = alloc::format!("{:?}", func);
        assert!(printed.contains("JmpIf"));
        assert!(printed.contains("Jmp"));
    }

    /// Build a module and add the add function to it.
    #[test]
    fn build_module() {
        let mut mb = ModuleBuilder::new();
        let fb = FunctionBuilder::new(vec![Type::I32], vec![Type::I32], vec![]);
        mb.add_function(fb.build());
        let module = mb.build();
        assert_eq!(module.functions.len(), 1);
        // In N2, type table is nonempty when functions are added or imported.
        // N1 tests just check the functions are there.
        assert!(module.globals.is_empty());
    }
}

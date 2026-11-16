//! Decoded semantic function-body model.
//!
//! The data types now live in `njvm_ir::sir::semantic_ir`. This module
//! re-exports them and keeps the checks that return `WasmError`
//! (`validate`, `ensure_prepare_supported`) as free functions, because
//! njvm-ir does not depend on `WasmError`.

pub(crate) use njvm_ir::sir::semantic_ir::*;

#[allow(unused_imports)]
use crate::value_type::ValueType;
use crate::error::WasmError;

use super::common::SemanticTarget;

#[cfg(any(debug_assertions, test))]
fn semantic_op_result_arity(kind: &SemanticOpKind) -> Option<usize> {
    match kind {
        SemanticOpKind::CallDirect { results, .. }
        | SemanticOpKind::CallIndirect { results, .. }
        | SemanticOpKind::CallRef { results, .. }
        | SemanticOpKind::ReturnCallDirect { results, .. }
        | SemanticOpKind::ReturnCallIndirect { results, .. }
        | SemanticOpKind::ReturnCallRef { results, .. } => Some(*results as usize),
        SemanticOpKind::Block { results, .. }
        | SemanticOpKind::Loop { results, .. }
        | SemanticOpKind::If { results, .. }
        | SemanticOpKind::TryTable { results, .. } => Some(*results as usize),
        SemanticOpKind::AllocExnRef { .. } => Some(1),
        SemanticOpKind::Primitive(kind) => Some(super::primitive_op::stack_effect(kind).1 as usize),
        _ => None,
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn requires_simd(semantic: &SemanticProgram) -> bool {
    semantic.local_types
        .iter()
        .chain(semantic.result_types.iter())
        .chain(semantic.op_result_types.values().flat_map(|types| types.iter()))
        .any(|value_type| matches!(value_type, ValueType::V128))
        || semantic.ops.iter().any(|op| {
            matches!(
                &op.kind,
                SemanticOpKind::Primitive(kind)
                    if matches!(super::primitive_op::result_type(kind), Some(ValueType::V128))
            )
        })
}

pub(crate) fn ensure_prepare_supported(semantic: &SemanticProgram) -> Result<(), WasmError> {
    // Only read on backends without SIMD (the cfg block below).
    let _ = semantic;
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    if requires_simd(semantic) {
        return Err(WasmError::invalid(
            "SIMD is not supported by the selected backend",
        ));
    }
    Ok(())
}

#[cfg(any(debug_assertions, test))]
pub(crate) fn validate(semantic: &SemanticProgram) -> Result<(), WasmError> {
    let len = semantic.ops.len();

    for (_index, op) in semantic.ops.iter().enumerate() {
        match &op.kind {
            SemanticOpKind::LocalGet { idx }
            | SemanticOpKind::LocalSet { idx }
            | SemanticOpKind::LocalTee { idx } => {
                if *idx >= semantic.local_count {
                    return Err(WasmError::internal(
                        "semantic op uses out-of-range local (local_count=)",
                    ));
                }
            }
            SemanticOpKind::If { else_target, .. } => {
                validate_target(*else_target, len)?;
            }
            SemanticOpKind::Else { end_target } => {
                validate_target(*end_target, len)?;
            }
            SemanticOpKind::Br { target, .. }
            | SemanticOpKind::BrIf { target, .. }
            | SemanticOpKind::BrOnNull { target, .. }
            | SemanticOpKind::BrOnNonNull { target, .. }
            | SemanticOpKind::BrOnCast { target, .. }
            | SemanticOpKind::BrOnCastFail { target, .. } => {
                validate_target(*target, len)?;
            }
            SemanticOpKind::BrTable { entries } => {
                for entry in entries {
                    validate_target(entry.target, len)?;
                }
            }
            SemanticOpKind::TryTable { catches, .. } => {
                for catch in catches {
                    validate_target(catch.target, len)?;
                }
            }
            SemanticOpKind::ReturnVoid if semantic.results != 0 => {
                return Err(WasmError::internal(
                    "semantic return_void at op does not match function result arity",
                ));
            }
            SemanticOpKind::ReturnOne if semantic.results != 1 => {
                return Err(WasmError::internal(
                    "semantic return_one at op does not match function result arity",
                ));
            }
            SemanticOpKind::Return { arity } if *arity != semantic.results => {
                return Err(WasmError::internal(
                    "semantic return at op has arity , expected",
                ));
            }
            SemanticOpKind::ReturnCallDirect { results, .. }
            | SemanticOpKind::ReturnCallIndirect { results, .. }
            | SemanticOpKind::ReturnCallRef { results, .. }
                if *results != semantic.results =>
            {
                return Err(WasmError::internal(
                    "semantic tail call at op does not match function result arity",
                ));
            }
            _ => {}
        }
    }

    // Validate local_types length when present.
    if !semantic.local_types.is_empty() && semantic.local_types.len() != semantic.local_count as usize {
        return Err(WasmError::internal(
            "semantic local_types length does not match local_count",
        ));
    }
    if !semantic.result_types.is_empty() && semantic.result_types.len() != semantic.results as usize {
        return Err(WasmError::internal(
            "semantic result_types length does not match result arity",
        ));
    }

    // Validate op_result_types indices are in range and match arity.
    for (op_idx, result_types) in &semantic.op_result_types {
        if *op_idx >= len {
            return Err(WasmError::internal(
                "op_result_types entry for op index is out of range (ops len=)",
            ));
        }
        if let Some(op) = semantic.ops.get(*op_idx) {
            let Some(expected_results) = semantic_op_result_arity(&op.kind) else {
                return Err(WasmError::internal(
                    "op_result_types entry at op attached to a non-producing op",
                ));
            };
            if result_types.len() != expected_results {
                return Err(WasmError::internal(
                    "op_result_types at op has types but op expects results",
                ));
            }
        }
    }

    Ok(())
}

#[cfg(not(any(debug_assertions, test)))]
#[inline]
pub(crate) fn validate(semantic: &SemanticProgram) -> Result<(), WasmError> {
    Ok(())
}

#[cfg(any(debug_assertions, test))]
fn validate_target(target: SemanticTarget, len: usize) -> Result<(), WasmError> {
    if target.is_pending() {
        return Err(WasmError::internal(
            "semantic target is still pending at validation time",
        ));
    }
    if target.index().as_usize() >= len {
        return Err(WasmError::internal(
            "semantic target is out of range for semantic program length",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections;
    use crate::value_type::ValueType;
    use super::super::primitive_op::PrimitiveOpKind;

    #[test]
    fn rejects_out_of_range_local_access() {
        let semantic = SemanticProgram {
            params: 0,
            results: 0,
            local_count: 1,
            max_stack_height: 0,
            ops: collections::vec![SemanticOp {
                kind: SemanticOpKind::LocalGet { idx: 1 },
            }],
            local_types: collections::vec![],
            result_types: collections::vec![],
            op_result_types: tracked_alloc::collections::BTreeMap::new(),
        };

        let error = validate(&semantic)
            .expect_err("semantic validation should fail");
        assert!(error.message().contains("out-of-range local"));
    }

    #[test]
    fn rejects_out_of_range_branch_target() {
        let semantic = SemanticProgram {
            params: 0,
            results: 0,
            local_count: 0,
            max_stack_height: 0,
            ops: collections::vec![SemanticOp {
                kind: SemanticOpKind::Br {
                    stack_drop: 0,
                    arity: 0,
                    target: SemanticTarget::new(3),
                },
            }],
            local_types: collections::vec![],
            result_types: collections::vec![],
            op_result_types: tracked_alloc::collections::BTreeMap::new(),
        };

        let error = validate(&semantic)
            .expect_err("semantic validation should fail");
        assert!(error.message().contains("out of range"));
    }

    #[test]
    fn rejects_mismatched_return_arity() {
        let semantic = SemanticProgram {
            params: 0,
            results: 1,
            local_count: 0,
            max_stack_height: 1,
            ops: collections::vec![SemanticOp {
                kind: SemanticOpKind::ReturnVoid,
            }],
            local_types: collections::vec![],
            result_types: collections::vec![],
            op_result_types: tracked_alloc::collections::BTreeMap::new(),
        };

        let error = validate(&semantic)
            .expect_err("semantic validation should fail");
        assert!(error
            .message()
            .contains("does not match function result arity"));
    }

    #[test]
    fn accepts_structured_control_result_types() {
        let semantic = SemanticProgram {
            params: 1,
            results: 1,
            local_count: 1,
            max_stack_height: 1,
            ops: collections::vec![
                SemanticOp {
                    kind: SemanticOpKind::LocalGet { idx: 0 },
                },
                SemanticOp {
                    kind: SemanticOpKind::If {
                        params: 0,
                        results: 1,
                        else_target: SemanticTarget::new(4),
                    },
                },
                SemanticOp {
                    kind: SemanticOpKind::Primitive(PrimitiveOpKind::I32Const { value: 1 }),
                },
                SemanticOp {
                    kind: SemanticOpKind::Else {
                        end_target: SemanticTarget::new(6),
                    },
                },
                SemanticOp {
                    kind: SemanticOpKind::Primitive(PrimitiveOpKind::I32Const { value: 2 }),
                },
                SemanticOp {
                    kind: SemanticOpKind::End,
                },
                SemanticOp {
                    kind: SemanticOpKind::ReturnOne,
                },
            ],
            local_types: collections::vec![ValueType::I32],
            result_types: collections::vec![ValueType::I32],
            op_result_types: tracked_alloc::collections::BTreeMap::from([(
                1usize,
                collections::vec![ValueType::I32],
            )]),
        };

        validate(&semantic)
            .expect("if result type metadata should be accepted");
    }
}

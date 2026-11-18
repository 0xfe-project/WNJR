//! NJVM-IR builder: construct functions and modules in memory.

use alloc::string::ToString;

use crate::collections::Vec;

use super::expr::Expr;
use super::module::{
    DataSegment, ElementSegment, Export, ExportKind, Function, FunctionType, Global, GlobalType,
    Import, ImportKind, InitExpr, MemoryType, Module, TableType, Tag,
};
use super::op::{LabelId, Op};
use super::types::{ConstVal, Type};

/// A builder for constructing an NJVM-IR function incrementally.
///
/// Usage:
/// 1. `FunctionBuilder::new(params, results, locals)`
/// 2. Call `push_op(...)` to append ops to the body.
/// 3. Use `next_label()` to allocate a fresh label id (or pick your own).
/// 4. Call `build()` to finalize and return the `Function`.
///
/// No verifier is run in N1; that happens in N3. For now, the builder just
/// tracks the body as a `Vec<Op>`.
pub struct FunctionBuilder {
    params: Vec<Type>,
    results: Vec<Type>,
    locals: Vec<Type>,
    body: Vec<Op>,
    next_label_id: LabelId,
}

impl FunctionBuilder {
    /// Create a new function builder with the given signature and locals.
    pub fn new(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>) -> Self {
        FunctionBuilder {
            params,
            results,
            locals,
            body: Vec::new(),
            next_label_id: 0,
        }
    }

    /// Allocate a fresh label id. You can call this before emitting the
    /// `Label` op, then use the returned id in `Jmp`/`JmpIf` targets.
    pub fn next_label(&mut self) -> LabelId {
        let id = self.next_label_id;
        self.next_label_id += 1;
        id
    }

    /// Append an operation to the function body.
    pub fn push_op(&mut self, op: Op) {
        self.body.push(op);
    }

    /// Convenience: push a `Label` op with the given id and params.
    pub fn label(&mut self, id: LabelId, params: Vec<Type>) {
        self.push_op(Op::Label { id, params });
    }

    /// Convenience: push an unconditional jump.
    pub fn jmp(&mut self, target: LabelId) {
        self.push_op(Op::Jmp { target });
    }

    /// Convenience: push a conditional jump.
    pub fn jmp_if(&mut self, cond: Expr, target: LabelId) {
        self.push_op(Op::JmpIf { cond, target });
    }

    /// Finalize and return the constructed function.
    pub fn build(self) -> Function {
        Function {
            params: self.params,
            results: self.results,
            locals: self.locals,
            body: self.body,
        }
    }
}

/// A builder for constructing an NJVM-IR module incrementally.
pub struct ModuleBuilder {
    module: Module,
}

impl ModuleBuilder {
    /// Create a new empty module builder.
    pub fn new() -> Self {
        ModuleBuilder {
            module: Module::new(),
        }
    }

    /// Add a function type to the module.
    pub fn add_type(&mut self, ty: FunctionType) -> u32 {
        let idx = self.module.types.len() as u32;
        self.module.types.push(ty);
        idx
    }

    /// Add an import to the module.
    pub fn add_import(&mut self, import: Import) {
        self.module.imports.push(import);
    }

    /// Convenience: import a function with the given type index.
    pub fn import_function(&mut self, module: &str, name: &str, type_idx: u32) {
        self.module.imports.push(Import {
            module: module.to_string(),
            name: name.to_string(),
            kind: ImportKind::Function(type_idx),
        });
    }

    /// Convenience: import a shared memory.
    pub fn import_memory(&mut self, module: &str, name: &str, min: u32, max: Option<u32>) {
        self.module.imports.push(Import {
            module: module.to_string(),
            name: name.to_string(),
            kind: ImportKind::Memory(MemoryType {
                min,
                max,
                shared: true,
            }),
        });
    }

    /// Convenience: import a mutable i32 global.
    pub fn import_global_i32_mut(&mut self, module: &str, name: &str) {
        self.module.imports.push(Import {
            module: module.to_string(),
            name: name.to_string(),
            kind: ImportKind::Global(GlobalType {
                ty: Type::I32,
                mutable: true,
            }),
        });
    }

    /// Add a function definition to the module.
    pub fn add_function(&mut self, func: Function) {
        self.module.functions.push(func);
    }

    /// Add a table to the module.
    pub fn add_table(&mut self, table: TableType) {
        self.module.tables.push(table);
    }

    /// Add an element segment to the module.
    pub fn add_element(&mut self, elem: ElementSegment) {
        self.module.elements.push(elem);
    }

    /// Add a memory to the module.
    pub fn add_memory(&mut self, memory: MemoryType) {
        self.module.memories.push(memory);
    }

    /// Add a global to the module.
    pub fn add_global(&mut self, global: Global) {
        self.module.globals.push(global);
    }

    /// Convenience: add a mutable i32 global initialized to `init_value`.
    pub fn add_i32_global(&mut self, init_value: i32) -> u32 {
        let idx = self.module.imported_globals() + self.module.globals.len() as u32;
        self.module.globals.push(Global {
            ty: GlobalType {
                ty: Type::I32,
                mutable: true,
            },
            init: InitExpr::Const(ConstVal::I32(init_value)),
        });
        idx
    }

    /// Add the two conventional mutable i32 globals, `stack_pointer` and
    /// `tls_base`, record their indices on the module, and return them as
    /// `(stack_pointer, tls_base)` in the global index space.
    pub fn add_stack_pointer_and_tls_base(&mut self, stack_top: i32, tls_base: i32) -> (u32, u32) {
        let sp = self.add_i32_global(stack_top);
        let tls = self.add_i32_global(tls_base);
        self.module.stack_pointer = Some(sp);
        self.module.tls_base = Some(tls);
        (sp, tls)
    }

    /// Add an export to the module.
    pub fn add_export(&mut self, export: Export) {
        self.module.exports.push(export);
    }

    /// Convenience: export a function.
    pub fn export_function(&mut self, name: &str, func_idx: u32) {
        self.module.exports.push(Export {
            name: name.to_string(),
            kind: ExportKind::Function,
            index: func_idx,
        });
    }

    /// Convenience: export a memory.
    pub fn export_memory(&mut self, name: &str, memory_idx: u32) {
        self.module.exports.push(Export {
            name: name.to_string(),
            kind: ExportKind::Memory,
            index: memory_idx,
        });
    }

    /// Set the start function.
    pub fn set_start(&mut self, func_idx: u32) {
        self.module.start = Some(func_idx);
    }

    /// Add a data segment to the module.
    pub fn add_data(&mut self, data: DataSegment) {
        self.module.data.push(data);
    }

    /// Add a tag to the module.
    pub fn add_tag(&mut self, tag: Tag) {
        self.module.tags.push(tag);
    }

    /// Finalize and return the constructed module.
    pub fn build(self) -> Module {
        self.module
    }
}

impl Default for ModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

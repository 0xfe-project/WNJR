//! NJVM-IR function and module structures.
//!
//! Index spaces follow wasm: for functions, tables, memories, globals and
//! tags, imported entries come first, then the ones defined in this module.
//! A function index `i` means import function `i` when `i` is below the
//! number of imported functions, otherwise `functions[i - imported]`.

use alloc::string::String;

use crate::collections::Vec;

use super::op::Op;
use super::types::{ConstVal, Type};

/// A function in NJVM-IR.
///
/// N1 (this task) defines the function body as a flat `Vec<Op>` with labels
/// and jumps. N3 (verifier) will check that the CFG is reducible and that
/// labels are declared before use. For now, the builder just appends ops.
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    /// Parameter types.
    pub params: Vec<Type>,
    /// Return types (multiple results are allowed).
    pub results: Vec<Type>,
    /// Local variable types (beyond parameters).
    pub locals: Vec<Type>,
    /// Function body: a sequence of ops.
    pub body: Vec<Op>,
}

impl Function {
    /// Create a new function with the given signature and locals. Body starts empty.
    pub fn new(params: Vec<Type>, results: Vec<Type>, locals: Vec<Type>) -> Self {
        Function {
            params,
            results,
            locals,
            body: Vec::new(),
        }
    }

    /// The signature of this function.
    pub fn signature(&self) -> FunctionType {
        FunctionType {
            params: self.params.clone(),
            results: self.results.clone(),
        }
    }
}

/// A function type signature.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunctionType {
    pub params: Vec<Type>,
    pub results: Vec<Type>,
}

/// A constant initializer: the value of a global, or the offset of an
/// active data/element segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InitExpr {
    /// A literal.
    Const(ConstVal),
    /// The value of an imported, immutable global (global index space).
    GlobalGet(u32),
}

impl InitExpr {
    /// `i32.const v`.
    pub fn i32(v: i32) -> Self {
        InitExpr::Const(ConstVal::I32(v))
    }
}

/// Whether a data or element segment is copied at instantiation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SegmentMode {
    /// Copied into memory/table `index` at `offset` when the module is
    /// instantiated.
    Active { index: u32, offset: InitExpr },
    /// Copied only by an explicit `memory.init`/`table.init`. Required for
    /// data under wasi-threads, where every thread instantiates the module
    /// and active segments would overwrite memory already written.
    Passive,
}

/// A table. Its element type is always `funcref`: table 0 is the
/// function-pointer table, and a C function pointer is an index into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableType {
    pub min: u32,
    pub max: Option<u32>,
}

/// An element segment: function indices placed into a table.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ElementSegment {
    pub mode: SegmentMode,
    /// Indices in the function index space.
    pub funcs: Vec<u32>,
}

/// A linear memory with 32-bit addresses, sized in 64 KiB pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemoryType {
    pub min: u32,
    pub max: Option<u32>,
    /// Shared memory (threads). wasi-threads requires memory 0 to be an
    /// imported shared memory. A shared memory must have a `max`.
    pub shared: bool,
}

/// The type of a global.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalType {
    pub ty: Type,
    pub mutable: bool,
}

/// A global defined in this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Global {
    pub ty: GlobalType,
    pub init: InitExpr,
}

/// A data segment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DataSegment {
    pub mode: SegmentMode,
    pub bytes: Vec<u8>,
}

/// An exception tag: the types of the values a `throw` carries.
/// setjmp/longjmp use one module-level tag.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tag {
    pub params: Vec<Type>,
}

/// What an import brings in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImportKind {
    /// A function of type `types[i]`.
    Function(u32),
    Table(TableType),
    Memory(MemoryType),
    Global(GlobalType),
    Tag(Tag),
}

/// An import.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub kind: ImportKind,
}

/// The index space an export refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportKind {
    Function,
    Table,
    Memory,
    Global,
    Tag,
}

/// An export.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Export {
    pub name: String,
    pub kind: ExportKind,
    /// Index in the index space named by `kind`.
    pub index: u32,
}

/// An NJVM-IR module.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    /// Function types referenced by imported functions. Signatures of
    /// defined functions and tags are added to the wasm type section by the
    /// encoder as needed.
    pub types: Vec<FunctionType>,
    pub imports: Vec<Import>,
    /// Defined functions. Their indices start after the imported functions.
    pub functions: Vec<Function>,
    pub tables: Vec<TableType>,
    pub memories: Vec<MemoryType>,
    pub globals: Vec<Global>,
    pub tags: Vec<Tag>,
    pub exports: Vec<Export>,
    /// Start function (function index space).
    pub start: Option<u32>,
    pub elements: Vec<ElementSegment>,
    pub data: Vec<DataSegment>,
    /// Global index of the conventional mutable i32 `stack_pointer`.
    pub stack_pointer: Option<u32>,
    /// Global index of the conventional mutable i32 `tls_base`.
    pub tls_base: Option<u32>,
}

impl Module {
    /// Create an empty module.
    pub fn new() -> Self {
        Module {
            types: Vec::new(),
            imports: Vec::new(),
            functions: Vec::new(),
            tables: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            tags: Vec::new(),
            exports: Vec::new(),
            start: None,
            elements: Vec::new(),
            data: Vec::new(),
            stack_pointer: None,
            tls_base: None,
        }
    }

    fn count_imports(&self, f: impl Fn(&ImportKind) -> bool) -> u32 {
        self.imports.iter().filter(|i| f(&i.kind)).count() as u32
    }

    /// Number of imported functions.
    pub fn imported_functions(&self) -> u32 {
        self.count_imports(|k| matches!(k, ImportKind::Function(_)))
    }
    /// Number of imported tables.
    pub fn imported_tables(&self) -> u32 {
        self.count_imports(|k| matches!(k, ImportKind::Table(_)))
    }
    /// Number of imported memories.
    pub fn imported_memories(&self) -> u32 {
        self.count_imports(|k| matches!(k, ImportKind::Memory(_)))
    }
    /// Number of imported globals.
    pub fn imported_globals(&self) -> u32 {
        self.count_imports(|k| matches!(k, ImportKind::Global(_)))
    }
    /// Number of imported tags.
    pub fn imported_tags(&self) -> u32 {
        self.count_imports(|k| matches!(k, ImportKind::Tag(_)))
    }
}

impl Default for Module {
    fn default() -> Self {
        Self::new()
    }
}

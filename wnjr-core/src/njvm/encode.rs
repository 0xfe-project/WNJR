//! Encode an NJVM-IR module as a WebAssembly binary.
//!
//! N2 task: the encoder writes every function body as `unreachable`, since op
//! lowering happens in a later task (code is a placeholder, see the code
//! section below). The module-level facts (types, imports, functions, tables,
//! memories, tags, globals, exports, start, elements, data) are fully encoded.

use alloc::collections::BTreeMap;

use wasm_encoder::{
    CodeSection, ConstExpr, DataCountSection, DataSection, DataSegment as WasmDataSegment,
    DataSegmentMode, ElementSection, Elements, EntityType, ExportKind as WasmExportKind,
    ExportSection, Function as WasmFunction, FunctionSection, GlobalSection,
    GlobalType as WasmGlobalType, Ieee32, Ieee64, ImportSection, MemorySection,
    MemoryType as WasmMemoryType, Module as WasmModule, RefType, StartSection, TableSection,
    TableType as WasmTableType, TagKind, TagSection, TagType as WasmTagType, TypeSection, ValType,
};

use crate::collections::{vec, Vec};

use super::module::{ExportKind, ImportKind, InitExpr, Module, SegmentMode};
use super::types::{ConstVal, Type};

/// Encoding error: the item at `index` failed to encode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// A narrow integer type (I8/U8/I16/U16) appeared in the signature of the
    /// defined function at `index` (unified function index space, imports
    /// first). wasm has no such value type.
    NarrowTypeInSignature { index: u32, ty: Type },
    /// A narrow integer type appeared in `Module::types[index]`.
    NarrowTypeInDeclaredType { index: u32, ty: Type },
    /// A narrow integer type appeared in the defined global at `index`
    /// (global index space, imports first).
    NarrowTypeInGlobal { index: u32, ty: Type },
    /// A narrow integer type appeared in the defined tag at `index`
    /// (position in `Module::tags`).
    NarrowTypeInTag { index: u32, ty: Type },
    /// A narrow integer type appeared in the import at `index` (position in
    /// `Module::imports`).
    NarrowTypeInImport { index: u32, ty: Type },
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EncodeError::NarrowTypeInSignature { index, ty } => write!(
                f,
                "function signature at index {index} contains unsupported narrow type {ty:?}"
            ),
            EncodeError::NarrowTypeInDeclaredType { index, ty } => {
                write!(f, "declared type {index} contains unsupported narrow type {ty:?}")
            }
            EncodeError::NarrowTypeInGlobal { index, ty } => {
                write!(f, "global at index {index} has unsupported narrow type {ty:?}")
            }
            EncodeError::NarrowTypeInTag { index, ty } => {
                write!(f, "tag at index {index} has unsupported narrow type {ty:?}")
            }
            EncodeError::NarrowTypeInImport { index, ty } => {
                write!(f, "import at index {index} has unsupported narrow type {ty:?}")
            }
        }
    }
}

/// Interns function types into the wasm type section, deduplicating by shape.
struct TypeInterner {
    section: TypeSection,
    map: BTreeMap<(Vec<ValType>, Vec<ValType>), u32>,
    next: u32,
}

impl TypeInterner {
    fn new() -> Self {
        TypeInterner {
            section: TypeSection::new(),
            map: BTreeMap::new(),
            next: 0,
        }
    }

    /// Append a declared type at the next index, without deduplication, so
    /// `Module::types[i]` is wasm type `i`. Later lookups may reuse it.
    fn declare(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        let idx = self.next;
        self.section.ty().function(params.clone(), results.clone());
        self.map.entry((params, results)).or_insert(idx);
        self.next += 1;
        idx
    }

    /// Intern `(params) -> (results)`, returning its type index.
    fn intern(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        let key = (params.clone(), results.clone());
        if let Some(&idx) = self.map.get(&key) {
            return idx;
        }
        self.section.ty().function(params, results);
        let idx = self.next;
        self.map.insert(key, idx);
        self.next += 1;
        idx
    }
}

/// Encode an NJVM-IR module as a wasm binary.
///
/// Every function body is encoded as `unreachable` — a placeholder for N2.
/// Op lowering (turning the NJVM-IR body into real wasm instructions) is a
/// later task. Returns an error, carrying the failing item's index, when a
/// narrow type (I8/U8/I16/U16) appears where wasm needs i32/i64/f32/f64.
pub fn encode(module: &Module) -> Result<Vec<u8>, EncodeError> {
    let mut wasm = WasmModule::new();

    let mut types = TypeInterner::new();
    for (i, ft) in module.types.iter().enumerate() {
        let params = to_val_types(&ft.params, FnSig::DeclaredType(i as u32))?;
        let results = to_val_types(&ft.results, FnSig::DeclaredType(i as u32))?;
        types.declare(params, results);
    }
    // Signature errors report the function's index in the unified function
    // index space (imported functions first).
    let imported_funcs = module.imported_functions();
    let mut func_type_indices: Vec<u32> = Vec::new();
    for (i, func) in module.functions.iter().enumerate() {
        let owner = FnSig::Signature(imported_funcs + i as u32);
        let params = to_val_types(&func.params, owner)?;
        let results = to_val_types(&func.results, owner)?;
        func_type_indices.push(types.intern(params, results));
    }
    let mut tag_type_indices: Vec<u32> = Vec::new();
    for (i, tag) in module.tags.iter().enumerate() {
        let params = to_val_types(&tag.params, FnSig::Tag(i as u32))?;
        tag_type_indices.push(types.intern(params, Vec::new()));
    }

    // Imported tags need their signature in the type section too; intern them
    // before the section is emitted. Indexed by import position.
    let mut import_tag_types: BTreeMap<usize, u32> = BTreeMap::new();
    for (i, imp) in module.imports.iter().enumerate() {
        if let ImportKind::Tag(tag) = &imp.kind {
            let params = to_val_types(&tag.params, FnSig::Import(i as u32))?;
            import_tag_types.insert(i, types.intern(params, Vec::new()));
        }
    }

    wasm.section(&types.section);

    // Import section.
    if !module.imports.is_empty() {
        let mut imports = ImportSection::new();
        for (i, imp) in module.imports.iter().enumerate() {
            let entity = match &imp.kind {
                ImportKind::Function(type_idx) => EntityType::Function(*type_idx),
                ImportKind::Table(tt) => EntityType::Table(WasmTableType {
                    element_type: RefType::FUNCREF,
                    minimum: tt.min as u64,
                    maximum: tt.max.map(|m| m as u64),
                    table64: false,
                    shared: false,
                }),
                ImportKind::Memory(mt) => EntityType::Memory(WasmMemoryType {
                    minimum: mt.min as u64,
                    maximum: mt.max.map(|m| m as u64),
                    memory64: false,
                    shared: mt.shared,
                    page_size_log2: None,
                }),
                ImportKind::Global(gt) => EntityType::Global(WasmGlobalType {
                    val_type: to_val_type(gt.ty).map_err(|_| EncodeError::NarrowTypeInImport {
                        index: i as u32,
                        ty: gt.ty,
                    })?,
                    mutable: gt.mutable,
                    shared: false,
                }),
                ImportKind::Tag(_) => EntityType::Tag(WasmTagType {
                    kind: TagKind::Exception,
                    func_type_idx: import_tag_types[&i],
                }),
            };
            imports.import(&imp.module, &imp.name, entity);
        }
        wasm.section(&imports);
    }

    // Function section.
    if !module.functions.is_empty() {
        let mut functions = FunctionSection::new();
        for &type_idx in &func_type_indices {
            functions.function(type_idx);
        }
        wasm.section(&functions);
    }

    // Table section.
    if !module.tables.is_empty() {
        let mut tables = TableSection::new();
        for table in &module.tables {
            tables.table(WasmTableType {
                element_type: RefType::FUNCREF,
                minimum: table.min as u64,
                maximum: table.max.map(|m| m as u64),
                table64: false,
                shared: false,
            });
        }
        wasm.section(&tables);
    }

    // Memory section.
    if !module.memories.is_empty() {
        let mut memories = MemorySection::new();
        for memory in &module.memories {
            memories.memory(WasmMemoryType {
                minimum: memory.min as u64,
                maximum: memory.max.map(|m| m as u64),
                memory64: false,
                shared: memory.shared,
                page_size_log2: None,
            });
        }
        wasm.section(&memories);
    }

    // Tag section.
    if !module.tags.is_empty() {
        let mut tags = TagSection::new();
        for &type_idx in &tag_type_indices {
            tags.tag(WasmTagType {
                kind: TagKind::Exception,
                func_type_idx: type_idx,
            });
        }
        wasm.section(&tags);
    }

    // Global section.
    if !module.globals.is_empty() {
        let mut globals = GlobalSection::new();
        for (i, global) in module.globals.iter().enumerate() {
            let val_ty = to_val_type(global.ty.ty).map_err(|_| EncodeError::NarrowTypeInGlobal {
                // Global index space: imported globals first.
                index: module.imported_globals() + i as u32,
                ty: global.ty.ty,
            })?;
            let init = to_const_expr(&global.init);
            globals.global(
                WasmGlobalType {
                    val_type: val_ty,
                    mutable: global.ty.mutable,
                    shared: false,
                },
                &init,
            );
        }
        wasm.section(&globals);
    }

    // Export section.
    if !module.exports.is_empty() {
        let mut exports = ExportSection::new();
        for export in &module.exports {
            let kind = match export.kind {
                ExportKind::Function => WasmExportKind::Func,
                ExportKind::Table => WasmExportKind::Table,
                ExportKind::Memory => WasmExportKind::Memory,
                ExportKind::Global => WasmExportKind::Global,
                ExportKind::Tag => WasmExportKind::Tag,
            };
            exports.export(&export.name, kind, export.index);
        }
        wasm.section(&exports);
    }

    // Start section.
    if let Some(start_idx) = module.start {
        wasm.section(&StartSection {
            function_index: start_idx,
        });
    }

    // Element section.
    if !module.elements.is_empty() {
        let mut elements = ElementSection::new();
        for elem in &module.elements {
            match elem.mode {
                SegmentMode::Active { index, offset } => {
                    let offset_expr = to_const_expr(&offset);
                    elements.active(
                        Some(index),
                        &offset_expr,
                        Elements::Functions(elem.funcs.as_slice().into()),
                    );
                }
                SegmentMode::Passive => {
                    elements.passive(Elements::Functions(elem.funcs.as_slice().into()));
                }
            }
        }
        wasm.section(&elements);
    }

    // Data count section: declares the number of data segments so that
    // `memory.init`/`data.drop` (and passive segments) validate.
    if !module.data.is_empty() {
        wasm.section(&DataCountSection {
            count: module.data.len() as u32,
        });
    }

    // Code section. PLACEHOLDER: every function body is `unreachable`.
    // Lowering NJVM-IR ops into real wasm instructions is a later task; here
    // we only encode a well-formed body so the module parses and validates.
    if !module.functions.is_empty() {
        let mut code = CodeSection::new();
        for _ in &module.functions {
            let mut body = WasmFunction::new(vec![]); // no locals in the placeholder
            body.instructions().unreachable().end();
            code.function(&body);
        }
        wasm.section(&code);
    }

    // Data section.
    if !module.data.is_empty() {
        let mut data = DataSection::new();
        for seg in &module.data {
            match seg.mode {
                SegmentMode::Active { index, offset } => {
                    let offset_expr = to_const_expr(&offset);
                    data.segment(WasmDataSegment {
                        mode: DataSegmentMode::Active {
                            memory_index: index,
                            offset: &offset_expr,
                        },
                        data: seg.bytes.iter().copied(),
                    });
                }
                SegmentMode::Passive => {
                    data.segment(WasmDataSegment {
                        mode: DataSegmentMode::Passive,
                        data: seg.bytes.iter().copied(),
                    });
                }
            }
        }
        wasm.section(&data);
    }

    Ok(wasm.finish())
}

/// Which item a signature belongs to, for error reporting.
#[derive(Clone, Copy)]
enum FnSig {
    DeclaredType(u32),
    Signature(u32),
    Tag(u32),
    Import(u32),
}

/// Map an NJVM `Type` to a wasm `ValType`. Narrow integer types have no wasm
/// value type and are rejected.
fn to_val_type(ty: Type) -> Result<ValType, ()> {
    match ty {
        Type::I8 | Type::U8 | Type::I16 | Type::U16 => Err(()),
        Type::I32 | Type::U32 | Type::Ptr => Ok(ValType::I32),
        Type::I64 | Type::U64 => Ok(ValType::I64),
        Type::F32 => Ok(ValType::F32),
        Type::F64 => Ok(ValType::F64),
    }
}

/// Map a slice of NJVM types to wasm value types, attributing any narrow-type
/// error to the right item.
fn to_val_types(tys: &[Type], owner: FnSig) -> Result<Vec<ValType>, EncodeError> {
    let mut out = Vec::with_capacity(tys.len());
    for &ty in tys {
        match to_val_type(ty) {
            Ok(v) => out.push(v),
            Err(()) => {
                return Err(match owner {
                    FnSig::DeclaredType(index) => {
                        EncodeError::NarrowTypeInDeclaredType { index, ty }
                    }
                    FnSig::Signature(index) => EncodeError::NarrowTypeInSignature { index, ty },
                    FnSig::Tag(index) => EncodeError::NarrowTypeInTag { index, ty },
                    FnSig::Import(index) => EncodeError::NarrowTypeInImport { index, ty },
                });
            }
        }
    }
    Ok(out)
}

/// Convert an NJVM `InitExpr` to a wasm constant expression.
fn to_const_expr(init: &InitExpr) -> ConstExpr {
    match init {
        InitExpr::Const(ConstVal::I32(v)) => ConstExpr::i32_const(*v),
        InitExpr::Const(ConstVal::I64(v)) => ConstExpr::i64_const(*v),
        InitExpr::Const(ConstVal::F32(bits)) => {
            ConstExpr::f32_const(Ieee32::from(f32::from_bits(*bits)))
        }
        InitExpr::Const(ConstVal::F64(bits)) => {
            ConstExpr::f64_const(Ieee64::from(f64::from_bits(*bits)))
        }
        InitExpr::GlobalGet(idx) => ConstExpr::global_get(*idx),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::vec;
    use crate::njvm::{
        DataSegment, ElementSegment, Function, FunctionType, InitExpr, ModuleBuilder, SegmentMode,
        TableType, Tag,
    };
    use wasmparser::{Payload, ValidPayload, Validator};

    /// (a) A minimal module with one defined function.
    #[test]
    fn encode_minimal_module() {
        let mut mb = ModuleBuilder::new();
        mb.add_function(Function::new(vec![Type::I32], vec![Type::I32], vec![]));
        let module = mb.build();
        let bytes = encode(&module).expect("encode");
        // The bytes must parse and validate.
        let rt_module = crate::Module::new("minimal", &bytes).expect("parse and validate");
        assert_eq!(rt_module.functions.len(), 1);
    }

    /// (b) A comprehensive module: imported shared memory, two globals
    /// (stack_pointer/tls_base), one passive data segment, a tag, a table with an
    /// element segment referencing two functions, an imported function, and two
    /// exports. Verify with wasmparser and assert section counts.
    #[test]
    fn encode_comprehensive_module() {
        let mut mb = ModuleBuilder::new();

        // Imported function of type (i32) -> i32.
        let type_idx = mb.add_type(FunctionType {
            params: vec![Type::I32],
            results: vec![Type::I32],
        });
        mb.import_function("env", "host_add", type_idx);

        // Imported shared memory.
        mb.import_memory("env", "memory", 1, Some(16));

        // Two mutable i32 globals: stack_pointer and tls_base.
        let (_sp, _tls) = mb.add_stack_pointer_and_tls_base(65536, 0);

        // One passive data segment.
        mb.add_data(DataSegment {
            mode: SegmentMode::Passive,
            bytes: vec![0x42, 0x43],
        });

        // A tag for setjmp/longjmp (params: [i32]).
        mb.add_tag(Tag {
            params: vec![Type::I32],
        });

        // Table 0 with a max.
        mb.add_table(TableType {
            min: 2,
            max: Some(2),
        });

        // Two defined functions: f0 and f1.
        mb.add_function(Function::new(vec![], vec![], vec![]));
        mb.add_function(Function::new(vec![], vec![], vec![]));

        // Function index space: 0 = import host_add, 1 = f0, 2 = f1.
        // An active element segment puts f0 and f1 at table[0] and table[1].
        mb.add_element(ElementSegment {
            mode: SegmentMode::Active {
                index: 0,
                offset: InitExpr::i32(0),
            },
            funcs: vec![1, 2],
        });

        // Export f1 and memory 0 (re-export of the imported memory).
        mb.export_function("f1", 2);
        mb.export_memory("memory", 0);

        let module = mb.build();
        let bytes = encode(&module).expect("encode");

        // Parse and validate with WNJR runtime.
        let rt_module = crate::Module::new("comprehensive", &bytes).expect("parse and validate");
        // Total functions: 1 import + 2 defined = 3.
        assert_eq!(rt_module.functions.len(), 3);

        // Parse with wasmparser and assert section counts.
        let mut validator = Validator::new();
        let parser = wasmparser::Parser::new(0);
        let mut types = 0;
        let mut imports = 0;
        let mut functions = 0;
        let mut tables = 0;
        let mut memories = 0;
        let mut tags = 0;
        let mut globals = 0;
        let mut exports = 0;
        let mut elements = 0;
        let mut data_count_seen = false;
        let mut code_entries = 0;
        let mut data_segments = 0;

        for payload in parser.parse_all(&bytes) {
            let payload = payload.expect("parse payload");
            match validator.payload(&payload).expect("validate") {
                ValidPayload::End(_) => break,
                _ => {}
            }
            match payload {
                Payload::TypeSection(reader) => types = reader.count(),
                Payload::ImportSection(reader) => {
                    // A group may hold several imports under the compact
                    // encoding; count individual imports, not groups.
                    for group in reader {
                        imports += match group.expect("import group") {
                            wasmparser::Imports::Single(..) => 1,
                            wasmparser::Imports::Compact1 { items, .. } => items.count(),
                            wasmparser::Imports::Compact2 { names, .. } => names.count(),
                        };
                    }
                }
                Payload::FunctionSection(reader) => functions = reader.count(),
                Payload::TableSection(reader) => tables = reader.count(),
                Payload::MemorySection(reader) => memories = reader.count(),
                Payload::TagSection(reader) => tags = reader.count(),
                Payload::GlobalSection(reader) => globals = reader.count(),
                Payload::ExportSection(reader) => exports = reader.count(),
                Payload::ElementSection(reader) => elements = reader.count(),
                Payload::DataCountSection { count, .. } => {
                    data_count_seen = true;
                    assert_eq!(count, 1);
                }
                Payload::CodeSectionEntry(_) => code_entries += 1,
                Payload::DataSection(reader) => data_segments = reader.count(),
                _ => {}
            }
        }

        // Assertions. Note: wasm-encoder may group imports via compact encoding, so
        // `imports` here is the number of groups, not the true import count.
        // Expected:
        // - TypeSection: at least 2 types ((i32)->i32 for import, ()->() for defined functions).
        // - FunctionSection: 2 entries (the defined functions f0, f1).
        // - TableSection: 1.
        // - MemorySection: 0 (imported).
        // - TagSection: 1.
        // - GlobalSection: 2.
        // - ExportSection: 2.
        // - ElementSection: 1.
        // - DataCount: 1.
        // - Code: 2 entries (one per defined function).
        // - DataSection: 1.

        // (i32)->(i32) declared for the import, ()->() shared by f0/f1,
        // (i32)->() for the tag.
        assert_eq!(types, 3);
        assert_eq!(imports, 2, "host_add + memory");
        assert_eq!(functions, 2, "expected 2 functions");
        assert_eq!(tables, 1);
        assert_eq!(memories, 0, "memory is imported");
        assert_eq!(tags, 1);
        assert_eq!(globals, 2);
        assert_eq!(exports, 2);
        assert_eq!(elements, 1);
        assert!(data_count_seen);
        assert_eq!(code_entries, 2);
        assert_eq!(data_segments, 1);
    }

    /// (c) A narrow integer type (I8) in a function signature must return an error.
    #[test]
    fn encode_narrow_type_error() {
        let mut mb = ModuleBuilder::new();
        mb.add_function(Function::new(vec![Type::I8], vec![Type::I32], vec![]));
        let module = mb.build();
        let err = encode(&module).expect_err("should fail");
        match err {
            EncodeError::NarrowTypeInSignature { index, ty } => {
                assert_eq!(index, 0, "function 0 in unified space");
                assert_eq!(ty, Type::I8);
            }
            _ => panic!("expected NarrowTypeInSignature, got {:?}", err),
        }
    }
}

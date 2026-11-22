use tracked_alloc::string::String;

use crate::collections;
use crate::op_decoder::{Decoder, Immediate, OpStream, OpcodeHandler};
use crate::opcodes::{Opcode, WasmOpcode};

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
use self::type_defs::{CompositeType, StorageType};
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
use crate::value_type::ValueType;
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
use entities::FunctionType;
use entities::{
    Data, Element, ElementInit, Function, FunctionDef, Global, GlobalDef, Memory, MemoryDef, Table,
    TableDef, Tag, TagDef,
};

pub(crate) mod entities;
pub(crate) mod parser;
pub(crate) mod type_context;
pub(crate) mod type_defs;
pub(crate) mod validator;

use crate::error::WasmError;

/// One entry of a module's import section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportDecl<'a> {
    /// The module the entry is asked of, as the binary spells it.
    pub module: &'a str,
    /// The name within that module.
    pub name: &'a str,
    /// What kind of thing it is.
    pub kind: ImportKind,
}

/// The kinds of thing a module can import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    Func,
    Table,
    Memory,
    Global,
    Tag,
}

/// A parsed, validated WebAssembly module, ready for instantiation.
///
/// Validation is independent of the execution engine. Instantiation checks
/// imports and runtime resource limits, but does not validate the code again.
#[derive(Clone)]
pub struct Module {
    name: String,
    binary_version: u32,
    pub(crate) types: type_context::TypeContext,
    pub(crate) functions: collections::Vec<Function>,
    pub(crate) tables: collections::Vec<Table>,
    pub(crate) memories: collections::Vec<Memory>,
    pub(crate) globals: collections::Vec<Global>,
    pub(crate) tags: collections::Vec<Tag>,
    pub(crate) elements: collections::Vec<Element>,
    pub(crate) data: collections::Vec<Data>,
    start_func_index: Option<usize>,
    data_count: Option<usize>,
    #[cfg(all())]
    pub(crate) code: tracked_alloc::sync::Arc<
        parking_lot::Mutex<Option<crate::vm::jit::runtime::code::PrecompiledModule>>,
    >,
}

// SAFETY: Module is immutable after parse and validation. Its type table is
// Arc<[Arc<DefType>]>, function specs are Send + Sync (see FunctionSpec), and
// the remaining fields are plain owned data. Sharing one Module across threads
// through Arc<Module> does not race.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

impl Module {
    /// Parse and validate a WebAssembly binary.
    ///
    /// This always performs full validation, including in `no_std` builds.
    /// The input is copied into owned module storage and may be released after
    /// this returns. `name` is a diagnostic label, not an import namespace.
    pub fn new(name: &str, bin: &[u8]) -> Result<Self, WasmError> {
        let module = parser::parse_module(name, bin)?;
        validator::Validator::new(&module).validate()?;
        Ok(module)
    }

    /// Parse a binary whose WebAssembly validity the caller has established.
    ///
    /// Binary decoding and target capability checks still run. This avoids
    /// repeating semantic validation when loading externally validated input;
    /// ordinary callers should use [`Self::new`].
    ///
    /// # Safety
    ///
    /// `bin` must be a valid WebAssembly core module under the proposals
    /// supported by this runtime, including all function bodies, type and index
    /// references, and constant expressions. Validation of another binary,
    /// successful WAT encoding, or successful decoding alone is insufficient.
    /// Passing an invalid module can lead to undefined behavior during later
    /// compilation or execution, even if this constructor returns `Ok`.
    ///
    /// Calling this constructor requires an explicit unsafe block:
    ///
    /// ```compile_fail
    /// wnjr_core::Module::new_unchecked("empty", b"\0asm\x01\0\0\0").unwrap();
    /// ```
    pub unsafe fn new_unchecked(name: &str, bin: &[u8]) -> Result<Self, WasmError> {
        let module = parser::parse_module(name, bin)?;
        module.ensure_simd_supported()?;
        Ok(module)
    }

    pub(crate) fn ensure_simd_supported(&self) -> Result<(), WasmError> {
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        if self.requires_simd() {
            return Err(WasmError::invalid("SIMD is not supported on this CPU"));
        }
        Ok(())
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    pub(crate) fn requires_simd(&self) -> bool {
        self.types
            .as_slice()
            .iter()
            .any(|def_type| match &def_type.composite {
                CompositeType::Func(func_type) => function_type_requires_simd(func_type),
                CompositeType::Struct(struct_type) => struct_type
                    .fields
                    .iter()
                    .any(|field| storage_type_requires_simd(&field.storage)),
                CompositeType::Array(array_type) => {
                    storage_type_requires_simd(&array_type.element.storage)
                }
            })
            || self
                .functions
                .iter()
                .any(|func| function_requires_simd(func))
            || self
                .globals
                .iter()
                .any(|global| matches!(global.value_type(), ValueType::V128))
            || self
                .tags
                .iter()
                .any(|tag| function_type_requires_simd(tag.func_type()))
            || self.elements.iter().any(element_requires_simd)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Everything this module imports, in the order the binary declares it.
    ///
    /// The embedder needs this to check an import object before instantiating:
    /// an entry it cannot find is a link error, and the message has to name what
    /// is missing. The kinds are the four this engine links plus tags.
    pub fn imports(&self) -> collections::Vec<ImportDecl<'_>> {
        let mut imports = collections::Vec::new();
        for function in self.functions.iter() {
            if let FunctionDef::Import { module, name, .. } = &function.def {
                imports.push(ImportDecl {
                    module: module.as_str(),
                    name: name.as_str(),
                    kind: ImportKind::Func,
                });
            }
        }
        for table in self.tables.iter() {
            if let TableDef::Import { module, name, .. } = table.def() {
                imports.push(ImportDecl {
                    module: module.as_str(),
                    name: name.as_str(),
                    kind: ImportKind::Table,
                });
            }
        }
        for memory in self.memories.iter() {
            if let MemoryDef::Import { module, name, .. } = memory.def() {
                imports.push(ImportDecl {
                    module: module.as_str(),
                    name: name.as_str(),
                    kind: ImportKind::Memory,
                });
            }
        }
        for global in self.globals.iter() {
            if let GlobalDef::Import { module, name, .. } = global.def() {
                imports.push(ImportDecl {
                    module: module.as_str(),
                    name: name.as_str(),
                    kind: ImportKind::Global,
                });
            }
        }
        for tag in self.tags.iter() {
            if let TagDef::Import { module, name, .. } = tag.def() {
                imports.push(ImportDecl {
                    module: module.as_str(),
                    name: name.as_str(),
                    kind: ImportKind::Tag,
                });
            }
        }
        imports
    }

    pub(crate) fn version(&self) -> u32 {
        self.binary_version
    }

    pub(crate) fn types(&self) -> &type_context::TypeContext {
        &self.types
    }

    pub(crate) fn functions(&self) -> &[Function] {
        &self.functions
    }

    pub(crate) fn memories(&self) -> &[Memory] {
        &self.memories
    }

    pub(crate) fn tables(&self) -> &[Table] {
        &self.tables
    }

    pub(crate) fn globals(&self) -> &[Global] {
        &self.globals
    }

    pub(crate) fn tags(&self) -> &[Tag] {
        &self.tags
    }

    pub(crate) fn elements(&self) -> &[Element] {
        &self.elements
    }

    pub(crate) fn data(&self) -> &[Data] {
        &self.data
    }

    pub(crate) fn start_function_index(&self) -> Option<usize> {
        self.start_func_index
    }

    pub(crate) fn data_count(&self) -> Option<usize> {
        self.data_count
    }

    /// Functions whose identity may leave direct-call-only code.
    ///
    /// Exactly the module's declared-reference set: function indices found
    /// in element segments, exports, and table/global constant initializers.
    /// A valid module's code-section `ref.func` operands are required to be
    /// a subset of this set, so function bodies are never scanned here; the
    /// validator enforces that requirement with this same set.
    pub(crate) fn escapable_functions(&self) -> Result<collections::Vec<bool>, WasmError> {
        let mut escapable = collections::vec![false; self.functions.len()];

        for (index, function) in self.functions.iter().enumerate() {
            if !function.export_names().is_empty() {
                escapable[index] = true;
            }
        }

        for element in &self.elements {
            match element.get_init() {
                ElementInit::FunctionIndexes(indices) => {
                    for &index in indices {
                        mark_escapable(&mut escapable, index)?;
                    }
                }
                ElementInit::InitExprs { exprs, .. } => {
                    for expr in exprs {
                        scan_ref_funcs(expr, &mut escapable)?;
                    }
                }
            }
        }

        for table in &self.tables {
            if let Some(expr) = table.spec().init_expr() {
                scan_ref_funcs(expr, &mut escapable)?;
            }
        }
        for global in &self.globals {
            if let Some(spec) = global.spec() {
                scan_ref_funcs(spec.init_expr(), &mut escapable)?;
            }
        }

        Ok(escapable)
    }
}

fn mark_escapable(escapable: &mut [bool], index: usize) -> Result<(), WasmError> {
    let slot = escapable
        .get_mut(index)
        .ok_or_else(|| WasmError::invalid("ref.func: function index out of range"))?;
    *slot = true;
    Ok(())
}

struct RefFuncScan<'a> {
    escapable: &'a mut [bool],
}

impl OpcodeHandler for RefFuncScan<'_> {
    fn on_decode_begin(&mut self) -> Result<(), WasmError> {
        Ok(())
    }

    fn on_stream<'x, 'y, 'z>(
        &mut self,
        stream: &mut OpStream<'x, 'y, 'z>,
    ) -> Result<(), WasmError> {
        while let Some(decoded) = stream.next()? {
            if let (WasmOpcode::OP(Opcode::REF_FUNC), Immediate::FunctionIndex(function_index)) =
                (decoded.wasm_op, &decoded.imm)
            {
                mark_escapable(self.escapable, *function_index as usize)?;
            }
        }
        Ok(())
    }

    fn on_decode_end(&mut self) -> Result<(), WasmError> {
        Ok(())
    }
}

fn scan_ref_funcs(code: &[u8], escapable: &mut [bool]) -> Result<(), WasmError> {
    let mut scan = RefFuncScan { escapable };
    let mut decoder = Decoder::new(code);
    decoder.add_handler(&mut scan);
    decoder.decode_function()
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline]
fn function_type_requires_simd(func_type: &FunctionType) -> bool {
    func_type
        .params()
        .iter()
        .chain(func_type.results().iter())
        .any(|value_type| matches!(value_type, ValueType::V128))
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline]
fn storage_type_requires_simd(storage: &StorageType) -> bool {
    matches!(storage, StorageType::Val(ValueType::V128))
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline]
fn function_requires_simd(func: &Function) -> bool {
    function_type_requires_simd(func.func_type())
        || func.spec().is_some_and(|spec| {
            spec.locals()
                .iter()
                .any(|value_type| matches!(value_type, ValueType::V128))
        })
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline]
fn element_requires_simd(element: &Element) -> bool {
    matches!(
        element.get_init(),
        ElementInit::InitExprs {
            value_type: ValueType::V128,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::Module;

    #[test]
    fn parser_rejects_simd_value_types() {
        let wasm = wat::parse_str("(module (type (func (param v128))))")
            .expect("wat should encode a module with a v128 parameter");

        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            let err = Module::new("simd-types", &wasm)
                .expect_err("non-SIMD builds should reject v128 value types");
            assert_eq!(
                err,
                crate::WasmError::invalid("SIMD is not supported on this CPU")
            );
        }

        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        {
            Module::new("simd-types", &wasm)
                .expect("SIMD-enabled builds should accept v128 type syntax");
        }
    }
}

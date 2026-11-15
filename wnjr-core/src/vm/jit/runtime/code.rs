use tracked_alloc::{boxed::Box, sync::Arc};

use crate::collections;

use crate::{
    error::WasmError,
    vm::{
        jit::backend::BackendConfig,
        jit::machine::machine_ir::{
            MachineConstData, MachineConstId, MachineFuncId, MachineFunctionAbi, MachineModule,
            MachineModuleAbi,
        },
        jit::runtime::{context::NativeContext, dispatch_view::NativeDispatchMetadata},
    },
};

/// Native entry point: identical signature across all architectures.
pub(crate) type NativeRootEntry = unsafe extern "C" fn(*mut NativeContext, *mut u64) -> u32;

pub(crate) trait CodegenModuleView: core::fmt::Debug {
    fn backend(&self) -> BackendConfig;
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    fn memory0_is64(&self) -> Option<bool>;
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    fn memory0_is_shared(&self) -> Option<bool>;

    fn runtime_for(&self, id: MachineFuncId) -> Option<&MachineFunctionAbi>;
    fn const_ptr(&self, id: MachineConstId) -> Option<*const u8>;
}

#[derive(Clone, Debug)]
pub(crate) struct AlignedConstData {
    storage: Box<[u64]>,
}

impl AlignedConstData {
    pub(crate) fn new(record: &MachineConstData) -> Result<Self, WasmError> {
        if record.align as usize > core::mem::align_of::<u64>() {
            return Err(WasmError::internal(
                "machine const requires unsupported alignment",
            ));
        }

        let words = record.bytes.len().div_ceil(core::mem::size_of::<u64>());
        let mut storage = collections::vec![0u64; words.max(1)].into_boxed_slice();
        unsafe {
            core::ptr::copy_nonoverlapping(
                record.bytes.as_ptr(),
                storage.as_mut_ptr().cast::<u8>(),
                record.bytes.len(),
            );
        }
        Ok(Self { storage })
    }

    #[inline]
    pub(crate) fn as_ptr(&self) -> *const u8 {
        self.storage.as_ptr().cast::<u8>()
    }
}

/// Sizes the guard region under the wasm stack; only the guard-page runtime
/// consumes it.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[inline]
fn max_frame_bytes(abi: &MachineModuleAbi) -> usize {
    abi.functions
        .iter()
        .map(|function| usize::from(function.total_frame_slots) * core::mem::size_of::<u64>())
        .max()
        .unwrap_or(0)
}

/// One module's finished native code, shared by every instance of the module.
///
/// Written into `Module.code` once, after the CodeBuffer, const pool and
/// function-info table are in their final place; the first writer wins and
/// nothing overwrites it. Code is written only when its compile made no
/// import-dependent decision (see `JitInstance::cross_instance_inlined` and
/// `import_tag_static_catch`): code compiled without one is correct for any
/// binding of the imports. Reuse also requires the memory facts lowering read
/// from the instance to match the reusing instance.
#[derive(Clone, Debug)]
pub(crate) struct PrecompiledModule {
    pub(crate) compiled: Arc<CompiledNativeModule>,
    /// Root entry per module function index (None for imports).
    pub(crate) entries: collections::Vec<Option<NativeRootEntry>>,
    pub(crate) memory_facts: EffectiveMemoryFacts,
}

/// Memory facts that lowering reads from the instance, not the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectiveMemoryFacts {
    /// Bounds checks were left to guard pages.
    pub(crate) use_guard_pages: bool,
    /// The value `MachineModuleAbi::memory0_shared` was given.
    pub(crate) memory0_shared: Option<bool>,
}

#[derive(Debug)]
pub(crate) struct CompiledNativeModule {
    backend: BackendConfig,
    module: Option<MachineModule>,
    abi: Option<MachineModuleAbi>,
    aligned_consts: collections::Vec<AlignedConstData>,
    dispatch_metadata: NativeDispatchMetadata,
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    max_frame_bytes: usize,
    /// Owned executable CodeBuffer for the streaming compile path, shared by
    /// every instance of the module through `Arc<CompiledNativeModule>`. The
    /// batch path (IR-dump builds) still writes into `ModuleInst.native_buf`.
    /// Holding the finalized buffer here lets `FunctionSpec.native_code`
    /// entries point into code that outlives any single instance.
    code_buffer: Option<super::code_buf::CodeBuffer>,
}

// SAFETY: CompiledNativeModule is immutable after construction. All fields are
// Send (Vecs, Options, machine code, metadata) and the module is never mutated
// after the compilation phase, so it can be safely shared across threads in Arc.
unsafe impl Send for CompiledNativeModule {}
unsafe impl Sync for CompiledNativeModule {}

impl CompiledNativeModule {
    pub(crate) fn new(
        backend: BackendConfig,
        module: MachineModule,
        abi: MachineModuleAbi,
    ) -> Result<Self, WasmError> {
        if backend.is_32bit_gp_target() {
            module.validate_32bit_gp_target(backend.first_fp_reg())?;
        }
        let mut aligned_consts = collections::Vec::with_capacity(module.consts.len());
        for konst in &module.consts {
            aligned_consts.push(AlignedConstData::new(konst)?);
        }
        let dispatch_metadata = NativeDispatchMetadata::new(backend, &abi);
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        let max_frame_bytes = max_frame_bytes(&abi);
        Ok(Self {
            backend,
            module: Some(module),
            abi: Some(abi),
            aligned_consts,
            dispatch_metadata,
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            max_frame_bytes,
            code_buffer: None,
        })
    }

    pub(crate) fn from_streamed(
        backend: BackendConfig,
        abi: MachineModuleAbi,
        aligned_consts: collections::Vec<AlignedConstData>,
    ) -> Self {
        let dispatch_metadata = NativeDispatchMetadata::new(backend, &abi);
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        let max_frame_bytes = max_frame_bytes(&abi);
        Self {
            backend,
            module: None,
            abi: Some(abi),
            aligned_consts,
            dispatch_metadata,
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            max_frame_bytes,
            code_buffer: None,
        }
    }

    /// Hand the finalized executable buffer to this module. The buffer is
    /// then freed only when the last `Arc<CompiledNativeModule>` drops, so
    /// every instance that shares this code keeps the mapping alive.
    pub(crate) fn with_code_buffer(mut self, buf: super::code_buf::CodeBuffer) -> Self {
        self.code_buffer = Some(buf);
        self
    }

    #[inline]
    pub(crate) const fn backend(&self) -> BackendConfig {
        self.backend
    }

    #[inline]
    pub(crate) fn module(&self) -> &MachineModule {
        match &self.module {
            Some(module) => module,
            None => panic!("compiled machine module is not retained for this backend"),
        }
    }

    #[inline]
    pub(crate) fn abi(&self) -> &MachineModuleAbi {
        match &self.abi {
            Some(abi) => abi,
            None => panic!("compiled machine ABI is not retained for this backend"),
        }
    }

    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    #[inline]
    pub(crate) const fn max_frame_bytes(&self) -> usize {
        self.max_frame_bytes
    }

    #[inline]
    pub(crate) fn strip_machine_ir_for_runtime(&mut self) {
        self.module = None;
    }

    #[inline]
    pub(crate) fn const_ptr(&self, id: MachineConstId) -> Option<*const u8> {
        self.aligned_consts
            .get(id.0 as usize)
            .map(AlignedConstData::as_ptr)
    }

    #[inline]
    pub(crate) const fn dispatch_metadata(&self) -> &NativeDispatchMetadata {
        &self.dispatch_metadata
    }

    #[inline]
    pub(crate) fn publish_local_call_infos(&self, base: *const u8) {
        self.dispatch_metadata.local_call_infos().publish(base);
    }
}

impl CodegenModuleView for CompiledNativeModule {
    #[inline]
    fn backend(&self) -> BackendConfig {
        self.backend
    }

    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    fn memory0_is64(&self) -> Option<bool> {
        self.abi.as_ref().and_then(|abi| abi.memory0_is64)
    }

    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    fn memory0_is_shared(&self) -> Option<bool> {
        self.abi.as_ref().and_then(|abi| abi.memory0_shared)
    }

    #[inline]
    fn runtime_for(&self, id: MachineFuncId) -> Option<&MachineFunctionAbi> {
        self.abi
            .as_ref()
            .and_then(|abi| abi.functions.get(id.0 as usize))
    }

    #[inline]
    fn const_ptr(&self, id: MachineConstId) -> Option<*const u8> {
        CompiledNativeModule::const_ptr(self, id)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NativeCode {
    compiled: Arc<CompiledNativeModule>,
    func_id: MachineFuncId,
    entry: Option<NativeRootEntry>,
}

impl NativeCode {
    #[inline]
    pub(crate) fn new(compiled: Arc<CompiledNativeModule>, func_id: MachineFuncId) -> Self {
        Self {
            compiled,
            func_id,
            entry: None,
        }
    }

    #[inline]
    pub(crate) fn with_entry(mut self, entry: Option<NativeRootEntry>) -> Self {
        self.entry = entry;
        self
    }

    #[inline]
    pub(crate) const fn func_id(&self) -> MachineFuncId {
        self.func_id
    }

    #[inline]
    pub(crate) fn compiled(&self) -> &CompiledNativeModule {
        &self.compiled
    }

    #[cfg(test)]
    pub(crate) fn compiled_rc(&self) -> &Arc<CompiledNativeModule> {
        &self.compiled
    }

    #[inline]
    pub(crate) fn native_entry(&self) -> Option<NativeRootEntry> {
        self.entry
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NativeCodeCache {
    compiled: bool,
}

impl NativeCodeCache {
    #[inline]
    pub(crate) const fn is_compiled(self) -> bool {
        self.compiled
    }

    #[inline]
    pub(crate) const fn compiled() -> Self {
        Self { compiled: true }
    }
}

//! WebAssembly 2.0 Runtime Instances (no_std, single-module).

use crate::collections;
use crate::config::Config;

use alloc::rc::Rc as AllocRc;
use core::cell::{Cell, Ref, RefCell, RefMut, UnsafeCell};
use tracked_alloc::rc::Rc;
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
use tracked_alloc::sync::Arc;

use crate::error::WasmError;
use crate::utils::limits::Limits;
use crate::value_type::ValueType;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
use crate::vm::jit::runtime::guard_pages::GuardPageMemory;
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
use crate::vm::jit::shared_memory::SharedMemory;
use crate::vm::tag::TagIdentity;
use crate::vm::value::{RefValue, Value};

/// A non-capturing host function pointer.
///
/// Kept as a public alias for source compatibility with embedders that name or
/// cast plain host functions explicitly. [`Import::func`](crate::Import::func)
/// also accepts capturing [`Fn`] callbacks.
pub type HostFn = fn(&mut Caller, &[crate::Value], &mut [crate::Value]) -> Result<(), WasmError>;

type DynHostCallback =
    dyn for<'a> Fn(&mut Caller<'a>, &[Value], &mut [Value]) -> Result<(), WasmError>;

/// A clonable, potentially capturing host callback.
///
/// Instances borrow their import list during construction, so callbacks use
/// shared ownership when they are installed into a module. WNJR's
/// runtime is single-threaded; captured mutable state can use interior
/// mutability such as [`core::cell::Cell`] or [`core::cell::RefCell`].
#[derive(Clone)]
pub(crate) struct HostCallback {
    callback: Rc<DynHostCallback>,
}

impl HostCallback {
    pub(crate) fn from_host<F>(callback: F) -> Self
    where
        F: Fn(&mut Caller<'_>, &[crate::Value], &mut [crate::Value]) -> Result<(), WasmError>
            + 'static,
    {
        Self::new(move |caller, params, results| {
            let context = caller
                .context
                .ok_or_else(|| WasmError::internal("host callback has no context"))?;
            let mut converted = crate::value::ValueBuffer::new(params.len(), crate::Value::Unknown);
            for (dst, value) in converted.iter_mut().zip(params.iter().copied()) {
                *dst = crate::Value::from_vm(value, context.world());
            }
            let mut returned = crate::value::ValueBuffer::new(results.len(), crate::Value::Unknown);
            match callback(caller, &converted, &mut returned) {
                Ok(()) => {
                    for (dst, value) in results.iter_mut().zip(returned.iter().copied()) {
                        *dst = context
                            .import_value(value)
                            .map_err(|error| WasmError::trap(error.message()))?;
                    }
                    Ok(())
                }
                Err(WasmError {
                    repr: crate::error::ErrorRepr::HostThrowValues { tag, args },
                }) => {
                    let args = args
                        .into_iter()
                        .map(|value| context.import_value(value))
                        .collect::<Result<collections::Vec<_>, _>>()
                        .map_err(|error| WasmError::trap(error.message()))?;
                    Err(WasmError {
                        repr: crate::error::ErrorRepr::HostThrow { tag, args },
                    })
                }
                Err(error) => {
                    if let Some(exn) = error.exception() {
                        context
                            .import_value(crate::Value::Ref(
                                exn,
                                crate::value_type::RefType::exnref(),
                            ))
                            .map_err(|error| WasmError::trap(error.message()))?;
                    }
                    Err(error)
                }
            }
        })
    }
    pub(crate) fn new<F>(callback: F) -> Self
    where
        F: for<'a, 'b, 'c, 'd> Fn(
                &'a mut Caller<'b>,
                &'c [Value],
                &'d mut [Value],
            ) -> Result<(), WasmError>
            + 'static,
    {
        let callback: AllocRc<DynHostCallback> = AllocRc::new(callback);
        let callback: Rc<DynHostCallback> = callback.into();
        Self { callback }
    }

    #[inline]
    pub(crate) fn call(
        &self,
        caller: &mut Caller<'_>,
        params: &[Value],
        results: &mut [Value],
    ) -> Result<(), WasmError> {
        (self.callback)(caller, params, results)
    }
}

impl core::fmt::Debug for HostCallback {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HostCallback").finish_non_exhaustive()
    }
}

enum CallerMemory<'a> {
    Borrowed(&'a mut [u8]),
    Shared(MemInst),
}

pub(crate) trait HostValueContext {
    fn world(&self) -> usize;
    fn import_value(&self, value: crate::Value) -> Result<Value, WasmError>;
    fn validate_results(&self, values: &[Value], types: &[ValueType]) -> Result<(), WasmError>;
}

pub struct Caller<'a> {
    memory: Option<CallerMemory<'a>>,
    shared_borrow_active: Cell<bool>,
    context: Option<&'a dyn HostValueContext>,
}

impl<'a> Caller<'a> {
    #[inline]
    pub fn new(memory: Option<&'a mut [u8]>) -> Self {
        Self {
            memory: memory.map(CallerMemory::Borrowed),
            shared_borrow_active: Cell::new(false),
            context: None,
        }
    }

    #[inline]
    pub(crate) fn from_shared_memory(
        memory: Option<MemInst>,
        context: &'a dyn HostValueContext,
    ) -> Caller<'a> {
        Caller {
            memory: memory.map(CallerMemory::Shared),
            shared_borrow_active: Cell::new(false),
            context: Some(context),
        }
    }

    /// Resolve types only after the callback returns: no instance-body borrow
    /// may span arbitrary host code and a possible reentrant invocation.
    pub(crate) fn validate_results(
        &self,
        values: &[Value],
        types: &[ValueType],
    ) -> Result<(), WasmError> {
        let context = self
            .context
            .ok_or_else(|| WasmError::internal("host callback is missing its result checker"))?;
        context.validate_results(values, types)
    }

    fn begin_shared_borrow(&self) {
        if self.shared_borrow_active.get() {
            return;
        }
        let Some(CallerMemory::Shared(memory)) = self.memory.as_ref() else {
            return;
        };
        memory
            .begin_host_callback_borrow()
            .expect("host memory borrow must pass invocation preflight");
        self.shared_borrow_active.set(true);
    }

    #[inline]
    pub fn memory(&self) -> Option<&[u8]> {
        self.begin_shared_borrow();
        match self.memory.as_ref()? {
            CallerMemory::Borrowed(memory) => Some(memory),
            CallerMemory::Shared(memory) => {
                let ptr = memory.memory_ptr();
                let len = memory.memory_len();
                Some(unsafe { core::slice::from_raw_parts(ptr, len) })
            }
        }
    }

    #[inline]
    pub fn memory_mut(&mut self) -> Option<&mut [u8]> {
        self.begin_shared_borrow();
        match self.memory.as_mut()? {
            CallerMemory::Borrowed(memory) => Some(memory),
            CallerMemory::Shared(memory) => {
                let ptr = memory.memory_ptr();
                let len = memory.memory_len();
                Some(unsafe { core::slice::from_raw_parts_mut(ptr, len) })
            }
        }
    }

    /// Returns a Send-able handle to the current shared memory, if the caller
    /// has one. Returns None for non-shared memory or Borrowed memory.
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    pub fn shared_memory(&self) -> Option<crate::vm::entities::SharedMemHandle> {
        match &self.memory {
            Some(CallerMemory::Shared(mem_inst)) => {
                mem_inst
                    .shared
                    .as_ref()
                    .map(|shared| crate::vm::entities::SharedMemHandle {
                        shared: Arc::clone(shared),
                        limits: mem_inst.limits.clone(),
                    })
            }
            _ => None,
        }
    }

    /// Construct an exception from host code. Use as:
    ///
    /// ```rust
    /// use wnjr_core::{Caller, TagIdentity, Value, WasmError};
    ///
    /// fn throw_i32(tag: TagIdentity) -> Result<(), WasmError> {
    ///     Err(Caller::throw(tag, vec![Value::I32(42)]))
    /// }
    /// ```
    ///
    /// `tag` must be a live `TagIdentity` (obtained via
    /// `Instance::tag_identity(...)` or `Import::tag_typed_with_handle(...)`).
    /// Payload arity and value types must match the tag's function-type
    /// params; a mistyped throw surfaces to wasm as
    /// `WasmError::trap("host threw mistyped exception")`.
    ///
    /// The interpreter can catch host throws with `try_table`. The JIT
    /// currently propagates host throws to the embedder even when the caller
    /// has a matching handler; catching them across a call is not implemented.
    #[inline]
    pub fn throw(tag: TagIdentity, args: impl Into<collections::Vec<crate::Value>>) -> WasmError {
        WasmError {
            repr: crate::error::ErrorRepr::HostThrowValues {
                tag,
                args: args.into(),
            },
        }
    }
}

impl Drop for Caller<'_> {
    fn drop(&mut self) {
        if !self.shared_borrow_active.get() {
            return;
        }
        let Some(CallerMemory::Shared(memory)) = self.memory.as_ref() else {
            return;
        };
        memory.end_host_callback_borrow();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TableInst {
    pub(crate) elements: Rc<RefCell<collections::Vec<RefValue>>>,
    // pub(crate) so `vm::jit::entities::TableInstJit` can read it; the bump
    // in `elements_mut` is unconditional because a shared table mutated by
    // any engine must invalidate JIT cached views.
    pub(crate) revision: Rc<Cell<u64>>,
    pub(crate) limits: Limits,
    pub(crate) value_type: ValueType,
}

impl TableInst {
    pub(crate) fn current_limits(&self) -> Result<Limits, WasmError> {
        Ok(if self.limits.is64 {
            Limits::new_64(self.size(), self.limits.max())?
        } else {
            Limits::new(self.size(), self.limits.max())?
        })
    }

    pub(crate) fn new(limits: Limits, value_type: ValueType) -> Self {
        let initial_size = limits.min();
        TableInst {
            elements: Rc::new(RefCell::new(collections::vec![
                RefValue::null();
                initial_size
            ])),
            revision: Rc::new(Cell::new(0)),
            limits,
            value_type,
        }
    }

    #[inline]
    pub(crate) fn elements(&self) -> Ref<'_, collections::Vec<RefValue>> {
        self.elements.borrow()
    }

    #[inline]
    pub(crate) fn elements_mut(&self) -> RefMut<'_, collections::Vec<RefValue>> {
        self.revision.set(self.revision.get().wrapping_add(1));
        self.elements.borrow_mut()
    }

    #[inline]
    pub(crate) fn size(&self) -> usize {
        self.elements.borrow().len()
    }
}

#[derive(Debug)]
pub(crate) struct MemBacking {
    pub(crate) data: collections::Vec<u8>,
    pub(crate) host_callback_borrowed: Cell<bool>,
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    pub(crate) guard: Option<GuardPageMemory>,
}

#[derive(Debug, Clone)]
pub(crate) struct MemInst {
    pub(crate) backing: Rc<RefCell<MemBacking>>,
    pub(crate) limits: Limits,
    /// Arc-wrapped shared memory (threads proposal). When present, memory_ptr
    /// and memory_len read from here instead of backing.
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    pub(crate) shared: Option<Arc<SharedMemory>>,
}

/// A Send-able handle to a shared memory for cross-thread instance creation.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[derive(Clone)]
pub struct SharedMemHandle {
    pub(crate) shared: Arc<SharedMemory>,
    pub(crate) limits: Limits,
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
unsafe impl Send for SharedMemHandle {}
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
unsafe impl Sync for SharedMemHandle {}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
impl SharedMemHandle {
    /// Create a fresh shared memory from the given limits.
    ///
    /// The limits must carry the shared bit and declare a maximum: the whole
    /// maximum is reserved up front so the base pointer never moves and every
    /// thread that imports this handle sees the same bytes.
    pub fn new(limits: Limits) -> Result<Self, WasmError> {
        if !limits.is_shared() {
            return Err(WasmError::invalid(
                "SharedMemHandle requires limits with the shared flag",
            ));
        }
        Ok(Self {
            shared: Arc::new(SharedMemory::new(&limits)?),
            limits,
        })
    }

    /// The limits this memory was created with.
    pub fn limits(&self) -> Limits {
        self.limits
    }
}

/// Enforce the engine's `wasm_memory_max_pages` against the memory's
/// initial page count. Declared maximums are type-level growth ceilings;
/// `memory.grow` applies the runtime cap when growth is requested.
pub(crate) fn check_memory_quota(config: &Config, limits: &Limits) -> Result<(), WasmError> {
    let initial_pages = limits.min();
    let configured = config.get_wasm_memory_max_pages() as usize;
    if initial_pages > configured {
        return Err(WasmError::memory_exceeds_runtime_limit());
    }
    Ok(())
}

impl MemInst {
    pub(crate) fn current_limits(&self) -> Result<Limits, WasmError> {
        let pages = self.current_pages();
        let max = self.limits.max();
        Ok(match (self.limits.is64, self.limits.is_shared()) {
            (true, true) => {
                Limits::new_shared_64(pages, max.expect("shared memory declares a maximum"))?
            }
            (true, false) => Limits::new_64(pages, max)?,
            (false, true) => {
                Limits::new_shared(pages, max.expect("shared memory declares a maximum"))?
            }
            (false, false) => Limits::new(pages, max)?,
        })
    }

    /// Build a memory from shared limits. The reservation is fixed at max,
    /// so the resulting instance is the object other threads import.
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    pub(crate) fn new_shared(config: &Config, limits: Limits) -> Result<Self, WasmError> {
        check_memory_quota(config, &limits)?;
        debug_assert!(limits.is_shared(), "new_shared requires shared limits");
        let shared = Arc::new(SharedMemory::new(&limits)?);
        Ok(MemInst {
            backing: Rc::new(RefCell::new(MemBacking {
                data: collections::Vec::new(),
                host_callback_borrowed: Cell::new(false),
                guard: None,
            })),
            limits,
            shared: Some(shared),
        })
    }

    /// The shared reservation, when this memory is shared.
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    #[inline]
    pub(crate) fn shared_memory(&self) -> Option<&Arc<SharedMemory>> {
        self.shared.as_ref()
    }

    /// Reconstruct a MemInst from a SharedMemHandle for importing into a new instance.
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    pub(crate) fn from_shared_handle(h: crate::vm::entities::SharedMemHandle) -> Self {
        MemInst {
            backing: Rc::new(RefCell::new(MemBacking {
                data: collections::Vec::new(),
                host_callback_borrowed: Cell::new(false),
                guard: None,
            })),
            limits: h.limits,
            shared: Some(h.shared),
        }
    }

    /// Allocate with guard-page backing (mmap + PROT_NONE guard region).
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    pub(crate) fn new_guarded(config: &Config, limits: Limits) -> Result<Self, WasmError> {
        check_memory_quota(config, &limits)?;
        let guard = GuardPageMemory::new(limits.min())?;
        Ok(MemInst {
            backing: Rc::new(RefCell::new(MemBacking {
                data: collections::Vec::new(),
                host_callback_borrowed: Cell::new(false),
                guard: Some(guard),
            })),
            limits,
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            shared: None,
        })
    }

    #[inline]
    pub(crate) fn backing_mut(&self) -> RefMut<'_, MemBacking> {
        self.backing.borrow_mut()
    }

    #[inline]
    pub(crate) fn host_callback_borrowed(&self) -> bool {
        self.backing.borrow().host_callback_borrowed.get()
    }

    pub(crate) fn begin_host_callback_borrow(&self) -> Result<(), WasmError> {
        let backing = self.backing.borrow();
        if backing.host_callback_borrowed.replace(true) {
            return Err(WasmError::trap(
                "linear memory is already borrowed by a host callback",
            ));
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn end_host_callback_borrow(&self) {
        self.backing.borrow().host_callback_borrowed.set(false);
    }

    #[inline]
    pub(crate) fn current_pages(&self) -> usize {
        self.memory_len() / crate::constants::WASM_PAGE_SIZE
    }

    /// Pointer to the memory buffer (works for Vec, guard-page, and shared backing).
    #[inline]
    pub(crate) fn memory_ptr(&self) -> *mut u8 {
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if let Some(ref shared) = self.shared {
            return shared.base();
        }
        let backing = self.backing.borrow();
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if let Some(ref g) = backing.guard {
            return g.base();
        }
        backing.data.as_ptr() as *mut u8
    }

    /// Current committed size in bytes.
    #[inline]
    pub(crate) fn memory_len(&self) -> usize {
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if let Some(ref shared) = self.shared {
            return shared.committed_bytes();
        }
        let backing = self.backing.borrow();
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if let Some(ref g) = backing.guard {
            return g.len();
        }
        backing.data.len()
    }
}

#[derive(Debug)]
pub(crate) struct GlobalCell {
    raw: UnsafeCell<u64>,
}

impl GlobalCell {
    #[inline]
    fn new(raw: u64) -> Self {
        Self {
            raw: UnsafeCell::new(raw),
        }
    }

    #[inline]
    pub(crate) fn raw_ptr(&self) -> *mut u64 {
        self.raw.get()
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub(crate) struct GlobalInst {
    pub(crate) raw_ptr: *mut u64,
    pub(crate) cell: Rc<GlobalCell>,
    pub(crate) mutable: bool,
    pub(crate) value_type: ValueType,
}

impl GlobalInst {
    pub(crate) fn new_raw(raw: u64, mutable: bool, value_type: ValueType) -> Self {
        let cell = Rc::new(GlobalCell::new(raw));
        let raw_ptr = cell.raw_ptr();
        GlobalInst {
            raw_ptr,
            cell,
            mutable,
            value_type,
        }
    }

    #[inline]
    pub(crate) fn raw(&self) -> u64 {
        // Safety: wnjr stores are single-threaded; generated code and host
        // API access use the same raw cell identity for imported globals.
        unsafe { *self.raw_ptr }
    }

    #[inline]
    pub(crate) fn set_raw(&mut self, raw: u64) {
        // Safety: see `raw`.
        unsafe {
            *self.raw_ptr = raw;
        }
    }

    #[inline]
    pub(crate) fn raw_ptr(&self) -> *mut u64 {
        self.raw_ptr
    }
}

//! Host handles to wasm linear memories.
//!
//! A [`MemHandle`] is how an embedder owns a wasm memory from the outside: it
//! keeps the reservation alive, reports the base and length, and can grow the
//! memory the same way the `memory.grow` instruction does. It exists only for
//! guard-page memories -- a heap-backed memory reallocates on grow, so a pointer
//! into it stops being the base, and handing one out would be a lie.

use tracked_alloc::sync::Arc;

use crate::constants::{MAX_MEM_PAGES, WASM_PAGE_SIZE};
use crate::error::WasmError;
use crate::utils::limits::Limits;
use crate::vm::entities::MemInst;
use crate::vm::jit::runtime::guard_pages::{GrowListener, GuardPageMemory};

/// A `Send + Sync` owner of one wasm linear memory.
///
/// Cloning shares the memory: same base, same length, same id, one reservation
/// released when the last clone and the last instance using it are gone.
pub struct MemHandle {
    guard: Arc<GuardPageMemory>,
    id: u64,
    limits: Limits,
}

impl Clone for MemHandle {
    fn clone(&self) -> Self {
        Self {
            guard: Arc::clone(&self.guard),
            id: self.id,
            limits: self.limits,
        }
    }
}

impl MemHandle {
    /// Create a memory with `initial_pages` committed and `max_pages` as its
    /// declared ceiling.
    ///
    /// The whole address space is reserved up front (8 GB + 64 KB on the native
    /// backends), so the base never moves and growing never copies bytes.
    pub fn new(initial_pages: usize, max_pages: Option<usize>) -> Result<Self, WasmError> {
        let limits = Limits::new(initial_pages, max_pages)?;
        Ok(Self::from_guard(
            Arc::new(GuardPageMemory::new(initial_pages)?),
            crate::vm::entities::next_memory_id(),
            limits,
        ))
    }

    /// Create a shared memory: `SharedArrayBuffer` semantics, atomics allowed.
    ///
    /// Every thread that imports the handle sees the same bytes. The whole
    /// maximum is reserved up front, so the base never moves.
    pub fn new_shared(initial_pages: usize, max_pages: usize) -> Result<Self, WasmError> {
        let limits = Limits::new_shared(initial_pages, max_pages)?;
        Ok(Self::from_guard(
            Arc::new(GuardPageMemory::new(initial_pages)?),
            crate::vm::entities::next_memory_id(),
            limits,
        ))
    }

    pub(crate) fn from_guard(guard: Arc<GuardPageMemory>, id: u64, limits: Limits) -> Self {
        Self { guard, id, limits }
    }

    /// The address of byte 0. Stable for the life of the handle: growing
    /// commits pages behind it instead of moving it.
    #[inline]
    pub fn base(&self) -> *mut u8 {
        self.guard.base()
    }

    /// Current length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.guard.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.guard.len() == 0
    }

    /// The largest length the memory may reach, in bytes.
    ///
    /// This is the declared maximum when the memory has one, and the address
    /// space ceiling for its index type otherwise -- what
    /// `WebAssembly.Memory.prototype.buffer.maxByteLength` reports.
    #[inline]
    pub fn capacity(&self) -> usize {
        let pages = match self.limits.max() {
            Some(max) => max,
            None if self.limits.is64 => usize::MAX / WASM_PAGE_SIZE,
            None => MAX_MEM_PAGES,
        };
        pages
            .saturating_mul(WASM_PAGE_SIZE)
            .min(self.guard.reservation_size())
    }

    /// Identity of this memory, unique for the process lifetime.
    ///
    /// Shared with every instance that imports the memory, so an embedder can
    /// key an invalidation flag or a wait queue on it without hashing addresses.
    #[inline]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Grow by `delta_pages`, returning the previous page count.
    ///
    /// Behaves like `memory.grow`: grows up to [`Self::capacity`] and re-applies
    /// the same ceiling, so a host cannot push a memory past what the module
    /// declared. The base does not move and no byte is copied or zeroed twice:
    /// the pages committed here are fresh anonymous pages.
    pub fn grow(&self, delta_pages: usize) -> Result<usize, WasmError> {
        let old_pages = self.len() / WASM_PAGE_SIZE;
        let cap_pages = self.capacity() / WASM_PAGE_SIZE;
        let new_pages = old_pages
            .checked_add(delta_pages)
            .ok_or_else(|| WasmError::invalid("memory grow overflow"))?;
        if new_pages > cap_pages {
            return Err(WasmError::invalid(
                "memory grow exceeds the declared maximum",
            ));
        }
        let old = self.guard.grow(delta_pages)?;
        self.guard.notify_grow(self.id);
        Ok(old)
    }

    /// Register what to run after a successful grow, replacing any earlier one.
    ///
    /// The callback runs on the thread that grew the memory while the engine
    /// still holds the instance, so it must only record that the view is stale
    /// and return. A failed grow never calls it.
    pub fn set_grow_listener(&self, listener: Option<alloc::boxed::Box<GrowListener>>) {
        self.guard.state().set_on_grow(listener);
    }

    /// The memory as an instance sees it, for binding to an import.
    pub(crate) fn to_mem_inst(&self) -> Result<MemInst, WasmError> {
        MemInst::from_guarded(self.limits, Arc::clone(&self.guard), self.id)
    }
}

impl core::fmt::Debug for MemHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MemHandle")
            .field("id", &self.id)
            .field("base", &self.base())
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

//! Guard-page memory allocator for the native JIT backend.
//!
//! Reserves a large virtual address region (8 GB + 64 KB) per wasm linear
//! memory. The committed (RW) region tracks the current memory size; the
//! remainder is mapped PROT_NONE so that any out-of-bounds access faults
//! into the guard region. A signal handler converts the fault to a wasm trap
//! without explicit per-access bounds checks in JIT code.
//!
//! This module is gated on `#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]`. All OS coupling
//! — reservation, per-page commit, release — is delegated to
//! [`crate::vm::jit::runtime::os`].

use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use tracked_alloc::sync::Arc;

use crate::error::WasmError;

use super::os;
#[cfg(debug_assertions)]
use tracked_alloc::{
    AllocationDescriptor, AllocationHandle, AllocationState, RUNTIME_MEMORY_OWNER,
};

const WASM_PAGE_SIZE: usize = crate::constants::WASM_PAGE_SIZE;

/// Total virtual reservation per memory: 8 GB + 64 KB.
///
/// wasm32 max address (2^32 - 1) + max offset (2^32 - 1) + max access (16)
/// fits within this range. No OOB access can escape the guard region.
const GUARD_RESERVATION: usize = 8 * 1024 * 1024 * 1024 + 64 * 1024;

/// Minimum guard range after the usable wasm stack.
///
/// The stack guard is also expanded by the module's largest frame footprint
/// so a single overflowing callee-frame probe cannot jump beyond the protected
/// reservation.
const STACK_GUARD_MIN_BYTES: usize = WASM_PAGE_SIZE;

#[inline]
const fn align_up(value: usize, align: usize) -> usize {
    value.div_ceil(align) * align
}

/// The OS reservation a guarded memory lives in.
///
/// Owned through an `Arc` so that a host handle keeps the reservation alive
/// after the instance that created it is gone: the mapping is released when the
/// last owner drops, not when the instance does.
pub(crate) struct Reservation {
    base: *mut u8,
    size: usize,
}

// SAFETY: the reservation is a process-private mapping of zeroed pages. Wasm
// touches it only from the thread that owns the instance; an embedder reaches
// it as a raw address plus atomics, never as a Rust reference.
unsafe impl Send for Reservation {}
unsafe impl Sync for Reservation {}

impl Reservation {
    #[inline]
    pub(crate) fn base(&self) -> *mut u8 {
        self.base
    }

    #[inline]
    pub(crate) fn size(&self) -> usize {
        self.size
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        os::release_guarded(self.base, self.size);
    }
}

/// Called with the memory's id and its new length in bytes after a grow.
///
/// It runs on the thread that grew the memory, while the engine holds the
/// instance, so it must not call back into the engine. Recording the new length
/// and returning is the intended use.
pub type GrowListener = dyn Fn(u64, usize) + Send + Sync;

/// The part of a guarded memory that more than one owner reads: the committed
/// length and the grow callback.
///
/// The engine writes the length as it commits pages. A host handle reads it to
/// answer "how long is this memory now", which is why it is an atomic and not a
/// plain field: the handle is `Send + Sync` even though wasm runs on one
/// thread.
pub(crate) struct GuardState {
    committed: AtomicUsize,
    /// A `Box<GrowListener>` as a raw pointer: `AtomicPtr` holds sized types, and
    /// a boxed trait object is the sized handle to a callback of unknown type.
    on_grow: AtomicPtr<alloc::boxed::Box<GrowListener>>,
}

impl GuardState {
    fn new(committed: usize) -> Self {
        Self {
            committed: AtomicUsize::new(committed),
            on_grow: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    #[inline]
    pub(crate) fn committed(&self) -> usize {
        self.committed.load(Ordering::Acquire)
    }

    /// Install the grow callback, replacing any earlier one.
    ///
    /// The earlier callback is leaked rather than freed: a grow running on
    /// another thread may still be inside it, and this is not a hot path.
    /// Registering once per memory, before the instance runs, leaks nothing.
    pub(crate) fn set_on_grow(&self, listener: Option<alloc::boxed::Box<GrowListener>>) {
        let raw = match listener {
            Some(cb) => alloc::boxed::Box::into_raw(alloc::boxed::Box::new(cb)),
            None => core::ptr::null_mut(),
        };
        self.on_grow.store(raw, Ordering::Release);
    }

    fn fire(&self, id: u64, bytes: usize) {
        let raw = self.on_grow.load(Ordering::Acquire);
        if raw.is_null() {
            return;
        }
        // SAFETY: the pointer came from `Box::into_raw` in `set_on_grow` and is
        // never freed -- a replaced listener is leaked -- and the pointee is a
        // `Box<dyn Fn + Send + Sync>`, so it may be called from any thread.
        let listener: &GrowListener = unsafe { &**raw };
        listener(id, bytes);
    }
}

/// A wasm linear memory backed by an OS reservation with guard pages.
///
/// The base pointer is stable for the lifetime of the allocation (no
/// reallocation on grow). Only the committed size changes.
pub(crate) struct GuardPageMemory {
    reservation: Arc<Reservation>,
    state: Arc<GuardState>,
    #[cfg(debug_assertions)]
    trace: core::cell::RefCell<AllocationHandle>,
}

// SAFETY: the reservation is process-private and not aliased; `state` is the
// atomic half of the memory, meant to be shared.
unsafe impl Send for GuardPageMemory {}
unsafe impl Sync for GuardPageMemory {}

impl GuardPageMemory {
    /// Allocate a guarded memory with `initial_pages` committed.
    pub(crate) fn new(initial_pages: usize) -> Result<Self, WasmError> {
        let initial_bytes = initial_pages * WASM_PAGE_SIZE;
        if initial_bytes > GUARD_RESERVATION {
            return Err(WasmError::internal(
                "guard-page memory: initial size exceeds reservation".into(),
            ));
        }

        let base =
            os::reserve_guarded(GUARD_RESERVATION).map_err(|msg| WasmError::internal(msg))?;
        let reservation = Arc::new(Reservation {
            base,
            size: GUARD_RESERVATION,
        });

        if initial_bytes > 0 {
            if let Err(msg) = os::commit_guarded(base, 0, initial_bytes) {
                return Err(WasmError::internal(msg));
            }
        }

        Ok(Self {
            reservation,
            state: Arc::new(GuardState::new(initial_bytes)),
            #[cfg(debug_assertions)]
            trace: core::cell::RefCell::new(AllocationHandle::new(
                AllocationDescriptor::new(RUNTIME_MEMORY_OWNER, "GuardPageMemory"),
                AllocationState::new(initial_bytes)
                    .with_len(initial_bytes)
                    .with_capacity(GUARD_RESERVATION)
                    .with_ptr(base as usize),
            )),
        })
    }

    #[cfg(debug_assertions)]
    #[inline]
    fn trace_state(&self) -> AllocationState {
        AllocationState::new(self.len())
            .with_len(self.len())
            .with_capacity(GUARD_RESERVATION)
            .with_ptr(self.base() as usize)
    }

    /// Grow by `delta_pages`. Returns the old size in pages.
    ///
    /// Commits pages and only then publishes the new length, so a reader never
    /// sees a length whose pages are not committed yet. A failure leaves the
    /// length alone and fires no callback.
    pub(crate) fn grow(&self, delta_pages: usize) -> Result<usize, WasmError> {
        let committed = self.len();
        let old_pages = committed / WASM_PAGE_SIZE;
        let new_bytes = committed
            .checked_add(delta_pages * WASM_PAGE_SIZE)
            .ok_or_else(|| WasmError::internal("guard-page memory: grow overflow"))?;
        if new_bytes > GUARD_RESERVATION {
            return Err(WasmError::internal(
                "guard-page memory: grow exceeds reservation".into(),
            ));
        }

        if new_bytes > committed {
            os::commit_guarded(self.base(), committed, new_bytes - committed)
                .map_err(|msg| WasmError::internal(msg))?;
            self.state.committed.store(new_bytes, Ordering::Release);
        }
        #[cfg(debug_assertions)]
        self.trace.borrow_mut().update(self.trace_state());
        Ok(old_pages)
    }

    /// The grow callback shared with host handles.
    #[inline]
    pub(crate) fn state(&self) -> &Arc<GuardState> {
        &self.state
    }

    /// Announce a successful grow to the embedder, if one registered.
    ///
    /// Called by the engine after the length has been published. Separate from
    /// [`Self::grow`] so a caller can widen the commit and notify as one step,
    /// and so a failed grow has nothing to announce.
    #[inline]
    pub(crate) fn notify_grow(&self, id: u64) {
        self.state.fire(id, self.len());
    }

    #[inline]
    pub(crate) fn base(&self) -> *mut u8 {
        self.reservation.base()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.state.committed()
    }

    /// Bytes the reservation can ever commit, fixed at creation.
    #[inline]
    pub(crate) fn reservation_size(&self) -> usize {
        self.reservation.size()
    }
}

impl Drop for GuardPageMemory {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        self.trace.borrow_mut().remove();
        // The reservation itself is released by `Reservation::drop`, when the
        // last owner -- this instance or a host handle -- lets go of it.
    }
}

impl core::fmt::Debug for GuardPageMemory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GuardPageMemory")
            .field("base", &self.base())
            .field("committed", &self.len())
            .finish()
    }
}

/// Native wasm-value stack backed by an OS reservation with a guard range
/// immediately above the usable stack bytes.
pub(crate) struct GuardPageStack {
    base: *mut u8,
    usable: usize,
    guard: usize,
    #[cfg(debug_assertions)]
    trace: AllocationHandle,
}

// SAFETY: The reserved region is process-private and not aliased.
unsafe impl Send for GuardPageStack {}

impl GuardPageStack {
    pub(crate) fn new(requested_bytes: usize, max_frame_bytes: usize) -> Result<Self, WasmError> {
        let usable = align_up(
            requested_bytes.max(core::mem::size_of::<u64>()),
            WASM_PAGE_SIZE,
        );
        let guard = align_up(max_frame_bytes.max(STACK_GUARD_MIN_BYTES), WASM_PAGE_SIZE);
        let total = usable
            .checked_add(guard)
            .ok_or_else(|| WasmError::internal("guard-page stack: reservation overflow"))?;

        let base = os::reserve_guarded(total).map_err(|msg| WasmError::internal(msg))?;
        if let Err(msg) = os::commit_guarded(base, 0, usable) {
            os::release_guarded(base, total);
            return Err(WasmError::internal(msg));
        }

        Ok(Self {
            base,
            usable,
            guard,
            #[cfg(debug_assertions)]
            trace: AllocationHandle::new(
                AllocationDescriptor::new(RUNTIME_MEMORY_OWNER, "GuardPageStack"),
                AllocationState::new(usable)
                    .with_len(usable)
                    .with_capacity(total)
                    .with_ptr(base as usize),
            ),
        })
    }

    #[inline]
    pub(crate) fn base(&self) -> *mut u64 {
        self.base.cast()
    }

    #[inline]
    pub(crate) fn end(&self) -> *mut u64 {
        unsafe { self.base.add(self.usable).cast() }
    }

    #[inline]
    pub(crate) fn guard_end(&self) -> *mut u8 {
        unsafe { self.base.add(self.usable + self.guard) }
    }

    #[inline]
    pub(crate) fn supports(&self, requested_bytes: usize, max_frame_bytes: usize) -> bool {
        self.usable >= requested_bytes.max(core::mem::size_of::<u64>())
            && self.guard >= max_frame_bytes.max(STACK_GUARD_MIN_BYTES)
    }
}

impl Drop for GuardPageStack {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        self.trace.remove();
        os::release_guarded(self.base, self.usable + self.guard);
    }
}

impl core::fmt::Debug for GuardPageStack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GuardPageStack")
            .field("base", &self.base)
            .field("usable", &self.usable)
            .field("guard", &self.guard)
            .finish()
    }
}

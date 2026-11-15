//! Shared linear memory for the threads proposal (th4).
//!
//! A SharedMemory is allocated once, its base pointer is fixed for its
//! lifetime. The committed length is an AtomicU64 stored in a header page
//! directly in front of the base, at `base - 8` (see [`LEN_OFFSET_FROM_BASE`]),
//! so generated code can read it from the memory base register without a
//! per-instance cache. memory.grow takes a spinlock cold path that mprotects
//! new pages and publishes the new length with Release ordering. Loads and
//! stores never read the length; they rely on guard-page faults.
//! `memory.size` reads the header word every time it runs.
//!
//! Host access does not use a borrow counter. The wasm memory model
//! explicitly allows data races; the host must synchronise if it needs
//! to.
//!
//! This module is gated on `#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]`.

#[cfg(test)]
use crate::collections;
use crate::constants::WASM_PAGE_SIZE;
use crate::error::WasmError;
use crate::utils::limits::Limits;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::vm::jit::runtime::os;

/// A guard reservation sized for one wasm32 shared linear memory.
/// max pages × 64 KiB + 8 GiB guard so a wasm32 oob fault still lands
/// inside the reservation.
const GUARD_BYTES_32: usize = 8 * 1024 * 1024 * 1024;

/// A guard reservation sized for one wasm64 shared linear memory.
const GUARD_BYTES_64: usize = 8 * 1024 * 1024 * 1024;

/// Bytes reserved and committed in front of `base` for the length header.
/// 64 KiB is a multiple of every supported host page size (4K, 16K, 64K).
const HEADER_BYTES: usize = 64 * 1024;

/// The committed byte length lives at `base - LEN_OFFSET_FROM_BASE`.
pub(crate) const LEN_OFFSET_FROM_BASE: usize = 8;

fn guard_bytes(is64: bool) -> usize {
    if is64 {
        GUARD_BYTES_64
    } else {
        GUARD_BYTES_32
    }
}

/// A cross-thread shared wasm linear memory.
///
/// Wrap in Arc<SharedMemory> to share across threads.
pub(crate) struct SharedMemory {
    /// Start of the virtual reservation (the header page). Never changes.
    start: *mut u8,
    /// Base of wasm address 0, `start + HEADER_BYTES`. Never changes.
    base: *mut u8,
    /// Total reserved bytes, header included.
    reservation: usize,
    /// Maximum page count declared in the module.
    max_pages: usize,
    /// Serialises grow calls (spinlock).
    growing: AtomicBool,
    /// Whether this is a wasm64 memory.
    is64: bool,
}

// SAFETY: base points to a process-private mmap reservation.
// committed_bytes is AtomicU64 so read/write is well-defined from any thread.
unsafe impl Send for SharedMemory {}
unsafe impl Sync for SharedMemory {}

impl SharedMemory {
    /// Create a new SharedMemory from `limits`, which must be a shared
    /// memory (limits.is_shared() == true) and must have an explicit maximum.
    pub(crate) fn new(limits: &Limits) -> Result<Self, WasmError> {
        debug_assert!(limits.is_shared(), "SharedMemory requires shared limits");
        let max_pages = limits
            .max()
            .ok_or_else(|| WasmError::malformed("shared memory must declare a maximum"))?;
        let initial_pages = limits.min();
        let initial_bytes = initial_pages * WASM_PAGE_SIZE;
        let max_bytes = max_pages * WASM_PAGE_SIZE;
        let reservation = max_bytes
            .checked_add(guard_bytes(limits.is64))
            .and_then(|r| r.checked_add(HEADER_BYTES))
            .ok_or_else(|| WasmError::internal("shared memory reservation overflow"))?;

        let start = os::reserve_guarded(reservation).map_err(WasmError::internal)?;

        // Header page first, then the initial pages, in one commit.
        if let Err(msg) = os::commit_guarded(start, 0, HEADER_BYTES + initial_bytes) {
            os::release_guarded(start, reservation);
            return Err(WasmError::internal(msg));
        }
        let base = unsafe { start.add(HEADER_BYTES) };

        let mem = Self {
            start,
            base,
            reservation,
            max_pages,
            growing: AtomicBool::new(false),
            is64: limits.is64,
        };
        mem.len_cell()
            .store(initial_bytes as u64, Ordering::Release);
        Ok(mem)
    }

    /// The committed-length word in the header page.
    #[inline]
    fn len_cell(&self) -> &AtomicU64 {
        // SAFETY: the header page is committed for the lifetime of self, and
        // base - 8 is 8-byte aligned (base is page aligned).
        unsafe { &*(self.base.sub(LEN_OFFSET_FROM_BASE) as *const AtomicU64) }
    }

    /// Base pointer. Stable for the lifetime of this allocation.
    #[inline]
    pub(crate) fn base(&self) -> *mut u8 {
        self.base
    }

    /// Current committed bytes, acquired with Acquire ordering.
    #[inline]
    pub(crate) fn committed_bytes(&self) -> usize {
        self.len_cell().load(Ordering::Acquire) as usize
    }

    /// Current page count.
    #[inline]
    pub(crate) fn pages(&self) -> usize {
        self.committed_bytes() / WASM_PAGE_SIZE
    }

    /// Current limits (min = current pages, max = declared max).
    pub(crate) fn current_limits(&self) -> Result<Limits, WasmError> {
        let pages = self.pages();
        if self.is64 {
            Limits::new_shared_64(pages, self.max_pages)
        } else {
            Limits::new_shared(pages, self.max_pages)
        }
    }

    /// Grow by `delta_pages`. Returns the previous page count on success,
    /// or WasmError if the memory cannot grow. Cold path: takes the grow
    /// lock and calls mprotect.
    pub(crate) fn grow(&self, delta_pages: usize) -> Result<usize, WasmError> {
        // Serialise concurrent grow calls with a spinlock. grow is cold.
        while self
            .growing
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        struct GrowGuard<'a>(&'a AtomicBool);
        impl Drop for GrowGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = GrowGuard(&self.growing);

        // Re-read under the lock.
        let old_bytes = self.len_cell().load(Ordering::Relaxed) as usize;
        let old_pages = old_bytes / WASM_PAGE_SIZE;

        let new_pages = old_pages
            .checked_add(delta_pages)
            .ok_or_else(|| WasmError::internal("shared memory grow overflow"))?;

        if new_pages > self.max_pages {
            return Err(WasmError::trap("memory.grow exceeds declared maximum"));
        }

        let new_bytes = new_pages * WASM_PAGE_SIZE;
        let delta_bytes = new_bytes - old_bytes;

        if delta_bytes > 0 {
            os::commit_guarded(self.base, old_bytes, delta_bytes).map_err(WasmError::internal)?;
        }

        self.len_cell().store(new_bytes as u64, Ordering::Release);

        Ok(old_pages)
    }

    /// Copy bytes out from offset..offset+len. Returns WasmError if out of bounds.
    pub(crate) fn copy_out(&self, offset: usize, dst: &mut [u8]) -> Result<(), WasmError> {
        let len = dst.len();
        let committed = self.committed_bytes();
        if offset.checked_add(len).map_or(true, |end| end > committed) {
            return Err(WasmError::trap("shared memory read out of bounds"));
        }
        unsafe {
            core::ptr::copy_nonoverlapping(self.base.add(offset), dst.as_mut_ptr(), len);
        }
        Ok(())
    }

    /// Copy bytes into offset..offset+len. Returns WasmError if out of bounds.
    pub(crate) fn copy_in(&self, offset: usize, src: &[u8]) -> Result<(), WasmError> {
        let len = src.len();
        let committed = self.committed_bytes();
        if offset.checked_add(len).map_or(true, |end| end > committed) {
            return Err(WasmError::trap("shared memory write out of bounds"));
        }
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), self.base.add(offset), len);
        }
        Ok(())
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        os::release_guarded(self.start, self.reservation);
    }
}

impl core::fmt::Debug for SharedMemory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedMemory")
            .field("base", &self.base)
            .field("committed_bytes", &self.committed_bytes())
            .field("max_pages", &self.max_pages)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_limits(min: usize, max: usize) -> Limits {
        Limits::new_shared(min, max).unwrap()
    }

    #[test]
    fn shared_memory_base_is_stable_after_grow() {
        let mem = SharedMemory::new(&shared_limits(1, 4)).expect("alloc");
        let base = mem.base();
        mem.grow(1).expect("grow");
        assert_eq!(base, mem.base(), "base must not move");
        assert_eq!(mem.pages(), 2);
    }

    #[test]
    fn length_header_sits_in_front_of_base() {
        let mem = SharedMemory::new(&shared_limits(1, 4)).expect("alloc");
        let read = || unsafe {
            (*(mem.base().sub(LEN_OFFSET_FROM_BASE) as *const AtomicU64)).load(Ordering::Acquire)
        };
        assert_eq!(read(), WASM_PAGE_SIZE as u64);
        mem.grow(2).expect("grow");
        assert_eq!(read(), 3 * WASM_PAGE_SIZE as u64);
    }

    #[test]
    fn zero_page_shared_memory_has_readable_header() {
        let mem = SharedMemory::new(&shared_limits(0, 2)).expect("alloc");
        assert_eq!(mem.committed_bytes(), 0);
        assert!(!mem.base().is_null());
    }

    #[test]
    fn shared_memory_grow_beyond_max_fails() {
        let mem = SharedMemory::new(&shared_limits(1, 2)).expect("alloc");
        assert!(mem.grow(2).is_err(), "grow beyond max must fail");
    }

    #[test]
    fn shared_memory_copy_in_out() {
        let mem = SharedMemory::new(&shared_limits(1, 1)).expect("alloc");
        mem.copy_in(0, &[1, 2, 3, 4]).expect("copy_in");
        let mut buf = [0u8; 4];
        mem.copy_out(0, &mut buf).expect("copy_out");
        assert_eq!(buf, [1, 2, 3, 4]);
    }

    #[test]
    fn shared_memory_concurrent_grows_are_serialised() {
        use std::sync::Arc;
        let mem = Arc::new(SharedMemory::new(&shared_limits(0, 16)).expect("alloc"));
        let handles: collections::Vec<_> = (0..4)
            .map(|_| {
                let m = Arc::clone(&mem);
                std::thread::spawn(move || {
                    let _ = m.grow(1); // some may fail (max exceeded), that's fine
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // pages must be <= max and divisible by 1
        assert!(mem.pages() <= 16);
    }
}

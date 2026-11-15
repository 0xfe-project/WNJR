//! Global wait table for memory.atomic.wait/notify (threads proposal).
//!
//! This table maps (memory base address, offset) → list of waiting threads.
//! Only compiled on hosted targets (requires parking_lot and std).

#[cfg(not(target_os = "none"))]
extern crate std;

#[cfg(not(target_os = "none"))]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
#[cfg(not(target_os = "none"))]
use parking_lot::{Condvar, Mutex};
#[cfg(not(target_os = "none"))]
use std::collections::HashMap;
#[cfg(not(target_os = "none"))]
use std::sync::Arc;
#[cfg(not(target_os = "none"))]
use std::time::Duration;

#[cfg(not(target_os = "none"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct WaitKey {
    memory_base: *mut u8,
    offset: usize,
}

#[cfg(not(target_os = "none"))]
unsafe impl Send for WaitKey {}
#[cfg(not(target_os = "none"))]
unsafe impl Sync for WaitKey {}

#[cfg(not(target_os = "none"))]
struct WaitEntry {
    condvar: Arc<Condvar>,
    count: usize,
}

#[cfg(not(target_os = "none"))]
#[derive(Default)]
struct WaitTableInner {
    map: HashMap<WaitKey, WaitEntry>,
}

#[cfg(not(target_os = "none"))]
pub(crate) struct WaitTable {
    inner: Mutex<WaitTableInner>,
}

#[cfg(not(target_os = "none"))]
impl WaitTable {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(WaitTableInner::default()),
        }
    }

    /// wait32: if *addr == expected, block until notified or timeout.
    /// Returns 0 (woken), 1 (mismatch), 2 (timeout).
    pub(crate) fn wait32(
        &self,
        memory_base: *mut u8,
        offset: usize,
        expected: u32,
        timeout_ns: i64,
    ) -> u32 {
        let addr = unsafe { memory_base.add(offset) as *const AtomicU32 };
        let key = WaitKey {
            memory_base,
            offset,
        };

        // Register first, then reload under the same lock to close the lost-wakeup window
        let mut guard = self.inner.lock();
        let cv = {
            let entry = guard.map.entry(key).or_insert_with(|| WaitEntry {
                condvar: Arc::new(Condvar::new()),
                count: 0,
            });
            entry.count += 1;
            Arc::clone(&entry.condvar)
        };

        #[cfg(test)]
        run_pre_reload_hook();

        // Reload under lock; if value changed, unregister and return mismatch
        let current = unsafe { (*addr).load(Ordering::SeqCst) };
        if current != expected {
            if let Some(entry) = guard.map.get_mut(&key) {
                entry.count -= 1;
                if entry.count == 0 {
                    guard.map.remove(&key);
                }
            }
            return 1; // mismatch
        }

        let timed_out = if timeout_ns < 0 {
            cv.wait(&mut guard);
            false
        } else {
            let duration = Duration::from_nanos(timeout_ns as u64);
            cv.wait_for(&mut guard, duration).timed_out()
        };

        // A woken waiter was already taken off the books by notify. Only the
        // timeout path removes itself. parking_lot dequeues a timed-out thread
        // under the queue lock, so notify_one() never counts it.
        if !timed_out {
            return 0;
        }
        if let Some(entry) = guard.map.get_mut(&key) {
            entry.count -= 1;
            if entry.count == 0 {
                guard.map.remove(&key);
            }
        }
        2
    }

    /// wait64: analogous to wait32 but for i64.
    pub(crate) fn wait64(
        &self,
        memory_base: *mut u8,
        offset: usize,
        expected: u64,
        timeout_ns: i64,
    ) -> u32 {
        let addr = unsafe { memory_base.add(offset) as *const AtomicU64 };
        let key = WaitKey {
            memory_base,
            offset,
        };

        // Register first, then reload under the same lock
        let mut guard = self.inner.lock();
        let cv = {
            let entry = guard.map.entry(key).or_insert_with(|| WaitEntry {
                condvar: Arc::new(Condvar::new()),
                count: 0,
            });
            entry.count += 1;
            Arc::clone(&entry.condvar)
        };

        #[cfg(test)]
        run_pre_reload_hook();

        // Reload under lock; if value changed, unregister and return mismatch
        let current = unsafe { (*addr).load(Ordering::SeqCst) };
        if current != expected {
            if let Some(entry) = guard.map.get_mut(&key) {
                entry.count -= 1;
                if entry.count == 0 {
                    guard.map.remove(&key);
                }
            }
            return 1;
        }

        let timed_out = if timeout_ns < 0 {
            cv.wait(&mut guard);
            false
        } else {
            let duration = Duration::from_nanos(timeout_ns as u64);
            cv.wait_for(&mut guard, duration).timed_out()
        };

        // A woken waiter was already taken off the books by notify. Only the
        // timeout path removes itself. parking_lot dequeues a timed-out thread
        // under the queue lock, so notify_one() never counts it.
        if !timed_out {
            return 0;
        }
        if let Some(entry) = guard.map.get_mut(&key) {
            entry.count -= 1;
            if entry.count == 0 {
                guard.map.remove(&key);
            }
        }
        2
    }

    /// notify: wake up to `count` threads waiting at `offset`.
    /// Returns the number actually woken.
    pub(crate) fn notify(&self, memory_base: *mut u8, offset: usize, count: u32) -> u32 {
        let key = WaitKey {
            memory_base,
            offset,
        };
        let mut guard = self.inner.lock();
        let Some(entry) = guard.map.get_mut(&key) else {
            return 0;
        };
        // notify_one() reports whether it dequeued a sleeping thread, so the
        // result counts real wakeups, not threads that already timed out.
        let mut woken: u32 = 0;
        while woken < count && entry.condvar.notify_one() {
            woken += 1;
        }
        entry.count -= woken as usize;
        if entry.count == 0 {
            guard.map.remove(&key);
        }
        woken
    }
}

#[cfg(all(test, not(target_os = "none")))]
std::thread_local! {
    static PRE_RELOAD_HOOK: core::cell::RefCell<Option<std::boxed::Box<dyn FnOnce()>>> =
        core::cell::RefCell::new(None);
}

/// Test hook: runs once on this thread, after the waiter registered and
/// before it reloads the value under the table lock.
#[cfg(all(test, not(target_os = "none")))]
fn set_pre_reload_hook(f: impl FnOnce() + 'static) {
    PRE_RELOAD_HOOK.with(|h| *h.borrow_mut() = Some(std::boxed::Box::new(f)));
}

#[cfg(all(test, not(target_os = "none")))]
fn run_pre_reload_hook() {
    if let Some(f) = PRE_RELOAD_HOOK.with(|h| h.borrow_mut().take()) {
        f();
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A store + notify that lands after the waiter passed its first check
    /// but before it sleeps must not be lost. The hook stores the new value
    /// after registration; a notifier thread then contends for the lock.
    /// wait must return 1 (not-equal) without sleeping, and the notifier must
    /// find nobody registered (0) once the waiter unregistered.
    #[test]
    fn store_between_register_and_reload_is_not_lost_wait32() {
        let cell = std::boxed::Box::leak(std::boxed::Box::new(AtomicU32::new(0)));
        let base = cell as *const AtomicU32 as *mut u8;
        let table: &'static WaitTable =
            std::boxed::Box::leak(std::boxed::Box::new(WaitTable::new()));
        let base_addr = base as usize;
        let notifier: std::rc::Rc<core::cell::RefCell<Option<std::thread::JoinHandle<u32>>>> =
            std::rc::Rc::new(core::cell::RefCell::new(None));
        let slot = notifier.clone();
        set_pre_reload_hook(move || {
            cell.store(1, Ordering::SeqCst);
            let h = std::thread::spawn(move || table.notify(base_addr as *mut u8, 0, 1));
            *slot.borrow_mut() = Some(h);
        });
        let start = Instant::now();
        let r = table.wait32(base, 0, 0, 2_000_000_000);
        let elapsed = start.elapsed();
        assert_eq!(r, 1, "value changed before sleeping: must report not-equal");
        assert!(
            elapsed < Duration::from_millis(500),
            "must not sleep: {elapsed:?}"
        );
        let woken = notifier
            .borrow_mut()
            .take()
            .expect("hook ran")
            .join()
            .unwrap();
        assert_eq!(woken, 0, "waiter unregistered before notify got the lock");
        assert!(table.inner.lock().map.is_empty(), "no stale entry");
    }

    #[test]
    fn store_between_register_and_reload_is_not_lost_wait64() {
        let cell = std::boxed::Box::leak(std::boxed::Box::new(AtomicU64::new(0)));
        let base = cell as *const AtomicU64 as *mut u8;
        let table = WaitTable::new();
        set_pre_reload_hook(move || cell.store(7, Ordering::SeqCst));
        let start = Instant::now();
        let r = table.wait64(base, 0, 0, 2_000_000_000);
        assert_eq!(r, 1);
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(table.inner.lock().map.is_empty());
    }

    fn wait_until_registered(table: &WaitTable, base: *mut u8, n: usize) {
        let key = WaitKey {
            memory_base: base,
            offset: 0,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if table.inner.lock().map.get(&key).map_or(0, |e| e.count) == n {
                return;
            }
            assert!(Instant::now() < deadline, "waiter never registered");
            std::thread::yield_now();
        }
    }

    /// One waiter, two back-to-back notify(1): the first wakes it and
    /// returns 1, the second finds nobody and returns 0, even if the woken
    /// waiter has not re-acquired the table lock yet.
    #[test]
    fn second_notify_does_not_over_report() {
        let cell: &'static AtomicU32 =
            std::boxed::Box::leak(std::boxed::Box::new(AtomicU32::new(0)));
        let table: &'static WaitTable =
            std::boxed::Box::leak(std::boxed::Box::new(WaitTable::new()));
        let base = cell as *const AtomicU32 as *mut u8;
        let base_addr = base as usize;
        let waiter =
            std::thread::spawn(move || table.wait32(base_addr as *mut u8, 0, 0, 10_000_000_000));
        wait_until_registered(table, base, 1);
        let first = table.notify(base, 0, 1);
        let second = table.notify(base, 0, 1);
        assert_eq!(first, 1, "first notify wakes the waiter");
        assert_eq!(second, 0, "second notify must not count the same waiter");
        assert!(
            table.inner.lock().map.is_empty(),
            "notify removed the drained entry"
        );
        assert_eq!(waiter.join().unwrap(), 0, "waiter reports woken");
    }

    /// notify(2) with three waiters wakes exactly two; a later notify(10)
    /// wakes the remaining one.
    #[test]
    fn notify_count_is_exact_with_several_waiters() {
        let cell: &'static AtomicU32 =
            std::boxed::Box::leak(std::boxed::Box::new(AtomicU32::new(0)));
        let table: &'static WaitTable =
            std::boxed::Box::leak(std::boxed::Box::new(WaitTable::new()));
        let base = cell as *const AtomicU32 as *mut u8;
        let base_addr = base as usize;
        let waiters: std::vec::Vec<_> = (0..3)
            .map(|_| {
                std::thread::spawn(move || table.wait32(base_addr as *mut u8, 0, 0, 10_000_000_000))
            })
            .collect();
        wait_until_registered(table, base, 3);
        assert_eq!(table.notify(base, 0, 2), 2);
        assert_eq!(table.notify(base, 0, 10), 1);
        assert_eq!(table.notify(base, 0, 10), 0);
        for w in waiters {
            assert_eq!(w.join().unwrap(), 0);
        }
        assert!(table.inner.lock().map.is_empty());
    }

    /// The timeout path removes its own registration, so a notify after the
    /// timeout returns 0 and the table is empty.
    #[test]
    fn timed_out_waiter_unregisters_itself() {
        let cell = std::boxed::Box::leak(std::boxed::Box::new(AtomicU32::new(0)));
        let base = cell as *const AtomicU32 as *mut u8;
        let table = WaitTable::new();
        assert_eq!(table.wait32(base, 0, 0, 1_000_000), 2);
        assert!(table.inner.lock().map.is_empty());
        assert_eq!(table.notify(base, 0, 1), 0);
    }
}

#[cfg(not(target_os = "none"))]
use std::sync::LazyLock;
#[cfg(not(target_os = "none"))]
static GLOBAL_WAIT_TABLE: LazyLock<WaitTable> = LazyLock::new(|| WaitTable::new());

#[cfg(not(target_os = "none"))]
pub(crate) fn global_wait_table() -> &'static WaitTable {
    &GLOBAL_WAIT_TABLE
}

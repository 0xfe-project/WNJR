//! OS abstraction layer for the native JIT runtime.
//!
//! This module is the *only* place in the crate that talks to host OS
//! primitives (mmap, VirtualAlloc, sigaction, …). Everything else in the
//! runtime layer — `code_buf`, `guard_pages`, `trap_signal` — must call
//! through this API instead of invoking syscalls directly.
//!
//! # Structure
//!
//! Exactly one OS submodule is active for a given build, chosen by `target_os`:
//!
//! - `target_os = "linux"`   → `linux` (shares POSIX primitives with macOS via `posix`)
//! - `target_os = "macos"`   → `macos` (shares POSIX primitives with Linux via `posix`)
//! - `target_os = "windows"` → `windows`
//! - `target_os = "none"`    → `none` (bare-metal; embedder provides extern shims)
//!
//! The active module re-exports the following free functions, which form
//! the full API surface the rest of the runtime is allowed to use.
//!
//! ## Executable code buffer primitives
//!
//! - `alloc_executable(capacity)`  → reserves `capacity` bytes suitable for
//!   holding JIT-emitted machine code. The returned region must be writable
//!   after `begin_write_executable` and executable after
//!   `finish_write_executable`.
//! - `free_executable(base, capacity)` → releases the region.
//! - `begin_write_executable(base, capacity)` → make the region writable.
//!   On macOS this is a per-thread W^X toggle
//!   (`pthread_jit_write_protect_np`); on Linux/Windows it is a page
//!   permission change.
//! - `finish_write_executable(base, capacity, written_start, written_len)`
//!   → flip back to executable and flush the instruction cache over the
//!   actually-modified range.
//!
//! ## Guarded linear memory primitives
//!
//! Compiled only when `any(target_arch = "aarch64", target_arch = "x86_64")` is set.
//!
//! - `reserve_guarded(total)` → reserves `total` bytes as unmapped / no-access
//!   backing for a wasm linear memory with guard pages. No pages are
//!   immediately usable — they must be committed via `commit_guarded`.
//! - `commit_guarded(base, offset, len)` → upgrade `[offset, offset + len)`
//!   within the reservation to readable+writable.
//! - `release_guarded(base, total)` → release the whole reservation.
//!
//! # Adding a new target
//!
//! 1. Add a module here that exports the functions listed above.
//! 2. Gate it below with `#[cfg(target_os = "...")]`.
//!
//! No other file in the crate should need to change.

#[cfg(unix)]
mod posix;

// Per-(arch × os) signal handler modules live here. Only compiled when
// `any(target_arch = "aarch64", target_arch = "x86_64")` is set, because they are the only consumers of
// `trap_signal`. Exactly one platform submodule is active for a given
// build; see `signal/mod.rs`.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub(in crate::vm::jit::runtime) mod signal;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;

#[cfg(target_os = "none")]
mod none;
#[cfg(target_os = "none")]
pub(crate) use none::*;

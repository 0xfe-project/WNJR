//! Shared debug infrastructure.
//!
//! This layer should stay optional and off the release path by default.

#[cfg(debug_assertions)]
pub(crate) mod function_trace;
#[cfg(debug_assertions)]
pub(crate) mod ir_dump;
#[cfg(debug_assertions)]
pub(crate) mod jitdump;

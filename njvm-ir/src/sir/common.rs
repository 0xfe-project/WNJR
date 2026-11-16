//! Shared semantic/frontend ids and small types.
//!
//! This module must stay semantic-only. It must not expose backend frame-slot
//! or rotating-window placement details.

/// Semantic instruction index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticIndex(u32);

impl SemanticIndex {
    #[inline]
    pub(crate) const fn new(raw: usize) -> Self {
        Self(raw as u32)
    }

    #[inline]
    pub(crate) const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Structured control-flow target in semantic space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticTarget(SemanticIndex);

impl SemanticTarget {
    #[inline]
    pub(crate) const fn new(raw: usize) -> Self {
        Self(SemanticIndex::new(raw))
    }

    #[inline]
    pub(crate) const fn index(self) -> SemanticIndex {
        self.0
    }

    #[inline]
    pub(crate) const fn pending() -> Self {
        Self(SemanticIndex(u32::MAX))
    }

    #[inline]
    pub(crate) const fn is_pending(self) -> bool {
        self.0 .0 == u32::MAX
    }
}

/// Semantic branch-table entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrTableEntry {
    pub(crate) target: SemanticTarget,
    pub(crate) stack_drop: u32,
    pub(crate) arity: u16,
}

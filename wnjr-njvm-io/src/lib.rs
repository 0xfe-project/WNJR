//! NJVM-IR MessagePack interchange format.
//!
//! The NJVM-IR `wnjr_core::njvm::Module` is the contract between the
//! language frontends (the C compiler `wnjr-cc`, the JS compiler `porffor`)
//! and the Rust transform path. A frontend emits a module as a MessagePack
//! file; the Rust side reads it back with [`read_module`], which runs the N3
//! verifier before any transform sees it. [`write_module`] produces the same
//! bytes from Rust (used by tests and the `njvm-dump` tool). [`decode_module`]
//! reads without verifying, for diagnostics.
//!
//! # Wire format
//!
//! The whole file is one MessagePack value: a 2-entry map (the *envelope*)
//!
//! ```text
//! { "version": <uint>, "module": <module-map> }
//! ```
//!
//! `version` is [`FORMAT_VERSION`]. A reader rejects any other value with
//! [`Error::UnsupportedVersion`] before looking at the module.
//!
//! The module and everything under it is produced by `serde` with its default
//! derive encoding, written through `rmp_serde::to_vec_named` (struct fields
//! are MessagePack string keys, not positions). The rules a hand-written
//! writer (the C frontend) must follow:
//!
//! - A **struct** is a map from field-name string to value. `Module`,
//!   `Function`, `FunctionType`, `Import`, `Export`, `Global`, `Tag`,
//!   `DataSegment`, `DataLabelFixup`, `ElementSegment`, `TableType`,
//!   `MemoryType`, `GlobalType` are structs. Field order is not significant to the reader, but the Rust
//!   writer emits them in declaration order.
//! - A **fieldless enum variant** (e.g. `Type::I32`, `Op::Nop`,
//!   `Op::Unreachable`, `Op::MemorySize`, `SegmentMode::Passive`) is just
//!   the variant-name string.
//! - Any **other enum variant** is a 1-entry map `{ "<Variant>": <payload> }`
//!   (serde "external tagging"). The payload is:
//!   - a struct-like variant (e.g. `Op::Jmp`, `Op::Store`,
//!     `Expr::Binary`): a map of its fields;
//!   - a single-field (newtype) variant (e.g. `Expr::LocalGet`,
//!     `ImportKind::Function`, `InitExpr::Const`): the inner value;
//!   - a tuple variant: an array of its fields (none of the current IR enums
//!     use this; the `(ConstVal, Vec<Op>)` cases of `Op::Switch` are tuples
//!     *inside* a list and encode as 2-element arrays).
//! - [`Option`] is `nil` (0xc0) for `None`, or the bare inner value for `Some`.
//! - A `Vec<T>` is a MessagePack array. `Vec<u8>` (data-segment bytes) is an
//!   array of integers, **not** a `bin` blob.
//! - Floating-point constants are stored as **raw IEEE-754 bits**:
//!   `ConstVal::F32` wraps a `u32`, `ConstVal::F64` a `u64`. The writer
//!   must emit the bit pattern, never a MessagePack `float`. This keeps NaN
//!   payloads exact and lets the enum derive `Eq`/`Hash`.
//! - Integers may use any MessagePack width that fits (the reader accepts a
//!   `u32` field written as `uint 8` through `uint 64`); the Rust writer uses
//!   the smallest.
//!
//! The reader is tolerant in the ways `rmp_serde` is by default: a fieldless
//! variant may also arrive as a 1-entry map with a `nil` payload, a struct
//! variant's fields may arrive as a positional array, unknown map keys are
//! skipped, and trailing bytes after the envelope are ignored. A writer that
//! follows the rules above interoperates regardless.
//!
//! # Byte examples
//!
//! These are asserted against the encoder in the crate's tests
//! (`tests/wire_format.rs`; the envelope prefix and the F64 constant too), so the documentation and the code cannot drift.
//!
//! | value | bytes (hex) |
//! |-------|-------------|
//! | `Type::I32` | `a3 49 33 32` |
//! | `Op::Nop` | `a3 4e 6f 70` |
//! | `Op::Unreachable` | `ab 55 6e 72 65 61 63 68 61 62 6c 65` |
//! | [`Expr::LocalGet(1)`](wnjr_core::njvm::Expr::LocalGet) | `81 a8 4c 6f 63 61 6c 47 65 74 01` |
//! | [`Op::Jmp { target: 3 }`](wnjr_core::njvm::Op::Jmp) | `81 a3 4a 6d 70 81 a6 74 61 72 67 65 74 03` |
//!
//! | `Expr::Const { ty: F64, val: ConstVal::F64(1.0f64.to_bits()) }` | `81 a5 43 6f 6e 73 74 82 a2 74 79 a3 46 36 34 a3 76 61 6c 81 a3 46 36 34 cf 3f f0 00 00 00 00 00 00` |
//!
//! Reading the `Jmp` example: `81` map of 1 entry; `a3 4a 6d 70`
//! the key string `"Jmp"`; `81` the value, a map of 1 entry;
//! `a6 74 61 72 67 65 74` the key string `"target"`; `03` the
//! `positive fixint 3`.
//!
//! The `version` envelope wrapping a whole module is shown in
//! [`FORMAT_VERSION`].

use std::fmt;

use wnjr_core::njvm::{verify_module, Module, VerifyError};

use serde::{Deserialize, Serialize};

/// The interchange format version written into every file's envelope.
///
/// A one-function module (`fn f()`, empty body) round-trips as the
/// envelope `{ "version": 1, "module": { .. } }`; the leading bytes are
/// `82 a7 76 65 72 73 69 6f 6e 01 a6 6d 6f 64 75 6c 65 ..` (map of 2:
/// `"version"` → `1`, then `"module"` → the module map).
pub const FORMAT_VERSION: u32 = 1;

/// The on-disk envelope: a version tag and the module.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Envelope {
    version: u32,
    module: Module,
}

/// Just the version, to reject an unknown format before decoding the module.
#[derive(Deserialize)]
struct VersionHeader {
    version: u32,
    // The module is skipped. Deserializing still walks its bytes, so a
    // truncated file fails here as a decode error, not a version error.
    #[allow(dead_code)]
    #[serde(default)]
    module: serde::de::IgnoredAny,
}

/// Why reading a module failed.
#[derive(Debug)]
pub enum Error {
    /// The bytes are not a well-formed MessagePack envelope (truncated,
    /// wrong shape, a float where raw bits were expected, ...).
    Decode(rmp_serde::decode::Error),
    /// The envelope's `version` is not [`FORMAT_VERSION`].
    UnsupportedVersion {
        /// The version found in the file.
        found: u32,
        /// The only version this build understands.
        expected: u32,
    },
    /// The module decoded but the N3 verifier rejected it. The shape was
    /// right; the IR was not well-formed.
    Verify(VerifyError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Decode(e) => write!(f, "msgpack decode error: {e}"),
            Error::UnsupportedVersion { found, expected } => write!(
                f,
                "unsupported NJVM-IR format version {found} (this build reads {expected})"
            ),
            Error::Verify(e) => write!(f, "verifier rejected the module: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Decode(e) => Some(e),
            Error::Verify(_) | Error::UnsupportedVersion { .. } => None,
        }
    }
}

/// Serialize a module to the versioned MessagePack envelope.
///
/// The types are total over their own shape, so this does not fail in
/// practice; a serializer error would still surface as a panic rather than a
/// corrupt file.
pub fn write_module(module: &Module) -> Vec<u8> {
    let envelope = Envelope {
        version: FORMAT_VERSION,
        module: module.clone(),
    };
    rmp_serde::to_vec_named(&envelope).expect("NJVM-IR module is always serializable")
}

/// Decode a module from the envelope **without** verifying it.
///
/// The version is still checked. Use this for diagnostics (the `njvm-dump`
/// tool) where you want to see even an IR that would fail verification. For
/// anything that feeds a transform, use [`read_module`].
pub fn decode_module(bytes: &[u8]) -> Result<Module, Error> {
    let header: VersionHeader = rmp_serde::from_slice(bytes).map_err(Error::Decode)?;
    if header.version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion {
            found: header.version,
            expected: FORMAT_VERSION,
        });
    }
    let envelope: Envelope = rmp_serde::from_slice(bytes).map_err(Error::Decode)?;
    Ok(envelope.module)
}

/// Decode a module from the envelope and run the N3 verifier on it.
///
/// Errors are ordered: a malformed envelope is [`Error::Decode`], a
/// well-formed envelope with the wrong version is [`Error::UnsupportedVersion`]
/// (reported before the module is fully decoded), and a well-formed module the
/// verifier rejects is [`Error::Verify`]. `serde` only guarantees the shape;
/// every semantic rule is the verifier's.
pub fn read_module(bytes: &[u8]) -> Result<Module, Error> {
    let module = decode_module(bytes)?;
    verify_module(&module).map_err(Error::Verify)?;
    Ok(module)
}

pub mod dump;


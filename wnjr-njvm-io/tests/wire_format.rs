//! Wire-format byte examples asserted against the library docs.

use wnjr_core::njvm::{ConstVal, Expr, Op, Type};
use wnjr_njvm_io::{write_module, FORMAT_VERSION};

mod common;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect::<Vec<_>>().join(" ")
}

/// Serialize a value using rmp_serde and return its bytes as hex.
fn bytes_of<T: serde::Serialize>(v: &T) -> String {
    hex(&rmp_serde::to_vec_named(v).unwrap())
}

/// The lib.rs doctable claims these bytes; these tests anchor the text to the
/// actual encoding.
#[test]
fn documented_bytes_are_correct() {
    assert_eq!(bytes_of(&Type::I32), "a3 49 33 32");
    assert_eq!(bytes_of(&Op::Nop), "a3 4e 6f 70");
    assert_eq!(bytes_of(&Op::Unreachable), "ab 55 6e 72 65 61 63 68 61 62 6c 65");
    assert_eq!(bytes_of(&Expr::LocalGet(1)), "81 a8 4c 6f 63 61 6c 47 65 74 01");
    assert_eq!(
        bytes_of(&Op::Jmp { target: 3 }),
        "81 a3 4a 6d 70 81 a6 74 61 72 67 65 74 03"
    );
}

/// The envelope prefix documented on `FORMAT_VERSION`:
/// `82 a7 76 65 72 73 69 6f 6e 01 a6 6d 6f 64 75 6c 65`.
#[test]
fn envelope_version_prefix() {
    let bytes = write_module(&common::add());
    assert_eq!(hex(&bytes[..17]), "82 a7 76 65 72 73 69 6f 6e 01 a6 6d 6f 64 75 6c 65");
    assert_eq!(bytes[9], FORMAT_VERSION as u8);
}

/// Floats travel as raw bits: `ConstVal::F64(1.0)` is the uint64
/// `cf 3f f0 00 00 00 00 00 00`, never a msgpack float (`cb`).
#[test]
fn f64_const_is_raw_bits() {
    let e = Expr::Const { ty: Type::F64, val: ConstVal::f64(1.0) };
    assert_eq!(
        bytes_of(&e),
        "81 a5 43 6f 6e 73 74 82 a2 74 79 a3 46 36 34 a3 76 61 6c 81 a3 46 36 34 cf 3f f0 00 00 00 00 00 00"
    );
}

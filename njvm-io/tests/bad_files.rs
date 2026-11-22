//! Bad files: each failure is reported as the right error kind.

mod common;

use wnjr_core::njvm::{Function, ModuleBuilder, Op, VerifyErrorKind};
use njvm_io::{read_module, write_module, Error, FORMAT_VERSION};

/// The envelope begins `82 a7 "version" <fixint>`; the version byte is at 9.
const VERSION_BYTE: usize = 9;

#[test]
fn truncated_bytes_are_a_decode_error() {
    let bytes = write_module(&common::nested_loop());
    // Every strict prefix must fail to decode, never panic or verify.
    for cut in [0, 1, VERSION_BYTE, bytes.len() / 2, bytes.len() - 1] {
        match read_module(&bytes[..cut]) {
            Err(Error::Decode(_)) => {}
            other => panic!("cut at {cut}: expected Decode, got {other:?}"),
        }
    }
}

#[test]
fn wrong_version_is_a_version_error() {
    let mut bytes = write_module(&common::add());
    assert_eq!(&bytes[..VERSION_BYTE], b"\x82\xa7version", "envelope prefix changed");
    assert_eq!(bytes[VERSION_BYTE], FORMAT_VERSION as u8);
    bytes[VERSION_BYTE] = 2; // positive fixint 2
    match read_module(&bytes) {
        Err(Error::UnsupportedVersion { found: 2, expected }) => assert_eq!(expected, FORMAT_VERSION),
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[test]
fn well_formed_but_invalid_is_a_verifier_error() {
    // Jmp to a label that is never declared: serde accepts the shape, the
    // verifier rejects it.
    let mut mb = ModuleBuilder::new();
    let mut f = Function::new(vec![], vec![], vec![]);
    f.body = vec![Op::Nop, Op::Jmp { target: 7 }];
    mb.add_function(f);
    let bytes = write_module(&mb.build());
    match read_module(&bytes) {
        Err(Error::Verify(e)) => {
            assert_eq!((e.func, e.at.as_slice()), (0, &[1usize][..]));
            assert_eq!(e.kind, VerifyErrorKind::UndeclaredLabel { label: 7 });
        }
        other => panic!("expected Verify, got {other:?}"),
    }
}

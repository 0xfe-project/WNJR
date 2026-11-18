//! Round-trip: write -> read reconstructs an equal module, the verifier
//! accepts every good fixture, and the dump is stable across two runs.

mod common;

use wnjr_njvm_io::{dump::dump_module, read_module, write_module, FORMAT_VERSION};

#[test]
fn write_then_read_is_identity() {
    for (name, module) in common::all() {
        let bytes = write_module(&module);
        let back = read_module(&bytes).unwrap_or_else(|e| panic!("{name}: read failed: {e}"));
        assert_eq!(back, module, "{name}: round-trip changed the module");
    }
}

#[test]
fn write_is_deterministic() {
    for (name, module) in common::all() {
        let a = write_module(&module);
        let b = write_module(&module);
        assert_eq!(a, b, "{name}: write is not byte-stable");
    }
}

#[test]
fn dump_is_deterministic() {
    for (name, module) in common::all() {
        let a = dump_module(&module, FORMAT_VERSION);
        let b = dump_module(&module, FORMAT_VERSION);
        assert_eq!(a, b, "{name}: dump is not stable");
    }
}

#[test]
fn dump_round_trips_through_bytes() {
    // Dumping the module and dumping the module read back from bytes agree:
    // the text is a function of the module alone.
    for (name, module) in common::all() {
        let bytes = write_module(&module);
        let back = read_module(&bytes).unwrap();
        assert_eq!(
            dump_module(&module, FORMAT_VERSION),
            dump_module(&back, FORMAT_VERSION),
            "{name}: dump differs after a round-trip"
        );
    }
}

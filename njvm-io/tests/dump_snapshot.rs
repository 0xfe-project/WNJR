//! Dump snapshot. `tests/snapshots/showcase.msgpack` is a committed test
//! file (written from `common::showcase()`); running the `njvm-dump` binary
//! on it must print exactly `tests/snapshots/showcase.dump`.
//! `UPDATE_SNAPSHOTS=1 cargo test -p njvm-io` rewrites both files.

mod common;

use std::path::PathBuf;
use std::process::Command;

use njvm_io::write_module;

fn snap(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(name)
}

fn update() -> bool {
    std::env::var_os("UPDATE_SNAPSHOTS").is_some()
}

#[test]
fn committed_msgpack_matches_writer() {
    let bytes = write_module(&common::showcase());
    let path = snap("showcase.msgpack");
    if update() {
        std::fs::write(&path, &bytes).unwrap();
    }
    let golden = std::fs::read(&path).expect("showcase.msgpack missing; run with UPDATE_SNAPSHOTS=1");
    assert_eq!(bytes, golden, "writer output differs from the committed test file");
}

#[test]
fn njvm_dump_output_matches_snapshot() {
    let out = Command::new(env!("CARGO_BIN_EXE_njvm-dump"))
        .arg(snap("showcase.msgpack"))
        .output()
        .expect("run njvm-dump");
    assert!(out.status.success(), "njvm-dump failed: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "verify: ok\n");
    let text = String::from_utf8(out.stdout).unwrap();
    let path = snap("showcase.dump");
    if update() {
        std::fs::write(&path, &text).unwrap();
    }
    let golden = std::fs::read_to_string(&path).expect("showcase.dump missing; run with UPDATE_SNAPSHOTS=1");
    assert_eq!(text, golden, "njvm-dump output differs from the snapshot");
}

#[test]
fn njvm_dump_exit_codes() {
    let dir = std::env::temp_dir().join(format!("njvm-dump-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let truncated = dir.join("truncated.msgpack");
    let bytes = write_module(&common::add());
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_njvm-dump")).arg(&truncated).output().unwrap();
    assert_eq!(st.status.code(), Some(2));
    assert!(st.stdout.is_empty());

    let mut m = common::add();
    m.functions[0].body.push(wnjr_core::njvm::Op::Jmp { target: 9 });
    let invalid = dir.join("invalid.msgpack");
    std::fs::write(&invalid, write_module(&m)).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_njvm-dump")).arg(&invalid).output().unwrap();
    assert_eq!(st.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&st.stdout).contains("jmp L9"));
    std::fs::remove_dir_all(&dir).ok();
}

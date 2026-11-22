//! `njvm-dump FILE`: print an NJVM-IR msgpack file as text.
//!
//! The text (see `njvm_io::dump`) goes to stdout. The module is decoded
//! without verification so that even a rejected module can be inspected; the
//! verifier then runs and its verdict goes to stderr.
//!
//! Exit status: 0 dumped and verified, 1 usage or I/O error, 2 decode or
//! version error (nothing dumped), 3 dumped but the verifier rejected it.

use std::process::ExitCode;

use wnjr_core::njvm::verify_module;
use njvm_io::{decode_module, dump::dump_module, FORMAT_VERSION};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, path] = args.as_slice() else {
        eprintln!("usage: njvm-dump FILE");
        return ExitCode::from(1);
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("njvm-dump: {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let module = match decode_module(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("njvm-dump: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    print!("{}", dump_module(&module, FORMAT_VERSION));
    match verify_module(&module) {
        Ok(()) => {
            eprintln!("verify: ok");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("verify: {e}");
            ExitCode::from(3)
        }
    }
}

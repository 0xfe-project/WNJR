//! Run a NJVM-IR module produced by a frontend (wnjr-cc) on WNJR.
//!
//! Usage: njvm-run <module.msgpack> [export] [arg...]
//!
//! The module is decoded without verification, then put through the same
//! pipeline the harness uses — computed goto lowering, setjmp lowering, type
//! erasure, stackify, encode — and executed on WNJR. Verification happens
//! inside `compile`, i.e. after the pre-lowering markers are gone: a frontend
//! module may legitimately contain `LabelAddr`/`GotoIndirect`, which the N3
//! verifier rejects by design. The result is printed one value per line; a trap
//! prints `trap` and exits 3.

use std::process::ExitCode;

use wnjr_njvm_e2e::{compile, run_wnjr, Arg, Outcome};

fn usage() -> String {
    "usage: njvm-run <module.msgpack> [export] [arg...]".to_string()
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("njvm-run: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Err(usage());
    }
    let path = &args[0];
    let export = args.get(1).cloned().unwrap_or_else(|| "main".to_string());
    let mut inputs = Vec::new();
    for a in &args[2..] {
        let v: i32 = a.parse().map_err(|_| format!("bad i32 argument {a:?}"))?;
        inputs.push(Arg::I32(v));
    }

    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let module = wnjr_njvm_io::decode_module(&bytes).map_err(|e| format!("{e}"))?;
    let wasm = compile(&module, &[])?;
    let outcomes = run_wnjr(&wasm, &export, &[inputs], false)?;
    match &outcomes[0] {
        Outcome::Trap => {
            println!("trap");
            std::process::exit(3);
        }
        Outcome::Ret(vals) => {
            for v in vals {
                let s = match v {
                    Arg::I32(x) => x.to_string(),
                    Arg::I64(x) => x.to_string(),
                    Arg::F32(b) => format!("f32:{b}"),
                    Arg::F64(b) => format!("f64:{b}"),
                };
                println!("{s}");
            }
            Ok(())
        }
    }
}

//! `njvm-wasm IN.msgpack OUT.wasm`: compile a NJVM-IR module to wasm bytes.
//!
//! A debugging aid: it runs the same pipeline as `njvm-run` (computed goto
//! lowering, setjmp lowering, type erasure, stackify, encode) and writes the
//! wasm so another host can run exactly what WNJR ran.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = args.as_slice() else {
        eprintln!("usage: njvm-wasm IN.msgpack OUT.wasm");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("njvm-wasm: {input}: {e}");
            return ExitCode::from(2);
        }
    };
    let module = match wnjr_njvm_io::decode_module(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("njvm-wasm: {input}: {e}");
            return ExitCode::from(2);
        }
    };
    let wasm = match wnjr_njvm_e2e::compile(&module, &[]) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("njvm-wasm: {e}");
            return ExitCode::from(3);
        }
    };
    if let Err(e) = std::fs::write(output, &wasm) {
        eprintln!("njvm-wasm: {output}: {e}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

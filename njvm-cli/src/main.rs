//! Run or emit a NJVM-IR module produced by a frontend (njvm-cc).
//!
//! Usage: njvm-cli [run] <module.mp> [--guest-arg <s>]... [export] [arg...]
//!        njvm-cli [run] <module.mp> [--guest-arg <s>]... --batch <calls.txt>
//!        njvm-cli emit <module.mp> <out.wasm>
//!
//! The subcommand is optional: a bare `.mp` path means `run`. `emit` does
//! not lower to the engine — it writes the wasm bytes the same pipeline would
//! hand to WNJR, so another host can run exactly what WNJR ran.
//!
//! `--guest-arg` is one word of the argv the guest's `args_get` returns. A
//! module that reads its own command line — a compiler run on WNJR — needs it;
//! a function under test does not, and gets `"njvm-cli"` as before.
//!
//! Batch mode runs every call in one instance, in file order, so a call sees
//! the globals and statics an earlier call wrote — the same thing the C driver
//! the harness generates does. Each line of the calls file is `"<export>
//! [int arg...]"`; comments (#) and blank lines are skipped. Output is one
//! `"<export> [arg...] -> value"` line per call, ready to diff against the C
//! driver's output.
//!
//! The module is decoded without verification, then put through the same
//! pipeline the harness uses — computed goto lowering, setjmp lowering, type
//! erasure, stackify, encode — and executed on WNJR. Verification happens
//! inside `compile`, i.e. after the pre-lowering markers are gone: a frontend
//! module may legitimately contain `LabelAddr`/`GotoIndirect`, which the N3
//! verifier rejects by design. A run prints one value per line; a trap prints
//! `trap` and exits 3.

use std::process::ExitCode;

use njvm_e2e::{compile, run_wnjr_sequence, Arg, Outcome};

fn usage(program: &str) -> String {
    format!(
        "usage: {program} [run] <module.mp> [--guest-arg <s>]... [export] [arg...]\n\n       \
         {program} [run] <module.mp> [--guest-arg <s>]... --batch <calls.txt>\n\n       \
         {program} emit <module.mp> <out.wasm>"
    )
}

fn main() -> ExitCode {
    let program = std::env::args().next().unwrap_or_else(|| "njvm-cli".to_string());
    let program = program.rsplit('/').next().unwrap_or("njvm-cli").to_string();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(|s| s.as_str()) {
        None => Err(usage(&program)),
        Some("emit") => emit(&args[1..]),
        Some("run") => run(&args[1..], &program),
        // A bare path is `run`: the module is what a user has in hand, and
        // making them spell the verb would be a word they never need.
        Some(_) => run(&args, &program),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{program}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `emit IN.mp OUT.wasm`: write the wasm bytes the pipeline produces.
fn emit(args: &[String]) -> Result<ExitCode, String> {
    let [input, output] = args else {
        return Err("emit takes <module.mp> <out.wasm>".to_string());
    };
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let module = njvm_io::decode_module(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let wasm = compile(&module, &[])?;
    std::fs::write(output, &wasm).map_err(|e| format!("{output}: {e}"))?;
    Ok(ExitCode::SUCCESS)
}

fn run(args: &[String], program: &str) -> Result<ExitCode, String> {
    // --guest-arg takes a value and may repeat, so pull them out first and let
    // the rest keep its positional meaning.
    let mut guest_args: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    let mut argv = args.iter();
    while let Some(a) = argv.next() {
        if a == "--guest-arg" {
            guest_args.push(argv.next().ok_or_else(|| usage(program))?.clone());
        } else {
            rest.push(a.clone());
        }
    }
    let args = rest;
    if args.is_empty() {
        return Err(usage(program));
    }
    let path = &args[0];
    if args.get(1).map(|s| s.as_str()) == Some("--batch") {
        let calls_path = args.get(2).ok_or_else(|| usage(program))?;
        return batch(path, calls_path, &guest_args);
    }
    let export = args.get(1).cloned().unwrap_or_else(|| "main".to_string());
    let mut inputs = Vec::new();
    for a in args.get(2..).unwrap_or(&[]) {
        let v: i32 = a.parse().map_err(|_| format!("bad i32 argument {a:?}"))?;
        inputs.push(Arg::I32(v));
    }

    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let module = njvm_io::decode_module(&bytes).map_err(|e| format!("{e}"))?;
    let wasm = compile(&module, &[])?;
    // A program's last printf arrives only if something flushes it, and a module
    // that has stdio has __stdio_exit to do exactly that — the same call the
    // batch path appends. Without it a run that ends by returning from main
    // loses whatever was still in the buffer, which reads as a silent failure.
    let mut calls: Vec<(String, Vec<Arg>)> = vec![(export.clone(), inputs)];
    if module.exports.iter().any(|e| e.name == "__stdio_exit") {
        calls.push(("__stdio_exit".to_string(), Vec::new()));
    }
    let outcomes = run_wnjr_sequence(&wasm, &calls, false, &guest_args)?;
    match &outcomes[0] {
        Outcome::Trap => {
            println!("trap");
            Ok(ExitCode::from(3))
        }
        Outcome::Ret(vals) => {
            for v in vals {
                println!("{}", format_value(v));
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn format_value(v: &Arg) -> String {
    match v {
        Arg::I32(x) => x.to_string(),
        Arg::I64(x) => x.to_string(),
        Arg::F32(b) => format!("f32:{b}"),
        Arg::F64(b) => format!("f64:{b}"),
    }
}

/// Read the calls file, run every call on one instance, and print one
/// "<export> [arg...] -> value" line per call.
fn batch(path: &str, calls_path: &str, guest_args: &[String]) -> Result<ExitCode, String> {
    let text = std::fs::read_to_string(calls_path).map_err(|e| format!("{calls_path}: {e}"))?;
    let mut calls: Vec<(String, Vec<Arg>)> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let export = words.next().expect("non-empty").to_string();
        let mut inputs = Vec::new();
        let mut label = export.clone();
        for w in words {
            let v: i32 = w.parse().map_err(|_| format!("bad i32 argument {w:?} in {line:?}"))?;
            inputs.push(Arg::I32(v));
            label.push(' ');
            label.push_str(w);
        }
        calls.push((export, inputs));
        labels.push(label);
    }

    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let module = njvm_io::decode_module(&bytes).map_err(|e| format!("{e}"))?;

    // A real program ends by flushing stdio, so the last printf of the C driver
    // the harness compares against arrives. Nothing exits here, so the calls
    // file says so: __stdio_exit is the flush an exiting libc does. It is not
    // printed, and a module without stdio does not have it.
    let flushed = module.exports.iter().any(|e| e.name == "__stdio_exit");
    let printed = calls.len();
    if flushed {
        calls.push(("__stdio_exit".to_string(), Vec::new()));
    }

    let wasm = compile(&module, &[])?;
    let outcomes = run_wnjr_sequence(&wasm, &calls, false, guest_args)?;
    let mut trapped = false;
    for (label, outcome) in labels.iter().zip(outcomes.iter()).take(printed) {
        let value = match outcome {
            Outcome::Trap => {
                trapped = true;
                "trap".to_string()
            }
            Outcome::Ret(vals) => vals.iter().map(format_value).collect::<Vec<_>>().join(" "),
        };
        println!("{label} -> {value}");
    }
    if trapped {
        return Ok(ExitCode::from(3));
    }
    Ok(ExitCode::SUCCESS)
}

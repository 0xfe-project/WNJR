//! NJVM-IR end-to-end differential harness (task A5 / N10).
//!
//! A [Case] is a hand-built NJVM-IR module, an exported function, inputs, and
//! the outcome a native Rust oracle computed for each input. The pipeline is
//!
//! ```text
//! computed goto lowering -> verify -> [ IR passes, verify after each ] ->
//! setjmp lowering -> verify -> encode (type erasure, stackify, switch,
//! load/store, call, trivial ops) -> wasm bytes
//! ```
//!
//! The bytes then run on **WNJR** (through `Module::new`, parse + validate +
//! decode, one fresh instance per input) and on **bun** (JavaScriptCore, one
//! fresh instance per input), each host in a child process with a per-case
//! deadline (a pass may produce a loop that never ends), and the outcomes are compared bit for bit
//! (traps compare equal to traps; floats compare by their bits, any NaN equal to any NaN).
//!
//! [check] is the differential entry point for optimisation passes: pass an
//! IR-to-IR pass list and every case runs twice, with no passes and with the
//! list. A pass that changes any outcome on either host is reported.
//! Passes run after computed goto lowering and before setjmp lowering, which
//! is where the optimisers (inline, fold + DCE, CFG simplify) sit in the
//! pipeline order. The lowering passes run after them and are not optional.
//!
//! Hosts: bun must be on PATH (or named by `NJVM_E2E_BUN`). There is no
//! fallback host: a missing bun, or a bun without wasm EH, is a failure.

pub mod cases;
pub mod walk;

use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use wnjr_core::njvm::{self, ExportKind, Module, Type};
use wnjr_core::{Config, Engine, Module as WasmModule, RuntimeWorld, Value};

// ───────────────────────────── values ─────────────────────────────

/// One wasm-level argument or result. Floats are carried as raw bits so NaN
/// payloads and the sign of zero are compared exactly.
#[derive(Clone, Copy, Debug)]
pub enum Arg {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
}

/// Floats compare by bits, except that every NaN equals every NaN: wasm leaves
/// the sign and payload of an arithmetic NaN to the implementation, so two
/// correct hosts may differ there.
impl PartialEq for Arg {
    fn eq(&self, other: &Arg) -> bool {
        match (*self, *other) {
            (Arg::I32(a), Arg::I32(b)) => a == b,
            (Arg::I64(a), Arg::I64(b)) => a == b,
            (Arg::F32(a), Arg::F32(b)) => a == b || (f32::from_bits(a).is_nan() && f32::from_bits(b).is_nan()),
            (Arg::F64(a), Arg::F64(b)) => a == b || (f64::from_bits(a).is_nan() && f64::from_bits(b).is_nan()),
            _ => false,
        }
    }
}
impl Eq for Arg {}

impl Arg {
    pub fn f32(v: f32) -> Arg {
        Arg::F32(v.to_bits())
    }
    pub fn f64(v: f64) -> Arg {
        Arg::F64(v.to_bits())
    }
    pub fn kind(self) -> Kind {
        match self {
            Arg::I32(_) => Kind::I32,
            Arg::I64(_) => Kind::I64,
            Arg::F32(_) => Kind::F32,
            Arg::F64(_) => Kind::F64,
        }
    }
    fn to_value(self) -> Value {
        match self {
            Arg::I32(v) => Value::I32(v),
            Arg::I64(v) => Value::I64(v),
            Arg::F32(b) => Value::F32(f32::from_bits(b)),
            Arg::F64(b) => Value::F64(f64::from_bits(b)),
        }
    }
    fn from_value(v: &Value) -> Result<Arg, String> {
        match v {
            Value::I32(x) => Ok(Arg::I32(*x)),
            Value::I64(x) => Ok(Arg::I64(*x)),
            Value::F32(x) => Ok(Arg::F32(x.to_bits())),
            Value::F64(x) => Ok(Arg::F64(x.to_bits())),
            other => Err(format!("unexpected result value {other:?}")),
        }
    }
    fn to_json(self) -> serde_json::Value {
        let (t, v) = match self {
            Arg::I32(v) => ("i32", v.to_string()),
            Arg::I64(v) => ("i64", v.to_string()),
            Arg::F32(b) => ("f32", b.to_string()),
            Arg::F64(b) => ("f64", b.to_string()),
        };
        serde_json::json!({ "t": t, "v": v })
    }
    fn from_json(j: &serde_json::Value) -> Result<Arg, String> {
        let t = j["t"].as_str().ok_or("result without type")?;
        let v = j["v"].as_str().ok_or("result without value")?;
        let bad = |e: std::num::ParseIntError| format!("bad {t} value {v:?}: {e}");
        Ok(match t {
            "i32" => Arg::I32(v.parse().map_err(bad)?),
            "i64" => Arg::I64(v.parse().map_err(bad)?),
            "f32" => Arg::F32(v.parse().map_err(bad)?),
            "f64" => Arg::F64(v.parse().map_err(bad)?),
            _ => return Err(format!("unknown result type {t}")),
        })
    }
}

/// The wasm value class of a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    I32,
    I64,
    F32,
    F64,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::I32 => "i32",
            Kind::I64 => "i64",
            Kind::F32 => "f32",
            Kind::F64 => "f64",
        }
    }
    fn of(ty: Type) -> Result<Kind, String> {
        Ok(match ty {
            Type::I32 | Type::U32 | Type::Ptr => Kind::I32,
            Type::I64 | Type::U64 => Kind::I64,
            Type::F32 => Kind::F32,
            Type::F64 => Kind::F64,
            t => return Err(format!("narrow result type {t:?}")),
        })
    }
}

/// What one call did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Any wasm trap, or an uncaught wasm exception.
    Trap,
    Ret(Vec<Arg>),
}

impl Outcome {
    pub fn i32(v: i32) -> Outcome {
        Outcome::Ret(vec![Arg::I32(v)])
    }
    pub fn i64(v: i64) -> Outcome {
        Outcome::Ret(vec![Arg::I64(v)])
    }
    pub fn f32(v: f32) -> Outcome {
        Outcome::Ret(vec![Arg::f32(v)])
    }
    pub fn f64(v: f64) -> Outcome {
        Outcome::Ret(vec![Arg::f64(v)])
    }
}

// ───────────────────────────── cases and passes ─────────────────────────────

/// An end-to-end test case: module, entry point, inputs and expected outcomes.
pub struct Case {
    pub name: String,
    pub module: Module,
    /// Name of the exported function to call.
    pub export: String,
    /// One call (on a fresh instance) per entry: arguments and the outcome the
    /// native oracle says it must have.
    pub calls: Vec<(Vec<Arg>, Outcome)>,
    /// The function reads no memory or global it did not write itself, so
    /// WNJR may reuse one instance across inputs (re-created after a trap).
    /// Stateful cases get a fresh instance per input on both hosts.
    pub stateless: bool,
}

/// An IR-to-IR pass under test.
pub struct Pass {
    pub name: String,
    pub run: Box<dyn Fn(&Module) -> Module>,
}

impl Pass {
    pub fn new(name: &str, run: impl Fn(&Module) -> Module + 'static) -> Pass {
        Pass { name: name.to_string(), run: Box::new(run) }
    }
}

/// Run the whole pipeline on `module` with `passes`, returning wasm bytes.
pub fn compile(module: &Module, passes: &[Pass]) -> Result<Vec<u8>, String> {
    let mut m = njvm::computed_goto::lower_module(module).map_err(|e| format!("computed goto lowering: {e:?}"))?;
    njvm::verify_module(&m).map_err(|e| format!("verifier (input): {e}"))?;
    for p in passes {
        m = (p.run)(&m);
        njvm::verify_module(&m).map_err(|e| format!("verifier after pass {}: {e}", p.name))?;
    }
    m = njvm::setjmp::lower_module(&m).map_err(|e| format!("setjmp lowering: {e:?}"))?;
    njvm::verify_module(&m).map_err(|e| format!("verifier (after setjmp lowering): {e}"))?;
    njvm::encode::encode(&m).map_err(|e| format!("encode: {e}"))
}

/// The wasm result kinds of an exported function.
pub fn result_kinds(module: &Module, export: &str) -> Result<Vec<Kind>, String> {
    let e = module
        .exports
        .iter()
        .find(|e| e.name == export && e.kind == ExportKind::Function)
        .ok_or_else(|| format!("no function export {export:?}"))?;
    let local = e
        .index
        .checked_sub(module.imported_functions())
        .ok_or("exported function is an import")?;
    let f = module.functions.get(local as usize).ok_or("export index out of range")?;
    f.results.iter().map(|&t| Kind::of(t)).collect()
}

// ───────────────────────────── hosts ─────────────────────────────

/// Run `export` on WNJR once per input, each on a fresh instance. `Err`
/// means the bytes did not load (parse, validate or instantiate failed).
/// Call each `(export, args)` in order on one instance, so that a call sees
/// what an earlier call wrote — the same thing a C host does when it runs a
/// translation unit's functions one after another. With `fresh_per_call` every
/// call gets its own instance instead.
pub fn run_wnjr_sequence(
    bytes: &[u8],
    calls: &[(String, Vec<Arg>)],
    fresh_per_call: bool,
) -> Result<Vec<Outcome>, String> {
    let engine = Engine::new(Config::new()).map_err(|e| format!("engine: {e:?}"))?;
    let mut out = Vec::new();
    // Parse, validate and decode once; the instance is reused across calls.
    let parsed = WasmModule::new("njvm-e2e", bytes).map_err(|e| format!("WNJR Module::new: {e:?}"))?;
    let mut live: Option<(RuntimeWorld, _)> = None;
    for (export, args) in calls {
        if live.is_none() {
            let mut world = RuntimeWorld::new();
            let id = world.instantiate(&engine, parsed.clone(), &[]).map_err(|e| format!("WNJR instantiate: {e:?}"))?;
            live = Some((world, id));
        }
        let (world, id) = live.as_mut().expect("instance");
        let vals: Vec<Value> = args.iter().map(|a| a.to_value()).collect();
        let r = match world.invoke(*id, export, &vals) {
            Ok(vs) => Outcome::Ret(vs.iter().map(Arg::from_value).collect::<Result<_, _>>()?),
            Err(e) if e.is_trap() || e.is_exception() => Outcome::Trap,
            Err(e) => return Err(format!("WNJR invoke {export}: {e:?}")),
        };
        if fresh_per_call || r == Outcome::Trap {
            let (mut world, id) = live.take().expect("instance");
            let _ = world.free(id);
        }
        out.push(r);
    }
    if let Some((mut world, id)) = live {
        let _ = world.free(id);
    }
    Ok(out)
}

pub fn run_wnjr(bytes: &[u8], export: &str, inputs: &[Vec<Arg>], stateless: bool) -> Result<Vec<Outcome>, String> {
    // `stateless` is the case's own flag, and it reads backwards: the bun runner
    // builds a fresh instance per input for every case (bun/runner.js: "so memory
    // and globals start identical on every host"), so a case that is *not*
    // stateless is one whose calls must each start from a clean instance. A
    // stateless case may reuse one instance, which no case asks for yet — it
    // would stop matching bun.
    let calls: Vec<(String, Vec<Arg>)> = inputs.iter().map(|a| (export.to_string(), a.clone())).collect();
    run_wnjr_sequence(bytes, &calls, !stateless)
}
/// One module to run on a host.
pub struct Job {
    pub bytes: Vec<u8>,
    pub export: String,
    pub ret: Vec<Kind>,
    pub inputs: Vec<Vec<Arg>>,
    pub stateless: bool,
}

impl Job {
    fn to_json(&self) -> serde_json::Value {
        let hex = self.bytes.iter().map(|x| format!("{x:02x}")).collect::<String>();
        serde_json::json!({
            "hex": hex,
            "export": self.export,
            "ret": self.ret.iter().map(|k| k.name()).collect::<Vec<_>>(),
            "inputs": self.inputs.iter().map(|a| a.iter().map(|x| x.to_json()).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "stateless": self.stateless,
        })
    }
}

fn outcome_to_json(o: &Outcome) -> serde_json::Value {
    match o {
        Outcome::Trap => serde_json::json!({ "trap": true }),
        Outcome::Ret(v) => serde_json::json!({ "ret": v.iter().map(|x| x.to_json()).collect::<Vec<_>>() }),
    }
}

fn bun_command() -> String {
    std::env::var("NJVM_E2E_BUN").unwrap_or_else(|_| "bun".to_string())
}

/// `bun --version`, or an error when bun cannot be started.
pub fn bun_version() -> Result<String, String> {
    let out = Command::new(bun_command()).arg("--version").output().map_err(|e| format!("cannot start bun: {e}"))?;
    if !out.status.success() {
        return Err(format!("bun --version failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ───────────────────────────── host processes ─────────────────────────────

/// Both hosts run in child processes. Neither WNJR nor bun can interrupt a
/// running wasm function from inside the process, and a pass under test may
/// well produce an infinite loop; a child can be killed.
#[derive(Clone, Copy, Debug)]
pub enum Host {
    Wnjr,
    Bun,
}

impl Host {
    fn name(self) -> &'static str {
        match self {
            Host::Wnjr => "WNJR",
            Host::Bun => "bun",
        }
    }
}

static WNJR_RUNNER: OnceLock<String> = OnceLock::new();

/// Tell the harness where the `njvm-e2e-wnjr` binary of this crate is. Cargo
/// gives integration tests its path as `env!("CARGO_BIN_EXE_njvm-e2e-wnjr")`;
/// other users can set `NJVM_E2E_WNJR_RUNNER` instead.
pub fn set_wnjr_runner(path: &str) {
    let _ = WNJR_RUNNER.set(path.to_string());
}

fn host_command(host: Host) -> Result<Command, String> {
    match host {
        Host::Bun => {
            let mut c = Command::new(bun_command());
            c.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/bun/runner.js"));
            Ok(c)
        }
        Host::Wnjr => {
            let path = WNJR_RUNNER
                .get()
                .cloned()
                .or_else(|| std::env::var("NJVM_E2E_WNJR_RUNNER").ok())
                .ok_or("WNJR runner binary unknown: call set_wnjr_runner(env!(\"CARGO_BIN_EXE_njvm-e2e-wnjr\")) or set NJVM_E2E_WNJR_RUNNER")?;
            Ok(Command::new(path))
        }
    }
}

/// How long one job may go without producing its result line
/// (`NJVM_E2E_JOB_MS`, default 3000). A job that exceeds it is reported as
/// timed out (a loop that does not terminate), not as a harness failure.
pub fn job_deadline() -> Duration {
    let ms = std::env::var("NJVM_E2E_JOB_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    Duration::from_millis(ms)
}

fn parse_line(line: &str) -> Result<Result<Vec<Outcome>, String>, String> {
    let r: serde_json::Value = serde_json::from_str(line).map_err(|e| format!("host output is not JSON: {e}: {line}"))?;
    if let Some(e) = r.get("error") {
        return Ok(Err(e.as_str().unwrap_or("?").to_string()));
    }
    let outs = r["outs"].as_array().ok_or("job without outs")?;
    let mut v = Vec::new();
    for o in outs {
        if o.get("trap").is_some() {
            v.push(Outcome::Trap);
        } else {
            let rets = o["ret"].as_array().ok_or("outcome without ret")?;
            v.push(Outcome::Ret(rets.iter().map(Arg::from_json).collect::<Result<_, _>>()?));
        }
    }
    Ok(Ok(v))
}

/// Run all jobs on one host, in order. The host prints one JSON line per job
/// as soon as the job is done. A job that stays silent past [job_deadline] gets
/// its process killed and an `Err` result, and the remaining jobs go to a
/// fresh process. The outer `Err` is a harness failure (the host does not start).
pub fn run_jobs(host: Host, jobs: &[Job]) -> Result<Vec<Result<Vec<Outcome>, String>>, String> {
    let deadline = job_deadline();
    let mut results: Vec<Result<Vec<Outcome>, String>> = Vec::with_capacity(jobs.len());
    let mut start = 0;
    let mut dead_in_a_row = 0;
    while start < jobs.len() {
        let payload = serde_json::Value::Array(jobs[start..].iter().map(Job::to_json).collect()).to_string();
        let mut child = host_command(host)?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", host.name()))?;
        let mut stdin = child.stdin.take().expect("piped");
        let writer = thread::spawn(move || {
            let _ = stdin.write_all(payload.as_bytes());
        });
        let stdout = child.stdout.take().expect("piped");
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let mut stderr = child.stderr.take().expect("piped");
        let err_reader = thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            s
        });

        let mut got = 0;
        let mut timed_out = false;
        while start + got < jobs.len() {
            match rx.recv_timeout(deadline) {
                Ok(line) => {
                    results.push(parse_line(&line)?);
                    got += 1;
                }
                Err(RecvTimeoutError::Timeout) => {
                    timed_out = true;
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if timed_out {
            let _ = child.kill();
        }
        let status = child.wait().map_err(|e| format!("wait for {}: {e}", host.name()))?;
        let _ = writer.join();
        let _ = reader.join();
        let stderr_text = err_reader.join().unwrap_or_default();
        if start + got >= jobs.len() {
            break;
        }
        // The job at start + got did not answer.
        if timed_out {
            dead_in_a_row = 0;
            results.push(Err(format!("timed out after {} ms (does not terminate?)", deadline.as_millis())));
        } else {
            dead_in_a_row = if got == 0 { dead_in_a_row + 1 } else { 0 };
            if dead_in_a_row >= 3 {
                return Err(format!("{} died three times in a row ({status}): {stderr_text}", host.name()));
            }
            let tail: String = stderr_text.chars().rev().take(400).collect::<String>().chars().rev().collect();
            results.push(Err(format!("{} process died on this job ({status}): {tail}", host.name())));
        }
        start += got + 1;
    }
    Ok(results)
}

/// The body of the `njvm-e2e-wnjr` binary: read jobs (the same JSON the bun runner
/// reads) from stdin, run each on WNJR and print one result line per job.
pub fn wnjr_runner_main() -> Result<(), String> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|e| format!("stdin: {e}"))?;
    let jobs: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("stdin is not JSON: {e}"))?;
    let stdout = std::io::stdout();
    for job in jobs.as_array().ok_or("jobs is not an array")? {
        let line = (|| -> Result<Vec<Outcome>, String> {
            let hex = job["hex"].as_str().ok_or("job without hex")?;
            let bytes = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|e| e.to_string()))
                .collect::<Result<Vec<u8>, String>>()?;
            let export = job["export"].as_str().ok_or("job without export")?;
            let inputs = job["inputs"]
                .as_array()
                .ok_or("job without inputs")?
                .iter()
                .map(|a| a.as_array().ok_or("input is not an array".to_string())?.iter().map(Arg::from_json).collect::<Result<Vec<_>, _>>())
                .collect::<Result<Vec<_>, _>>()?;
            run_wnjr(&bytes, export, &inputs, job["stateless"].as_bool().unwrap_or(false))
        })();
        let json = match line {
            Ok(outs) => serde_json::json!({ "outs": outs.iter().map(outcome_to_json).collect::<Vec<_>>() }),
            Err(e) => serde_json::json!({ "error": e }),
        };
        let mut w = stdout.lock();
        writeln!(w, "{json}").map_err(|e| format!("stdout: {e}"))?;
        w.flush().map_err(|e| format!("stdout: {e}"))?;
    }
    Ok(())
}

// ───────────────────────────── running a suite ─────────────────────────────

/// What both hosts did with one case (`Err`: the case did not compile or a
/// host rejected the module).
pub struct CaseRun {
    pub wnjr: Result<Vec<Outcome>, String>,
    pub bun: Result<Vec<Outcome>, String>,
}

/// Compile every case with `passes` and run it on WNJR and on bun (the two hosts
/// run side by side, each in its own process).
pub fn execute(cases: &[Case], passes: &[Pass]) -> Result<Vec<CaseRun>, String> {
    let mut runs: Vec<Option<CaseRun>> = Vec::new();
    let mut jobs = Vec::new();
    let mut job_of = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let compiled = compile(&c.module, passes).and_then(|bytes| {
            let ret = result_kinds(&c.module, &c.export)?;
            Ok((bytes, ret))
        });
        match compiled {
            Err(e) => runs.push(Some(CaseRun { wnjr: Err(e.clone()), bun: Err(e) })),
            Ok((bytes, ret)) => {
                let inputs: Vec<Vec<Arg>> = c.calls.iter().map(|(a, _)| a.clone()).collect();
                runs.push(None);
                jobs.push(Job { bytes, export: c.export.clone(), ret, inputs, stateless: c.stateless });
                job_of.push(i);
            }
        }
    }
    let (wnjr, bun) = thread::scope(|s| {
        let w = s.spawn(|| run_jobs(Host::Wnjr, &jobs));
        let b = run_jobs(Host::Bun, &jobs);
        (w.join().expect("WNJR host thread"), b)
    });
    let (wnjr, bun) = (wnjr?, bun?);
    for ((i, w), b) in job_of.into_iter().zip(wnjr).zip(bun) {
        runs[i] = Some(CaseRun { wnjr: w, bun: b });
    }
    Ok(runs.into_iter().map(|r| r.expect("run")).collect())
}

// ───────────────────────────── comparing ─────────────────────────────

/// One disagreement.
#[derive(Clone, Debug)]
pub struct Mismatch {
    pub case: String,
    /// Index into the case's calls, when the disagreement is about one call.
    pub call: Option<usize>,
    /// "baseline" (no passes) or "with passes".
    pub run: &'static str,
    /// What was compared: "WNJR vs bun", "WNJR vs oracle", "bun vs oracle",
    /// "passes vs baseline (WNJR)", "passes vs baseline (bun)", "compile/load".
    pub what: String,
    pub left: String,
    pub right: String,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.run, self.case)?;
        if let Some(c) = self.call {
            write!(f, " call #{c}")?;
        }
        write!(f, ": {}: {} <> {}", self.what, self.left, self.right)
    }
}

fn cmp_outcomes(
    out: &mut Vec<Mismatch>,
    case: &str,
    run: &'static str,
    what: &str,
    a: &[Outcome],
    b: &[Outcome],
) {
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i), b.get(i));
        if x != y {
            out.push(Mismatch {
                case: case.to_string(),
                call: Some(i),
                run,
                what: what.to_string(),
                left: format!("{x:?}"),
                right: format!("{y:?}"),
            });
        }
    }
}

fn load_failures(out: &mut Vec<Mismatch>, case: &str, run: &'static str, r: &CaseRun) -> bool {
    let mut bad = false;
    for (host, res) in [("WNJR", &r.wnjr), ("bun", &r.bun)] {
        if let Err(e) = res {
            bad = true;
            out.push(Mismatch {
                case: case.to_string(),
                call: None,
                run,
                what: format!("compile/load on {host}"),
                left: e.clone(),
                right: String::new(),
            });
        }
    }
    bad
}

/// Compare one run of the suite: hosts against each other and (when
/// `oracle`) against the native oracle.
fn compare_run(cases: &[Case], runs: &[CaseRun], run: &'static str, oracle: bool) -> Vec<Mismatch> {
    let mut out = Vec::new();
    for (c, r) in cases.iter().zip(runs) {
        if load_failures(&mut out, &c.name, run, r) {
            continue;
        }
        let (w, b) = (r.wnjr.as_ref().unwrap(), r.bun.as_ref().unwrap());
        cmp_outcomes(&mut out, &c.name, run, "WNJR vs bun", w, b);
        if oracle {
            let expect: Vec<Outcome> = c.calls.iter().map(|(_, o)| o.clone()).collect();
            cmp_outcomes(&mut out, &c.name, run, "WNJR vs oracle", w, &expect);
            cmp_outcomes(&mut out, &c.name, run, "bun vs oracle", b, &expect);
        }
    }
    out
}

/// Differential check of a pass list. Runs every case with no passes (hosts
/// against each other and against the oracle), then with `passes` (hosts
/// against each other, and each host against its own baseline). Empty result:
/// everything agrees. `Err`: the harness itself could not run (bun missing).
pub fn check(cases: &[Case], passes: &[Pass]) -> Result<Vec<Mismatch>, String> {
    let base = execute(cases, &[])?;
    check_against(cases, &base, passes)
}

/// Like [check] with the baseline already run.
pub fn check_against(cases: &[Case], base: &[CaseRun], passes: &[Pass]) -> Result<Vec<Mismatch>, String> {
    let mut out = compare_run(cases, base, "baseline", true);
    if passes.is_empty() {
        return Ok(out);
    }
    let opt = execute(cases, passes)?;
    out.extend(compare_run(cases, &opt, "with passes", false));
    for (c, (b, o)) in cases.iter().zip(base.iter().zip(&opt)) {
        for (host, bb, oo) in [("WNJR", &b.wnjr, &o.wnjr), ("bun", &b.bun, &o.bun)] {
            if let (Ok(x), Ok(y)) = (bb, oo) {
                cmp_outcomes(&mut out, &c.name, "with passes", &format!("passes vs baseline ({host})"), x, y);
            }
        }
    }
    Ok(out)
}

/// The differential for each pass alone and then for the whole list, so a
/// failure names the pass that caused it. Returns `(label, mismatches)`.
pub fn check_each(cases: &[Case], passes: &[Pass]) -> Result<Vec<(String, Vec<Mismatch>)>, String> {
    let base = execute(cases, &[])?;
    let mut out = Vec::new();
    if passes.len() > 1 {
        for p in passes {
            let one = std::slice::from_ref(p);
            out.push((p.name.clone(), check_against(cases, &base, one)?));
        }
    }
    let label = passes.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(" + ");
    out.push((label, check_against(cases, &base, passes)?));
    Ok(out)
}

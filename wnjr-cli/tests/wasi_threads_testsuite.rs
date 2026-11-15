//! Runs the vendored wasi-threads test suite through the real CLI binary.
//!
//! Every case is a `.wat` module plus a `.json` holding the exit code it must
//! produce. The runner encodes the module, runs it as a child process and
//! compares the child's exit status with the expected number - the assertion is
//! on the value, not on "it finished".
//!
//! Out of process is not incidental: most of these cases check that `proc_exit`
//! called from one thread terminates the others, including threads parked in
//! `wait32` or spinning in a guest loop. Only a process boundary can observe
//! that.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// No case in the suite should take longer than this; a case that does is a
/// hang and must fail rather than wedge the test run.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wasi-threads-testsuite")
}

/// Where encoded modules go. Under the workspace target directory so nothing
/// lands outside the repo.
fn artifact_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/wasi-threads-testsuite");
    std::fs::create_dir_all(&dir).expect("create the artifact directory");
    dir
}

struct Outcome {
    exit_code: Option<i32>,
    timed_out: bool,
    stderr: String,
}

fn run_case(cli: &str, wasm: &Path) -> Outcome {
    let mut child = Command::new(cli)
        .arg(wasm)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("could not start {}: {error}", cli));
    // Hold the write end of the guest's stdin open without writing to it, so a
    // guest reading fd 0 blocks instead of seeing end of file. One case relies
    // on exactly that: it exits the process from a worker thread while main is
    // parked in fd_read.
    let _stdin = child.stdin.take();

    let deadline = Instant::now() + CASE_TIMEOUT;
    loop {
        match child.try_wait().expect("could not poll the child process") {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Outcome {
                    exit_code: None,
                    timed_out: true,
                    stderr: String::new(),
                };
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }

    let output = child
        .wait_with_output()
        .expect("could not collect the child's output");
    Outcome {
        exit_code: output.status.code(),
        timed_out: false,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn expected_exit_code(wat: &Path) -> i32 {
    let json = wat.with_extension("json");
    if !json.exists() {
        // The suite has no .json for the "return from _start" cases; the
        // wasi-threads rule is that returning from _start is proc_exit(0).
        return 0;
    }
    let text = std::fs::read_to_string(&json)
        .unwrap_or_else(|error| panic!("{}: {error}", json.display()));
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("parse the expected json");
    parsed["exit_code"]
        .as_i64()
        .unwrap_or_else(|| panic!("{} has no exit_code", json.display())) as i32
}

#[test]
fn wasi_threads_testsuite() {
    let cli = env!("CARGO_BIN_EXE_wnjr-cli");
    let suite = suite_dir();
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&suite)
        .expect("the vendored wasi-threads suite is missing")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wat"))
        // macOS writes an AppleDouble "._name" sidecar next to every file when
        // the checkout lives on a volume that has no native resource forks. It
        // has the same extension as the case it shadows, so skip it by name.
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("._"))
        })
        .collect();
    cases.sort();

    // A missing or half-copied suite must not pass by finding nothing.
    assert!(
        cases.len() >= 14,
        "expected the whole wasi-threads suite, found {} cases in {}",
        cases.len(),
        suite.display()
    );

    let artifacts = artifact_dir();
    let mut failures: Vec<String> = Vec::new();
    for wat in &cases {
        let name = wat.file_stem().unwrap().to_string_lossy().into_owned();
        let expected = expected_exit_code(wat);
        let wasm = artifacts.join(format!("{name}.wasm"));
        let bytes = match wat::parse_file(wat) {
            Ok(bytes) => bytes,
            Err(error) => {
                failures.push(format!("{name}: could not encode the module: {error}"));
                continue;
            }
        };
        std::fs::write(&wasm, &bytes).expect("write the encoded module");

        let outcome = run_case(cli, &wasm);
        if outcome.timed_out {
            failures.push(format!(
                "{name}: hung for {:?} (expected exit {expected})",
                CASE_TIMEOUT
            ));
            continue;
        }
        match outcome.exit_code {
            Some(code) if code == expected => {}
            Some(code) => failures.push(format!(
                "{name}: exited {code}, expected {expected}\n{}",
                outcome.stderr.trim_end()
            )),
            None => failures.push(format!(
                "{name}: died on a signal instead of exiting {expected}\n{}",
                outcome.stderr.trim_end()
            )),
        }
    }

    // Print per-case results
    let mut passed = Vec::new();
    for wat in &cases {
        let name = wat.file_stem().unwrap().to_string_lossy().into_owned();
        if !failures.iter().any(|f| f.starts_with(&format!("{name}:"))) {
            passed.push(name);
        }
    }
    eprintln!("\n=== wasi-threads testsuite results ===");
    for name in &passed {
        eprintln!("PASS  {}", name);
    }
    for failure in &failures {
        eprintln!("FAIL  {}", failure.lines().next().unwrap_or(failure));
    }
    eprintln!(
        "\nTotal: {} pass, {} fail, {} unsupported",
        passed.len(),
        failures.len(),
        0
    );
    eprintln!("(All cases have expected semantics; unsupported count = 0 for this suite.)\n");

    assert!(
        failures.is_empty(),
        "{} of {} wasi-threads cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

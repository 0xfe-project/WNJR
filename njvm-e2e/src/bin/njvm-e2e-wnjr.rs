//! Child-process WNJR host for the e2e harness: jobs on stdin, one result
//! line per job on stdout. See `njvm_e2e::wnjr_runner_main`.

fn main() {
    if let Err(e) = njvm_e2e::wnjr_runner_main() {
        eprintln!("njvm-e2e-wnjr: {e}");
        std::process::exit(2);
    }
}

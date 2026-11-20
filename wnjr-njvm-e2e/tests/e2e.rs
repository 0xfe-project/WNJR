//! End-to-end NJVM-IR -> wasm: WNJR against bun against a native oracle.
//!
//! Needs bun on PATH (tested with bun 1.4.4, JavaScriptCore, wasm EH with
//! try_table/exnref). Run: `cargo test --offline -p wnjr-njvm-e2e`.
//! No network is used.

use wnjr_njvm_e2e::walk::{commute_pure_operands, swap_add_for_sub};
use wnjr_njvm_e2e::{bun_version, cases, check, Mismatch, Pass};

fn init() {
    wnjr_njvm_e2e::set_wnjr_runner(env!("CARGO_BIN_EXE_njvm-e2e-wnjr"));
}

fn report(ms: &[Mismatch]) -> String {
    ms.iter().take(20).map(|m| m.to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
fn bun_is_available_and_supports_wasm_eh() {
    let v = bun_version().expect("bun must be installed");
    eprintln!("bun {v}");
    // The setjmp cases below only run if bun accepts try_table; a missing EH
    // support shows up as a compile/load mismatch there, not as a skip.
}

/// Every case, no passes: WNJR == bun == the native oracle.
#[test]
fn baseline_wnjr_equals_bun_equals_oracle() {
    init();
    let all = cases::all();
    assert!(all.len() > 100, "case library shrank: {}", all.len());
    let ms = check(&all, &[]).expect("harness");
    assert!(ms.is_empty(), "{} mismatches:\n{}", ms.len(), report(&ms));
}

/// The differential for a correct, non-trivial pass reports nothing.
#[test]
fn correct_pass_is_clean() {
    init();
    let all = cases::all();
    let passes = [Pass::new("commute-pure-operands", commute_pure_operands)];
    let ms = check(&all, &passes).expect("harness");
    assert!(ms.is_empty(), "{} mismatches:\n{}", ms.len(), report(&ms));
}

/// Self-test: a pass that turns every Add into Sub still verifies and
/// encodes, and the harness must report it.
#[test]
fn wrong_pass_is_reported() {
    init();
    let all = cases::all();
    let passes = [Pass::new("add-to-sub", swap_add_for_sub)];
    let ms = check(&all, &passes).expect("harness");
    assert!(!ms.is_empty(), "the harness accepted a pass that rewrites Add to Sub");
    // It must be caught as a change against the baseline, on both hosts.
    for host in ["WNJR", "bun"] {
        let what = format!("passes vs baseline ({host})");
        assert!(ms.iter().any(|m| m.what == what), "{host}: no baseline difference reported");
    }
    let cases_hit: std::collections::BTreeSet<_> = ms.iter().map(|m| m.case.as_str()).collect();
    assert!(cases_hit.contains("control/loop_sum"), "loop_sum not caught: {cases_hit:?}");
}

/// With several passes, check_each names the one that breaks things: the
/// correct pass is clean on its own, the wrong one is not, and so is the list.
#[test]
fn check_each_names_the_guilty_pass() {
    init();
    let all = cases::all();
    let passes = [Pass::new("commute-pure-operands", commute_pure_operands), Pass::new("add-to-sub", swap_add_for_sub)];
    let per = wnjr_njvm_e2e::check_each(&all, &passes).expect("harness");
    let get = |name: &str| per.iter().find(|(l, _)| l == name).unwrap_or_else(|| panic!("no entry {name}: {:?}", per.iter().map(|p| &p.0).collect::<Vec<_>>())).1.len();
    assert_eq!(get("commute-pure-operands"), 0, "the correct pass was blamed");
    assert!(get("add-to-sub") > 0, "the wrong pass was not blamed");
    assert!(get("commute-pure-operands + add-to-sub") > 0, "the combined list was not reported");
}

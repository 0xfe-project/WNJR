# wnjr-njvm-e2e

End-to-end differential harness for NJVM-IR (task #57, A5/N10).

Hand-built IR goes through the real pipeline and the resulting wasm runs on two hosts. The outcomes are compared with each other and with a native Rust oracle computed per call.

```
computed goto lowering -> verify -> [ passes under test, verify after each ] ->
setjmp lowering -> verify -> encode (erase, stackify, switch, load/store, call) -> wasm bytes
                                                                   |-> WNJR (Module::new, instantiate, invoke)
                                                                   '-> bun  (WebAssembly.Module / Instance)
```

## Hosts and how to run

- bun 1.4.4 (revision 38afee6c84fcf48c0edb2466aa4b27d287a59c30), at `~/.bun/bin/bun`. It supports wasm EH (`try_table`, `catch`, `catch_all_ref`, `throw_ref`), which the setjmp cases need. There is no fallback host: a missing bun, or one without EH, fails the tests. Set `NJVM_E2E_BUN` to use another binary.
- WNJR runs in a child process (`njvm-e2e-wnjr`, built by this crate), not in the test process. Neither host can interrupt a running wasm function, and a pass under test may emit an infinite loop. The harness kills a host that is silent for `NJVM_E2E_JOB_MS` (default 3000 ms) on one case, reports that case as timed out, and restarts the host on the remaining cases.
- Run, offline:

```
cargo test --offline -p wnjr-njvm-e2e
```

The whole suite takes about 2 minutes (111 s on the arm64 dev machine); most of it is the wrong-pass tests waiting out the per-case deadline.

## Using it as an optimisation-pass benchmark

```rust
let passes = [Pass::new("my-pass", |m: &Module| my_pass(m))];
let mismatches = wnjr_njvm_e2e::check(&cases::all(), &passes)?;     // empty = clean
let per_pass   = wnjr_njvm_e2e::check_each(&cases::all(), &passes)?; // each pass alone, then the list
```

Passes are IR-to-IR (`&Module -> Module`). They run after computed goto lowering and before setjmp lowering, and the verifier runs after each. `check` runs every case with no passes (hosts against each other and against the oracle) and with the list (hosts against each other, and each host against its own baseline). Floats compare by bits, any NaN equal to any NaN.

Cases that touch memory, globals or setjmp buffers are built with `stateless: false`: they get a fresh instance per input on both hosts. The others reuse one instance per module.

## Cases (`src/cases.rs`)

Arithmetic of i32/u32/i64/u64 (every binary and unary operator, against a native oracle including traps), f32/f64, all conversions, narrow types, select. Control flow: loops, nested loops, diamond, forward and backward goto, BrTable (with fall-through, inside a loop), Switch (dense, sparse, u32 across the signed boundary), computed goto (select, loop, threaded dispatch). Memory: narrow loads and stores, float/int reinterpretation, bounds traps, tagged pointers (tag in the high 32 bits) for load/store, arrays, memory.fill and memory.copy. Calls: recursion, mutual recursion, mixed argument types, call_indirect (wrong signature, null slot, out of bounds), globals. Exceptions (try/catch). setjmp/longjmp: single level, two levels, two sites, in a loop, foreign label rethrown, unset buffer.

## Tests (`tests/e2e.rs`)

- `baseline_wnjr_equals_bun_equals_oracle`: all cases, no passes.
- `correct_pass_is_clean`: a correct pass (commute pure operands) reports nothing.
- `wrong_pass_is_reported`: self-test. A pass that rewrites every Add to Sub still verifies and encodes. The harness must report it against the baseline on both hosts.
- `check_each_names_the_guilty_pass`: with two passes, the correct one is clean alone and the wrong one is blamed.

## Falsification (manual, done once)

Each key step below was commented out or altered in `wnjr-core`, `baseline_wnjr_equals_bun_equals_oracle` was run, and the file was restored with `git checkout`. All nine went red. The first lines of each red output:

### 01 computed_goto: dispatch index sub -> add: RED

```
26 mismatches:
[baseline] control/computed_goto_select call #0: WNJR vs oracle: Some(Ret([I32(30)])) <> Some(Ret([I32(10)]))
[baseline] control/computed_goto_select call #1: WNJR vs oracle: Some(Trap) <> Some(Ret([I32(20)]))
```

### 02 setjmp: resumed setjmp returns 0 instead of longjmp value: RED

```
10 mismatches:
[baseline] setjmp/single_level: compile/load on WNJR: timed out after 3000 ms (does not terminate?) <> 
[baseline] setjmp/single_level: compile/load on bun: timed out after 3000 ms (does not terminate?) <> 
```

### 03 stackify: back edges no longer mark loop heads: RED

```
28 mismatches:
[baseline] control/loop_sum: compile/load on WNJR: encode: function 0: Stackify(DoubleEmit(1)) <> 
[baseline] control/loop_sum: compile/load on bun: encode: function 0: Stackify(DoubleEmit(1)) <> 
```

### 04 erase/addr: tagged pointer not wrapped to i32: RED

```
6 mismatches:
[baseline] memory/tagged_load_store: compile/load on WNJR: WNJR Module::new: WasmError { repr: Invalid("type mismatch: expected , got") } <> 
[baseline] memory/tagged_load_store: compile/load on bun: CompileError: WebAssembly.Module doesn't validate: I32Store pointer type mismatch, in function at index 0 (evaluating 'new WebAssembly.Module(
```

### 05 erase/switch: dense br_table base subtract -> add: RED

```
10 mismatches:
[baseline] control/switch_u32_boundary call #0: WNJR vs oracle: Some(Ret([I32(0)])) <> Some(Ret([I32(1)]))
[baseline] control/switch_u32_boundary call #1: WNJR vs oracle: Some(Ret([I32(0)])) <> Some(Ret([I32(2)]))
```

### 06 erase/binary: i32 signed/unsigned div swapped: RED

```
144 mismatches:
[baseline] arith/i32/div call #14: WNJR vs oracle: Some(Ret([I32(0)])) <> Some(Ret([I32(-1)]))
[baseline] arith/i32/div call #27: WNJR vs oracle: Some(Ret([I32(2147483647)])) <> Some(Ret([I32(0)]))
```

### 07 erase/call: direct call uses function 0: RED

```
16 mismatches:
[baseline] call/mutual_recursion call #1: WNJR vs oracle: Some(Ret([I32(1)])) <> Some(Ret([I32(0)]))
[baseline] call/mutual_recursion call #3: WNJR vs oracle: Some(Ret([I32(1)])) <> Some(Ret([I32(0)]))
```

### 08 erase/convert: i8 normalisation dropped: RED

```
24 mismatches:
[baseline] convert/narrow_i8 call #4: WNJR vs oracle: Some(Ret([I32(128)])) <> Some(Ret([I32(-128)]))
[baseline] convert/narrow_i8 call #5: WNJR vs oracle: Some(Ret([I32(255)])) <> Some(Ret([I32(-1)]))
```

### 09 encode/elements: active element segment loses its first function: RED

```
4 mismatches:
[baseline] call/call_indirect call #0: WNJR vs oracle: Some(Ret([I32(36)])) <> Some(Ret([I32(5)]))
[baseline] call/call_indirect call #1: WNJR vs oracle: Some(Trap) <> Some(Ret([I32(36)]))
```


## Known defects

- The optimisation passes O1 (CFG simplify), O2 (fold + DCE), O3 (inline) and O4 (spill) are not in this tree, so there is no falsification against them. Only the correct and the wrong test passes exercise the pass list.
- Falsification covers computed goto, setjmp, stackify, erase (address wrap, switch, binary, call, convert) and encode (elements). It was done by hand, not as an automated mutation test.
- The harness does not compare memory after a call, only return values and traps.

# wcc's NJVM-IR on WNJR against native clang -O1

The same C file built two ways and timed as a whole process:

- **native** — `clang -O1` straight to machine code.
- **wnjr** — `wcc` to NJVM-IR, then WNJR's JIT through `njvm-cli run`.

Both columns must print the same checksum. That is checked before any time is looked at, and the run stops if they disagree.

Machine: Apple M5, arm64, darwin 25.5.0, 32 GiB. 3 runs each after 3 discarded warm-up runs, 2026-11-22 02:07 UTC.

Times are the **fastest** of the runs. Nothing about timing is ever faster than the work, so the fastest run is the one least polluted by the machine; the median and every sample are at the bottom of this file so the spread stays visible.

## Wall clock per process

| benchmark | checksum | native ms | wnjr ms | wnjr / native |
|---|---|---:|---:|---:|
| startup | 1 | 1.1 | 74.9 | 70.35× |
| matmul | 10276522263000 | 149.1 | 225.6 | 1.51× |
| pi | 452413269 | 137.1 | 345.9 | 2.52× |
| mergesort | 4579674966457524707 | 46.7 | 164.4 | 3.52× |
| nqueens | 365596 | 111.3 | 199.1 | 1.79× |

The **startup** row is what a program that computes nothing costs: the price of the process, and inside the wnjr column also of decoding and compiling the module. Subtracting it leaves the work itself:

| benchmark | native work ms | wnjr work ms | wnjr / native |
|---|---:|---:|---:|
| matmul | 148.0 | 150.7 | 1.02× |
| pi | 136.0 | 271.0 | 1.99× |
| mergesort | 45.7 | 89.5 | 1.96× |
| nqueens | 110.3 | 124.2 | 1.13× |

## Compiling the benchmark, for scale

These are not comparable to each other: clang compiles one short file, wcc compiles that file plus all 275 translation units of the libc, because wcc links nothing yet.

| benchmark | clang -O1 ms | wcc ms (including the whole libc) |
|---|---:|---:|
| startup | 209.0 | 941.3 |
| matmul | 56.6 | 805.2 |
| pi | 43.2 | 805.8 |
| mergesort | 50.5 | 849.2 |
| nqueens | 48.0 | 808.2 |

## What this does and does not say

- It says how fast the whole of wcc's output runs on WNJR next to LLVM's, which is the question being asked. It does not split that gap between the IR wcc writes and the code WNJR's JIT makes of it. Splitting it needs the JIT instrumented or its input held constant while one thing changes, not another column of numbers.
- The gap has one part that is not about the benchmarks at all: the wnjr column pays for decoding and compiling a module of the whole libc inside every process, which is most of the **startup** row. A real program pays it once; these pay it every run, and the work-subtracted table above is what is left after taking it off.
- The two columns differ in more than one thing at once: wcc has no optimizer at all, WNJR's JIT is a young backend, and the two programs do not even run the same instructions. A ratio here is a starting point, not a diagnosis.
- Nothing here is comparable to `bench/` at the top of the repository. Those numbers are Rust programs compiled to wasm by rustc, and they measure WNJR against other wasm runtimes. These are C programs written by a compiler with no optimization passes, measured against LLVM.
- Raw samples:

  - startup: native min 1.1 median 1.1 [1.2, 1.1, 1.1] ms; wnjr min 74.9 median 77.5 [80.1, 74.9, 77.5] ms
  - matmul: native min 149.1 median 155.3 [155.3, 155.5, 149.1] ms; wnjr min 225.6 median 230.1 [225.6, 230.1, 237.5] ms
  - pi: native min 137.1 median 141.3 [137.1, 141.3, 158.3] ms; wnjr min 345.9 median 349.1 [353.5, 349.1, 345.9] ms
  - mergesort: native min 46.7 median 47.0 [47.0, 47.4, 46.7] ms; wnjr min 164.4 median 165.3 [165.3, 165.5, 164.4] ms
  - nqueens: native min 111.3 median 111.5 [111.5, 111.3, 112.6] ms; wnjr min 199.1 median 202.9 [204.5, 199.1, 202.9] ms

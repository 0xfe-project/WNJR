# wcc's NJVM-IR on WNJR against native clang -O1

The same C file built two ways and timed as a whole process:

- **native** — `clang -O1` straight to machine code.
- **wnjr** — `wcc` to NJVM-IR, then WNJR's JIT through `njvm-run`.

Both columns must print the same checksum. That is checked before any time is looked at, and the run stops if they disagree.

Machine: Apple M5, arm64, darwin 25.5.0, 32 GiB. 5 runs each after 3 discarded warm-up runs, 2026-11-21 16:47 UTC.

Times are the **fastest** of the runs. Nothing about timing is ever faster than the work, so the fastest run is the one least polluted by the machine; the median and every sample are at the bottom of this file so the spread stays visible.

## Wall clock per process

| benchmark | checksum | native ms | wnjr ms | wnjr / native |
|---|---|---:|---:|---:|
| startup | 1 | 1.4 | 111.8 | 77.50× |
| matmul | 10276522263000 | 151.4 | 533.6 | 3.52× |
| pi | 452413269 | 136.8 | 1198.0 | 8.76× |
| mergesort | 4579674966457524707 | 47.7 | 325.1 | 6.81× |
| nqueens | 365596 | 110.3 | 781.0 | 7.08× |

The **startup** row is what a program that computes nothing costs: the price of the process, and inside the wnjr column also of decoding and compiling the module. Subtracting it leaves the work itself:

| benchmark | native work ms | wnjr work ms | wnjr / native |
|---|---:|---:|---:|
| matmul | 150.0 | 421.8 | 2.81× |
| pi | 135.4 | 1086.2 | 8.02× |
| mergesort | 46.3 | 213.4 | 4.61× |
| nqueens | 108.8 | 669.2 | 6.15× |

## Compiling the benchmark, for scale

These are not comparable to each other: clang compiles one short file, wcc compiles that file plus all 275 translation units of the libc, because wcc links nothing yet.

| benchmark | clang -O1 ms | wcc ms (including the whole libc) |
|---|---:|---:|
| startup | 45.2 | 859.3 |
| matmul | 58.5 | 840.2 |
| pi | 42.0 | 896.3 |
| mergesort | 52.9 | 863.4 |
| nqueens | 37.3 | 797.7 |

## What this does and does not say

- It says how fast the whole of wcc's output runs on WNJR next to LLVM's, which is the question being asked. It does not split that gap between the IR wcc writes and the code WNJR's JIT makes of it. Splitting it needs the JIT instrumented or its input held constant while one thing changes, not another column of numbers.
- The gap has one part that is not about the benchmarks at all: the wnjr column pays for decoding and compiling a module of the whole libc inside every process, which is most of the **startup** row. A real program pays it once; these pay it every run, and the work-subtracted table above is what is left after taking it off.
- The two columns differ in more than one thing at once: wcc has no optimizer at all, WNJR's JIT is a young backend, and the two programs do not even run the same instructions. A ratio here is a starting point, not a diagnosis.
- Nothing here is comparable to `bench/` at the top of the repository. Those numbers are Rust programs compiled to wasm by rustc, and they measure WNJR against other wasm runtimes. These are C programs written by a compiler with no optimization passes, measured against LLVM.
- Raw samples:

  - startup: native min 1.4 median 1.6 [1.6, 1.9, 1.7, 1.5, 1.4] ms; wnjr min 111.8 median 115.3 [116.2, 111.8, 114.2, 116.2, 115.3] ms
  - matmul: native min 151.4 median 158.9 [160.8, 151.4, 158.8, 158.9, 160.8] ms; wnjr min 533.6 median 546.7 [533.6, 548.7, 550.7, 541.5, 546.7] ms
  - pi: native min 136.8 median 137.3 [139.2, 137.1, 137.3, 137.8, 136.8] ms; wnjr min 1198.0 median 1204.3 [1204.3, 1202.0, 1215.2, 1198.0, 1207.4] ms
  - mergesort: native min 47.7 median 47.9 [47.9, 49.3, 47.7, 48.4, 47.8] ms; wnjr min 325.1 median 326.9 [326.9, 326.8, 329.3, 325.1, 327.9] ms
  - nqueens: native min 110.3 median 120.1 [120.1, 122.8, 123.0, 116.5, 110.3] ms; wnjr min 781.0 median 789.4 [788.9, 789.4, 781.0, 790.3, 792.6] ms

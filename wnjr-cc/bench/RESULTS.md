# wcc's NJVM-IR on WNJR against native clang -O1

The same C file built two ways and timed as a whole process:

- **native** — `clang -O1` straight to machine code.
- **wnjr** — `wcc` to NJVM-IR, then WNJR's JIT through `njvm-run`.

Both columns must print the same checksum. That is checked before any time is looked at, and the run stops if they disagree.

Machine: Apple M5, arm64, darwin 25.5.0, 32 GiB. 5 runs each after 3 discarded warm-up runs, 2026-11-21 18:11 UTC.

Times are the **fastest** of the runs. Nothing about timing is ever faster than the work, so the fastest run is the one least polluted by the machine; the median and every sample are at the bottom of this file so the spread stays visible.

## Wall clock per process

| benchmark | checksum | native ms | wnjr ms | wnjr / native |
|---|---|---:|---:|---:|
| startup | 1 | 1.9 | 83.5 | 43.37× |
| matmul | 10276522263000 | 140.5 | 251.1 | 1.79× |
| pi | 452413269 | 137.9 | 353.4 | 2.56× |
| mergesort | 4579674966457524707 | 46.4 | 221.2 | 4.76× |
| nqueens | 365596 | 109.9 | 618.0 | 5.62× |

The **startup** row is what a program that computes nothing costs: the price of the process, and inside the wnjr column also of decoding and compiling the module. Subtracting it leaves the work itself:

| benchmark | native work ms | wnjr work ms | wnjr / native |
|---|---:|---:|---:|
| matmul | 138.6 | 167.6 | 1.21× |
| pi | 136.0 | 269.8 | 1.98× |
| mergesort | 44.5 | 137.7 | 3.09× |
| nqueens | 108.0 | 534.5 | 4.95× |

## Compiling the benchmark, for scale

These are not comparable to each other: clang compiles one short file, wcc compiles that file plus all 275 translation units of the libc, because wcc links nothing yet.

| benchmark | clang -O1 ms | wcc ms (including the whole libc) |
|---|---:|---:|
| startup | 43.1 | 940.1 |
| matmul | 42.6 | 801.3 |
| pi | 42.1 | 813.7 |
| mergesort | 50.3 | 787.5 |
| nqueens | 40.5 | 775.2 |

## What this does and does not say

- It says how fast the whole of wcc's output runs on WNJR next to LLVM's, which is the question being asked. It does not split that gap between the IR wcc writes and the code WNJR's JIT makes of it. Splitting it needs the JIT instrumented or its input held constant while one thing changes, not another column of numbers.
- The gap has one part that is not about the benchmarks at all: the wnjr column pays for decoding and compiling a module of the whole libc inside every process, which is most of the **startup** row. A real program pays it once; these pay it every run, and the work-subtracted table above is what is left after taking it off.
- The two columns differ in more than one thing at once: wcc has no optimizer at all, WNJR's JIT is a young backend, and the two programs do not even run the same instructions. A ratio here is a starting point, not a diagnosis.
- Nothing here is comparable to `bench/` at the top of the repository. Those numbers are Rust programs compiled to wasm by rustc, and they measure WNJR against other wasm runtimes. These are C programs written by a compiler with no optimization passes, measured against LLVM.
- Raw samples:

  - startup: native min 1.9 median 2.5 [3.0, 3.1, 2.5, 2.1, 1.9] ms; wnjr min 83.5 median 85.0 [85.3, 83.5, 84.6, 85.0, 85.3] ms
  - matmul: native min 140.5 median 141.9 [145.9, 141.9, 141.9, 140.5, 148.5] ms; wnjr min 251.1 median 253.8 [251.1, 253.8, 255.5, 253.0, 255.0] ms
  - pi: native min 137.9 median 138.2 [138.4, 137.9, 138.3, 138.0, 138.2] ms; wnjr min 353.4 median 355.0 [353.4, 353.7, 355.0, 356.5, 357.0] ms
  - mergesort: native min 46.4 median 46.5 [46.5, 46.4, 47.1, 46.5, 46.5] ms; wnjr min 221.2 median 222.1 [222.1, 225.4, 221.2, 222.5, 221.3] ms
  - nqueens: native min 109.9 median 110.4 [110.3, 110.4, 109.9, 110.6, 110.8] ms; wnjr min 618.0 median 621.1 [621.1, 618.0, 618.8, 622.1, 625.1] ms

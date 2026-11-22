# wcc's NJVM-IR on WNJR against native clang -O1

The same C file built two ways and timed as a whole process:

- **native** — `clang -O1` straight to machine code.
- **wnjr** — `wcc` to NJVM-IR, then WNJR's JIT through `njvm-run`.

Both columns must print the same checksum. That is checked before any time is looked at, and the run stops if they disagree.

Machine: Apple M5, arm64, darwin 25.5.0, 32 GiB. 5 runs each after 3 discarded warm-up runs, 2026-11-22 01:51 UTC.

Times are the **fastest** of the runs. Nothing about timing is ever faster than the work, so the fastest run is the one least polluted by the machine; the median and every sample are at the bottom of this file so the spread stays visible.

## Wall clock per process

| benchmark | checksum | native ms | wnjr ms | wnjr / native |
|---|---|---:|---:|---:|
| startup | 1 | 2.3 | 72.6 | 32.02× |
| matmul | 10276522263000 | 151.5 | 222.7 | 1.47× |
| pi | 452413269 | 136.3 | 342.1 | 2.51× |
| mergesort | 4579674966457524707 | 45.5 | 174.7 | 3.84× |
| nqueens | 365596 | 109.1 | 194.8 | 1.79× |

The **startup** row is what a program that computes nothing costs: the price of the process, and inside the wnjr column also of decoding and compiling the module. Subtracting it leaves the work itself:

| benchmark | native work ms | wnjr work ms | wnjr / native |
|---|---:|---:|---:|
| matmul | 149.2 | 150.2 | 1.01× |
| pi | 134.0 | 269.5 | 2.01× |
| mergesort | 43.2 | 102.1 | 2.36× |
| nqueens | 106.8 | 122.3 | 1.14× |

## Compiling the benchmark, for scale

These are not comparable to each other: clang compiles one short file, wcc compiles that file plus all 275 translation units of the libc, because wcc links nothing yet.

| benchmark | clang -O1 ms | wcc ms (including the whole libc) |
|---|---:|---:|
| startup | 107.4 | 735.7 |
| matmul | 47.1 | 764.6 |
| pi | 47.9 | 863.0 |
| mergesort | 46.3 | 835.5 |
| nqueens | 44.0 | 795.6 |

## What this does and does not say

- It says how fast the whole of wcc's output runs on WNJR next to LLVM's, which is the question being asked. It does not split that gap between the IR wcc writes and the code WNJR's JIT makes of it. Splitting it needs the JIT instrumented or its input held constant while one thing changes, not another column of numbers.
- The gap has one part that is not about the benchmarks at all: the wnjr column pays for decoding and compiling a module of the whole libc inside every process, which is most of the **startup** row. A real program pays it once; these pay it every run, and the work-subtracted table above is what is left after taking it off.
- The two columns differ in more than one thing at once: wcc has no optimizer at all, WNJR's JIT is a young backend, and the two programs do not even run the same instructions. A ratio here is a starting point, not a diagnosis.
- Nothing here is comparable to `bench/` at the top of the repository. Those numbers are Rust programs compiled to wasm by rustc, and they measure WNJR against other wasm runtimes. These are C programs written by a compiler with no optimization passes, measured against LLVM.
- Raw samples:

  - startup: native min 2.3 median 2.7 [2.7, 2.7, 2.4, 2.7, 2.3] ms; wnjr min 72.6 median 87.0 [72.6, 87.0, 101.6, 106.2, 73.2] ms
  - matmul: native min 151.5 median 154.5 [154.5, 165.8, 151.9, 154.7, 151.5] ms; wnjr min 222.7 median 223.6 [225.5, 223.4, 225.2, 222.7, 223.6] ms
  - pi: native min 136.3 median 136.6 [137.7, 136.3, 136.6, 137.9, 136.3] ms; wnjr min 342.1 median 343.8 [372.3, 350.8, 342.4, 343.8, 342.1] ms
  - mergesort: native min 45.5 median 45.7 [46.1, 45.8, 45.7, 45.5, 45.6] ms; wnjr min 174.7 median 175.4 [175.4, 174.7, 175.8, 175.0, 179.0] ms
  - nqueens: native min 109.1 median 110.7 [109.8, 111.3, 111.4, 110.7, 109.1] ms; wnjr min 194.8 median 195.2 [200.4, 195.8, 194.8, 195.2, 195.2] ms

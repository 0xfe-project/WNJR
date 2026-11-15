# WASI Benchmark Results

WNJR 0.7.0 (`d3e4edec`), measured 2026-11-12 (UTC) on an Apple M5 MacBook Air.
This is the record of that release, kept as it was measured; the tree has been
rewritten since (no interpreter, renamed) and nothing here has been re-run
against it. The July 2026 Apple M4 record this file used to hold is in its git
history, and so is the x86_64 half of this one — measured on a machine we do not
run, so it is not repeated here. Do not read any of them against each other:
the machines, every competitor version and two of the benchmark programs
changed.

| Runtime | Version | Class |
|---|---|---|
| WNJR (JIT) | 0.7.0, `d3e4edec`, `--release` | compiled |
| WNJR (interpreter) | same binary, `--interp` | interpreted |
| Cranelift / wasmtime | 50.0.1, official release binary | compiled (optimizing) |
| V8 / Node.js | Node 26.13.1, V8 14.6.202.34 | compiled (TurboFan) |
| Winch / wasmtime | 50.0.1, same binary, `-C compiler=winch` | compiled (baseline) |
| wasm3 | 0.9.0 at `0cd38327`, clang, tail-call build | interpreted |
| wasmi | 2.0.0 | interpreted |

| | Apple M5 |
|---|---|
| Machine | MacBook Air, 4 P + 6 E cores, 24 GB, no fan |
| OS | macOS 27.0.1 |
| rustc | 1.99.0 |
| wasm3 compiler | Apple clang 21.0.0 |
| Power and placement | mains power, Low Power Mode off, unpinned |
| Rounds | 2: forward order, then reverse |
| Gap between sweeps | 75 s idle |

Winch is wasmtime's **baseline** compiler: single-pass, built to compile fast
rather than to run fast. It shares the compiled chart with the optimizing tier
because it is a compiler, but it is not trying to win that comparison.

Charts: [README.md](README.md)

## How to read these numbers

**Every metric is a rate, and higher is always better.** Each benchmark
self-times to a wall-clock target (2 s) and reports work per second, so a
sweep of the suite costs about the same on any engine. CoreMark runs in the
harness's `--target-seconds` mode in every cell; that is CoreMark's explicit
non-standard regression mode, not an official CoreMark result.

**Every cell is the mean of interleaved rounds.** One sweep is one engine
running the whole suite, about 30 to 50 s on one core. The engines take turns,
and the next round runs them in the opposite order, so no engine always runs
first or on a warmer machine.

**Apple M5.** The machine has no fan, so each sweep is followed by 75 s idle.
After the two rounds a control sweep repeated the JIT and Cranelift: the JIT
control read 0.94 to 1.02 of its own mean per metric (geomean 0.99) and the
Cranelift control 1.00 to 1.10 (geomean 1.03), so the machine did not drift
downwards over the 28 minutes. The largest ratio between an engine's two
rounds on any metric was 1.03 for the JIT, 1.10 for Cranelift, 1.04 for V8,
1.08 for Winch, 1.06 for the interpreter, 1.06 for wasmi and 1.11 for wasm3.
Read differences inside those bands as ties.


## Apple M5

### Integer / Control Flow

<!-- chart file="benchmark_integer.svg" title="Integer / Control Flow" machine="Apple M5" -->
| Benchmark | WNJR (JIT) | Cranelift | V8 | Winch | WNJR (interp) | wasm3 | wasmi |
|---|---:|---:|---:|---:|---:|---:|---:|
| CoreMark (score) | **52,125** | 51,455 | 50,109 | 22,332 | **10,021** | 5,209 | 6,502 |
| SHA-256 (MB/s) | **246.9** | 239.9 | 206.9 | 127.2 | **45.88** | 28.48 | 33.77 |
| bzip2 (MB/s) | 31.76 | 30.11 | **33.07** | 16.00 | **5.57** | 3.17 | 4.80 |
| LZ4 compress (MB/s) | 891.9 | 936.0 | **962.7** | 682.6 | **369.7** | 204.0 | 236.5 |
| LZ4 decompress (MB/s) | **3,343** | 3,275 | 3,148 | 1,555 | **648.6** | 405.3 | 490.9 |
| sqlite (iteration/s) | 173,672 | 164,969 | **176,639** | 66,876 | **16,522** | 7,272 | 12,241 |
<!-- endchart -->

### Lua

<!-- chart file="benchmark_lua.svg" title="Lua Benchmarks" machine="Apple M5" -->
| Benchmark | WNJR (JIT) | Cranelift | V8 | Winch | WNJR (interp) | wasm3 | wasmi |
|---|---:|---:|---:|---:|---:|---:|---:|
| lua / fib (fib20/s) | 3,102 | **3,347** | 2,074 | 1,541 | **417.5** | 232.3 | 320.2 |
| lua / sunfish (score) | 16,328 | 15,374 | **16,916** | 6,125 | **1,750** | 965.5 | 1,348 |
| lua / json (score) | 35,109 | 31,552 | **35,618** | 14,056 | **3,514** | 1,901 | 2,617 |
<!-- endchart -->

### Floating Point

<!-- chart file="benchmark_fp.svg" title="Floating-Point Benchmarks" machine="Apple M5" -->
| Benchmark | WNJR (JIT) | Cranelift | V8 | Winch | WNJR (interp) | wasm3 | wasmi |
|---|---:|---:|---:|---:|---:|---:|---:|
| mandelbrot (Kpixel/s) | **563.2** | 563.1 | 328.8 | 194.6 | **176.6** | 127.0 | 170.3 |
| c-ray (Kpixel/s) | 11,182 | 11,483 | **12,210** | 3,194 | **1,457** | 811.7 | 973.6 |
<!-- endchart -->

### Memory Bound (STREAM)

<!-- chart file="benchmark_memory.svg" title="Memory-Bound Benchmarks (STREAM)" machine="Apple M5" note="STREAM Copy=lowered to memory.copy — measures the engine's bulk copy, not dispatch" -->
| Benchmark | WNJR (JIT) | Cranelift | V8 | Winch | WNJR (interp) | wasm3 | wasmi |
|---|---:|---:|---:|---:|---:|---:|---:|
| STREAM Copy (MB/s)¹ | **165,912** | 156,219 | 165,176 | 165,777 | 124,644 | 164,821 | **165,092** |
| STREAM Scale (MB/s) | 63,772 | 59,544 | **89,186** | 31,306 | **11,000** | 6,891 | 9,488 |
| STREAM Add (MB/s) | 90,763 | 67,453 | **97,113** | 29,839 | **12,076** | 6,767 | 9,048 |
| STREAM Triad (MB/s) | 78,466 | 61,653 | **88,866** | 29,765 | **10,898** | 6,123 | 8,166 |
<!-- endchart -->

### Call Overhead

<!-- chart file="benchmark_calls.svg" title="Call Overhead (funcref)" machine="Apple M5" -->
| Benchmark | WNJR (JIT) | Cranelift | V8 | Winch | WNJR (interp) | wasm3 | wasmi |
|---|---:|---:|---:|---:|---:|---:|---:|
| funcref direct (M calls/s) | **4,350** | 865.3 | 4,280 | 287.4 | **378.2** | 172.1 | 246.9 |
| funcref exported-table (M calls/s) | 1,197 | 885.7 | **4,294** | 388.3 | **386.7** | 163.5 | 139.6 |
<!-- endchart -->

¹ The current clang lowers STREAM's copy loop to a bulk `memory.copy`, so this
row measures each engine's bulk-copy path, not its dispatch. That is why very
different compilers land within a few percent of each other here. On the M5
the WNJR interpreter is the exception: its copy handler reads 124,644
MB/s where every other engine reads 156,000 to 166,000, in both rounds. This is
a gap in the interpreter's ARM64 bulk copy on this core, not noise.

## Summary

**Against the other optimizing compilers, the JIT is level on this suite: a
little ahead of Cranelift, and level with V8, which takes the most rows.**

| | Apple M5 |
|---|---|
| Best-of-four over the 17 metrics | V8 10, WNJR 6, Cranelift 1, Winch 0 |
| WNJR / Cranelift, geomean of all 17 | 1.18 |
| WNJR / Cranelift, geomean of the 14 application metrics | 1.06 |
| WNJR / V8, geomean of all 17 | 0.95 |
| WNJR / V8, geomean of the 14 application metrics | 1.03 |
| Metrics where WNJR is ahead of Cranelift | 14 of 17 |
| Metrics where WNJR is ahead of V8 | 7 of 17 |

The 14 application metrics leave out the two funcref rows, which are call
micro-benchmarks with ratios from 0.2 to 5.0, and STREAM Copy, which is the
host's `memcpy`.

- **CoreMark.** WNJR 52,125, Cranelift 51,455, V8 50,109, a 1 to 4% lead that
  is inside the round-to-round band.
- **Ahead of V8:** mandelbrot (1.71×), Lua fib (1.50×), LZ4 decompress (1.06×),
  and SHA-256 (1.19×).
- **Behind V8:** the exported-table funcref call, where V8 is 3.6× ahead; the
  STREAM arithmetic kernels (Scale 0.72×, Add and Triad 0.82 to 0.93×); c-ray
  (0.92×); and LZ4 compress (0.93×).
- **Ahead of Cranelift:** the direct funcref call (5.0×); the Lua json
  benchmark (1.11×); STREAM Add and Triad (1.35×, 1.27×).
- **Behind Cranelift:** LZ4 compress (0.95×) and c-ray (0.97×); Lua fib (0.93×).

The picture differs on the wasmi-benchmarks corpus, where the same release
leads both compilers on x64 Linux, x64 Windows and ARM64 macOS; see
[the standings there](../wasmi_benchmarks/README.md).

**Winch sits about halfway between the two classes**, which is what a baseline
compiler is for: a median 0.40× of WNJR's JIT and 0.45× of Cranelift, and a
median 2.5× and 2.7× of WNJR's interpreter.

**Against the other interpreters, WNJR's interpreter is the fastest on every
dispatch-sensitive benchmark**: 16 of the 17 metrics, the exception being the
`memory.copy` row above.

| vs | worst | best | median |
|---|---:|---:|---:|
| wasm3 | 1.39× (mandelbrot) | 2.36× (funcref exported-table) | 1.80× |
| wasmi 2.0 | 1.04× (mandelbrot) | 2.77× (funcref exported-table) | 1.34× |

These ranges leave out STREAM Copy. wasmi 2.0 is the closer rival on every row
but the exported-table call, so the margin over whichever of the two is faster
is wasmi's row in practice. CoreMark reads 1.54× wasmi and 1.92× wasm3. The JIT
is a median 6.5× of the interpreter.

## Caveats worth knowing

- **The funcref rows and the sqlite row are new since the July record.** The
  suite gained `funcref.wasm`, two call micro-benchmarks, and replaced
  speedtest1 with `sqlite_bench.wasm`: a fixed in-memory database and a
  repeatable select/update/restore work unit, reported as `iteration/s`.
- **The Lua benchmarks may run on a 1-second clock under wasmtime.** wasmtime
  47 returned `0.0` from Lua's `os.clock()` (no process-CPU clock), so the
  scripts fall back to `os.time()`; whether 50.0.1 still does was not checked.
  `bench.lua` calibrates an approximately 10 ms work chunk, then uses a
  separate tick-aligned window and checks the clock after each chunk, so the
  endpoint overshoot is about one chunk rather than one second either way.
- **The M5 is a fanless laptop and ran unpinned.** macOS has no core pinning,
  and the cooling gaps stand in for a fan; the control sweeps above are the
  evidence that this was enough. Absolute scores on it will not match a
  machine with active cooling under sustained load.
- **V8 runs through Node's WASI** (`run_v8.mjs`), the other engines through
  their own command lines, so process startup is not comparable across them;
  the self-timed window excludes it.
- wasmi needs a `--` separator before guest arguments (`--cli-args "--dir . --"`)
  or its argument parser consumes flags like `--memdb` itself.


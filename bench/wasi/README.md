# WASI Benchmarks

Seven WebAssembly runtimes, measured 2026-11-12 (UTC) on an Apple M5 MacBook
Air: WNJR 0.7.0's JIT and interpreter,
Wasmtime Cranelift 50.0.1, V8 TurboFan (Node.js 26.13.1), Wasmtime Winch
50.0.1, wasm3 0.9.0, and wasmi 2.0.0.

Every metric is a **rate — higher is better**. The benchmarks self-time to a
wall-clock target (2 s by default) and report work per second, so a regression
suite costs about the same on any engine and the numbers stay comparable across
a 20× spread in speed. The harness invokes CoreMark's explicit non-standard
regression mode for this bounded run, in every cell. CoreMark's source and bare
command line remain compatible with upstream EEMBC CoreMark: without the
option, its own calibration runs the measured interval for at least 10 seconds
and emits the official result format.

Each cell is the mean of two interleaved rounds on the M5, with 75 s of cooling
after each engine's sweep because that machine has no fan. This is the 0.7.0
record as it was measured; the tree has been rewritten since and nothing here
has been re-run against it.

The compiled and interpreted engines get **separate charts**: a compiler is ~6×
an interpreter here, so putting both on one scale would crush the interpreter
bars into slivers and hide the comparison that matters for each class. Winch is
wasmtime's baseline (single-pass) compiler, so it rides the compiled chart but
lands about halfway to the interpreters — that gap *is* the optimizing tier.

**[Full tables, method, and caveats → RESULTS.md](RESULTS.md)**

## Apple M5

### Integer / Control Flow

![integer — compiled](../charts/benchmark_integer.svg)

![integer — interpreters](../charts/benchmark_integer_interp.svg)

### Lua Interpreter

![lua — compiled](../charts/benchmark_lua.svg)

![lua — interpreters](../charts/benchmark_lua_interp.svg)

### Floating Point

![fp — compiled](../charts/benchmark_fp.svg)

![fp — interpreters](../charts/benchmark_fp_interp.svg)

### Memory Bound (STREAM)

![memory — compiled](../charts/benchmark_memory.svg)

![memory — interpreters](../charts/benchmark_memory_interp.svg)

### Call Overhead

![calls — compiled](../charts/benchmark_calls.svg)

![calls — interpreters](../charts/benchmark_calls_interp.svg)

STREAM Copy is the one row that does not measure the engine's generated code:
the current clang lowers the copy loop to a bulk `memory.copy`, so every
runtime ends up in its bulk-copy path. It is kept because it is a fair
comparison of that path, and on the M5 it shows the WNJR interpreter's
copy handler a quarter behind the other engines — see the footnote in
[RESULTS.md](RESULTS.md#apple-m5).

The funcref rows are call micro-benchmarks: a direct call through a function
reference, and the same call through an exported table.

## Where WNJR stands

Against the other **optimizing compilers** the JIT is level on this suite. It
is a little ahead of Cranelift 50: a geomean 1.06× over the 14 application
metrics, ahead on 14 of all 17. It is level with V8 14.6 at 1.03×, and V8 takes
the most rows: best-of-four over the 17 metrics is V8 10, WNJR 6, Cranelift 1,
Winch 0. CoreMark reads WNJR 52,125, Cranelift 51,455, V8 50,109. It beats V8
clearly on mandelbrot (1.71×) and Lua fib (1.5×), and trails it clearly on the
exported-table call (V8 is 3.6× ahead) and the STREAM arithmetic kernels.

On the wasmi-benchmarks corpus the same release leads both compilers on three
platforms; [those standings](../wasmi_benchmarks/README.md) are the other half
of the picture.

**Winch** takes no row but the `memcpy` tie, as expected of a baseline
compiler: a median 0.40× of WNJR's JIT, and a median 2.5× and 2.7× of WNJR's
interpreter.

Its **interpreter** is the fastest on every dispatch-sensitive benchmark: 16 of
17 metrics, where it loses only the `memory.copy` row. Against wasmi 2.0, the
closer rival on nearly every row, the margin is 1.04–2.77× (median 1.34×);
against wasm3 the median is 1.80×. CoreMark reads 1.54× wasmi and 1.92× wasm3.

## Running them

```sh
python3 run_tests.py                 # JIT; 2s self-calibrated tests
python3 run_tests.py --interp        # this repo, interpreter
python3 run_tests.py --time 10       # 10s per benchmark
python3 run_tests.py --exec "<path>/wasmtime run" --cli-args "--dir ."
python3 run_tests.py --exec "<path>/wasmtime run" --cli-args "-C compiler=winch --dir ."
python3 run_tests.py --exec "<path>/wasm3"
python3 run_tests.py --exec "$HOME/.cargo/bin/wasmi" --cli-args "--dir . --"   # 2.0: binary is `wasmi`
node run_v8.mjs --time 2             # V8 via Node's WASI
make                                 # rebuild every .wasm from source
python3 gen_svg.py                   # redraw the charts from RESULTS.md
```

`run_tests.py` passes `--target-seconds=<seconds>` to CoreMark. This keeps CI
and local regression runs bounded, separates calibration from the reported
sample, and labels the result as non-standard. Invoke `coremark.wasm` without
that option when producing an official CoreMark result; the default upstream
10-second-minimum behavior is unchanged.

`gen_svg.py` reads its numbers out of the RESULTS.md tables, so the charts and
the tables cannot disagree — update the table, re-run it, done. It writes into
[`bench/charts/`](../charts), the one place this project keeps generated
images; the pages here and the top-level README both link into it.

Eleven files come out of a run: the ten above — five benchmark groups, each
split by engine class — plus [`coremark.svg`](../charts/coremark.svg), the
summary the top-level README leads with. That one compares the three
interpreters on a single shared scale and orders them fastest-first; the
CoreMark row in the larger interpreter charts uses the same ordering, and their
other rows retain their stable runtime order.


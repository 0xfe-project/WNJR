# wasi-threads performance

Three parallel workloads, the same Rust source on every host: Rust native,
Node.js with `@emnapi/wasi-threads`, Bun driving that same Node runner, and Bun
driving our own `wnjr-cli`.

Everything here was measured on one machine, sequentially, with nothing else
running.

| | |
|---|---|
| CPU | Apple M5, 10 cores, 32 GB (arm64 macOS) |
| rustc | 1.102.0-nightly (5f4effd57 2026-11-13) |
| node | v26.13.1 |
| bun | 1.4.4 |
| WNJR | `ff1418c2` (c2d: shared code, no per-thread JIT) |
| @emnapi/wasi-threads | ^1.1.0 |

## Workloads

| benchmark | n | work |
|---|---|---|
| `mergesort` | 4,000,000 | sort `u64` chunks in parallel, then merge pairwise down to one array |
| `matmul` | 768 | `C = A x B`, 768x768 f64, each thread takes a row block of C |
| `pi` | 40,000,000 | Monte Carlo hit test, samples split across threads |

Every benchmark runs with 4 threads: the main thread takes a share itself and
spawns the other three. Each one is run twice in a single process - a cold pass
that pays for JIT compilation, then a warm pass that is timed - because WNJR
compiles on first call and a cold number measures the compiler, not the code.
`work_ms` below is the warm pass.

Each benchmark prints a checksum of its entire result, and the checksum has to
agree across hosts before any timing is worth reading:

    mergesort  e4e5bee23c85a02d
    matmul     0858eaa69051276e
    pi         0000000001df5943

All four hosts produce exactly these three values.

## Building

Native:

    cargo build --release
    ./target/release/wt-perf mergesort 4000000 4

Wasm, with the flags this task asked for:

    rustup target add wasm32-wasip1-threads   # nightly, once
    cargo +nightly build --release --target wasm32-wasip1-threads \
        -Z build-std=std,panic_abort -Z build-std-features=optimize_for_size

`.cargo/config.toml` adds `--export=malloc --export=free`. `@emnapi/wasi-threads`
speaks the single-argument `wasi.thread-spawn` ABI and allocates an 8-byte
tid/error cell inside the guest with the guest's own allocator; Rust links
dlmalloc into a wasip1-threads module already but does not export it. Our own
runtime does not need the export - it is there for the Node path.

## Hosts

| # | host | driver | wasm |
|---|---|---|---|
| 1 | Rust native | `./target/release/wt-perf` | none, straight to the CPU |
| 2 | Node.js | `runners/node-wasi-threads.mjs` | `target/wasm32-wasip1-threads/release/wt-perf.wasm` |
| 3 | Bun | `bun runners/node-wasi-threads.mjs` | the same wasm, the same runner |
| 4 | Bun -> `wnjr-cli` | `runners/bun-wnjr.mjs` | the same wasm, run by our runtime |

Reproduce a whole column with:

    runners/run-native-matrix.sh /tmp/native.log 3
    runners/run-node-matrix.sh   /tmp/node.log   3
    runners/run-bun-matrix.sh    /tmp/bun.log    3
    runners/run-wnjr-matrix.sh   /tmp/wnjr.log   3

## Results

`work_ms`, median of 3 runs, 4 threads. Lower is better. Measured on ARM64 M5 macOS, WNJR HEAD `ff1418c2` (c2d: shared code across instances), all reps sequential.

| benchmark | native | node+emnapi | bun+emnapi | wnjr-cli |
|---|---|---|---|---|
| mergesort n=4e6 | **117.1 ms** | 174.3 ms | 169.5 ms | 139.2 ms |
| matmul n=768 | **15.7 ms** | 31.0 ms | 32.7 ms | 31.6 ms |
| pi n=4e7 | **7.8 ms** | 16.1 ms | 11.6 ms | 18.4 ms |

Relative to native:

| benchmark | node | bun | wnjr-cli |
|---|---|---|---|
| mergesort | 1.49× | 1.45× | 1.19× |
| matmul | 1.97× | 2.08× | 2.01× |
| pi | 2.07× | 1.49× | 2.37× |

Raw runs (3 reps, work_ms):

    native    mergesort 117.121 / 116.988 / 117.258
              matmul    15.890 / 15.744 / 15.717
              pi        7.662 / 7.780 / 8.493

    node      mergesort 174.258 / 171.471 / 178.288
              matmul    30.719 / 30.977 / 33.897
              pi        21.164 / 16.085 / 15.919

    bun       mergesort 222.161 / 169.225 / 169.523
              matmul    50.535 / 32.706 / 32.397
              pi        11.874 / 10.970 / 11.572

    wnjr-cli  mergesort 138.951 / 139.206 / 140.252
              matmul    31.462 / 31.645 / 31.795
              pi        18.598 / 18.410 / 18.261

wnjr-cli run straight from the shell (`./target/release/wnjr-cli ...`) or driven by bun (`runners/bun-wnjr.mjs`) gives identical numbers: bun adds no measurable overhead.

## Notes on the host setup

- **Node needs a preloaded worker pool.** `@emnapi/wasi-threads` takes a worker
  from a pool (`reuseWorker: { size: 8 }`) that is filled before the module
  starts (`preloadWorkers()`). A worker created on demand, while the main thread
  is already inside wasm, never boots: the guest's `pthread_create` blocks the
  only thread that could finish starting it. The pool is created and preloaded
  before `_start`, so its cost is setup, not benchmark time.
- **Node delivers the message payload, not a MessageEvent.** The worker side has
  to wrap it as `{ data }` before handing it to `ThreadMessageHandler`.
- **Bun's `node:wasi` returns nothing from `start()`.** Node returns the exit
  code. The runner treats a missing return as 0 and trusts the guest's own
  `RESULT` line and checksum instead.
- **The guest exports `malloc`/`free` only for the Node path.** WNJR allocates
  the thread cell itself.

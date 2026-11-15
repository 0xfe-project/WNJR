// Runs a wasm32-wasip1-threads module on Node.js.
//
// node:wasi has no wasi.thread-spawn, so @emnapi/wasi-threads supplies it on
// top of worker_threads: a worker is spawned per wasi thread, loads the same
// module, binds the same shared memory, and enters wasi_thread_start.
//
// Usage: node node-wasi-threads.mjs <module.wasm> [args...]

import { readFile } from "node:fs/promises";
import { WASI } from "node:wasi";
import { Worker } from "node:worker_threads";
import { WASIThreads } from "@emnapi/wasi-threads";
import { importedMemoryType } from "./wasm-memory-type.mjs";

const wasmPath = process.argv[2];
const args = process.argv.slice(3);
if (!wasmPath) {
  console.error("usage: node node-wasi-threads.mjs <module.wasm> [args...]");
  process.exit(2);
}

const bytes = await readFile(wasmPath);

const tCompile = performance.now();
const wasmModule = await WebAssembly.compile(bytes);
const compileMs = performance.now() - tCompile;

// Rust's wasm32-wasip1-threads std imports its memory through env.memory and
// declares it shared, so the host has to create it before instantiation.
const memType = importedMemoryType(bytes);
if (!memType) throw new Error("module does not import a memory (wasi-threads needs env.memory)");
const memory = new WebAssembly.Memory({
  initial: memType.initial,
  maximum: memType.maximum,
  shared: memType.shared,
});

const wasi = new WASI({
  version: "preview1",
  args: [wasmPath, ...args],
  env: {},
  returnOnExit: true,
});

const wasiThreads = new WASIThreads({
  wasi,
  // A worker created while the main thread sits inside wasm never boots: the
  // guest's pthread_create blocks the thread that would finish starting it. So
  // the workers are created and preloaded before the module starts, and a
  // spawn just hands work to an idle one.
  reuseWorker: { size: 8 },
  // Block the spawning thread until the worker has actually started, so a
  // guest that measures its own spawn cost measures a real thread.
  waitThreadStart: 10000,
  onCreateWorker: () => {
    console.error("[node] creating worker");
    const worker = new Worker(new URL("./node-wasi-threads-child.mjs", import.meta.url), {
      workerData: { wasmPath, args },
    });
    // A worker that dies before it reports "loaded" would otherwise only show
    // up as the library's spawn timeout.
    worker.on("error", (e) => console.error("[node] worker error:", e && (e.stack || e.message)));
    return worker;
  },
});

const tStart = performance.now();
const instance = await WebAssembly.instantiate(wasmModule, {
  env: { memory },
  wasi_snapshot_preview1: wasi.wasiImport,
  ...wasiThreads.getImportObject(),
});

wasiThreads.setup(instance, wasmModule, memory);
const tPreload = performance.now();
await wasiThreads.preloadWorkers();
const preloadMs = performance.now() - tPreload;

let exitCode = 0;
try {
  const started = wasiThreads.start(instance, wasmModule, memory);
  // node:wasi's start() returns the exit code; bun's returns nothing.
  exitCode =
    typeof started === "number"
      ? started
      : started && typeof started.exitCode === "number"
        ? started.exitCode
        : 0;
} catch (e) {
  console.error("trap:", e && e.message);
  exitCode = 134;
}
const runMs = performance.now() - tStart;
console.error(
  `[node] compile_ms=${compileMs.toFixed(1)} preload_ms=${preloadMs.toFixed(1)} run_ms=${runMs.toFixed(1)} exit=${exitCode}`,
);
wasiThreads.terminateAllThreads();
process.exit(typeof exitCode === "number" ? exitCode : 0);

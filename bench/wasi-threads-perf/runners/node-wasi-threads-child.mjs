// The worker side of node-wasi-threads.mjs: one worker per wasi thread.
//
// The parent blocks its own thread while a thread starts (that is what
// wasi.thread-spawn does), so anything this file logs with console.* can only
// reach the terminal once the parent is unblocked - or never, if it died. Logs
// go straight to a file instead when WT_DEBUG_FILE is set.

import { appendFileSync, readFileSync } from "node:fs";
import { WASI } from "node:wasi";
import { parentPort, workerData } from "node:worker_threads";
import { ThreadMessageHandler, WASIThreads } from "@emnapi/wasi-threads";

const logFile = process.env.WT_DEBUG_FILE;
const t0 = Date.now();
const log = (msg) => {
  if (!logFile) return;
  try {
    appendFileSync(logFile, "[child " + process.pid + " +" + (Date.now() - t0) + "ms] " + msg + "\n");
  } catch {}
};

log("boot");
process.on("uncaughtException", (e) => log("uncaught: " + (e && (e.stack || e.message))));
process.on("unhandledRejection", (e) => log("rejection: " + (e && (e.stack || e.message))));

const bytes = readFileSync(workerData.wasmPath);
const compiled = new WebAssembly.Module(bytes);
log("module compiled");

const postMessage = (msg) => parentPort.postMessage(msg);
const handler = new ThreadMessageHandler({
  postMessage,
  async onLoad({ wasmModule, wasmMemory }) {
    log("onLoad module=" + !!wasmModule + " memory=" + !!wasmMemory);
    const wasi = new WASI({ version: "preview1", args: workerData.args, env: {} });
    const wasiThreads = new WASIThreads({
      wasi,
      childThread: true,
      postMessage: (msg) => parentPort.postMessage(msg),
    });
    const mod = wasmModule ?? compiled;
    const originalInstance = await WebAssembly.instantiate(mod, {
      env: { memory: wasmMemory },
      wasi_snapshot_preview1: wasi.wasiImport,
      ...wasiThreads.getImportObject(),
    });
    log("instantiated");
    // in a child thread it must be initialize, never start
    const instance = wasiThreads.initialize(originalInstance, mod, wasmMemory);
    log("initialized");
    return { module: mod, instance };
  },
});

// node delivers the message payload itself; the handler expects a MessageEvent
parentPort.on("message", (data) => {
  const e = { data };
  log("message type=" + (data && data.__emnapi__ && data.__emnapi__.type));
  try {
    handler.handle(e);
  } catch (err) {
    log("handle failed: " + (err && (err.stack || err.message)));
  }
});

// Runs a wasm32-wasip1-threads module on our own runtime (wnjr-cli), from bun.
//
// wnjr-cli implements wasi.thread-spawn itself, so there is no worker helper and
// no @emnapi/wasi-threads here: bun forks one wnjr-cli process and the runtime
// does the rest. A trap in the guest shows up as a non-zero exit, and it is
// reported as such rather than folded into the timing table.
//
// Usage: bun bun-wnjr.mjs <wnjr-cli> <module.wasm> [args...]

const [cliPath, wasmPath, ...args] = process.argv.slice(2);
if (!cliPath || !wasmPath) {
  console.error("usage: bun bun-wnjr.mjs <wnjr-cli> <module.wasm> [args...]");
  process.exit(2);
}

const t0 = performance.now();
const child = Bun.spawn([cliPath, wasmPath, ...args], {
  stdout: "pipe",
  stderr: "pipe",
});
const [stdout, stderr, exitCode] = await Promise.all([
  new Response(child.stdout).text(),
  new Response(child.stderr).text(),
  child.exited,
]);
const wallMs = performance.now() - t0;

for (const line of stdout.split("\n")) {
  if (line.trim() !== "") console.log(line);
}
if (stderr.trim() !== "") console.error(stderr.trim());

const result = stdout.split("\n").find((line) => line.startsWith("RESULT "));
console.error(
  `[bun-wnjr] wall_ms=${wallMs.toFixed(1)} exit=${exitCode} result=${result ? "yes" : "no"}`,
);
process.exit(exitCode);

// Compares the same C program built two ways:
//
//   native  clang -O1 -> machine code, run directly
//   wnjr    wcc -> NJVM-IR -> WNJR's JIT, run by njvm-run
//
// and reports process wall clock for both. The checksum each program prints has
// to agree between the two columns before any time is worth reading: a faster
// wrong answer measures nothing.
//
// Every number is a whole process, start to finish, so it carries the cost of
// starting up. The startup program at the same size isolates that: its time is
// reported next to the others, and "work" is the total minus it. For the wnjr
// column that subtraction also takes off module decoding and compilation, which
// happens inside the process today.
//
// Usage: node run.mjs [--runs N] [--only name,name]
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { cpus, totalmem, platform, release, arch } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");
const work = join(here, ".work");

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
let runs = Number(opt("--runs", "5"));
const only = opt("--only", "");
const benches = (only ? only.split(",") : ["startup", "matmul", "pi", "mergesort", "nqueens"]).filter(Boolean);

mkdirSync(work, { recursive: true });

function sh(cmd, cmdArgs) {
  const r = spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8" });
  if (r.status !== 0)
    throw new Error(`${cmd} ${cmdArgs.join(" ")} failed (status ${r.status}):\n${r.stderr || r.stdout}`);
  return r;
}

function build() {
  sh("make", ["-C", join("wnjr-cc")]);
  sh("cargo", ["build", "--offline", "-q", "--manifest-path", "Cargo.toml", "-p", "wnjr-njvm-e2e", "--bin", "njvm-run"]);
}

// Returns { median, samples, out } for one column, one program.
//
// The first executions of a program and a module that were just written are not
// measurements: the system is paging them in, and a program that was just linked
// also pays for its signature being checked. Both settle after a few runs --
// matmul and pi lost a third of their time over the first four -- so three runs
// are made and thrown away before anything is counted.
//
// The number reported is the minimum, not the median. Everything that goes wrong
// in a timing run adds time and nothing subtracts it, so the fastest run is the
// one least polluted; the median and every sample are printed next to it so the
// spread stays visible.
function time(cmd, cmdArgs) {
  const samples = [];
  let out = "";
  for (let i = 0; i < 3; i++)
    spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8" });
  for (let i = 0; i < runs; i++) {
    const t0 = performance.now();
    const r = spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8" });
    const dt = performance.now() - t0;
    if (r.status !== 0)
      throw new Error(`${cmd} ${cmdArgs.join(" ")} on run ${i + 1} exited ${r.status}:\n${r.stderr || ""}`);
    out = r.stdout;
    samples.push(dt);
  }
  const sorted = [...samples].sort((a, b) => a - b);
  const median = sorted.length % 2 ? sorted[(sorted.length - 1) / 2] : (sorted[sorted.length / 2 - 1] + sorted[sorted.length / 2]) / 2;
  return { min: sorted[0], median, samples, out };
}

function checksum(out, what) {
  const m = out.match(/checksum (-?\d+)/);
  if (!m)
    throw new Error(`${what} printed no checksum, it printed: ${JSON.stringify(out.slice(0, 200))}`);
  return m[1];
}

const wcc = join(root, "wnjr-cc", "wcc");
const njvmRun = join(root, "target", "debug", "njvm-run");
const cc = ["clang", "-O1"];

build();

// wcc writes one module per translation unit and links nothing yet, so a program
// that prints is the benchmark source plus the whole libc. That makes the wcc
// compile time below the cost of the entire libc as well, and it is why the
// compile column is labelled the way it is.
const units = sh("bash", ["-c", `. ${join("wnjr-cc", "test", "libc-units.sh")}; libc_units ${join("wnjr-cc-libc")}; echo "$units"`])
  .stdout.trim().split(/\s+/).filter(Boolean);

const rows = [];
for (const name of benches) {
  const src = join(here, `${name}.c`);
  if (!existsSync(src))
    throw new Error(`no such benchmark: ${name}`);

  const nativeBin = join(work, `${name}.native`);
  const module = join(work, `${name}.mp`);

  let t0 = performance.now();
  sh(cc[0], [...cc.slice(1), "-o", nativeBin, src]);
  const nativeBuildMs = performance.now() - t0;

  t0 = performance.now();
  sh(wcc, ["-o", module, ...units, src]);
  const wnjrBuildMs = performance.now() - t0;

  const native = time(nativeBin, []);
  const wnjr = time(njvmRun, [module, "main"]);

  const nSum = checksum(native.out, `${name} native`);
  const wSum = checksum(wnjr.out, `${name} wnjr`);
  if (nSum !== wSum)
    throw new Error(`${name}: the two columns disagree, native says ${nSum} and wnjr says ${wSum}`);

  rows.push({ name, checksum: nSum, nativeMs: native.min, wnjrMs: wnjr.min, nativeMedian: native.median, wnjrMedian: wnjr.median, nativeSamples: native.samples, wnjrSamples: wnjr.samples, nativeBuildMs, wnjrBuildMs });
  console.log(`${name.padEnd(10)} checksum ${nSum.padStart(20)}  native ${native.min.toFixed(1).padStart(8)} ms   wnjr ${wnjr.min.toFixed(1).padStart(8)} ms`);
}

const base = rows.find((r) => r.name === "startup");
const work_ = (r) => ({
  native: Math.max(0, r.nativeMs - (base ? base.nativeMs : 0)),
  wnjr: Math.max(0, r.wnjrMs - (base ? base.wnjrMs : 0)),
});

const fmt = (v, d = 1) => v.toFixed(d);
const lines = [];
lines.push("# wcc's NJVM-IR on WNJR against native clang -O1");
lines.push("");
lines.push("The same C file built two ways and timed as a whole process:");
lines.push("");
lines.push("- **native** — `clang -O1` straight to machine code.");
lines.push("- **wnjr** — `wcc` to NJVM-IR, then WNJR's JIT through `njvm-run`.");
lines.push("");
lines.push("Both columns must print the same checksum. That is checked before any time is looked at, and the run stops if they disagree.");
lines.push("");
lines.push(`Machine: ${cpus()[0]?.model?.trim() ?? "unknown"}, ${arch()}, ${platform()} ${release()}, ${(totalmem() / 2 ** 30).toFixed(0)} GiB. ${runs} runs each after 3 discarded warm-up runs, ${new Date().toISOString().slice(0, 16).replace("T", " ")} UTC.`);
lines.push("");
lines.push("Times are the **fastest** of the runs. Nothing about timing is ever faster than the work, so the fastest run is the one least polluted by the machine; the median and every sample are at the bottom of this file so the spread stays visible.");
lines.push("");
lines.push("## Wall clock per process");
lines.push("");
lines.push("| benchmark | checksum | native ms | wnjr ms | wnjr / native |");
lines.push("|---|---|---:|---:|---:|");
for (const r of rows)
  lines.push(`| ${r.name} | ${r.checksum} | ${fmt(r.nativeMs)} | ${fmt(r.wnjrMs)} | ${(r.wnjrMs / r.nativeMs).toFixed(2)}× |`);
lines.push("");
if (base) {
  lines.push("The **startup** row is what a program that computes nothing costs: the price of the process, and inside the wnjr column also of decoding and compiling the module. Subtracting it leaves the work itself:");
  lines.push("");
  lines.push("| benchmark | native work ms | wnjr work ms | wnjr / native |");
  lines.push("|---|---:|---:|---:|");
  for (const r of rows) {
    if (r.name === "startup")
      continue;
    const w = work_(r);
    lines.push(`| ${r.name} | ${fmt(w.native)} | ${fmt(w.wnjr)} | ${w.native > 0 ? (w.wnjr / w.native).toFixed(2) + "×" : "—"} |`);
  }
  lines.push("");
}
lines.push("## Compiling the benchmark, for scale");
lines.push("");
lines.push("These are not comparable to each other: clang compiles one short file, wcc compiles that file plus all 275 translation units of the libc, because wcc links nothing yet.");
lines.push("");
lines.push("| benchmark | clang -O1 ms | wcc ms (including the whole libc) |");
lines.push("|---|---:|---:|");
for (const r of rows)
  lines.push(`| ${r.name} | ${fmt(r.nativeBuildMs)} | ${fmt(r.wnjrBuildMs)} |`);
lines.push("");
lines.push("## What this does and does not say");
lines.push("");
lines.push("- It says how fast the whole of wcc's output runs on WNJR next to LLVM's, which is the question being asked. It does not split that gap between the IR wcc writes and the code WNJR's JIT makes of it. Splitting it needs the JIT instrumented or its input held constant while one thing changes, not another column of numbers.");
lines.push("- The gap has one part that is not about the benchmarks at all: the wnjr column pays for decoding and compiling a module of the whole libc inside every process, which is most of the **startup** row. A real program pays it once; these pay it every run, and the work-subtracted table above is what is left after taking it off.");
lines.push("- The two columns differ in more than one thing at once: wcc has no optimizer at all, WNJR's JIT is a young backend, and the two programs do not even run the same instructions. A ratio here is a starting point, not a diagnosis.");
lines.push("- Nothing here is comparable to `bench/` at the top of the repository. Those numbers are Rust programs compiled to wasm by rustc, and they measure WNJR against other wasm runtimes. These are C programs written by a compiler with no optimization passes, measured against LLVM.");
lines.push("- Raw samples:");
lines.push("");
for (const r of rows) {
  lines.push(`  - ${r.name}: native min ${fmt(r.nativeMs)} median ${fmt(r.nativeMedian)} [${r.nativeSamples.map((v) => fmt(v)).join(", ")}] ms; wnjr min ${fmt(r.wnjrMs)} median ${fmt(r.wnjrMedian)} [${r.wnjrSamples.map((v) => fmt(v)).join(", ")}] ms`);
}
lines.push("");

writeFileSync(join(here, "RESULTS.md"), lines.join("\n"));
console.log("");
console.log("wrote " + join(here, "RESULTS.md"));

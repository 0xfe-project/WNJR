// Counts machine-IR ops inside each natural loop of a benchmark, so a change to
// wcc's codegen or to WNJR's JIT can be judged by a number instead of by feel.
//
// The input is the dump WNJR writes when SF_NATIVE_DUMP_DIR is set: one section
// per function, each with the SSA form and then the machine form that reaches the
// encoder. Only the machine form is counted; it is what becomes instructions.
//
// "Per iteration" is not a number this script can know, because a loop body with
// a branch runs different blocks on different trips. What it reports is the whole
// loop body: every block that belongs to the natural loop of a back edge, with its
// op count and how many of those ops touch memory. Both go down together when the
// code gets better, and neither can be faked by moving code across a boundary.
//
// Usage: node ir-count.mjs [--rebuild] [name ...]
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, existsSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");
const work = join(here, ".work");

const args = process.argv.slice(2);
const rebuild = args.includes("--rebuild");
const names = args.filter((a) => !a.startsWith("--"));
const benches = names.length ? names : ["pi", "matmul", "mergesort", "nqueens"];
mkdirSync(work, { recursive: true });

function sh(cmd, cmdArgs) {
  const r = spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8" });
  if (r.status !== 0)
    throw new Error(`${cmd} ${cmdArgs.join(" ")} failed (status ${r.status}):\n${r.stderr || r.stdout}`);
  return r;
}

const MEM = /^(indexed_load|indexed_store|load\.|store\.|memory\.)/;

function parse(text) {
  const fns = [];
  let fn = null;
  let inMachine = false;
  let block = null;
  const close = () => {
    if (block) fn.blocks.push(block);
    block = null;
  };
  for (const line of text.split("\n")) {
    const mFn = line.match(/^\[function (\d+)\]/);
    if (mFn) {
      close();
      fn = { index: Number(mFn[1]), blocks: [] };
      fns.push(fn);
      inMachine = false;
      continue;
    }
    if (!fn) continue;
    if (/^machine_ir:/.test(line)) { inMachine = true; continue; }
    if (/^ssa_ir:/.test(line)) { inMachine = false; close(); continue; }
    if (!inMachine) continue;
    const mBlk = line.match(/^  block b(\d+)/);
    if (mBlk) {
      close();
      block = { id: `b${mBlk[1]}`, ops: [], term: "" };
      continue;
    }
    if (!block) continue;
    const mOp = line.match(/^    \d+: (.*)$/);
    if (mOp) { block.ops.push(mOp[1]); continue; }
    const mTerm = line.match(/^    term: (.*)$/);
    if (mTerm) { block.term = mTerm[1]; close(); }
  }
  close();
  return fns.filter((f) => f.blocks.length);
}

function successors(blocks, byId) {
  const succ = new Map();
  for (const b of blocks) {
    const ids = [...b.term.matchAll(/\bb(\d+)\b/g)].map((m) => `b${m[1]}`).filter((id) => byId.has(id));
    succ.set(b.id, ids);
  }
  return succ;
}

function reaches(succ, from, to) {
  const seen = new Set();
  const stack = [from];
  while (stack.length) {
    const x = stack.pop();
    if (x === to) return true;
    if (seen.has(x)) continue;
    seen.add(x);
    for (const y of succ.get(x) ?? []) stack.push(y);
  }
  return false;
}

// A back edge u -> v (v reaches u) has the natural loop {v} plus every block that
// reaches u without going through v.
function loops(blocks, byId) {
  const succ = successors(blocks, byId);
  const found = new Map();
  for (const b of blocks) {
    for (const v of succ.get(b.id) ?? []) {
      if (!reaches(succ, v, b.id)) continue;
      const body = new Set([v]);
      const stack = [b.id];
      const seen = new Set();
      while (stack.length) {
        const x = stack.pop();
        if (x === v || seen.has(x)) continue;
        seen.add(x);
        body.add(x);
        for (const p of blocks) {
          if (body.has(p.id) || seen.has(p.id)) continue;
          if ((succ.get(p.id) ?? []).includes(x)) stack.push(p.id);
        }
      }
      const key = [...body].sort().join(",");
      if (!found.has(key)) found.set(key, body);
    }
  }
  return [...found.values()];
}

for (const name of benches) {
  const src = join(here, `${name}.c`);
  if (!existsSync(src)) throw new Error(`no such benchmark: ${name}`);
  const module = join(work, `${name}.mp`);
  if (rebuild || !existsSync(module)) {
    const units = sh("bash", ["-c", `. ${join("njvm-cc", "test", "libc-units.sh")}; libc_units ${join("njvm-cc-libc")}; echo "$units"`])
      .stdout.trim().split(/\s+/).filter(Boolean);
    sh(join(root, "njvm-cc", "wcc"), ["-o", module, ...units, join("njvm-cc", "bench", `${name}.c`)]);
  }
  const dumpDir = join(work, `dump-${name}`);
  rmSync(dumpDir, { recursive: true, force: true });
  mkdirSync(dumpDir, { recursive: true });
  const run = spawnSync(join(root, "target", "debug", "njvm-cli"), [module, "main"], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, SF_NATIVE_DUMP_DIR: dumpDir },
  });
  if (run.status !== 0) throw new Error(`njvm-cli run ${name} exited ${run.status}: ${run.stderr}`);
  const text = readFileSync(join(dumpDir, "native_index.txt"), "utf8");
  const fns = parse(text);

  console.log(`\n## ${name}`);
  for (const fn of fns) {
    const byId = new Map(fn.blocks.map((b) => [b.id, b]));
    const ls = loops(fn.blocks, byId);
    if (!ls.length) continue;
    const totalOps = fn.blocks.reduce((n, b) => n + b.ops.length, 0);
    const totalMem = fn.blocks.reduce((n, b) => n + b.ops.filter((o) => MEM.test(o)).length, 0);
    console.log(`function ${fn.index}: ${fn.blocks.length} blocks, ${totalOps} ops, ${totalMem} memory ops`);
    for (const body of [...ls].sort((a, b) => a.size - b.size)) {
      const ids = [...body].sort((a, b) => Number(a.slice(1)) - Number(b.slice(1)));
      const ops = ids.reduce((n, id) => n + byId.get(id).ops.length, 0);
      const mem = ids.reduce((n, id) => n + byId.get(id).ops.filter((o) => MEM.test(o)).length, 0);
      const detail = ids.map((id) => `${id}:${byId.get(id).ops.length}/${byId.get(id).ops.filter((o) => MEM.test(o)).length}`).join(" ");
      console.log(`  loop [${ids.join(" ")}]  ops=${ops}  mem=${mem}   (${detail})`);
    }
  }
}

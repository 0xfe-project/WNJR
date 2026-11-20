// Runs wasm modules on bun and reports results as JSON. Driven by the
// wnjr-njvm-e2e harness (src/lib.rs); not meant to be run by hand.
//
// stdin : [{ hex, export, ret: ["i32"|"i64"|"f32"|"f64"], inputs: [[arg]] }]
//         arg = { t, v } with v a decimal string (floats: raw IEEE bits).
// stdout: one JSON line per job, printed as soon as the job is done:
//         { error } | { outs: [{ trap } | { ret: [arg] }] }
// The harness kills this process when a job takes too long (an infinite loop
// cannot be interrupted from inside) and restarts it on the remaining jobs.
const jobs = JSON.parse(await Bun.stdin.text());
const dv = new DataView(new ArrayBuffer(8));

function toJs(a) {
  switch (a.t) {
    case "i32": return Number(a.v) | 0;
    case "i64": return BigInt.asIntN(64, BigInt(a.v));
    case "f32": dv.setUint32(0, Number(a.v) >>> 0); return dv.getFloat32(0);
    case "f64": dv.setBigUint64(0, BigInt(a.v)); return dv.getFloat64(0);
  }
  throw new Error("bad arg type " + a.t);
}

function fromJs(t, x) {
  switch (t) {
    case "i32": return { t, v: String(x | 0) };
    case "i64": return { t, v: String(BigInt.asIntN(64, x)) };
    case "f32": dv.setFloat32(0, x); return { t, v: String(dv.getUint32(0)) };
    case "f64": dv.setFloat64(0, x); return { t, v: String(dv.getBigUint64(0)) };
  }
  throw new Error("bad result type " + t);
}

function results(types, r) {
  const list = r === undefined ? [] : Array.isArray(r) ? r : [r];
  if (list.length !== types.length) {
    throw new Error("expected " + types.length + " results, got " + list.length);
  }
  return list.map((x, i) => fromJs(types[i], x));
}

for (const job of jobs) {
  try {
    const bytes = Uint8Array.from(job.hex.match(/../g) ?? [], (h) => parseInt(h, 16));
    const mod = new WebAssembly.Module(bytes);
    const outs = [];
    for (const input of job.inputs) {
      // A fresh instance per input, so memory and globals start identical on every host.
      const inst = new WebAssembly.Instance(mod);
      let r;
      try {
        r = inst.exports[job.export](...input.map(toJs));
      } catch (e) {
        // A wasm trap (RuntimeError) and an uncaught wasm exception count as a
        // trap; anything else (a missing export, a JS TypeError) is a harness error.
        if (e instanceof WebAssembly.RuntimeError || e instanceof WebAssembly.Exception) {
          outs.push({ trap: true });
          continue;
        }
        throw e;
      }
      outs.push({ ret: results(job.ret, r) });
    }
    process.stdout.write(JSON.stringify({ outs }) + "\n");
  } catch (e) {
    process.stdout.write(JSON.stringify({ error: String(e) }) + "\n");
  }
}

import { readFile } from "node:fs/promises";
const bytes = await readFile(process.argv[2]);
const m = new WebAssembly.Module(bytes);
console.log(JSON.stringify(WebAssembly.Module.imports(m).find((i) => i.kind === "memory"), null, 1));

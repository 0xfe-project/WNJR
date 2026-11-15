// Node's WebAssembly.Module.imports() reports kind "memory" but not its
// limits, and wasi-threads modules import their memory (env.memory) instead of
// defining it, so the host has to construct a matching WebAssembly.Memory.
// This reads the limits straight out of the import section.

export function importedMemoryType(bytes) {
  const u8 = new Uint8Array(bytes.buffer ?? bytes, bytes.byteOffset ?? 0, bytes.byteLength);
  let p = 8; // magic + version
  const u32 = () => {
    let result = 0;
    let shift = 0;
    for (;;) {
      const b = u8[p++];
      result |= (b & 0x7f) << shift;
      if ((b & 0x80) === 0) return result >>> 0;
      shift += 7;
    }
  };
  const skip = (n) => {
    p += n;
  };
  const str = () => {
    const n = u32();
    const s = new TextDecoder().decode(u8.subarray(p, p + n));
    p += n;
    return s;
  };
  while (p < u8.length) {
    const id = u8[p++];
    const size = u32();
    const end = p + size;
    if (id === 2) {
      const count = u32();
      for (let i = 0; i < count; i++) {
        const module = str();
        const name = str();
        const kind = u8[p++];
        if (kind === 0x02) {
          const flags = u8[p++];
          const initial = u32();
          const maximum = flags & 0x01 ? u32() : null;
          return {
            module,
            name,
            initial,
            maximum,
            shared: (flags & 0x02) !== 0,
            memory64: (flags & 0x04) !== 0,
          };
        }
        throw new Error("import with non-memory kind " + kind + " before the memory import");
      }
    }
    p = end;
    if (id === 0) break;
  }
  return null;
}

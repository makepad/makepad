// The V8 side of examples/wasm_diff.rs: runs every case of a batch file on
// the generated modules (scalar `run0`; four-wide `run1` on all n
// elements, as the runtime runs it, and once per element with ctx's
// element base advanced, as it runs calls whose outputs may not hold every
// record) and compares memory with the interpreter's image (all
// of it but the frame, which is scratch: guard words after it included).
// Usage: node [--no-liftoff | --liftoff-only] wasm_diff.mjs <batch file>
// Prints `PROBE fused|unfused|none`, one `FAIL <case> <mode> <what>` per
// failing run, then `DONE <cases> <runs> <fails>`.
import fs from 'node:fs';

const buf = fs.readFileSync(process.argv[2]);
const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
let at = 0;
const u32 = () => { const x = dv.getUint32(at, true); at += 4; return x; };
const bytes = (n) => { const b = buf.subarray(at, at + n); at += n; return b; };
const words = (n) => { const w = new Uint32Array(n); for (let k = 0; k < n; k++) w[k] = dv.getUint32(at + 4 * k, true); at += 4 * n; return w; };
if (u32() !== 0x31464457) throw new Error('not a wasm_diff batch');

// Is f32x4.relaxed_madd fused on this engine and machine?
let fused = false;
{
  const probe = bytes(u32());
  try {
    const i = new WebAssembly.Instance(new WebAssembly.Module(probe), { env: { memory: new WebAssembly.Memory({ initial: 1 }) } });
    const f = Math.fround;
    fused = i.exports.fma(f(1.0000001), f(1.0000001), f(-1.0000002)) !== 0;
    console.log(`PROBE ${fused ? 'fused' : 'unfused'}`);
  } catch (e) {
    console.log('PROBE none');
  }
}

const count = u32();
let runs = 0, fails = 0;
for (let c = 0; c < count; c++) {
  const name = new TextDecoder().decode(bytes(u32()));
  const [n, kbase, ctx, state, shared, table, frame] = [u32(), u32(), u32(), u32(), u32(), u32(), u32()];
  const image = words(u32());
  const [skipLo, skipHi] = [u32(), u32()];
  const expected = words(image.length);
  const nmods = u32();
  for (let m = 0; m < nmods; m++) {
    const flags = u32();
    const mod = bytes(u32());
    if ((flags & 1) && !fused) continue;
    const tag = (flags & 1) ? 'R' : '';
    let inst, mem;
    try {
      mem = new WebAssembly.Memory({ initial: Math.ceil(image.length * 4 / 65536) + 1 });
      inst = new WebAssembly.Instance(new WebAssembly.Module(mod), { env: { memory: mem } });
    } catch (e) {
      console.log(`FAIL ${c} compile${tag} ${e.message} (${name})`);
      fails++;
      continue;
    }
    const X = inst.exports;
    for (const mode of (flags & 2) ? ['scalar', 'simd', 'simd1'] : ['scalar']) {
      const I = new Uint32Array(mem.buffer);
      I.fill(0);
      I.set(image);
      runs++;
      try {
        if (mode === 'scalar') {
          X.run0(ctx, state, shared, table, n, frame);
        } else if (mode === 'simd') {
          X.run1(ctx, state, shared, table, n, frame);
        } else {
          const base = I[kbase >> 2];
          for (let e = 0; e < n; e++) {
            I[kbase >> 2] = base + e;
            X.run1(ctx, state, shared, table, 1, frame);
          }
          I[kbase >> 2] = base;
        }
      } catch (e) {
        console.log(`FAIL ${c} ${mode}${tag} trap ${e.message} (${name})`);
        fails++;
        continue;
      }
      for (let k = 0; k < expected.length; k++) {
        if (k >= skipLo && k < skipHi) continue;
        if (I[k] !== expected[k]) {
          console.log(`FAIL ${c} ${mode}${tag} word ${k} (byte ${4 * k}) interpreter ${expected[k].toString(16)} wasm ${I[k].toString(16)} (${name})`);
          fails++;
          break;
        }
      }
    }
  }
}
console.log(`DONE ${count} ${runs} ${fails}`);

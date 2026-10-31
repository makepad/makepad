// The V8 side of examples/wasm_v8.rs: renders every shader of a batch on
// its generated module, call by call (the host's ctx/state changes applied
// before each), and compares the FNV hash of both output channels with
// the interpreter's.
// Usage: node wasm_v8.mjs <batch file>
// Prints `OK <name> <ns per frame>` or `FAIL <name> <what>` per shader.
import fs from 'node:fs';

const buf = fs.readFileSync(process.argv[2]);
const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
let at = 0;
const u32 = () => { const x = dv.getUint32(at, true); at += 4; return x; };
const bytes = (n) => { const b = buf.subarray(at, at + n); at += n; return b; };
const words = () => { const n = u32(); const w = new Uint32Array(n); for (let k = 0; k < n; k++) w[k] = dv.getUint32(at + 4 * k, true); at += 4 * n; return w; };
if (u32() !== 0x31415641) throw new Error('not an audio wasm batch');
const MAX = 128;

function fnv(chans) {
  // 64-bit FNV-1a over the little-endian bytes of every word, in BigInt.
  let h = 0xcbf29ce484222325n;
  const p = 0x100000001b3n, m = (1n << 64n) - 1n;
  for (const w of chans) {
    for (let k = 0; k < w.length; k++) {
      let x = w[k];
      for (let b = 0; b < 4; b++) {
        h ^= BigInt(x & 0xff);
        h = (h * p) & m;
        x >>>= 8;
      }
    }
  }
  return h;
}

const count = u32();
for (let s = 0; s < count; s++) {
  const name = new TextDecoder().decode(bytes(u32()));
  const mod = bytes(u32());
  const frameWords = u32();
  const shared = words(), ctx0 = words(), state0 = words(), inL = words(), inR = words();
  const total = inL.length;
  // Layout (byte addresses, 16-aligned): ctx, state, shared, io, 4 channels, frame.
  let top = 64;
  const alloc = (w) => { const a = top; top += (Math.max(w, 1) * 4 + 16 + 15) & ~15; return a; };
  const ctx = alloc(ctx0.length), state = alloc(state0.length), sh = alloc(shared.length), io = alloc(4);
  const ch = [alloc(MAX), alloc(MAX), alloc(MAX), alloc(MAX)];
  const frame = alloc(frameWords);
  const ncalls = u32();
  const calls = [];
  for (let c = 0; c < ncalls; c++) {
    const f = u32(), n = u32(), np = u32();
    const patch = [];
    for (let k = 0; k < np; k++) patch.push([u32(), u32(), u32()]);
    calls.push([f, n, patch]);
  }
  const want = BigInt(u32()) | (BigInt(u32()) << 32n);
  let inst, mem;
  try {
    mem = new WebAssembly.Memory({ initial: Math.ceil(top / 65536) + 1 });
    inst = new WebAssembly.Instance(new WebAssembly.Module(mod), { env: { memory: mem } });
  } catch (e) {
    console.log(`FAIL ${name} compile: ${e.message}`);
    continue;
  }
  const run = inst.exports.run0;
  const render = () => {
    const I = new Uint32Array(mem.buffer);
    I.fill(0);
    I.set(ctx0, ctx >> 2);
    I.set(state0, state >> 2);
    I.set(shared, sh >> 2);
    for (let k = 0; k < 4; k++) I[(io >> 2) + k] = ch[k];
    const outL = new Uint32Array(total), outR = new Uint32Array(total);
    for (const [f, n, patch] of calls) {
      for (const [r, w, x] of patch) I[((r === 0 ? ctx : state) >> 2) + w] = x;
      I.set(inL.subarray(f, f + n), ch[0] >> 2);
      I.set(inR.subarray(f, f + n), ch[1] >> 2);
      I.set(outL.subarray(f, f + n), ch[2] >> 2);
      I.set(outR.subarray(f, f + n), ch[3] >> 2);
      run(ctx, state, sh, io, n, frame);
      outL.set(I.subarray(ch[2] >> 2, (ch[2] >> 2) + n), f);
      outR.set(I.subarray(ch[3] >> 2, (ch[3] >> 2) + n), f);
    }
    return [outL, outR];
  };
  try {
    const got = fnv(render());
    if (got !== want) {
      console.log(`FAIL ${name} hash ${got.toString(16)} want ${want.toString(16)}`);
      continue;
    }
    // Warm (optimized tier), then time a few renders (copies included).
    for (let k = 0; k < 3; k++) render();
    const reps = 5;
    const t0 = performance.now();
    for (let k = 0; k < reps; k++) render();
    const ns = (performance.now() - t0) * 1e6 / (reps * total);
    console.log(`OK ${name} ${ns.toFixed(1)}`);
  } catch (e) {
    console.log(`FAIL ${name} trap ${e.message}`);
  }
}

// AVA3 holds each shader's scalar wasm, memory layout, initial images and
// planar inputs once. Each slicing has its host calls and expected output words.
import { test } from "makepad-web-test";

const response = await fetch(new URL("audio.bin", import.meta.url));
if (!response.ok) throw new Error(`audio.bin: HTTP ${response.status}`);
const batch = await response.arrayBuffer();
const view = new DataView(batch);
let at = 0;
function u32() {
    const value = view.getUint32(at, true);
    at += 4;
    return value;
}
function bytes() {
    const length = u32();
    const data = new Uint8Array(batch, at, length);
    at += length;
    return data;
}
function words() {
    const data = new Uint32Array(u32());
    for (let i = 0; i < data.length; i++) data[i] = u32();
    return data;
}
function performanceCase(outs) {
    const slices = words().join(",");
    const calls = Array.from({ length: u32() }, () => {
        const frame = u32(), n = u32();
        const patch = Array.from({ length: u32() }, () => [u32(), u32(), u32()]);
        return { frame, n, patch };
    });
    return { slices, calls, expected: Array.from({ length: outs }, words) };
}

if (u32() !== 0x33415641) throw new Error("expected an AVA3 audio batch");
const count = u32(), maxFrames = u32();
if (!count) throw new Error("empty audio batch");
for (let s = 0; s < count; s++) {
    const name = new TextDecoder().decode(bytes());
    const moduleBytes = bytes();
    const frameWords = u32(), ins = u32(), outs = u32();
    const shared = words(), ctx0 = words(), state0 = words();
    const inputs = Array.from({ length: ins }, words);
    const cases = Array.from({ length: u32() }, () => performanceCase(outs));

    test(name, () => {
        // Byte offsets in the imported memory. Even one-frame calls require
        // channel buffers sized for the module's full block capacity.
        let top = 64;
        const alloc = (wordCount) => {
            const address = top;
            top += Math.ceil(Math.max(wordCount, 1) / 4) * 16;
            return address;
        };
        const ctx = alloc(ctx0.length), state = alloc(state0.length), sh = alloc(shared.length);
        const io = alloc(ins + outs);
        const channels = Array.from({ length: ins + outs }, () => alloc(maxFrames));
        const scratch = alloc(frameWords);
        const memory = new WebAssembly.Memory({ initial: Math.ceil(top / 65536) });
        const { exports: { run0 } } = new WebAssembly.Instance(new WebAssembly.Module(moduleBytes), { env: { memory } });
        const image = new Uint32Array(memory.buffer);
        image.set(shared, sh / 4);
        image.set(channels, io / 4);
        const buffers = channels.map(ptr => image.subarray(ptr / 4, ptr / 4 + maxFrames));
        for (const { slices, calls, expected } of cases) {
            image.set(ctx0, ctx / 4);
            image.set(state0, state / 4);
            image.fill(0, scratch / 4, scratch / 4 + frameWords);
            const outputs = Array.from({ length: outs }, () => new Uint32Array(expected[0].length));
            for (const { frame, n, patch } of calls) {
                for (const [region, word, value] of patch) image[(region === 0 ? ctx : state) / 4 + word] = value;
                for (let ch = 0; ch < ins; ch++) buffers[ch].set(inputs[ch].subarray(frame, frame + n));
                for (let ch = 0; ch < outs; ch++) buffers[ins + ch].set(outputs[ch].subarray(frame, frame + n));
                run0(ctx, state, sh, io, n, scratch);
                for (let ch = 0; ch < outs; ch++) outputs[ch].set(buffers[ins + ch].subarray(0, n), frame);
            }
            for (let frame = 0; frame < expected[0].length; frame++) {
                for (let ch = 0; ch < outs; ch++) {
                    const actual = outputs[ch][frame], want = expected[ch][frame];
                    if (actual !== want) {
                        const hex = word => word.toString(16).padStart(8, "0");
                        throw new Error(`${name}, slices ${slices}: wasm differs at frame ${frame}, output channel ${ch}: got ${hex(actual)}, expected ${hex(want)}`);
                    }
                }
            }
        }
    });
}
if (at !== batch.byteLength) throw new Error("unread audio batch data");

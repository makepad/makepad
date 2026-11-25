// The batch comes from tests/web_wasm.rs, with the AIR interpreter's image
// and an exact module followed by a relaxed module when the program uses FMA.
import { test } from "makepad-web-test";

const response = await fetch("/batch.bin");
if (!response.ok) throw new Error(`batch.bin: HTTP ${response.status}`);
const buffer = await response.arrayBuffer();
const view = new DataView(buffer);
let offset = 0;
function word() {
    const n = view.getUint32(offset, true);
    offset += 4;
    return n;
}
function blob() {
    const length = word();
    const bytes = new Uint8Array(buffer, offset, length);
    offset += bytes.length;
    return bytes;
}
function words(length) {
    return Uint32Array.from({ length }, word);
}
const decoder = new TextDecoder();
const string = () => decoder.decode(blob());

function probeFma(probe) {
    if (!WebAssembly.validate(probe)) return "unavailable";
    const memory = new WebAssembly.Memory({ initial: 1 });
    const { fma } = new WebAssembly.Instance(new WebAssembly.Module(probe), { env: { memory } }).exports;
    return fma(1.0000001, 1.0000001, -1.0000002) !== 0 ? "fused" : "unfused";
}
const relaxed = probeFma(blob());

function readCase() {
    const name = string();
    const n = word(), base = word(), work = word(), ctx = word(), state = word(), shared = word(), table = word(), frame = word();
    const scratchStart = word(), scratchEnd = word();
    const length = word();
    const image = words(length), expected = words(length);
    const modules = Array.from({ length: word() }, () => ({ simd: word(), bytes: blob() }));
    return { name, n, base, work, ctx, state, shared, table, frame, scratchStart, scratchEnd, image, expected, modules };
}

function compare(c, exports, image, mode) {
    image.fill(0);
    image.set(c.image);
    if (mode === "run1 per element") {
        const baseWord = c.base / 4;
        // Kernel::run_wasm sums each one-element call's K_WORK and restores K_BASE.
        const workWord = c.work / 4;
        const saved = image[baseWord];
        let sum = 0n;
        for (let i = 0; i < c.n; i++) {
            image[baseWord] = saved + i;
            image.fill(0, workWord, workWord + 2);
            exports.run1(c.ctx, c.state, c.shared, c.table, 1, c.frame);
            sum += BigInt(image[workWord]) | (BigInt(image[workWord + 1]) << 32n);
        }
        image[baseWord] = saved;
        image[workWord] = Number(sum & 0xffffffffn);
        image[workWord + 1] = Number(sum >> 32n);
    } else {
        exports[mode](c.ctx, c.state, c.shared, c.table, c.n, c.frame);
    }
    for (let k = 0; k < c.expected.length; k++) {
        if (k >= c.scratchStart && k < c.scratchEnd) continue;
        if (image[k] !== c.expected[k]) {
            throw new Error(`word ${k} (byte ${4 * k}) interpreter ${c.expected[k].toString(16)} wasm ${image[k].toString(16)}`);
        }
    }
}

function checkModule(c, module, fma, fail) {
    let exports, image;
    try {
        const memory = new WebAssembly.Memory({ initial: Math.ceil(c.image.byteLength / 65536) + 1 });
        exports = new WebAssembly.Instance(new WebAssembly.Module(module.bytes), { env: { memory } }).exports;
        image = new Uint32Array(memory.buffer);
    } catch (error) {
        fail(`${c.name} ${fma} compile: ${error.message}`);
        return 0;
    }

    const modes = module.simd ? ["run0", "run1", "run1 per element"] : ["run0"];
    for (const mode of modes) {
        try {
            compare(c, exports, image, mode);
        } catch (error) {
            fail(`${c.name} ${fma} ${mode}: ${error.message}`);
        }
    }
    return modes.length;
}

while (offset < buffer.byteLength) {
    const group = string();
    const cases = Array.from({ length: word() }, readCase);
    test(group, (t) => {
        const failures = [];
        let omitted = 0;
        function fail(message) {
            if (failures.length < 20) failures.push(message);
            else omitted++;
        }
        let runs = 0;
        for (const c of cases) {
            runs += checkModule(c, c.modules[0], "exact", fail);
            if (c.modules.length === 2 && relaxed === "fused") runs += checkModule(c, c.modules[1], "relaxed", fail);
        }
        t.diagnostic(`${cases.length} cases, ${runs} runs, relaxed_madd ${relaxed}`);
        if (omitted) failures.push(`${omitted} more failures omitted`);
        if (failures.length) throw new Error(failures.join("\n"));
    });
}

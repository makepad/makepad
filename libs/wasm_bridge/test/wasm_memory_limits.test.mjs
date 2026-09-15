import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("../src/wasm_bridge.js", import.meta.url), "utf8")
    .replace(/^export /gm, "");
const { WasmBridge } = new Function(`${source}\nreturn { WasmBridge };`)();

function encode_var_u32(value) {
    const out = [];
    do {
        let byte = value & 0x7f;
        value >>>= 7;
        if (value !== 0) {
            byte |= 0x80;
        }
        out.push(byte);
    } while (value !== 0);
    return out;
}

function name_bytes(str) {
    const encoded = Array.from(new TextEncoder().encode(str));
    return [...encode_var_u32(encoded.length), ...encoded];
}

function section(id, payload) {
    return [id, ...encode_var_u32(payload.length), ...payload];
}

function memory_limits({ min, max = null, shared = false }) {
    let flags = 0;
    if (max != null) {
        flags |= 0x01;
    }
    if (shared) {
        flags |= 0x02;
    }
    const bytes = [flags, ...encode_var_u32(min)];
    if (max != null) {
        bytes.push(...encode_var_u32(max));
    }
    return bytes;
}

function import_memory_section(limits) {
    const payload = [
        1,
        ...name_bytes("env"),
        ...name_bytes("memory"),
        0x02,
        ...memory_limits(limits),
    ];
    return section(2, payload);
}

function defined_memory_section(limits) {
    const payload = [1, ...memory_limits(limits)];
    return section(5, payload);
}

function wasm_module(...sections) {
    return Uint8Array.from([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, ...sections.flat()]);
}

test("import-memory min 64 max 16384 shared", () => {
    const bytes = wasm_module(import_memory_section({ min: 64, max: 16384, shared: true }));
    assert.deepEqual(WasmBridge.parse_wasm_memory_limits(bytes), {
        min: 64,
        max: 16384,
        shared: true,
    });
});

test("import-memory min 112 no max", () => {
    const bytes = wasm_module(import_memory_section({ min: 112 }));
    assert.deepEqual(WasmBridge.parse_wasm_memory_limits(bytes), {
        min: 112,
        max: null,
        shared: false,
    });
});

test("defined memory min 200", () => {
    const bytes = wasm_module(defined_memory_section({ min: 200 }));
    assert.deepEqual(WasmBridge.parse_wasm_memory_limits(bytes), {
        min: 200,
        max: null,
        shared: false,
    });
});

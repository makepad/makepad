import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("./web.js", import.meta.url), "utf8")
    .replace(/^import .*wasm_bridge\.js"\n/, "")
    .replace(/^export /gm, "");
const load = new Function("WasmBridge", `${source}\nreturn {
    makepad_create_breadcrumb_ring,
    makepad_create_report_gate,
    makepad_truncate_report
};`);
const {
    makepad_create_breadcrumb_ring,
    makepad_create_report_gate,
    makepad_truncate_report
} = load(class WasmBridge {});

test("breadcrumb ring retains thirty bounded entries with offsets", () => {
    const ring = makepad_create_breadcrumb_ring(30, 300, 1000);
    for (let index = 0; index < 35; index += 1) {
        ring.push("log", [String(index), "x".repeat(400)], 1000 + index, 7);
    }
    const entries = ring.snapshot();
    assert.equal(entries.length, 30);
    assert.equal(entries[0].ms, 5);
    assert.equal(entries[0].worker, 7);
    assert.equal(entries[29].text.length, 300);
});

test("report gate deduplicates kind, message, and top frame and enforces its cap", () => {
    const gate = makepad_create_report_gate(2);
    assert.equal(gate.accept("window.error", { message: "boom", stack: "Error\n at one.js:1:1" }), true);
    assert.equal(gate.accept("window.error", { message: "boom", stack: "Error\n at one.js:1:1" }), false);
    assert.equal(gate.accept("window.error", { message: "boom", stack: "Error\n at two.js:1:1" }), true);
    assert.equal(gate.accept("worker.error", { message: "different" }), false);
    assert.equal(gate.count(), 2);
});

test("GET truncation drops breadcrumbs before shortening data and stays valid JSON", () => {
    const breadcrumbs = Array.from({ length: 30 }, () => ({ text: "b".repeat(300) }));
    const without_breadcrumbs = JSON.parse(makepad_truncate_report({
        v: 1,
        kind: "window.error",
        breadcrumbs,
        data: { message: "keep me" }
    }, 1024));
    assert.equal("breadcrumbs" in without_breadcrumbs, false);
    assert.deepEqual(without_breadcrumbs.data, { message: "keep me" });

    const report = {
        v: 1,
        kind: "window.error",
        breadcrumbs,
        data: { message: "d".repeat(20000) }
    };
    const text = makepad_truncate_report(report, 8192);
    const parsed = JSON.parse(text);
    assert.ok(new TextEncoder().encode(text).byteLength <= 8192);
    assert.equal("breadcrumbs" in parsed, false);
    assert.equal(parsed.data.truncated, true);
});

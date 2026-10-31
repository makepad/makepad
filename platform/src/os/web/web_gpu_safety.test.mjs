import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// The GPU safety layer of web.js: the frame watchdog's decisions and the
// live-bytes ledger (GPU-SAFETY.md).
const web_source = readFileSync(new URL("./web.js", import.meta.url), "utf8")
  .replace(/^import .*wasm_bridge\.js"\n/, "")
  .replace(/^export /gm, "");
const web = new Function(
  "WasmBridge",
  "window",
  "document",
  "navigator",
  "screen",
  "performance",
  "console",
  `${web_source}\nreturn {
    MAKEPAD_GPU_HUNG_FRAME_MS,
    MAKEPAD_GPU_MAX_FRAMES_IN_FLIGHT,
    makepad_create_gpu_watchdog,
    makepad_gpu_watchdog_complete,
    makepad_create_gpu_ledger,
    makepad_gpu_ledger_set,
    makepad_gpu_ledger_free,
    makepad_gpu_ledger_frame,
    makepad_gpu_ledger_stats,
    makepad_install_gpu_ledger,
  };`,
)(
  class { static is_phone() { return false; } },
  { addEventListener() {}, navigator: { userAgent: "test" } },
  {},
  { userAgent: "test" },
  { width: 0, height: 0 },
  { now: () => 0 },
  { log() {}, warn() {}, error() {} },
);

test("frames of any ordinary length are not hung", () => {
  const w = web.makepad_create_gpu_watchdog();
  for (const ms of [4, 16, 33, 100, 500, web.MAKEPAD_GPU_HUNG_FRAME_MS]) {
    for (let i = 0; i < 100; i++) assert.equal(web.makepad_gpu_watchdog_complete(w, ms), false);
  }
  assert.equal(w.hung_frames, 0);
  assert.equal(w.worst_ms, web.MAKEPAD_GPU_HUNG_FRAME_MS);
});

test("a frame running past the hang bound is counted as hung", () => {
  const w = web.makepad_create_gpu_watchdog();
  assert.equal(web.makepad_gpu_watchdog_complete(w, web.MAKEPAD_GPU_HUNG_FRAME_MS + 1), true);
  assert.equal(w.hung_frames, 1);
});

test("the ledger counts live bytes, re-specification replaces, deletion frees", () => {
  const l = web.makepad_create_gpu_ledger();
  const a = {}, b = {};
  web.makepad_gpu_ledger_set(l, "texture", a, "level0", 1000);
  web.makepad_gpu_ledger_set(l, "texture", a, "level0", 4000);
  web.makepad_gpu_ledger_set(l, "buffer", b, 0, 500);
  let s = web.makepad_gpu_ledger_stats(l);
  assert.equal(s.texture_bytes, 4000);
  assert.equal(s.buffer_bytes, 500);
  assert.equal(s.live_textures, 1);
  web.makepad_gpu_ledger_free(l, a);
  web.makepad_gpu_ledger_free(l, a);
  s = web.makepad_gpu_ledger_stats(l);
  assert.equal(s.texture_bytes, 0);
  assert.equal(s.live_bytes, 500);
  assert.equal(s.peak_bytes, 4500);
});

test("a texture per frame on average over a second is reported once; steady frames never", () => {
  const big = 8 * 1024 * 1024;
  const reports = [];
  const report = (m) => reports.push(m);
  // Targets created once, then many frames that allocate nothing.
  const steady = web.makepad_create_gpu_ledger();
  for (let i = 0; i < 8; i++) web.makepad_gpu_ledger_set(steady, "texture", {}, "0", big);
  for (let frame = 0; frame < 600; frame++) web.makepad_gpu_ledger_frame(steady, frame * 16.7, 0, 1285, report);
  // An atlas that grows now and then: allocations, but not in every frame.
  const growing = web.makepad_create_gpu_ledger();
  for (let frame = 0; frame < 600; frame++) {
    if (frame % 10 === 0) web.makepad_gpu_ledger_set(growing, "texture", {}, "0", big);
    web.makepad_gpu_ledger_frame(growing, frame * 16.7, 0, 1285, report);
  }
  assert.equal(reports.length, 0);
  // Two new targets in every frame, the last frame's freed.
  const churn = web.makepad_create_gpu_ledger();
  let last = [];
  for (let frame = 0; frame < 600; frame++) {
    for (const t of last) web.makepad_gpu_ledger_free(churn, t);
    last = [{}, {}];
    for (const t of last) web.makepad_gpu_ledger_set(churn, "texture", t, "0", big);
    web.makepad_gpu_ledger_frame(churn, frame * 16.7, 0, 1285, report);
  }
  assert.equal(web.makepad_gpu_ledger_stats(churn).live_textures, 2);
  assert.equal(reports.length, 1);
  assert.match(reports[0], /textures allocated in \d+ frames within a second/);
  // One allocation every other frame (an animated atlas) is not reported.
  const half = web.makepad_create_gpu_ledger();
  const before = reports.length;
  for (let frame = 0; frame < 600; frame++) {
    if (frame % 2 === 0) web.makepad_gpu_ledger_set(half, "texture", {}, "0", big);
    web.makepad_gpu_ledger_frame(half, frame * 16.7, 0, 1285, report);
  }
  assert.equal(reports.length, before);
  // A load burst: 30 textures across a few slow frames, once, is not churn.
  const burst = web.makepad_create_gpu_ledger();
  for (let k = 0; k < 30; k++) web.makepad_gpu_ledger_set(burst, "texture", {}, "0", 1024);
  for (const at of [0, 300, 600, 900, 1200]) web.makepad_gpu_ledger_frame(burst, at, 0, 1285, report);
  for (let frame = 0; frame < 300; frame++) web.makepad_gpu_ledger_frame(burst, 1300 + frame * 16.7, 0, 1285, report);
  assert.equal(reports.length, before);
});

test("GL_OUT_OF_MEMORY is reported once with the live bytes", () => {
  const OOM = 1285;
  const reports = [];
  const l = web.makepad_create_gpu_ledger();
  web.makepad_gpu_ledger_set(l, "texture", {}, "0", 64 * 1048576);
  web.makepad_gpu_ledger_frame(l, 0, 0, OOM, (m) => reports.push(m));
  web.makepad_gpu_ledger_frame(l, 16, OOM, OOM, (m) => reports.push(m));
  web.makepad_gpu_ledger_frame(l, 32, OOM, OOM, (m) => reports.push(m));
  assert.equal(reports.length, 1);
  assert.match(reports[0], /out of memory.*64 MB live/);
});

test("the installed ledger follows a context's calls and never changes them", () => {
  const calls = [];
  let bound_texture = null;
  let bound_buffer = null;
  const gl = {
    TEXTURE_2D: 1, TEXTURE_BINDING_2D: 2, ARRAY_BUFFER: 3, ARRAY_BUFFER_BINDING: 4,
    RGBA16F: 5, RGBA: 6, UNSIGNED_BYTE: 7, HALF_FLOAT: 8, FLOAT: 9,
    getParameter(p) { return p === 2 ? bound_texture : p === 4 ? bound_buffer : null; },
    texImage2D(...a) { calls.push(["texImage2D", a.length]); return "t"; },
    texStorage2D(...a) { calls.push(["texStorage2D", a.length]); },
    deleteTexture() { calls.push(["deleteTexture"]); },
    bufferData(...a) { calls.push(["bufferData", a.length]); },
    deleteBuffer() { calls.push(["deleteBuffer"]); },
  };
  const l = web.makepad_create_gpu_ledger();
  web.makepad_install_gpu_ledger(gl, l);
  bound_texture = { name: "t1" };
  assert.equal(gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 100, 50, 0, gl.RGBA, gl.UNSIGNED_BYTE, null), "t");
  const t2 = { name: "t2" };
  bound_texture = t2;
  gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA16F, 10, 10);
  bound_buffer = { name: "b" };
  gl.bufferData(gl.ARRAY_BUFFER, 4096, 0);
  let s = web.makepad_gpu_ledger_stats(l);
  assert.equal(s.texture_bytes, 100 * 50 * 4 + 10 * 10 * 8);
  assert.equal(s.buffer_bytes, 4096);
  gl.deleteTexture(t2);
  gl.deleteBuffer(bound_buffer);
  s = web.makepad_gpu_ledger_stats(l);
  assert.equal(s.texture_bytes, 100 * 50 * 4);
  assert.equal(s.buffer_bytes, 0);
  assert.deepEqual(calls.map((c) => c[0]), ["texImage2D", "texStorage2D", "bufferData", "deleteTexture", "deleteBuffer"]);
  assert.equal(calls[0][1], 9);
});

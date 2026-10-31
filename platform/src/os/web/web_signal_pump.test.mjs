import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// One wasm pump per animation frame: signals from other threads wait for a
// requested frame and go in its batch instead of pumping on their own.

const frames = [];
const web_window = {
  devicePixelRatio: 1,
  innerWidth: 800,
  innerHeight: 600,
  navigator: { userAgent: "test", platform: "test" },
  addEventListener() {},
  requestAnimationFrame(callback) {
    frames.push(callback);
    return frames.length;
  },
  cancelAnimationFrame() {},
};
class MockWasmBridge {
  static is_phone() {
    return false;
  }
}
const source = readFileSync(new URL("./web.js", import.meta.url), "utf8")
  .replace(/^import .*wasm_bridge\.js"\n/, "")
  .replace(/^export /gm, "");
const load = new Function(
  "WasmBridge",
  "window",
  "document",
  "navigator",
  "screen",
  "performance",
  "console",
  `${source}\nreturn { WasmWebBrowser };`,
);
const { WasmWebBrowser } = load(
  MockWasmBridge,
  web_window,
  {},
  web_window.navigator,
  { width: 0, height: 0 },
  { now: () => 0 },
  { log() {}, warn() {}, error() {} },
);

// A browser with the wasm side stubbed: each pump records its batch; the
// app asks for the next frame every pump (it is animating) and, when told,
// wakes the UI during the pump the way a worker finishing a job does.
function browser() {
  const b = Object.create(WasmWebBrowser.prototype);
  b.wasm = {};
  b.webgl_context_lost = false;
  b.req_anim_frame_id = 0;
  b.signal_flags = 0;
  b.pumps = [];
  b.wake_during_pump = false;
  b.exports = {
    wasm_check_signal: () => {
      const flags = b.signal_flags;
      b.signal_flags = 0;
      return flags;
    },
  };
  b.batch = [];
  b.to_wasm = {
    ToWasmSignal: (args) => b.batch.push(["Signal", args.flags]),
    ToWasmAnimationFrame: () => b.batch.push(["AnimationFrame"]),
    ToWasmRedrawAll: () => b.batch.push(["RedrawAll"]),
  };
  b.do_wasm_pump = () => {
    b.pumps.push(b.batch);
    b.batch = [];
    // (bounded: without the deferral each wake pumps and wakes again)
    if (b.wake_during_pump && b.pumps.length < 20) {
      b.signal_flags |= 1;
      b.js_wake_ui();
    }
    b.FromWasmRequestAnimationFrame();
  };
  b.gpu_watchdog_hold = () => false;
  b.gpu_timer_poll = () => {};
  b.gpu_timer_begin = () => null;
  b.gpu_timer_end = () => {};
  b.gpu_watchdog_submit = () => {};
  b.pending_webgl_shader_count = 0;
  return b;
}

const microtasks = () => new Promise((resolve) => setTimeout(resolve, 0));

function run_frame() {
  const callback = frames.shift();
  assert.ok(callback, "an animation frame was requested");
  callback(16.0);
}

test("a signal with no frame coming pumps at once", async () => {
  frames.length = 0;
  const b = browser();
  b.signal_flags = 1;
  b.js_wake_ui();
  await microtasks();
  assert.deepEqual(b.pumps, [[["Signal", 1]]]);
});

test("a signal while a frame is requested waits for it and goes in its batch", async () => {
  frames.length = 0;
  const b = browser();
  b.FromWasmRequestAnimationFrame();
  b.signal_flags = 2;
  b.js_wake_ui();
  await microtasks();
  assert.equal(b.pumps.length, 0);
  run_frame();
  assert.deepEqual(b.pumps, [[["Signal", 2], ["AnimationFrame"]]]);
});

test("a wake during a frame's pump does not pump again in that task", async () => {
  frames.length = 0;
  const b = browser();
  b.wake_during_pump = true;
  b.FromWasmRequestAnimationFrame();
  for (let frame = 0; frame < 3; frame += 1) {
    run_frame();
    await microtasks();
    assert.equal(b.pumps.length, frame + 1, "one pump per frame");
  }
  assert.deepEqual(b.pumps[0], [["AnimationFrame"]]);
  assert.deepEqual(b.pumps[1], [["Signal", 1], ["AnimationFrame"]]);
});

test("the signal poll defers to a requested frame too", () => {
  frames.length = 0;
  const b = browser();
  let poll = null;
  const saved = web_window.setInterval;
  web_window.setInterval = (callback) => {
    poll = callback;
    return 1;
  };
  try {
    b.start_signal_poll();
  } finally {
    web_window.setInterval = saved;
  }
  b.FromWasmRequestAnimationFrame();
  b.signal_flags = 1;
  poll();
  assert.equal(b.pumps.length, 0);
  run_frame();
  assert.deepEqual(b.pumps, [[["Signal", 1], ["AnimationFrame"]]]);
  b.signal_flags = 1;
  frames.length = 0;
  b.req_anim_frame_id = 0;
  poll();
  assert.deepEqual(b.pumps[1], [["Signal", 1]]);
});

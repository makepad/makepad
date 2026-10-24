import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// Audio without threads: the UI thread renders ahead into a FIFO (MakepadAudioFifoOutput)
// that a worklet without wasm plays (makepad_audio_fifo_worklet). Here both run against a
// virtual audio device: the worklet's render quanta at the sample rate, the page's pumps on a
// timer and on animation frames, messages between them delivered as the clock moves.

const source = readFileSync(new URL("./web.js", import.meta.url), "utf8")
  .replace(/^import .*wasm_bridge\.js"\n/, "")
  .replace(/^export /gm, "");

const clock = { ms: 0 };
const load = new Function(
  "WasmBridge",
  "window",
  "document",
  "navigator",
  "screen",
  "performance",
  "console",
  `${source}\nreturn { MakepadAudioFifoOutput, makepad_audio_fifo_worklet, MAKEPAD_AUDIO_BLOCK_FRAMES, MAKEPAD_AUDIO_FIFO_LEAD_S, MAKEPAD_AUDIO_PUMP_INTERVAL_S };`,
);
const web = load(
  class {},
  undefined,
  {},
  { userAgent: "test", platform: "test" },
  { width: 0, height: 0 },
  { now: () => clock.ms },
  { log() {}, warn() {}, error() {} },
);
const BLOCK = web.MAKEPAD_AUDIO_BLOCK_FRAMES;
const RATE = 48000;
const OUTPUT_LATENCY_S = 0.012;

// The worklet's processor class, from the same source text the page loads as a blob.
function worklet_class() {
  let registered = null;
  class AudioWorkletProcessor {
    constructor() {
      this.port = { onmessage: null, postMessage: () => {} };
    }
  }
  new Function("AudioWorkletProcessor", "registerProcessor", `(${web.makepad_audio_fifo_worklet.toString()})();`)(
    AudioWorkletProcessor,
    (name, cls) => {
      assert.equal(name, "makepad-audio-fifo");
      registered = cls;
    },
  );
  return registered;
}

// A device with one fifo output: the context's clock, the worklet, the two ports and the app's
// render (left = +stream frame, right = -stream frame, so order and gaps show in the output).
function device({ state = "running" } = {}) {
  const Processor = worklet_class();
  const processor = new Processor();
  const to_worklet = [];
  const to_page = [];
  const page_port = { onmessage: null, postMessage: (message) => to_worklet.push(message) };
  processor.port.postMessage = (message) => to_page.push(message);
  const d = {
    quanta: 0,
    start_ms: clock.ms,
    out_left: [],
    out_right: [],
    renders: [],
    processor,
    context: {
      sampleRate: RATE,
      state,
      get currentTime() {
        return (d.quanta * BLOCK) / RATE;
      },
      // The quantum the device plays now was rendered OUTPUT_LATENCY_S ago.
      getOutputTimestamp() {
        const context_time = Math.max(0, (clock.ms - d.start_ms) / 1000 - OUTPUT_LATENCY_S);
        return { contextTime: context_time, performanceTime: d.start_ms + (context_time + OUTPUT_LATENCY_S) * 1000 };
      },
    },
  };
  const planes = new Float32Array(2 * BLOCK);
  d.fifo = new web.MakepadAudioFifoOutput(d.context, page_port, (frames, stream_frame, heard_at) => {
    assert.equal(frames, BLOCK);
    d.renders.push({ stream_frame, heard_at, now: clock.ms / 1000 });
    for (let i = 0; i < frames; i++) {
      planes[i] = stream_frame + i;
      planes[frames + i] = -(stream_frame + i);
    }
    return planes;
  });
  d.deliver = () => {
    for (const message of to_worklet.splice(0)) processor.port.onmessage({ data: message });
    for (const message of to_page.splice(0)) page_port.onmessage && page_port.onmessage({ data: message });
  };
  // One render quantum of the audio thread.
  d.quantum = () => {
    globalThis.currentFrame = d.quanta * BLOCK;
    const left = new Float32Array(BLOCK);
    const right = new Float32Array(BLOCK);
    const alive = processor.process([], [[left, right]]);
    d.quanta += 1;
    d.out_left.push(...left);
    d.out_right.push(...right);
    return alive;
  };
  // The page's event loop and the audio thread until `until_ms`: quanta at the sample
  // rate, the pump timer, 60 Hz animation frames (none in `stalls`: a busy main thread).
  d.run = (until_ms, stalls = []) => {
    const quantum_ms = (BLOCK / RATE) * 1000;
    const pump_ms = web.MAKEPAD_AUDIO_PUMP_INTERVAL_S * 1000;
    let next_pump = clock.ms;
    let next_frame = clock.ms;
    while (clock.ms < until_ms) {
      const next_quantum = d.start_ms + d.quanta * quantum_ms;
      const t = Math.min(next_quantum, next_pump, next_frame);
      clock.ms = t;
      d.deliver();
      const stalled = stalls.some(([a, b]) => t >= a && t < b);
      if (t === next_quantum && d.context.state === "running") {
        d.quantum();
      }
      if (t === next_pump) {
        if (!stalled) d.fifo.pump();
        next_pump += pump_ms;
      }
      if (t === next_frame) {
        if (!stalled) d.fifo.pump();
        next_frame += 1000 / 60;
      }
      if (d.context.state !== "running" && t === next_quantum) {
        d.start_ms += quantum_ms;
      }
    }
    d.deliver();
  };
  return d;
}

// The output from stream frame 0 on (frame 1 is the first non-zero sample; before it the
// worklet played silence, unfed).
function played(d) {
  const first = d.out_left.indexOf(1) - 1;
  assert.ok(first >= 0);
  return { left: d.out_left.slice(first), right: d.out_right.slice(first) };
}

test("steady state: in order, no underrun, consumed counts and lead hold", () => {
  clock.ms = 1000;
  const d = device();
  d.run(3000);
  const { left, right } = played(d);
  for (let i = 0; i < left.length; i++) {
    assert.equal(left[i], i, `left ${i}`);
    assert.equal(right[i], -i, `right ${i}`);
  }
  assert.equal(d.processor.underruns, 0);
  assert.equal(d.fifo.report.underruns, 0);
  // The worklet's reports count what it played.
  // Every quantum played the stream except the first (it ran before the first pump, unfed).
  assert.equal(d.processor.consumed, (d.quanta - 1) * BLOCK);
  assert.ok(d.fifo.report.consumed <= d.processor.consumed && d.fifo.report.consumed >= d.processor.consumed - 8 * BLOCK);
  // Two seconds at 48 kHz: what was played plus at most the lead (and one block of rounding).
  const lead = d.fifo.lead_frames;
  assert.equal(lead, Math.ceil((web.MAKEPAD_AUDIO_FIFO_LEAD_S * RATE) / BLOCK) * BLOCK);
  assert.ok(d.fifo.written >= d.processor.consumed && d.fifo.written <= d.processor.consumed + lead + BLOCK);
  // Every block rendered once, in stream order.
  d.renders.forEach((r, i) => assert.equal(r.stream_frame, i * BLOCK));
  assert.equal(d.fifo.blocks, d.renders.length);
});

test("heard times: a block is heard after it is rendered, about the lead plus the device latency", () => {
  clock.ms = 1000;
  const d = device();
  d.run(2000);
  for (let i = 1; i < d.renders.length; i++) {
    assert.ok(d.renders[i].heard_at > d.renders[i - 1].heard_at);
  }
  const late = d.renders.filter((r) => r.now > 1.5);
  assert.ok(late.length > 100);
  for (const r of late) {
    const ahead = r.heard_at - r.now;
    assert.ok(ahead >= OUTPUT_LATENCY_S && ahead <= web.MAKEPAD_AUDIO_FIFO_LEAD_S + OUTPUT_LATENCY_S + 0.02, `ahead ${ahead}`);
  }
  // The heard frame: what the device plays now, OUTPUT_LATENCY_S behind the worklet.
  const behind = (d.fifo.consumed() - d.fifo.heard_frame()) / RATE;
  assert.ok(Math.abs(behind - OUTPUT_LATENCY_S) < (2 * BLOCK) / RATE, `behind ${behind}`);
});

test("a stalled main thread underruns, counts it, and loses or repeats no sample", () => {
  clock.ms = 1000;
  const d = device();
  d.run(1500);
  assert.equal(d.processor.underruns, 0);
  d.run(2500, [[1500, 1600]]);
  assert.ok(d.processor.underruns > 0);
  assert.equal(d.fifo.report.underruns, d.processor.underruns);
  assert.equal(d.fifo.report.underrun_frames, d.processor.underrun_frames);
  // The stall was 100 ms (plus the last pump's distance before it); the FIFO held 40.
  const silent_ms = (d.processor.underrun_frames / RATE) * 1000;
  assert.ok(silent_ms > 45 && silent_ms < 85, `silent ${silent_ms} ms`);
  const { left } = played(d);
  const sound = left.filter((v, i) => v !== 0 || i === 0);
  for (let i = 0; i < sound.length; i++) assert.equal(sound[i], i);
  // Refilled after the stall: no underrun since.
  const after = d.processor.underruns;
  d.run(3500);
  assert.equal(d.processor.underruns, after);
});

test("a suspended context renders nothing ahead", () => {
  clock.ms = 1000;
  const d = device({ state: "suspended" });
  d.run(1200);
  assert.equal(d.renders.length, 0);
  d.context.state = "running";
  d.run(1400);
  assert.ok(d.renders.length > 0);
  assert.equal(d.processor.underruns, 0);
});

test("stop ends the worklet; a new output starts its stream at frame zero", () => {
  clock.ms = 1000;
  const d = device();
  d.run(1300);
  d.fifo.stop();
  d.deliver();
  assert.equal(d.quantum(), false);
  const e = device();
  e.run(1600);
  assert.equal(e.renders[0].stream_frame, 0);
  const { left } = played(e);
  for (let i = 0; i < left.length; i++) assert.equal(left[i], i);
  assert.equal(e.processor.underruns, 0);
});

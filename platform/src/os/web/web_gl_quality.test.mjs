import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const quiet_console = {
  log() {},
  warn() {},
  error() {},
};

const web_window = {
  devicePixelRatio: 2,
  innerWidth: 1920,
  innerHeight: 1080,
  navigator: { userAgent: "test", platform: "test" },
  addEventListener() {},
};
const web_document = {
  fullscreenEnabled: false,
  webkitFullscreenEnabled: false,
  mozFullscreenEnabled: false,
  fullscreenElement: null,
  webkitFullscreenElement: null,
  mozFullscreenElement: null,
};
class MockWasmBridge {
  static phone = false;
  static is_phone() {
    return this.phone;
  }
}

const web_source = readFileSync(new URL("./web.js", import.meta.url), "utf8")
  .replace(/^import .*wasm_bridge\.js"\n/, "")
  .replace(/^export /gm, "");
const load_web = new Function(
  "WasmBridge",
  "window",
  "document",
  "navigator",
  "screen",
  "performance",
  "console",
  `${web_source}\nreturn {
    MAKEPAD_WEBGL_PIXEL_BUDGET,
    WasmWebBrowser,
    makepad_crash_reporter,
    makepad_compute_webgl_size,
    makepad_device_pixel_ratio
  };`,
);
const web = load_web(
  MockWasmBridge,
  web_window,
  web_document,
  web_window.navigator,
  { width: 0, height: 0 },
  { now: () => 0 },
  quiet_console,
);

const gl_window = {
  devicePixelRatio: 2,
  location: { reload() {} },
  media: [],
  cancelled: [],
  cleared_intervals: [],
  cleared_timeouts: [],
  addEventListener() {},
  matchMedia(query) {
    const media = {
      query,
      listener: null,
      addEventListener(_name, listener) {
        this.listener = listener;
      },
      removeEventListener(_name, listener) {
        if (this.listener === listener) this.listener = null;
      },
    };
    this.media.push(media);
    return media;
  },
  setInterval(callback) {
    this.interval_callback = callback;
    return 91;
  },
  clearInterval(id) {
    this.cleared_intervals.push(id);
  },
  clearTimeout(id) {
    this.cleared_timeouts.push(id);
  },
  requestAnimationFrame() {
    throw new Error("an animation frame must not be requested in this test");
  },
  cancelAnimationFrame(id) {
    this.cancelled.push(id);
  },
};

function mock_element(tag) {
  return {
    tag,
    type: "",
    textContent: "",
    style: {},
    children: [],
    listeners: {},
    setAttribute(name, value) {
      this[name] = value;
    },
    appendChild(child) {
      this.children.push(child);
      child.parentNode = this;
    },
    addEventListener(name, listener) {
      this.listeners[name] = listener;
    },
  };
}

const gl_document = {
  body: mock_element("body"),
  createElement: mock_element,
};
const gl_messages = { errors: [], warnings: [] };
const gl_console = {
  log() {},
  warn(...parts) {
    gl_messages.warnings.push(parts);
  },
  error(...parts) {
    gl_messages.errors.push(parts);
  },
};

const web_gl_source = readFileSync(
  new URL("./web_gl.js", import.meta.url),
  "utf8",
)
  .replace(/^import \{[\s\S]*?\} from "\.\/web\.js";\n/, "")
  .replace(/^export /gm, "");
const load_web_gl = new Function(
  "MAKEPAD_WEBGL_PIXEL_BUDGET",
  "WasmWebBrowser",
  "makepad_compute_webgl_size",
  "makepad_device_pixel_ratio",
  "window",
  "document",
  "performance",
  "console",
  `${web_gl_source}\nreturn {
    WasmWebGL,
    makepad_query_webgl_limits,
    makepad_render_target_size
  };`,
);
const web_gl = load_web_gl(
  web.MAKEPAD_WEBGL_PIXEL_BUDGET,
  web.WasmWebBrowser,
  web.makepad_compute_webgl_size,
  web.makepad_device_pixel_ratio,
  gl_window,
  gl_document,
  { now: () => 0 },
  gl_console,
);

function assert_bounded(size, budget = web.MAKEPAD_WEBGL_PIXEL_BUDGET) {
  assert.ok(size.width * size.height <= budget);
  assert.equal(size.width, Math.floor(size.logical_width * size.scale));
  assert.equal(size.height, Math.floor(size.logical_height * size.scale));
}

test("large, retina, phone, and hidden viewports use a uniform bounded DPI", () => {
  const limits = { max_width: 8192, max_height: 8192 };

  const large = web.makepad_compute_webgl_size(10000, 10000, 1.5, limits);
  assert.ok(large.scale < 1);
  assert_bounded(large);

  const retina = web.makepad_compute_webgl_size(800, 600, 1.5, limits);
  assert.equal(retina.scale, 1.5);
  assert.deepEqual([retina.width, retina.height], [1200, 900]);

  const phone = web.makepad_compute_webgl_size(390, 844, 1.0, limits);
  assert.equal(phone.scale, 1.0);
  assert.deepEqual([phone.width, phone.height], [390, 844]);

  MockWasmBridge.phone = true;
  const phone_browser = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
    canvas: { getAttribute() { return null; } },
    render_quality: null,
  });
  assert.equal(phone_browser.ensure_render_quality().dpr_ceiling, 1.0);
  MockWasmBridge.phone = false;

  const hidden = web.makepad_compute_webgl_size(0, Number.NaN, 1.5, limits);
  assert.deepEqual([hidden.width, hidden.height], [0, 0]);
  assert.equal(hidden.scale, 1.5);
});

test("canvas pixels and WASM window info receive the same effective DPI", () => {
  const viewports = [];
  const canvas = {
    width: 0,
    height: 0,
    offsetWidth: 1920,
    offsetHeight: 1080,
    getAttribute() {
      return null;
    },
  };
  const browser = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
    canvas,
    detect: { is_add_to_homescreen_safari: false },
    gl: { viewport: (...args) => viewports.push(args) },
    webgl_context_lost: false,
    webgl_limits: { max_width: 4096, max_height: 4096 },
    render_quality: null,
    window_info: {},
  });
  web.WasmWebBrowser.prototype.update_window_info.call(browser);

  assert.equal(browser.window_info.dpi_factor, browser.dpi_factor);
  assert.equal(canvas.width, Math.floor(1920 * browser.window_info.dpi_factor));
  assert.equal(canvas.height, Math.floor(1080 * browser.window_info.dpi_factor));
  assert.deepEqual(viewports.at(-1), [0, 0, canvas.width, canvas.height]);
  assert.deepEqual(
    [browser.window_info.inner_width, browser.window_info.inner_height],
    [1920, 1080],
  );
});

test("unchanged canvas dimensions do not reallocate its backbuffer", () => {
  let width = 960;
  let height = 720;
  let width_sets = 0;
  let height_sets = 0;
  const canvas = {
    offsetWidth: 640,
    offsetHeight: 480,
    getAttribute() { return null; },
    get width() { return width; },
    set width(value) { width = value; width_sets += 1; },
    get height() { return height; },
    set height(value) { height = value; height_sets += 1; },
  };
  const browser = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
    canvas,
    detect: { is_add_to_homescreen_safari: false },
    gl: { viewport() {} },
    webgl_context_lost: false,
    webgl_limits: { max_width: 4096, max_height: 4096 },
    render_quality: null,
    window_info: {},
  });

  browser.update_window_info();

  assert.equal(width_sets, 0);
  assert.equal(height_sets, 0);
});

test("timers reject malformed delays and clamp repeating rates", () => {
  const intervals = [];
  const timeouts = [];
  const previous_set_interval = web_window.setInterval;
  const previous_set_timeout = web_window.setTimeout;
  web_window.setInterval = (_callback, delay) => {
    intervals.push(delay);
    return intervals.length;
  };
  web_window.setTimeout = (_callback, delay) => {
    timeouts.push(delay);
    return timeouts.length;
  };
  try {
    const browser = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
      webgl_context_lost: false,
      timers: [],
      to_wasm: { ToWasmTimerFired() {} },
      do_wasm_pump() {},
    });
    browser.FromWasmStartTimer({ timer_id: 1, repeats: true, interval: Number.NaN });
    browser.FromWasmStartTimer({ timer_id: 2, repeats: true, interval: Number.POSITIVE_INFINITY });
    browser.FromWasmStartTimer({ timer_id: 3, repeats: true, interval: -1 });
    browser.FromWasmStartTimer({ timer_id: 4, repeats: true, interval: 0 });
    browser.FromWasmStartTimer({ timer_id: 5, repeats: true, interval: 0.003 });
    browser.FromWasmStartTimer({ timer_id: 6, repeats: true, interval: 0.01 });
    browser.FromWasmStartTimer({ timer_id: 7, repeats: false, interval: 0 });

    assert.deepEqual(intervals, [4, 4, 10]);
    assert.deepEqual(timeouts, [0]);
    assert.equal(browser.timers.length, 4);
  } finally {
    web_window.setInterval = previous_set_interval;
    web_window.setTimeout = previous_set_timeout;
  }
});

test("legacy HTTP requests preserve arbitrary binary bodies", () => {
  const previous_xhr = globalThis.XMLHttpRequest;
  let request;
  class MockXMLHttpRequest {
    constructor() {
      request = this;
      this.listeners = {};
      this.upload = { addEventListener() {} };
    }
    open() {}
    setRequestHeader() {}
    addEventListener(name, listener) { this.listeners[name] = listener; }
    send(body) { this.body = body; }
  }
  globalThis.XMLHttpRequest = MockXMLHttpRequest;
  try {
    let frees = 0;
    const browser = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
      webgl_context_lost: false,
      legacy_http_requests: new Set(),
      clone_data_u8() { return new Uint8Array([0xff, 0x00, 0x80]); },
      free_data_u8() { frees += 1; },
    });
    assert.doesNotThrow(() => browser.FromWasmHTTPRequest({
      method: "POST",
      url: "/binary",
      headers: "",
      body: {},
    }));
    assert.ok(request.body instanceof Uint8Array);
    assert.deepEqual([...request.body], [0xff, 0x00, 0x80]);
    assert.equal(frees, 1);
  } finally {
    if (previous_xhr === undefined) {
      delete globalThis.XMLHttpRequest;
    } else {
      globalThis.XMLHttpRequest = previous_xhr;
    }
  }
});

test("render targets are uniformly bounded and malformed sizes are rejected", () => {
  const limits = { max_width: 2048, max_height: 1024 };
  const target = web_gl.makepad_render_target_size(4096, 2048, limits);
  assert.equal(target.ok, true);
  assert.equal(target.scaled, true);
  assert.deepEqual([target.width, target.height], [2048, 1024]);
  assert.ok(target.width * target.height <= web.MAKEPAD_WEBGL_PIXEL_BUDGET);
  assert.equal(target.width / target.height, 2);

  assert.equal(
    web_gl.makepad_render_target_size(Number.POSITIVE_INFINITY, 100, limits).ok,
    false,
  );
  assert.equal(web_gl.makepad_render_target_size(0, 100, limits).ok, false);
});

test("a malformed render pass is rejected without GL allocation or exception", () => {
  const viewports = [];
  const errors_before = gl_messages.errors.length;
  const subject = Object.assign(Object.create(web_gl.WasmWebGL.prototype), {
    canvas: { getAttribute() { return null; } },
    gl: {
      FRAMEBUFFER: 1,
      bindFramebuffer() {},
      viewport: (...args) => viewports.push(args),
    },
    webgl_context_lost: false,
    webgl_limits: { max_width: 2048, max_height: 2048 },
    render_quality: null,
    render_target_rejected: false,
    _render_target_size_reports: new Set(),
    active_render_target_textures: new Set([{ id: "old-target" }]),
    xr: undefined,
  });
  const malformed = {
    width: Number.POSITIVE_INFINITY,
    height: 100,
    color_targets: [],
    depth_target: { attached: false },
  };

  assert.doesNotThrow(() => subject.FromWasmBeginRenderTexture(malformed));
  assert.doesNotThrow(() => subject.FromWasmBeginRenderTexture(malformed));
  assert.equal(subject.render_target_rejected, true);
  assert.equal(subject.active_render_target_textures.size, 0);
  assert.deepEqual(viewports, [[0, 0, 0, 0], [0, 0, 0, 0]]);
  assert.equal(gl_messages.errors.length, errors_before + 1);
});

function mock_gl() {
  const calls = new Map();
  const gl = {
    MAX_TEXTURE_SIZE: 1,
    MAX_CUBE_MAP_TEXTURE_SIZE: 2,
    MAX_RENDERBUFFER_SIZE: 3,
    MAX_VIEWPORT_DIMS: 4,
    MAX_VERTEX_UNIFORM_VECTORS: 5,
    MAX_FRAGMENT_UNIFORM_VECTORS: 6,
    MAX_VERTEX_ATTRIBS: 7,
    getParameter(parameter) {
      calls.set(parameter, (calls.get(parameter) || 0) + 1);
      if (parameter === this.MAX_TEXTURE_SIZE) return 8192;
      if (parameter === this.MAX_CUBE_MAP_TEXTURE_SIZE) return 4096;
      if (parameter === this.MAX_RENDERBUFFER_SIZE) return 4096;
      if (parameter === this.MAX_VIEWPORT_DIMS) return new Int32Array([6144, 3072]);
      if (parameter === this.MAX_VERTEX_ATTRIBS) return 16;
      return 256;
    },
    getExtension() {
      return null;
    },
  };
  gl.calls = calls;
  return gl;
}

function webgl_subject(attributes = {}) {
  const gl = mock_gl();
  const parent = mock_element("parent");
  const canvas = mock_element("canvas");
  canvas.width = 1200;
  canvas.height = 800;
  canvas.parentNode = parent;
  canvas.getAttribute = (name) => name in attributes ? attributes[name] : null;
  canvas.hasAttribute = (name) => name in attributes;
  canvas.getContext = (_kind, options) => {
    canvas.context_options = options;
    return gl;
  };
  const subject = Object.assign(Object.create(web_gl.WasmWebGL.prototype), {
    canvas,
    handlers: { on_screen_resize() { subject.resize_count += 1; } },
    resize_count: 0,
    timers: [],
    window_info: {
      dpi_factor: 1.25,
      inner_width: 960,
      inner_height: 640,
    },
    webgl_context_lost: false,
  });
  return { subject, canvas, gl, parent };
}

test("GL limits are queried once and canvas MSAA is opt-in", () => {
  const first = webgl_subject();
  assert.equal(first.subject.init_webgl_context(), true);
  assert.equal(first.canvas.context_options.antialias, false);
  assert.equal(first.canvas.context_options.powerPreference, "default");
  assert.equal("preferLowPowerToHighPerformance" in first.canvas.context_options, false);
  assert.deepEqual(first.subject.webgl_limits, {
    max_texture_size: 8192,
    max_cube_map_texture_size: 4096,
    max_width: 4096,
    max_height: 3072,
  });
  assert.equal(first.subject.init_webgl_context(), true);
  assert.equal(first.gl.calls.get(first.gl.MAX_TEXTURE_SIZE), 1);
  assert.equal(first.gl.calls.get(first.gl.MAX_CUBE_MAP_TEXTURE_SIZE), 1);
  assert.equal(first.gl.calls.get(first.gl.MAX_RENDERBUFFER_SIZE), 1);
  assert.equal(first.gl.calls.get(first.gl.MAX_VIEWPORT_DIMS), 1);
  assert.equal(first.gl.calls.get(first.gl.MAX_VERTEX_ATTRIBS), 1);

  const opted_in = webgl_subject({ antialias: "" });
  assert.equal(opted_in.subject.init_webgl_context(), true);
  assert.equal(opted_in.canvas.context_options.antialias, true);
});

function render_target_subject({ float_extension = true } = {}) {
  let object_id = 0;
  const calls = { framebuffers: 0, textures: 0, status: 0, images: 0 };
  const gl = {
    FRAMEBUFFER: 1,
    TEXTURE_2D: 2,
    COLOR_ATTACHMENT0: 3,
    DEPTH_STENCIL_ATTACHMENT: 4,
    FRAMEBUFFER_COMPLETE: 5,
    COLOR_BUFFER_BIT: 1,
    DEPTH_BUFFER_BIT: 2,
    STENCIL_BUFFER_BIT: 4,
    NO_ERROR: 0,
    RGBA: 6,
    UNSIGNED_BYTE: 7,
    LINEAR: 8,
    NEAREST: 9,
    TEXTURE_MAG_FILTER: 10,
    TEXTURE_MIN_FILTER: 11,
    TEXTURE_WRAP_S: 12,
    TEXTURE_WRAP_T: 13,
    CLAMP_TO_EDGE: 14,
    createFramebuffer() {
      calls.framebuffers += 1;
      return { id: ++object_id };
    },
    createTexture() {
      calls.textures += 1;
      return { id: ++object_id };
    },
    bindFramebuffer() {},
    bindTexture() {},
    texParameteri() {},
    texImage2D() { calls.images += 1; },
    framebufferTexture2D() {},
    getError() { return this.NO_ERROR; },
    checkFramebufferStatus() {
      calls.status += 1;
      return this.next_status || this.FRAMEBUFFER_COMPLETE;
    },
    viewport() {},
    depthMask() {},
    clearColor() {},
    clearDepth() {},
    clear() {},
  };
  const subject = Object.assign(Object.create(web_gl.WasmWebGL.prototype), {
    gl,
    ext_color_buffer_float: float_extension ? {} : null,
    webgl_context_lost: false,
    render_target_rejected: false,
    texture_pass_front_face_cw: false,
    webgl_limits: { max_width: 1024, max_height: 1024 },
    framebuffers: [],
    textures: [],
    xr: undefined,
    _render_target_size_reports: new Set(),
    ensure_render_quality() {
      return { pixel_budget: web.MAKEPAD_WEBGL_PIXEL_BUDGET };
    },
  });
  return { subject, gl, calls };
}

function render_target_args(texture_id, width = 100, height = 100, format = 0) {
  return {
    pass_id: 1,
    width,
    height,
    color_targets: [{
      texture_id,
      format,
      init_only: false,
      clear_color: { r: 0, g: 0, b: 0, a: 0 },
    }],
    depth_target: { attached: false },
  };
}

test("R32F targets reject missing float support and any safety downscale before allocation", () => {
  const missing = render_target_subject({ float_extension: false });
  missing.subject.textures[7] = { _render_target_valid: true };
  missing.subject.FromWasmBeginRenderTexture(render_target_args(7, 100, 100, 1));
  assert.equal(missing.subject.render_target_rejected, true);
  assert.equal(missing.subject.textures[7]._render_target_valid, false);
  assert.equal(missing.calls.framebuffers, 0);
  assert.equal(missing.calls.textures, 0);

  const oversized = render_target_subject();
  oversized.subject.webgl_limits = { max_width: 64, max_height: 64 };
  oversized.subject.FromWasmBeginRenderTexture(render_target_args(8, 128, 64, 1));
  assert.equal(oversized.subject.render_target_rejected, true);
  assert.equal(oversized.calls.framebuffers, 0);
  assert.equal(oversized.calls.textures, 0);
});

test("changing same-sized framebuffer attachments rechecks completeness and invalidates failures", () => {
  const { subject, gl, calls } = render_target_subject();
  subject.FromWasmBeginRenderTexture(render_target_args(1));
  assert.equal(subject.render_target_rejected, false);
  assert.equal(calls.status, 1);

  subject.textures[2] = {
    _width: 100,
    _height: 100,
    _format: 0,
    _render_target_valid: true,
  };
  gl.next_status = 999;
  subject.FromWasmBeginRenderTexture(render_target_args(2));
  assert.equal(calls.status, 2);
  assert.equal(subject.render_target_rejected, true);
  assert.equal(subject.textures[2]._render_target_valid, false);
  assert.equal(calls.images, 1);
});

test("physical DPR listener does not loop when effective DPR is lower", () => {
  const { subject } = webgl_subject();
  subject.init_webgl_context();
  assert.equal(subject.physical_device_dpi, 2);
  assert.equal(subject.window_info.dpi_factor, 1.25);

  gl_window.devicePixelRatio = 3;
  gl_window.media.at(-1).listener();
  assert.equal(subject.resize_count, 1);
  assert.equal(subject.physical_device_dpi, 3);
  gl_window.media.at(-1).listener();
  assert.equal(subject.resize_count, 1);
});

test("context loss stops queued work and reloads only after an explicit click", () => {
  gl_window.devicePixelRatio = 2;
  let reloads = 0;
  gl_window.location.reload = () => { reloads += 1; };
  const { subject, canvas, parent } = webgl_subject();
  subject.init_webgl_context();
  subject.req_anim_frame_id = 11;
  subject.webgl_shader_poll_frame_id = 12;
  subject.video_anim_frame_id = 13;
  subject.loader_after_presented_frame_id = 14;
  subject.poll_timer = 15;
  subject.webgl_shader_summary_timer = 16;
  subject.timers = [
    { repeats: true, sys_id: 17 },
    { repeats: false, sys_id: 18 },
  ];
  let prevent_default_calls = 0;
  const loss = canvas.listeners.webglcontextlost;
  loss({ preventDefault() { prevent_default_calls += 1; } });

  assert.equal(subject.webgl_context_lost, true);
  assert.equal(prevent_default_calls, 0);
  assert.equal(reloads, 0);
  assert.deepEqual(gl_window.cancelled.slice(-4), [11, 12, 13, 14]);
  assert.equal(subject.timers.length, 0);
  assert.equal(parent.children.length, 1);
  assert.equal(
    gl_messages.errors.filter(parts => String(parts[0]).includes("context lost")).length,
    1,
  );

  loss({ preventDefault() { prevent_default_calls += 1; } });
  assert.equal(parent.children.length, 1);
  assert.equal(reloads, 0);
  const reload_button = parent.children[0].children[1];
  reload_button.listeners.click();
  assert.equal(reloads, 1);
});

test("a lost-context GL allocation failure is terminal, not a Wasm crash", () => {
  const reporter = web.makepad_crash_reporter;
  const previous = {
    is_wasm_dead: reporter.is_wasm_dead,
    mark_wasm_dead: reporter.mark_wasm_dead,
    report: reporter.report,
  };
  const reports = [];
  let marked_dead = 0;
  reporter.is_wasm_dead = () => false;
  reporter.mark_wasm_dead = () => { marked_dead += 1; };
  reporter.report = (...args) => { reports.push(args); return Promise.resolve(true); };
  try {
    let loss_queries = 0;
    let cleanup = 0;
    let overlays = 0;
    const subject = Object.assign(Object.create(web_gl.WasmWebGL.prototype), {
      gl: {
        VERTEX_SHADER: 1,
        createShader() { return null; },
        shaderSource(shader) {
          assert.equal(shader, null);
          throw new TypeError("null shader allocation");
        },
        isContextLost() { loss_queries += 1; return true; },
      },
      canvas: { width: 10, height: 10 },
      window_info: {},
      physical_device_dpi: 1,
      webgl_context_lost: false,
      buffer_upload_serial: 0,
      to_wasm: {},
      new_to_wasm: () => ({}),
      wasm_process_msg() {
        return {
          dispatch_on_app: () => this.FromWasmCompileWebGLShader({
            shader_id: 0,
            vertex: "",
            pixel: "",
          }),
          free() {},
        };
      },
      webgl_shader_timeline_start: undefined,
      webgl_shader_batch_program_count: 0,
      active_render_target_textures: new Set(),
      stop_webgl_runtime() { cleanup += 1; },
      show_webgl_context_lost_message() { overlays += 1; },
    });

    assert.doesNotThrow(() => subject.do_wasm_pump());
    subject.handle_webgl_context_lost({});
    subject.do_wasm_pump();
    assert.equal(subject.webgl_context_lost, true);
    assert.equal(loss_queries, 1);
    assert.equal(cleanup, 1);
    assert.equal(overlays, 1);
    assert.equal(marked_dead, 0);
    assert.equal(reports.length, 0);

    const wasm_error = new Error("actual Wasm failure");
    const actual = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
      webgl_context_lost: false,
      gl: { isContextLost() { return false; } },
      buffer_upload_serial: 0,
      to_wasm: {},
      new_to_wasm: () => ({}),
      wasm_process_msg() { throw wasm_error; },
    });
    assert.throws(() => actual.do_wasm_pump(), /actual Wasm failure/);
    assert.equal(marked_dead, 1);
    assert.equal(reports.length, 1);
    assert.equal(reports[0][0], "window.error");
  } finally {
    reporter.is_wasm_dead = previous.is_wasm_dead;
    reporter.mark_wasm_dead = previous.mark_wasm_dead;
    reporter.report = previous.report;
  }
});

test("lost context during vertex rejection skips Wasm message free", () => {
  const reporter = web.makepad_crash_reporter;
  const previous = {
    is_wasm_dead: reporter.is_wasm_dead,
    mark_wasm_dead: reporter.mark_wasm_dead,
    report: reporter.report,
  };
  let marked_dead = 0;
  let reports = 0;
  reporter.is_wasm_dead = () => false;
  reporter.mark_wasm_dead = () => { marked_dead += 1; };
  reporter.report = () => { reports += 1; return Promise.resolve(true); };
  try {
    let dispatches = 0;
    let frees = 0;
    let cleanup = 0;
    let overlays = 0;
    let workers_terminated = false;
    const ordinary_rejections = () => gl_messages.errors.filter(
      parts => String(parts[0]).includes("vertex submission rejected"),
    ).length;
    const rejection_count = ordinary_rejections();
    const subject = Object.assign(Object.create(web_gl.WasmWebGL.prototype), {
      gl: { isContextLost() { return true; } },
      canvas: { width: 10, height: 10 },
      window_info: {},
      physical_device_dpi: 1,
      webgl_context_lost: false,
      buffer_upload_serial: 0,
      to_wasm: {},
      new_to_wasm: () => ({}),
      wasm_process_msg() {
        return {
          dispatch_on_app: () => {
            dispatches += 1;
            this.report_vertex_submission_once(
              "instancebufferinvalid",
              "WebGL buffer upload error37442",
              {},
            );
          },
          free() {
            frees += 1;
            if (workers_terminated) {
              throw new Error("freed Wasm message after worker termination");
            }
          },
        };
      },
      reset_active_render_target_textures() {},
      stop_webgl_runtime() {
        cleanup += 1;
        workers_terminated = true;
      },
      show_webgl_context_lost_message() { overlays += 1; },
    });

    assert.doesNotThrow(() => subject.do_wasm_pump());
    assert.equal(dispatches, 1);
    assert.equal(subject.webgl_context_lost, true);
    assert.equal(cleanup, 1);
    assert.equal(overlays, 1);
    assert.equal(frees, 0);
    assert.equal(ordinary_rejections(), rejection_count);
    assert.equal(marked_dead, 0);
    assert.equal(reports, 0);
  } finally {
    reporter.is_wasm_dead = previous.is_wasm_dead;
    reporter.mark_wasm_dead = previous.mark_wasm_dead;
    reporter.report = previous.report;
  }
});

test("terminal context loss abandons workers and disposes owned I/O and media once", () => {
  const calls = {
    wasm: 0,
    worker: 0,
    fetch: 0,
    xhr: 0,
    socket: 0,
    video_pause: 0,
    video_load: 0,
    audio_disconnect: 0,
    audio_close: 0,
    geolocation: 0,
  };
  const worker = {
    onmessage() {},
    onerror() {},
    onmessageerror() {},
    terminate() { calls.worker += 1; },
  };
  const controller = { abort() { calls.fetch += 1; } };
  const xhr = { abort() { calls.xhr += 1; } };
  const socket = {
    onopen() {},
    onmessage() {},
    onerror() {},
    onclose() {},
    close() { calls.socket += 1; },
  };
  const video = {
    pause() { calls.video_pause += 1; },
    load() { calls.video_load += 1; },
    removeAttribute() {},
    removeEventListener() {},
  };
  const player = {
    video,
    playing: true,
    disposed: false,
    handlers: { loadedmetadata() {}, ended() {}, play() {}, pause() {} },
  };
  const audio_worklet = {
    port: { onmessage() {} },
    onprocessorerror() {},
    disconnect() { calls.audio_disconnect += 1; },
  };
  const audio_context = {
    close() {
      calls.audio_close += 1;
      throw new Error("media close failed");
    },
  };
  const parent = mock_element("parent");
  const subject = Object.assign(Object.create(web_gl.WasmWebGL.prototype), {
    webgl_context_lost: false,
    canvas: { width: 10, height: 10, parentNode: parent },
    window_info: {},
    physical_device_dpi: 1,
    timers: [],
    workers: new Map([[7, {
      worker,
      thread_info: { tls_ptr: 64, alloc_words: 8 },
      started: true,
      closed: false,
    }]]),
    thread_stack_arena: [{ ptr: 32, words: 4 }],
    network_http_requests: new Map([["1:2", {
      state: "active",
      stall_timer: null,
      controller,
    }]]),
    network_http_hosts: new Map([["host", {}]]),
    legacy_http_requests: new Set([xhr]),
    network_web_sockets: { socket },
    midi_inputs: [],
    geo_watch_id: 44,
    video_players: { video: player },
    pending_render_texture_captures: new Set(),
    audio_start_args: { pending: true },
    audio_callback_watchdog: null,
    audio_startup_cancel: null,
    audio_worklet,
    audio_context,
    exports: new Proxy({}, { get: () => () => { calls.wasm += 1; } }),
    to_wasm: new Proxy({}, { get: () => () => { calls.wasm += 1; } }),
    reset_active_render_target_textures() {},
    release_device_pixel_ratio_media_query() {},
  });
  const late_video_callback = player.handlers.loadedmetadata;
  const previous_geolocation = web_window.navigator.geolocation;
  web_window.navigator.geolocation = {
    clearWatch(id) {
      assert.equal(id, 44);
      calls.geolocation += 1;
    },
  };

  subject.handle_webgl_context_lost({});
  subject.handle_webgl_context_lost({});
  late_video_callback();
  web_window.navigator.geolocation = previous_geolocation;

  assert.deepEqual(calls, {
    wasm: 0,
    worker: 1,
    fetch: 1,
    xhr: 1,
    socket: 1,
    video_pause: 1,
    video_load: 1,
    audio_disconnect: 1,
    audio_close: 1,
    geolocation: 1,
  });
  assert.equal(worker.onmessage, null);
  assert.equal(subject.workers.size, 0);
  assert.equal(subject.thread_stack_arena.length, 0);
  assert.equal(subject.network_http_requests.size, 0);
  assert.equal(subject.video_players.video, undefined);
  assert.equal(subject.geo_watch_id, undefined);
  assert.equal(parent.children.length, 1);
});

test("late wake, media, and awaited worker startup refuse terminal Wasm work", async () => {
  let wasm_calls = 0;
  const wake = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
    webgl_context_lost: false,
    ui_wake_queued: false,
    exports: { wasm_check_signal() { wasm_calls += 1; return 1; } },
    to_wasm: { ToWasmSignal() { wasm_calls += 1; } },
    do_wasm_pump() { wasm_calls += 1; },
  });
  wake.js_wake_ui();
  wake.webgl_context_lost = true;
  await Promise.resolve();

  let resolve_secondary;
  const secondary_ready = new Promise(resolve => { resolve_secondary = resolve; });
  let allocations = 0;
  const thread = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
    webgl_context_lost: false,
    wasm: { _secondary_ready: secondary_ready, _has_thread_support: true },
    workers: new Map(),
    alloc_thread_stack() { allocations += 1; return {}; },
  });
  thread.create_thread({ request_id: 9, context_ptr: 0, stack_size: 0, name: "late" });
  thread.webgl_context_lost = true;
  resolve_secondary();

  let resolve_devices;
  const devices = new Promise(resolve => { resolve_devices = resolve; });
  const previous_media_devices = web_window.navigator.mediaDevices;
  web_window.navigator.mediaDevices = { enumerateDevices: () => devices };
  const media = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
    webgl_context_lost: false,
    to_wasm: { ToWasmAudioDeviceList() { wasm_calls += 1; } },
    do_wasm_pump() { wasm_calls += 1; },
  });
  media.FromWasmQueryAudioDevices({});
  media.webgl_context_lost = true;
  resolve_devices([]);
  await Promise.resolve();
  await Promise.resolve();
  web_window.navigator.mediaDevices = previous_media_devices;

  assert.equal(wasm_calls, 0);
  assert.equal(allocations, 0);
  assert.equal(thread.workers.size, 0);
});

test("lifecycle listeners stay inert after terminal abandon", () => {
  const window_listeners = {};
  const document_listeners = {};
  const previous_window_listener = web_window.addEventListener;
  const previous_document_listener = web_document.addEventListener;
  const previous_hidden = web_document.hidden;
  web_window.addEventListener = (name, listener) => { window_listeners[name] = listener; };
  web_document.addEventListener = (name, listener) => { document_listeners[name] = listener; };
  web_document.hidden = false;
  try {
    let wasm_calls = 0;
    const browser = Object.assign(Object.create(web.WasmWebBrowser.prototype), {
      webgl_context_lost: false,
      to_wasm: { ToWasmAppLifecycle() { wasm_calls += 1; } },
      do_wasm_pump() { wasm_calls += 1; },
      shutdown_thread_runtime() { wasm_calls += 1; },
    });
    browser.bind_app_lifecycle();
    browser.webgl_context_lost = true;
    web_document.hidden = true;
    document_listeners.visibilitychange();
    window_listeners.pagehide({ persisted: false });
    window_listeners.pageshow({ persisted: true });

    assert.equal(wasm_calls, 0);
    assert.equal(browser.lifecycle_is_visible, true);
    assert.equal(browser.lifecycle_shutdown_sent, false);
  } finally {
    web_window.addEventListener = previous_window_listener;
    web_document.addEventListener = previous_document_listener;
    web_document.hidden = previous_hidden;
  }
});

test("terminal cleanup cancels readback polling without touching lost GL", () => {
  let next_frame = 40;
  const callbacks = new Map();
  const previous_request = gl_window.requestAnimationFrame;
  gl_window.requestAnimationFrame = callback => {
    next_frame += 1;
    callbacks.set(next_frame, callback);
    return next_frame;
  };
  const gl_calls = { wait: 0, lost: 0, delete: 0 };
  const gl = {
    FRAMEBUFFER: 1,
    FRAMEBUFFER_BINDING: 2,
    PIXEL_PACK_BUFFER: 3,
    PIXEL_PACK_BUFFER_BINDING: 4,
    PACK_ALIGNMENT: 5,
    COLOR_ATTACHMENT0: 6,
    TEXTURE_2D: 7,
    FRAMEBUFFER_COMPLETE: 8,
    STREAM_READ: 9,
    RGBA: 10,
    UNSIGNED_BYTE: 11,
    SYNC_GPU_COMMANDS_COMPLETE: 12,
    NO_ERROR: 0,
    createFramebuffer: () => ({}),
    createBuffer: () => ({}),
    getParameter: () => null,
    bindFramebuffer() {},
    framebufferTexture2D() {},
    checkFramebufferStatus() { return this.FRAMEBUFFER_COMPLETE; },
    bindBuffer() {},
    bufferData() {},
    pixelStorei() {},
    readPixels() {},
    fenceSync: () => ({}),
    flush() {},
    getError() { return this.NO_ERROR; },
    deleteSync() { gl_calls.delete += 1; },
    deleteBuffer() { gl_calls.delete += 1; },
    deleteFramebuffer() { gl_calls.delete += 1; },
    isContextLost() { gl_calls.lost += 1; return true; },
    clientWaitSync() { gl_calls.wait += 1; return 0; },
  };
  let wasm_calls = 0;
  const subject = Object.assign(Object.create(web_gl.WasmWebGL.prototype), {
    gl,
    wasm: {},
    webgl_context_lost: false,
    textures: [{ _render_target_valid: true, _width: 2, _height: 2 }],
    pending_render_texture_captures: new Set(),
    video_players: {},
    timers: [],
    to_wasm: { ToWasmRenderTextureCapture() { wasm_calls += 1; } },
    do_wasm_pump() { wasm_calls += 1; },
    release_device_pixel_ratio_media_query() {},
    stop_terminal_web_runtime() {},
  });

  subject.FromWasmRequestRenderTextureCapture({ texture_id: 0 });
  const queued_callback = callbacks.get(next_frame);
  assert.equal(subject.pending_render_texture_captures.size, 1);
  subject.webgl_context_lost = true;
  subject.stop_webgl_runtime();
  queued_callback();
  gl_window.requestAnimationFrame = previous_request;

  assert.ok(gl_window.cancelled.includes(next_frame));
  assert.equal(subject.pending_render_texture_captures.size, 0);
  assert.deepEqual(gl_calls, { wait: 0, lost: 0, delete: 0 });
  assert.equal(wasm_calls, 0);
});

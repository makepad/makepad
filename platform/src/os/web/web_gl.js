import {
  WasmWebBrowser,
  makepad_compute_webgl_size,
  makepad_device_pixel_ratio,
} from "./web.js";

const UBO_BINDING_PASS = 0;
const UBO_BINDING_DRAW_LIST = 1;
const UBO_BINDING_DRAW_CALL = 2;
const UBO_BINDING_USER = 3;
const UBO_BINDING_LIVE = 4;
const UBO_BINDING_CUSTOM_BASE = 5;
const STANDARD_UNIFORMS = [
  "pass_uniforms", "draw_list_uniforms", "draw_call_uniforms", "user_uniforms", "live_uniforms",
];

const MAKEPAD_WEBGL_FALLBACK_DIMENSION = 2048;
const MAKEPAD_WEBGL_FALLBACK_VERTEX_ATTRIBS = 16;
// WebGL's largest vertex attribute stride, in bytes.
const MAKEPAD_WEBGL_MAX_ATTRIB_STRIDE = 255;
// The instance data texture's row and its layers' height, in words (WebGL 2's
// minimum MAX_TEXTURE_SIZE). A layer is one window of 2048 * 2048 words; the
// shader emitter's INSTANCE_WORD_ROW_SHIFT addresses the same row and layer.
const MAKEPAD_INSTANCE_WORD_ROW = 2048;
const MAKEPAD_INSTANCE_WINDOW_WORDS = MAKEPAD_INSTANCE_WORD_ROW * MAKEPAD_INSTANCE_WORD_ROW;
// No byte or triangle caps of the runtime's own (GPU-SAFETY.md): sizes are
// bounded by the device's reported limits and by the wasm memory a slice
// must lie in; a driver that cannot allocate says so (GL_OUT_OF_MEMORY,
// checked where textures and buffers are allocated).
const MAKEPAD_WEBGL_MAX_SUBMISSION_REPORTS = 64;
// @section video-playback
const MAKEPAD_WEBGL_VIDEO_UPLOAD_FORMAT = "video-rgba8";
// @end video-playback
// With checks on (see gl_checks), an allocation's GL error is read at the
// latest this long after it: getError waits for the GPU process, so it runs
// when no earlier frame is still on the GPU, else at most this often.
const MAKEPAD_WEBGL_ALLOCATION_CHECK_MS = 2000;

const MAKEPAD_WEBGL_BGRA_IMAGE_UPLOAD = {
  faces: 1,
  bytes_per_texel: 4,
  elements_per_texel: 1,
  element_size: 4,
  nearest: false,
};

// GL error and framebuffer-completeness queries (gl.getError,
// gl.checkFramebufferStatus) run only when the page sets
// `window.makepad_gl_checks = true` before the runtime starts: development
// pages and the export's instrumented safety run do; a shipped page never
// queries. Each query waits for the GPU process to finish everything
// queued, behind a busy GPU a stall of whole GPU frames (25-100 ms) on
// every allocating frame. Without them a failed allocation surfaces as a
// lost context (webglcontextlost) or draws nothing.
function makepad_gl_checks_on() {
  return typeof window !== "undefined" && window.makepad_gl_checks === true;
}

function makepad_webgl_limit(value) {
  const number = Number(value);
  return Number.isFinite(number) && number >= 1
    ? Math.floor(number)
    : MAKEPAD_WEBGL_FALLBACK_DIMENSION;
}

function makepad_webgl_vertex_attrib_limit(value) {
  const number = Number(value);
  return Number.isSafeInteger(number) && number >= 1
    ? number
    : MAKEPAD_WEBGL_FALLBACK_VERTEX_ATTRIBS;
}

const INSTANCE_FAILURE_BACKING = 0;
const INSTANCE_FAILURE_REGION = 1;
const INSTANCE_FAILURE_MIRROR = 2;

function makepad_is_u32(value) {
  return Number.isInteger(value) && value >= 0 && value <= 0xffffffff;
}

function makepad_instance_region_valid(region) {
  return makepad_is_u32(region?.index) && makepad_is_u32(region.gen);
}

// Source offsets for PIXEL_UNPACK_BUFFER are in bytes, texture coordinates in words.
// Words [first, end) of the bound slab's unpack buffer into its array texture:
// per layer they cross, a partial head row, whole rows and a partial tail row.
// Pixel-unpack state is assumed at its defaults (UNPACK_ROW_LENGTH,
// UNPACK_IMAGE_HEIGHT and UNPACK_SKIP_* 0): any other uploader that changes it
// restores it.
function makepad_copy_instance_words(gl, first, end) {
  const row = MAKEPAD_INSTANCE_WORD_ROW;
  const put = (word, w, h) => {
    const at = word % MAKEPAD_INSTANCE_WINDOW_WORDS;
    gl.texSubImage3D(gl.TEXTURE_2D_ARRAY, 0, at % row, Math.floor(at / row), Math.floor(word / MAKEPAD_INSTANCE_WINDOW_WORDS),
      w, h, 1, gl.RED_INTEGER, gl.UNSIGNED_INT, word * 4);
  };
  while (first < end) {
    const stop = Math.min(end, (Math.floor(first / MAKEPAD_INSTANCE_WINDOW_WORDS) + 1) * MAKEPAD_INSTANCE_WINDOW_WORDS);
    const x = first % row;
    if (x) {
      const head = Math.min(row - x, stop - first);
      put(first, head, 1);
      first += head;
    }
    const rows = Math.floor((stop - first) / row);
    if (rows) {
      put(first, row, rows);
      first += rows * row;
    }
    if (first < stop) {
      put(first, stop - first, 1);
      first = stop;
    }
  }
}

function makepad_safe_product(left, right) {
  if (
    !Number.isSafeInteger(left) ||
    left < 0 ||
    !Number.isSafeInteger(right) ||
    right < 0
  ) {
    return null;
  }
  const product = left * right;
  return Number.isSafeInteger(product) ? product : null;
}

function makepad_safe_sum(left, right) {
  if (
    !Number.isSafeInteger(left) ||
    left < 0 ||
    !Number.isSafeInteger(right) ||
    right < 0
  ) {
    return null;
  }
  const sum = left + right;
  return Number.isSafeInteger(sum) ? sum : null;
}

function makepad_webgl_attrib_pointer(gl, attr, byte_offset) {
  const offset = byte_offset + attr.offset;
  if (attr.integer) {
    gl.vertexAttribIPointer(attr.loc, attr.size, attr.gl_type, attr.stride, offset);
  } else {
    gl.vertexAttribPointer(attr.loc, attr.size, attr.gl_type, !!attr.normalized, attr.stride, offset);
  }
}

function makepad_swap_red_blue(value) {
  return ((value & 0xff) << 16) | (value & 0xff00ff00) | ((value >>> 16) & 0xff);
}

export function makepad_query_webgl_limits(gl) {
  const max_texture_size = makepad_webgl_limit(
    gl.getParameter(gl.MAX_TEXTURE_SIZE),
  );
  const max_cube_map_texture_size = makepad_webgl_limit(
    gl.getParameter(gl.MAX_CUBE_MAP_TEXTURE_SIZE),
  );
  const max_renderbuffer_size = makepad_webgl_limit(
    gl.getParameter(gl.MAX_RENDERBUFFER_SIZE),
  );
  const viewport = gl.getParameter(gl.MAX_VIEWPORT_DIMS);
  const layers = Number(gl.getParameter(gl.MAX_ARRAY_TEXTURE_LAYERS));
  const max_viewport_width = makepad_webgl_limit(viewport && viewport[0]);
  const max_viewport_height = makepad_webgl_limit(viewport && viewport[1]);
  return {
    max_texture_size,
    max_cube_map_texture_size,
    max_vertex_texture_units: gl.getParameter(gl.MAX_VERTEX_TEXTURE_IMAGE_UNITS),
    max_fragment_texture_units: gl.getParameter(gl.MAX_TEXTURE_IMAGE_UNITS),
    max_combined_texture_units: gl.getParameter(gl.MAX_COMBINED_TEXTURE_IMAGE_UNITS),
    // WebGL 2's minimum when the context does not say
    max_array_texture_layers: Number.isSafeInteger(layers) && layers >= 1 ? layers : 256,
    max_width: Math.min(
      max_texture_size,
      max_renderbuffer_size,
      max_viewport_width,
    ),
    max_height: Math.min(
      max_texture_size,
      max_renderbuffer_size,
      max_viewport_height,
    ),
  };
}

// A render target is bounded by the device's limits only: the drawable's
// pixel budget (a page author's `renderpixelbudget`) sizes the canvas, never
// the app's own targets, which it sizes itself.
export function makepad_render_target_size(width, height, limits) {
  if (
    !Number.isSafeInteger(width) ||
    !Number.isSafeInteger(height) ||
    width <= 0 ||
    height <= 0
  ) {
    return { ok: false, reason: "dimensions must be positive safe integers" };
  }
  const size = makepad_compute_webgl_size(width, height, 1.0, limits);
  if (size.width <= 0 || size.height <= 0) {
    return { ok: false, reason: "dimensions cannot fit the WebGL limits" };
  }
  return {
    ok: true,
    requested_width: width,
    requested_height: height,
    width: size.width,
    height: size.height,
    scale: size.scale,
    scaled: size.width !== width || size.height !== height,
  };
}

export class WasmWebGL extends WasmWebBrowser {
  constructor(wasm, dispatch, canvas) {
    super(wasm, dispatch, canvas);
    if (wasm === undefined) {
      return;
    }
    this.uniform_stream_buf = null;
    this.zero_uniform_buf = null;
    this.uniform_stream_len = 0;
    this.uniform_stream_ptr = 0;
    this.bound_ubo = [];
    this.xr_pass_uniform_buf = null;
    this.xr_pass_words = null;
    this.draw_shaders = [];
    this.array_buffers = [];
    // Per instance slab slot: its buffer, written regions and lazy R32UI
    // mirror (texture-fetch shaders, `mp_inst_data` in shader_glsl.rs).
    this.instance_slabs = new Map();
    this.index_buffers = [];
    this.vaos = [];
    this.textures = [];
    this.framebuffers = [];
    this.active_render_target_textures = new Set();
    this.xr = undefined;
    this._missing_shader_ids = new Set();
    this._gl_error_reports = new Set();
    this.gl_checks = makepad_gl_checks_on();
    this._vertex_submission_reports = new Set();
    this._texture_upload_reports = new Set();
    this._invalid_texture_upload_ids = new Set();
    this._sampler_fallback_textures = new Map();
    this._sampler_fallback_texture_failures = new Set();
    this._webgl_shader_version = 0;
    this.pending_webgl_shader_count = 0;
    // FromWasmSpreadFirstDraws: a program's first draw goes in a frame of its
    // own (first_draw_this_frame), the others wait (first_draws_deferred).
    this.first_draw_spread = false;
    this.first_draw_this_frame = false;
    this.first_draws_deferred = false;
    this.webgl_shader_poll_frame_id = 0;
    this.webgl_shader_timeline_start = undefined;
    this.webgl_shader_batch_program_count = 0;
    this.webgl_shader_batch_failed_count = 0;
    this.webgl_shader_summary_timer = undefined;
    // @section video-playback
    this.video_players = {};
    // @end video-playback
    this.pending_render_texture_captures = new Set();
    this.bgra_upload_scratch = new Uint32Array(0);
    if (this.init_webgl_context()) {
      this.load_deps();
    }
  }

  // webGL API

  // @section xr
  on_xr_animation_frame(time, frame) {
    if (this.webgl_context_lost) {
      return;
    }
    function empty_transform() {
      return {
        orientation: {
          a: 0,
          b: 0,
          c: 0,
          d: 0,
        },
        position: {
          x: 0,
          y: 0,
          z: 0,
        },
      };
    }

    function to_transform(pose_transform, tgt) {
      let po = pose_transform.inverse.orientation;
      let pp = pose_transform.position;
      let o = tgt.orientation;
      o.a = po.x;
      o.b = po.y;
      o.c = po.z;
      o.d = po.w;
      let p = tgt.position;
      p.x = pp.x;
      p.y = pp.y;
      p.z = pp.z;
    }

    function get_matrices(layer, view, tgt) {
      tgt.view = view;
      tgt.viewport = layer.getViewport(view);
      tgt.projection_matrix = view.projectionMatrix;
      tgt.transform_matrix = view.transform.inverse.matrix;
      tgt.invtransform_matrix = view.transform.matrix;
      tgt.camera_pos = view.transform.inverse.position;
    }

    if (this.xr == undefined) {
      return;
    }

    let ref_space = this.xr.ref_space;
    let xr = this.xr;

    xr.session.requestAnimationFrame(this.xr.on_animation_frame);
    xr.pose = frame.getViewerPose(ref_space);

    let left_view = xr.pose.views[0];
    let right_view = xr.pose.views[1];

    get_matrices(xr.layer, xr.pose.views[0], xr.left_eye);
    get_matrices(xr.layer, xr.pose.views[1], xr.right_eye);

    if (xr.xr_update === undefined) {
      xr.xr_update = {
        time: 0,
        head_transform: empty_transform(),
        inputs: [],
      };
    }

    let xr_update = xr.xr_update;
    xr_update.time = time / 1000.0;

    to_transform(this.xr.pose.transform, xr_update.head_transform);

    let inputs = xr_update.inputs;
    for (let i = 0; i < inputs.length; i++) {
      inputs[i].active = false;
    }

    let input_sources = this.xr.session.inputSources;
    for (let i = 0; i < input_sources.length; i++) {
      if (inputs[i] === undefined) {
        inputs[i] = {
          active: false,
          grip: empty_transform(),
          ray: empty_transform(),
          hand: 0,
          buttons: [],
          axes: [],
        };
      }
      let input = inputs[i];
      let input_source = input_sources[i];

      let grip_pose = frame.getPose(input_source.gripSpace, ref_space);
      let ray_pose = frame.getPose(input_source.targetRaySpace, ref_space);

      if (grip_pose == null || ray_pose == null) {
        input.active = false;
        continue;
      }

      to_transform(grip_pose.transform, input.grip);
      to_transform(ray_pose.transform, input.ray);

      let buttons = input.buttons;
      let input_buttons = input_source.gamepad.buttons;
      for (let i = 0; i < input_buttons.length; i++) {
        if (buttons[i] === undefined) {
          buttons[i] = { pressed: 0, value: 0 };
        }
        buttons[i].pressed = input_buttons[i].pressed ? 1 : 0;
        buttons[i].value = input_buttons[i].value;
      }
      let axes = input.axes;
      let input_axes = input_source.gamepad.axes;
      for (let i = 0; i < input_axes.length; i++) {
        axes[i] = input_axes[i];
      }
    }

    this.to_wasm.ToWasmXRUpdate(xr_update);
    this.to_wasm.ToWasmAnimationFrame({ time: time / 1000.0, in_a_row: true });
    this.in_animation_frame = true;
    this.do_wasm_pump();
    this.in_animation_frame = false;
  }

  FromWasmXrStartPresenting(args) {
    if (this.webgl_context_lost || this.xr !== undefined) {
      return;
    }
    // alright lets fire up the xr stuff
    navigator.xr
      .requestSession("immersive-vr", { requiredFeatures: ["local-floor"] })
      .then((session) => {
        if (this.webgl_context_lost) {
          session.end();
          return;
        }
        let layer = new XRWebGLLayer(session, this.gl, {
          antialias: false,
          depth: true,
          stencil: false,
          ignoreDepthValues: false,
          framebufferScaleFactor: 1.5,
        });
        session.updateRenderState({ baseLayer: layer });
        session.requestReferenceSpace("local-floor").then((ref_space) => {
          if (this.webgl_context_lost) {
            session.end();
            return;
          }
          window.localStorage.setItem("xr_presenting", "true");
          this.xr = {
            left_eye: {},
            right_eye: {},
            layer,
            ref_space,
            session,
            on_animation_frame: (t, f) => this.on_xr_animation_frame(t, f),
          };
          session.requestAnimationFrame(this.xr.on_animation_frame);
          session.addEventListener("end", () => {
            window.localStorage.setItem("xr_presenting", "false");
            this.xr = undefined;
            this.FromWasmRequestAnimationFrame();
          });
        });
      });
  }

  FromWasmXrStopPresenting() {}
  // @end xr

  get_uniform_block_binding(program, name, binding, block_sizes) {
    const gl = this.gl;
    const index = gl.getUniformBlockIndex(program, name);
    if (index === gl.INVALID_INDEX) return null;
    if (binding >= (this.max_uniform_buffer_bindings || 24)) {
      this.report_vertex_submission_once("ubo-binding:" + binding, "uniform binding exceeds the device limit", { name, binding });
      return null;
    }
    gl.uniformBlockBinding(program, index, binding);
    if (block_sizes && typeof gl.getActiveUniformBlockParameter === "function") {
      block_sizes[binding] = gl.getActiveUniformBlockParameter(program, index, gl.UNIFORM_BLOCK_DATA_SIZE) || 0;
    }
    return binding;
  }

  FromWasmUploadUniformStream({ data }) {
    const gl = this.gl;
    this.uniform_stream_len = 0;
    this.uniform_stream_ptr = 0;
    const checked = this.make_validated_wasm_view(data, 1, true, "uniform stream", Uint8Array);
    if (!checked.ok) {
      this.report_vertex_submission_once("uniform-stream:upload", checked.reason, { data });
      return;
    }
    if (!this.uniform_stream_buf) this.uniform_stream_buf = gl.createBuffer();
    if (!this.uniform_stream_buf) {
      this.report_vertex_submission_once("uniform-stream:allocation", "uniform stream buffer allocation failed", {});
      return;
    }
    gl.bindBuffer(gl.UNIFORM_BUFFER, this.uniform_stream_buf);
    gl.bufferData(gl.UNIFORM_BUFFER, checked.array, gl.STREAM_DRAW);
    if (this.bound_ubo) this.bound_ubo.length = 0;
    // where the stream sits in wasm memory: a view is made at use, as memory may grow meanwhile
    this.uniform_stream_ptr = data.ptr;
    this.uniform_stream_len = data.len;
  }

  upload_uniform_buffer_data(gl, gl_buf, data, usage = gl.DYNAMIC_DRAW) {
    if (!gl_buf || !data || data.length == 0) {
      return;
    }
    gl.bindBuffer(gl.UNIFORM_BUFFER, gl_buf);
    this.upload_buffer_data(gl, gl.UNIFORM_BUFFER, gl_buf, data, usage);
    gl.bindBuffer(gl.UNIFORM_BUFFER, null);
  }

  upload_buffer_data(gl, target, gl_buf, data, usage) {
    const byte_length = data.byteLength || data.length * 4;
    if (gl_buf._buffer_byte_length !== byte_length) {
      gl.bufferData(target, data, usage);
      if (target === gl.UNIFORM_BUFFER) this.invalidate_ubo_buffer(gl_buf);
      gl_buf._buffer_byte_length = byte_length;
    } else {
      gl.bufferSubData(target, 0, data);
    }
  }

  bind_uniform_block(gl, binding, buf, off = 0, len = -1) {
    if (binding == null || !buf || len === 0) return;
    const cache = this.bound_ubo || (this.bound_ubo = []);
    const cur = cache[binding];
    if (cur && cur.buf === buf && cur.off === off && cur.len === len) return;
    if (cur) cur.buf = null;
    if (len > 0) {
      gl.bindBufferRange(gl.UNIFORM_BUFFER, binding, buf, off, len);
    } else {
      gl.bindBufferBase(gl.UNIFORM_BUFFER, binding, buf);
    }
    if (cur) {
      cur.buf = buf;
      cur.off = off;
      cur.len = len;
    } else {
      cache[binding] = { buf, off, len };
    }
  }

  invalidate_ubo_buffer(buf) {
    if (!buf || !this.bound_ubo) return;
    for (const entry of this.bound_ubo) {
      if (entry && entry.buf === buf) entry.buf = null;
    }
  }

  effective_uniform_align() {
    const raw = this.gpu_info?.uniform_buffer_offset_alignment || 256;
    const clamped = Math.min(256, Math.max(16, raw | 0));
    let align = 16;
    while (align < clamped) align <<= 1;
    return align;
  }

  effective_max_uniform_block() {
    const raw = this.gpu_info?.max_uniform_block_size || 65536;
    return Math.max(16384, raw | 0);
  }

  ensure_zero_uniform_buf(gl, need) {
    const max_block = this.effective_max_uniform_block();
    const target_size = Math.min(max_block, Math.max(256, need));
    if (this.zero_uniform_buf && (this.zero_uniform_buf._buffer_byte_length || 0) >= target_size) {
      return this.zero_uniform_buf;
    }
    if (!this.zero_uniform_buf) this.zero_uniform_buf = gl.createBuffer();
    if (!this.zero_uniform_buf) return null;
    let size = 256;
    while (size < target_size && size < max_block) size <<= 1;
    size = Math.min(size, max_block);
    gl.bindBuffer(gl.UNIFORM_BUFFER, this.zero_uniform_buf);
    gl.bufferData(gl.UNIFORM_BUFFER, new Uint8Array(size), gl.STATIC_DRAW);
    this.invalidate_ubo_buffer(this.zero_uniform_buf);
    this.zero_uniform_buf._buffer_byte_length = size;
    gl.bindBuffer(gl.UNIFORM_BUFFER, null);
    return this.zero_uniform_buf;
  }

  bind_stream_uniform(gl, shader, binding, range) {
    if (binding == null) return;
    if (range.len === 0) {
      const need = Math.max(shader.block_sizes?.[binding] || 0, 16);
      const zero = this.ensure_zero_uniform_buf(gl, need);
      if (!zero) throw new Error("zero uniform buffer allocation failed");
      this.bind_uniform_block(gl, binding, zero);
      return;
    }
    const align = this.effective_uniform_align();
    const max_block = this.effective_max_uniform_block();
    const stride = Math.min(Math.ceil(Math.max(range.len, 16) / align) * align, max_block);
    const len = Math.min(stride, this.uniform_stream_len - range.off);
    this.bind_uniform_block(gl, binding, this.uniform_stream_buf, range.off, len);
  }

  // With checks on (gl_checks), allocations are checked for a GL error
  // later, not where they happen: gl.getError() waits for the GPU process to
  // finish everything queued, which behind a busy GPU stalls this thread for
  // whole GPU frames (25-100 ms per allocating frame, every plate change and
  // resize). `undo` takes back what one allocation made (delete what it
  // created, invalidate what it re-specified) when a later check finds an
  // error; until then the resource is used as allocated (a failed texture
  // samples black, a failed buffer draws nothing). With checks off, nothing.
  defer_allocation_check(undo) {
    if (!this.gl_checks) {
      return;
    }
    const pending = this.pending_allocation_checks || (this.pending_allocation_checks = []);
    if (pending.length === 0) {
      this.pending_allocation_since = performance.now();
    }
    pending.push(undo);
  }

  // One getError for every allocation since the last check: when `idle`
  // (no earlier frame on the GPU, so it is cheap), when the oldest is
  // MAKEPAD_WEBGL_ALLOCATION_CHECK_MS old, or when `force`d. On an error
  // every allocation since the last clean check is undone (fail closed: the
  // error cannot say which one failed). Returns the error read (0 if none
  // or not checked).
  check_allocations(force, idle) {
    const pending = this.pending_allocation_checks;
    if (!pending || pending.length === 0) {
      return 0;
    }
    if (this.webgl_context_lost) {
      this.pending_allocation_checks = [];
      return 0;
    }
    if (!force && !idle && performance.now() - this.pending_allocation_since < MAKEPAD_WEBGL_ALLOCATION_CHECK_MS) {
      return 0;
    }
    const gl = this.gl;
    this.pending_allocation_checks = [];
    const error = gl.getError();
    if (error === gl.NO_ERROR) {
      return 0;
    }
    const key = "allocation:" + error;
    const reports = this._gl_error_reports || (this._gl_error_reports = new Set());
    if (!reports.has(key)) {
      reports.add(key);
      const message = `WebGL2 error ${error} after ${pending.length} allocation(s): they are undone`;
      console.error(message);
      if (typeof window !== "undefined" && typeof window.makepad_report_browser_issue === "function") {
        window.makepad_report_browser_issue("webgl.error", { where: "allocation", error, message });
      }
    }
    for (const undo of pending) {
      try {
        undo(error);
      } catch (_error) {
      }
    }
    return error;
  }

  report_render_target_size_once(kind, detail) {
    const reports = this._render_target_size_reports ||
      (this._render_target_size_reports = new Set());
    if (reports.has(kind)) {
      return;
    }
    reports.add(kind);
    const message = `makepad: WebGL render target ${kind}`;
    if (kind === "scaled to safety limits") {
      console.warn(message, detail);
    } else {
      console.error(message, detail);
    }
  }

  reset_active_render_target_textures() {
    if (!this.active_render_target_textures) {
      this.active_render_target_textures = new Set();
      return;
    }
    this.active_render_target_textures.clear();
  }

  set_active_render_target_textures(color_attachments, depth_attachment) {
    this.reset_active_render_target_textures();
    for (const texture of color_attachments) {
      this.active_render_target_textures.add(texture);
    }
    if (depth_attachment) {
      this.active_render_target_textures.add(depth_attachment);
    }
  }

  reject_render_target(reason, args) {
    this.reset_active_render_target_textures();
    this.render_target_rejected = true;
    if (args && Array.isArray(args.color_targets)) {
      for (const target of args.color_targets) {
        const texture = target && this.textures && this.textures[target.texture_id];
        if (texture) {
          texture._render_target_valid = false;
        }
      }
    }
    if (args && args.depth_target && args.depth_target.attached) {
      const texture = this.textures && this.textures[args.depth_target.texture_id];
      if (texture) {
        texture._render_target_valid = false;
      }
    }
    this.report_render_target_size_once("rejected", {
      reason,
      width: args && args.width,
      height: args && args.height,
      limits: this.webgl_limits,
    });
    if (!this.webgl_context_lost && this.gl) {
      try {
        this.gl.bindFramebuffer(this.gl.FRAMEBUFFER, null);
        this.gl.viewport(0, 0, 0, 0);
      } catch (_error) {
      }
    }
  }

  report_vertex_submission_once(key, reason, detail) {
    if (this.gl?.isContextLost?.()) throw new Error("WebGL context lost during submission");
    const reports = this._vertex_submission_reports ||
      (this._vertex_submission_reports = new Set());
    if (reports.has(key) || reports.size >= MAKEPAD_WEBGL_MAX_SUBMISSION_REPORTS) {
      return;
    }
    reports.add(key);
    console.error(`makepad: WebGL vertex submission rejected: ${reason}`, detail);
  }

  numeric_buffer_for_update(table, buffer_id, kind) {
    if (!Number.isSafeInteger(buffer_id) || buffer_id < 0) {
      this.report_vertex_submission_once(
        `${kind}:invalid-id`,
        `${kind} buffer id is not a nonnegative safe integer`,
        { buffer_id },
      );
      return null;
    }
    let buffer = table[buffer_id];
    if (!buffer) {
      buffer = table[buffer_id] = {
        gl_buf: null,
        upload_version: 0,
      };
    }
    buffer.upload_version = (buffer.upload_version || 0) + 1;
    buffer.valid = false;
    buffer.byte_length = 0;
    buffer.length = 0;
    if (kind === "index") {
      buffer.index_type = null;
      buffer.index_width = 0;
      buffer.max_index = -1;
    } else {
      buffer.source_kind = null;
    }
    return buffer;
  }

  validate_wasm_slice(slice, element_size, length_is_bytes, label) {
    if (!slice || !Number.isSafeInteger(slice.ptr) || slice.ptr < 0) {
      return { ok: false, reason: `${label} pointer is invalid` };
    }
    if (!Number.isSafeInteger(slice.len) || slice.len < 0) {
      return { ok: false, reason: `${label} length is invalid` };
    }
    if (slice.ptr % element_size !== 0) {
      return { ok: false, reason: `${label} pointer is unaligned` };
    }
    const byte_length = length_is_bytes
      ? slice.len
      : makepad_safe_product(slice.len, element_size);
    if (byte_length === null || byte_length % element_size !== 0) {
      return { ok: false, reason: `${label} byte length is invalid` };
    }
    const end = makepad_safe_sum(slice.ptr, byte_length);
    const memory = this.memory && this.memory.buffer;
    if (
      (byte_length > 0 && slice.ptr === 0) ||
      end === null ||
      !memory ||
      !Number.isSafeInteger(memory.byteLength) ||
      end > memory.byteLength
    ) {
      return { ok: false, reason: `${label} is outside Wasm memory` };
    }
    return {
      ok: true,
      byte_length,
      element_count: byte_length / element_size,
      memory,
    };
  }

  make_validated_wasm_view(slice, element_size, length_is_bytes, label, Type) {
    const checked = this.validate_wasm_slice(
      slice,
      element_size,
      length_is_bytes,
      label,
    );
    if (!checked.ok) {
      return checked;
    }
    try {
      return {
        ...checked,
        array: new Type(checked.memory, slice.ptr, checked.element_count),
      };
    } catch (error) {
      return {
        ok: false,
        reason: `${label} view could not be created: ${error && error.message ? error.message : String(error)}`,
      };
    }
  }

  report_texture_upload_once(fault_class, reason, detail) {
    const reports = this._texture_upload_reports ||
      (this._texture_upload_reports = new Set());
    if (
      reports.has(fault_class) ||
      reports.size >= MAKEPAD_WEBGL_MAX_SUBMISSION_REPORTS
    ) {
      return;
    }
    reports.add(fault_class);
    console.error(`makepad: WebGL texture upload rejected: ${reason}`, detail);
  }

  clear_texture_upload_allocation(texture) {
    if (!texture) {
      return;
    }
    texture._texture_upload_width = undefined;
    texture._texture_upload_height = undefined;
    texture._texture_upload_format = undefined;
  }

  clear_render_target_allocation(texture) {
    if (!texture) {
      return;
    }
    texture._width = undefined;
    texture._height = undefined;
    texture._requested_width = undefined;
    texture._requested_height = undefined;
    texture._format = undefined;
    texture._depth = undefined;
  }

  invalidate_texture_dependencies(texture) {
    if (!texture) {
      return;
    }
    texture._render_target_valid = false;
    this.clear_render_target_allocation(texture);
    for (const framebuffer of this.framebuffers || []) {
      if (
        framebuffer &&
        ((Array.isArray(framebuffer._color_attachments) &&
          framebuffer._color_attachments.includes(texture)) ||
          framebuffer._depth_attachment === texture)
      ) {
        // The GL attachment is repaired by the next render-pass begin. Drop
        // the identity cache now so it cannot conceal a replaced object.
        framebuffer._color_attachments = undefined;
        framebuffer._depth_attachment = undefined;
      }
    }
    if (
      this.active_render_target_textures &&
      this.active_render_target_textures.has(texture)
    ) {
      this.active_render_target_textures.clear();
      this.render_target_rejected = true;
    }
  }

  reject_texture_upload(args, fault_class, reason, detail = {}) {
    const texture_id = args && args.texture_id;
    if (Number.isSafeInteger(texture_id) && texture_id >= 0) {
      const texture = this.textures && this.textures[texture_id];
      this.invalidate_texture_dependencies(texture);
      const invalid_ids = this._invalid_texture_upload_ids ||
        (this._invalid_texture_upload_ids = new Set());
      invalid_ids.add(texture_id);
    }
    this.report_texture_upload_once(fault_class, reason, {
      texture_id,
      width: args && args.width,
      height: args && args.height,
      ...detail,
    });
    return null;
  }

  admit_texture_upload(args, options) {
    if (!args || !Number.isSafeInteger(args.texture_id) || args.texture_id < 0) {
      return this.reject_texture_upload(
        args,
        "invalid-id",
        "texture id is not a nonnegative safe integer",
      );
    }
    if (
      !Number.isSafeInteger(args.width) ||
      !Number.isSafeInteger(args.height) ||
      args.width <= 0 ||
      args.height <= 0
    ) {
      return this.reject_texture_upload(
        args,
        "invalid-dimensions",
        "texture dimensions must be positive safe integers",
      );
    }
    if (options.faces === 6 && !options.array && args.width !== args.height) {
      return this.reject_texture_upload(
        args,
        "non-square-cube",
        "cube texture faces must be square",
      );
    }

    const limits = this.webgl_limits || {};
    const max_dimension = makepad_webgl_limit(
      options.faces === 6 && !options.array
        ? limits.max_cube_map_texture_size
        : limits.max_texture_size,
    );
    if (args.width > max_dimension || args.height > max_dimension) {
      return this.reject_texture_upload(
        args,
        "device-dimension-limit",
        "texture dimensions exceed the cached WebGL device limit",
        { max_dimension },
      );
    }

    const face_texels = makepad_safe_product(args.width, args.height);
    const texels = face_texels === null
      ? null
      : makepad_safe_product(face_texels, options.faces);
    const allocation_bytes = texels === null
      ? null
      : makepad_safe_product(texels, options.bytes_per_texel);
    const required_elements = texels === null
      ? null
      : makepad_safe_product(texels, options.elements_per_texel);
    if (
      face_texels === null ||
      texels === null ||
      allocation_bytes === null ||
      required_elements === null
    ) {
      return this.reject_texture_upload(
        args,
        "unsafe-size",
        "texture size arithmetic is unsafe",
      );
    }

    const data = args.data;
    if (!data || !Number.isSafeInteger(data.ptr) || data.ptr < 0) {
      return this.reject_texture_upload(
        args,
        "invalid-source-pointer",
        "texture source pointer is invalid",
      );
    }
    if (!Number.isSafeInteger(data.len) || data.len < 0) {
      return this.reject_texture_upload(
        args,
        "invalid-source-length",
        "texture source length is invalid",
      );
    }
    if (data.ptr % options.element_size !== 0) {
      return this.reject_texture_upload(
        args,
        "unaligned-source",
        "texture source pointer is unaligned",
      );
    }
    if (data.len < required_elements) {
      return this.reject_texture_upload(
        args,
        "short-source",
        "texture source is shorter than the full texel allocation",
        { required_elements, declared_elements: data.len },
      );
    }
    const declared_bytes = makepad_safe_product(data.len, options.element_size);
    const end = declared_bytes === null
      ? null
      : makepad_safe_sum(data.ptr, declared_bytes);
    const memory = this.memory && this.memory.buffer;
    if (
      data.ptr === 0 ||
      declared_bytes === null ||
      end === null ||
      !memory ||
      !Number.isSafeInteger(memory.byteLength) ||
      end > memory.byteLength
    ) {
      return this.reject_texture_upload(
        args,
        "source-memory-range",
        "texture source is outside Wasm memory",
      );
    }
    return {
      args,
      options,
      memory,
      face_texels,
      texels,
      allocation_bytes,
      required_elements,
    };
  }

  make_texture_source_view(admission, Type) {
    try {
      return new Type(
        admission.memory,
        admission.args.data.ptr,
        admission.required_elements,
      );
    } catch (error) {
      return this.reject_texture_upload(
        admission.args,
        "source-view",
        `texture source view could not be created: ${error && error.message ? error.message : String(error)}`,
      );
    }
  }

  get_bgra_upload_scratch(count) {
    if (
      !(this.bgra_upload_scratch instanceof Uint32Array) ||
      this.bgra_upload_scratch.length < count ||
      // Not kept at more than twice what uploads need.
      this.bgra_upload_scratch.length > count * 2
    ) {
      this.bgra_upload_scratch = new Uint32Array(count);
    }
    return this.bgra_upload_scratch;
  }

  make_bgra_upload_view(admission) {
    const source = this.make_texture_source_view(admission, Uint32Array);
    if (!source) {
      return null;
    }
    try {
      const converted = this.get_bgra_upload_scratch(admission.required_elements);
      for (let i = 0; i < admission.required_elements; i++) {
        converted[i] = makepad_swap_red_blue(source[i]);
      }
      return new Uint8Array(
        converted.buffer,
        0,
        admission.required_elements * 4,
      );
    } catch (error) {
      return this.reject_texture_upload(
        admission.args,
        "bgra-conversion",
        `BGRA texture conversion failed: ${error && error.message ? error.message : String(error)}`,
      );
    }
  }

  // The texture target a shader sampler type binds.
  sampler_target(ty) {
    const gl = this.gl;
    if (ty === "samplerCube") return gl.TEXTURE_CUBE_MAP;
    if (ty === "sampler3D") return gl.TEXTURE_3D;
    if (ty === "sampler2DArray") return gl.TEXTURE_2D_ARRAY;
    return gl.TEXTURE_2D;
  }

  // All current mutable uploads supply level zero, including images with CPU-side mips.
  set_texture_storage_info(target, texture, filterable, levels = 1) {
    this.gl.texParameteri(target, this.gl.TEXTURE_MAX_LEVEL, levels - 1);
    texture._filterable = !!filterable;
  }

  configure_texture_parameters(gl, target, nearest, cube) {
    const filter = nearest ? gl.NEAREST : gl.LINEAR;
    gl.texParameteri(target, gl.TEXTURE_MAG_FILTER, filter);
    gl.texParameteri(target, gl.TEXTURE_MIN_FILTER, filter);
    gl.texParameteri(target, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(target, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    if (cube) {
      gl.texParameteri(target, gl.TEXTURE_WRAP_R, gl.CLAMP_TO_EDGE);
    }
  }

  // A 1x1 texture for a sampler the draw gave none: opaque black RGBA, or
  // for a shadow sampler (`shadow`) a depth texture at the far plane (a
  // shadow sampler reading a colour texture is a GL_INVALID_OPERATION that
  // drops the whole draw), so nothing is shadowed.
  get_sampler_fallback_texture(target, shadow = false) {
    const gl = this.gl;
    if (!gl || this.webgl_context_lost) {
      return null;
    }
    const key = shadow ? "shadow" : target;
    const textures = this._sampler_fallback_textures ||
      (this._sampler_fallback_textures = new Map());
    if (textures.has(key)) {
      return textures.get(key);
    }
    const failures = this._sampler_fallback_texture_failures ||
      (this._sampler_fallback_texture_failures = new Set());
    if (failures.has(key)) {
      return null;
    }

    let texture = null;
    try {
      texture = gl.createTexture();
      if (!texture) {
        throw new Error("WebGL sampler fallback allocation returned null");
      }
      const cube = target === gl.TEXTURE_CUBE_MAP;
      gl.bindTexture(target, texture);
      this.set_texture_storage_info(target, texture, !shadow);
      texture._depth = shadow;
      this.configure_texture_parameters(gl, target, false, cube);
      const pixel = new Uint8Array(4);
      const faces = cube
        ? [
            gl.TEXTURE_CUBE_MAP_POSITIVE_X,
            gl.TEXTURE_CUBE_MAP_NEGATIVE_X,
            gl.TEXTURE_CUBE_MAP_POSITIVE_Y,
            gl.TEXTURE_CUBE_MAP_NEGATIVE_Y,
            gl.TEXTURE_CUBE_MAP_POSITIVE_Z,
            gl.TEXTURE_CUBE_MAP_NEGATIVE_Z,
          ]
        : [gl.TEXTURE_2D];
      if (target === gl.TEXTURE_2D_ARRAY || target === gl.TEXTURE_3D) {
        gl.texImage3D(target, 0, gl.RGBA, 1, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
      } else if (shadow) {
        gl.texImage2D(
          gl.TEXTURE_2D,
          0,
          gl.DEPTH_COMPONENT32F,
          1,
          1,
          0,
          gl.DEPTH_COMPONENT,
          gl.FLOAT,
          new Float32Array([1]),
        );
      } else {
        for (const face of faces) {
          gl.texImage2D(
            face,
            0,
            gl.RGBA,
            1,
            1,
            0,
            gl.RGBA,
            gl.UNSIGNED_BYTE,
            pixel,
          );
        }
      }
      const allocation_error = this.gl_checks ? gl.getError() : gl.NO_ERROR;
      if (allocation_error !== gl.NO_ERROR) {
        throw new Error(`WebGL sampler fallback allocation error ${allocation_error}`);
      }
    } catch (_error) {
      failures.add(key);
      if (texture) {
        try {
          gl.deleteTexture(texture);
        } catch (_delete_error) {
        }
      }
      return null;
    }

    texture._texture_target = target;
    texture._render_target_valid = true;
    textures.set(key, texture);
    return texture;
  }

  // The texture's storage holds a valid `upload_format` image of this size,
  // so new data can go into it without allocating.
  can_update_texture_in_place(texture, width, height, upload_format) {
    return (
      texture._texture_upload_width === width &&
      texture._texture_upload_height === height &&
      texture._texture_upload_format === upload_format &&
      texture._width === undefined &&
      texture._height === undefined &&
      texture._format === undefined &&
      texture._depth === undefined &&
      texture._render_target_valid !== false
    );
  }

  upload_admitted_texture(admission, upload_format, source, upload) {
    const gl = this.gl;
    const args = admission.args;
    const target = admission.options.array
      ? gl.TEXTURE_2D_ARRAY
      : admission.options.faces === 6
        ? gl.TEXTURE_CUBE_MAP
        : gl.TEXTURE_2D;
    const old_texture = this.textures[args.texture_id];
    const replace = !!old_texture && old_texture._texture_target !== target;
    let texture = replace ? null : old_texture;
    let created = false;

    if (!texture) {
      try {
        texture = gl.createTexture();
      } catch (error) {
        return this.reject_texture_upload(
          args,
          "create-texture-exception",
          `WebGL texture allocation threw: ${error && error.message ? error.message : String(error)}`,
        );
      }
      if (!texture) {
        return this.reject_texture_upload(
          args,
          "create-texture-null",
          "WebGL texture allocation returned null",
        );
      }
      created = true;
    }

    const allocation_changed =
      created ||
      !this.can_update_texture_in_place(texture, args.width, args.height, upload_format);
    if (allocation_changed && old_texture) {
      this.invalidate_texture_dependencies(old_texture);
    }
    texture._render_target_valid = false;
    try {
      gl.bindTexture(target, texture);
      this.set_texture_storage_info(target, texture, upload_format !== "rgba32f" || this.float_linear);
      if (allocation_changed) {
        this.configure_texture_parameters(
          gl,
          target,
          admission.options.nearest,
          admission.options.faces === 6 && !admission.options.array,
        );
      }
      upload(gl, target, allocation_changed, source, admission);
      if (allocation_changed) {
        this.defer_allocation_check((allocation_error) => {
          const current = this.textures[args.texture_id] === texture;
          if (created && current) {
            try {
              gl.deleteTexture(texture);
            } catch (_delete_error) {
            }
            this.textures[args.texture_id] = undefined;
          }
          this.invalidate_texture_dependencies(texture);
          if (current) {
            this.reject_texture_upload(args, "gl-upload", `WebGL texture upload failed: WebGL texture allocation error ${allocation_error}`);
          }
        });
      }
    } catch (error) {
      if (created) {
        try {
          gl.deleteTexture(texture);
        } catch (_delete_error) {
        }
      }
      this.invalidate_texture_dependencies(old_texture);
      return this.reject_texture_upload(
        args,
        "gl-upload",
        `WebGL texture upload failed: ${error && error.message ? error.message : String(error)}`,
      );
    }

    texture._texture_target = target;
    texture._texture_upload_width = args.width;
    texture._texture_upload_height = args.height;
    texture._texture_upload_format = upload_format;
    this.clear_render_target_allocation(texture);
    texture._render_target_valid = true;
    this.textures[args.texture_id] = texture;
    if (this._invalid_texture_upload_ids) {
      this._invalid_texture_upload_ids.delete(args.texture_id);
    }

    if (replace) {
      // A WebGLTexture cannot change its first-bound target. Publishing only
      // after success keeps the old object available for invalidation, while
      // deleting it prevents an attached FBO from retaining stale storage.
      this.invalidate_texture_dependencies(old_texture);
      try {
        gl.deleteTexture(old_texture);
      } catch (error) {
        this.report_texture_upload_once(
          "delete-replaced-texture",
          `replaced WebGL texture cleanup failed: ${error && error.message ? error.message : String(error)}`,
          { texture_id: args.texture_id },
        );
      }
    }
    return texture;
  }

  upload_numeric_buffer(gl, target, buffer, array) {
    let bound = false;
    try {
      if (!buffer.gl_buf) {
        buffer.gl_buf = gl.createBuffer();
      }
      if (!buffer.gl_buf) {
        return { ok: false, reason: "WebGL buffer allocation returned null" };
      }
      gl.bindBuffer(target, buffer.gl_buf);
      bound = true;
      // Storage grows to the next power of two and shrinks below a quarter
      // (web_gl.rs charges the upload budget the same way): data that changes
      // size every frame (kernel geometry, animated instances) updates in place
      // instead of re-specifying the buffer each frame. Draws read the data's
      // own length (buffer.byte_length), never the storage's.
      const previous_byte_length = buffer.gl_buf._buffer_byte_length;
      const needed = array.byteLength;
      let capacity = 256;
      while (capacity < needed) capacity *= 2;
      const allocation_changed = previous_byte_length === undefined
        || needed > previous_byte_length
        || needed * 4 < previous_byte_length && capacity !== previous_byte_length;
      if (allocation_changed) {
        gl.bufferData(target, capacity, gl.DYNAMIC_DRAW);
        buffer.gl_buf._buffer_byte_length = capacity;
      }
      if (needed !== 0) {
        gl.bufferSubData(target, 0, array);
      }
      if (allocation_changed) {
        const gl_buf = buffer.gl_buf;
        this.defer_allocation_check((error) => {
          gl_buf._buffer_byte_length = undefined;
          if (buffer.gl_buf === gl_buf) {
            buffer.valid = false;
          }
          this.report_vertex_submission_once(`buffer-allocation:${error}`, `WebGL buffer upload error ${error}`, {});
        });
      }
      return { ok: true };
    } catch (error) {
      if (buffer.gl_buf) {
        buffer.gl_buf._buffer_byte_length = undefined;
      }
      return {
        ok: false,
        reason: `WebGL buffer upload failed: ${error && error.message ? error.message : String(error)}`,
      };
    } finally {
      if (bound) {
        try {
          gl.bindBuffer(target, null);
        } catch (_error) {
        }
      }
    }
  }

  report_missing_shader_once(where, shader_id, vao_id) {
    if (this._missing_shader_ids.has(shader_id)) {
      return;
    }
    this._missing_shader_ids.add(shader_id);
    console.error("Missing shader in " + where, shader_id, vao_id);
  }

  webgl_type_from_code(code) {
    switch (code) {
      case 0:
        return this.gl.FLOAT;
      case 1:
        return this.gl.HALF_FLOAT;
      case 2:
        return this.gl.UNSIGNED_SHORT;
      case 3:
        return this.gl.SHORT;
      case 4:
        return this.gl.UNSIGNED_BYTE;
      case 5:
        return this.gl.BYTE;
      case 6:
        return this.gl.UNSIGNED_INT;
      case 7:
        return this.gl.INT;
      default:
        return null;
    }
  }

  webgl_attrib_locations(program, base, slots) {
    let attrib_locs = [];
    if (!Number.isSafeInteger(slots) || slots < 0 || slots * 4 > 255) {
      attrib_locs.push({
        invalid_reason: `legacy ${base} slot count ${slots} has an invalid stride`,
      });
      return attrib_locs;
    }
    let attribs = slots >> 2;
    if ((slots & 3) != 0) attribs++;
    for (let i = 0; i < attribs; i++) {
      let size = slots - i * 4;
      if (size > 4) size = 4;
      attrib_locs.push({
        loc: this.gl.getAttribLocation(program, base + i),
        offset: i * 16,
        size: size,
        stride: slots * 4,
        integer: false,
        normalized: false,
        gl_type: this.gl.FLOAT,
        type_code: 0,
      });
    }
    return attrib_locs;
  }

  webgl_typed_attrib_locations(program, table) {
    let attrib_locs = [];
    if (!Array.isArray(table)) {
      attrib_locs.push({ invalid_reason: "typed attribute table is not an array" });
      return attrib_locs;
    }
    for (let i = 0; i < table.length; i++) {
      let attrib = table[i];
      if (!attrib || typeof attrib.name !== "string") {
        attrib_locs.push({ invalid_reason: `attribute ${i} has no valid name` });
        continue;
      }
      attrib_locs.push({
        loc: this.gl.getAttribLocation(program, attrib.name),
        offset: attrib.offset,
        size: attrib.size,
        stride: attrib.stride,
        integer: !!attrib.integer,
        normalized: !!attrib.normalized,
        gl_type: this.webgl_type_from_code(attrib.gl_type),
        type_code: attrib.gl_type,
      });
    }
    return attrib_locs;
  }

  get_max_vertex_attribs() {
    if (this.max_vertex_attribs === undefined) {
      this.max_vertex_attribs = makepad_webgl_vertex_attrib_limit(
        this.gl.getParameter(this.gl.MAX_VERTEX_ATTRIBS),
      );
    }
    return this.max_vertex_attribs;
  }

  validate_webgl_attrib_layout(shader) {
    if (shader.attrib_layout) {
      return shader.attrib_layout;
    }
    const locations = new Set();
    const validate_group = (attribs, label) => {
      if (!Array.isArray(attribs)) {
        return { ok: false, reason: `${label} attributes are missing` };
      }
      let stride = null;
      const active = [];
      for (let i = 0; i < attribs.length; i++) {
        const attr = attribs[i];
        if (attr && attr.invalid_reason) {
          return { ok: false, reason: attr.invalid_reason };
        }
        if (!attr) {
          return { ok: false, reason: `${label} attribute ${i} is missing` };
        }
        if (!Number.isSafeInteger(attr.size) || attr.size < 1 || attr.size > 4) {
          return { ok: false, reason: `${label} attribute ${i} has invalid component count` };
        }
        const type = attr.type_code;
        const element_size = type === 0 || type === 6 || type === 7
          ? 4
          : type === 1 || type === 2 || type === 3
            ? 2
            : type === 4 || type === 5
              ? 1
              : 0;
        if (element_size === 0 || attr.gl_type === null || attr.gl_type === undefined) {
          return { ok: false, reason: `${label} attribute ${i} has unsupported type ${type}` };
        }
        if (attr.integer && (type === 0 || type === 1)) {
          return { ok: false, reason: `${label} integer attribute ${i} uses a floating-point type` };
        }
        if (attr.integer && attr.normalized) {
          return { ok: false, reason: `${label} integer attribute ${i} cannot be normalized` };
        }
        if (
          !Number.isSafeInteger(attr.stride) ||
          attr.stride <= 0 ||
          attr.stride > 255
        ) {
          return { ok: false, reason: `${label} attribute ${i} has invalid stride ${attr.stride}` };
        }
        if (stride === null) {
          stride = attr.stride;
        } else if (stride !== attr.stride) {
          return { ok: false, reason: `${label} attributes disagree on record stride` };
        }
        if (!Number.isSafeInteger(attr.offset) || attr.offset < 0) {
          return { ok: false, reason: `${label} attribute ${i} has invalid offset` };
        }
        if (attr.offset % element_size !== 0 || attr.stride % element_size !== 0) {
          return { ok: false, reason: `${label} attribute ${i} is unaligned` };
        }
        const span = makepad_safe_product(attr.size, element_size);
        const end = span === null ? null : makepad_safe_sum(attr.offset, span);
        if (end === null || end > attr.stride) {
          return { ok: false, reason: `${label} attribute ${i} extends past its record` };
        }
        if (!Number.isSafeInteger(attr.loc) || attr.loc < -1) {
          return { ok: false, reason: `${label} attribute ${i} has invalid location` };
        }
        if (attr.loc === -1) {
          continue;
        }
        if (attr.loc >= this.get_max_vertex_attribs()) {
          return { ok: false, reason: `${label} attribute ${i} exceeds MAX_VERTEX_ATTRIBS` };
        }
        if (locations.has(attr.loc)) {
          return { ok: false, reason: `duplicate active attribute location ${attr.loc}` };
        }
        locations.add(attr.loc);
        active.push({ ...attr, element_size, span, end });
      }
      return { ok: true, stride: stride === null ? 0 : stride, active };
    };

    const geometry = validate_group(shader.geom_attribs, "geometry");
    if (!geometry.ok) {
      shader.attrib_layout = geometry;
      return geometry;
    }
    const instance = validate_group(shader.inst_attribs, "instance");
    if (!instance.ok) {
      shader.attrib_layout = instance;
      return instance;
    }
    shader.attrib_layout = { ok: true, geometry, instance };
    return shader.attrib_layout;
  }

  schedule_webgl_shader_summary() {
    if (
      this.pending_webgl_shader_count != 0 ||
      this.webgl_shader_timeline_start === undefined ||
      this.webgl_shader_summary_timer !== undefined
    ) {
      return;
    }
    this.webgl_shader_summary_timer = setTimeout(() => {
      this.webgl_shader_summary_timer = undefined;
      if (this.pending_webgl_shader_count != 0) {
        return;
      }
      if (this.webgl_shader_batch_failed_count > 0) {
        console.error(
          `webgl shaders: ${this.webgl_shader_batch_program_count} programs, ${this.webgl_shader_batch_failed_count} failed, ${(performance.now() - this.webgl_shader_timeline_start).toFixed(1)} ms`,
        );
      }
      this.webgl_shader_timeline_start = undefined;
      this.webgl_shader_batch_program_count = 0;
      this.webgl_shader_batch_failed_count = 0;
    }, 0);
  }

  // WColorTarget codes: 1=R32F, 2=RGBA32F, 3=RGBA16F, 4=R16F.
  add_blend_error(format) {
    if (format >= 1 && format <= 4 && !this.ext_color_buffer_float) {
      return "blend_op: @Add on a float target requires EXT_color_buffer_float";
    }
    if ((format === 1 || format === 2) && !this.ext_float_blend) {
      return "blend_op: @Add on a 32-bit float target requires EXT_float_blend";
    }
    return null;
  }

  make_webgl_sampler(address, compare, min_filter, mag_filter, anisotropy) {
    const gl = this.gl;
    const sampler = gl.createSampler();
    if (!sampler) return null;
    const wrap = [gl.REPEAT, gl.CLAMP_TO_EDGE, gl.CLAMP_TO_EDGE, gl.MIRRORED_REPEAT][address];
    for (const axis of [gl.TEXTURE_WRAP_S, gl.TEXTURE_WRAP_T, gl.TEXTURE_WRAP_R]) {
      gl.samplerParameteri(sampler, axis, wrap);
    }
    gl.samplerParameteri(sampler, gl.TEXTURE_MIN_FILTER, min_filter);
    gl.samplerParameteri(sampler, gl.TEXTURE_MAG_FILTER, mag_filter);
    if (compare) {
      gl.samplerParameteri(sampler, gl.TEXTURE_COMPARE_MODE, gl.COMPARE_REF_TO_TEXTURE);
      gl.samplerParameteri(sampler, gl.TEXTURE_COMPARE_FUNC, gl.LEQUAL);
    }
    if (this.anisotropy_ext && anisotropy > 1) {
      gl.samplerParameterf(sampler, this.anisotropy_ext.TEXTURE_MAX_ANISOTROPY_EXT,
        Math.min(anisotropy, this.max_anisotropy));
    }
    return sampler;
  }

  texture_unit_error(textures, instance_texture) {
    const inst_data = Number(instance_texture);
    let vertex = inst_data, fragment = 0;
    const slots = new Set();
    for (const binding of textures) {
      slots.add(binding.slot);
      if (binding.stages & 1) vertex++;
      if (binding.stages & 2) fragment++;
    }
    const combined = textures.length + inst_data;
    const caps = this.webgl_limits;
    if (vertex <= caps.max_vertex_texture_units && fragment <= caps.max_fragment_texture_units &&
        combined <= caps.max_combined_texture_units) return null;
    return `texture unit limit exceeded: need vertex=${vertex} (max ${caps.max_vertex_texture_units}), ` +
      `fragment=${fragment} (max ${caps.max_fragment_texture_units}), combined=${combined} (max ${caps.max_combined_texture_units}) ` +
      `[logical_textures=${slots.size}, extra_sampler_pairs=${textures.length - slots.size}, pick_twins=0, inst_data=${inst_data}]`;
  }

  fail_webgl_shader(shader, stage, info_log) {
    if (shader.compile_failed) return;
    let gl = this.gl;
    console.error(
      `webgl.compile_fail.${stage} shader ${shader.shader_name || shader.shader_id} (${shader.shader_id}): ${info_log}`,
    );
    if (shader.vsh) gl.deleteShader(shader.vsh);
    if (shader.fsh) gl.deleteShader(shader.fsh);
    for (const binding of shader.texture_locs || []) {
      if (binding.sampler) gl.deleteSampler(binding.sampler);
    }
    if (shader.program) gl.deleteProgram(shader.program);
    for (const entry of shader.custom_uniform_buffers || []) {
      if (entry && entry.buffer) {
        this.invalidate_ubo_buffer(entry.buffer);
        gl.deleteBuffer(entry.buffer);
      }
    }
    shader.compile_failed = true;
    this.draw_shaders[shader.shader_id] = { compile_failed: true };
    this.pending_webgl_shader_count -= shader.pending ? 1 : 0;
    shader.pending = false;
    if (!shader.linked) {
      this.webgl_shader_batch_failed_count++;
      this.to_wasm.ToWasmWebGLShadersDone({ count: 1, waited: 0 });
      this.schedule_webgl_shader_summary();
    } else if (shader.first_draw_waiting) {
      shader.first_draw_waiting = false;
      this.to_wasm.ToWasmWebGLShadersDone({ count: 1, waited: 1 });
    }
  }

  finish_webgl_shader(shader) {
    let gl = this.gl;
    let status_started = performance.now();
    let vertex_ok = gl.getShaderParameter(shader.vsh, gl.COMPILE_STATUS);
    let fragment_ok = gl.getShaderParameter(shader.fsh, gl.COMPILE_STATUS);
    let link_ok = gl.getProgramParameter(shader.program, gl.LINK_STATUS);
    shader.status_ms += performance.now() - status_started;

    if (!vertex_ok) {
      this.fail_webgl_shader(shader, "vertex", gl.getShaderInfoLog(shader.vsh));
      return false;
    }
    if (!fragment_ok) {
      this.fail_webgl_shader(shader, "fragment", gl.getShaderInfoLog(shader.fsh));
      return false;
    }
    if (!link_ok) {
      const has_instance_texture = /\buniform\s+(?:highp\s+)?usampler2DArray\s+mp_inst_data\s*;/.test(shader.vertex);
      const units = this.texture_unit_error(shader.textures, has_instance_texture);
      const log = gl.getProgramInfoLog(shader.program);
      this.fail_webgl_shader(shader, units ? "units" : "link", units ? `${units}: ${log}` : log);
      return false;
    }

    const texture_locs = [];
    for (const binding of shader.textures) {
      const loc = gl.getUniformLocation(shader.program, binding.name);
      if (loc !== null) texture_locs.push({ ...binding, loc, unit: texture_locs.length });
    }
    const inst_data_loc = gl.getUniformLocation(shader.program, "mp_inst_data");
    const inst_data_unit = inst_data_loc === null ? null : texture_locs.length;
    const units = this.texture_unit_error(texture_locs, inst_data_unit !== null);
    if (units) {
      this.fail_webgl_shader(shader, "units", units);
      return false;
    }

    shader.texture_locs = texture_locs;
    gl.useProgram(shader.program);
    for (const b of texture_locs) {
      b.target = this.sampler_target(b.ty);
      b.nearest = this.nearest_samplers[b.address * 2 + Number(b.compare)];
      b.sampler = this.make_webgl_sampler(b.address, b.compare,
        b.filter ? gl.LINEAR_MIPMAP_LINEAR : gl.NEAREST_MIPMAP_LINEAR,
        b.filter ? gl.LINEAR : gl.NEAREST, b.anisotropy);
      if (!b.sampler || !b.nearest) {
        this.fail_webgl_shader(shader, "sampler", `cannot allocate sampler for ${b.name}`);
        return false;
      }
      gl.uniform1i(b.loc, b.unit);
    }
    if (inst_data_unit !== null) gl.uniform1i(inst_data_loc, inst_data_unit);

    const block_sizes = [0, 0, 0, 0, 0];
    const finished_shader = {
      shader_id: shader.shader_id,
      shader_name: shader.shader_name,
      outputs: shader.outputs,
      linked: true,
      vertex: shader.vertex,
      pixel: shader.pixel,
      geom_attribs:
        shader.geom_attribs && shader.geom_attribs.length
          ? this.webgl_typed_attrib_locations(shader.program, shader.geom_attribs)
          : this.webgl_attrib_locations(
              shader.program,
              "packed_geometry_",
              shader.geometry_slots,
            ),
      // A record wider than an attribute stride is read whole from
      // `mp_inst_data`: no instance attributes.
      inst_attribs:
        inst_data_loc && shader.instance_slots * 4 > MAKEPAD_WEBGL_MAX_ATTRIB_STRIDE
          ? []
          : shader.inst_attribs && shader.inst_attribs.length
          ? this.webgl_typed_attrib_locations(shader.program, shader.inst_attribs)
          : this.webgl_attrib_locations(
              shader.program,
              "packed_instance_",
              shader.instance_slots,
            ),
      inst_data_loc,
      inst_data_unit,
      inst_base_loc: gl.getUniformLocation(shader.program, "mp_inst_base"),
      pass_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "passUniforms", UBO_BINDING_PASS, block_sizes,
      ),
      draw_list_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "draw_listUniforms", UBO_BINDING_DRAW_LIST, block_sizes,
      ),
      draw_call_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "draw_callUniforms", UBO_BINDING_DRAW_CALL, block_sizes,
      ),
      user_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "userUniforms", UBO_BINDING_USER, block_sizes,
      ),
      live_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "liveUniforms", UBO_BINDING_LIVE, block_sizes,
      ),
      clip_y_flip_loc: gl.getUniformLocation(shader.program, "mp_clip_y_flip"),
      frag_y_loc: gl.getUniformLocation(shader.program, "mp_frag_y"),
      block_sizes,
      texture_locs: texture_locs,
      geometry_slots: shader.geometry_slots,
      instance_slots: shader.instance_slots,
      program: shader.program,
      version: (this._webgl_shader_version || 0) + 1,
    };
    this._webgl_shader_version = finished_shader.version;
    this.draw_shaders[shader.shader_id] = finished_shader;
    gl.deleteShader(shader.vsh);
    gl.deleteShader(shader.fsh);
    this.pending_webgl_shader_count -= shader.pending ? 1 : 0;
    shader.pending = false;
    // (an error here is read with the next allocation check)
    this.defer_allocation_check(() => {});
    // The wasm side counts queued compiles; this closes one so
    // Cx::draw_shaders_pending can tell a bake its draws are no longer dropped.
    // With first draws spread, it stays open until the program's first draw.
    if (this.first_draw_spread) {
      finished_shader.first_draw_waiting = true;
      this.to_wasm.ToWasmWebGLShadersLinked({ count: 1 });
    } else {
      this.to_wasm.ToWasmWebGLShadersDone({ count: 1, waited: 0 });
    }
    this.schedule_webgl_shader_summary();
    return true;
  }

  poll_pending_webgl_shaders() {
    if (
      this.webgl_context_lost ||
      !this.parallel_shader_compile ||
      this.pending_webgl_shader_count == 0
    ) {
      return 0;
    }
    let ready_count = 0;
    for (let shader of this.draw_shaders) {
      if (!shader || !shader.pending) {
        continue;
      }
      let status_started = performance.now();
      let complete = this.gl.getProgramParameter(
        shader.program,
        this.parallel_shader_compile.COMPLETION_STATUS_KHR,
      );
      shader.status_ms += performance.now() - status_started;
      if (complete && this.finish_webgl_shader(shader)) {
        ready_count++;
      }
    }
    return ready_count;
  }

  schedule_webgl_shader_poll() {
    if (
      this.webgl_context_lost ||
      this.webgl_shader_poll_frame_id ||
      this.pending_webgl_shader_count == 0
    ) {
      return;
    }
    this.webgl_shader_poll_frame_id = window.requestAnimationFrame(() => {
      this.webgl_shader_poll_frame_id = 0;
      if (this.wasm == null || this.webgl_context_lost) {
        return;
      }
      // An animation frame is coming: it takes the programs up at its start
      // (see FromWasmRequestAnimationFrame), before its own GL work.
      if (this.req_anim_frame_id) {
        this.schedule_webgl_shader_poll();
        return;
      }
      if (this.poll_pending_webgl_shaders() != 0) {
        this.to_wasm.ToWasmRedrawAll();
        this.FromWasmRequestAnimationFrame();
      }
      this.schedule_webgl_shader_poll();
    });
  }

  // Spread programs' first draws one a frame (Cx::spread_first_draws). Off,
  // the programs still waiting for theirs are released.
  FromWasmSpreadFirstDraws(args) {
    this.first_draw_spread = !!args.on;
    if (this.first_draw_spread) {
      return;
    }
    let released = 0;
    for (const shader of this.draw_shaders) {
      if (shader && shader.first_draw_waiting) {
        shader.first_draw_waiting = false;
        released++;
      }
    }
    if (released != 0) {
      this.to_wasm.ToWasmWebGLShadersDone({ count: released, waited: released });
      this.to_wasm.ToWasmRedrawAll();
      this.FromWasmRequestAnimationFrame();
    }
  }

  FromWasmCompileWebGLShader(args) {
    let gl = this.gl;
    let started_at = performance.now();
    if (this.webgl_shader_timeline_start === undefined) {
      this.webgl_shader_timeline_start = started_at;
    }
    this.webgl_shader_batch_program_count++;
    const blend_error = args.blend_add && this.add_blend_error(args.color_format);
    if (blend_error) {
      this.fail_webgl_shader(args, "blend", blend_error);
      return;
    }

    let vsh = gl.createShader(gl.VERTEX_SHADER);
    gl.shaderSource(vsh, args.vertex);
    let stage_started = performance.now();
    gl.compileShader(vsh);
    let vertex_ms = performance.now() - stage_started;

    let fsh = gl.createShader(gl.FRAGMENT_SHADER);
    gl.shaderSource(fsh, args.pixel);
    stage_started = performance.now();
    gl.compileShader(fsh);
    let fragment_ms = performance.now() - stage_started;

    let program = gl.createProgram();
    gl.attachShader(program, vsh);
    gl.attachShader(program, fsh);
    stage_started = performance.now();
    gl.linkProgram(program);
    let link_ms = performance.now() - stage_started;

    let shader = {
      shader_id: args.shader_id,
      shader_name: args.shader_name,
      outputs: args.outputs,
      vertex: args.vertex,
      pixel: args.pixel,
      geometry_slots: args.geometry_slots,
      instance_slots: args.instance_slots,
      textures: args.textures,
      geom_attribs: args.geom_attribs,
      inst_attribs: args.inst_attribs,
      vsh: vsh,
      fsh: fsh,
      program: program,
      pending: !!this.parallel_shader_compile,
      started_at: started_at,
      vertex_ms: vertex_ms,
      fragment_ms: fragment_ms,
      link_ms: link_ms,
      status_ms: 0,
    };
    this.draw_shaders[args.shader_id] = shader;

    if (shader.pending) {
      this.pending_webgl_shader_count++;
      this.schedule_webgl_shader_poll();
    } else {
      this.finish_webgl_shader(shader);
    }
  }

  FromWasmAllocIndexBuffer(args) {
    const gl = this.gl;
    const buffer_id = args && args.buffer_id;
    const buffer = this.numeric_buffer_for_update(
      this.index_buffers,
      buffer_id,
      "index",
    );
    if (!buffer) {
      return;
    }
    const reject = (reason) => {
      this.report_vertex_submission_once(
        `index-upload:${buffer_id}:${reason}`,
        reason,
        { buffer_id, upload_version: buffer.upload_version },
      );
    };
    const index_width = args && args.index_width;
    if (index_width !== 2 && index_width !== 4) {
      reject(`unsupported index width ${index_width}`);
      return;
    }
    if (
      !args.data ||
      !Number.isSafeInteger(args.data.len) ||
      args.data.len < 0 ||
      !args.byte_data ||
      !Number.isSafeInteger(args.byte_data.len) ||
      args.byte_data.len < 0
    ) {
      reject("index upload descriptors are invalid");
      return;
    }
    if (
      index_width === 2 &&
      args.data &&
      Number.isSafeInteger(args.data.len) &&
      args.data.len !== 0
    ) {
      reject("u16 index upload also contains u32 data");
      return;
    }
    if (
      index_width === 4 &&
      args.byte_data &&
      Number.isSafeInteger(args.byte_data.len) &&
      args.byte_data.len !== 0
    ) {
      reject("u32 index upload also contains byte data");
      return;
    }
    const inactive_index_slice = index_width === 2
      ? this.validate_wasm_slice(args.data, 4, false, "unused u32 index data")
      : this.validate_wasm_slice(args.byte_data, 1, true, "unused byte index data");
    if (!inactive_index_slice.ok) {
      reject(inactive_index_slice.reason);
      return;
    }

    const checked = index_width === 2
      ? this.make_validated_wasm_view(
          args.byte_data,
          2,
          true,
          "u16 index data",
          Uint16Array,
        )
      : this.make_validated_wasm_view(
          args.data,
          4,
          false,
          "u32 index data",
          Uint32Array,
        );
    if (!checked.ok) {
      reject(checked.reason);
      return;
    }

    let max_index = -1;
    const restart_index = index_width === 2 ? 0xffff : 0xffffffff;
    for (let i = 0; i < checked.array.length; i++) {
      const index = checked.array[i];
      if (index === restart_index) {
        reject(`fixed primitive-restart index 0x${restart_index.toString(16)} is not valid in TRIANGLES data`);
        return;
      }
      if (index > max_index) {
        max_index = index;
      }
    }

    const uploaded = this.upload_numeric_buffer(
      gl,
      gl.ELEMENT_ARRAY_BUFFER,
      buffer,
      checked.array,
    );
    if (!uploaded.ok) {
      reject(uploaded.reason);
      return;
    }
    buffer.valid = true;
    buffer.byte_length = checked.byte_length;
    buffer.length = checked.element_count;
    buffer.index_width = index_width;
    buffer.index_type = index_width === 2 ? gl.UNSIGNED_SHORT : gl.UNSIGNED_INT;
    buffer.max_index = max_index;
  }

  report_instance_upload_failed(identity, scope, reason, region) {
    const failure = { slab_slot: identity?.slab_slot, backing_generation: identity?.backing_generation, scope, region };
    this.report_vertex_submission_once(`instance-upload:${scope}:${reason}`, reason, failure);
    if (makepad_is_u32(identity?.slab_slot) && makepad_is_u32(identity.backing_generation)) {
      this.to_wasm.ToWasmInstanceUploadFailed(failure);
    }
  }

  FromWasmCreateInstanceSlab(args) {
    const gl = this.gl;
    if (!makepad_is_u32(args?.slab_slot) || !makepad_is_u32(args.backing_generation)) {
      this.report_vertex_submission_once("instance-slab:id", "invalid instance slab identity", args);
      return;
    }
    const existing = this.instance_slabs.get(args.slab_slot);
    if (existing) {
      if (existing.backing_generation !== args.backing_generation || existing.capacity_bytes !== args.capacity_bytes) {
        this.report_instance_upload_failed(args, INSTANCE_FAILURE_BACKING, "instance slab slot is already occupied");
      }
      return;
    }
    const slab = {
      slab_slot: args.slab_slot, backing_generation: args.backing_generation,
      capacity_bytes: args.capacity_bytes, written_end: 0, valid: false, gl_buf: null,
      regions: new Map(), dirty: new Map(), mirror: null, mirror_failed: false,
    };
    this.instance_slabs.set(slab.slab_slot, slab);
    const fail = (reason) => {
      if (this.instance_slabs.get(slab.slab_slot) !== slab) return;
      slab.valid = false;
      if (slab.gl_buf) gl.deleteBuffer(slab.gl_buf);
      slab.gl_buf = null;
      this.report_instance_upload_failed(slab, INSTANCE_FAILURE_BACKING, reason);
    };
    if (!Number.isSafeInteger(slab.capacity_bytes) || slab.capacity_bytes <= 0 || slab.capacity_bytes % 4) {
      fail("invalid instance slab capacity");
      return;
    }
    try {
      slab.gl_buf = gl.createBuffer();
      if (!slab.gl_buf) { fail("instance slab allocation returned null"); return; }
      slab.gl_buf._instance_ledger_role = "slab";
      gl.bindBuffer(gl.ARRAY_BUFFER, slab.gl_buf);
      gl.bufferData(gl.ARRAY_BUFFER, slab.capacity_bytes, gl.DYNAMIC_DRAW);
      slab.valid = true;
      this.defer_allocation_check(() => fail("instance slab allocation failed"));
    } catch (error) {
      fail(`instance slab allocation failed: ${error.message || error}`);
    }
  }

  FromWasmWriteInstanceRegion(args) {
    const slab = this.instance_slabs.get(args?.slab_slot);
    if (!slab || slab.backing_generation !== args.backing_generation) {
      this.report_instance_upload_failed(args, INSTANCE_FAILURE_BACKING, "instance slab is missing or invalid");
      return;
    }
    if (!slab.valid) return; // Create already reported the failed backing.
    const { region, byte_off, byte_len, parts } = args;
    if (!makepad_instance_region_valid(region)) {
      slab.regions.delete(region?.index);
      this.report_vertex_submission_once("instance-region:id", "invalid instance region token", args);
      return;
    }
    const fail = (reason) => {
      slab.regions.delete(region.index);
      this.report_instance_upload_failed(slab, INSTANCE_FAILURE_REGION, reason, region);
    };
    const end = makepad_safe_sum(byte_off, byte_len);
    if (end === null || end > slab.capacity_bytes || byte_off % 4 || byte_len % 4 || !Array.isArray(parts)) {
      fail("invalid instance region extent or parts");
      return;
    }
    let size = 0;
    for (const part of parts) {
      const checked = this.validate_wasm_slice(part, 1, true, "instance region part");
      if (!checked.ok) { fail(checked.reason); return; }
      size += checked.byte_length;
      if (size > byte_len) break;
    }
    if (size !== byte_len) { fail("instance region parts do not fill the destination"); return; }
    try {
      const gl = this.gl;
      const bytes = new Uint8Array(this.memory.buffer);
      gl.bindBuffer(gl.ARRAY_BUFFER, slab.gl_buf);
      let offset = byte_off;
      for (const part of parts) {
        // The WebGL2 overload reads directly from Wasm. A zero length means
        // "to the end of the view" to GL, so omit empty segments.
        if (part.len) gl.bufferSubData(gl.ARRAY_BUFFER, offset, bytes, part.ptr, part.len);
        offset += part.len;
      }
    } catch (error) {
      fail(`instance region write failed: ${error.message || error}`);
      return;
    }
    const written = { gen: region.gen, byte_off, byte_len };
    slab.regions.set(region.index, written);
    if (!byte_len || slab.mirror_failed) return;
    slab.written_end = Math.max(slab.written_end, end);
    if (slab.mirror && byte_len > (slab.dirty.get(byte_off)?.byte_len || 0)) slab.dirty.set(byte_off, written);
  }

  FromWasmAllocArrayBuffer(args) {
    const gl = this.gl;
    const buffer_id = args && args.buffer_id;
    const buffer = this.numeric_buffer_for_update(
      this.array_buffers,
      buffer_id,
      "array",
    );
    if (!buffer) {
      return;
    }
    const reject = (reason) => {
      this.report_vertex_submission_once(
        `array-upload:${buffer_id}:${reason}`,
        reason,
        { buffer_id, upload_version: buffer.upload_version },
      );
    };
    const byte_length = args && args.byte_data && args.byte_data.len;
    const float_length = args && args.data && args.data.len;
    if (
      !args ||
      !args.byte_data ||
      !Number.isSafeInteger(byte_length) ||
      byte_length < 0 ||
      !args.data ||
      !Number.isSafeInteger(float_length) ||
      float_length < 0
    ) {
      reject("array upload descriptors are invalid");
      return;
    }
    if (
      Number.isSafeInteger(byte_length) &&
      byte_length !== 0 &&
      Number.isSafeInteger(float_length) &&
      float_length !== 0
    ) {
      reject("array upload contains both byte and f32 data");
      return;
    }
    const compact = Number.isSafeInteger(byte_length) && byte_length !== 0;
    const inactive_array_slice = compact
      ? this.validate_wasm_slice(args.data, 4, false, "unused f32 vertex data")
      : this.validate_wasm_slice(args.byte_data, 1, true, "unused byte vertex data");
    if (!inactive_array_slice.ok) {
      reject(inactive_array_slice.reason);
      return;
    }
    const checked = compact
      ? this.make_validated_wasm_view(
          args.byte_data,
          1,
          true,
          "byte vertex data",
          Uint8Array,
        )
      : this.make_validated_wasm_view(
          args && args.data,
          4,
          false,
          "f32 vertex data",
          Float32Array,
        );
    if (!checked.ok) {
      reject(checked.reason);
      return;
    }

    // Do not reject NaN f32 words here. Legacy layouts bit-cast integer IDs
    // through f32 storage, and those payloads can legitimately spell a NaN.
    const uploaded = this.upload_numeric_buffer(
      gl,
      gl.ARRAY_BUFFER,
      buffer,
      checked.array,
    );
    if (!uploaded.ok) {
      reject(uploaded.reason);
      return;
    }
    buffer.valid = true;
    buffer.byte_length = checked.byte_length;
    buffer.length = checked.element_count;
    buffer.source_kind = compact ? "bytes" : "f32";
  }

  preflight_webgl_draw(args, vao, shader) {
    const geometry_buffer = this.array_buffers[vao.geom_vb_id];
    const instance_buffer = this.instance_slabs.get(vao.slab_slot);
    if (!instance_buffer?.valid || instance_buffer.backing_generation !== vao.backing_generation) {
      return { ok: false, reason: "instance slab backing is missing or invalid" };
    }
    const written = instance_buffer.regions.get(vao.region.index);
    if (!written || written.gen !== vao.region.gen || written.byte_off !== vao.byte_off ||
        written.byte_len !== vao.byte_len) {
      return { ok: false, reason: "instance region is missing or has a different extent" };
    }
    const instance_bytes = written.byte_off + written.byte_len - vao.instance_start;
    const index_buffer = this.index_buffers[vao.geom_ib_id];
    if (!geometry_buffer || !geometry_buffer.valid || !geometry_buffer.gl_buf) {
      return { ok: false, reason: "geometry buffer is missing or invalid" };
    }
    if (!instance_buffer || !instance_buffer.valid || !instance_buffer.gl_buf) {
      return { ok: false, reason: "instance buffer is missing or invalid" };
    }
    if (!index_buffer || !index_buffer.valid || !index_buffer.gl_buf) {
      return { ok: false, reason: "index buffer is missing or invalid" };
    }
    if (!shader.program) {
      return { ok: false, reason: "shader program allocation failed" };
    }
    const layout = this.validate_webgl_attrib_layout(shader);
    if (!layout.ok) {
      return layout;
    }
    if (
      (args.index_width !== 2 && args.index_width !== 4) ||
      args.index_width !== index_buffer.index_width
    ) {
      return {
        ok: false,
        reason: `draw index width ${args.index_width} does not match uploaded width ${index_buffer.index_width}`,
      };
    }
    if (
      !Number.isSafeInteger(index_buffer.length) ||
      index_buffer.length < 0 ||
      !Number.isSafeInteger(index_buffer.max_index) ||
      index_buffer.max_index < -1
    ) {
      return { ok: false, reason: "index buffer metadata is invalid" };
    }

    const geometry_stride = layout.geometry.stride;
    if (
      geometry_stride > 0 &&
      geometry_buffer.byte_length % geometry_stride !== 0
    ) {
      return { ok: false, reason: "geometry buffer contains a partial record" };
    }
    if (index_buffer.max_index >= 0) {
      for (const attr of layout.geometry.active) {
        const record_offset = makepad_safe_product(
          index_buffer.max_index,
          geometry_stride,
        );
        const required = record_offset === null
          ? null
          : makepad_safe_sum(record_offset, attr.end);
        if (required === null || required > geometry_buffer.byte_length) {
          return {
            ok: false,
            reason: `geometry attribute at location ${attr.loc} is out of bounds for index ${index_buffer.max_index}`,
          };
        }
      }
    }

    // A shader reading instance data from `mp_inst_data` has the record's
    // whole width as its stride, whichever part of it are attributes.
    const instance_stride = shader.inst_data_loc
      ? shader.instance_slots * 4
      : layout.instance.stride;
    let resident_rows = 0;
    if (instance_stride === 0) {
      if (instance_bytes !== 0) {
        return {
          ok: false,
          reason: "instance buffer has data but no physical attribute stride",
        };
      }
    } else {
      if (instance_bytes % instance_stride !== 0 || (shader.inst_data_loc && vao.instance_start % instance_stride)) {
        return { ok: false, reason: "instance buffer contains a partial record" };
      }
      resident_rows = instance_bytes / instance_stride;
      if (!Number.isSafeInteger(resident_rows)) {
        return { ok: false, reason: "instance count is not a safe integer" };
      }
    }
    const ranges = args.instance_ranges;
    if (!Array.isArray(ranges)) {
      return { ok: false, reason: "instance ranges must be an array" };
    }
    let drawn_rows = 0;
    for (const range of ranges) {
      const end = makepad_safe_sum(range?.first, range?.count);
      if (end === null || range.count === 0) {
        return { ok: false, reason: "invalid instance range" };
      }
      if (end > resident_rows) {
        return { ok: false, reason: `instance range [${range.first}, ${end}) exceeds resident row count ${resident_rows}` };
      }
      drawn_rows += range.count;
    }
    const expanded_triangles = makepad_safe_product(
      Math.floor(index_buffer.length / 3),
      drawn_rows,
    );
    if (expanded_triangles === null) {
      return { ok: false, reason: "expanded triangle count is unsafe" };
    }

    const align = this.effective_uniform_align();
    const max_block = this.effective_max_uniform_block();
    const stream_len = this.uniform_stream_len || 0;
    const standard_bindings = [
      shader.pass_uniforms_binding,
      shader.draw_list_uniforms_binding,
      shader.draw_call_uniforms_binding,
      shader.user_uniforms_binding,
      shader.live_uniforms_binding,
    ];
    for (let i = 0; i < STANDARD_UNIFORMS.length; i++) {
      const name = STANDARD_UNIFORMS[i];
      const binding = standard_bindings[i];
      const r = args[name];
      if (!r || !makepad_is_u32(r.off) || !makepad_is_u32(r.len)) {
        return { ok: false, reason: name + " has an invalid uniform stream range" };
      }
      const declared = binding != null ? (shader.block_sizes?.[binding] || 0) : 0;
      if (declared > max_block) {
        return { ok: false, reason: name + " uniform block exceeds device limit" };
      }
      if (r.len === 0) continue;
      if (r.off % align !== 0 || r.len > max_block || r.off + r.len > stream_len) {
        return { ok: false, reason: name + " has an invalid uniform stream range" };
      }
      if (binding != null && declared > 0) {
        const stride = Math.min(Math.ceil(Math.max(r.len, 16) / align) * align, max_block);
        if (declared > stride || r.off + declared > stream_len) {
          return { ok: false, reason: name + " uniform block is shorter than shader declaration" };
        }
      }
    }
    if (this.xr && this.xr.in_xr_pass && (args.pass_uniforms.len < 48 * 4 || args.pass_uniforms.len % 4 !== 0)) {
      return { ok: false, reason: "XR pass uniform range must contain at least 48 whole f32 values" };
    }

    if (!Array.isArray(shader.texture_locs)) {
      return { ok: false, reason: "shader texture metadata is invalid" };
    }
    const sampler_textures = [];
    for (const texture_loc of shader.texture_locs) {
      const slot = texture_loc.slot;
      if (sampler_textures[slot]) continue;
      const texture_id = args.textures && args.textures[slot];
      const expected_target = texture_loc.target;
      let texture;
      if (texture_id === undefined) {
        texture = this.get_sampler_fallback_texture(expected_target, texture_loc.ty === "sampler2DShadow");
      } else {
        if (!Number.isSafeInteger(texture_id) || texture_id < 0) {
          return { ok: false, reason: `texture ${slot} has an invalid id` };
        }
        texture = this.textures[texture_id];
        if (!texture) {
          if (
            this._invalid_texture_upload_ids &&
            this._invalid_texture_upload_ids.has(texture_id)
          ) {
            return { ok: false, reason: `texture ${texture_id} is invalid` };
          }
          texture = this.get_sampler_fallback_texture(expected_target, texture_loc.ty === "sampler2DShadow");
        } else {
          if (texture._render_target_valid === false) {
            return { ok: false, reason: `texture ${texture_id} is invalid` };
          }
          if (
            texture._texture_target !== undefined &&
            texture._texture_target !== expected_target
          ) {
            return {
              ok: false,
              reason: `texture ${texture_id} has the wrong sampler target`,
            };
          }
          if (
            this.active_render_target_textures &&
            this.active_render_target_textures.has(texture)
          ) {
            return {
              ok: false,
              reason: `texture ${texture_id} is an active render target`,
            };
          }
        }
      }
      if (!texture) {
        return { ok: false, reason: "sampler fallback texture allocation failed" };
      }
      sampler_textures[slot] = texture;
    }

    let instance_data_texture = null;
    if (shader.inst_data_loc) {
      instance_data_texture = this.instance_slab_texture(instance_buffer, this.webgl_limits.max_array_texture_layers);
      if (!instance_data_texture) return { ok: false, reason: "instance slab mirror is unavailable" };
    }

    return {
      ok: true,
      geometry_buffer,
      instance_buffer,
      index_buffer,
      layout,
      sampler_textures,
      instance_data_texture,
      indices: index_buffer.length,
      instance_ranges: ranges,
      instance_stride,
    };
  }

  // The slab's words as an R32UI array texture of MAKEPAD_INSTANCE_WORD_ROW-word
  // rows: a slab past one window of words takes as many layers as it needs, so
  // every draw reads its records whole, wherever they sit in the slab.
  instance_slab_texture(slab, max_layers) {
    if (slab.mirror_failed || (slab.mirror && slab.dirty.size === 0)) return slab.mirror;
    const gl = this.gl;
    const row = MAKEPAD_INSTANCE_WORD_ROW;
    const words = slab.capacity_bytes / 4;
    const rows = Math.max(1, Math.ceil(words / row));
    const layers = Math.ceil(rows / row);
    const fail = (reason) => {
      if (this.instance_slabs.get(slab.slab_slot) !== slab || slab.mirror_failed) return;
      slab.mirror_failed = true;
      slab.dirty.clear();
      if (slab.mirror) gl.deleteTexture(slab.mirror);
      slab.mirror = null;
      this.report_instance_upload_failed(slab, INSTANCE_FAILURE_MIRROR, reason);
    };
    try {
      let first = 0, end = 0;
      if (!slab.mirror) {
        if (layers > max_layers) {
          fail(`instance slab of ${words} words needs ${layers} texture windows and the device takes ${max_layers}`);
          return null;
        }
        if (layers > 1) {
          console.log(`makepad: instance slab ${slab.slab_slot} of ${words} words spans ${layers} texture windows of ${MAKEPAD_INSTANCE_WINDOW_WORDS} words`);
        }
        slab.mirror = gl.createTexture();
        if (!slab.mirror) { fail("instance slab mirror allocation returned null"); return null; }
        slab.mirror._instance_ledger_role = "mirror";
        gl.bindTexture(gl.TEXTURE_2D_ARRAY, slab.mirror);
        this.set_texture_storage_info(gl.TEXTURE_2D_ARRAY, slab.mirror, false);
        gl.texImage3D(gl.TEXTURE_2D_ARRAY, 0, gl.R32UI, row, layers > 1 ? row : rows, layers, 0, gl.RED_INTEGER, gl.UNSIGNED_INT, null);
        gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        this.defer_allocation_check(() => fail("instance slab mirror allocation failed"));
        // Seed one contiguous prefix, including gaps between aligned regions.
        end = Math.min(words, slab.written_end / 4);
      } else {
        gl.bindTexture(gl.TEXTURE_2D_ARRAY, slab.mirror);
      }
      gl.bindBuffer(gl.PIXEL_UNPACK_BUFFER, slab.gl_buf);
      for (const range of slab.dirty.values()) {
        const start = range.byte_off / 4;
        if (start >= words || !range.byte_len) continue;
        if (start !== end) {
          makepad_copy_instance_words(gl, first, end);
          first = start;
        }
        end = Math.min(words, start + range.byte_len / 4);
      }
      makepad_copy_instance_words(gl, first, end);
      slab.dirty.clear();
      return slab.mirror;
    } catch (error) {
      fail(`instance slab mirror upload failed: ${error.message || error}`);
      return null;
    } finally {
      gl.bindBuffer(gl.PIXEL_UNPACK_BUFFER, null);
    }
  }

  configure_webgl_vao(vao, shader, preflight) {
    const gl = this.gl;
    const layout = preflight.layout;
    const instance_byte_offset = vao.instance_start + preflight.instance_ranges[0].first * preflight.instance_stride;
    let new_vao = null;
    try {
      new_vao = gl.createVertexArray();
      if (!new_vao) {
        return false;
      }
      gl.bindVertexArray(new_vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, preflight.geometry_buffer.gl_buf);
      for (const attr of layout.geometry.active) {
        makepad_webgl_attrib_pointer(gl, attr, 0);
        gl.enableVertexAttribArray(attr.loc);
        gl.vertexAttribDivisor(attr.loc, 0);
      }

      gl.bindBuffer(gl.ARRAY_BUFFER, preflight.instance_buffer.gl_buf);
      for (const attr of layout.instance.active) {
        makepad_webgl_attrib_pointer(gl, attr, instance_byte_offset);
        gl.enableVertexAttribArray(attr.loc);
        gl.vertexAttribDivisor(attr.loc, 1);
      }
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, preflight.index_buffer.gl_buf);
      gl.bindVertexArray(null);
    } catch (error) {
      try {
        gl.bindVertexArray(null);
      } catch (_bind_error) {
      }
      if (new_vao && typeof gl.deleteVertexArray === "function") {
        gl.deleteVertexArray(new_vao);
      }
      this.report_vertex_submission_once(
        `vao-config:${vao.shader_id}:${vao.geom_vb_id}:${vao.slab_slot}`,
        `VAO configuration failed: ${error && error.message ? error.message : String(error)}`,
        { vao },
      );
      return false;
    }

    const old_gl_vao = vao.gl_vao;
    vao.gl_vao = new_vao;
    vao.ready = true;
    vao.shader_version = shader.version;
    vao.geometry_gl_buf = preflight.geometry_buffer.gl_buf;
    vao.instance_gl_buf = preflight.instance_buffer.gl_buf;
    vao.instance_byte_offset = instance_byte_offset;
    vao.index_gl_buf = preflight.index_buffer.gl_buf;
    if (old_gl_vao && typeof gl.deleteVertexArray === "function") {
      gl.deleteVertexArray(old_gl_vao);
    }
    return true;
  }

  FromWasmAllocVao(args) {
    let gl = this.gl;
    if (
      !args ||
      !Number.isSafeInteger(args.vao_id) ||
      args.vao_id < 0 ||
      !Number.isSafeInteger(args.shader_id) ||
      args.shader_id < 0 ||
      !Number.isSafeInteger(args.geom_ib_id) ||
      args.geom_ib_id < 0 ||
      !Number.isSafeInteger(args.geom_vb_id) ||
      args.geom_vb_id < 0
    ) {
      this.report_vertex_submission_once(
        "vao:invalid-metadata",
        "VAO ids are not nonnegative safe integers",
        { args },
      );
      return;
    }
    const end = makepad_safe_sum(args.byte_off, args.byte_len);
    if (!makepad_is_u32(args.slab_slot) || !makepad_is_u32(args.backing_generation) ||
        !makepad_instance_region_valid(args.region) || end === null ||
        !Number.isSafeInteger(args.instance_byte_offset) ||
        args.instance_byte_offset < args.byte_off || args.instance_byte_offset > end) {
      this.free_webgl_vao(args.vao_id);
      this.report_vertex_submission_once("vao:instance-region", "invalid VAO instance region", args);
      return;
    }
    let old_vao = this.vaos[args.vao_id];
    if (old_vao && old_vao.gl_vao && old_vao.shader_id === args.shader_id &&
        old_vao.geom_ib_id === args.geom_ib_id && old_vao.geom_vb_id === args.geom_vb_id &&
        old_vao.slab_slot === args.slab_slot && old_vao.backing_generation === args.backing_generation) {
      // The rows moved inside the same backing (a rewritten recording, an evacuation):
      // the GL VAO stays, and the draw re-points the instance
      // attributes at the new offset. A replaced GL buffer is still caught at the draw.
      old_vao.region = args.region;
      old_vao.byte_off = args.byte_off;
      old_vao.byte_len = args.byte_len;
      old_vao.instance_start = args.instance_byte_offset;
      return;
    }
    if (old_vao && old_vao.gl_vao) {
      gl.deleteVertexArray(old_vao.gl_vao);
    }
    this.vaos[args.vao_id] = {
      gl_vao: null,
      shader_id: args.shader_id,
      geom_ib_id: args.geom_ib_id,
      geom_vb_id: args.geom_vb_id,
      slab_slot: args.slab_slot,
      backing_generation: args.backing_generation,
      region: args.region,
      byte_off: args.byte_off,
      byte_len: args.byte_len,
      instance_start: args.instance_byte_offset,
      ready: false,
    };

    let shader = this.draw_shaders[args.shader_id];
    if (shader && shader.compile_failed) return;
    if (!shader) {
      this.report_missing_shader_once(
        "FromWasmAllocVao",
        args.shader_id,
        args.vao_id,
      );
      return;
    }
    // Configuration is deliberately lazy. Draw preflight knows the uploaded
    // maximum index and can reject bad geometry before any attrib pointer call.
  }

  free_webgl_vao(id) {
    const vao = this.vaos[id];
    if (!vao) return;
    const gl = this.gl;
    if (vao.gl_vao) gl.deleteVertexArray(vao.gl_vao);
    this.vaos[id] = undefined;
  }

  FromWasmFreeWebGLResources(args) {
    const gl = this.gl;
    const array_buffer_ids = new Set(args.array_buffer_ids || []);
    const index_buffer_ids = new Set(args.index_buffer_ids || []);
    const vao_ids = new Set(args.vao_ids || []);
    const retired_slabs = new Map();
    for (const { slab_slot, backing_generation } of args.instance_slabs || []) {
      const slab = this.instance_slabs.get(slab_slot);
      if (slab && slab.backing_generation === backing_generation) retired_slabs.set(slab_slot, slab);
    }

    // Deleting a buffer does not release it while a VAO still references it.
    // Find and retire all dependent VAOs in one table pass.
    for (let id = 0; id < this.vaos.length; id++) {
      const vao = this.vaos[id];
      if (
        vao &&
        (array_buffer_ids.has(vao.geom_vb_id) ||
          retired_slabs.get(vao.slab_slot)?.backing_generation === vao.backing_generation ||
          index_buffer_ids.has(vao.geom_ib_id))
      ) {
        vao_ids.add(id);
      }
    }

    for (const id of vao_ids) this.free_webgl_vao(id);
    for (const id of array_buffer_ids) {
      const buffer = this.array_buffers[id];
      if (!buffer) continue;
      if (buffer.gl_buf) gl.deleteBuffer(buffer.gl_buf);
      this.array_buffers[id] = undefined;
    }
    for (const slab of retired_slabs.values()) {
      if (slab.mirror) gl.deleteTexture(slab.mirror);
      if (slab.gl_buf) gl.deleteBuffer(slab.gl_buf);
      this.instance_slabs.delete(slab.slab_slot);
    }
    for (const id of index_buffer_ids) {
      const buffer = this.index_buffers[id];
      if (!buffer) continue;
      if (buffer.gl_buf) gl.deleteBuffer(buffer.gl_buf);
      this.index_buffers[id] = undefined;
    }
    for (const id of new Set(args.texture_ids || [])) {
      if (this._invalid_texture_upload_ids) {
        this._invalid_texture_upload_ids.delete(id);
      }
      const texture = this.textures[id];
      if (!texture) continue;
      gl.deleteTexture(texture);
      this.textures[id] = undefined;
    }
    for (const id of new Set(args.framebuffer_ids || [])) {
      const framebuffer = this.framebuffers[id];
      if (!framebuffer) continue;
      gl.deleteFramebuffer(framebuffer);
      this.framebuffers[id] = undefined;
    }

    // Sparse ids can be reused directly; trimming only empty tails prevents
    // the JavaScript tables themselves retaining a repeated-cycle highwater.
    for (const table of [
      this.vaos,
      this.array_buffers,
      this.index_buffers,
      this.textures,
      this.framebuffers,
    ]) {
      while (table.length && table[table.length - 1] === undefined) {
        table.pop();
      }
    }
  }

  draw_webgl_instance_ranges(vao, shader, preflight) {
    const gl = this.gl;
    const attributes = preflight.layout.instance.active;
    for (const { first, count } of preflight.instance_ranges) {
      const byte_offset = vao.instance_start + first * preflight.instance_stride;
      if (attributes.length && vao.instance_byte_offset !== byte_offset) {
        gl.bindBuffer(gl.ARRAY_BUFFER, preflight.instance_buffer.gl_buf);
        // If a pointer call throws, the next range must set every pointer again.
        vao.instance_byte_offset = null;
        for (const attr of attributes) {
          makepad_webgl_attrib_pointer(gl, attr, byte_offset);
        }
        vao.instance_byte_offset = byte_offset;
      }
      if (shader.inst_base_loc != null) {
        const base = byte_offset / preflight.instance_stride;
        if (shader._inst_base !== base) {
          gl.uniform1i(shader.inst_base_loc, base);
          shader._inst_base = base;
        }
      }
      gl.drawElementsInstanced(gl.TRIANGLES, preflight.indices, preflight.index_buffer.index_type, 0, count);
    }
  }

  FromWasmDrawCall(args) {
    var gl = this.gl;
    if (
      !args ||
      !Number.isSafeInteger(args.shader_id) ||
      args.shader_id < 0 ||
      !Number.isSafeInteger(args.vao_id) ||
      args.vao_id < 0
    ) {
      this.report_vertex_submission_once(
        "draw:invalid-ids",
        "draw shader/VAO ids are not nonnegative safe integers",
        { args },
      );
      return;
    }

    if (Array.isArray(args.instance_ranges) && args.instance_ranges.length === 0) return;
    let shader = this.draw_shaders[args.shader_id];
    if (shader && shader.pending) {
      return;
    }
    if (shader && shader.compile_failed) return;
    if (!shader) {
      this.report_missing_shader_once(
        "FromWasmDrawCall",
        args.shader_id,
        args.vao_id,
      );
      return;
    }

    if (args.alpha_blend && args.blend_add) {
      const targets = this.render_color_targets;
      for (let i = 0; targets && i < targets.length; i++) {
        if (!((shader.outputs || 1) & (1 << i))) continue;
        const reason = this.add_blend_error(targets[i] && targets[i].format);
        if (reason) {
          this.fail_webgl_shader(shader, "blend", reason);
          return;
        }
      }
    }
    if (this.render_target_rejected) return;

    let vao = this.vaos[args.vao_id];
    if (!vao) {
      this.report_vertex_submission_once(
        `draw:${args.vao_id}:missing-vao`,
        "VAO is missing",
        { shader_id: args.shader_id, vao_id: args.vao_id },
      );
      return;
    }
    if (vao.shader_id !== args.shader_id) {
      vao.shader_id = args.shader_id;
      vao.ready = false;
    }
    const preflight = this.preflight_webgl_draw(args, vao, shader);
    if (!preflight.ok) {
      if (preflight.not_ready) {
        return;
      }
      this.report_vertex_submission_once(
        `draw:${args.vao_id}:${preflight.reason}`,
        preflight.reason,
        { shader_id: args.shader_id, vao_id: args.vao_id },
      );
      return;
    }
    const needs_configuration =
      !vao.ready ||
      !vao.gl_vao ||
      vao.shader_version !== shader.version ||
      vao.geometry_gl_buf !== preflight.geometry_buffer.gl_buf ||
      vao.instance_gl_buf !== preflight.instance_buffer.gl_buf ||
      vao.index_gl_buf !== preflight.index_buffer.gl_buf;
    if (
      needs_configuration &&
      !this.configure_webgl_vao(vao, shader, preflight)
    ) {
      this.report_vertex_submission_once(
        `draw:${args.vao_id}:vao-allocation`,
        "VAO is missing or invalid",
        { shader_id: args.shader_id, vao_id: args.vao_id },
      );
      return;
    }

    // A program's first draw is where the driver builds it for this draw (on
    // Direct3D, ANGLE compiles the shader variant then: up to seconds of the
    // GPU process for a big one, whatever the link did). Spread, it is the
    // frame's only first draw; another program's waits for the next frame.
    if (shader.first_draw_waiting) {
      if (this.first_draw_this_frame) {
        this.first_draws_deferred = true;
        return;
      }
      this.first_draw_this_frame = true;
      shader.first_draw_waiting = false;
      this.to_wasm.ToWasmWebGLShadersDone({ count: 1, waited: 1 });
    }

    let vao_bound = false;
    try {
      gl.useProgram(shader.program);
      gl.depthMask(!!args.depth_write);
      gl.depthFunc(args.depth_test_greater ? gl.GREATER : gl.LEQUAL);
      // Blending is the pass default (FromWasmSetDefaultDepthAndBlendMode);
      // Add/Max override that state, and alpha_blend=false turns it off.
      // The finally block restores Over; its default path needs no extra calls.
      if (!args.alpha_blend) {
        gl.disable(gl.BLEND);
      } else if (args.blend_add) {
        gl.blendFunc(gl.ONE, gl.ONE);
        gl.blendEquationSeparate(gl.FUNC_ADD, gl.FUNC_ADD);
      } else if (args.blend_max) {
        gl.blendEquationSeparate(gl.MAX, gl.MAX);
      }
      const cull_mode = args.cull_mode || 0;
      if (cull_mode === 0) {
        if (this.bound_cull_enabled !== false) {
          gl.disable(gl.CULL_FACE);
          this.bound_cull_enabled = false;
        }
      } else {
        if (this.bound_cull_enabled !== true) {
          gl.enable(gl.CULL_FACE);
          this.bound_cull_enabled = true;
        }
        const face = cull_mode === 2 ? gl.FRONT : gl.BACK;
        if (this.bound_cull_face !== face) {
          gl.cullFace(face);
          this.bound_cull_face = face;
        }
      }
      // A framebuffer's rows are stored bottom-up: texture passes invert
      // every vertex shader's clip-space Y (`mp_clip_y_flip`, applied after
      // the shader's own vertex code, see shader_glsl.rs), so the texels land
      // in the top-left row order Metal and D3D produce and every consumer
      // plain-samples. The inversion reverses triangle winding: front faces
      // are clockwise there and counter-clockwise on the canvas, matching Metal.
      const clip_y_flip = this.texture_pass_front_face_cw ? 1 : 0;
      if (shader.clip_y_flip_loc && shader._clip_y_flip !== clip_y_flip) {
        gl.uniform1f(shader.clip_y_flip_loc, clip_y_flip);
        shader._clip_y_flip = clip_y_flip;
      }
      // `fragment_position()` counts rows from the target's top: a texture pass already stores
      // them top-down, the canvas bottom-up (y = height - gl_FragCoord.y).
      if (shader.frag_y_loc) {
        const frag_y0 = clip_y_flip ? 0 : gl.drawingBufferHeight;
        const frag_y1 = clip_y_flip ? 1 : -1;
        if (shader._frag_y0 !== frag_y0 || shader._frag_y1 !== frag_y1) {
          gl.uniform2f(shader.frag_y_loc, frag_y0, frag_y1);
          shader._frag_y0 = frag_y0;
          shader._frag_y1 = frag_y1;
        }
      }
      gl.frontFace(this.texture_pass_front_face_cw ? gl.CW : gl.CCW);
      gl.bindVertexArray(vao.gl_vao);
      vao_bound = true;

      if (!shader.custom_uniform_buffers) shader.custom_uniform_buffers = [];
      for (let slot = 0; slot < (args.custom_uniforms || []).length; slot++) {
        const input = args.custom_uniforms[slot];
        let entry = shader.custom_uniform_buffers[slot];
        if (!entry) {
          entry = { buffer: gl.createBuffer(), binding: this.get_uniform_block_binding(shader.program, input.block_name, UBO_BINDING_CUSTOM_BASE + slot) };
          shader.custom_uniform_buffers[slot] = entry;
        }
        const checked = this.make_validated_wasm_view(input.data, 1, true, "custom uniforms", Uint8Array);
        if (!checked.ok || !entry.buffer) continue;
        if (entry.lo !== input.generation_lo || entry.hi !== input.generation_hi) {
          this.upload_uniform_buffer_data(gl, entry.buffer, checked.array, gl.DYNAMIC_DRAW);
          entry.lo = input.generation_lo;
          entry.hi = input.generation_hi;
        }
        this.bind_uniform_block(gl, entry.binding, entry.buffer);
      }
      this.bind_stream_uniform(gl, shader, shader.draw_list_uniforms_binding, args.draw_list_uniforms);
      this.bind_stream_uniform(gl, shader, shader.draw_call_uniforms_binding, args.draw_call_uniforms);
      this.bind_stream_uniform(gl, shader, shader.user_uniforms_binding, args.user_uniforms);
      this.bind_stream_uniform(gl, shader, shader.live_uniforms_binding, args.live_uniforms);

      for (const b of shader.texture_locs) {
        const texture = preflight.sampler_textures[b.slot];
        gl.activeTexture(gl.TEXTURE0 + b.unit);
        gl.bindTexture(b.target, texture);
        gl.bindSampler(b.unit, texture._filterable || (texture._depth && b.compare) ? b.sampler : b.nearest);
      }
      if (shader.inst_data_loc) {
        gl.activeTexture(gl.TEXTURE0 + shader.inst_data_unit);
        gl.bindTexture(gl.TEXTURE_2D_ARRAY, preflight.instance_data_texture);
        gl.bindSampler(shader.inst_data_unit, null);
      }

      const xr = this.xr;
      if (xr && xr.in_xr_pass) {
        const range = args.pass_uniforms;
        if (shader.pass_uniforms_binding != null) {
          if (!this.xr_pass_uniform_buf) this.xr_pass_uniform_buf = gl.createBuffer();
          if (!this.xr_pass_uniform_buf) throw new Error("XR pass buffer allocation failed");
          const xr_bytes = Math.ceil(Math.max(range.len, shader.block_sizes?.[UBO_BINDING_PASS] || 0) / 4) * 4;
          if (this.xr_pass_words?.byteLength !== xr_bytes) {
            this.xr_pass_words = new Float32Array(xr_bytes / 4);
          }
          const dst = new Uint8Array(this.xr_pass_words.buffer);
          dst.set(new Uint8Array(this.memory.buffer, this.uniform_stream_ptr + range.off, range.len));
          if (xr_bytes > range.len) dst.fill(0, range.len);
        }
        const draw_eye = (eye) => {
          const viewport = eye.viewport;
          gl.viewport(viewport.x, viewport.y, viewport.width, viewport.height);
          if (shader.pass_uniforms_binding != null) {
            this.xr_pass_words.set(eye.projection_matrix, 0);
            this.xr_pass_words.set(eye.transform_matrix, 16);
            this.xr_pass_words.set(eye.invtransform_matrix, 32);
            this.upload_uniform_buffer_data(gl, this.xr_pass_uniform_buf, this.xr_pass_words);
            this.bind_uniform_block(gl, shader.pass_uniforms_binding, this.xr_pass_uniform_buf);
          }
          this.draw_webgl_instance_ranges(vao, shader, preflight);
        };
        draw_eye(xr.left_eye);
        draw_eye(xr.right_eye);
      } else {
        this.bind_stream_uniform(gl, shader, shader.pass_uniforms_binding, args.pass_uniforms);
        this.draw_webgl_instance_ranges(vao, shader, preflight);
      }
    } catch (error) {
      this.report_vertex_submission_once(
        `draw:${args.vao_id}:exception`,
        `draw setup failed: ${error && error.message ? error.message : String(error)}`,
        { shader_id: args.shader_id, vao_id: args.vao_id },
      );
    } finally {
      if (vao_bound) {
        try {
          gl.bindVertexArray(null);
        } catch (_error) {
        }
      }
      try {
        gl.depthMask(true);
      } catch (_error) {
      }
      if (!args.alpha_blend) {
        try {
          gl.enable(gl.BLEND);
        } catch (_error) {
        }
      } else if (args.blend_add || args.blend_max) {
        try {
          if (args.blend_add) gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
          gl.blendEquationSeparate(gl.FUNC_ADD, gl.FUNC_ADD);
        } catch (_error) {
        }
      }
    }
  }

  FromWasmAllocTextureImage2D_BGRAu8_32(args) {
    const admission = this.admit_texture_upload(args, MAKEPAD_WEBGL_BGRA_IMAGE_UPLOAD);
    if (!admission) return;
    const source = this.make_bgra_upload_view(admission);
    if (!source) return;
    this.upload_admitted_texture(
      admission,
      "bgra8",
      source,
      (gl, target, allocate, data, checked) => {
        if (allocate) {
          gl.texImage2D(
            target,
            0,
            gl.RGBA,
            checked.args.width,
            checked.args.height,
            0,
            gl.RGBA,
            gl.UNSIGNED_BYTE,
            data,
          );
        } else {
          gl.texSubImage2D(
            target,
            0,
            0,
            0,
            checked.args.width,
            checked.args.height,
            gl.RGBA,
            gl.UNSIGNED_BYTE,
            data,
          );
        }
      },
    );
  }

  // Only the changed rect goes up while the texture holds this image at this
  // size, otherwise the whole image does.
  FromWasmUpdateTextureRect_BGRAu8_32(args) {
    const image = {
      texture_id: args.texture_id,
      width: args.image_width,
      height: args.image_height,
      data: args.data,
    };
    const admission = this.admit_texture_upload(image, MAKEPAD_WEBGL_BGRA_IMAGE_UPLOAD);
    if (!admission) return;
    const texture = this.textures[args.texture_id];
    const right = makepad_safe_sum(args.x, args.width);
    const bottom = makepad_safe_sum(args.y, args.height);
    const rect_fits =
      args.width > 0 &&
      args.height > 0 &&
      right !== null &&
      right <= args.image_width &&
      bottom !== null &&
      bottom <= args.image_height;
    if (
      !texture ||
      !rect_fits ||
      !this.can_update_texture_in_place(texture, args.image_width, args.image_height, "bgra8")
    ) {
      return this.FromWasmAllocTextureImage2D_BGRAu8_32(image);
    }
    const source = this.make_texture_source_view(admission, Uint32Array);
    if (!source) return;
    const gl = this.gl;
    try {
      const count = args.width * args.height;
      const converted = this.get_bgra_upload_scratch(count);
      let out = 0;
      for (let y = args.y; y < args.y + args.height; y++) {
        const row = y * args.image_width + args.x;
        for (let x = 0; x < args.width; x++) {
          converted[out++] = makepad_swap_red_blue(source[row + x]);
        }
      }
      gl.bindTexture(gl.TEXTURE_2D, texture);
      this.set_texture_storage_info(gl.TEXTURE_2D, texture, true);
      gl.texSubImage2D(
        gl.TEXTURE_2D,
        0,
        args.x,
        args.y,
        args.width,
        args.height,
        gl.RGBA,
        gl.UNSIGNED_BYTE,
        new Uint8Array(converted.buffer, 0, count * 4),
      );
    } catch (error) {
      this.reject_texture_upload(
        image,
        "gl-upload",
        `WebGL texture upload failed: ${error && error.message ? error.message : String(error)}`,
      );
    }
  }

  FromWasmAllocTextureImage2D_Ru8(args) {
    const admission = this.admit_texture_upload(args, {
      faces: 1,
      bytes_per_texel: 1,
      elements_per_texel: 1,
      element_size: 1,
      nearest: false,
    });
    if (!admission) return;
    const source = this.make_texture_source_view(admission, Uint8Array);
    if (!source) return;
    this.upload_admitted_texture(
      admission,
      "r8",
      source,
      (gl, target, allocate, data, checked) => {
        try {
          gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
          if (allocate) {
            gl.texImage2D(
              target,
              0,
              gl.R8,
              checked.args.width,
              checked.args.height,
              0,
              gl.RED,
              gl.UNSIGNED_BYTE,
              data,
            );
          } else {
            gl.texSubImage2D(
              target,
              0,
              0,
              0,
              checked.args.width,
              checked.args.height,
              gl.RED,
              gl.UNSIGNED_BYTE,
              data,
            );
          }
        } finally {
          gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
        }
      },
    );
  }

  FromWasmAllocTextureImage2D_RGBAf32(args) {
    const admission = this.admit_texture_upload(args, {
      faces: 1,
      bytes_per_texel: 16,
      elements_per_texel: 4,
      element_size: 4,
      nearest: true,
    });
    if (!admission) return;
    const source = this.make_texture_source_view(admission, Float32Array);
    if (!source) return;
    this.upload_admitted_texture(
      admission,
      "rgba32f",
      source,
      (gl, target, allocate, data, checked) => {
        if (allocate) {
          gl.texImage2D(
            target,
            0,
            gl.RGBA32F,
            checked.args.width,
            checked.args.height,
            0,
            gl.RGBA,
            gl.FLOAT,
            data,
          );
        } else {
          gl.texSubImage2D(
            target,
            0,
            0,
            0,
            checked.args.width,
            checked.args.height,
            gl.RGBA,
            gl.FLOAT,
            data,
          );
        }
      },
    );
  }

  FromWasmAllocTextureCube_BGRAu8_32(args) {
    const admission = this.admit_texture_upload(args, {
      faces: 6,
      bytes_per_texel: 4,
      elements_per_texel: 1,
      element_size: 4,
      nearest: false,
    });
    if (!admission) return;
    const source = this.make_bgra_upload_view(admission);
    if (!source) return;
    this.upload_admitted_texture(
      admission,
      "cube-bgra8",
      source,
      (gl, _target, allocate, data, checked) => {
        const faces = [
          gl.TEXTURE_CUBE_MAP_POSITIVE_X,
          gl.TEXTURE_CUBE_MAP_NEGATIVE_X,
          gl.TEXTURE_CUBE_MAP_POSITIVE_Y,
          gl.TEXTURE_CUBE_MAP_NEGATIVE_Y,
          gl.TEXTURE_CUBE_MAP_POSITIVE_Z,
          gl.TEXTURE_CUBE_MAP_NEGATIVE_Z,
        ];
        const face_bytes = checked.face_texels * 4;
        for (let i = 0; i < faces.length; i++) {
          const face = new Uint8Array(data.buffer, i * face_bytes, face_bytes);
          if (allocate) {
            gl.texImage2D(
              faces[i],
              0,
              gl.RGBA,
              checked.args.width,
              checked.args.height,
              0,
              gl.RGBA,
              gl.UNSIGNED_BYTE,
              face,
            );
          } else {
            gl.texSubImage2D(
              faces[i],
              0,
              0,
              0,
              checked.args.width,
              checked.args.height,
              gl.RGBA,
              gl.UNSIGNED_BYTE,
              face,
            );
          }
        }
      },
    );
  }

  // A 2D array's level 0 (layer-major BGRA), sampled as sampler2DArray.
  FromWasmAllocTextureArray_BGRAu8_32(args) {
    if (!args || !Number.isSafeInteger(args.layers) || args.layers <= 0) {
      return this.reject_texture_upload(args, "invalid-layers", "array texture layer count must be a positive safe integer");
    }
    const admission = this.admit_texture_upload(args, {
      faces: args.layers,
      array: true,
      bytes_per_texel: 4,
      elements_per_texel: 1,
      element_size: 4,
      nearest: false,
    });
    if (!admission) return;
    const source = this.make_bgra_upload_view(admission);
    if (!source) return;
    this.upload_admitted_texture(
      admission,
      // the layer count is part of the allocation
      `array-bgra8x${args.layers}`,
      source,
      (gl, target, allocate, data, checked) => {
        const w = checked.args.width;
        const h = checked.args.height;
        if (allocate) {
          gl.texImage3D(target, 0, gl.RGBA, w, h, args.layers, 0, gl.RGBA, gl.UNSIGNED_BYTE, data);
        } else {
          gl.texSubImage3D(target, 0, 0, 0, 0, w, h, args.layers, gl.RGBA, gl.UNSIGNED_BYTE, data);
        }
      },
    );
  }

  FromWasmBeginRenderTexture(args) {
    if (this.xr !== undefined) {
      this.xr.in_xr_pass = false;
    }
    this.texture_pass_front_face_cw = true;

    let gl = this.gl;
    if (
      !args ||
      !Array.isArray(args.color_targets) ||
      !args.depth_target
    ) {
      this.reject_render_target("missing color/depth target metadata", args);
      return;
    }
    this.render_color_targets = args.color_targets;
    const render_targets = args.color_targets.slice();
    if (args.depth_target.attached) {
      render_targets.push(args.depth_target);
    }
    for (const target of render_targets) {
      const texture = target && this.textures[target.texture_id];
      if (
        texture &&
        texture._texture_target !== undefined &&
        texture._texture_target !== gl.TEXTURE_2D
      ) {
        this.reject_render_target(
          `texture ${target.texture_id} was created for a non-2D target`,
          args,
        );
        return;
      }
    }
    const has_r32f_target = args.color_targets.some(
      (target) => target && target.format >= 1 && target.format <= 4,
    );
    if (has_r32f_target && !this.ext_color_buffer_float) {
      this.reject_render_target(
        "Float target requires EXT_color_buffer_float",
        args,
      );
      return;
    }
    const size = makepad_render_target_size(
      args.width,
      args.height,
      this.webgl_limits,
    );
    if (!size.ok) {
      this.reject_render_target(
        size.reason,
        args,
      );
      return;
    }
    if (has_r32f_target && size.scaled) {
      this.reject_render_target(
        "Float data targets cannot be downscaled",
        args,
      );
      return;
    }
    const render_width = size.width;
    const render_height = size.height;
    if (size.scaled) {
      this.report_render_target_size_once("scaled to safety limits", {
        requested: [size.requested_width, size.requested_height],
        allocated: [render_width, render_height],
        scale: size.scale,
        limits: this.webgl_limits,
      });
    }
    this.render_target_rejected = false;
    try {
    var gl_framebuffer =
      this.framebuffers[args.pass_id] ||
      (this.framebuffers[args.pass_id] = gl.createFramebuffer());
    if (!gl_framebuffer) {
      throw new Error("WebGL framebuffer allocation returned null");
    }
    gl.bindFramebuffer(gl.FRAMEBUFFER, gl_framebuffer);

    let clear_flags = 0;
    let clear_depth = 0.0;
    let clear_color = { r: 0, g: 0, b: 0, a: 0 };
    let allocation_changed = false;
    const color_attachments = [];
    // Several color targets (MRT): each clears to its own color.
    const color_clears = [];

    for (let i = 0; i < args.color_targets.length; i++) {
      let tgt = args.color_targets[i];

      var gl_tex =
        this.textures[tgt.texture_id] ||
        (this.textures[tgt.texture_id] = gl.createTexture());
      if (!gl_tex) {
        throw new Error(`WebGL color texture ${tgt.texture_id} allocation returned null`);
      }
      color_attachments.push(gl_tex);
      // resize or create texture
      clear_color = tgt.clear_color;
      if (
        gl_tex._width != render_width ||
        gl_tex._height != render_height ||
        gl_tex._format != tgt.format ||
        gl_tex._render_target_valid === false
      ) {
        gl_tex._texture_target = gl.TEXTURE_2D;
        gl.bindTexture(gl.TEXTURE_2D, gl_tex);
        allocation_changed = true;
        this.clear_texture_upload_allocation(gl_tex);

        clear_flags |= gl.COLOR_BUFFER_BIT;
        color_clears.push(i);

        gl_tex._width = render_width;
        gl_tex._height = render_height;
        gl_tex._requested_width = args.width;
        gl_tex._requested_height = args.height;
        gl_tex._format = tgt.format;
        gl_tex._depth = false;
        this.set_texture_storage_info(gl.TEXTURE_2D, gl_tex, (tgt.format !== 1 && tgt.format !== 2) || this.float_linear);
        if (tgt.format >= 1 && tgt.format <= 5) {
          // Data targets: 1 R32F (RenderRf32), 2 RGBA32F, 3 RGBA16F,
          // 4 R16F (RenderRf16), 5 R8 (RenderRu8). Float targets are
          // color-renderable only with EXT_color_buffer_float; 32-bit float
          // filtering is a separate extension, so those stay NEAREST, while
          // R16F and R8 filter natively in WebGL2.
          const filter = tgt.format >= 4 ? gl.LINEAR : gl.NEAREST;
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
          const internal = [0, gl.R32F, gl.RGBA32F, gl.RGBA16F, gl.R16F, gl.R8][tgt.format];
          const upload = tgt.format === 1 || tgt.format >= 4 ? gl.RED : gl.RGBA;
          const type = tgt.format === 5 ? gl.UNSIGNED_BYTE : (tgt.format === 3 || tgt.format === 4 ? gl.HALF_FLOAT : gl.FLOAT);
          gl.texImage2D(
            gl.TEXTURE_2D,
            0,
            internal,
            gl_tex._width,
            gl_tex._height,
            0,
            upload,
            type,
            null,
          );
        } else {
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
          gl.texImage2D(
            gl.TEXTURE_2D,
            0,
            gl.RGBA,
            gl_tex._width,
            gl_tex._height,
            0,
            gl.RGBA,
            gl.UNSIGNED_BYTE,
            null,
          );
        }
      } else if (!tgt.init_only) {
        clear_flags |= gl.COLOR_BUFFER_BIT;
        color_clears.push(i);
      }

      gl.framebufferTexture2D(
        gl.FRAMEBUFFER,
        gl.COLOR_ATTACHMENT0 + i,
        gl.TEXTURE_2D,
        gl_tex,
        0,
      );
    }
    // Attachments a previous use of this framebuffer had beyond this pass's.
    const previous_count = Array.isArray(gl_framebuffer._color_attachments)
      ? gl_framebuffer._color_attachments.length
      : 0;
    for (let i = args.color_targets.length; i < previous_count; i++) {
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0 + i, gl.TEXTURE_2D, null, 0);
    }
    if (args.color_targets.length !== 1 || previous_count > 1) {
      gl.drawBuffers(args.color_targets.map((_, i) => gl.COLOR_ATTACHMENT0 + i));
    }
    // Depth target: a real depth/stencil texture, so a texture pass
    // depth-tests exactly like the canvas and like Metal. Without it every
    // 3D scene rendered into a texture (the tilted map under its tilt-shift
    // blur, effect thumbnails) was draw-order only: roofs overpainted by
    // the walls drawn after them, buildings hollow, landmarks buried.
    let dt = args.depth_target;
    if (dt.attached) {
      var gl_dtex =
        this.textures[dt.texture_id] ||
        (this.textures[dt.texture_id] = gl.createTexture());
      if (!gl_dtex) {
        throw new Error(`WebGL depth texture ${dt.texture_id} allocation returned null`);
      }
      if (
        gl_dtex._width != render_width ||
        gl_dtex._height != render_height ||
        !gl_dtex._depth ||
        gl_dtex._format != -1 ||
        gl_dtex._render_target_valid === false
      ) {
        gl_dtex._texture_target = gl.TEXTURE_2D;
        gl.bindTexture(gl.TEXTURE_2D, gl_dtex);
        allocation_changed = true;
        this.clear_texture_upload_allocation(gl_dtex);
        gl_dtex._width = render_width;
        gl_dtex._height = render_height;
        gl_dtex._requested_width = args.width;
        gl_dtex._requested_height = args.height;
        gl_dtex._depth = true;
        this.set_texture_storage_info(gl.TEXTURE_2D, gl_dtex, false);
        gl_dtex._format = -1;
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        gl.texImage2D(
          gl.TEXTURE_2D,
          0,
          gl.DEPTH24_STENCIL8,
          render_width,
          render_height,
          0,
          gl.DEPTH_STENCIL,
          gl.UNSIGNED_INT_24_8,
          null,
        );
        clear_flags |= gl.DEPTH_BUFFER_BIT | gl.STENCIL_BUFFER_BIT;
      } else if (!dt.init_only) {
        clear_flags |= gl.DEPTH_BUFFER_BIT | gl.STENCIL_BUFFER_BIT;
      }
      clear_depth = dt.clear_depth;
      gl.framebufferTexture2D(
        gl.FRAMEBUFFER,
        gl.DEPTH_STENCIL_ATTACHMENT,
        gl.TEXTURE_2D,
        gl_dtex,
        0,
      );
    } else {
      gl.framebufferTexture2D(
        gl.FRAMEBUFFER,
        gl.DEPTH_STENCIL_ATTACHMENT,
        gl.TEXTURE_2D,
        null,
        0,
      );
    }
    const depth_attachment = dt.attached ? gl_dtex : null;
    const previous_color_attachments = gl_framebuffer._color_attachments;
    const attachments_changed =
      !Array.isArray(previous_color_attachments) ||
      previous_color_attachments.length !== color_attachments.length ||
      color_attachments.some(
        (texture, index) => previous_color_attachments[index] !== texture,
      ) ||
      gl_framebuffer._depth_attachment !== depth_attachment;
    if (allocation_changed) {
      // A failed target allocates again on its next use.
      const allocated = depth_attachment ? [...color_attachments, depth_attachment] : color_attachments.slice();
      this.defer_allocation_check(() => {
        for (const texture of allocated) {
          this.invalidate_texture_dependencies(texture);
        }
        this.report_render_target_size_once("allocation failed", { width: render_width, height: render_height });
      });
    }
    if (
      (allocation_changed || attachments_changed) &&
      this.gl_checks &&
      gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE
    ) {
      throw new Error("WebGL framebuffer is incomplete");
    }
    gl_framebuffer._color_attachments = color_attachments;
    gl_framebuffer._depth_attachment = depth_attachment;
    for (const texture of color_attachments) {
      texture._texture_target = gl.TEXTURE_2D;
      texture._render_target_valid = true;
    }
    if (depth_attachment) {
      depth_attachment._texture_target = gl.TEXTURE_2D;
      depth_attachment._render_target_valid = true;
    }
    if (this._invalid_texture_upload_ids) {
      for (const target of render_targets) {
        this._invalid_texture_upload_ids.delete(target.texture_id);
      }
    }
    this.set_active_render_target_textures(
      color_attachments,
      depth_attachment,
    );
    // The viewport uses the same uniform downscale as the attachments, so
    // normalized texture coordinates retain their meaning across passes.
    gl.viewport(0, 0, render_width, render_height);

    if (clear_flags !== 0) {
      // glClear honours the depth mask; the previous draw call may have
      // left it off.
      gl.depthMask(true);
      gl.clearDepth(clear_depth);
      if (args.color_targets.length > 1) {
        for (const i of color_clears) {
          const c = args.color_targets[i].clear_color;
          gl.clearBufferfv(gl.COLOR, i, [c.r, c.g, c.b, c.a]);
        }
        if ((clear_flags & ~gl.COLOR_BUFFER_BIT) !== 0) {
          gl.clear(clear_flags & ~gl.COLOR_BUFFER_BIT);
        }
      } else {
        gl.clearColor(clear_color.r, clear_color.g, clear_color.b, clear_color.a);
        gl.clear(clear_flags);
      }
    }
    } catch (error) {
      this.reject_render_target(
        error && error.message ? error.message : String(error),
        args,
      );
    }
  }

  FromWasmPollGpuCompletion(args) {
    const gl = this.gl;
    let fence = null;
    const finish = success => {
      if (fence && !gl.isContextLost()) gl.deleteSync(fence);
      fence = null;
      if (this.wasm == null) return;
      this.to_wasm.ToWasmGpuCompletion({ ...args, success });
      this.do_wasm_pump();
    };
    try {
      if (this.webgl_context_lost || gl.isContextLost()) { finish(false); return; }
      fence = gl.fenceSync(gl.SYNC_GPU_COMMANDS_COMPLETE, 0);
      if (!fence) { finish(false); return; }
      gl.flush();
    } catch (_) { finish(false); return; }
    const started = performance.now();
    const poll = () => {
      if (this.wasm == null || this.webgl_context_lost || gl.isContextLost()) { finish(false); return; }
      let status;
      try { status = gl.clientWaitSync(fence, 0, 0); }
      catch (_) { finish(false); return; }
      if (status === gl.TIMEOUT_EXPIRED) {
        if (performance.now() - started > 10000) { finish(false); return; }
        setTimeout(poll, 16);
      } else { finish(status !== gl.WAIT_FAILED); }
    };
    poll();
  }

  FromWasmRequestRenderTextureCapture(args) {
    const ticket_lo = args.ticket_lo || 0;
    const ticket_hi = args.ticket_hi || 0;
    if (ticket_lo || ticket_hi) this.readback_api_active = true;
    if (args.register_only) return;
    const send = (error, data = new Uint8Array(0), offset = 0, complete = true,
                  width = 0, height = 0) => {
      if (this.wasm == null || this.webgl_context_lost) return;
      this.to_wasm.ToWasmRenderTextureCapture({
        texture_id: args.texture_id, ticket_lo, ticket_hi,
        width, height, offset, complete, data, error,
      });
      this.do_wasm_pump();
    };
    if (this.webgl_context_lost) return; // loss notification already failed all tickets
    const gl = this.gl;
    const texture = this.textures[args.texture_id];
    if (!texture || texture._render_target_valid === false ||
        !texture._width || !texture._height) {
      send("render target is not allocated");
      return;
    }
    const width = texture._width;
    const height = texture._height;
    const byteLength = width * height * 4;
    if ((args.width && args.width !== width) || (args.height && args.height !== height)) {
      send("allocation changed");
      return;
    }
    this.readback_reserved_bytes ||= 0;
    if (!Number.isSafeInteger(byteLength) || byteLength <= 0 ||
        byteLength > 32 * 1024 * 1024 - this.readback_reserved_bytes ||
        this.pending_render_texture_captures.size >= 256) {
      send("readback capacity exceeded");
      return;
    }
    const framebuffer = gl.createFramebuffer();
    const pixelBuffer = gl.createBuffer();
    if (!framebuffer || !pixelBuffer) {
      if (pixelBuffer) gl.deleteBuffer(pixelBuffer);
      if (framebuffer) gl.deleteFramebuffer(framebuffer);
      send("could not allocate WebGL readback objects");
      return;
    }
    const oldFramebuffer = gl.getParameter(gl.READ_FRAMEBUFFER_BINDING);
    const oldPixelBuffer = gl.getParameter(gl.PIXEL_PACK_BUFFER_BINDING);
    const packState = [gl.PACK_ALIGNMENT, gl.PACK_ROW_LENGTH, gl.PACK_SKIP_ROWS, gl.PACK_SKIP_PIXELS]
      .filter(value => value !== undefined).map(name => [name, gl.getParameter(name)]);
    let fence = null;
    let queueError = "";
    try {
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, framebuffer);
      gl.framebufferTexture2D(gl.READ_FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texture, 0);
      if (gl.checkFramebufferStatus(gl.READ_FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) {
        throw new Error("render target framebuffer is incomplete");
      }
      gl.bindBuffer(gl.PIXEL_PACK_BUFFER, pixelBuffer);
      gl.bufferData(gl.PIXEL_PACK_BUFFER, byteLength, gl.STREAM_READ);
      for (const [name] of packState) gl.pixelStorei(name, name === gl.PACK_ALIGNMENT ? 1 : 0);
      gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, 0);
      fence = gl.fenceSync(gl.SYNC_GPU_COMMANDS_COMPLETE, 0);
      if (!fence || gl.getError() !== gl.NO_ERROR) throw new Error("could not queue WebGL2 readPixels");
      gl.flush();
    } catch (error) {
      queueError = String(error);
    } finally {
      for (const [name, value] of packState) gl.pixelStorei(name, value);
      gl.bindBuffer(gl.PIXEL_PACK_BUFFER, oldPixelBuffer);
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, oldFramebuffer);
    }
    const capture = { frame_id: 0, done: false };
    this.pending_render_texture_captures.add(capture);
    this.readback_reserved_bytes += byteLength;
    const finish = (error, release_gl = !this.webgl_context_lost) => {
      if (capture.done) return false;
      capture.done = true;
      if (capture.frame_id) window.cancelAnimationFrame(capture.frame_id);
      capture.frame_id = 0;
      this.pending_render_texture_captures.delete(capture);
      this.readback_reserved_bytes -= byteLength;
      if (release_gl) {
        if (fence) gl.deleteSync(fence);
        gl.deleteBuffer(pixelBuffer);
        gl.deleteFramebuffer(framebuffer);
      }
      if (error) send(error);
      return true;
    };
    capture.finish = finish;
    if (queueError) { finish(queueError); return; }
    let offset = 0;
    let ready = false;
    const pollStarted = performance.now();
    const poll = frameTime => {
      capture.frame_id = 0;
      if (capture.done) return;
      if (this.wasm == null) { finish(); return; }
      if (this.webgl_context_lost || gl.isContextLost()) {
        this.handle_webgl_context_lost();
        return;
      }
      if (!ready) {
        const status = gl.clientWaitSync(fence, 0, 0);
        if (status === gl.TIMEOUT_EXPIRED) {
          if (performance.now() - pollStarted > 10000) {
            finish("WebGL readback fence timed out");
          } else { capture.frame_id = window.requestAnimationFrame(poll); }
          return;
        }
        if (status === gl.WAIT_FAILED) { finish("WebGL readback fence failed"); return; }
        ready = true;
      }
      // One shared budget across ALL captures, not one budget per ticket.
      // Both getBufferSubData and the JS->wasm bridge receive only this chunk.
      if (this.readback_copy_frame !== frameTime) {
        this.readback_copy_frame = frameTime;
        this.readback_copy_bytes = 0;
      }
      const count = Math.min(byteLength - offset, 256 * 1024 - (this.readback_copy_bytes || 0));
      if (count > 0) {
        const data = new Uint8Array(count);
        const previousBuffer = gl.getParameter(gl.PIXEL_PACK_BUFFER_BINDING);
        try {
          gl.bindBuffer(gl.PIXEL_PACK_BUFFER, pixelBuffer);
          gl.getBufferSubData(gl.PIXEL_PACK_BUFFER, offset, data);
          if (gl.getError() !== gl.NO_ERROR) throw new Error("getBufferSubData failed");
        } catch (error) {
          finish(`WebGL readback copy failed: ${error}`);
          return;
        } finally {
          gl.bindBuffer(gl.PIXEL_PACK_BUFFER, previousBuffer);
        }
        const start = offset;
        offset += count;
        this.readback_copy_bytes = (this.readback_copy_bytes || 0) + count;
        const complete = offset === byteLength;
        if (complete && !finish()) return;
        send("", data, start, complete, width, height);
        if (complete) return;
      }
      capture.frame_id = window.requestAnimationFrame(poll);
    };
    capture.frame_id = window.requestAnimationFrame(poll);
  }

  FromWasmBeginRenderCanvas(args) {
    this.render_color_targets = undefined;
    this.reset_active_render_target_textures();
    this.render_target_rejected = false;
    let gl = this.gl;
    let xr = this.xr;
    this.texture_pass_front_face_cw = false;

    if (xr !== undefined) {
      xr.in_xr_pass = true;
      gl.bindFramebuffer(gl.FRAMEBUFFER, xr.layer.framebuffer);
      gl.viewport(0, 0, xr.layer.framebufferWidth, xr.layer.framebufferHeight);
    } else {
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    }
    let c = args.clear_color;
    gl.depthMask(true);
    gl.clearColor(c.r, c.g, c.b, c.a);
    gl.clearDepth(args.clear_depth);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
  }

  FromWasmSetDefaultDepthAndBlendMode() {
    let gl = this.gl;
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.blendEquationSeparate(gl.FUNC_ADD, gl.FUNC_ADD);
    gl.blendFuncSeparate(
      gl.ONE,
      gl.ONE_MINUS_SRC_ALPHA,
      gl.ONE,
      gl.ONE_MINUS_SRC_ALPHA,
    );
    gl.enable(gl.BLEND);
  }

  // Video Playback API

  // @section video-playback
  FromWasmPrepareVideoPlayback(args) {
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let video = document.createElement("video");
    video.crossOrigin = "anonymous";
    video.playsInline = true;
    video.preload = "auto";
    video.loop = args.should_loop;
    video.muted = args.autoplay; // Mute only if autoplay (browser requirement)

    let player = {
      video: video,
      texture_id: args.texture_id,
      video_id_lo: args.video_id_lo,
      video_id_hi: args.video_id_hi,
      playing: false,
      texture_initialized: false,
      disposed: false,
      handlers: {},
    };

    this.video_players[key] = player;

    player.handlers.loadedmetadata = () => {
      if (player.disposed) {
        return;
      }
      let duration_ms = Math.round(video.duration * 1000);
      // An audio-only source has no picture: its position is reported
      // every frame while it plays, for a host that follows its clock.
      player.audio_only = video.videoWidth === 0 && video.videoHeight === 0;
      this.to_wasm.ToWasmVideoPlaybackPrepared({
        video_id_lo: args.video_id_lo,
        video_id_hi: args.video_id_hi,
        video_width: video.videoWidth,
        video_height: video.videoHeight,
        duration_lo: duration_ms & 0xFFFFFFFF,
        duration_hi: Math.floor(duration_ms / 0x100000000),
      });
      this.do_wasm_pump();
    };

    player.handlers.ended = () => {
      if (player.disposed) {
        return;
      }
      player.playing = false;
      this.to_wasm.ToWasmVideoPlaybackCompleted({
        video_id_lo: args.video_id_lo,
        video_id_hi: args.video_id_hi,
      });
      this.do_wasm_pump();
    };

    player.handlers.play = () => {
      if (player.disposed) {
        return;
      }
      player.playing = true;
      this.ensure_video_animation_frame();
    };

    player.handlers.pause = () => {
      player.playing = false;
    };
    for (const [name, handler] of Object.entries(player.handlers)) {
      video.addEventListener(name, handler);
    }

    video.src = args.source_url;

    if (args.autoplay) {
      video.play().catch(e => {
        if (!player.disposed) {
          console.warn("Video autoplay failed:", e);
        }
      });
    }
  }

  dispose_video_player(player) {
    if (!player || player.disposed) {
      return;
    }
    player.disposed = true;
    player.playing = false;
    for (const [name, handler] of Object.entries(player.handlers || {})) {
      player.video.removeEventListener(name, handler);
    }
    player.handlers = {};
    try {
      player.video.pause();
      player.video.removeAttribute("src");
      player.video.load();
    } catch (_error) {
    }
  }

  FromWasmBeginVideoPlayback(args) {
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.play().catch(e => {
        if (!player.disposed) {
          console.warn("Video play failed:", e);
        }
      });
    }
  }

  FromWasmPauseVideoPlayback(args) {
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.pause();
    }
  }

  FromWasmResumeVideoPlayback(args) {
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.play().catch(e => {
        if (!player.disposed) {
          console.warn("Video resume failed:", e);
        }
      });
    }
  }

  FromWasmMuteVideoPlayback(args) {
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.muted = true;
    }
  }

  FromWasmUnmuteVideoPlayback(args) {
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.muted = false;
    }
  }

  FromWasmSeekVideoPlayback(args) {
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      let position_ms = args.position_ms_lo + args.position_ms_hi * 0x100000000;
      player.video.currentTime = position_ms / 1000.0;
    }
  }

  FromWasmCleanupVideoPlaybackResources(args) {
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      this.dispose_video_player(player);
      delete this.video_players[key];

      this.to_wasm.ToWasmVideoPlaybackResourcesReleased({
        video_id_lo: args.video_id_lo,
        video_id_hi: args.video_id_hi,
      });
      this.do_wasm_pump();
    }
  }

  ensure_video_animation_frame() {
    if (this.webgl_context_lost || this.video_anim_frame_id) {
      return;
    }
    this.video_anim_frame_id = window.requestAnimationFrame(() => {
      this.video_anim_frame_id = 0;
      if (this.webgl_context_lost) {
        return;
      }
      this.update_video_textures();
    });
  }

  update_video_textures() {
    if (this.webgl_context_lost) {
      return;
    }
    let gl = this.gl;
    let any_playing = false;
    let any_updated = false;

    for (let key in this.video_players) {
      let player = this.video_players[key];
      // A paused player uploads once after a context restore (needs_upload).
      if (!player.playing && !player.needs_upload) continue;

      if (player.playing) {
        any_playing = true;
      }

      let video = player.video;
      if (video.readyState < 2) continue;

      if (!Number.isSafeInteger(player.texture_id) || player.texture_id < 0) {
        this.report_texture_upload_once(
          "invalid-video-texture-id",
          "video texture id is not a nonnegative safe integer",
          { texture_id: player.texture_id },
        );
        continue;
      }

      const width = video.videoWidth;
      const height = video.videoHeight;
      if (
        Number.isSafeInteger(width) &&
        Number.isSafeInteger(height) &&
        width >= 0 &&
        height >= 0 &&
        (width === 0 || height === 0)
      ) {
        if (player.audio_only) {
          const ms = Math.round(video.currentTime * 1000);
          this.to_wasm.ToWasmVideoTextureUpdated({
            video_id_lo: player.video_id_lo,
            video_id_hi: player.video_id_hi,
            current_position_lo: ms & 0xFFFFFFFF,
            current_position_hi: Math.floor(ms / 0x100000000),
          });
          any_updated = true;
        }
        // Metadata can become ready before the decoder exposes dimensions.
        continue;
      }
      const video_args = {
        texture_id: player.texture_id,
        width,
        height,
      };
      if (
        !Number.isSafeInteger(width) ||
        !Number.isSafeInteger(height) ||
        width <= 0 ||
        height <= 0
      ) {
        this.reject_texture_upload(
          video_args,
          "invalid-video-dimensions",
          "video dimensions must be positive safe integers",
        );
        continue;
      }
      const max_dimension = makepad_webgl_limit(
        (this.webgl_limits || {}).max_texture_size,
      );
      if (width > max_dimension || height > max_dimension) {
        this.reject_texture_upload(
          video_args,
          "video-device-dimension-limit",
          "video dimensions exceed the cached WebGL device limit",
          { max_dimension },
        );
        continue;
      }
      const texels = makepad_safe_product(width, height);
      const allocation_bytes = texels === null
        ? null
        : makepad_safe_product(texels, 4);
      if (allocation_bytes === null) {
        this.reject_texture_upload(
          video_args,
          "unsafe-video-size",
          "video texture size arithmetic is unsafe",
        );
        continue;
      }

      const old_texture = this.textures[player.texture_id];
      let gl_tex = old_texture;
      if (
        gl_tex &&
        gl_tex._texture_target !== undefined &&
        gl_tex._texture_target !== gl.TEXTURE_2D
      ) {
        this.report_texture_upload_once(
          "video-texture-target",
          "video cannot replace a non-2D texture",
          { texture_id: player.texture_id },
        );
        continue;
      }
      let created = false;
      if (!gl_tex) {
        try {
          gl_tex = gl.createTexture();
        } catch (error) {
          this.reject_texture_upload(
            video_args,
            "video-create-texture-exception",
            `WebGL video texture allocation threw: ${error && error.message ? error.message : String(error)}`,
          );
          continue;
        }
        if (!gl_tex) {
          this.reject_texture_upload(
            video_args,
            "video-create-texture-null",
            "WebGL video texture allocation returned null",
          );
          continue;
        }
        created = true;
      }

      const allocation_changed =
        created ||
        gl_tex._texture_upload_width !== width ||
        gl_tex._texture_upload_height !== height ||
        gl_tex._texture_upload_format !== MAKEPAD_WEBGL_VIDEO_UPLOAD_FORMAT ||
        gl_tex._width !== undefined ||
        gl_tex._height !== undefined ||
        gl_tex._format !== undefined ||
        gl_tex._depth !== undefined ||
        gl_tex._render_target_valid === false;
      if (allocation_changed && old_texture) {
        this.invalidate_texture_dependencies(old_texture);
      }

      try {
        gl_tex._texture_target = gl.TEXTURE_2D;
        gl.bindTexture(gl.TEXTURE_2D, gl_tex);
        this.set_texture_storage_info(gl.TEXTURE_2D, gl_tex, true);

        if (allocation_changed) {
          this.configure_texture_parameters(gl, gl.TEXTURE_2D, false, false);
          gl.texImage2D(
            gl.TEXTURE_2D,
            0,
            gl.RGBA,
            gl.RGBA,
            gl.UNSIGNED_BYTE,
            video,
          );
          this.defer_allocation_check(() => {
            this.invalidate_texture_dependencies(gl_tex);
            player.texture_initialized = false;
          });
        } else {
          gl.texSubImage2D(
            gl.TEXTURE_2D,
            0,
            0,
            0,
            gl.RGBA,
            gl.UNSIGNED_BYTE,
            video,
          );
        }
        player.texture_initialized = true;
        player.needs_upload = false;
        gl_tex._texture_upload_width = width;
        gl_tex._texture_upload_height = height;
        gl_tex._texture_upload_format = MAKEPAD_WEBGL_VIDEO_UPLOAD_FORMAT;
        if (allocation_changed) {
          this.clear_render_target_allocation(gl_tex);
        }
        gl_tex._render_target_valid = true;
        if (created) {
          this.textures[player.texture_id] = gl_tex;
        }
        if (this._invalid_texture_upload_ids) {
          this._invalid_texture_upload_ids.delete(player.texture_id);
        }
      } catch (error) {
        if (created) {
          try {
            gl.deleteTexture(gl_tex);
          } catch (_delete_error) {
          }
        }
        this.reject_texture_upload(
          video_args,
          "video-texture-upload",
          `WebGL video texture upload failed: ${error && error.message ? error.message : String(error)}`,
        );
        continue;
      }

      any_updated = true;

      let current_ms = Math.round(video.currentTime * 1000);
      this.to_wasm.ToWasmVideoTextureUpdated({
        video_id_lo: player.video_id_lo,
        video_id_hi: player.video_id_hi,
        current_position_lo: current_ms & 0xFFFFFFFF,
        current_position_hi: Math.floor(current_ms / 0x100000000),
      });
    }

    if (any_updated) {
      this.do_wasm_pump();
    }
    if (any_playing) {
      this.ensure_video_animation_frame();
    }
  }
  // @end video-playback

  handle_device_pixel_ratio_change() {
    const next = makepad_device_pixel_ratio(window.devicePixelRatio);
    if (next === this.physical_device_dpi) {
      return false;
    }
    // This must track the physical DPR, not the budgeted effective DPR. The
    // latter can intentionally differ forever and would make polling resize
    // continuously.
    this.physical_device_dpi = next;
    if (typeof this.handlers.on_screen_resize === "function") {
      this.handlers.on_screen_resize();
    }
    return true;
  }

  release_device_pixel_ratio_media_query() {
    const mq = this._dpr_media_query;
    const listener = this._dpr_media_query_listener;
    if (mq && listener) {
      if (typeof mq.removeEventListener === "function") {
        mq.removeEventListener("change", listener);
      } else if (typeof mq.removeListener === "function") {
        mq.removeListener(listener);
      }
    }
    this._dpr_media_query = null;
    this._dpr_media_query_listener = null;
  }

  arm_device_pixel_ratio_media_query() {
    if (typeof window.matchMedia !== "function") {
      return false;
    }
    const mq = window.matchMedia(
      `(resolution: ${this.physical_device_dpi}dppx)`,
    );
    if (
      !mq ||
      (typeof mq.addEventListener !== "function" &&
        typeof mq.addListener !== "function")
    ) {
      return false;
    }
    const listener = () => {
      this.release_device_pixel_ratio_media_query();
      this.handle_device_pixel_ratio_change();
      this.arm_device_pixel_ratio_media_query();
    };
    this._dpr_media_query = mq;
    this._dpr_media_query_listener = listener;
    if (typeof mq.addEventListener === "function") {
      mq.addEventListener("change", listener);
    } else {
      mq.addListener(listener);
    }
    return true;
  }

  bind_device_pixel_ratio_change() {
    this.physical_device_dpi = makepad_device_pixel_ratio(
      window.devicePixelRatio,
    );
    if (this.arm_device_pixel_ratio_media_query()) {
      return;
    }
    this._dpr_poll_timer = window.setInterval(() => {
      this.handle_device_pixel_ratio_change();
    }, 1000);
  }

  // A lost context (webglcontextlost) pauses the graphics, not the app:
  // timers, network, audio, threads and media keep running and the app keeps
  // handling events, while its GL commands do nothing (MAKEPAD_WEBGL_COMMANDS)
  // and no animation frame runs. The browser is asked to give the context
  // back (preventDefault). When it does (webglcontextrestored) every table of
  // the old context's objects is emptied and the app is told
  // (ToWasmGpuReset): it uploads again what it keeps on the CPU side and
  // repaints, so the picture goes on from where the app is.
  show_webgl_context_lost_message() {
    if (this._webgl_context_lost_message || typeof document === "undefined") {
      return;
    }
    const message = document.createElement("div");
    const text = document.createElement("span");
    const reload = document.createElement("button");
    message.setAttribute("role", "alert");
    message.style.cssText =
      "position:absolute;inset:0;display:flex;gap:12px;align-items:center;" +
      "justify-content:center;padding:24px;background:#181818;color:white;" +
      "font:14px sans-serif;text-align:center;z-index:2147483647";
    text.textContent =
      "Graphics paused: the WebGL context was lost. They come back when the browser restores it.";
    reload.type = "button";
    reload.textContent = "Reload";
    reload.addEventListener("click", () => {
      if (window.location && typeof window.location.reload === "function") {
        window.location.reload();
      }
    }, { once: true });
    message.appendChild(text);
    message.appendChild(reload);
    const parent = this.canvas.parentNode || document.body;
    if (parent) {
      parent.appendChild(message);
      this._webgl_context_lost_message = message;
    }
  }

  hide_webgl_context_lost_message() {
    const message = this._webgl_context_lost_message;
    this._webgl_context_lost_message = null;
    if (message && message.parentNode) {
      message.parentNode.removeChild(message);
    }
  }

  handle_webgl_context_lost(event) {
    // Without preventDefault the browser never restores the context.
    if (event && typeof event.preventDefault === "function") {
      event.preventDefault();
    }
    if (this.webgl_context_lost) {
      return;
    }
    this.webgl_context_lost = true;
    for (const field of [
      "req_anim_frame_id",
      "webgl_shader_poll_frame_id",
      "video_anim_frame_id",
    ]) {
      if (this[field]) {
        window.cancelAnimationFrame(this[field]);
        this[field] = 0;
      }
    }
    this.clear_frame_timer();
    // Readbacks in flight read objects of the lost context: they end here
    // (their objects went with it) and every ticket fails as device lost.
    for (const capture of this.pending_render_texture_captures || []) {
      if (capture.finish) {
        capture.finish(undefined, false);
        continue;
      }
      capture.done = true;
      if (capture.frame_id) {
        window.cancelAnimationFrame(capture.frame_id);
        capture.frame_id = 0;
      }
    }
    if (this.pending_render_texture_captures) {
      this.pending_render_texture_captures.clear();
    }
    if (this.readback_api_active && this.to_wasm && this.to_wasm.ToWasmRenderTextureCapture) {
      this.to_wasm.ToWasmRenderTextureCapture({
        texture_id: 0, ticket_lo: 0xffffffff, ticket_hi: 0xffffffff,
        width: 0, height: 0, offset: 0, complete: true,
        data: new Uint8Array(0), error: "device lost",
      });
      // The loss can be found inside a message batch (a GL command that
      // threw): the app hears of it after that batch.
      queueMicrotask(() => this.do_wasm_pump());
    }
    this.reset_active_render_target_textures();
    this.render_target_rejected = true;
    console.warn(
      "makepad: WebGL context lost; graphics paused until the browser restores it",
      {
        physical_dpr: this.physical_device_dpi,
        effective_dpr: this.window_info && this.window_info.dpi_factor,
        canvas_width: this.canvas.width,
        canvas_height: this.canvas.height,
      },
    );
    this.show_webgl_context_lost_message();
  }

  // Forgets every object of a lost context: the restored one starts empty.
  reset_webgl_objects() {
    this.bound_cull_enabled = undefined;
    this.bound_cull_face = undefined;
    this.render_color_targets = undefined;
    this.uniform_stream_buf = null;
    this.zero_uniform_buf = null;
    this.uniform_stream_len = 0;
    this.uniform_stream_ptr = 0;
    this.bound_ubo = [];
    this.xr_pass_uniform_buf = null;
    this.xr_pass_words = null;
    this.draw_shaders = [];
    this.array_buffers = [];
    this.instance_slabs = new Map();
    this.index_buffers = [];
    this.vaos = [];
    this.textures = [];
    this.framebuffers = [];
    this.reset_active_render_target_textures();
    this._missing_shader_ids = new Set();
    this._invalid_texture_upload_ids = new Set();
    this._sampler_fallback_textures = new Map();
    this._sampler_fallback_texture_failures = new Set();
    this.pending_webgl_shader_count = 0;
    this.pending_allocation_checks = [];
    this.forget_lost_gpu_state();
  }

  handle_webgl_context_restored() {
    if (!this.webgl_context_lost) {
      return;
    }
    this.reset_webgl_objects();
    // Extensions are enabled per context: ask again.
    this.setup_webgl_context();
    this.webgl_context_lost = false;
    this.render_target_rejected = false;
    this.hide_webgl_context_lost_message();
    console.warn("makepad: WebGL context restored; rebuilding the GPU resources");
    // A paused video's frame is uploaded again too, not only playing ones.
    for (const player of Object.values(this.video_players || {})) {
      player.texture_initialized = false;
      player.needs_upload = true;
    }
    // The app forgets its GPU state, redraws everything and asks for a frame.
    this.to_wasm.ToWasmGpuReset({});
    this.do_wasm_pump();
    if (Object.keys(this.video_players || {}).length !== 0) {
      this.ensure_video_animation_frame();
    }
  }

  init_webgl_context() {
    if (this._webgl_context_initialized) {
      return !!this.gl;
    }
    this._webgl_context_initialized = true;
    var canvas = this.canvas;
    const has_attribute = (name) => typeof canvas.hasAttribute === "function"
      ? canvas.hasAttribute(name)
      : canvas.getAttribute(name) !== null;
    var options = {
      alpha: !has_attribute("noalpha"),
      depth: !has_attribute("nodepth"),
      stencil: !has_attribute("nostencil"),
      // Offscreen composition already performs its own passes. Browser MSAA
      // duplicates the default framebuffer cost, so it is explicit opt-in.
      antialias: has_attribute("antialias") && !has_attribute("noantialias"),
      premultipliedAlpha: has_attribute("premultipliedAlpha"),
      preserveDrawingBuffer: has_attribute("preserveDrawingBuffer"),
      powerPreference: "default",
      //xrCompatible: true
    };

    var gl = (this.gl = canvas.getContext("webgl2", options));

    if (!gl) {
      var span = document.createElement("span");
      span.style.color = "white";
      canvas.parentNode.replaceChild(span, canvas);
      span.innerHTML =
        "Sorry, makepad needs browser support for WebGL2 to run.<br/>Please update your browser or GPU drivers and try again.";
      return false;
    }

    canvas.addEventListener(
      "webglcontextlost",
      (event) => this.handle_webgl_context_lost(event),
      false,
    );
    canvas.addEventListener(
      "webglcontextrestored",
      () => this.handle_webgl_context_restored(),
      false,
    );
    // Live GPU bytes from the first allocation on (GPU-SAFETY.md).
    if (typeof this.install_gpu_ledger === "function") {
      this.install_gpu_ledger(gl);
    }
    this.setup_webgl_context();
    this.bind_device_pixel_ratio_change();
    return true;
  }

  // What is read or enabled per context: once at start and again after a
  // restore (a restored context has no extensions enabled).
  setup_webgl_context() {
    const gl = this.gl;
    this.bound_cull_enabled = false;
    this.bound_cull_face = gl.BACK;
    // Query immutable hardware ceilings once per context, then share the
    // cached values with canvas and targets.
    this.webgl_limits = makepad_query_webgl_limits(gl);
    this.max_vertex_attribs = makepad_webgl_vertex_attrib_limit(
      gl.getParameter(gl.MAX_VERTEX_ATTRIBS),
    );

    // With this extension compileShader/linkProgram only enqueue driver work.
    // Querying COMPILE_STATUS or LINK_STATUS before completion would turn the
    // operation synchronous again, so completion is polled from animation
    // frames and unfinished draws are skipped.
    this.parallel_shader_compile = gl.getExtension(
      "KHR_parallel_shader_compile",
    );

    // WebGL2 needs EXT_color_buffer_float to render/blend half-float targets;
    // blending 32-bit floats also needs EXT_float_blend. Enable both once
    // per context; Add reports missing support through its shader failure path.
    this.ext_color_buffer_float = gl.getExtension("EXT_color_buffer_float");
    this.ext_float_blend = gl.getExtension("EXT_float_blend");
    this.float_linear = !!gl.getExtension("OES_texture_float_linear");
    this.anisotropy_ext = gl.getExtension("EXT_texture_filter_anisotropic");
    this.max_anisotropy = this.anisotropy_ext
      ? gl.getParameter(this.anisotropy_ext.MAX_TEXTURE_MAX_ANISOTROPY_EXT) : 1;
    this.nearest_samplers = Array.from({ length: 8 }, (_, i) =>
      this.make_webgl_sampler(i >> 1, !!(i & 1), gl.NEAREST_MIPMAP_NEAREST, gl.NEAREST, 1));
    if (!this.ext_color_buffer_float) {
      console.error(
        "makepad: EXT_color_buffer_float unavailable — float render targets (GPU lightmap bake) will not work on this device",
      );
    }

    // check uniform count
    var max_vertex_uniforms = gl.getParameter(gl.MAX_VERTEX_UNIFORM_VECTORS);
    var max_fragment_uniforms = gl.getParameter(
      gl.MAX_FRAGMENT_UNIFORM_VECTORS,
    );

    this.max_uniform_buffer_bindings = gl.getParameter(gl.MAX_UNIFORM_BUFFER_BINDINGS) || 24;
    this.gpu_info = {
      uniform_buffer_offset_alignment: gl.getParameter(gl.UNIFORM_BUFFER_OFFSET_ALIGNMENT) || 256,
      max_uniform_block_size: gl.getParameter(gl.MAX_UNIFORM_BLOCK_SIZE) || 65536,
      min_uniforms: Math.min(max_vertex_uniforms, max_fragment_uniforms),
      min_uniform_vectors: Math.min(max_vertex_uniforms, max_fragment_uniforms),
      float_color_targets: !!this.ext_color_buffer_float,
      max_texture_size: this.webgl_limits.max_texture_size,
      vendor: gl.getParameter(gl.VENDOR) || "unknown",
      renderer: gl.getParameter(gl.RENDERER) || "unknown",
    };
    // Firefox reports the real (sanitized) GPU through RENDERER and warns
    // that WEBGL_debug_renderer_info is deprecated, so the extension is only
    // asked for when RENDERER is the masked "WebKit WebGL" placeholder
    // (Chrome, Safari).
    if (/^(webkit webgl|unknown)$/i.test(this.gpu_info.renderer)) {
      let debug_info = gl.getExtension("WEBGL_debug_renderer_info");
      if (debug_info) {
        this.gpu_info.vendor = gl.getParameter(debug_info.UNMASKED_VENDOR_WEBGL);
        this.gpu_info.renderer = gl.getParameter(
          debug_info.UNMASKED_RENDERER_WEBGL,
        );
      }
    }
  }
}

// The GL commands of the from-wasm protocol. While the context is lost they
// do nothing (after the restore the app's ToWasmGpuReset redoes what they
// would have made), and one that throws because the context went away
// inside a batch marks the loss instead of taking the rest of the batch
// (the app's other commands) down with it.
const MAKEPAD_WEBGL_COMMANDS = [
  "FromWasmCompileWebGLShader",
  "FromWasmAllocIndexBuffer",
  "FromWasmCreateInstanceSlab",
  "FromWasmWriteInstanceRegion",
  "FromWasmAllocArrayBuffer",
  "FromWasmAllocVao",
  "FromWasmFreeWebGLResources",
  "FromWasmUploadUniformStream",
  "FromWasmDrawCall",
  "FromWasmAllocTextureImage2D_BGRAu8_32",
  "FromWasmUpdateTextureRect_BGRAu8_32",
  "FromWasmAllocTextureImage2D_Ru8",
  "FromWasmAllocTextureImage2D_RGBAf32",
  "FromWasmAllocTextureCube_BGRAu8_32",
  "FromWasmAllocTextureArray_BGRAu8_32",
  "FromWasmBeginRenderTexture",
  "FromWasmBeginRenderCanvas",
  "FromWasmSetDefaultDepthAndBlendMode",
];
for (const name of MAKEPAD_WEBGL_COMMANDS) {
  const command = WasmWebGL.prototype[name];
  WasmWebGL.prototype[name] = function (args) {
    if (this.webgl_context_lost) {
      return;
    }
    try {
      return command.call(this, args);
    } catch (error) {
      if (this.gl && typeof this.gl.isContextLost === "function" && this.gl.isContextLost()) {
        this.handle_webgl_context_lost();
        return;
      }
      throw error;
    }
  };
}

function add_line_numbers_to_string(code) {
  var lines = code.split("\n");
  var out = "";
  for (let i = 0; i < lines.length; i++) {
    out += i + 1 + ": " + lines[i] + "\n";
  }
  return out;
}

function mat4_invert(out, a) {
  let a00 = a[0];
  let a01 = a[1];
  let a02 = a[2];
  let a03 = a[3];
  let a10 = a[4];
  let a11 = a[5];
  let a12 = a[6];
  let a13 = a[7];
  let a20 = a[8];
  let a21 = a[9];
  let a22 = a[10];
  let a23 = a[11];
  let a30 = a[12];
  let a31 = a[13];
  let a32 = a[14];
  let a33 = a[15];

  let b00 = a00 * a11 - a01 * a10;
  let b01 = a00 * a12 - a02 * a10;
  let b02 = a00 * a13 - a03 * a10;
  let b03 = a01 * a12 - a02 * a11;
  let b04 = a01 * a13 - a03 * a11;
  let b05 = a02 * a13 - a03 * a12;
  let b06 = a20 * a31 - a21 * a30;
  let b07 = a20 * a32 - a22 * a30;
  let b08 = a20 * a33 - a23 * a30;
  let b09 = a21 * a32 - a22 * a31;
  let b10 = a21 * a33 - a23 * a31;
  let b11 = a22 * a33 - a23 * a32;

  // Calculate the determinant
  let det =
    b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;

  if (!det) {
    return null;
  }
  det = 1.0 / det;

  out[0] = (a11 * b11 - a12 * b10 + a13 * b09) * det;
  out[1] = (a02 * b10 - a01 * b11 - a03 * b09) * det;
  out[2] = (a31 * b05 - a32 * b04 + a33 * b03) * det;
  out[3] = (a22 * b04 - a21 * b05 - a23 * b03) * det;
  out[4] = (a12 * b08 - a10 * b11 - a13 * b07) * det;
  out[5] = (a00 * b11 - a02 * b08 + a03 * b07) * det;
  out[6] = (a32 * b02 - a30 * b05 - a33 * b01) * det;
  out[7] = (a20 * b05 - a22 * b02 + a23 * b01) * det;
  out[8] = (a10 * b10 - a11 * b08 + a13 * b06) * det;
  out[9] = (a01 * b08 - a00 * b10 - a03 * b06) * det;
  out[10] = (a30 * b04 - a31 * b02 + a33 * b00) * det;
  out[11] = (a21 * b02 - a20 * b04 - a23 * b00) * det;
  out[12] = (a11 * b07 - a10 * b09 - a12 * b06) * det;
  out[13] = (a00 * b09 - a01 * b07 + a02 * b06) * det;
  out[14] = (a31 * b01 - a30 * b03 - a32 * b00) * det;
  out[15] = (a20 * b03 - a21 * b01 + a22 * b00) * det;

  return out;
}

function mat4_multiply(out, a, b) {
  let a00 = a[0];
  let a01 = a[1];
  let a02 = a[2];
  let a03 = a[3];
  let a10 = a[4];
  let a11 = a[5];
  let a12 = a[6];
  let a13 = a[7];
  let a20 = a[8];
  let a21 = a[9];
  let a22 = a[10];
  let a23 = a[11];
  let a30 = a[12];
  let a31 = a[13];
  let a32 = a[14];
  let a33 = a[15];

  // Cache only the current line of the second matrix
  let b0 = b[0];
  let b1 = b[1];
  let b2 = b[2];
  let b3 = b[3];
  out[0] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
  out[1] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
  out[2] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
  out[3] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;

  b0 = b[4];
  b1 = b[5];
  b2 = b[6];
  b3 = b[7];
  out[4] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
  out[5] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
  out[6] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
  out[7] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;

  b0 = b[8];
  b1 = b[9];
  b2 = b[10];
  b3 = b[11];
  out[8] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
  out[9] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
  out[10] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
  out[11] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;

  b0 = b[12];
  b1 = b[13];
  b2 = b[14];
  b3 = b[15];
  out[12] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
  out[13] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
  out[14] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
  out[15] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;
  return out;
}

function mat4_translation(out, v) {
  out[0] = 1;
  out[1] = 0;
  out[2] = 0;
  out[3] = 0;
  out[4] = 0;
  out[5] = 1;
  out[6] = 0;
  out[7] = 0;
  out[8] = 0;
  out[9] = 0;
  out[10] = 1;
  out[11] = 0;
  out[12] = v[0];
  out[13] = v[1];
  out[14] = v[2];
  out[15] = 1;
  return out;
}

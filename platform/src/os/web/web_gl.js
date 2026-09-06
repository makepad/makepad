import {
  MAKEPAD_WEBGL_PIXEL_BUDGET,
  WasmWebBrowser,
  makepad_compute_webgl_size,
  makepad_device_pixel_ratio,
} from "./web.js";

const MAKEPAD_WEBGL_FALLBACK_DIMENSION = 2048;
const MAKEPAD_WEBGL_FALLBACK_VERTEX_ATTRIBS = 16;
const MAKEPAD_WEBGL_MAX_BUFFER_BYTES = 64 * 1024 * 1024;
const MAKEPAD_WEBGL_MAX_TEXTURE_BYTES = 64 * 1024 * 1024;
const MAKEPAD_WEBGL_MAX_EXPANDED_TRIANGLES = 16 * 1024 * 1024;
const MAKEPAD_WEBGL_MAX_SUBMISSION_REPORTS = 64;
const MAKEPAD_WEBGL_VIDEO_UPLOAD_FORMAT = "video-rgba8";

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
  const max_viewport_width = makepad_webgl_limit(viewport && viewport[0]);
  const max_viewport_height = makepad_webgl_limit(viewport && viewport[1]);
  return {
    max_texture_size,
    max_cube_map_texture_size,
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

export function makepad_render_target_size(
  width,
  height,
  limits,
  pixel_budget = MAKEPAD_WEBGL_PIXEL_BUDGET,
) {
  if (
    !Number.isSafeInteger(width) ||
    !Number.isSafeInteger(height) ||
    width <= 0 ||
    height <= 0
  ) {
    return { ok: false, reason: "dimensions must be positive safe integers" };
  }
  const size = makepad_compute_webgl_size(
    width,
    height,
    1.0,
    limits,
    pixel_budget,
  );
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
    this.draw_shaders = [];
    this.array_buffers = [];
    this.index_buffers = [];
    this.vaos = [];
    this.textures = [];
    this.framebuffers = [];
    this.active_render_target_textures = new Set();
    this.xr = undefined;
    this._missing_shader_ids = new Set();
    this._gl_error_reports = new Set();
    this._vertex_submission_reports = new Set();
    this._texture_upload_reports = new Set();
    this._invalid_texture_upload_ids = new Set();
    this._sampler_fallback_textures = new Map();
    this._sampler_fallback_texture_failures = new Set();
    this._webgl_shader_version = 0;
    this.pending_webgl_shader_count = 0;
    this.webgl_shader_poll_frame_id = 0;
    this.webgl_shader_timeline_start = undefined;
    this.webgl_shader_batch_program_count = 0;
    this.webgl_shader_batch_failed_count = 0;
    this.webgl_shader_summary_timer = undefined;
    this.video_players = {};
    this.pending_render_texture_captures = new Set();
    this.bgra_upload_scratch = new Uint32Array(0);
    if (this.init_webgl_context()) {
      this.load_deps();
    }
  }

  // webGL API

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
    this.to_wasm.ToWasmAnimationFrame({ time: time / 1000.0 });
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

  get_uniform_block_binding(program, name) {
    let gl = this.gl;
    let index = gl.getUniformBlockIndex(program, name);
    if (index === gl.INVALID_INDEX) {
      return null;
    }
    gl.uniformBlockBinding(program, index, index);
    return index;
  }

  upload_uniform_buffer_from_ptr(gl, gl_buf, ptr_f32, gen_lo, gen_hi) {
    if (!gl_buf || ptr_f32.ptr == 0 || ptr_f32.len == 0) {
      return;
    }
    if (
      gl_buf._last_upload_gen_lo === gen_lo &&
      gl_buf._last_upload_gen_hi === gen_hi
    ) {
      return;
    }
    let data = new Float32Array(this.memory.buffer, ptr_f32.ptr, ptr_f32.len);
    this.upload_uniform_buffer_data(gl, gl_buf, data, gl.DYNAMIC_DRAW);
    gl_buf._last_upload_gen_lo = gen_lo;
    gl_buf._last_upload_gen_hi = gen_hi;
  }

  reset_uniform_buffer_upload_cache(gl_buf) {
    if (!gl_buf) {
      return;
    }
    gl_buf._last_upload_gen_lo = undefined;
    gl_buf._last_upload_gen_hi = undefined;
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
      gl_buf._buffer_byte_length = byte_length;
    } else {
      gl.bufferSubData(target, 0, data);
    }
  }

  bind_uniform_block(gl, binding, gl_buf) {
    if (binding === null || !gl_buf) {
      return;
    }
    gl.bindBufferBase(gl.UNIFORM_BUFFER, binding, gl_buf);
  }

  assert_no_gl_error(gl, where) {
    let err = gl.getError();
    if (err !== gl.NO_ERROR) {
      const key = where + ":" + err;
      if (!this._gl_error_reports.has(key)) {
        this._gl_error_reports.add(key);
        const message = "WebGL2 error " + err + " at " + where;
        console.error(message);
        if (typeof window.makepad_report_browser_issue === "function") {
          window.makepad_report_browser_issue("webgl.error", {
            where: where,
            error: err,
            message: message,
          });
        }
      }
    }
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
    if (byte_length > MAKEPAD_WEBGL_MAX_BUFFER_BYTES) {
      return {
        ok: false,
        reason: `${label} exceeds the ${MAKEPAD_WEBGL_MAX_BUFFER_BYTES}-byte limit`,
      };
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
    if (options.faces === 6 && args.width !== args.height) {
      return this.reject_texture_upload(
        args,
        "non-square-cube",
        "cube texture faces must be square",
      );
    }

    const limits = this.webgl_limits || {};
    const max_dimension = makepad_webgl_limit(
      options.faces === 6
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
    if (allocation_bytes > MAKEPAD_WEBGL_MAX_TEXTURE_BYTES) {
      return this.reject_texture_upload(
        args,
        "allocation-byte-limit",
        `texture exceeds the ${MAKEPAD_WEBGL_MAX_TEXTURE_BYTES}-byte allocation limit`,
        { allocation_bytes },
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

  make_bgra_upload_view(admission) {
    const source = this.make_texture_source_view(admission, Uint32Array);
    if (!source) {
      return null;
    }
    try {
      if (
        !(this.bgra_upload_scratch instanceof Uint32Array) ||
        this.bgra_upload_scratch.length < admission.required_elements ||
        this.bgra_upload_scratch.byteLength > MAKEPAD_WEBGL_MAX_TEXTURE_BYTES
      ) {
        this.bgra_upload_scratch = new Uint32Array(admission.required_elements);
      }
      const converted = this.bgra_upload_scratch;
      for (let i = 0; i < admission.required_elements; i++) {
        const value = source[i];
        converted[i] =
          ((value & 0xff) << 16) |
          (value & 0xff00ff00) |
          ((value >>> 16) & 0xff);
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

  get_sampler_fallback_texture(target) {
    const gl = this.gl;
    if (!gl || this.webgl_context_lost) {
      return null;
    }
    const textures = this._sampler_fallback_textures ||
      (this._sampler_fallback_textures = new Map());
    if (textures.has(target)) {
      return textures.get(target);
    }
    const failures = this._sampler_fallback_texture_failures ||
      (this._sampler_fallback_texture_failures = new Set());
    if (failures.has(target)) {
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
      const allocation_error = gl.getError();
      if (allocation_error !== gl.NO_ERROR) {
        throw new Error(`WebGL sampler fallback allocation error ${allocation_error}`);
      }
    } catch (_error) {
      failures.add(target);
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
    textures.set(target, texture);
    return texture;
  }

  upload_admitted_texture(admission, upload_format, source, upload) {
    const gl = this.gl;
    const args = admission.args;
    const target = admission.options.faces === 6
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
      texture._texture_upload_width !== args.width ||
      texture._texture_upload_height !== args.height ||
      texture._texture_upload_format !== upload_format ||
      texture._width !== undefined ||
      texture._height !== undefined ||
      texture._format !== undefined ||
      texture._depth !== undefined ||
      texture._render_target_valid === false;
    if (allocation_changed && old_texture) {
      this.invalidate_texture_dependencies(old_texture);
    }
    texture._render_target_valid = false;
    try {
      gl.bindTexture(target, texture);
      if (allocation_changed) {
        this.configure_texture_parameters(
          gl,
          target,
          admission.options.nearest,
          admission.options.faces === 6,
        );
      }
      upload(gl, target, allocation_changed, source, admission);
      if (allocation_changed) {
        const allocation_error = gl.getError();
        if (allocation_error !== gl.NO_ERROR) {
          throw new Error(`WebGL texture allocation error ${allocation_error}`);
        }
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
      const previous_byte_length = buffer.gl_buf._buffer_byte_length;
      const allocation_changed = previous_byte_length !== array.byteLength;
      this.upload_buffer_data(gl, target, buffer.gl_buf, array, gl.STATIC_DRAW);
      if (allocation_changed && typeof gl.getError === "function") {
        const error = gl.getError();
        if (error !== gl.NO_ERROR) {
          buffer.gl_buf._buffer_byte_length = undefined;
          return { ok: false, reason: `WebGL buffer upload error ${error}` };
        }
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

  fail_webgl_shader(shader, stage, info_log) {
    let gl = this.gl;
    console.error(
      "webgl.compile_fail." + stage + " " + shader.shader_id + " " + info_log,
    );
    gl.deleteShader(shader.vsh);
    gl.deleteShader(shader.fsh);
    gl.deleteProgram(shader.program);
    this.draw_shaders[shader.shader_id] = { compile_failed: true };
    this.pending_webgl_shader_count -= shader.pending ? 1 : 0;
    shader.pending = false;
    this.webgl_shader_batch_failed_count++;
    this.to_wasm.ToWasmWebGLShadersDone({ count: 1 });
    this.schedule_webgl_shader_summary();
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
      this.fail_webgl_shader(shader, "link", gl.getProgramInfoLog(shader.program));
      return false;
    }

    let texture_locs = [];
    for (let i = 0; i < shader.textures.length; i++) {
      let tex_name = shader.textures[i].name;
      let loc = gl.getUniformLocation(shader.program, "tex_" + tex_name);
      if (loc === null) {
        loc = gl.getUniformLocation(shader.program, "ds_" + tex_name);
      }
      texture_locs.push({
        name: tex_name,
        ty: shader.textures[i].ty,
        loc: loc,
      });
    }

    let pass_uniform_buf = null;
    let draw_list_uniform_buf = null;
    let live_uniform_buf = null;
    try {
      pass_uniform_buf = gl.createBuffer();
      draw_list_uniform_buf = gl.createBuffer();
      live_uniform_buf = gl.createBuffer();
    } catch (_error) {
    }
    const finished_shader = {
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
      inst_attribs:
        shader.inst_attribs && shader.inst_attribs.length
          ? this.webgl_typed_attrib_locations(shader.program, shader.inst_attribs)
          : this.webgl_attrib_locations(
              shader.program,
              "packed_instance_",
              shader.instance_slots,
            ),
      pass_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "passUniforms",
      ),
      draw_list_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "draw_listUniforms",
      ),
      draw_call_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "draw_callUniforms",
      ),
      user_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "userUniforms",
      ),
      live_uniforms_binding: this.get_uniform_block_binding(
        shader.program,
        "liveUniforms",
      ),
      pass_uniform_buf,
      draw_list_uniform_buf,
      live_uniform_buf,
      uniform_buffers_valid:
        !!pass_uniform_buf && !!draw_list_uniform_buf && !!live_uniform_buf,
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
    this.assert_no_gl_error(gl, "compile_shader_end");
    // The wasm side counts queued compiles; this closes one so
    // Cx::draw_shaders_pending can tell a bake its draws are no longer dropped.
    this.to_wasm.ToWasmWebGLShadersDone({ count: 1 });
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
      if (this.poll_pending_webgl_shaders() != 0) {
        this.to_wasm.ToWasmRedrawAll();
        this.FromWasmRequestAnimationFrame();
      }
      this.schedule_webgl_shader_poll();
    });
  }

  FromWasmCompileWebGLShader(args) {
    let gl = this.gl;
    let started_at = performance.now();
    if (this.webgl_shader_timeline_start === undefined) {
      this.webgl_shader_timeline_start = started_at;
    }
    this.webgl_shader_batch_program_count++;

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
    const instance_buffer = this.array_buffers[vao.inst_vb_id];
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
    if (shader.uniform_buffers_valid === false || vao.uniform_buffers_valid === false) {
      return { ok: false, reason: "uniform buffer allocation failed" };
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

    const instance_stride = layout.instance.stride;
    let instances = 0;
    if (instance_stride === 0) {
      if (instance_buffer.byte_length !== 0) {
        return {
          ok: false,
          reason: "instance buffer has data but no physical attribute stride",
        };
      }
    } else {
      if (instance_buffer.byte_length % instance_stride !== 0) {
        return { ok: false, reason: "instance buffer contains a partial record" };
      }
      instances = instance_buffer.byte_length / instance_stride;
      if (!Number.isSafeInteger(instances)) {
        return { ok: false, reason: "instance count is not a safe integer" };
      }
    }
    const expanded_triangles = makepad_safe_product(
      Math.floor(index_buffer.length / 3),
      instances,
    );
    if (expanded_triangles === null) {
      return { ok: false, reason: "expanded triangle count is unsafe" };
    }
    if (expanded_triangles > MAKEPAD_WEBGL_MAX_EXPANDED_TRIANGLES) {
      return {
        ok: false,
        reason: `expanded triangle count exceeds ${MAKEPAD_WEBGL_MAX_EXPANDED_TRIANGLES}`,
      };
    }

    const uniform_slices = [
      ["pass uniforms", args.pass_uniforms],
      ["draw-list uniforms", args.draw_list_uniforms],
      ["draw-call uniforms", args.draw_call_uniforms],
      ["user uniforms", args.user_uniforms],
      ["live uniforms", args.live_uniforms],
    ];
    for (const [label, slice] of uniform_slices) {
      const checked = this.validate_wasm_slice(slice, 4, false, label);
      if (!checked.ok) {
        return checked;
      }
    }
    if (
      this.xr !== undefined &&
      this.xr.in_xr_pass &&
      args.pass_uniforms.len < 48
    ) {
      return { ok: false, reason: "XR pass uniform buffer is shorter than 48 f32 values" };
    }

    const uniform_blocks = [
      [shader.pass_uniforms_binding, shader.pass_uniform_buf, "pass"],
      [shader.draw_list_uniforms_binding, shader.draw_list_uniform_buf, "draw-list"],
      [shader.draw_call_uniforms_binding, vao.draw_call_uniform_buf, "draw-call"],
      [shader.user_uniforms_binding, vao.user_uniform_buf, "user"],
      [shader.live_uniforms_binding, shader.live_uniform_buf, "live"],
    ];
    for (const [binding, buffer, label] of uniform_blocks) {
      if (binding !== null && !buffer) {
        return { ok: false, reason: `${label} uniform buffer allocation failed` };
      }
    }

    if (!Array.isArray(shader.texture_locs)) {
      return { ok: false, reason: "shader texture metadata is invalid" };
    }
    const sampler_textures = new Array(shader.texture_locs.length);
    for (let i = 0; i < shader.texture_locs.length; i++) {
      const texture_loc = shader.texture_locs[i];
      if (!texture_loc || texture_loc.loc == null) {
        continue;
      }
      const texture_id = args.textures && args.textures[i];
      const expected_target = texture_loc.ty === "samplerCube"
        ? this.gl.TEXTURE_CUBE_MAP
        : this.gl.TEXTURE_2D;
      let texture;
      if (texture_id === undefined) {
        texture = this.get_sampler_fallback_texture(expected_target);
      } else {
        if (!Number.isSafeInteger(texture_id) || texture_id < 0) {
          return { ok: false, reason: `texture ${i} has an invalid id` };
        }
        texture = this.textures[texture_id];
        if (!texture) {
          if (
            this._invalid_texture_upload_ids &&
            this._invalid_texture_upload_ids.has(texture_id)
          ) {
            return { ok: false, reason: `texture ${texture_id} is invalid` };
          }
          texture = this.get_sampler_fallback_texture(expected_target);
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
      sampler_textures[i] = texture;
    }

    return {
      ok: true,
      geometry_buffer,
      instance_buffer,
      index_buffer,
      layout,
      sampler_textures,
      indices: index_buffer.length,
      instances,
    };
  }

  configure_webgl_vao(vao, shader, preflight) {
    const gl = this.gl;
    const layout = preflight.layout;
    let new_vao = null;
    try {
      new_vao = gl.createVertexArray();
      if (!new_vao) {
        return false;
      }
      gl.bindVertexArray(new_vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, preflight.geometry_buffer.gl_buf);
      for (const attr of layout.geometry.active) {
        if (attr.integer) {
          gl.vertexAttribIPointer(
            attr.loc,
            attr.size,
            attr.gl_type,
            attr.stride,
            attr.offset,
          );
        } else {
          gl.vertexAttribPointer(
            attr.loc,
            attr.size,
            attr.gl_type,
            !!attr.normalized,
            attr.stride,
            attr.offset,
          );
        }
        gl.enableVertexAttribArray(attr.loc);
        gl.vertexAttribDivisor(attr.loc, 0);
      }

      gl.bindBuffer(gl.ARRAY_BUFFER, preflight.instance_buffer.gl_buf);
      for (const attr of layout.instance.active) {
        if (attr.integer) {
          gl.vertexAttribIPointer(
            attr.loc,
            attr.size,
            attr.gl_type,
            attr.stride,
            attr.offset,
          );
        } else {
          gl.vertexAttribPointer(
            attr.loc,
            attr.size,
            attr.gl_type,
            !!attr.normalized,
            attr.stride,
            attr.offset,
          );
        }
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
        `vao-config:${vao.shader_id}:${vao.geom_vb_id}:${vao.inst_vb_id}`,
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
    vao.index_gl_buf = preflight.index_buffer.gl_buf;
    this.reset_uniform_buffer_upload_cache(vao.draw_call_uniform_buf);
    this.reset_uniform_buffer_upload_cache(vao.user_uniform_buf);
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
      args.geom_vb_id < 0 ||
      !Number.isSafeInteger(args.inst_vb_id) ||
      args.inst_vb_id < 0
    ) {
      this.report_vertex_submission_once(
        "vao:invalid-metadata",
        "VAO ids are not nonnegative safe integers",
        { args },
      );
      return;
    }
    let old_vao = this.vaos[args.vao_id];
    if (old_vao && old_vao.gl_vao) {
      gl.deleteVertexArray(old_vao.gl_vao);
    }
    let draw_call_uniform_buf = old_vao && old_vao.draw_call_uniform_buf;
    let user_uniform_buf = old_vao && old_vao.user_uniform_buf;
    try {
      draw_call_uniform_buf = draw_call_uniform_buf || gl.createBuffer();
      user_uniform_buf = user_uniform_buf || gl.createBuffer();
    } catch (_error) {
    }
    let vao = (this.vaos[args.vao_id] = {
      gl_vao: null,
      shader_id: args.shader_id,
      geom_ib_id: args.geom_ib_id,
      geom_vb_id: args.geom_vb_id,
      inst_vb_id: args.inst_vb_id,
      draw_call_uniform_buf,
      user_uniform_buf,
      uniform_buffers_valid: !!draw_call_uniform_buf && !!user_uniform_buf,
      ready: false,
    });

    let shader = this.draw_shaders[args.shader_id];
    if (!shader || shader.compile_failed) {
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

  FromWasmFreeWebGLResources(args) {
    const gl = this.gl;
    const array_buffer_ids = new Set(args.array_buffer_ids || []);
    const index_buffer_ids = new Set(args.index_buffer_ids || []);
    const vao_ids = new Set(args.vao_ids || []);

    // Deleting a buffer does not release it while a VAO still references it.
    // Find all dependants in one table pass (rather than one scan per id) and
    // retire their private uniform buffers along with the VAO.
    for (let id = 0; id < this.vaos.length; id++) {
      const vao = this.vaos[id];
      if (
        vao &&
        (array_buffer_ids.has(vao.geom_vb_id) ||
          array_buffer_ids.has(vao.inst_vb_id) ||
          index_buffer_ids.has(vao.geom_ib_id))
      ) {
        vao_ids.add(id);
      }
    }

    for (const id of vao_ids) {
      const vao = this.vaos[id];
      if (!vao) continue;
      if (vao.gl_vao) gl.deleteVertexArray(vao.gl_vao);
      if (vao.draw_call_uniform_buf) gl.deleteBuffer(vao.draw_call_uniform_buf);
      if (vao.user_uniform_buf) gl.deleteBuffer(vao.user_uniform_buf);
      this.vaos[id] = undefined;
    }
    for (const id of array_buffer_ids) {
      const buffer = this.array_buffers[id];
      if (!buffer) continue;
      if (buffer.gl_buf) gl.deleteBuffer(buffer.gl_buf);
      this.array_buffers[id] = undefined;
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

  FromWasmDrawCall(args) {
    if (this.webgl_context_lost || this.render_target_rejected) {
      return;
    }
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

    let shader = this.draw_shaders[args.shader_id];
    if (shader && shader.pending) {
      return;
    }
    if (!shader || shader.compile_failed) {
      this.report_missing_shader_once(
        "FromWasmDrawCall",
        args.shader_id,
        args.vao_id,
      );
      return;
    }

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

    let vao_bound = false;
    try {
      gl.useProgram(shader.program);
      gl.depthMask(!!args.depth_write);
      if (args.backface_culling) {
        gl.enable(gl.CULL_FACE);
        gl.cullFace(gl.BACK);
      } else {
        gl.disable(gl.CULL_FACE);
      }
      // Texture passes render with an inverted projection Y (web_gl.rs
      // setup_render_pass), which reverses triangle winding: front faces are
      // clockwise there and counter-clockwise on the canvas, matching Metal.
      gl.frontFace(this.texture_pass_front_face_cw ? gl.CW : gl.CCW);
      gl.bindVertexArray(vao.gl_vao);
      vao_bound = true;

      if (args.reset_draw_uniforms) {
        this.reset_uniform_buffer_upload_cache(vao.draw_call_uniform_buf);
        this.reset_uniform_buffer_upload_cache(vao.user_uniform_buf);
      }

      this.upload_uniform_buffer_from_ptr(
        gl,
        shader.draw_list_uniform_buf,
        args.draw_list_uniforms,
        args.draw_list_uniforms_gen_lo,
        args.draw_list_uniforms_gen_hi,
      );
      this.upload_uniform_buffer_from_ptr(
        gl,
        vao.draw_call_uniform_buf,
        args.draw_call_uniforms,
        args.draw_call_uniforms_gen_lo,
        args.draw_call_uniforms_gen_hi,
      );
      this.upload_uniform_buffer_from_ptr(
        gl,
        vao.user_uniform_buf,
        args.user_uniforms,
        args.user_uniforms_gen_lo,
        args.user_uniforms_gen_hi,
      );
      this.upload_uniform_buffer_from_ptr(
        gl,
        shader.live_uniform_buf,
        args.live_uniforms,
        args.live_uniforms_gen_lo,
        args.live_uniforms_gen_hi,
      );

      this.bind_uniform_block(gl, shader.pass_uniforms_binding, shader.pass_uniform_buf);
      this.bind_uniform_block(gl, shader.draw_list_uniforms_binding, shader.draw_list_uniform_buf);
      this.bind_uniform_block(gl, shader.draw_call_uniforms_binding, vao.draw_call_uniform_buf);
      this.bind_uniform_block(gl, shader.user_uniforms_binding, vao.user_uniform_buf);
      this.bind_uniform_block(gl, shader.live_uniforms_binding, shader.live_uniform_buf);

      for (let i = 0; i < shader.texture_locs.length; i++) {
        const tex_loc = shader.texture_locs[i];
        if (!tex_loc || tex_loc.loc == null) {
          continue;
        }
        const target = tex_loc.ty === "samplerCube"
          ? gl.TEXTURE_CUBE_MAP
          : gl.TEXTURE_2D;
        gl.activeTexture(gl.TEXTURE0 + i);
        gl.bindTexture(target, preflight.sampler_textures[i]);
        gl.uniform1i(tex_loc.loc, i);
      }

      const xr = this.xr;
      if (xr !== undefined && xr.in_xr_pass) {
        const pass_uniforms = new Float32Array(
          this.memory.buffer,
          args.pass_uniforms.ptr,
          args.pass_uniforms.len,
        );
        const draw_eye = (eye) => {
          const viewport = eye.viewport;
          gl.viewport(viewport.x, viewport.y, viewport.width, viewport.height);
          for (let i = 0; i < 16; i++) pass_uniforms[i] = eye.projection_matrix[i];
          for (let i = 0; i < 16; i++) pass_uniforms[i + 16] = eye.transform_matrix[i];
          for (let i = 0; i < 16; i++) pass_uniforms[i + 32] = eye.invtransform_matrix[i];
          this.upload_uniform_buffer_data(gl, shader.pass_uniform_buf, pass_uniforms);
          gl.drawElementsInstanced(
            gl.TRIANGLES,
            preflight.indices,
            preflight.index_buffer.index_type,
            0,
            preflight.instances,
          );
        };
        draw_eye(xr.left_eye);
        draw_eye(xr.right_eye);
      } else {
        this.upload_uniform_buffer_from_ptr(
          gl,
          shader.pass_uniform_buf,
          args.pass_uniforms,
          args.pass_uniforms_gen_lo,
          args.pass_uniforms_gen_hi,
        );
        gl.drawElementsInstanced(
          gl.TRIANGLES,
          preflight.indices,
          preflight.index_buffer.index_type,
          0,
          preflight.instances,
        );
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
    }
  }

  FromWasmAllocTextureImage2D_BGRAu8_32(args) {
    const admission = this.admit_texture_upload(args, {
      faces: 1,
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

  FromWasmBeginRenderTexture(args) {
    if (this.webgl_context_lost) {
      return;
    }
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
      (target) => target && target.format === 1,
    );
    if (has_r32f_target && !this.ext_color_buffer_float) {
      this.reject_render_target(
        "R32F target requires EXT_color_buffer_float",
        args,
      );
      return;
    }
    const quality = this.ensure_render_quality();
    const size = makepad_render_target_size(
      args.width,
      args.height,
      this.webgl_limits,
      quality.pixel_budget,
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
        "R32F data targets cannot be downscaled",
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

        gl_tex._width = render_width;
        gl_tex._height = render_height;
        gl_tex._requested_width = args.width;
        gl_tex._requested_height = args.height;
        gl_tex._format = tgt.format;
        gl_tex._depth = false;
        if (tgt.format === 1) {
          // R32F data target (TextureFormat::RenderRf32). Color-renderable
          // only with EXT_color_buffer_float; NEAREST because float
          // filtering is a separate extension and consumers sample_nearest.
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
          gl.texImage2D(
            gl.TEXTURE_2D,
            0,
            gl.R32F,
            gl_tex._width,
            gl_tex._height,
            0,
            gl.RED,
            gl.FLOAT,
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
      }

      gl.framebufferTexture2D(
        gl.FRAMEBUFFER,
        gl.COLOR_ATTACHMENT0,
        gl.TEXTURE_2D,
        gl_tex,
        0,
      );
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
      const allocation_error = gl.getError();
      if (allocation_error !== gl.NO_ERROR) {
        throw new Error(`WebGL allocation error ${allocation_error}`);
      }
    }
    if (
      (allocation_changed || attachments_changed) &&
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
      gl.clearColor(clear_color.r, clear_color.g, clear_color.b, clear_color.a);
      gl.clearDepth(clear_depth);
      gl.clear(clear_flags);
    }
    } catch (error) {
      this.reject_render_target(
        error && error.message ? error.message : String(error),
        args,
      );
    }
  }

  FromWasmRequestRenderTextureCapture(args) {
    if (this.webgl_context_lost) {
      return;
    }
    const gl = this.gl;
    const texture = this.textures[args.texture_id];
    if (
      !texture ||
      texture._render_target_valid === false ||
      !texture._width ||
      !texture._height
    ) {
      this.to_wasm.ToWasmRenderTextureCapture({
        texture_id: args.texture_id,
        width: 0,
        height: 0,
        data: new Uint8Array(0),
        error: "render target is not allocated",
      });
      this.do_wasm_pump();
      return;
    }

    const width = texture._width;
    const height = texture._height;
    const byteLength = width * height * 4;
    const framebuffer = gl.createFramebuffer();
    const pixelBuffer = gl.createBuffer();
    if (!framebuffer || !pixelBuffer) {
      if (pixelBuffer) gl.deleteBuffer(pixelBuffer);
      if (framebuffer) gl.deleteFramebuffer(framebuffer);
      this.to_wasm.ToWasmRenderTextureCapture({
        texture_id: args.texture_id,
        width: 0,
        height: 0,
        data: new Uint8Array(0),
        error: "could not allocate WebGL readback objects",
      });
      this.do_wasm_pump();
      return;
    }
    const oldFramebuffer = gl.getParameter(gl.FRAMEBUFFER_BINDING);
    const oldPixelBuffer = gl.getParameter(gl.PIXEL_PACK_BUFFER_BINDING);
    const oldPackAlignment = gl.getParameter(gl.PACK_ALIGNMENT);
    gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
    gl.framebufferTexture2D(
      gl.FRAMEBUFFER,
      gl.COLOR_ATTACHMENT0,
      gl.TEXTURE_2D,
      texture,
      0,
    );
    if (gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) {
      gl.bindFramebuffer(gl.FRAMEBUFFER, oldFramebuffer);
      gl.deleteBuffer(pixelBuffer);
      gl.deleteFramebuffer(framebuffer);
      this.to_wasm.ToWasmRenderTextureCapture({
        texture_id: args.texture_id,
        width: 0,
        height: 0,
        data: new Uint8Array(0),
        error: "render target framebuffer is incomplete",
      });
      this.do_wasm_pump();
      return;
    }
    gl.bindBuffer(gl.PIXEL_PACK_BUFFER, pixelBuffer);
    gl.bufferData(gl.PIXEL_PACK_BUFFER, byteLength, gl.STREAM_READ);
    gl.pixelStorei(gl.PACK_ALIGNMENT, 1);
    // With a PIXEL_PACK_BUFFER bound, zero is a byte offset. The transfer is
    // queued on the producing WebGL command stream and does not copy to JS.
    gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, 0);
    const fence = gl.fenceSync(gl.SYNC_GPU_COMMANDS_COMPLETE, 0);
    gl.flush();
    gl.pixelStorei(gl.PACK_ALIGNMENT, oldPackAlignment);
    gl.bindBuffer(gl.PIXEL_PACK_BUFFER, oldPixelBuffer);
    gl.bindFramebuffer(gl.FRAMEBUFFER, oldFramebuffer);

    const capture = { frame_id: 0, done: false };
    this.pending_render_texture_captures.add(capture);
    const finish = (error, release_gl = !this.webgl_context_lost) => {
      if (capture.done) {
        return false;
      }
      capture.done = true;
      if (capture.frame_id) {
        window.cancelAnimationFrame(capture.frame_id);
        capture.frame_id = 0;
      }
      this.pending_render_texture_captures.delete(capture);
      if (release_gl) {
        if (fence) gl.deleteSync(fence);
        gl.deleteBuffer(pixelBuffer);
        gl.deleteFramebuffer(framebuffer);
      }
      if (error && !this.webgl_context_lost) {
        this.to_wasm.ToWasmRenderTextureCapture({
          texture_id: args.texture_id,
          width: 0,
          height: 0,
          data: new Uint8Array(0),
          error,
        });
        this.do_wasm_pump();
      }
      return true;
    };
    if (!fence || gl.getError() !== gl.NO_ERROR) {
      finish("could not queue WebGL2 readPixels");
      return;
    }

    const pollStarted = performance.now();
    const poll = () => {
      capture.frame_id = 0;
      if (capture.done) {
        return;
      }
      if (this.wasm == null) {
        finish();
        return;
      }
      if (this.webgl_context_lost || gl.isContextLost()) {
        finish(undefined, false);
        return;
      }
      const status = gl.clientWaitSync(fence, 0, 0);
      if (status === gl.TIMEOUT_EXPIRED) {
        if (performance.now() - pollStarted > 10000) {
          finish("WebGL readback fence timed out");
          return;
        }
        capture.frame_id = window.requestAnimationFrame(poll);
        return;
      }
      if (status === gl.WAIT_FAILED) {
        finish("WebGL readback fence failed");
        return;
      }
      const data = new Uint8Array(byteLength);
      try {
        gl.bindBuffer(gl.PIXEL_PACK_BUFFER, pixelBuffer);
        gl.getBufferSubData(gl.PIXEL_PACK_BUFFER, 0, data);
      } catch (error) {
        gl.bindBuffer(gl.PIXEL_PACK_BUFFER, oldPixelBuffer);
        finish(`WebGL readback copy failed: ${error}`);
        return;
      }
      gl.bindBuffer(gl.PIXEL_PACK_BUFFER, oldPixelBuffer);
      if (!finish()) {
        return;
      }
      this.to_wasm.ToWasmRenderTextureCapture({
        texture_id: args.texture_id,
        width,
        height,
        data,
        error: "",
      });
      this.do_wasm_pump();
    };
    if (!this.webgl_context_lost) {
      capture.frame_id = window.requestAnimationFrame(poll);
    }
  }

  FromWasmBeginRenderCanvas(args) {
    if (this.webgl_context_lost) {
      return;
    }
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

  FromWasmPrepareVideoPlayback(args) {
    if (this.webgl_context_lost) {
      return;
    }
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
      if (this.webgl_context_lost || player.disposed) {
        return;
      }
      let duration_ms = Math.round(video.duration * 1000);
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
      if (this.webgl_context_lost || player.disposed) {
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
      if (this.webgl_context_lost || player.disposed) {
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
        if (!this.webgl_context_lost && !player.disposed) {
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
    if (this.webgl_context_lost) {
      return;
    }
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.play().catch(e => {
        if (!this.webgl_context_lost && !player.disposed) {
          console.warn("Video play failed:", e);
        }
      });
    }
  }

  FromWasmPauseVideoPlayback(args) {
    if (this.webgl_context_lost) {
      return;
    }
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.pause();
    }
  }

  FromWasmResumeVideoPlayback(args) {
    if (this.webgl_context_lost) {
      return;
    }
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.play().catch(e => {
        if (!this.webgl_context_lost && !player.disposed) {
          console.warn("Video resume failed:", e);
        }
      });
    }
  }

  FromWasmMuteVideoPlayback(args) {
    if (this.webgl_context_lost) {
      return;
    }
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.muted = true;
    }
  }

  FromWasmUnmuteVideoPlayback(args) {
    if (this.webgl_context_lost) {
      return;
    }
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      player.video.muted = false;
    }
  }

  FromWasmSeekVideoPlayback(args) {
    if (this.webgl_context_lost) {
      return;
    }
    let key = args.video_id_lo + "_" + args.video_id_hi;
    let player = this.video_players[key];
    if (player) {
      let position_ms = args.position_ms_lo + args.position_ms_hi * 0x100000000;
      player.video.currentTime = position_ms / 1000.0;
    }
  }

  FromWasmCleanupVideoPlaybackResources(args) {
    if (this.webgl_context_lost) {
      return;
    }
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
      if (!player.playing) continue;

      any_playing = true;

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
      if (allocation_bytes > MAKEPAD_WEBGL_MAX_TEXTURE_BYTES) {
        this.reject_texture_upload(
          video_args,
          "video-allocation-byte-limit",
          `video texture exceeds the ${MAKEPAD_WEBGL_MAX_TEXTURE_BYTES}-byte allocation limit`,
          { allocation_bytes },
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
          const allocation_error = gl.getError();
          if (allocation_error !== gl.NO_ERROR) {
            throw new Error(`WebGL video texture allocation error ${allocation_error}`);
          }
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

  handle_device_pixel_ratio_change() {
    const next = makepad_device_pixel_ratio(window.devicePixelRatio);
    if (next === this.physical_device_dpi) {
      return false;
    }
    // This must track the physical DPR, not the budgeted effective DPR. The
    // latter can intentionally differ forever and would make polling resize
    // continuously.
    this.physical_device_dpi = next;
    if (
      !this.webgl_context_lost &&
      typeof this.handlers.on_screen_resize === "function"
    ) {
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
    if (this.webgl_context_lost || typeof window.matchMedia !== "function") {
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

  stop_webgl_runtime() {
    for (const field of [
      "req_anim_frame_id",
      "webgl_shader_poll_frame_id",
      "video_anim_frame_id",
      "loader_after_presented_frame_id",
    ]) {
      if (this[field]) {
        window.cancelAnimationFrame(this[field]);
        this[field] = 0;
      }
    }
    if (this.loader_fallback_timer) {
      window.clearTimeout(this.loader_fallback_timer);
      this.loader_fallback_timer = null;
    }
    if (this.webgl_shader_summary_timer !== undefined) {
      window.clearTimeout(this.webgl_shader_summary_timer);
      this.webgl_shader_summary_timer = undefined;
    }
    if (this.poll_timer !== undefined && this.poll_timer !== null) {
      window.clearInterval(this.poll_timer);
      this.poll_timer = null;
    }
    if (this._dpr_poll_timer !== undefined && this._dpr_poll_timer !== null) {
      window.clearInterval(this._dpr_poll_timer);
      this._dpr_poll_timer = null;
    }
    this.release_device_pixel_ratio_media_query();
    for (const timer of this.timers || []) {
      try {
        if (timer.repeats) {
          window.clearInterval(timer.sys_id);
        } else {
          window.clearTimeout(timer.sys_id);
        }
      } catch (error) {
        console.error(`makepad: terminal timer cleanup failed: ${error}`);
      }
    }
    if (this.timers) {
      this.timers.length = 0;
    }
    for (const capture of this.pending_render_texture_captures || []) {
      capture.done = true;
      if (capture.frame_id) {
        window.cancelAnimationFrame(capture.frame_id);
        capture.frame_id = 0;
      }
    }
    if (this.pending_render_texture_captures) {
      this.pending_render_texture_captures.clear();
    }
    for (const player of Object.values(this.video_players || {})) {
      try {
        this.dispose_video_player(player);
      } catch (error) {
        console.error(`makepad: terminal video cleanup failed: ${error}`);
      }
    }
    this.video_players = {};
    try {
      this.stop_terminal_web_runtime();
    } catch (error) {
      console.error(`makepad: terminal web cleanup failed: ${error}`);
    }
  }

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
    text.textContent = "Graphics stopped because the WebGL context was lost.";
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

  handle_webgl_context_lost(_event) {
    if (this.webgl_context_lost) {
      return;
    }
    // There is deliberately no preventDefault(): Makepad cannot reconstruct
    // all Rust-owned GPU resources yet, so claiming browser restoration would
    // leave a subtly broken app. A reload only happens after the user asks.
    this.webgl_context_lost = true;
    try {
      this.reset_active_render_target_textures();
    } catch (error) {
      console.error(`makepad: terminal render cleanup failed: ${error}`);
    }
    this.render_target_rejected = true;
    try {
      this.stop_webgl_runtime();
    } catch (error) {
      console.error(`makepad: terminal runtime cleanup failed: ${error}`);
    }
    const diagnostic = {
      physical_dpr: this.physical_device_dpi,
      effective_dpr: this.window_info && this.window_info.dpi_factor,
      canvas_width: this.canvas.width,
      canvas_height: this.canvas.height,
      css_width: this.window_info && this.window_info.inner_width,
      css_height: this.window_info && this.window_info.inner_height,
    };
    console.error(
      "makepad: WebGL context lost; rendering stopped until an explicit reload",
      diagnostic,
    );
    this.show_webgl_context_lost_message();
  }

  init_webgl_context() {
    if (this._webgl_context_initialized) {
      return !!this.gl && !this.webgl_context_lost;
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
    // Query immutable hardware ceilings exactly once, immediately after the
    // context exists, then share the cached values with canvas and targets.
    this.webgl_limits = makepad_query_webgl_limits(gl);
    this.max_vertex_attribs = makepad_webgl_vertex_attrib_limit(
      gl.getParameter(gl.MAX_VERTEX_ATTRIBS),
    );
    this.bind_device_pixel_ratio_change();

    // With this extension compileShader/linkProgram only enqueue driver work.
    // Querying COMPILE_STATUS or LINK_STATUS before completion would turn the
    // operation synchronous again, so completion is polled from animation
    // frames and unfinished draws are skipped.
    this.parallel_shader_compile = gl.getExtension(
      "KHR_parallel_shader_compile",
    );

    // Float color targets (RenderRf32): WebGL2 needs this extension for
    // R32F to be color-renderable. Requested up front so a missing GPU
    // capability is one hard, greppable error instead of a silent black
    // texture deep in a bake.
    this.ext_color_buffer_float = gl.getExtension("EXT_color_buffer_float");
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

    this.gpu_info = {
      min_uniforms: Math.min(max_vertex_uniforms, max_fragment_uniforms),
      vendor: "unknown",
      renderer: "unknown",
    };
    let debug_info = gl.getExtension("WEBGL_debug_renderer_info");

    if (debug_info) {
      this.gpu_info.vendor = gl.getParameter(debug_info.UNMASKED_VENDOR_WEBGL);
      this.gpu_info.renderer = gl.getParameter(
        debug_info.UNMASKED_RENDERER_WEBGL,
      );
    }
    return true;
  }
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

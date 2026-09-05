import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { performance as nodePerformance } from "node:perf_hooks";
import test from "node:test";

const source = readFileSync(new URL("./web_gl.js", import.meta.url), "utf8")
  .replace(/^import \{[\s\S]*?\} from "\.\/web\.js";\n/, "")
  .replace(/^export /gm, "");
const load = new Function(
  "MAKEPAD_WEBGL_PIXEL_BUDGET",
  "WasmWebBrowser",
  "makepad_compute_webgl_size",
  "makepad_device_pixel_ratio",
  "window",
  "document",
  "performance",
  "console",
  `${source}\nreturn { WasmWebGL };`,
);
const messages = [];
const { WasmWebGL } = load(
  2 * 1024 * 1024,
  class {},
  () => ({ width: 1, height: 1, scale: 1 }),
  () => 1,
  {},
  {},
  { now: () => 0 },
  { log() {}, warn() {}, error(...parts) { messages.push(parts); } },
);

function mock_gl() {
  let object_id = 0;
  const calls = {
    buffer_data: [],
    buffer_sub_data: [],
    pointers: [],
    integer_pointers: [],
    draws: [],
    created_vaos: [],
    deleted_vaos: [],
    get_errors: 0,
    get_parameters: 0,
  };
  const gl = {
    ARRAY_BUFFER: 1,
    ELEMENT_ARRAY_BUFFER: 2,
    UNIFORM_BUFFER: 3,
    STATIC_DRAW: 4,
    DYNAMIC_DRAW: 5,
    FLOAT: 10,
    HALF_FLOAT: 11,
    UNSIGNED_SHORT: 12,
    SHORT: 13,
    UNSIGNED_BYTE: 14,
    BYTE: 15,
    UNSIGNED_INT: 16,
    INT: 17,
    MAX_VERTEX_ATTRIBS: 18,
    NO_ERROR: 0,
    TRIANGLES: 20,
    CULL_FACE: 21,
    BACK: 22,
    CW: 23,
    CCW: 24,
    TEXTURE_2D: 25,
    TEXTURE_CUBE_MAP: 26,
    TEXTURE0: 100,
    FRAMEBUFFER: 101,
    FRAMEBUFFER_COMPLETE: 102,
    COLOR_ATTACHMENT0: 103,
    DEPTH_STENCIL_ATTACHMENT: 104,
    COLOR_BUFFER_BIT: 1 << 8,
    DEPTH_BUFFER_BIT: 1 << 9,
    STENCIL_BUFFER_BIT: 1 << 10,
    RGBA: 105,
    LINEAR: 106,
    NEAREST: 107,
    TEXTURE_MAG_FILTER: 108,
    TEXTURE_MIN_FILTER: 109,
    TEXTURE_WRAP_S: 110,
    TEXTURE_WRAP_T: 111,
    CLAMP_TO_EDGE: 112,
    DEPTH24_STENCIL8: 113,
    DEPTH_STENCIL: 114,
    UNSIGNED_INT_24_8: 115,
    OUT_OF_MEMORY: 1285,
    fail_buffer_allocation: false,
    fail_vao_allocation: false,
    next_error: 0,
    createBuffer() {
      return this.fail_buffer_allocation ? null : { id: ++object_id };
    },
    createVertexArray() {
      if (this.fail_vao_allocation) return null;
      const vao = { id: ++object_id };
      calls.created_vaos.push(vao);
      return vao;
    },
    deleteVertexArray(vao) { calls.deleted_vaos.push(vao); },
    createFramebuffer() { return { id: ++object_id }; },
    createTexture() { return { id: ++object_id }; },
    bindBuffer() {},
    bufferData(target, data, usage) {
      calls.buffer_data.push([target, data.byteLength, usage]);
    },
    bufferSubData(target, offset, data) {
      calls.buffer_sub_data.push([target, offset, data.byteLength]);
    },
    getError() {
      calls.get_errors += 1;
      const error = this.next_error;
      this.next_error = this.NO_ERROR;
      return error;
    },
    getParameter(parameter) {
      calls.get_parameters += 1;
      if (parameter === this.MAX_VERTEX_ATTRIBS) return 8;
      return 0;
    },
    bindFramebuffer() {},
    texParameteri() {},
    texImage2D() {},
    framebufferTexture2D() {},
    checkFramebufferStatus() { return this.FRAMEBUFFER_COMPLETE; },
    viewport() {},
    clearColor() {},
    clearDepth() {},
    clear() {},
    bindVertexArray() {},
    vertexAttribPointer(...args) { calls.pointers.push(args); },
    vertexAttribIPointer(...args) { calls.integer_pointers.push(args); },
    enableVertexAttribArray() {},
    vertexAttribDivisor() {},
    useProgram() {},
    depthMask() {},
    enable() {},
    disable() {},
    cullFace() {},
    frontFace() {},
    bindBufferBase() {},
    activeTexture() {},
    bindTexture() {},
    uniform1i() {},
    drawElementsInstanced(...args) { calls.draws.push(args); },
  };
  gl.calls = calls;
  return gl;
}

function subject() {
  const gl = mock_gl();
  return Object.assign(Object.create(WasmWebGL.prototype), {
    gl,
    memory: { buffer: new ArrayBuffer(4096) },
    array_buffers: [],
    index_buffers: [],
    vaos: [],
    draw_shaders: [],
    textures: [],
    framebuffers: [],
    active_render_target_textures: new Set(),
    webgl_context_lost: false,
    render_target_rejected: false,
    texture_pass_front_face_cw: false,
    xr: undefined,
    ext_color_buffer_float: {},
    webgl_limits: { max_width: 1024, max_height: 1024 },
    canvas: { width: 1024, height: 768 },
    max_vertex_attribs: 8,
    _missing_shader_ids: new Set(),
    _vertex_submission_reports: new Set(),
    _render_target_size_reports: new Set(),
    ensure_render_quality() { return { pixel_budget: 2 * 1024 * 1024 }; },
  });
}

const empty = () => ({ ptr: 0, len: 0 });
const f32_upload = (buffer_id, ptr, len) => ({
  buffer_id,
  data: { ptr, len },
  byte_data: empty(),
});
const byte_upload = (buffer_id, ptr, len) => ({
  buffer_id,
  data: empty(),
  byte_data: { ptr, len },
});
const u32_indices = (buffer_id, ptr, len) => ({
  buffer_id,
  data: { ptr, len },
  byte_data: empty(),
  index_width: 4,
});
const u16_indices = (buffer_id, ptr, len) => ({
  buffer_id,
  data: empty(),
  byte_data: { ptr, len },
  index_width: 2,
});

function attr(gl, loc, size, stride, offset, type_code = 0, integer = false) {
  const types = [
    gl.FLOAT,
    gl.HALF_FLOAT,
    gl.UNSIGNED_SHORT,
    gl.SHORT,
    gl.UNSIGNED_BYTE,
    gl.BYTE,
    gl.UNSIGNED_INT,
    gl.INT,
  ];
  return {
    loc,
    size,
    stride,
    offset,
    type_code,
    gl_type: types[type_code],
    integer,
    normalized: false,
  };
}

function shader(gl, geom_attribs, inst_attribs, version = 1) {
  return {
    pending: false,
    compile_failed: false,
    program: { id: `program-${version}` },
    version,
    geom_attribs,
    inst_attribs,
    texture_locs: [],
    geometry_slots: 99,
    instance_slots: 99,
    pass_uniforms_binding: null,
    draw_list_uniforms_binding: null,
    draw_call_uniforms_binding: null,
    user_uniforms_binding: null,
    live_uniforms_binding: null,
    pass_uniform_buf: { id: "pass" },
    draw_list_uniform_buf: { id: "list" },
    live_uniform_buf: { id: "live" },
    uniform_buffers_valid: true,
  };
}

function draw_args(index_width = 4, shader_id = 1, vao_id = 1) {
  return {
    shader_id,
    vao_id,
    index_width,
    depth_write: true,
    backface_culling: false,
    pass_uniforms: empty(),
    draw_list_uniforms: empty(),
    draw_call_uniforms: empty(),
    user_uniforms: empty(),
    live_uniforms: empty(),
    pass_uniforms_gen_lo: 1,
    pass_uniforms_gen_hi: 0,
    draw_list_uniforms_gen_lo: 1,
    draw_list_uniforms_gen_hi: 0,
    draw_call_uniforms_gen_lo: 1,
    draw_call_uniforms_gen_hi: 0,
    user_uniforms_gen_lo: 1,
    user_uniforms_gen_hi: 0,
    live_uniforms_gen_lo: 1,
    live_uniforms_gen_hi: 0,
    reset_draw_uniforms: false,
    textures: [],
  };
}

function alloc_vao(s, shader_id = 1) {
  s.FromWasmAllocVao({
    vao_id: 1,
    shader_id,
    geom_ib_id: 1,
    geom_vb_id: 1,
    inst_vb_id: 2,
  });
}

function prepare_triangle(s, geom_attribs, inst_attribs) {
  new Uint8Array(s.memory.buffer, 64, 24).fill(1);
  new Uint8Array(s.memory.buffer, 128, 8).fill(1);
  new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
  const draw_shader = shader(s.gl, geom_attribs, inst_attribs);
  s.draw_shaders[1] = draw_shader;
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
  s.FromWasmAllocArrayBuffer(byte_upload(2, 128, 8));
  s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
  alloc_vao(s);
  return draw_shader;
}

function render_texture_args(texture_id, depth_texture_id) {
  return {
    pass_id: 1,
    width: 1,
    height: 1,
    color_targets: [{
      texture_id,
      format: 0,
      init_only: false,
      clear_color: { r: 0, g: 0, b: 0, a: 0 },
    }],
    depth_target: depth_texture_id === undefined
      ? { attached: false }
      : { attached: true, texture_id: depth_texture_id, clear_depth: 1 },
  };
}

test("valid f32 geometry keeps NaN bit payloads and draws with uploaded u32 type", () => {
  const s = subject();
  new Float32Array(s.memory.buffer, 64, 6).set([0, 0, 1, 0, Number.NaN, 1]);
  new Float32Array(s.memory.buffer, 128, 8).fill(1);
  new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
  s.draw_shaders[1] = shader(
    s.gl,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 4, 16, 0)],
  );
  s.FromWasmAllocArrayBuffer(f32_upload(1, 64, 6));
  s.FromWasmAllocArrayBuffer(f32_upload(2, 128, 8));
  s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
  alloc_vao(s);
  s.FromWasmDrawCall(draw_args());

  assert.deepEqual(s.gl.calls.draws, [[s.gl.TRIANGLES, 3, s.gl.UNSIGNED_INT, 0, 2]]);
  assert.equal(s.index_buffers[1].max_index, 2);
  assert.equal(s.array_buffers[1].byte_length, 24);
  assert.equal(s.gl.calls.pointers.length, 2);
});

test("compact f16/u8/u16 attributes use physical byte stride for instance count", () => {
  const s = subject();
  new Uint8Array(s.memory.buffer, 64, 36).fill(1);
  new Uint8Array(s.memory.buffer, 128, 36).fill(2);
  new Uint16Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
  s.draw_shaders[1] = shader(
    s.gl,
    [
      attr(s.gl, 0, 2, 12, 0, 1),
      attr(s.gl, 1, 4, 12, 4, 4),
      attr(s.gl, 2, 2, 12, 8, 2),
      attr(s.gl, -1, 1, 12, 0, 4),
    ],
    [
      attr(s.gl, 3, 2, 12, 0, 1),
      attr(s.gl, 4, 4, 12, 4, 4),
      attr(s.gl, 5, 1, 12, 8, 6, true),
    ],
  );
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 36));
  s.FromWasmAllocArrayBuffer(byte_upload(2, 128, 36));
  s.FromWasmAllocIndexBuffer(u16_indices(1, 256, 6));
  alloc_vao(s);
  s.FromWasmDrawCall(draw_args(2));

  assert.deepEqual(s.gl.calls.draws[0], [s.gl.TRIANGLES, 3, s.gl.UNSIGNED_SHORT, 0, 3]);
  assert.equal(s.gl.calls.pointers.length, 5);
  assert.equal(s.gl.calls.integer_pointers.length, 1);
});

test("u32 and i32 storage can feed float shader attributes", () => {
  for (const type_code of [6, 7]) {
    const s = subject();
    prepare_triangle(
      s,
      [attr(s.gl, 0, 2, 8, 0, type_code, false)],
      [attr(s.gl, 1, 2, 8, 0)],
    );
    s.FromWasmDrawCall(draw_args());

    assert.equal(s.gl.calls.draws.length, 1);
    assert.equal(
      s.gl.calls.pointers[0][2],
      type_code === 6 ? s.gl.UNSIGNED_INT : s.gl.INT,
    );
    assert.equal(s.gl.calls.integer_pointers.length, 0);
  }
});

test("same-size buffer updates avoid GL error queries while allocation errors invalidate", () => {
  const s = subject();
  new Uint8Array(s.memory.buffer, 64, 16).fill(1);
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 8));
  assert.equal(s.gl.calls.get_errors, 1);

  for (let i = 0; i < 8; i++) {
    s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 8));
  }
  assert.equal(s.gl.calls.buffer_sub_data.length, 8);
  assert.equal(s.gl.calls.get_errors, 1);
  assert.equal(s.array_buffers[1].valid, true);

  s.gl.next_error = s.gl.OUT_OF_MEMORY;
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 12));
  assert.equal(s.gl.calls.get_errors, 2);
  assert.equal(s.array_buffers[1].valid, false);
  assert.equal(s.array_buffers[1].gl_buf._buffer_byte_length, undefined);
});

test("active sampler feedback rejects before draw and canvas reset permits sampling", () => {
  const s = subject();
  const draw_shader = prepare_triangle(
    s,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  draw_shader.texture_locs = [{ name: "source", ty: "sampler2D", loc: { id: 1 } }];
  const active_texture = { id: "active" };
  const active_depth_texture = { id: "active-depth" };
  const other_texture = { id: "other", _render_target_valid: true };
  s.textures[7] = active_texture;
  s.textures[8] = other_texture;
  s.textures[9] = active_depth_texture;
  s.FromWasmBeginRenderTexture(render_texture_args(7, 9));
  assert.equal(s.active_render_target_textures.has(active_texture), true);
  assert.equal(s.active_render_target_textures.has(active_depth_texture), true);

  const args = draw_args();
  args.textures = [7];
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 0);

  args.textures = [9];
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 0);

  draw_shader.texture_locs[0].loc = null;
  args.textures = [7];
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 1);

  draw_shader.texture_locs[0].loc = { id: 1 };
  args.textures = [8];
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 2);

  s.FromWasmBeginRenderCanvas({
    clear_color: { r: 0, g: 0, b: 0, a: 0 },
    clear_depth: 1,
  });
  args.textures = [7];
  s.FromWasmDrawCall(args);
  assert.equal(s.active_render_target_textures.size, 0);
  assert.equal(s.gl.calls.draws.length, 3);
});

test("missing sampler textures defer quietly, recover, and invalid targets reject", () => {
  const s = subject();
  const draw_shader = prepare_triangle(
    s,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  draw_shader.texture_locs = [{ name: "source", ty: "sampler2D", loc: { id: 1 } }];
  const args = draw_args();

  s.FromWasmDrawCall(args);
  args.textures = [21];
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 0);
  assert.equal(s._vertex_submission_reports.size, 0);

  s.textures[21] = { id: "loaded" };
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 1);

  s.textures[21]._render_target_valid = false;
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 1);
  assert.equal(s._vertex_submission_reports.size, 1);
  assert.match([...s._vertex_submission_reports][0], /texture 21 is invalid/);
});

test("bad pointers, unaligned indices, and over-limit buffers fail before allocation", () => {
  const cases = [
    (s) => s.FromWasmAllocArrayBuffer(f32_upload(1, 4092, 2)),
    (s) => s.FromWasmAllocIndexBuffer(u16_indices(1, 65, 2)),
    (s) => s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 64 * 1024 * 1024 + 1)),
  ];
  for (const run of cases) {
    const s = subject();
    run(s);
    const buffer = s.array_buffers[1] || s.index_buffers[1];
    assert.equal(buffer.valid, false);
    assert.equal(buffer.gl_buf, null);
    assert.equal(s.gl.calls.buffer_data.length, 0);
  }
});

test("out-of-range indices and u16/u32 draw mismatches never configure or draw", () => {
  for (const mismatch of [false, true]) {
    const s = subject();
    new Uint8Array(s.memory.buffer, 64, 24).fill(1);
    new Uint8Array(s.memory.buffer, 128, 8).fill(1);
    new Uint16Array(s.memory.buffer, 256, 3).set(mismatch ? [0, 1, 2] : [0, 1, 3]);
    s.draw_shaders[1] = shader(
      s.gl,
      [attr(s.gl, 0, 2, 8, 0)],
      [attr(s.gl, 1, 2, 8, 0)],
    );
    s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
    s.FromWasmAllocArrayBuffer(byte_upload(2, 128, 8));
    s.FromWasmAllocIndexBuffer(u16_indices(1, 256, 6));
    alloc_vao(s);
    s.FromWasmDrawCall(draw_args(mismatch ? 4 : 2));
    assert.equal(s.gl.calls.pointers.length, 0);
    assert.equal(s.gl.calls.draws.length, 0);
  }
});

test("fixed primitive-restart sentinels poison both u16 and u32 uploads", () => {
  const s = subject();
  new Uint16Array(s.memory.buffer, 64, 1)[0] = 0xffff;
  new Uint32Array(s.memory.buffer, 128, 1)[0] = 0xffffffff;
  s.FromWasmAllocIndexBuffer(u16_indices(1, 64, 2));
  s.FromWasmAllocIndexBuffer(u32_indices(2, 128, 1));
  assert.equal(s.index_buffers[1].valid, false);
  assert.equal(s.index_buffers[2].valid, false);
  assert.equal(s.gl.calls.buffer_data.length, 0);
});

test("malformed stride, attribute extent, and integer fetch fail before pointers", () => {
  const bad_attrs = [
    attr(mock_gl(), 0, 2, 256, 0),
    attr(mock_gl(), 0, 4, 16, 4),
    attr(mock_gl(), 0, 1, 4, 0, 0, true),
    attr(mock_gl(), 0, 1, 2, 0, 1, true),
    attr(mock_gl(), 0, 5, 20, 0),
    attr(mock_gl(), 0, 2, 8, 0, 8),
    attr(mock_gl(), 0, 2, 8, 1, 1),
    attr(mock_gl(), 8, 2, 8, 0),
    attr(mock_gl(), 1, 2, 8, 0),
  ];
  for (const bad_attr of bad_attrs) {
    const s = subject();
    bad_attr.gl_type = bad_attr.type_code === 0 ? s.gl.FLOAT : bad_attr.gl_type;
    new Uint8Array(s.memory.buffer, 64, 24).fill(1);
    new Uint8Array(s.memory.buffer, 128, 8).fill(1);
    new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
    s.draw_shaders[1] = shader(s.gl, [bad_attr], [attr(s.gl, 1, 2, 8, 0)]);
    s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
    s.FromWasmAllocArrayBuffer(byte_upload(2, 128, 8));
    s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
    alloc_vao(s);
    s.FromWasmDrawCall(draw_args());
    assert.equal(s.gl.calls.pointers.length, 0);
    assert.equal(s.gl.calls.draws.length, 0);
  }
});

test("missing buffers, bad UBO ranges, and partial instance records fail closed", () => {
  const run = (kind) => {
    const s = subject();
    s.draw_shaders[1] = shader(
      s.gl,
      [attr(s.gl, 0, 2, 8, 0)],
      [attr(s.gl, 1, 2, 8, 0)],
    );
    if (kind !== "missing") {
      new Uint8Array(s.memory.buffer, 64, 24).fill(1);
      new Uint8Array(s.memory.buffer, 128, kind === "partial" ? 10 : 8).fill(1);
      new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
      s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
      s.FromWasmAllocArrayBuffer(byte_upload(2, 128, kind === "partial" ? 10 : 8));
      s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
    }
    alloc_vao(s);
    const args = draw_args();
    if (kind === "ubo") args.pass_uniforms = { ptr: 4092, len: 2 };
    s.FromWasmDrawCall(args);
    assert.equal(s.gl.calls.pointers.length, 0);
    assert.equal(s.gl.calls.draws.length, 0);
  };
  run("missing");
  run("ubo");
  run("partial");
});

test("valid empty buffers allocate explicitly and submit a zero-count draw", () => {
  const s = subject();
  s.draw_shaders[1] = shader(
    s.gl,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  s.FromWasmAllocArrayBuffer(f32_upload(1, 0, 0));
  s.FromWasmAllocArrayBuffer(f32_upload(2, 0, 0));
  s.FromWasmAllocIndexBuffer(u32_indices(1, 0, 0));
  alloc_vao(s);
  s.FromWasmDrawCall(draw_args());
  assert.equal(s.gl.calls.buffer_data.length, 3);
  assert.deepEqual(s.gl.calls.draws[0], [s.gl.TRIANGLES, 0, s.gl.UNSIGNED_INT, 0, 0]);
});

test("expanded triangle metadata admits the exact boundary and rejects over it once", () => {
  const limit = 16 * 1024 * 1024;
  for (const [instances, allowed] of [[limit, true], [limit + 1, false]]) {
    const s = subject();
    prepare_triangle(
      s,
      [attr(s.gl, 0, 2, 8, 0)],
      [attr(s.gl, 1, 2, 8, 0)],
    );
    // Exercise preflight metadata without allocating a correspondingly huge
    // JS/Wasm buffer in this unit test.
    s.index_buffers[1].length = 3;
    s.array_buffers[2].byte_length = instances * 8;
    s.FromWasmDrawCall(draw_args());
    s.FromWasmDrawCall(draw_args());

    assert.equal(s.gl.calls.draws.length, allowed ? 2 : 0);
    assert.equal(s.gl.calls.pointers.length, allowed ? 2 : 0);
    assert.equal(s._vertex_submission_reports.size, allowed ? 0 : 1);
  }
});

test("invalid updates block stale data, valid recovery resumes, and shader changes recreate VAOs", () => {
  const s = subject();
  new Uint8Array(s.memory.buffer, 64, 24).fill(1);
  new Uint8Array(s.memory.buffer, 128, 8).fill(1);
  new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
  s.draw_shaders[1] = shader(
    s.gl,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
  s.FromWasmAllocArrayBuffer(byte_upload(2, 128, 8));
  s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
  alloc_vao(s);
  s.FromWasmDrawCall(draw_args());

  s.FromWasmAllocArrayBuffer(f32_upload(1, 65, 1));
  assert.equal(s.array_buffers[1].valid, false);
  assert.equal(s.array_buffers[1].upload_version, 2);
  s.FromWasmDrawCall(draw_args());
  assert.equal(s.gl.calls.draws.length, 1);

  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
  s.FromWasmDrawCall(draw_args());
  assert.equal(s.array_buffers[1].valid, true);
  assert.equal(s.array_buffers[1].upload_version, 3);
  assert.equal(s.gl.calls.draws.length, 2);
  assert.equal(s.gl.calls.created_vaos.length, 1);

  s.draw_shaders[1] = shader(
    s.gl,
    [attr(s.gl, 2, 2, 8, 0)],
    [attr(s.gl, 3, 2, 8, 0)],
    2,
  );
  s.FromWasmDrawCall(draw_args());
  assert.equal(s.gl.calls.draws.length, 3);
  assert.equal(s.gl.calls.created_vaos.length, 2);
  assert.equal(s.gl.calls.deleted_vaos.length, 1);
});

test("repeated draws reuse cached index/layout validation and configured VAO", (t) => {
  const s = subject();
  const make_view = s.make_validated_wasm_view;
  let index_reads = 0;
  s.make_validated_wasm_view = function(...args) {
    const checked = make_view.call(this, ...args);
    if (checked.ok && args[3] === "u32 index data") {
      checked.array = new Proxy(checked.array, {
        get(target, property) {
          if (typeof property === "string" && /^\d+$/.test(property)) {
            index_reads += 1;
          }
          const value = Reflect.get(target, property, target);
          return typeof value === "function" ? value.bind(target) : value;
        },
      });
    }
    return checked;
  };
  const draw_shader = prepare_triangle(
    s,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  const args = draw_args();
  s.FromWasmDrawCall(args);

  const cached_layout = draw_shader.attrib_layout;
  const reads_after_upload = index_reads;
  const created_vaos = s.gl.calls.created_vaos.length;
  const pointers = s.gl.calls.pointers.length;
  const get_errors = s.gl.calls.get_errors;
  const get_parameters = s.gl.calls.get_parameters;
  draw_shader.geom_attribs = new Proxy([], {
    get() { throw new Error("cached geometry layout was rescanned"); },
  });
  draw_shader.inst_attribs = new Proxy([], {
    get() { throw new Error("cached instance layout was rescanned"); },
  });
  new Uint32Array(s.memory.buffer, 256, 3).fill(0xffffffff);

  const iterations = 2000;
  const started = nodePerformance.now();
  for (let i = 0; i < iterations; i++) {
    s.FromWasmDrawCall(args);
  }
  const elapsed = nodePerformance.now() - started;

  assert.equal(draw_shader.attrib_layout, cached_layout);
  assert.equal(index_reads, reads_after_upload);
  assert.equal(s.gl.calls.created_vaos.length, created_vaos);
  assert.equal(s.gl.calls.pointers.length, pointers);
  assert.equal(s.gl.calls.get_errors, get_errors);
  assert.equal(s.gl.calls.get_parameters, get_parameters);
  assert.equal(s.gl.calls.draws.length, iterations + 1);
  t.diagnostic(`${iterations} cached mocked draws: ${elapsed.toFixed(2)} ms CPU`);
});

test("pending shaders preserve async readiness and VAO allocation failures skip draws", () => {
  const s = subject();
  s.draw_shaders[1] = { pending: true };
  alloc_vao(s);
  s.FromWasmDrawCall(draw_args());
  assert.equal(s.gl.calls.pointers.length, 0);
  assert.equal(s.gl.calls.created_vaos.length, 0);

  new Uint8Array(s.memory.buffer, 64, 24).fill(1);
  new Uint8Array(s.memory.buffer, 128, 8).fill(1);
  new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
  s.FromWasmAllocArrayBuffer(byte_upload(2, 128, 8));
  s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
  s.draw_shaders[1] = shader(
    s.gl,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  s.gl.fail_vao_allocation = true;
  s.FromWasmDrawCall(draw_args());
  assert.equal(s.gl.calls.pointers.length, 0);
  assert.equal(s.gl.calls.draws.length, 0);
});

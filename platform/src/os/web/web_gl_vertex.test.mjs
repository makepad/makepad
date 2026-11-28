import { assert, read_text, test } from "makepad-web-test";

const source = read_text(new URL("./web_gl.js", import.meta.url))
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
const logs = [];
const { WasmWebGL } = load(
  2 * 1024 * 1024,
  class {},
  () => ({ width: 1, height: 1, scale: 1 }),
  () => 1,
  {},
  {},
  { now: () => 0 },
  { log(...parts) { logs.push(parts.join(" ")); }, warn() {}, error(...parts) { messages.push(parts); } },
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
    created_textures: [],
    deleted_textures: [],
    texture_binds: [],
    texture_images: [],
    get_errors: 0,
    get_parameters: 0,
  };
  const gl = {
    ARRAY_BUFFER: 1,
    ELEMENT_ARRAY_BUFFER: 2,
    UNIFORM_BUFFER: 3,
    STATIC_DRAW: 4,
    DYNAMIC_DRAW: 5,
    STREAM_DRAW: 6,
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
    FRONT: 33,
    CW: 23,
    CCW: 24,
    TEXTURE_2D: 25,
    TEXTURE_CUBE_MAP: 26,
    TEXTURE_CUBE_MAP_POSITIVE_X: 27,
    TEXTURE_CUBE_MAP_NEGATIVE_X: 28,
    TEXTURE_CUBE_MAP_POSITIVE_Y: 29,
    TEXTURE_CUBE_MAP_NEGATIVE_Y: 30,
    TEXTURE_CUBE_MAP_POSITIVE_Z: 31,
    TEXTURE_CUBE_MAP_NEGATIVE_Z: 32,
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
    TEXTURE_WRAP_R: 116,
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
    deleteBuffer() {},
    deleteVertexArray(vao) { calls.deleted_vaos.push(vao); },
    createFramebuffer() { return { id: ++object_id }; },
    createTexture() {
      const texture = { id: ++object_id };
      calls.created_textures.push(texture);
      return texture;
    },
    deleteTexture(texture) { calls.deleted_textures.push(texture); },
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
    texImage2D(...args) { calls.texture_images.push(args); },
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
    depthFunc() {},
    depthMask() {},
    enable() {},
    disable() {},
    cullFace() {},
    frontFace() {},
    bindBufferBase() {},
    activeTexture() {},
    bindSampler() {},
    bindTexture(...args) { calls.texture_binds.push(args); },
    uniform1i() {},
    drawElementsInstanced(...args) { calls.draws.push(args); },
  };
  gl.calls = calls;
  return gl;
}

// `gl_checks`: the page's GL error queries (window.makepad_gl_checks).
function subject(gl_checks = true) {
  const gl = mock_gl();
  return Object.assign(Object.create(WasmWebGL.prototype), {
    gl,
    gl_checks,
    instance_slabs: new Map(),
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
    webgl_limits: { max_width: 1024, max_height: 1024, max_vertex_texture_units: 8, max_fragment_texture_units: 8, max_combined_texture_units: 16 },
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
  };
}

function draw_args(index_width = 4, instance_ranges = [{ first: 0, count: 1 }]) {
  return {
    shader_id: 1,
    vao_id: 1,
    index_width,
    instance_ranges,
    depth_write: true,
    cull_mode: 0,
    pass_uniforms: { off: 0, len: 0 },
    draw_list_uniforms: { off: 0, len: 0 },
    draw_call_uniforms: { off: 0, len: 0 },
    user_uniforms: { off: 0, len: 0 },
    live_uniforms: { off: 0, len: 0 },
    textures: [],
  };
}

// The fixtures' instance rows: region 0 of slab slot 0, as the Rust side writes and binds them.
const fixture_slab = { slab_slot: 0, backing_generation: 1 };
const fixture_rows = byte_len => ({ ...fixture_slab, region: { index: 0, gen: 1 }, byte_off: 0, byte_len });

function instance_rows(s, ptr, byte_len) {
  if (!s.instance_slabs.has(fixture_slab.slab_slot)) {
    s.FromWasmCreateInstanceSlab({ ...fixture_slab, capacity_bytes: Math.max(64, byte_len) });
  }
  s.FromWasmWriteInstanceRegion({ ...fixture_rows(byte_len), parts: [{ ptr, len: byte_len }] });
}

function alloc_vao(s, shader_id = 1, byte_len = 8) {
  s.FromWasmAllocVao({
    vao_id: 1,
    shader_id,
    geom_ib_id: 1,
    geom_vb_id: 1,
    ...fixture_rows(byte_len),
    instance_byte_offset: 0,
  });
}

function prepare_triangle(s, geom_attribs, inst_attribs) {
  new Uint8Array(s.memory.buffer, 64, 24).fill(1);
  new Uint8Array(s.memory.buffer, 128, 8).fill(1);
  new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
  const draw_shader = shader(s.gl, geom_attribs, inst_attribs);
  s.draw_shaders[1] = draw_shader;
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
  instance_rows(s, 128, 8);
  s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
  alloc_vao(s);
  return draw_shader;
}

test("web_uniform_stream_single_upload_and_bind_cache", () => {
  const s = subject(false);
  const gl = s.gl;
  const mapping = [], ranges = [], bases = [];
  Object.assign(gl, {
    INVALID_INDEX: 0xffff_ffff,
    UNIFORM_BLOCK_DATA_SIZE: 0x8a40,
    getShaderParameter: () => true,
    getProgramParameter: () => true,
    getAttribLocation: (_, name) => name === "packed_geometry_0" ? 0 : 1,
    getUniformLocation: () => null,
    deleteShader() {},
    getUniformBlockIndex: (program, name) => program.blocks.indexOf(name) >>> 0,
    getActiveUniformBlockParameter(program, index, parameter) {
      assert.equal(parameter, gl.UNIFORM_BLOCK_DATA_SIZE);
      return program.id === 1 && program.blocks[index] === "passUniforms" ? 32 : 16;
    },
    uniformBlockBinding(program, index, binding) { mapping.push([program.id, index, binding]); },
    bindBufferRange(target, binding, buf, off, len) { ranges.push([binding, buf, off, len]); },
    bindBufferBase(target, binding, buf) { bases.push([binding, buf]); },
  });
  s.to_wasm = { ToWasmWebGLShadersDone() {} };
  s.schedule_webgl_shader_summary = () => {};
  s.forget_lost_gpu_state = () => {};
  s.gpu_info = { uniform_buffer_offset_alignment: 64 };
  const prepare = () => prepare_triangle(s, [attr(gl, 0, 2, 8, 0)], [attr(gl, 1, 2, 8, 0)]);
  const link = (id, blocks) => {
    assert.equal(s.finish_webgl_shader({
      shader_id: id, program: { id, blocks }, vsh: {}, fsh: {}, status_ms: 0,
      geometry_slots: 2, instance_slots: 2, textures: [],
    }), true);
  };
  const shaderA = ["passUniforms", "draw_listUniforms", "draw_callUniforms", "liveUniforms", "customA"];
  const shaderB = ["customB", "passUniforms", "draw_listUniforms", "draw_callUniforms"];
  const upload = () => s.FromWasmUploadUniformStream({ data: { ptr: 512, len: 13 * 64 } });
  const args = draw_args();
  args.pass_uniforms = { off: 0, len: 16 };
  args.draw_list_uniforms = { off: 64, len: 16 };
  args.live_uniforms = { off: 128, len: 16 };

  prepare();
  link(1, shaderA);
  upload();
  const stream = s.uniform_stream_buf;
  for (let i = 0; i < 10; i++) {
    args.draw_call_uniforms = { off: (3 + i) * 64, len: 16 };
    s.FromWasmDrawCall(args);
  }
  const bindings = point => ranges.filter(([binding]) => binding === point);
  assert.equal(gl.calls.draws.length, 10);
  assert.deepEqual(gl.calls.buffer_data.filter(([target]) => target === gl.UNIFORM_BUFFER),
    [[gl.UNIFORM_BUFFER, 13 * 64, gl.STREAM_DRAW]]);
  assert.equal(gl.calls.buffer_sub_data.filter(([target]) => target === gl.UNIFORM_BUFFER).length, 0);
  assert.deepEqual(bindings(0), [[0, stream, 0, 64]]);
  assert.deepEqual(bindings(1), [[1, stream, 64, 64]]);
  assert.deepEqual(bindings(4), [[4, stream, 128, 64]]);
  assert.deepEqual(bindings(2), Array.from({ length: 10 }, (_, i) => [2, stream, (3 + i) * 64, 64]));

  s.reset_webgl_objects();
  assert.equal(s.uniform_stream_buf, null);
  assert.equal(s.uniform_stream_len, 0);
  assert.equal(s.bound_ubo.length, 0);
  prepare();
  link(1, shaderA);
  link(2, shaderB);
  upload();
  assert.notEqual(s.uniform_stream_buf, stream);
  ranges.length = 0;
  for (const id of [1, 2, 1, 1]) {
    args.shader_id = id;
    args.custom_uniforms = [{ block_name: id === 1 ? "customA" : "customB",
      data: { ptr: 384, len: 16 }, generation_lo: 1, generation_hi: 0 }];
    s.FromWasmDrawCall(args);
  }
  assert.equal(gl.calls.draws.length, 14);
  assert.deepEqual(mapping.filter(([id, index]) => id === 2 && index <= 1),
    [[2, 1, 0], [2, 0, 5]], "fixed_role_bindings_prevent_standard_custom_collision");
  assert.deepEqual(bindings(0), [[0, s.uniform_stream_buf, 0, 64]],
    "both pass declarations share one padded range binding");
  assert.deepEqual(bindings(1), [[1, s.uniform_stream_buf, 64, 64]]);
  const bufA = s.draw_shaders[1].custom_uniform_buffers[0].buffer;
  const bufB = s.draw_shaders[2].custom_uniform_buffers[0].buffer;
  assert.notEqual(bufA, bufB);
  assert.deepEqual(bases, [[5, bufA], [5, bufB], [5, bufA]],
    "custom_block_cache_switches_by_buffer_and_dedups_repeats");
});

test("web_uniform_stream_short_block_empty_range_and_align_sanitization", () => {
  const s = subject(false);
  const gl = s.gl;
  const program = prepare_triangle(s, [attr(gl, 0, 2, 8, 0)], [attr(gl, 1, 2, 8, 0)]);
  program.user_uniforms_binding = 3;
  program.block_sizes = [0, 0, 0, 16, 0];
  const binds = [];
  gl.bindBufferRange = (...args) => binds.push(args);
  gl.bindBufferBase = (...args) => binds.push(args);
  const upload = (align, len) => {
    s.gpu_info = { uniform_buffer_offset_alignment: align };
    s.FromWasmUploadUniformStream({ data: { ptr: 512, len } });
    binds.length = 0;
  };
  const draw = (off, len) => s.FromWasmDrawCall({ ...draw_args(), user_uniforms: { off, len } });

  upload(64, 64);
  const stream = s.uniform_stream_buf;
  draw(0, 8);
  assert.equal(gl.calls.draws.length, 1);
  assert.deepEqual(binds, [[gl.UNIFORM_BUFFER, 3, stream, 0, 64]]);

  upload(16, 32);
  program.block_sizes[3] = 32;
  draw(16, 16);
  draw(0, 16);
  assert.equal(gl.calls.draws.length, 1, "both the tail and middle declarations exceed their stride");
  assert.deepEqual(binds, [], "both draws fail before binding");

  program.block_sizes[3] = 16;
  for (const [raw, align] of [[4, 16], [48, 64]]) {
    upload(raw, 2 * align);
    assert.equal(s.effective_uniform_align(), align);
    const before = gl.calls.draws.length;
    draw(raw, 16);
    assert.equal(gl.calls.draws.length, before);
    assert.deepEqual(binds, []);
    draw(align, 16);
    assert.equal(gl.calls.draws.length, before + 1);
    assert.deepEqual(binds, [[gl.UNIFORM_BUFFER, 3, stream, align, align]]);
  }

  upload(64, 128);
  program.block_sizes[3] = 32;
  const uploads = [];
  gl.bufferData = (...args) => uploads.push(args);
  const before = gl.calls.draws.length;
  draw(64, 32);
  const stream_bind = [gl.UNIFORM_BUFFER, 3, stream, 64, 64];
  assert.deepEqual(binds, [stream_bind]);
  assert.equal(uploads.length, 0);
  draw(0, 0);
  assert.equal(uploads.length, 1);
  const [target, bytes, usage] = uploads[0];
  assert.equal(target, gl.UNIFORM_BUFFER);
  assert.equal(usage, gl.STATIC_DRAW);
  assert.ok(bytes instanceof Uint8Array && bytes.length >= 256 && bytes.every(b => b === 0));
  const zero_bind = [gl.UNIFORM_BUFFER, 3, s.zero_uniform_buf];
  assert.deepEqual(binds, [stream_bind, zero_bind]);
  draw(0, 0);
  assert.deepEqual(binds, [stream_bind, zero_bind], "consecutive empty ranges reuse the zero binding");
  draw(64, 32);
  assert.deepEqual(binds, [stream_bind, zero_bind, stream_bind]);
  assert.equal(uploads.length, 1);
  assert.equal(gl.calls.draws.length, before + 4);
  s.forget_lost_gpu_state = () => {};
  s.reset_webgl_objects();
  assert.equal(s.zero_uniform_buf, null);
});

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
  instance_rows(s, 128, 32);
  s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
  alloc_vao(s, 1, 32);
  s.FromWasmDrawCall(draw_args(4, [{ first: 0, count: 2 }]));

  assert.deepEqual(s.gl.calls.draws, [[s.gl.TRIANGLES, 3, s.gl.UNSIGNED_INT, 0, 2]]);
  assert.equal(s.index_buffers[1].max_index, 2);
  assert.equal(s.array_buffers[1].byte_length, 24);
  assert.equal(s.gl.calls.pointers.length, 2);
  s.FromWasmDrawCall(draw_args());
  assert.deepEqual(s.gl.calls.draws.map(draw => draw[4]), [2, 1], "a shorter prefix draws only its requested rows");
  assert.equal(s.gl.calls.pointers.length, 2, "changing just the prefix count reuses the pointers at zero");
});

test("compact f16/u8/u16 instance ranges use the physical byte stride", () => {
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
  instance_rows(s, 128, 36);
  s.FromWasmAllocIndexBuffer(u16_indices(1, 256, 6));
  alloc_vao(s, 1, 36);
  s.FromWasmDrawCall(draw_args(2, [{ first: 0, count: 3 }]));

  assert.deepEqual(s.gl.calls.draws[0], [s.gl.TRIANGLES, 3, s.gl.UNSIGNED_SHORT, 0, 3]);
  assert.equal(s.gl.calls.pointers.length, 5);
  assert.equal(s.gl.calls.integer_pointers.length, 1);
  s.FromWasmDrawCall(draw_args(2, [{ first: 2, count: 1 }, { first: 0, count: 2 }]));
  assert.deepEqual(s.gl.calls.draws.slice(1).map(draw => draw[4]), [1, 2], "both ranges draw in order with their own counts");
  assert.deepEqual(s.gl.calls.pointers.slice(5).map(p => [p[0], p[5]]),
    [[3, 24], [4, 28], [3, 0], [4, 4]], "only instance float pointers move, including the return to row zero");
  assert.deepEqual(s.gl.calls.integer_pointers.slice(1).map(p => [p[0], p[4]]),
    [[5, 32], [5, 8]], "the integer pointer includes the field offset at each range start");
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

test("updates within the buffer's storage avoid GL error queries while allocation errors invalidate", () => {
  const s = subject();
  new Uint8Array(s.memory.buffer, 64, 512).fill(1);
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 8));
  // The allocation is checked later (one deferred query), not where it happens.
  assert.equal(s.gl.calls.get_errors, 0);
  s.check_allocations(true);
  assert.equal(s.gl.calls.get_errors, 1);

  // Same size and other sizes within the storage (256 bytes) write in place.
  for (let i = 0; i < 8; i++) {
    s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 8 + 8 * i));
  }
  assert.equal(s.gl.calls.buffer_sub_data.length, 9);
  s.check_allocations(true);
  assert.equal(s.gl.calls.get_errors, 1);
  assert.equal(s.array_buffers[1].valid, true);
  assert.equal(s.array_buffers[1].byte_length, 64);

  // Growing past the storage re-specifies it (and checks for errors).
  s.gl.next_error = s.gl.OUT_OF_MEMORY;
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 300));
  assert.equal(s.gl.calls.get_errors, 1);
  assert.equal(s.check_allocations(true), s.gl.OUT_OF_MEMORY);
  assert.equal(s.gl.calls.get_errors, 2);
  assert.equal(s.array_buffers[1].valid, false);
  assert.equal(s.array_buffers[1].gl_buf._buffer_byte_length, undefined);
});

function linked_texture(name, slot, ty, target) {
  return { name, slot, ty, target, loc: { id: name }, unit: slot, sampler: {}, nearest: {} };
}

test("active sampler feedback rejects before draw and canvas reset permits sampling", () => {
  const s = subject();
  const draw_shader = prepare_triangle(
    s,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  draw_shader.texture_locs = [linked_texture("tex_source", 0, "sampler2D", s.gl.TEXTURE_2D)];
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

  const active_binding = draw_shader.texture_locs.pop();
  args.textures = [7];
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 1);

  draw_shader.texture_locs.push(active_binding);
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

test("mixed text samplers use one cached fallback and recover when the atlas arrives", () => {
  const s = subject();
  const draw_shader = prepare_triangle(
    s,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  draw_shader.texture_locs = ["curve", "band", "grayscale", "color", "msdf"]
    .map((name, slot) => linked_texture("tex_" + name, slot, "sampler2D", s.gl.TEXTURE_2D));
  const args = draw_args();
  const curve = { id: "curve", _texture_target: s.gl.TEXTURE_2D };
  const band = { id: "band", _texture_target: s.gl.TEXTURE_2D };
  s.textures[10] = curve;
  s.textures[11] = band;
  args.textures = [10, 11, 21, 21, 21];

  s.FromWasmDrawCall(args);
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 2);
  assert.equal(s._vertex_submission_reports.size, 0);
  assert.equal(s.gl.calls.created_textures.length, 1);
  assert.equal(s.gl.calls.texture_images.length, 1);
  const fallback = s._sampler_fallback_textures.get(s.gl.TEXTURE_2D);
  assert.ok(fallback);
  assert.deepEqual(
    s.gl.calls.texture_binds.slice(-5).map(([, texture]) => texture),
    [curve, band, fallback, fallback, fallback],
  );

  const loaded = { id: "loaded", _texture_target: s.gl.TEXTURE_2D };
  s.textures[21] = loaded;
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 3);
  assert.deepEqual(
    s.gl.calls.texture_binds.slice(-5).map(([, texture]) => texture),
    [curve, band, loaded, loaded, loaded],
  );
  assert.equal(s.gl.calls.created_textures.length, 1);

  loaded._texture_target = s.gl.TEXTURE_CUBE_MAP;
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 3);
  assert.equal(s._vertex_submission_reports.size, 1);
  assert.match([...s._vertex_submission_reports][0], /wrong sampler target/);

  loaded._texture_target = s.gl.TEXTURE_2D;
  loaded._render_target_valid = false;
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 3);
  assert.equal(s._vertex_submission_reports.size, 2);
  assert.match([...s._vertex_submission_reports][1], /texture 21 is invalid/);

  s.textures[21] = undefined;
  s._invalid_texture_upload_ids = new Set([21]);
  s.FromWasmDrawCall(args);
  assert.equal(s.gl.calls.draws.length, 3);
  assert.equal(s.gl.calls.created_textures.length, 1);
  assert.equal(s._vertex_submission_reports.size, 2);
});

test("missing cube samplers get one complete six-face fallback", () => {
  const s = subject();
  const draw_shader = prepare_triangle(
    s,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  draw_shader.texture_locs = [linked_texture("tex_environment", 0, "samplerCube", s.gl.TEXTURE_CUBE_MAP)];
  const args = draw_args();
  args.textures = [31];

  s.FromWasmDrawCall(args);
  s.FromWasmDrawCall(args);

  assert.equal(s.gl.calls.draws.length, 2);
  assert.equal(s.gl.calls.created_textures.length, 1);
  assert.deepEqual(
    s.gl.calls.texture_images.map(([face]) => face),
    [
      s.gl.TEXTURE_CUBE_MAP_POSITIVE_X,
      s.gl.TEXTURE_CUBE_MAP_NEGATIVE_X,
      s.gl.TEXTURE_CUBE_MAP_POSITIVE_Y,
      s.gl.TEXTURE_CUBE_MAP_NEGATIVE_Y,
      s.gl.TEXTURE_CUBE_MAP_POSITIVE_Z,
      s.gl.TEXTURE_CUBE_MAP_NEGATIVE_Z,
    ],
  );
});

test("bad pointers, unaligned indices, and buffers outside wasm memory fail before allocation", () => {
  const cases = [
    (s) => s.FromWasmAllocArrayBuffer(f32_upload(1, 4092, 2)),
    (s) => s.FromWasmAllocIndexBuffer(u16_indices(1, 65, 2)),
    (s) => s.FromWasmAllocArrayBuffer(byte_upload(1, 64, s.memory.buffer.byteLength)),
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
    instance_rows(s, 128, 8);
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
    instance_rows(s, 128, 8);
    s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
    alloc_vao(s);
    s.FromWasmDrawCall(draw_args());
    assert.equal(s.gl.calls.pointers.length, 0);
    assert.equal(s.gl.calls.draws.length, 0);
  }
});

test("missing buffers, bad UBO or instance ranges, and partial records fail closed", () => {
  const run = (kind) => {
    const s = subject();
    s.draw_shaders[1] = shader(
      s.gl,
      [attr(s.gl, 0, 2, 8, 0)],
      [attr(s.gl, 1, 2, 8, 0)],
    );
    if (kind !== "missing") {
      new Uint8Array(s.memory.buffer, 64, 24).fill(1);
      // A partial record: word-aligned, so the write lands, but not a whole 8-byte row.
      new Uint8Array(s.memory.buffer, 128, kind === "partial" ? 12 : 8).fill(1);
      new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
      s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 24));
      instance_rows(s, 128, kind === "partial" ? 12 : 8);
      s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
    }
    alloc_vao(s, 1, kind === "partial" ? 12 : 8);
    const args = draw_args();
    if (kind === "ubo") {
      s.FromWasmUploadUniformStream({ data: { ptr: 512, len: 256 } });
      args.pass_uniforms = { off: 256, len: 16 };
    }
    if (kind === "range") args.instance_ranges.push({ first: 1, count: 1 });
    s.FromWasmDrawCall(args);
    assert.equal(s.gl.calls.pointers.length, 0, `${kind} is rejected before pointer setup`);
    assert.equal(s.gl.calls.draws.length, 0, `${kind} cannot submit even the first range`);
    if (kind === "range") {
      assert.match([...s._vertex_submission_reports][0], /instance range \[1, 2\) exceeds resident row count 1/,
        "preflight reports the range and the resident bound");
    }
  };
  run("missing");
  run("ubo");
  run("partial");
  run("range");
});

test("empty buffers allocate explicitly and an empty range list needs no VAO setup", () => {
  const s = subject();
  s.draw_shaders[1] = shader(
    s.gl,
    [attr(s.gl, 0, 2, 8, 0)],
    [attr(s.gl, 1, 2, 8, 0)],
  );
  s.FromWasmAllocArrayBuffer(f32_upload(1, 0, 0));
  s.FromWasmCreateInstanceSlab({ ...fixture_slab, capacity_bytes: 64 });
  s.FromWasmAllocIndexBuffer(u32_indices(1, 0, 0));
  alloc_vao(s, 1, 0);
  s.FromWasmDrawCall(draw_args(4, []));
  assert.equal(s.gl.calls.buffer_data.length, 3);
  assert.equal(s.gl.calls.created_vaos.length, 0, "an empty draw never configures the GL VAO");
  assert.equal(s.gl.calls.draws.length, 0, "an empty draw submits no ranges");
});

test("expanded triangle counts have no cap of the runtime's own", () => {
  const many = 16 * 1024 * 1024;
  for (const [instances, allowed] of [[many, true], [many + 1, true]]) {
    const s = subject();
    prepare_triangle(
      s,
      [attr(s.gl, 0, 2, 8, 0)],
      [attr(s.gl, 1, 2, 8, 0)],
    );
    // Exercise preflight metadata without allocating a correspondingly huge
    // JS/Wasm buffer in this unit test.
    s.index_buffers[1].length = 3;
    s.instance_slabs.get(fixture_slab.slab_slot).regions.get(0).byte_len = instances * 8;
    s.vaos[1].byte_len = instances * 8;
    const args = draw_args(4, [{ first: 0, count: instances }]);
    s.FromWasmDrawCall(args);
    s.FromWasmDrawCall(args);

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
  instance_rows(s, 128, 8);
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
  const started = performance.now();
  for (let i = 0; i < iterations; i++) {
    s.FromWasmDrawCall(args);
  }
  const elapsed = performance.now() - started;

  assert.equal(draw_shader.attrib_layout, cached_layout);
  assert.equal(index_reads, reads_after_upload);
  assert.equal(s.gl.calls.created_vaos.length, created_vaos);
  assert.equal(s.gl.calls.pointers.length, pointers, "steady draws at row zero never re-point attributes");
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
  instance_rows(s, 128, 8);
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


test("slab writes preserve packed words and reject a bad segment before any part is written", () => {
  const s = subject();
  const gl = s.gl;
  prepare_triangle(s, [attr(gl, 0, 2, 8, 0)], [attr(gl, 1, 2, 8, 0)]);
  const identity = { slab_slot: 1, backing_generation: 3 };
  const first = { ...identity, region: { index: 0, gen: 2 }, byte_off: 0, byte_len: 8 };
  const next = { ...identity, region: { index: 1, gen: 2 }, byte_off: 16, byte_len: 16 };
  const bind = (binding, vao_id) => s.FromWasmAllocVao({
    ...binding, instance_byte_offset: binding.byte_off, vao_id, shader_id: 1, geom_ib_id: 1, geom_vb_id: 1,
  });
  const failures = [];
  s.to_wasm = { ToWasmInstanceUploadFailed: msg => failures.push(msg) };
  const uploads = [], allocations = [];
  gl.bufferData = (...args) => allocations.push(args);
  gl.bufferSubData = (_target, offset, bytes, ptr, len) => {
    uploads.push([offset, ...new Uint32Array(bytes.buffer, ptr, len / 4)]);
  };
  new Uint32Array(s.memory.buffer, 64, 4).set([0x7fc00001, 3, 9, 2]);
  s.check_allocations(true);
  s.FromWasmCreateInstanceSlab({ ...identity, capacity_bytes: 64 });
  s.FromWasmWriteInstanceRegion({ ...first, parts: [{ ptr: 64, len: 8 }] });
  s.FromWasmWriteInstanceRegion({ ...next, parts: [{ ptr: 72, len: 8 }, empty(), { ptr: 64, len: 8 }] });
  assert.deepEqual(allocations, [[gl.ARRAY_BUFFER, 64, gl.DYNAMIC_DRAW]], "one capacity allocation serves both regions");
  assert.deepEqual(uploads, [[0, 0x7fc00001, 3], [16, 9, 2], [24, 0x7fc00001, 3]],
    "parts advance the destination in bytes and preserve packed NaN payloads");
  bind(first, 1);
  bind(next, 2);
  s.FromWasmDrawCall(draw_args());
  assert.equal(gl.calls.draws.length, 1, "the first region draws before rejection");
  const pointers = gl.calls.pointers.length;
  for (const bad of [
    { parts: [{ ptr: 64, len: 4 }, { ptr: 4096, len: 4 }] },
    { parts: [{ ptr: 64, len: 4 }] },
    { byte_off: 60, parts: [{ ptr: 64, len: 8 }] },
  ]) {
    s.FromWasmWriteInstanceRegion({ ...first, ...bad });
    assert.equal(uploads.length, 3, "a bad source, total length or destination writes no part");
    assert.deepEqual(failures.at(-1), { ...identity, scope: 1, region: first.region }, "the rejection identifies only the failed token");
    s.FromWasmDrawCall(draw_args());
  }
  assert.equal(gl.calls.draws.length, 1, "an already configured VAO cannot draw a rejected region");
  assert.equal(gl.calls.pointers.length, pointers, "failed regions are rejected before pointer setup");
  s.FromWasmDrawCall({ ...draw_args(4, [{ first: 1, count: 1 }]), vao_id: 2 });
  assert.equal(gl.calls.draws.length, 2, "the sibling region in the same backing still draws");
  assert.equal(gl.calls.pointers.at(-1)[5], 24, "the attribute pointer includes the sibling region and range offsets");
  s.FromWasmDrawCall({ ...draw_args(4, [{ first: 2, count: 1 }]), vao_id: 2 });
  assert.equal(gl.calls.draws.length, 2, "a range inside the slab but past its region cannot draw");
  assert.equal(gl.calls.created_textures.length, 0, "attribute-only draws never allocate a mirror");
  assert.equal(s.instance_slabs.get(identity.slab_slot).dirty.size, 0, "an attribute-only slab keeps no dirty intervals");

  const new_token = { index: next.region.index, gen: next.region.gen + 1 };
  s.FromWasmWriteInstanceRegion({ ...next, region: new_token, byte_off: 32, parts: [{ ptr: 4096, len: 16 }] });
  assert.deepEqual(failures.at(-1), { ...identity, scope: 1, region: new_token }, "the failed replacement reports the new generation");
  s.FromWasmDrawCall({ ...draw_args(), vao_id: 2 });
  assert.equal(gl.calls.draws.length, 2, "a failed write at a reused index retires the older token bound to the VAO");

  const replacement = { ...identity, backing_generation: 4 };
  const create = { ...replacement, capacity_bytes: 64 };
  s.FromWasmCreateInstanceSlab(create);
  assert.deepEqual(failures.at(-1), { ...replacement, scope: 0, region: undefined },
    "an occupied-slot Create reports the incoming backing identity");
  assert.equal(allocations.length, 1, "a conflicting Create cannot replace live storage");
  const old_vao = s.vaos[2].gl_vao;
  s.FromWasmFreeWebGLResources({ instance_slabs: [identity] });
  s.gl_checks = false; // Leave only the retired backing in the pending allocation check.
  s.FromWasmCreateInstanceSlab(create);
  assert.equal(s.vaos[2], undefined, "Destroy retires the old VAO before Create reuses the slot");
  assert.ok(gl.calls.deleted_vaos.includes(old_vao), "Destroy deletes the configured GL VAO");
  const fresh = { ...next, ...replacement };
  s.FromWasmWriteInstanceRegion({ ...fresh, parts: [{ ptr: 64, len: 16 }] });
  bind(fresh, 2);
  const reports = failures.length;
  gl.next_error = gl.OUT_OF_MEMORY;
  s.check_allocations(true);
  assert.equal(failures.length, reports, "a deferred allocation failure for the old backing sends no new rejection");
  s.FromWasmFreeWebGLResources({ instance_slabs: [identity] });
  s.FromWasmDrawCall({ ...draw_args(), vao_id: 2 });
  assert.equal(gl.calls.draws.length, 3, "a stale free and deferred failure leave the new backing drawable");
  assert.ok(s.vaos[2].gl_vao && s.vaos[2].gl_vao !== old_vao, "the reused VAO slot configures a new GL VAO");

  // Rows that move inside the same backing (a rewritten recording, an evacuation) keep the
  // GL VAO; the draw re-points the instance attributes.
  const kept = s.vaos[2], kept_gl = kept.gl_vao, created = gl.calls.created_vaos.length;
  const moved = { ...replacement, region: { index: 4, gen: 2 }, byte_off: 40, byte_len: 16 };
  s.FromWasmWriteInstanceRegion({ ...moved, parts: [{ ptr: 64, len: 16 }] });
  bind(moved, 2);
  s.FromWasmDrawCall({ ...draw_args(), vao_id: 2 });
  assert.equal(gl.calls.draws.length, 4, "the moved rows draw");
  assert.ok(s.vaos[2] === kept && kept.gl_vao === kept_gl && gl.calls.created_vaos.length === created,
    "a move inside the backing keeps the GL VAO");
  assert.equal(gl.calls.pointers.at(-1)[5], 40, "the instance attribute points at the moved rows");
});

test("a shipped page (no makepad_gl_checks) never queries GL errors or framebuffer status", () => {
  const s = subject(false);
  let status_queries = 0;
  const status = s.gl.checkFramebufferStatus;
  s.gl.checkFramebufferStatus = function (...args) { status_queries += 1; return status.apply(this, args); };
  new Uint8Array(s.memory.buffer, 64, 512).fill(1);
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 8));
  s.FromWasmAllocArrayBuffer(byte_upload(1, 64, 300));
  s.textures[7] = undefined;
  s.FromWasmBeginRenderTexture(render_texture_args(7, 9));
  s.check_allocations(true);
  assert.equal(s.gl.calls.get_errors, 0);
  assert.equal(status_queries, 0);
  assert.equal(s.array_buffers[1].valid, true);
});

const WORD_ROW = 2048;
const WINDOW_WORDS = WORD_ROW * WORD_ROW;

// A GL whose slab buffers keep their bytes and whose R32UI array texture keeps
// its words (layer * WINDOW_WORDS + row * WORD_ROW + column), recording each
// draw's mp_inst_base, attribute offset and instance count.
function mirror_gl(s, mirror_words) {
  const gl = s.gl;
  const fetched_ranges = [];
  const draw = gl.drawElementsInstanced.bind(gl);
  Object.assign(gl, {
    TEXTURE_2D_ARRAY: 30,
    TEXTURE0: 40,
    R32UI: 31,
    RED_INTEGER: 32,
    UNSIGNED_INT: 33,
    PIXEL_UNPACK_BUFFER: 34,
    created: [],
    sub_images: [],
    createTexture() { const t = { id: `t${this.created.length}` }; this.created.push(t); return t; },
    deleteTexture(texture) { this.calls.deleted_textures.push(texture); },
    texImage3D(...args) { this.calls.texture_images.push(args); },
    texSubImage3D(...args) {
      this.sub_images.push(args);
      const [, , x, y, z, w, h, , , , offset] = args;
      mirror_words.set(new Uint32Array(this.unpack_bytes.buffer, offset, w * h), z * WINDOW_WORDS + y * WORD_ROW + x);
    },
    bindBuffer(target, buffer) {
      if (target === this.ARRAY_BUFFER) this.array = buffer;
      if (target === this.PIXEL_UNPACK_BUFFER) this.unpack_bytes = buffer?.bytes;
    },
    bufferData(target, capacity, usage) {
      this.calls.buffer_data.push([target, capacity, usage]);
      if (target === this.ARRAY_BUFFER && this.array?._instance_ledger_role === "slab") this.array.bytes = new Uint8Array(capacity);
    },
    bufferSubData(target, offset, src, ptr, len) {
      this.calls.buffer_sub_data.push([target, offset, len]);
      if (target === this.ARRAY_BUFFER && this.array?._instance_ledger_role === "slab") this.array.bytes.set(src.subarray(ptr, ptr + len), offset);
    },
    createBuffer() { return { id: "buffer" }; },
    deleteBuffer() {},
    deleteVertexArray() {},
    getProgramParameter: () => true,
    getShaderParameter: () => true,
    getUniformLocation: (_, id) => ["mp_inst_data", "mp_inst_base"].includes(id) ? { id } : null,
    INVALID_INDEX: 0xffff_ffff,
    getUniformBlockIndex: () => gl.INVALID_INDEX,
    getAttribLocation: (_, name) => name === "packed_geometry_0" ? 0 : name === "instance_word" ? 1 : -1,
    deleteShader() {},
    inst_data_units: [],
    uniform1i(loc, value) {
      if (loc.id === "mp_inst_base") this.inst_base = value;
      else this.inst_data_units.push([loc, value]);
    },
    viewport(x) { this.eye_x = x; },
    drawElementsInstanced(...args) {
      const pointer = this.calls.pointers.findLast(p => p[0] === 1);
      fetched_ranges.push({ eye: this.eye_x, base: this.inst_base, offset: pointer?.[5], count: args[4] });
      draw(...args);
    },
  });
  return { gl, fetched_ranges };
}

function link_mirror_shader(s, slots, instance_attribute = false) {
  s.to_wasm = { ToWasmWebGLShadersDone() {} };
  const before = s.gl.inst_data_units.length;
  assert.equal(s.finish_webgl_shader({
    shader_id: 1, program: {}, vsh: {}, fsh: {}, status_ms: 0, textures: [],
    geometry_slots: 2, instance_slots: slots,
    inst_attribs: instance_attribute ? [{ name: "instance_word", offset: 0, size: 1, stride: slots * 4, gl_type: 0 }] : [],
  }), true);
  const shader = s.draw_shaders[1];
  assert.deepEqual(s.gl.inst_data_units.slice(before), [[shader.inst_data_loc, 0]], "link assigns the mirror unit once");
  return shader;
}

// Word `i` as the vertex shader's _mp_inst_word reads it: ivec3(i & 2047, (i >> 11) & 2047, i >> 22).
const fetch_word = (mirror_words, i) => mirror_words[(i >> 22) * WINDOW_WORDS + ((i >> 11) & 2047) * WORD_ROW + (i & 2047)];

test("slab regions share a word texture and transfer only dirty words", () => {
  const s = subject();
  const mirror_words = new Uint32Array(WORD_ROW);
  const { gl, fetched_ranges } = mirror_gl(s, mirror_words);
  s.webgl_limits.max_array_texture_layers = 256;
  // A 21-word record read whole from the texture (no instance attributes).
  new Float32Array(s.memory.buffer, 64, 6).set([0, 0, 1, 0, 0, 1]);
  new Uint32Array(s.memory.buffer, 512, 42).set(Array.from({ length: 42 }, (_, i) => i + 100));
  new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
  const draw_shader = link_mirror_shader(s, 21);
  s.FromWasmAllocArrayBuffer(f32_upload(1, 64, 6));
  const identity = { slab_slot: 2, backing_generation: 9 };
  s.FromWasmCreateInstanceSlab({ ...identity, capacity_bytes: 512 });
  const first = { ...identity, region: { index: 0, gen: 0 }, byte_off: 0, byte_len: 168 };
  const bind = (region, offset = region.byte_off) => s.FromWasmAllocVao({
    ...region, instance_byte_offset: offset, vao_id: 1, shader_id: 1, geom_ib_id: 1, geom_vb_id: 1,
  });
  // Write the highest word first so a later write below it must preserve the high-water mark.
  s.FromWasmWriteInstanceRegion({
    ...identity, region: { index: 3, gen: 0 }, byte_off: 176, byte_len: 4, parts: [{ ptr: 512, len: 4 }],
  });
  s.FromWasmWriteInstanceRegion({ ...first, parts: [{ ptr: 512, len: 168 }] });
  assert.equal(s.instance_slabs.get(identity.slab_slot).dirty.size, 0, "writes before the first sampling draw retain no dirty intervals");
  s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
  bind(first);
  s.FromWasmDrawCall(draw_args(4, [{ first: 0, count: 2 }]));

  // The seed reaches word 44 across the alignment gap after the two 21-word instances.
  assert.deepEqual(gl.calls.draws, [[gl.TRIANGLES, 3, gl.UNSIGNED_INT, 0, 2]]);
  assert.equal(gl.created.length, 1);
  const image = gl.calls.texture_images.find((args) => args[2] === gl.R32UI);
  assert.deepEqual(image.slice(0, 6), [gl.TEXTURE_2D_ARRAY, 0, gl.R32UI, WORD_ROW, 1, 1], "a 128-word slab takes one row of one layer");
  assert.deepEqual(gl.sub_images, [
    [gl.TEXTURE_2D_ARRAY, 0, 0, 0, 0, 45, 1, 1, gl.RED_INTEGER, gl.UNSIGNED_INT, 0],
  ], "the first mirror transfer covers one prefix through the highest written word");
  assert.equal(mirror_words[44], 100, "the first sampling draw also seeds the earlier write beyond the drawn region");
  assert.deepEqual(gl.inst_data_units, [[draw_shader.inst_data_loc, 0]], "draw reuses the unit assigned at link");

  // Both eyes read the same two ranges from the existing mirror.
  draw_shader.pass_uniforms_binding = 0;
  const base_binds = [], range_binds = [];
  gl.bindBufferBase = (...args) => base_binds.push(args);
  gl.bindBufferRange = (...args) => range_binds.push(args);
  const eye = x => ({
    viewport: { x, y: 0, width: 1, height: 1 },
    projection_matrix: new Float32Array(16).fill(x + 1),
    transform_matrix: new Float32Array(16).fill(x + 2),
    invtransform_matrix: new Float32Array(16).fill(x + 3),
  });
  s.xr = { in_xr_pass: true, left_eye: eye(0), right_eye: eye(1) };
  const args = draw_args(4, [{ first: 1, count: 1 }, { first: 0, count: 1 }]);
  const pass_words = new Float32Array(s.memory.buffer, 1024 + 256, 49).fill(7);
  s.FromWasmUploadUniformStream({ data: { ptr: 1024, len: 512 } });
  args.pass_uniforms = { off: 256, len: pass_words.byteLength };
  s.FromWasmDrawCall(args);
  assert.deepEqual(fetched_ranges.slice(1).map(r => [r.eye, r.base, r.count]),
    [[0, 1, 1], [0, 0, 1], [1, 1, 1], [1, 0, 1]],
    "each eye draws both ranges with the matching mp_inst_base already bound");
  assert.deepEqual(base_binds, [[gl.UNIFORM_BUFFER, 0, s.xr_pass_uniform_buf]]);
  assert.equal(range_binds.length, 0, "the XR eyes share the override binding");
  assert.deepEqual([0, 16, 32, 48].map(i => s.xr_pass_words[i]), [2, 3, 4, 7]);
  assert.ok(pass_words.every(word => word === 7), "eye matrices preserve the packed pass");
  s.xr = undefined;
  s.FromWasmDrawCall(args);
  assert.deepEqual(range_binds, [[gl.UNIFORM_BUFFER, 0, s.uniform_stream_buf, 256, 256]]);

  // The same two resident rows serve a short prefix and a slice in a mixed shader.
  link_mirror_shader(s, draw_shader.instance_slots, true);
  const before_mixed = fetched_ranges.length;
  s.FromWasmDrawCall(draw_args(4, [{ first: 0, count: 1 }]));
  s.FromWasmDrawCall(draw_args(4, [{ first: 1, count: 1 }]));
  const selections = fetched_ranges.slice(before_mixed);
  assert.deepEqual(selections.map(r => r.count), [1, 1],
    "the prefix and slice each draw one of the buffer's two rows");
  assert.deepEqual(selections.map(r => r.offset), [0, 84],
    "the instance attribute points at the selected 21-word row when each draw runs");
  assert.deepEqual(selections.map(r => r.base), [0, 1],
    "mp_inst_base selects the same row for texture fetch when each draw runs");
  assert.equal(gl.sub_images.length, 1, "selecting ranges leaves the mirror current");

  assert.deepEqual([...mirror_words.slice(0, 42)], [...new Uint32Array(s.memory.buffer, 512, 42)],
    "the first sampling draw seeds the previously written words from the slab PBO");
  // A second, three-word stride begins at word 45.
  const second = { ...identity, region: { index: 1, gen: 4 }, byte_off: 180, byte_len: 72 };
  new Uint32Array(s.memory.buffer, 720, 18).set(Array.from({ length: 18 }, (_, i) => i + 200));
  s.FromWasmWriteInstanceRegion({ ...second, parts: [{ ptr: 720, len: 72 }] });
  const narrow = link_mirror_shader(s, 3, true);
  bind(second, second.byte_off + 12);
  const slice = draw_args(4, [{ first: 1, count: 1 }]);
  s.FromWasmDrawCall(slice);
  assert.equal(gl.created.length, 1, "different record strides share one R32UI texture");
  assert.deepEqual(gl.sub_images.slice(1).map(a => [a[2], a[3], a[4], a[5], a[6], a[10]]),
    [[45, 0, 0, 18, 1, 180]],
    "a later write transfers only its own words from the matching PBO offset");
  const selected = fetched_ranges.at(-1);
  assert.equal(selected.offset, 204, "attribute fetch adds the effective byte offset to the range start");
  assert.equal(selected.base, 17, "texture fetch uses the same absolute offset in three-word records");
  assert.deepEqual([...mirror_words.slice(selected.base * 3, selected.base * 3 + 3)], [206, 207, 208],
    "the narrow draw reads its own three words across the shared mirror");
  const before = [gl.calls.buffer_data.length, gl.calls.buffer_sub_data.length, gl.sub_images.length];
  const before_repeat = fetched_ranges.length;
  s.FromWasmDrawCall(slice);
  assert.equal(fetched_ranges.length, before_repeat + 1, "a clean mirror still issues the draw");
  assert.deepEqual([gl.calls.buffer_data.length, gl.calls.buffer_sub_data.length, gl.sub_images.length], before,
    "a clean repeat issues zero buffer allocations, uploads and mirror transfers");
  assert.equal(fetched_ranges.at(-1).base, 17, "the repeated draw keeps its three-word record base");
  const old = { ...identity, region: { index: 2, gen: 0 }, byte_off: 172, byte_len: 4 };
  s.FromWasmWriteInstanceRegion({ ...old, parts: [{ ptr: 720, len: 4 }] });
  const outside = { ...identity, region: { index: 2, gen: 1 }, byte_off: 252, byte_len: 12 };
  s.FromWasmWriteInstanceRegion({ ...outside, parts: [{ ptr: 720, len: 12 }] });
  bind(outside);
  const draws = gl.calls.draws.length;
  s.FromWasmDrawCall(draw_args());
  assert.equal(gl.calls.draws.length, draws + 1, "a region anywhere in the slab draws");
  assert.deepEqual([...mirror_words.slice(63, 66)], [200, 201, 202], "and its words reach the texture");
  bind(second);
  s.FromWasmDrawCall(slice);
  assert.equal(mirror_words[43], 200, "recycling a region token preserves already dirty slab words");
  s.FromWasmFreeWebGLResources({ instance_slabs: [identity] });
  assert.deepEqual(gl.calls.deleted_textures, [gl.created[0]], "destroying the backing frees its sole mirror");

  const fresh = { ...second, backing_generation: 10 };
  const failures = [];
  s.to_wasm = { ToWasmInstanceUploadFailed: msg => failures.push(msg) };
  s.FromWasmCreateInstanceSlab({ ...fresh, capacity_bytes: 256 });
  s.FromWasmWriteInstanceRegion({ ...fresh, parts: [{ ptr: 720, len: 72 }] });
  bind(fresh);
  gl.createTexture = () => null;
  const before_failure = gl.calls.draws.length;
  s.FromWasmDrawCall(slice);
  s.FromWasmDrawCall(slice);
  assert.deepEqual(failures, [{ slab_slot: fresh.slab_slot, backing_generation: 10, scope: 2, region: undefined }],
    "a failed mirror reports once for its backing");
  assert.equal(gl.calls.draws.length, before_failure, "sampling draws are suppressed immediately after mirror failure");
  s.FromWasmWriteInstanceRegion({ ...fresh, parts: [{ ptr: 720, len: 72 }] });
  assert.equal(s.instance_slabs.get(fresh.slab_slot).dirty.size, 0, "writes after mirror failure retain no dirty intervals");
  s.draw_shaders[1] = shader(gl, narrow.geom_attribs, narrow.inst_attribs, 4);
  s.FromWasmDrawCall(slice);
  assert.equal(gl.calls.draws.length, before_failure + 1, "the same region remains drawable through attributes");
});

test("a draw just past one texture window draws every instance, its words read from the next layer", () => {
  const s = subject();
  // 100-word records (past WebGL's attribute stride: read whole from the texture), three
  // records into the slab, one more than a window holds: one record straddles the window edge
  const slots = 100;
  const count = Math.floor(WINDOW_WORDS / slots) + 1;
  const words = count * slots;
  const start = 3 * slots * 4;
  s.memory = { buffer: new ArrayBuffer(4096 + words * 4) };
  const mirror_words = new Uint32Array(2 * WINDOW_WORDS);
  const { gl, fetched_ranges } = mirror_gl(s, mirror_words);
  s.webgl_limits.max_array_texture_layers = 256;
  new Float32Array(s.memory.buffer, 64, 6).set([0, 0, 1, 0, 0, 1]);
  new Uint32Array(s.memory.buffer, 256, 3).set([0, 1, 2]);
  const data = new Uint32Array(s.memory.buffer, 4096, words);
  for (let i = 0; i < words; i++) data[i] = i + 1;
  link_mirror_shader(s, slots);
  s.FromWasmAllocArrayBuffer(f32_upload(1, 64, 6));
  s.FromWasmAllocIndexBuffer(u32_indices(1, 256, 3));
  const identity = { slab_slot: 4, backing_generation: 1 };
  const region = { ...identity, region: { index: 0, gen: 0 }, byte_off: start, byte_len: words * 4 };
  const logged = logs.length;
  s.FromWasmCreateInstanceSlab({ ...identity, capacity_bytes: start + words * 4 });
  s.FromWasmWriteInstanceRegion({ ...region, parts: [{ ptr: 4096, len: words * 4 }] });
  const bind = (r) => s.FromWasmAllocVao({ ...r, instance_byte_offset: start, vao_id: 1, shader_id: 1, geom_ib_id: 1, geom_vb_id: 1 });
  bind(region);
  s.FromWasmDrawCall(draw_args(4, [{ first: 0, count }]));
  // the instance each word comes back for, read as the vertex shader reads it
  const misread = () => {
    const base = fetched_ranges.at(-1).base;
    for (let n = 0; n < count; n++) {
      for (let k = 0; k < slots; k++) {
        if (fetch_word(mirror_words, (n + base) * slots + k) !== data[n * slots + k]) return { instance: n, slot: k };
      }
    }
    return null;
  };

  assert.deepEqual(gl.calls.draws, [[gl.TRIANGLES, 3, gl.UNSIGNED_INT, 0, count]], "one draw takes every instance: none skipped, none split off");
  assert.equal(fetched_ranges.at(-1).base, 3, "mp_inst_base counts records from the slab's start");
  const image = gl.calls.texture_images.find(args => args[2] === gl.R32UI);
  assert.deepEqual(image.slice(0, 6), [gl.TEXTURE_2D_ARRAY, 0, gl.R32UI, WORD_ROW, WORD_ROW, 2], "the slab's words take two full layers");
  assert.equal(logs.slice(logged).filter(line => /spans 2 texture windows/.test(line)).length, 1, "the split is logged once");
  assert.equal(misread(), null, "every instance reads its own words, across the window edge and past it");

  // A rewrite crossing the window edge: a partial head and whole rows to the first layer, a
  // partial tail to the next.
  for (let i = 0; i < words; i++) data[i] += 7;
  const before = gl.sub_images.length;
  const rewritten = { ...region, region: { index: 0, gen: 1 } };
  s.FromWasmWriteInstanceRegion({ ...rewritten, parts: [{ ptr: 4096, len: words * 4 }] });
  bind(rewritten);
  s.FromWasmDrawCall(draw_args(4, [{ first: 0, count }]));
  const first = start / 4;
  assert.deepEqual(gl.sub_images.slice(before).map(a => [a[2], a[3], a[4], a[5], a[6], a[10]]), [
    [first, 0, 0, WORD_ROW - first, 1, first * 4],
    [0, 1, 0, WORD_ROW, WORD_ROW - 1, WORD_ROW * 4],
    [0, 0, 1, first + words - WINDOW_WORDS, 1, WINDOW_WORDS * 4],
  ], "each layer the rewrite crosses takes its own transfers from the matching PBO offsets");
  assert.equal(gl.calls.draws.length, 2, "the redraw takes every instance in one draw again");
  assert.equal(misread(), null, "every instance reads its rewritten words");
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("./web_gl.js", import.meta.url), "utf8")
  .replace(/^import \{[\s\S]*?\} from "\.\/web\.js";\n/, "")
  .replace(/^export /gm, "");
const load = new Function(
  "WasmWebBrowser",
  "console",
  "makepad_compute_webgl_size",
  "MAKEPAD_WEBGL_PIXEL_BUDGET",
  `${source}\nreturn WasmWebGL;`,
);
const WasmWebGL = load(class WasmWebBrowser {}, {
  log() {},
  warn() {},
  error() {},
}, (width, height) => ({ width, height, scale: 1 }), 16 * 1024 * 1024);

function makeMockGl() {
  let next_id = 1;
  const calls = {
    create: 0,
    deleted: [],
    binds: [],
    parameters: [],
    images: [],
    sub_images: [],
    pixel_store: [],
    get_errors: 0,
    get_parameters: 0,
    framebuffers: 0,
    framebuffer_binds: [],
    framebuffer_status: 0,
  };
  const gl = {
    NO_ERROR: 0,
    TEXTURE_2D: 1,
    TEXTURE_CUBE_MAP: 2,
    TEXTURE_CUBE_MAP_POSITIVE_X: 3,
    TEXTURE_CUBE_MAP_NEGATIVE_X: 4,
    TEXTURE_CUBE_MAP_POSITIVE_Y: 5,
    TEXTURE_CUBE_MAP_NEGATIVE_Y: 6,
    TEXTURE_CUBE_MAP_POSITIVE_Z: 7,
    TEXTURE_CUBE_MAP_NEGATIVE_Z: 8,
    TEXTURE_MAG_FILTER: 9,
    TEXTURE_MIN_FILTER: 10,
    TEXTURE_WRAP_S: 11,
    TEXTURE_WRAP_T: 12,
    TEXTURE_WRAP_R: 13,
    LINEAR: 14,
    NEAREST: 15,
    CLAMP_TO_EDGE: 16,
    RGBA: 17,
    RGBA32F: 18,
    R8: 19,
    RED: 20,
    UNSIGNED_BYTE: 21,
    FLOAT: 22,
    UNPACK_ALIGNMENT: 23,
    FRAMEBUFFER: 24,
    COLOR_ATTACHMENT0: 25,
    DEPTH_STENCIL_ATTACHMENT: 26,
    FRAMEBUFFER_COMPLETE: 27,
    COLOR_BUFFER_BIT: 1,
    DEPTH_BUFFER_BIT: 2,
    STENCIL_BUFFER_BIT: 4,
    createTexture() {
      calls.create += 1;
      if (this.create_throws) throw new Error("create failed");
      if (this.create_null) return null;
      return { id: next_id++ };
    },
    deleteTexture(texture) {
      calls.deleted.push(texture);
    },
    bindTexture(...args) {
      calls.binds.push(args);
    },
    texParameteri(...args) {
      calls.parameters.push(args);
    },
    texImage2D(...args) {
      calls.images.push(args);
      if (this.image_throws) throw new Error("image failed");
    },
    texSubImage2D(...args) {
      calls.sub_images.push(args);
      if (this.sub_image_throws) throw new Error("sub-image failed");
    },
    pixelStorei(...args) {
      calls.pixel_store.push(args);
      if (this.pixel_store_throws_at === calls.pixel_store.length) {
        throw new Error("pixel store failed");
      }
    },
    getError() {
      calls.get_errors += 1;
      const error = this.next_error || this.NO_ERROR;
      this.next_error = this.NO_ERROR;
      return error;
    },
    getParameter() {
      calls.get_parameters += 1;
      return 8192;
    },
    createFramebuffer() {
      calls.framebuffers += 1;
      return { id: `framebuffer-${calls.framebuffers}` };
    },
    bindFramebuffer(...args) {
      calls.framebuffer_binds.push(args);
    },
    framebufferTexture2D() {},
    checkFramebufferStatus() {
      calls.framebuffer_status += 1;
      return this.FRAMEBUFFER_COMPLETE;
    },
    viewport() {},
    depthMask() {},
    clearColor() {},
    clearDepth() {},
    clear() {},
  };
  gl.calls = calls;
  return gl;
}

function subject(memory_bytes = 4096) {
  const gl = makeMockGl();
  const value = Object.assign(Object.create(WasmWebGL.prototype), {
    gl,
    memory: { buffer: new ArrayBuffer(memory_bytes) },
    textures: [],
    framebuffers: [],
    active_render_target_textures: new Set(),
    webgl_limits: {
      max_texture_size: 8192,
      max_cube_map_texture_size: 4096,
    },
    bgra_upload_scratch: new Uint32Array(0),
    _texture_upload_reports: new Set(),
    _invalid_texture_upload_ids: new Set(),
    _render_target_size_reports: new Set(),
    render_target_rejected: false,
    webgl_context_lost: false,
    ext_color_buffer_float: {},
    xr: undefined,
    video_players: {},
    to_wasm: { ToWasmVideoTextureUpdated() {} },
    do_wasm_pump() {},
    ensure_video_animation_frame() {},
    ensure_render_quality() { return { pixel_budget: 16 * 1024 * 1024 }; },
  });
  return value;
}

function upload_args(texture_id, width, height, ptr, len) {
  return { texture_id, width, height, data: { ptr, len } };
}

function render_target_args(texture_id, width, height, pass_id = 1) {
  return {
    pass_id,
    width,
    height,
    color_targets: [{
      texture_id,
      format: 0,
      init_only: false,
      clear_color: { r: 0, g: 0, b: 0, a: 0 },
    }],
    depth_target: { attached: false },
  };
}

test("valid BGRA, odd-width R8, RGBA32F, and cube uploads preserve their formats", () => {
  const bgra = subject();
  new Uint32Array(bgra.memory.buffer, 4, 2).set([0x44332211, 0xddccbbaa]);
  bgra.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(1, 1, 1, 4, 2));
  assert.equal(bgra.textures[1]._render_target_valid, true);
  assert.equal(bgra.textures[1]._texture_upload_format, "bgra8");
  assert.deepEqual(
    [...bgra.gl.calls.images[0].at(-1)],
    [0x33, 0x22, 0x11, 0x44],
  );

  const r8 = subject();
  new Uint8Array(r8.memory.buffer, 1, 3).set([9, 8, 7]);
  r8.FromWasmAllocTextureImage2D_Ru8(upload_args(2, 3, 1, 1, 3));
  assert.equal(r8.gl.calls.images[0][2], r8.gl.R8);
  assert.deepEqual(r8.gl.calls.pixel_store, [
    [r8.gl.UNPACK_ALIGNMENT, 1],
    [r8.gl.UNPACK_ALIGNMENT, 4],
  ]);

  const float = subject();
  new Float32Array(float.memory.buffer, 4, 8).set([
    Number.NaN, 1, 2, 3, 4, 5, 6, 7,
  ]);
  float.FromWasmAllocTextureImage2D_RGBAf32(upload_args(3, 2, 1, 4, 8));
  assert.equal(float.gl.calls.images[0][2], float.gl.RGBA32F);
  assert.equal(float.gl.calls.images[0].at(-1)[0], Number.NaN);
  assert.ok(float.gl.calls.parameters.some((entry) => entry[2] === float.gl.NEAREST));

  const cube = subject();
  new Uint32Array(cube.memory.buffer, 4, 24).fill(0xff102030);
  cube.FromWasmAllocTextureCube_BGRAu8_32(upload_args(4, 2, 2, 4, 24));
  assert.equal(cube.gl.calls.images.length, 6);
  assert.deepEqual(
    cube.gl.calls.images.map((entry) => entry[0]),
    [
      cube.gl.TEXTURE_CUBE_MAP_POSITIVE_X,
      cube.gl.TEXTURE_CUBE_MAP_NEGATIVE_X,
      cube.gl.TEXTURE_CUBE_MAP_POSITIVE_Y,
      cube.gl.TEXTURE_CUBE_MAP_NEGATIVE_Y,
      cube.gl.TEXTURE_CUBE_MAP_POSITIVE_Z,
      cube.gl.TEXTURE_CUBE_MAP_NEGATIVE_Z,
    ],
  );
  assert.equal(cube.textures[4]._texture_target, cube.gl.TEXTURE_CUBE_MAP);
});

test("64 MiB is admitted exactly and format/cube byte accounting rejects the next allocation", () => {
  const s = subject(4);
  s.memory.buffer = { byteLength: 64 * 1024 * 1024 + 4 };
  const bgra_boundary = s.admit_texture_upload(
    upload_args(1, 4096, 4096, 4, 4096 * 4096),
    {
      faces: 1,
      bytes_per_texel: 4,
      elements_per_texel: 1,
      element_size: 4,
      nearest: false,
    },
  );
  assert.equal(bgra_boundary.allocation_bytes, 64 * 1024 * 1024);

  s.memory.buffer = { byteLength: 8192 * 8192 + 1 };
  assert.equal(s.admit_texture_upload(
    upload_args(2, 8192, 8192, 1, 8192 * 8192),
    {
      faces: 1,
      bytes_per_texel: 1,
      elements_per_texel: 1,
      element_size: 1,
      nearest: false,
    },
  ).allocation_bytes, 64 * 1024 * 1024);

  s.memory.buffer = { byteLength: 2048 * 2048 * 16 + 4 };
  assert.equal(s.admit_texture_upload(
    upload_args(3, 2048, 2048, 4, 2048 * 2048 * 4),
    {
      faces: 1,
      bytes_per_texel: 16,
      elements_per_texel: 4,
      element_size: 4,
      nearest: true,
    },
  ).allocation_bytes, 64 * 1024 * 1024);

  assert.equal(s.admit_texture_upload(
    upload_args(4, 2048, 2048, 4, 2048 * 2048 * 6),
    {
      faces: 6,
      bytes_per_texel: 4,
      elements_per_texel: 1,
      element_size: 4,
      nearest: false,
    },
  ), null);
  assert.equal(s.gl.calls.create, 0);
});

test("bad dimensions and slices reject before typed scratch or any GL call", () => {
  const cases = [
    (s) => s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(1, 0, 1, 4, 1)),
    (s) => s.FromWasmAllocTextureImage2D_Ru8(upload_args(1, 1.5, 1, 4, 2)),
    (s) => s.FromWasmAllocTextureImage2D_RGBAf32(
      upload_args(1, Number.MAX_SAFE_INTEGER + 1, 1, 4, 4),
    ),
    (s) => {
      s.webgl_limits.max_texture_size = Number.MAX_SAFE_INTEGER;
      s.FromWasmAllocTextureImage2D_Ru8(
        upload_args(1, Number.MAX_SAFE_INTEGER, 2, 4, 4),
      );
    },
    (s) => s.FromWasmAllocTextureImage2D_Ru8(upload_args(1, 8193, 1, 4, 8193)),
    (s) => s.FromWasmAllocTextureCube_BGRAu8_32(upload_args(1, 2, 1, 4, 12)),
    (s) => s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(1, 2, 2, 4, 3)),
    (s) => s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(1, 1, 1, 2, 1)),
    (s) => s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(1, 1, 1, 4096, 1)),
    (s) => s.FromWasmAllocTextureImage2D_Ru8(upload_args(1, 1, 1, 0, 1)),
    (s) => s.FromWasmAllocTextureImage2D_Ru8(upload_args(1, 1, 1, 4, 1.5)),
  ];
  for (const run of cases) {
    const s = subject();
    run(s);
    assert.equal(s.bgra_upload_scratch.length, 0);
    assert.equal(s.gl.calls.create, 0);
    assert.equal(s.gl.calls.binds.length, 0);
    assert.equal(s.gl.calls.images.length, 0);
    assert.equal(s.gl.calls.pixel_store.length, 0);
    assert.equal(s.gl.calls.get_errors, 0);
    assert.equal(s.gl.calls.get_parameters, 0);
    assert.equal(s._invalid_texture_upload_ids.has(1), true);
  }
});

test("unsafe texture ids reject without indexing the shared texture table", () => {
  for (const texture_id of [-1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
    const s = subject();
    s.FromWasmAllocTextureImage2D_Ru8(upload_args(texture_id, 1, 1, 4, 1));
    assert.equal(s.textures.length, 0);
    assert.equal(s.gl.calls.create, 0);
    assert.equal(s.gl.calls.binds.length, 0);
  }
});

test("invalid updates poison stale textures and a later valid allocation recovers the same object", () => {
  const s = subject();
  new Uint32Array(s.memory.buffer, 4, 2).fill(0xff000000);
  s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(7, 1, 1, 4, 1));
  const texture = s.textures[7];
  assert.equal(texture._render_target_valid, true);

  s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(7, 2, 1, 4, 1));
  assert.equal(s.textures[7], texture);
  assert.equal(texture._render_target_valid, false);
  assert.equal(s.gl.calls.images.length, 1);

  s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(7, 2, 1, 4, 2));
  assert.equal(s.textures[7], texture);
  assert.equal(texture._render_target_valid, true);
  assert.equal(texture._texture_upload_width, 2);
  assert.equal(s._invalid_texture_upload_ids.has(7), false);
  assert.equal(s.gl.calls.images.length, 2);
});

test("allocation errors delete new objects, invalidate existing ones, and recover", () => {
  const fresh = subject();
  new Uint8Array(fresh.memory.buffer, 4, 2).fill(1);
  fresh.gl.next_error = 77;
  fresh.FromWasmAllocTextureImage2D_Ru8(upload_args(3, 1, 1, 4, 1));
  assert.equal(fresh.textures[3], undefined);
  assert.equal(fresh.gl.calls.deleted.length, 1);
  assert.equal(fresh._invalid_texture_upload_ids.has(3), true);
  fresh.FromWasmAllocTextureImage2D_Ru8(upload_args(3, 1, 1, 4, 1));
  assert.equal(fresh.textures[3]._render_target_valid, true);

  const existing = subject();
  new Uint8Array(existing.memory.buffer, 4, 2).fill(1);
  existing.FromWasmAllocTextureImage2D_Ru8(upload_args(4, 1, 1, 4, 1));
  const texture = existing.textures[4];
  existing.gl.next_error = 91;
  existing.FromWasmAllocTextureImage2D_Ru8(upload_args(4, 2, 1, 4, 2));
  assert.equal(existing.textures[4], texture);
  assert.equal(texture._render_target_valid, false);
  assert.equal(existing.gl.calls.deleted.length, 0);
  existing.FromWasmAllocTextureImage2D_Ru8(upload_args(4, 2, 1, 4, 2));
  assert.equal(texture._render_target_valid, true);
});

test("createTexture null/throw and R8 exceptions fail closed with alignment restored", () => {
  for (const mode of ["create_null", "create_throws"]) {
    const s = subject();
    new Uint8Array(s.memory.buffer, 4, 1).fill(1);
    s.gl[mode] = true;
    assert.doesNotThrow(() => {
      s.FromWasmAllocTextureImage2D_Ru8(upload_args(5, 1, 1, 4, 1));
    });
    assert.equal(s.textures[5], undefined);
    assert.equal(s._invalid_texture_upload_ids.has(5), true);
  }

  const failed_image = subject();
  new Uint8Array(failed_image.memory.buffer, 4, 3).fill(1);
  failed_image.gl.image_throws = true;
  failed_image.FromWasmAllocTextureImage2D_Ru8(upload_args(6, 3, 1, 4, 3));
  assert.deepEqual(failed_image.gl.calls.pixel_store, [
    [failed_image.gl.UNPACK_ALIGNMENT, 1],
    [failed_image.gl.UNPACK_ALIGNMENT, 4],
  ]);
  assert.equal(failed_image.gl.calls.deleted.length, 1);

  const failed_update = subject();
  new Uint8Array(failed_update.memory.buffer, 4, 3).fill(1);
  failed_update.FromWasmAllocTextureImage2D_Ru8(upload_args(7, 3, 1, 4, 3));
  failed_update.gl.sub_image_throws = true;
  failed_update.FromWasmAllocTextureImage2D_Ru8(upload_args(7, 3, 1, 4, 3));
  assert.deepEqual(failed_update.gl.calls.pixel_store.slice(-2), [
    [failed_update.gl.UNPACK_ALIGNMENT, 1],
    [failed_update.gl.UNPACK_ALIGNMENT, 4],
  ]);
  assert.equal(failed_update.textures[7]._render_target_valid, false);
});

test("same-size updates use sub-images without new allocation or device queries", () => {
  const variants = [
    ["FromWasmAllocTextureImage2D_BGRAu8_32", upload_args(1, 1, 1, 4, 1), 1],
    ["FromWasmAllocTextureImage2D_Ru8", upload_args(2, 3, 1, 4, 3), 1],
    ["FromWasmAllocTextureImage2D_RGBAf32", upload_args(3, 1, 1, 4, 4), 1],
    ["FromWasmAllocTextureCube_BGRAu8_32", upload_args(4, 1, 1, 4, 6), 6],
  ];
  for (const [method, args, upload_count] of variants) {
    const s = subject();
    s[method](args);
    s[method](args);
    assert.equal(s.gl.calls.create, 1);
    assert.equal(s.gl.calls.images.length, upload_count);
    assert.equal(s.gl.calls.sub_images.length, upload_count);
    assert.equal(s.gl.calls.get_errors, 1);
    assert.equal(s.gl.calls.get_parameters, 0);
  }
});

test("2D/cube target switches replace transactionally and invalidate stale FBO identity", () => {
  const s = subject();
  new Uint32Array(s.memory.buffer, 4, 6).fill(0xff001122);
  s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(9, 1, 1, 4, 1));
  const old_2d = s.textures[9];
  const framebuffer = {
    _color_attachments: [old_2d],
    _depth_attachment: null,
  };
  s.framebuffers.push(framebuffer);
  s.active_render_target_textures.add(old_2d);

  s.FromWasmAllocTextureCube_BGRAu8_32(upload_args(9, 1, 1, 4, 6));
  const cube = s.textures[9];
  assert.notEqual(cube, old_2d);
  assert.equal(cube._texture_target, s.gl.TEXTURE_CUBE_MAP);
  assert.equal(cube._render_target_valid, true);
  assert.equal(old_2d._render_target_valid, false);
  assert.deepEqual(s.gl.calls.deleted, [old_2d]);
  assert.equal(framebuffer._color_attachments, undefined);
  assert.equal(s.active_render_target_textures.size, 0);
  assert.equal(s.render_target_rejected, true);

  s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(9, 1, 1, 4, 1));
  assert.notEqual(s.textures[9], cube);
  assert.equal(s.textures[9]._texture_target, s.gl.TEXTURE_2D);
  assert.deepEqual(s.gl.calls.deleted, [old_2d, cube]);
});

test("image and render-target writers force each other to restore storage", () => {
  const s = subject();
  new Float32Array(s.memory.buffer, 4, 8).fill(1);

  s.FromWasmAllocTextureImage2D_RGBAf32(upload_args(10, 2, 1, 4, 8));
  const texture = s.textures[10];
  s.FromWasmBeginRenderTexture(render_target_args(10, 2, 1));

  assert.equal(s.gl.calls.images.length, 2);
  assert.equal(s.gl.calls.images[1][2], s.gl.RGBA);
  assert.equal(texture._texture_upload_format, undefined);
  assert.equal(texture._texture_target, s.gl.TEXTURE_2D);
  assert.equal(s.gl.calls.framebuffer_status, 1);

  s.FromWasmAllocTextureImage2D_RGBAf32(upload_args(10, 2, 1, 4, 8));
  assert.equal(s.gl.calls.images.length, 3);
  assert.equal(s.gl.calls.sub_images.length, 0);
  assert.equal(s.gl.calls.images[2][2], s.gl.RGBA32F);
  assert.equal(texture._width, undefined);
  assert.equal(s.framebuffers[1]._color_attachments, undefined);

  s.FromWasmBeginRenderTexture(render_target_args(10, 2, 1));
  assert.equal(s.gl.calls.images.length, 4);
  assert.equal(s.gl.calls.images[3][2], s.gl.RGBA);
  assert.equal(s.gl.calls.framebuffer_status, 2);
});

test("a successful render target recovers an id rejected by image admission", () => {
  const s = subject();
  s.FromWasmAllocTextureImage2D_Ru8(upload_args(11, 2, 1, 4, 1));
  assert.equal(s._invalid_texture_upload_ids.has(11), true);

  s.FromWasmBeginRenderTexture(render_target_args(11, 2, 1));
  assert.equal(s.render_target_rejected, false);
  assert.equal(s._invalid_texture_upload_ids.has(11), false);
  assert.equal(s.textures[11]._render_target_valid, true);
  assert.equal(s.textures[11]._texture_target, s.gl.TEXTURE_2D);
});

test("render targets reject cube ids before an incompatible texture bind", () => {
  const s = subject();
  new Uint32Array(s.memory.buffer, 4, 6).fill(0xff001122);
  s.FromWasmAllocTextureCube_BGRAu8_32(upload_args(12, 1, 1, 4, 6));
  const texture_bind_count = s.gl.calls.binds.length;

  s.FromWasmBeginRenderTexture(render_target_args(12, 1, 1));
  assert.equal(s.render_target_rejected, true);
  assert.equal(s.gl.calls.binds.length, texture_bind_count);
  assert.equal(s.gl.calls.framebuffers, 0);
});

test("video replaces poster storage, recovers its id, and forces the next image allocation", () => {
  const s = subject();
  new Uint32Array(s.memory.buffer, 4, 2).fill(0xff001122);
  s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(13, 1, 1, 4, 1));
  const texture = s.textures[13];
  s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(13, 2, 1, 4, 1));
  assert.equal(s._invalid_texture_upload_ids.has(13), true);

  s.video_players.demo = {
    playing: true,
    texture_id: 13,
    texture_initialized: false,
    video: { readyState: 2, currentTime: 0, videoWidth: 1, videoHeight: 1 },
    video_id_lo: 1,
    video_id_hi: 0,
  };
  s.update_video_textures();
  assert.equal(texture._texture_target, s.gl.TEXTURE_2D);
  assert.equal(texture._texture_upload_width, 1);
  assert.equal(texture._texture_upload_height, 1);
  assert.equal(texture._texture_upload_format, "video-rgba8");
  assert.equal(texture._render_target_valid, true);
  assert.equal(s._invalid_texture_upload_ids.has(13), false);

  s.FromWasmBeginRenderTexture(render_target_args(13, 1, 1));
  assert.equal(texture._texture_upload_format, undefined);
  s.update_video_textures();
  assert.equal(texture._texture_upload_format, "video-rgba8");

  s.FromWasmAllocTextureImage2D_BGRAu8_32(upload_args(13, 1, 1, 4, 1));
  assert.equal(s.gl.calls.images.length, 5);
  assert.equal(s.gl.calls.sub_images.length, 0);
});

test("video dimensions are admitted before GL and cached storage uses sub-images", () => {
  const oversized = subject();
  let oversized_updates = 0;
  oversized.to_wasm.ToWasmVideoTextureUpdated = () => oversized_updates++;
  oversized.video_players.demo = {
    playing: true,
    texture_id: 20,
    texture_initialized: false,
    video: {
      readyState: 2,
      currentTime: 0,
      videoWidth: 8192,
      videoHeight: 8192,
    },
    video_id_lo: 1,
    video_id_hi: 0,
  };
  oversized.update_video_textures();
  assert.equal(oversized.gl.calls.create, 0);
  assert.equal(oversized.gl.calls.binds.length, 0);
  assert.equal(oversized.gl.calls.images.length, 0);
  assert.equal(oversized.gl.calls.get_errors, 0);
  assert.equal(oversized.gl.calls.get_parameters, 0);
  assert.equal(oversized_updates, 0);

  const s = subject();
  let updates = 0;
  s.to_wasm.ToWasmVideoTextureUpdated = () => updates++;
  const video = {
    readyState: 2,
    currentTime: 0,
    videoWidth: 0,
    videoHeight: 0,
  };
  s.video_players.demo = {
    playing: true,
    texture_id: 21,
    texture_initialized: false,
    video,
    video_id_lo: 1,
    video_id_hi: 0,
  };
  s.update_video_textures();
  assert.equal(s.gl.calls.create, 0);
  assert.equal(s._texture_upload_reports.size, 0);

  video.videoWidth = -1;
  video.videoHeight = 2160;
  s.update_video_textures();
  assert.equal(s.gl.calls.create, 0);
  assert.equal(s._invalid_texture_upload_ids.has(21), true);
  assert.equal(updates, 0);

  video.videoWidth = 3840;
  s.update_video_textures();
  assert.equal(s.gl.calls.create, 1);
  assert.equal(s.gl.calls.images.length, 1);
  assert.equal(s.gl.calls.sub_images.length, 0);
  assert.equal(s.gl.calls.get_errors, 1);
  assert.equal(s.gl.calls.get_parameters, 0);
  assert.equal(s._invalid_texture_upload_ids.has(21), false);
  assert.equal(updates, 1);

  s.update_video_textures();
  assert.equal(s.gl.calls.create, 1);
  assert.equal(s.gl.calls.images.length, 1);
  assert.equal(s.gl.calls.sub_images.length, 1);
  assert.equal(s.gl.calls.get_errors, 1);
  assert.equal(s.gl.calls.get_parameters, 0);
  assert.equal(updates, 2);

  video.videoWidth = 1920;
  video.videoHeight = 1080;
  s.update_video_textures();
  assert.equal(s.gl.calls.images.length, 2);
  assert.equal(s.gl.calls.sub_images.length, 1);
  assert.equal(s.gl.calls.get_errors, 2);
  assert.equal(updates, 3);
});

test("video createTexture exceptions fail closed and can recover", () => {
  const s = subject();
  let updates = 0;
  s.to_wasm.ToWasmVideoTextureUpdated = () => updates++;
  s.video_players.demo = {
    playing: true,
    texture_id: 22,
    texture_initialized: false,
    video: {
      readyState: 2,
      currentTime: 0,
      videoWidth: 1280,
      videoHeight: 720,
    },
    video_id_lo: 1,
    video_id_hi: 0,
  };
  s.gl.create_throws = true;
  assert.doesNotThrow(() => s.update_video_textures());
  assert.equal(s.textures[22], undefined);
  assert.equal(s._invalid_texture_upload_ids.has(22), true);
  assert.equal(updates, 0);

  s.gl.create_throws = false;
  s.update_video_textures();
  assert.equal(s.textures[22]._render_target_valid, true);
  assert.equal(s._invalid_texture_upload_ids.has(22), false);
  assert.equal(updates, 1);
});

test("diagnostics are deduplicated by fault class", () => {
  const s = subject();
  const args = upload_args(1, 0, 1, 4, 1);
  s.FromWasmAllocTextureImage2D_BGRAu8_32(args);
  s.FromWasmAllocTextureImage2D_BGRAu8_32(args);
  assert.deepEqual([...s._texture_upload_reports], ["invalid-dimensions"]);
});

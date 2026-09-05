import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("./web_gl.js", import.meta.url), "utf8")
  .replace(/^import \{[\s\S]*?\} from "\.\/web\.js";\n/, "")
  .replace(/^export /gm, "");
const load = new Function(
  "WasmWebBrowser",
  `${source}\nreturn WasmWebGL;`,
);
const WasmWebGL = load(class WasmWebBrowser {});

function makeMockGl() {
  let nextId = 1;
  const live = new Set();
  const deleted = [];
  const make = (kind) => {
    const object = { kind, id: nextId++ };
    live.add(object);
    return object;
  };
  const remove = (kind, object) => {
    if (object && live.delete(object)) deleted.push([kind, object.id]);
  };
  return {
    live,
    deleted,
    ARRAY_BUFFER: 1,
    ELEMENT_ARRAY_BUFFER: 2,
    STATIC_DRAW: 3,
    TEXTURE_2D: 4,
    TEXTURE_MAG_FILTER: 5,
    TEXTURE_MIN_FILTER: 6,
    TEXTURE_WRAP_S: 7,
    TEXTURE_WRAP_T: 8,
    LINEAR: 9,
    CLAMP_TO_EDGE: 10,
    RGBA: 11,
    UNSIGNED_BYTE: 12,
    UNSIGNED_INT: 13,
    NO_ERROR: 0,
    createBuffer: () => make("buffer"),
    createVertexArray: () => make("vao"),
    createTexture: () => make("texture"),
    createFramebuffer: () => make("framebuffer"),
    deleteBuffer: (object) => remove("buffer", object),
    deleteVertexArray: (object) => remove("vao", object),
    deleteTexture: (object) => remove("texture", object),
    deleteFramebuffer: (object) => remove("framebuffer", object),
    bindBuffer() {},
    bufferData() {},
    bufferSubData() {},
    bindTexture() {},
    texParameteri() {},
    texImage2D() {},
    getError: () => 0,
  };
}

function makeHarness() {
  const harness = Object.create(WasmWebGL.prototype);
  harness.gl = makeMockGl();
  harness.memory = { buffer: new ArrayBuffer(64) };
  harness.draw_shaders = [{ pending: true }];
  harness.array_buffers = [];
  harness.index_buffers = [];
  harness.vaos = [];
  harness.textures = [];
  harness.framebuffers = [];
  harness.bgra_upload_scratch = new Uint32Array(0);
  harness._missing_shader_ids = new Set();
  return harness;
}

function allocateResourceSet(harness, id) {
  const ptr = { ptr: 4, len: 4 };
  const empty = { ptr: 0, len: 0 };
  harness.FromWasmAllocArrayBuffer({ buffer_id: id, data: ptr, byte_data: empty });
  harness.FromWasmAllocArrayBuffer({ buffer_id: id + 1, data: ptr, byte_data: empty });
  harness.FromWasmAllocIndexBuffer({
    buffer_id: id,
    data: { ptr: 4, len: 3 },
    byte_data: empty,
    index_width: 4,
  });
  harness.FromWasmAllocVao({
    vao_id: id,
    shader_id: 0,
    geom_ib_id: id,
    geom_vb_id: id,
    inst_vb_id: id + 1,
  });
  harness.FromWasmAllocTextureImage2D_BGRAu8_32({
    texture_id: id,
    width: 1,
    height: 1,
    data: { ptr: 4, len: 1 },
  });
  harness.framebuffers[id] = harness.gl.createFramebuffer();
}

function freeResourceSet(harness, id) {
  harness.FromWasmFreeWebGLResources({
    array_buffer_ids: [id, id + 1],
    index_buffer_ids: [id],
    // Deliberately omit the VAO: buffer dependency cleanup must find it.
    vao_ids: [],
    texture_ids: [id],
    framebuffer_ids: [id],
  });
}

test("WebGL retirement is idempotent, dependency-safe, and bounded across slot reuse", () => {
  const harness = makeHarness();

  // A disjoint live set must survive every cleanup batch.
  allocateResourceSet(harness, 10);
  assert.ok(harness.array_buffers[10].valid && harness.index_buffers[10].valid);
  const shared = {
    array: harness.array_buffers[10].gl_buf,
    index: harness.index_buffers[10].gl_buf,
    vao: harness.vaos[10].gl_vao,
    texture: harness.textures[10],
    framebuffer: harness.framebuffers[10],
  };
  const sharedLiveCount = harness.gl.live.size;

  // Explicit VAO retirement also owns its UBOs, without touching the shared
  // buffers that VAO happened to reference.
  harness.FromWasmAllocVao({
    vao_id: 5,
    shader_id: 0,
    geom_ib_id: 10,
    geom_vb_id: 10,
    inst_vb_id: 11,
  });
  const explicitVao = harness.vaos[5];
  harness.FromWasmFreeWebGLResources({
    array_buffer_ids: [],
    index_buffer_ids: [],
    vao_ids: [5],
    texture_ids: [],
    framebuffer_ids: [],
  });
  assert.equal(harness.vaos[5], undefined);
  assert.ok(!harness.gl.live.has(explicitVao.gl_vao));
  assert.ok(!harness.gl.live.has(explicitVao.draw_call_uniform_buf));
  assert.ok(!harness.gl.live.has(explicitVao.user_uniform_buf));
  assert.equal(harness.gl.live.size, sharedLiveCount);

  let maximumLiveCount = sharedLiveCount;
  for (let cycle = 0; cycle < 100; cycle++) {
    allocateResourceSet(harness, 0);
    maximumLiveCount = Math.max(maximumLiveCount, harness.gl.live.size);
    const retiredVao = harness.vaos[0];
    const retiredArray = harness.array_buffers[0].gl_buf;

    freeResourceSet(harness, 0);
    assert.equal(harness.vaos[0], undefined);
    assert.equal(harness.array_buffers[0], undefined);
    assert.ok(!harness.gl.live.has(retiredVao.gl_vao));
    assert.ok(!harness.gl.live.has(retiredVao.draw_call_uniform_buf));
    assert.ok(!harness.gl.live.has(retiredVao.user_uniform_buf));
    assert.ok(!harness.gl.live.has(retiredArray));

    const deletes = harness.gl.deleted.length;
    freeResourceSet(harness, 0);
    assert.equal(harness.gl.deleted.length, deletes);
    assert.equal(harness.gl.live.size, sharedLiveCount);
  }

  assert.equal(harness.array_buffers[10].gl_buf, shared.array);
  assert.equal(harness.index_buffers[10].gl_buf, shared.index);
  assert.equal(harness.vaos[10].gl_vao, shared.vao);
  assert.equal(harness.textures[10], shared.texture);
  assert.equal(harness.framebuffers[10], shared.framebuffer);
  assert.ok(maximumLiveCount <= sharedLiveCount + 8);
  assert.ok(harness.array_buffers.length <= 12);
  assert.ok(harness.index_buffers.length <= 11);
  assert.ok(harness.vaos.length <= 11);
  assert.ok(harness.textures.length <= 11);
  assert.ok(harness.framebuffers.length <= 11);
});

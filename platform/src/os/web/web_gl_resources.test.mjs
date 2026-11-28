import { assert, read_text, test } from "makepad-web-test";

const source = read_text(new URL("./web_gl.js", import.meta.url))
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
  harness.instance_slabs = new Map();
  harness.index_buffers = [];
  harness.vaos = [];
  harness.textures = [];
  harness.framebuffers = [];
  harness.bgra_upload_scratch = new Uint32Array(0);
  harness._missing_shader_ids = new Set();
  return harness;
}

// A resource set's instance rows: a slab slot apart from the ones the batch free below retires.
const setSlab = id => ({ slab_slot: 100 + id, backing_generation: 7 });
const setRows = id => ({
  ...setSlab(id), region: { index: 0, gen: 0 }, byte_off: 0, byte_len: 16, instance_byte_offset: 0,
});

function allocateResourceSet(harness, id) {
  const ptr = { ptr: 4, len: 4 };
  const empty = { ptr: 0, len: 0 };
  harness.FromWasmAllocArrayBuffer({ buffer_id: id, data: ptr, byte_data: empty });
  harness.FromWasmCreateInstanceSlab({ ...setSlab(id), capacity_bytes: 16 });
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
    ...setRows(id),
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
    instance_slabs: [setSlab(id)],
    array_buffer_ids: [id],
    index_buffer_ids: [id],
    // Deliberately omit the VAO: buffer and slab dependency cleanup must find it.
    vao_ids: [],
    texture_ids: [id],
    framebuffer_ids: [id],
  });
}

test("WebGL retirement is idempotent, dependency-safe, and bounded across slot reuse", () => {
  const harness = makeHarness();

  // The packet stream and a disjoint live set survive every cleanup batch.
  harness.FromWasmUploadUniformStream({ data: { ptr: 4, len: 16 } });
  const stream = harness.uniform_stream_buf;
  assert.ok(harness.gl.live.has(stream));
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

  // Allocating a VAO does not allocate any private uniform buffers.
  harness.FromWasmAllocVao({
    vao_id: 5,
    shader_id: 0,
    geom_ib_id: 10,
    geom_vb_id: 10,
    ...setRows(10),
  });
  assert.equal(harness.gl.live.size, sharedLiveCount);
  const explicitVao = harness.vaos[5];
  explicitVao.gl_vao = harness.gl.createVertexArray();
  harness.FromWasmFreeWebGLResources({
    array_buffer_ids: [],
    index_buffer_ids: [],
    vao_ids: [5],
    texture_ids: [],
    framebuffer_ids: [],
  });
  assert.equal(harness.vaos[5], undefined);
  assert.ok(!harness.gl.live.has(explicitVao.gl_vao));
  assert.equal(harness.gl.live.size, sharedLiveCount);

  // Both slabs use the live geometry at 10. Retiring them in one batch
  // must find every dependent VAO, even those sharing one of the backings.
  const slabs = [10, 11].map(slab_slot => ({ slab_slot, backing_generation: 1 }));
  const failed = { slab_slot: 10, backing_generation: 0 };
  const failures = [];
  harness.to_wasm = { ToWasmInstanceUploadFailed: msg => failures.push(msg) };
  const allocate = harness.gl.createBuffer;
  harness.gl.createBuffer = () => null;
  harness.FromWasmCreateInstanceSlab({ ...failed, capacity_bytes: 16 });
  assert.deepEqual(failures, [{ ...failed, scope: 0, region: undefined }], "a null GL buffer reports a backing failure");
  harness.FromWasmFreeWebGLResources({ instance_slabs: [failed] });
  harness.gl.createBuffer = allocate;
  for (const identity of slabs) harness.FromWasmCreateInstanceSlab({ ...identity, capacity_bytes: 16 });
  for (let id = 0; id < 3; id++) {
    harness.FromWasmAllocVao({
      ...slabs[id % 2], vao_id: id, shader_id: 0, geom_ib_id: 10, geom_vb_id: 10,
      region: { index: id, gen: 0 }, byte_off: 0, byte_len: 16, instance_byte_offset: 0,
    });
  }
  harness.FromWasmFreeWebGLResources({ instance_slabs: slabs });
  assert.ok([0, 1, 2].every(id => harness.vaos[id] === undefined), "a batch free retires every dependent VAO");
  assert.ok(!harness.instance_slabs.has(10) && !harness.instance_slabs.has(11), "a batch free removes both slab slots");
  assert.ok(harness.instance_slabs.has(setSlab(10).slab_slot), "a batch free keeps the live set's slab");
  assert.equal(harness.gl.live.size, sharedLiveCount, "freeing slabs releases their buffers");
  assert.equal(harness.array_buffers[10].gl_buf, shared.array, "a slab slot may alias a live geometry buffer id");

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
  assert.ok(harness.gl.live.has(stream));
  assert.ok(maximumLiveCount <= sharedLiveCount + 6);
  assert.ok(harness.array_buffers.length <= 12);
  assert.ok(harness.index_buffers.length <= 11);
  assert.ok(harness.vaos.length <= 11);
  assert.ok(harness.textures.length <= 11);
  assert.ok(harness.framebuffers.length <= 11);
});

test("completion reports only a signalled GPU fence and retires it", async () => {
  const harness = makeHarness();
  harness.wasm = {};
  const reports = [];
  harness.to_wasm = { ToWasmGpuCompletion: value => reports.push(value) };
  harness.do_wasm_pump = () => {};
  let polls = 0, deleted = 0;
  Object.assign(harness.gl, {
    SYNC_GPU_COMMANDS_COMPLETE: 20, TIMEOUT_EXPIRED: 21, WAIT_FAILED: 22,
    fenceSync: () => ({}), flush() {}, isContextLost: () => false,
    clientWaitSync: (_fence, flags, timeout) => {
      assert.equal(flags, 0); assert.equal(timeout, 0);
      return ++polls < 3 ? 21 : 23;
    },
    deleteSync: () => deleted++,
  });
  harness.FromWasmPollGpuCompletion({ serial_lo: 17, serial_hi: 2 });
  assert.equal(reports.length, 0);
  await new Promise(resolve => setTimeout(resolve, 70));
  assert.deepEqual(reports, [{ serial_lo: 17, serial_hi: 2, success: true }]);
  assert.equal(deleted, 1);
  harness.gl.clientWaitSync = () => harness.gl.WAIT_FAILED;
  harness.FromWasmPollGpuCompletion({ serial_lo: 18, serial_hi: 2 });
  assert.deepEqual(reports.at(-1), { serial_lo: 18, serial_hi: 2, success: false });
  assert.equal(deleted, 2);
});

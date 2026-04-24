// WebGPU backend. Rust: `platform/src/os/web/web_gpu.rs` (WGSL compile) and
// `platform/src/os/web/web_render.rs` (same `FromWasm*` draw protocol as WebGL).
import { WasmWebBrowser } from "./web.js";

// WebGPU backend skeleton.
//
// NOTE: This is intentionally not enabled by default yet because Makepad’s
// current shader sources are GLSL ES (WebGL2). The full WebGPU backend will be
// introduced alongside a new batched render protocol and WGSL pipeline.

export class WasmWebGPU extends WasmWebBrowser {
  static async is_supported() {
    return typeof navigator !== "undefined" && !!navigator.gpu;
  }

  static async try_create(wasm, dispatch, canvas) {
    if (!(await WasmWebGPU.is_supported())) {
      return null;
    }
    try {
      return new WasmWebGPU(wasm, dispatch, canvas);
    } catch (_e) {
      return null;
    }
  }

  constructor(wasm, dispatch, canvas) {
    super(wasm, dispatch, canvas);
    if (wasm === undefined) {
      return;
    }
    this.render_api = 1;
    this.canvas = canvas;
    this.dispatch = dispatch;

    this.gpu = navigator.gpu;
    this.adapter = null;
    this.device = null;
    this.queue = null;
    this.context = null;
    this.format = null;

    // Resource streaming scaffolding (ring buffers + caches).
    this.buffers = {
      uniforms: null,
      geometry: null,
      instances: null,
      indices: null,
    };
    this.pipeline_cache = new Map();
    this.texture_cache = new Map();
    this.vaos = [];
    this.array_buffers = [];
    this.index_buffers = [];
    this.draw_shaders = [];
    this.textures = [];

    this._encoder = null;
    this._pass = null;
    this._depth_tex = null;
    this._depth_view = null;
    this._last_size = { w: 0, h: 0 };

    // Async init; after WebGPU init, run the same dependency/bootstrap path.
    this._webgpu_init_promise = this.init_webgpu_context();
    this._webgpu_init_promise.then(() => this.load_deps());
  }

  async init_webgpu_context() {
    this.adapter = await this.gpu.requestAdapter({ powerPreference: "high-performance" });
    if (!this.adapter) {
      throw new Error("WebGPU adapter unavailable");
    }
    this.device = await this.adapter.requestDevice();
    this.queue = this.device.queue;
    this.context = this.canvas.getContext("webgpu");
    if (!this.context) {
      throw new Error("WebGPU context unavailable");
    }
    this.format = navigator.gpu.getPreferredCanvasFormat();
    this.context.configure({
      device: this.device,
      format: this.format,
      alphaMode: "premultiplied",
    });

    // Minimal gpu_info for ToWasmInit.
    this.gpu_info = this.gpu_info || { min_uniform_vectors: 0, vendor: "webgpu", renderer: "webgpu" };

    // Default ring buffer sizes (can be tuned later).
    this.buffers.uniforms = new WgpuRingBuffer(this.device, 4 * 1024 * 1024, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
    this.buffers.geometry = new WgpuRingBuffer(this.device, 8 * 1024 * 1024, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
    this.buffers.instances = new WgpuRingBuffer(this.device, 8 * 1024 * 1024, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
    this.buffers.indices = new WgpuRingBuffer(this.device, 4 * 1024 * 1024, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST);
  }

  // ---- Protocol handlers ----

  FromWasmCompileWebGPUShader(args) {
    // Create shader module + render pipeline. We keep it simple: one pipeline per shader_id.
    const device = this.device;

    const module = device.createShaderModule({ code: args.wgsl });

    const geom_vec4s = Math.ceil(args.geometry_slots / 4);
    const inst_vec4s = Math.ceil(args.instance_slots / 4);

    const vertexBuffers = [];
    if (geom_vec4s > 0) {
      vertexBuffers.push({
        arrayStride: geom_vec4s * 16,
        stepMode: "vertex",
        attributes: new Array(geom_vec4s).fill(0).map((_, i) => ({
          shaderLocation: i,
          offset: i * 16,
          format: "float32x4",
        })),
      });
    }
    if (inst_vec4s > 0) {
      const baseLoc = geom_vec4s;
      vertexBuffers.push({
        arrayStride: inst_vec4s * 16,
        stepMode: "instance",
        attributes: new Array(inst_vec4s).fill(0).map((_, i) => ({
          shaderLocation: baseLoc + i,
          offset: i * 16,
          format: "float32x4",
        })),
      });
    }

    // Uniform buffers: pass, draw_list, draw_call, dyn(user), live(scope).
    // These bindings match the existing WebGL protocol grouping; we’ll optimize later.
    // Build bind group layout by scanning WGSL `@binding(N)` declarations.
    // This avoids needing the full uniform-buffer binding map in JS.
    const binding_kinds = new Map(); // binding -> "buffer" | "sampler" | "texture"
    const binding_vars = new Map(); // binding -> var name
    const usedBindings = new Set();
    const bindingDecl = /@binding\((\d+)\)\s+var(?:<[^>]+>)?\s+([A-Za-z0-9_]+)\s*:\s*([^;]+);/g;
    let match;
    const layoutEntries = [];
    const textureBindings = [];
    const samplerBindings = [];
    let textureBindingIndex = 0;
    while ((match = bindingDecl.exec(args.wgsl)) !== null) {
      const binding = parseInt(match[1], 10) | 0;
      if (usedBindings.has(binding)) continue;
      usedBindings.add(binding);
      const varName = match[2];
      const ty = match[3];
      if (ty.includes("sampler")) {
        binding_kinds.set(binding, "sampler");
        binding_vars.set(binding, varName);
        const samplerIndex = (args.sampler_binding_base | 0) <= binding
          ? binding - (args.sampler_binding_base | 0)
          : -1;
        const samplerDesc =
          samplerIndex >= 0 && samplerIndex < (args.samplers || []).length
            ? args.samplers[samplerIndex]
            : null;
        layoutEntries.push({
          binding,
          visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT,
          sampler: {
            type:
              samplerDesc && (samplerDesc.filter | 0) !== 0
                ? "filtering"
                : "non-filtering",
          },
        });
        samplerBindings.push({ binding, samplerIndex });
      } else if (ty.includes("texture_")) {
        binding_kinds.set(binding, "texture");
        binding_vars.set(binding, varName);
        let sampleType = "float";
        if (ty.includes("texture_depth_")) sampleType = "depth";
        else if (ty.includes("<i32>")) sampleType = "sint";
        else if (ty.includes("<u32>")) sampleType = "uint";
        else {
          const samplerIndex = (args.texture_sampler_indices || [])[textureBindingIndex];
          const samplerDesc =
            samplerIndex !== undefined && samplerIndex < (args.samplers || []).length
              ? args.samplers[samplerIndex]
              : null;
          if (samplerDesc && (samplerDesc.filter | 0) === 0) {
            sampleType = "unfilterable-float";
          }
        }
        const viewDimension = ty.includes("_2d_array")
          ? "2d-array"
          : ty.includes("_cube_array")
            ? "cube-array"
            : ty.includes("_cube")
              ? "cube"
              : "2d";
        layoutEntries.push({
          binding,
          visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT,
          texture: { sampleType, viewDimension },
        });
        textureBindings.push({
          binding,
          textureIndex: textureBindingIndex,
          viewDimension,
          declaredSampleType: sampleType,
        });
        textureBindingIndex += 1;
      } else {
        // Default to uniform buffer for now.
        binding_kinds.set(binding, "buffer");
        binding_vars.set(binding, varName);
        layoutEntries.push({
          binding,
          visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT,
          buffer: { type: "uniform" },
        });
      }
    }
    layoutEntries.sort((a, b) => a.binding - b.binding);
    const bindGroupLayout = device.createBindGroupLayout({ entries: layoutEntries });
    const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [bindGroupLayout] });

    const pipeline = device.createRenderPipeline({
      layout: pipelineLayout,
      vertex: { module, entryPoint: "vertex_main", buffers: vertexBuffers },
      fragment: {
        module,
        entryPoint: "fragment_main",
        targets: [{ format: this.format, blend: { color: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" }, alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" } } }],
      },
      primitive: { topology: "triangle-list", cullMode: "none" },
      depthStencil: { format: "depth24plus", depthWriteEnabled: true, depthCompare: "less-equal" },
    });

    // Allocate persistent uniform buffers per shader.
    const makeUbo = (byteSize) =>
      device.createBuffer({
        size: Math.max(256, (byteSize + 255) & ~255),
        usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
      });
    const samplerBase = args.sampler_binding_base | 0;
    const samplerCount = (args.samplers || []).length;
    const samplers = (args.samplers || []).map((s) => this.create_sampler_from_desc(s));
    const texBase = args.texture_binding_base | 0;

    const shader = {
      pipeline,
      bindGroupLayout,
      shaderModule: module,
      vertexBuffers,
      binding_kinds,
      binding_vars,
      baseLayoutEntries: layoutEntries,
      textureBindings,
      samplerBindings,
      pipelineVariants: new Map(),
      ubos: new Map(), // binding -> GPUBuffer
      ubo_pass: makeUbo(2048),
      ubo_draw_list: makeUbo(2048),
      ubo_draw_call: makeUbo(2048),
      ubo_user: makeUbo(2048),
      ubo_live: makeUbo(2048),
      sampler_binding_base: samplerBase,
      sampler_count: samplerCount,
      texture_binding_base: texBase,
      texture_count: args.textures.length | 0,
      xr_depth_binding: args.xr_depth_binding | 0,
      texture_sampler_indices: args.texture_sampler_indices || [],
      samplerDescs: args.samplers || [],
      samplers,
      geom_vec4s,
      inst_vec4s,
      geometry_slots: args.geometry_slots,
      instance_slots: args.instance_slots,
    };

    // Ensure we have a buffer for every declared uniform binding.
    for (const [binding, kind] of binding_kinds.entries()) {
      if (kind !== "buffer") continue;
      shader.ubos.set(binding, makeUbo(2048));
    }
    // Alias known buffers by name (when present).
    for (const [binding, varName] of binding_vars.entries()) {
      if (!binding_kinds.get(binding) || binding_kinds.get(binding) !== "buffer") continue;
      if (varName.includes("unibuf_draw_pass")) shader.ubo_pass = shader.ubos.get(binding);
      else if (varName.includes("unibuf_draw_list")) shader.ubo_draw_list = shader.ubos.get(binding);
      else if (varName.includes("unibuf_draw_call")) shader.ubo_draw_call = shader.ubos.get(binding);
      else if (varName.includes("_mp_dyn_uniforms")) shader.ubo_user = shader.ubos.get(binding);
      else if (varName.includes("_mp_scope_uniforms")) shader.ubo_live = shader.ubos.get(binding);
    }
    shader.baseBindGroup = null;

    this.draw_shaders[args.shader_id] = shader;
  }

  create_sampler_from_desc(desc) {
    const device = this.device;
    const filter = desc.filter | 0;
    const address = desc.address | 0;
    const coord = desc.coord | 0;
    // Only normalized coords are supported in WebGPU samplers; Pixel is handled in shader.
    const magFilter = filter === 0 ? "nearest" : "linear";
    const minFilter = filter === 0 ? "nearest" : "linear";
    const addressMode =
      address === 0
        ? "repeat"
        : address === 1
          ? "clamp-to-edge"
          : address === 2
            ? "clamp-to-edge"
            : "mirror-repeat";
    return device.createSampler({
      magFilter,
      minFilter,
      addressModeU: addressMode,
      addressModeV: addressMode,
      addressModeW: addressMode,
    });
  }

  get_sampler_resource(desc, bindingType) {
    if (bindingType !== "non-filtering" || !desc || (desc.filter | 0) === 0) {
      return this.create_sampler_from_desc(desc || { filter: 0, address: 1, coord: 0 });
    }
    if (!this._sampler_variant_cache) this._sampler_variant_cache = new Map();
    const key = `${desc.address | 0}:${desc.coord | 0}:non-filtering`;
    let sampler = this._sampler_variant_cache.get(key);
    if (sampler) return sampler;
    sampler = this.create_sampler_from_desc({
      ...desc,
      filter: 0,
    });
    this._sampler_variant_cache.set(key, sampler);
    return sampler;
  }

  sample_type_for_texture_entry(entry, declaredSampleType) {
    if (!entry) return declaredSampleType;
    if (declaredSampleType === "depth" || declaredSampleType === "sint" || declaredSampleType === "uint") {
      return declaredSampleType;
    }
    switch (entry.format) {
      case "rgba32float":
      case "r32float":
        return "unfilterable-float";
      default:
        return "float";
    }
  }

  make_pipeline_variant_key(shader, textureEntries) {
    const textureKey = shader.textureBindings
      .map(({ textureIndex, declaredSampleType }) =>
        this.sample_type_for_texture_entry(textureEntries[textureIndex], declaredSampleType))
      .join("|");
    const samplerKey = shader.samplerDescs
      .map((_, samplerIndex) => this.sampler_binding_type_for_index(shader, samplerIndex, textureEntries))
      .join("|");
    return `${textureKey}::${samplerKey}`;
  }

  sampler_binding_type_for_index(shader, samplerIndex, textureEntries) {
    const hasUnfilterableTexture = shader.textureBindings.some(({ textureIndex, declaredSampleType }) => {
      return shader.texture_sampler_indices[textureIndex] === samplerIndex
        && this.sample_type_for_texture_entry(textureEntries[textureIndex], declaredSampleType) === "unfilterable-float";
    });
    if (hasUnfilterableTexture) return "non-filtering";
    const desc = shader.samplerDescs[samplerIndex];
    return desc && (desc.filter | 0) !== 0 ? "filtering" : "non-filtering";
  }

  get_pipeline_variant(shader, textureEntries) {
    const key = this.make_pipeline_variant_key(shader, textureEntries);
    let variant = shader.pipelineVariants.get(key);
    if (variant) return variant;

    const layoutEntries = shader.baseLayoutEntries.map((entry) => {
      if (entry.texture) {
        const textureBinding = shader.textureBindings.find((item) => item.binding === entry.binding);
        if (!textureBinding) return entry;
        return {
          ...entry,
          texture: {
            ...entry.texture,
            sampleType: this.sample_type_for_texture_entry(
              textureEntries[textureBinding.textureIndex],
              textureBinding.declaredSampleType,
            ),
          },
        };
      }
      if (entry.sampler) {
        const samplerBinding = shader.samplerBindings.find((item) => item.binding === entry.binding);
        if (!samplerBinding) return entry;
        return {
          ...entry,
          sampler: {
            type: this.sampler_binding_type_for_index(
              shader,
              samplerBinding.samplerIndex,
              textureEntries,
            ),
          },
        };
      }
      return entry;
    });

    const bindGroupLayout = this.device.createBindGroupLayout({ entries: layoutEntries });
    const pipelineLayout = this.device.createPipelineLayout({ bindGroupLayouts: [bindGroupLayout] });
    const pipeline = this.device.createRenderPipeline({
      layout: pipelineLayout,
      vertex: { module: shader.shaderModule, entryPoint: "vertex_main", buffers: shader.vertexBuffers },
      fragment: {
        module: shader.shaderModule,
        entryPoint: "fragment_main",
        targets: [{ format: this.format, blend: { color: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" }, alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" } } }],
      },
      primitive: { topology: "triangle-list", cullMode: "none" },
      depthStencil: { format: "depth24plus", depthWriteEnabled: true, depthCompare: "less-equal" },
    });

    variant = { bindGroupLayout, pipeline };
    shader.pipelineVariants.set(key, variant);
    return variant;
  }

  create_bind_group_for_shader(shader, textureViews, textureEntries, variant) {
    const entries = [];
    // Bind all declared uniform buffers (even if we don't populate them yet).
    for (const [binding, kind] of shader.binding_kinds.entries()) {
      if (kind !== "buffer") continue;
      const buf = shader.ubos.get(binding) || shader.ubo_pass;
      entries.push({ binding, resource: { buffer: buf } });
    }

    // Bind all samplers once.
    for (let i = 0; i < shader.sampler_count; i++) {
      const b = shader.sampler_binding_base + i;
      if (shader.binding_kinds?.get(b) !== "sampler") continue;
      const bindingType = this.sampler_binding_type_for_index(shader, i, textureEntries);
      const desc = shader.samplerDescs[i];
      const useOriginal =
        (bindingType === "filtering" && desc && (desc.filter | 0) !== 0)
        || (bindingType === "non-filtering" && (!desc || (desc.filter | 0) === 0));
      entries.push({
        binding: b,
        resource: useOriginal
          ? (shader.samplers[i] || this.get_fallback_sampler())
          : this.get_sampler_resource(desc, bindingType),
      });
    }

    // Bind textures at `texture_binding_base + i` (base comes from WGSL generator).
    // We assume the generator packs textures sequentially.
    const texBase = (shader.texture_binding_base | 0);
    for (let i = 0; i < shader.texture_count; i++) {
      const view = textureViews[i] || this.get_fallback_texture_view();
      const b = texBase + i;
      if (shader.binding_kinds?.get(b) !== "texture") continue;
      entries.push({ binding: b, resource: view });
    }

    if (shader.binding_kinds?.get(shader.xr_depth_binding) === "texture") {
      entries.push({
        binding: shader.xr_depth_binding,
        resource: this.get_fallback_depth_texture_view(),
      });
    }

    return this.device.createBindGroup({ layout: variant.bindGroupLayout, entries });
  }

  get_fallback_sampler() {
    if (!this._fallback_sampler) {
      this._fallback_sampler = this.device.createSampler({ magFilter: "nearest", minFilter: "nearest" });
    }
    return this._fallback_sampler;
  }

  get_fallback_texture_view() {
    if (!this._fallback_texture) {
      this._fallback_texture = this.device.createTexture({
        size: [1, 1, 1],
        format: "rgba8unorm",
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
      });
      this.queue.writeTexture({ texture: this._fallback_texture }, new Uint8Array([255, 0, 255, 255]), { bytesPerRow: 4 }, { width: 1, height: 1, depthOrArrayLayers: 1 });
      this._fallback_texture_view = this._fallback_texture.createView();
    }
    return this._fallback_texture_view;
  }

  get_fallback_depth_texture_view() {
    if (!this._fallback_depth_texture) {
      this._fallback_depth_texture = this.device.createTexture({
        size: [1, 1, 1],
        format: "depth24plus",
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.RENDER_ATTACHMENT,
      });
      this._fallback_depth_texture_view = this._fallback_depth_texture.createView();
    }
    return this._fallback_depth_texture_view;
  }

  FromWasmAllocArrayBuffer(args) {
    // Generic upload used for geometry + instances (Float32).
    const device = this.device;
    let entry = this.array_buffers[args.buffer_id];
    const f32 = new Float32Array(this.memory.buffer, args.data.ptr, args.data.len);
    const byteLength = f32.byteLength;
    if (!entry || !entry.buf || entry.byteLength !== byteLength) {
      entry = this.array_buffers[args.buffer_id] = {
        buf: device.createBuffer({ size: Math.max(4, byteLength), usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST }),
        byteLength,
        length: f32.length,
        data: null,
        packed: new Map(),
      };
    }
    // Copy out of shared memory for writeBuffer compatibility/perf predictability.
    const copy = f32.slice();
    this.queue.writeBuffer(entry.buf, 0, copy.buffer, copy.byteOffset, copy.byteLength);
    entry.length = f32.length;
    entry.data = copy;
    entry.byteLength = byteLength;
    entry.packed = new Map();
  }

  get_packed_vertex_buffer(entry, logicalSlots, packedVec4s) {
    if (!entry || !entry.data || logicalSlots <= 0) return entry;
    const strideFloats = packedVec4s * 4;
    if (strideFloats <= logicalSlots) return entry;

    const key = `${logicalSlots}:${strideFloats}:${entry.length}`;
    let packed = entry.packed?.get(key);
    if (packed) return packed;

    const itemCount = (entry.length / logicalSlots) | 0;
    const out = new Float32Array(itemCount * strideFloats);
    for (let i = 0; i < itemCount; i++) {
      const srcOffset = i * logicalSlots;
      const dstOffset = i * strideFloats;
      out.set(entry.data.subarray(srcOffset, srcOffset + logicalSlots), dstOffset);
    }

    packed = {
      buf: this.device.createBuffer({
        size: Math.max(4, out.byteLength),
        usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
      }),
      byteLength: out.byteLength,
      length: out.length,
      logicalLength: entry.length,
      data: out,
    };
    this.queue.writeBuffer(packed.buf, 0, out.buffer, out.byteOffset, out.byteLength);
    entry.packed.set(key, packed);
    return packed;
  }

  FromWasmAllocIndexBuffer(args) {
    const device = this.device;
    let entry = this.index_buffers[args.buffer_id];
    const u32 = new Uint32Array(this.memory.buffer, args.data.ptr, args.data.len);
    const byteLength = u32.byteLength;
    if (!entry || !entry.buf || entry.byteLength !== byteLength) {
      entry = this.index_buffers[args.buffer_id] = {
        buf: device.createBuffer({ size: Math.max(4, byteLength), usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST }),
        byteLength,
        length: u32.length,
      };
    }
    const copy = u32.slice();
    this.queue.writeBuffer(entry.buf, 0, copy.buffer, copy.byteOffset, copy.byteLength);
    entry.length = u32.length;
  }

  FromWasmAllocVao(args) {
    // Store the tuple so a draw can find its buffers.
    this.vaos[args.vao_id] = {
      shader_id: args.shader_id,
      geom_ib_id: args.geom_ib_id,
      geom_vb_id: args.geom_vb_id,
      inst_vb_id: args.inst_vb_id,
    };
  }

  FromWasmBeginRenderCanvas(args) {
    const w = this.canvas.width | 0;
    const h = this.canvas.height | 0;
    if (w !== this._last_size.w || h !== this._last_size.h || !this._depth_tex) {
      this._last_size.w = w;
      this._last_size.h = h;
      this._depth_tex = this.device.createTexture({
        size: [Math.max(1, w), Math.max(1, h), 1],
        format: "depth24plus",
        usage: GPUTextureUsage.RENDER_ATTACHMENT,
      });
      this._depth_view = this._depth_tex.createView();
    }

    const colorView = this.context.getCurrentTexture().createView();
    this._encoder = this.device.createCommandEncoder();
    this._pass = this._encoder.beginRenderPass({
      colorAttachments: [
        {
          view: colorView,
          clearValue: args.clear_color,
          loadOp: "clear",
          storeOp: "store",
        },
      ],
      depthStencilAttachment: {
        view: this._depth_view,
        depthClearValue: args.clear_depth,
        depthLoadOp: "clear",
        depthStoreOp: "store",
      },
    });
  }

  FromWasmSetDefaultDepthAndBlendMode() {
    // Pipeline state covers this on WebGPU; no-op.
  }

  FromWasmRenderCommandBuffer(args) {
    const CMD_DRAW = 1;
    const NONE_TEX = 0xffffffff;
    const words = new Uint32Array(this.memory.buffer, args.words.ptr, args.words.len);
    let at = 0;

    while (at < words.length) {
      const cmd = words[at++];
      if (cmd === 0) break;
      if (cmd !== CMD_DRAW) break;

      const shader_id = words[at++];
      const vao_id = words[at++];
      const depth_write = words[at++] !== 0;
      const backface_culling = words[at++] !== 0;

      const pass_ptr = words[at++]; const pass_len = words[at++];
      const draw_list_ptr = words[at++]; const draw_list_len = words[at++];
      const draw_call_ptr = words[at++]; const draw_call_len = words[at++];
      const user_ptr = words[at++]; const user_len = words[at++];
      const live_ptr = words[at++]; const live_len = words[at++];

      const shader = this.draw_shaders[shader_id];
      const vao = this.vaos[vao_id];
      if (!shader || !vao || !this._pass) {
        at += 16;
        continue;
      }

      // Upload uniforms (copy out of shared memory) and grow UBOs if needed.
      const copyF32 = (ptr, len) => new Float32Array(this.memory.buffer, ptr, len).slice();
      const pass_u = copyF32(pass_ptr, pass_len);
      const list_u = copyF32(draw_list_ptr, draw_list_len);
      const call_u = copyF32(draw_call_ptr, draw_call_len);
      const user_u = copyF32(user_ptr, user_len);
      const live_u = copyF32(live_ptr, live_len);
      this.ensure_ubo(shader, "ubo_pass", pass_u.byteLength);
      this.ensure_ubo(shader, "ubo_draw_list", list_u.byteLength);
      this.ensure_ubo(shader, "ubo_draw_call", call_u.byteLength);
      this.ensure_ubo(shader, "ubo_user", user_u.byteLength);
      this.ensure_ubo(shader, "ubo_live", live_u.byteLength);
      this.queue.writeBuffer(shader.ubo_pass, 0, pass_u.buffer, pass_u.byteOffset, pass_u.byteLength);
      this.queue.writeBuffer(shader.ubo_draw_list, 0, list_u.buffer, list_u.byteOffset, list_u.byteLength);
      this.queue.writeBuffer(shader.ubo_draw_call, 0, call_u.buffer, call_u.byteOffset, call_u.byteLength);
      this.queue.writeBuffer(shader.ubo_user, 0, user_u.buffer, user_u.byteOffset, user_u.byteLength);
      this.queue.writeBuffer(shader.ubo_live, 0, live_u.buffer, live_u.byteOffset, live_u.byteLength);

      // Pipeline is created with defaults; depth_write/backface_culling are ignored for now.
      // Build bind group for this draw based on textures.
      const textureViews = new Array(shader.texture_count);
      const textureEntries = new Array(shader.texture_count);
      const texIdsAt = at;
      for (let i = 0; i < shader.texture_count; i++) {
        const texId = words[texIdsAt + i];
        if (texId !== NONE_TEX) {
          const tex = this.textures[texId];
          textureViews[i] = tex ? tex.view : null;
          textureEntries[i] = tex || null;
        } else {
          textureViews[i] = null;
          textureEntries[i] = null;
        }
      }

      const variant = this.get_pipeline_variant(shader, textureEntries);
      const bindGroup = this.create_bind_group_for_shader(shader, textureViews, textureEntries, variant);

      this._pass.setPipeline(variant.pipeline);
      this._pass.setBindGroup(0, bindGroup);

      const geomRaw = this.array_buffers[vao.geom_vb_id];
      const instRaw = this.array_buffers[vao.inst_vb_id];
      const ib = this.index_buffers[vao.geom_ib_id];
      if (!geomRaw || !instRaw || !ib) {
        at += 16;
        continue;
      }

      const geom = this.get_packed_vertex_buffer(geomRaw, shader.geometry_slots, shader.geom_vec4s);
      const inst = this.get_packed_vertex_buffer(instRaw, shader.instance_slots, shader.inst_vec4s);

      this._pass.setVertexBuffer(0, geom.buf);
      this._pass.setVertexBuffer(1, inst.buf);
      this._pass.setIndexBuffer(ib.buf, "uint32");

      const indexCount = ib.length | 0;
      const instanceCount = ((instRaw.length | 0) / shader.instance_slots) | 0;

      // Skip texture ids (already consumed for bind group).
      at += 16;

      this._pass.drawIndexed(indexCount, instanceCount, 0, 0, 0);
    }

    // End + submit each pump (simple but correct).
    if (this._pass) {
      this._pass.end();
      this._pass = null;
    }
    if (this._encoder) {
      const cmd = this._encoder.finish();
      this._encoder = null;
      this.queue.submit([cmd]);
    }
  }

  ensure_ubo(shader, field, requiredBytes) {
    const buf = shader[field];
    if (buf && buf.size >= requiredBytes) return;
    const nextSize = Math.max(256, (requiredBytes + 255) & ~255);
    shader[field] = this.device.createBuffer({
      size: nextSize,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    });
  }

  // --- Texture uploads from wasm ---

  FromWasmAllocTextureImage2D_BGRAu8_32(args) {
    // Upload as RGBA8; input is BGRA in u32 but wasm packs it already for WebGL path.
    // Treat it as raw bytes.
    const w = args.width | 0;
    const h = args.height | 0;
    const bytes = new Uint8Array(this.memory.buffer, args.data.ptr, w * h * 4).slice();
    let entry = this.textures[args.texture_id];
    if (!entry || entry.w !== w || entry.h !== h || entry.format !== "rgba8unorm") {
      const texture = this.device.createTexture({
        size: [w, h, 1],
        format: "rgba8unorm",
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
      });
      entry = this.textures[args.texture_id] = {
        texture,
        view: texture.createView(),
        w,
        h,
        format: "rgba8unorm",
      };
    }
    this.queue.writeTexture(
      { texture: entry.texture },
      bytes,
      { bytesPerRow: w * 4 },
      { width: w, height: h, depthOrArrayLayers: 1 },
    );
  }

  FromWasmAllocTextureImage2D_Ru8(args) {
    const w = args.width | 0;
    const h = args.height | 0;
    const bytes = new Uint8Array(this.memory.buffer, args.data.ptr, w * h).slice();
    let entry = this.textures[args.texture_id];
    if (!entry || entry.w !== w || entry.h !== h || entry.format !== "r8unorm") {
      const texture = this.device.createTexture({
        size: [w, h, 1],
        format: "r8unorm",
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
      });
      entry = this.textures[args.texture_id] = {
        texture,
        view: texture.createView(),
        w,
        h,
        format: "r8unorm",
      };
    }
    this.queue.writeTexture(
      { texture: entry.texture },
      bytes,
      { bytesPerRow: w },
      { width: w, height: h, depthOrArrayLayers: 1 },
    );
  }

  FromWasmAllocTextureImage2D_RGBAf32(args) {
    const w = args.width | 0;
    const h = args.height | 0;
    const f32 = new Float32Array(this.memory.buffer, args.data.ptr, w * h * 4).slice();
    let entry = this.textures[args.texture_id];
    if (!entry || entry.w !== w || entry.h !== h || entry.format !== "rgba32float") {
      const texture = this.device.createTexture({
        size: [w, h, 1],
        format: "rgba32float",
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
      });
      entry = this.textures[args.texture_id] = {
        texture,
        view: texture.createView(),
        w,
        h,
        format: "rgba32float",
      };
    }
    this.queue.writeTexture(
      { texture: entry.texture },
      new Uint8Array(f32.buffer),
      { bytesPerRow: w * 16 },
      { width: w, height: h, depthOrArrayLayers: 1 },
    );
  }
}

// Simple suballocator for streaming updates into a single GPUBuffer.
class WgpuRingBuffer {
  constructor(device, byteLength, usage) {
    this.device = device;
    this.byteLength = byteLength;
    this.usage = usage;
    this.buffer = device.createBuffer({
      size: byteLength,
      usage,
      mappedAtCreation: false,
    });
    this.at = 0;
    this.frame_id = 0;
  }

  begin_frame() {
    this.frame_id++;
    this.at = 0;
  }

  alloc(byteLength, align = 256) {
    const aligned = (this.at + (align - 1)) & ~(align - 1);
    if (aligned + byteLength > this.byteLength) {
      // Wrap. In the “real” backend we’ll need multi-frame fencing;
      // for now this is a skeleton used behind an opt-in flag.
      this.at = 0;
      return this.alloc(byteLength, align);
    }
    this.at = aligned + byteLength;
    return aligned;
  }

  write_u8(queue, offset, u8) {
    queue.writeBuffer(this.buffer, offset, u8.buffer, u8.byteOffset, u8.byteLength);
  }

  write_f32(queue, offset, f32) {
    queue.writeBuffer(this.buffer, offset, f32.buffer, f32.byteOffset, f32.byteLength);
  }
}

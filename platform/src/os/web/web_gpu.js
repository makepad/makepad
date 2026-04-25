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
    /** @type {GPUTextureFormat|null} color attachment format for current render pass */
    this._pass_color_format = null;
    /** @type {{ w: number, h: number }|null} */
    this._pass_extent = null;
    this.xr = undefined;
    this.video_players = {};
    this.video_anim_frame_id = 0;
    this._pending_until_ready = [];
    /** default depth/blend — pipelines encode blend; depth compare is default less-equal */
    this._default_depth_write = true;
    this._default_backface_cull = false;

    // Async init; buffer protocol calls until the device exists.
    this._webgpu_init_promise = this.init_webgpu_context();
    this._webgpu_init_promise.then(() => {
      const queued = this._pending_until_ready;
      this._pending_until_ready = [];
      for (const q of queued) {
        const fn = this[q.name];
        if (typeof fn === "function") {
          try {
            fn.call(this, q.args);
          } catch (e) {
            console.error(`[makepad:webgpu] queued call failed name=${q.name} err=${e && e.message ? e.message : e}`);
          }
        }
      }
      this.load_deps();
    });
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

    this._frame_id = 0;
    this._draw_count = 0;
    this.device.addEventListener('uncapturederror', (ev) => {
      console.error('[makepad:webgpu] uncaptured device error:', ev.error.message);
    });

    // Minimal gpu_info for ToWasmInit.
    this.gpu_info = this.gpu_info || { min_uniform_vectors: 0, vendor: "webgpu", renderer: "webgpu" };

    // Default ring buffer sizes (can be tuned later).
    this.buffers.uniforms = new WgpuRingBuffer(this.device, 4 * 1024 * 1024, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
    this.buffers.geometry = new WgpuRingBuffer(this.device, 8 * 1024 * 1024, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
    this.buffers.instances = new WgpuRingBuffer(this.device, 8 * 1024 * 1024, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
    this.buffers.indices = new WgpuRingBuffer(this.device, 4 * 1024 * 1024, GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST);

    // Some shaders reference low texture ids very early (before the app allocates real textures).
    // Provide a stable default so draws don't sample "missing" and output nothing.
    if (!this.textures[3]) {
      const tex = this.device.createTexture({
        size: [1, 1, 1],
        format: "rgba8unorm",
        usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
      });
      this.queue.writeTexture({ texture: tex }, new Uint8Array([255, 255, 255, 255]), { bytesPerRow: 4 }, { width: 1, height: 1, depthOrArrayLayers: 1 });
      this.textures[3] = { texture: tex, view: tex.createView(), w: 1, h: 1, format: "rgba8unorm" };
    }
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
    const textureNameToIndex = new Map((args.textures || []).map((t, i) => [t.name, i]));
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
        const mappedIndex = textureNameToIndex.get(varName);
        const textureIndex = mappedIndex !== undefined ? mappedIndex : textureBindingIndex;
        textureBindings.push({
          binding,
          textureIndex,
          viewDimension,
          declaredSampleType: sampleType,
        });
        if (mappedIndex === undefined) {
          textureBindingIndex += 1;
        }
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
      id: args.shader_id | 0,
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
      ubo_binding_pass: -1,
      ubo_binding_draw_list: -1,
      ubo_binding_draw_call: -1,
      ubo_binding_user: -1,
      ubo_binding_live: -1,
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
      if (varName.includes("unibuf_draw_pass")) { shader.ubo_pass = shader.ubos.get(binding); shader.ubo_binding_pass = binding; }
      else if (varName.includes("unibuf_draw_list")) { shader.ubo_draw_list = shader.ubos.get(binding); shader.ubo_binding_draw_list = binding; }
      else if (varName.includes("unibuf_draw_call")) { shader.ubo_draw_call = shader.ubos.get(binding); shader.ubo_binding_draw_call = binding; }
      else if (varName.includes("_mp_dyn_uniforms")) { shader.ubo_user = shader.ubos.get(binding); shader.ubo_binding_user = binding; }
      else if (varName.includes("_mp_scope_uniforms")) { shader.ubo_live = shader.ubos.get(binding); shader.ubo_binding_live = binding; }
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

  make_pipeline_variant_key(shader, textureEntries, colorFormat, depthWrite, backfaceCulling) {
    const textureKey = shader.textureBindings
      .map(({ textureIndex, declaredSampleType }) =>
        this.sample_type_for_texture_entry(textureEntries[textureIndex], declaredSampleType))
      .join("|");
    const samplerKey = shader.samplerDescs
      .map((_, samplerIndex) => this.sampler_binding_type_for_index(shader, samplerIndex, textureEntries))
      .join("|");
    const dw = depthWrite ? 1 : 0;
    const bf = backfaceCulling ? 1 : 0;
    const hd = this._pass_has_depth ? 1 : 0;
    return `${colorFormat}|${dw}|${bf}|${hd}|${textureKey}::${samplerKey}`;
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

  get_pipeline_variant(shader, textureEntries, depthWrite, backfaceCulling) {
    const colorFormat = this._pass_color_format || this.format;
    const key = this.make_pipeline_variant_key(shader, textureEntries, colorFormat, depthWrite, backfaceCulling);
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
    const cullMode = backfaceCulling ? "back" : "none";
    const hasDepthAttachment = !!this._pass_has_depth;
    let pipeline;
    try {
      pipeline = this.device.createRenderPipeline({
        layout: pipelineLayout,
        vertex: { module: shader.shaderModule, entryPoint: "vertex_main", buffers: shader.vertexBuffers },
        fragment: {
          module: shader.shaderModule,
          entryPoint: "fragment_main",
          targets: [{
            format: colorFormat,
            blend: {
              color: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
              alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
            },
          }],
        },
        primitive: { topology: "triangle-list", cullMode },
        depthStencil: hasDepthAttachment
          ? {
              format: "depth24plus",
              depthWriteEnabled: depthWrite,
              depthCompare: "less-equal",
            }
          : undefined,
      });
    } catch (err) {
      throw err;
    }

    variant = { bindGroupLayout, pipeline, key };
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

    // Bind samplers using their declared WGSL bindings.
    for (const sb of shader.samplerBindings || []) {
      const samplerIndex = sb.samplerIndex | 0;
      const b = sb.binding | 0;
      if (shader.binding_kinds?.get(b) !== "sampler") continue;
      const bindingType = this.sampler_binding_type_for_index(shader, samplerIndex, textureEntries);
      const desc = shader.samplerDescs[samplerIndex];
      const useOriginal =
        (bindingType === "filtering" && desc && (desc.filter | 0) !== 0)
        || (bindingType === "non-filtering" && (!desc || (desc.filter | 0) === 0));
      entries.push({
        binding: b,
        resource: useOriginal
          ? (shader.samplers[samplerIndex] || this.get_fallback_sampler())
          : this.get_sampler_resource(desc, bindingType),
      });
    }

    // Bind textures using their declared WGSL bindings (do not assume contiguous packing).
    for (const tb of shader.textureBindings || []) {
      const isDepth = tb.declaredSampleType === "depth";
      const entry = textureEntries[tb.textureIndex];
      const view = isDepth
        ? ((entry && entry.depthView) ? entry.depthView : this.get_fallback_depth_texture_view())
        : (textureViews[tb.textureIndex] || this.get_fallback_texture_view());
      const b = tb.binding | 0;
      if (shader.binding_kinds?.get(b) !== "texture") continue;
      entries.push({ binding: b, resource: view });
    }

    // WGSL may contain multiple declarations that collapse to the same binding in our simplified model.
    // WebGPU rejects duplicate binding entries, so keep the last write per binding.
    const byBinding = new Map();
    for (const e of entries) byBinding.set(e.binding | 0, e);
    const uniqueEntries = Array.from(byBinding.values()).sort((a, b) => (a.binding | 0) - (b.binding | 0));

    return this.device.createBindGroup({ layout: variant.bindGroupLayout, entries: uniqueEntries });
  }

  get_bind_group_for_shader(shader, textureViews, textureEntries, variant, texIds, pool_idx = 0) {
    if (!shader.bindGroups) shader.bindGroups = new Map();
    let key = variant.key + '|P' + pool_idx;
    for (let i = 0; i < shader.texture_count; i++) {
        const tid = texIds[i];
        const entry = textureEntries[i];
        const ver = entry ? (entry.version | 0) : 0;
        key += '|' + (tid == null ? 'n' : tid) + ':' + ver;
    }
    let bg = shader.bindGroups.get(key);
    if (bg) return bg;
    bg = this.create_bind_group_for_shader(shader, textureViews, textureEntries, variant);
    shader.bindGroups.set(key, bg);
    return bg;
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
    const requestedByteLength = f32.byteLength;
    if (!entry || !entry.buf || entry.byteLength < requestedByteLength) {
      const newByteLength = Math.max(requestedByteLength, entry ? entry.byteLength * 2 : 4096);
      entry = this.array_buffers[args.buffer_id] = {
        buf: device.createBuffer({ size: Math.max(4, newByteLength), usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST }),
        byteLength: newByteLength,
        length: f32.length,
        data: null,
        packed: new Map(),
        version: 0,
      };
    }
    // Copy out of shared memory for writeBuffer compatibility/perf predictability.
    const copy = f32.slice();
    this.queue.writeBuffer(entry.buf, 0, copy.buffer, copy.byteOffset, requestedByteLength);
    entry.length = f32.length;
    entry.data = copy;
    entry.version = (entry.version || 0) + 1;
    if (!entry.packed) entry.packed = new Map();
  }

  get_packed_vertex_buffer(entry, logicalSlots, packedVec4s) {
    if (!entry || !entry.data || logicalSlots <= 0) return entry;
    const strideFloats = packedVec4s * 4;
    if (strideFloats <= logicalSlots) return entry;

    const key = `${logicalSlots}:${strideFloats}`;
    let packed = entry.packed?.get(key);
    if (packed && packed.version === entry.version) return packed;

    const itemCount = (entry.length / logicalSlots) | 0;
    const requiredByteLength = itemCount * strideFloats * 4;

    if (!packed || packed.byteLength < requiredByteLength) {
      const newByteLength = Math.max(requiredByteLength, packed ? packed.byteLength * 2 : 4096);
      packed = {
        buf: this.device.createBuffer({
          size: Math.max(4, newByteLength),
          usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
        }),
        byteLength: newByteLength,
        length: itemCount * strideFloats,
        logicalLength: entry.length,
        data: null,
        version: 0,
      };
      if (!entry.packed) entry.packed = new Map();
      entry.packed.set(key, packed);
    }

    const out = new Float32Array(itemCount * strideFloats);
    for (let i = 0; i < itemCount; i++) {
      const srcOffset = i * logicalSlots;
      const dstOffset = i * strideFloats;
      out.set(entry.data.subarray(srcOffset, srcOffset + logicalSlots), dstOffset);
    }

    this.queue.writeBuffer(packed.buf, 0, out.buffer, out.byteOffset, out.byteLength);
    
    packed.length = out.length;
    packed.logicalLength = entry.length;
    packed.version = entry.version;
    return packed;
  }

  FromWasmAllocIndexBuffer(args) {
    const device = this.device;
    let entry = this.index_buffers[args.buffer_id];
    const u32 = new Uint32Array(this.memory.buffer, args.data.ptr, args.data.len);
    const requestedByteLength = u32.byteLength;
    if (!entry || !entry.buf || entry.byteLength < requestedByteLength) {
      const newByteLength = Math.max(requestedByteLength, entry ? entry.byteLength * 2 : 4096);
      entry = this.index_buffers[args.buffer_id] = {
        buf: device.createBuffer({ size: Math.max(4, newByteLength), usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST }),
        byteLength: newByteLength,
        length: u32.length,
      };
    }
    const copy = u32.slice();
    this.queue.writeBuffer(entry.buf, 0, copy.buffer, copy.byteOffset, requestedByteLength);
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
    if (!this.device) {
      return;
    }
    this._frame_id = (this._frame_id || 0) + 1;
    if (this.xr !== undefined) {
      this.xr.in_xr_pass = true;
    }
    let w = this.canvas.width | 0;
    let h = this.canvas.height | 0;
    let colorView;
    let depthView = null;
    const xr = this.xr;
    if (xr !== undefined && xr.in_xr_pass && xr.layer) {
      const L = xr.layer;
      if (L.colorTexture) {
        colorView = L.colorTexture.createView();
        w = L.framebufferWidth || w;
        h = L.framebufferHeight || h;
      } else {
        colorView = this.context.getCurrentTexture().createView();
      }
      if (L.depthStencilTexture) {
        depthView = L.depthStencilTexture.createView();
      }
    } else {
      colorView = this.context.getCurrentTexture().createView();
    }

    // NOTE: WebGL path is effectively "no depth" for most 2D UI.
    // Creating a depth attachment here can accidentally occlude 2D content if
    // depth_write is enabled on some draw calls. Keep depth only when XR provides it.
    // (Render-to-texture can still request depth explicitly via depth_targets.)

    this._pass_color_format = this.format;
    this._pass_extent = { w, h };
    this._pass_has_depth = false;

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
      depthStencilAttachment: undefined,
    });
  }

  FromWasmBeginRenderTexture(args) {
    if (!this.device) {
      return;
    }
    if (this.xr !== undefined) {
      this.xr.in_xr_pass = false;
    }

    const w = Math.max(1, args.width | 0);
    const h = Math.max(1, args.height | 0);
    const tgt = args.color_targets[0];
    const texId = tgt.texture_id | 0;
    const clearColor = tgt.clear_color;
    const needResize = (() => {
      const e = this.textures[texId];
      return !e || !e.texture || e.rtW !== w || e.rtH !== h;
    })();
    const loadOpColor = needResize || !tgt.init_only ? "clear" : "load";

    let entry = this.textures[texId];
    if (needResize) {
      const texture = this.device.createTexture({
        size: [w, h, 1],
        format: "bgra8unorm",
        usage:
          GPUTextureUsage.RENDER_ATTACHMENT
          | GPUTextureUsage.TEXTURE_BINDING
          | GPUTextureUsage.COPY_DST,
      });
      entry = this.textures[texId] = {
        texture,
        view: texture.createView(),
        w,
        h,
        format: "bgra8unorm",
        rtW: w,
        rtH: h,
        cube: false,
        version: (entry ? (entry.version | 0) + 1 : 1),
      };
    }

    let depthStencilAttachment;
    const dt = args.depth_target;
    if (dt && dt.texture_id) {
      const did = dt.texture_id | 0;
      const needDepth = (() => {
        const e = this.textures[did];
        return !e || !e.depthTexture || e.rtDW !== w || e.rtDH !== h;
      })();
      let dEntry = this.textures[did];
      if (needDepth) {
        const depthTexture = this.device.createTexture({
          size: [w, h, 1],
          format: "depth24plus",
          usage: GPUTextureUsage.RENDER_ATTACHMENT,
        });
        dEntry = this.textures[did] = {
          ...dEntry,
          depthTexture,
          depthView: depthTexture.createView(),
          rtDW: w,
          rtDH: h,
          version: (dEntry ? (dEntry.version | 0) + 1 : 1),
        };
      }
      const dView = this.textures[did].depthView;
      depthStencilAttachment = {
        view: dView,
        depthClearValue: dt.clear_depth,
        depthLoadOp: needDepth || !dt.init_only ? "clear" : "load",
        depthStoreOp: "store",
      };
    }

    this._pass_color_format = "bgra8unorm";
    this._pass_extent = { w, h };
    this._pass_has_depth = !!depthStencilAttachment;

    this._encoder = this.device.createCommandEncoder();
    this._pass = this._encoder.beginRenderPass({
      colorAttachments: [
        {
          view: entry.view,
          clearValue: clearColor,
          loadOp: loadOpColor,
          storeOp: "store",
        },
      ],
      depthStencilAttachment,
    });
  }

  FromWasmAllocTextureCube_BGRAu8_32(args) {
    if (!this.device) {
      return;
    }
    const w = Math.max(1, args.width | 0);
    const h = Math.max(1, args.height | 0);
    const rowBytes = w * 4;
    const bytesPerRow = (rowBytes + 255) & ~255;
    const faceBytes = rowBytes * h;
    const all = new Uint8Array(this.memory.buffer, args.data.ptr, faceBytes * 6).slice();
    const texture = this.device.createTexture({
      dimension: "2d",
      size: [w, h, 6],
      format: "bgra8unorm",
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
    });
    const staging = new Uint8Array(bytesPerRow * h);
    for (let face = 0; face < 6; face++) {
      const slice = all.subarray(face * faceBytes, (face + 1) * faceBytes);
      staging.fill(0);
      for (let row = 0; row < h; row++) {
        staging.set(slice.subarray(row * rowBytes, row * rowBytes + rowBytes), row * bytesPerRow);
      }
      this.queue.writeTexture(
        { texture, origin: { x: 0, y: 0, z: face } },
        staging,
        { offset: 0, bytesPerRow, rowsPerImage: h },
        { width: w, height: h, depthOrArrayLayers: 1 },
      );
    }
    const entry = this.textures[args.texture_id];
    this.textures[args.texture_id] = {
      texture,
      view: texture.createView({ dimension: "cube" }),
      w,
      h,
      format: "bgra8unorm",
      cube: true,
      version: (entry ? (entry.version | 0) + 1 : 1),
    };
  }

  FromWasmSetDefaultDepthAndBlendMode() {
    this._default_depth_write = true;
    this._default_backface_cull = false;
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

      const texIdsAt = at;
      const texIds = [];
      for (let i = 0; i < shader.texture_count; i++) {
        texIds.push(words[texIdsAt + i]);
      }
      while (texIds.length < 16) {
        texIds.push(NONE_TEX);
      }
      at += 16;

      this._emitSingleDraw(
        shader,
        vao,
        pass_ptr, pass_len,
        draw_list_ptr, draw_list_len,
        draw_call_ptr, draw_call_len,
        user_ptr, user_len,
        live_ptr, live_len,
        depth_write,
        backface_culling,
        texIds,
      );
    }

    // End + submit each pump (simple but correct).
    if (this._pass) {
      this._pass.end();
      this._pass = null;
    }
    if (this._encoder) {
      const cmd = this._encoder.finish();
      this._encoder = null;
      this.device.pushErrorScope('validation');
      this.device.pushErrorScope('internal');
      this.queue.submit([cmd]);
      this.device.popErrorScope().then(e => { if (e) console.error('[makepad:webgpu] internal GPU error:', e.message); });
      this.device.popErrorScope().then(e => { if (e) console.error('[makepad:webgpu] validation GPU error:', e.message); });
    }
  }

  on_xr_animation_frame(time, frame) {
    this._frame_id = (this._frame_id || 0) + 1;
    function empty_transform() {
      return {
        orientation: { a: 0, b: 0, c: 0, d: 0 },
        position: { x: 0, y: 0, z: 0 },
      };
    }
    function to_transform(pose_transform, tgt) {
      const po = pose_transform.inverse.orientation;
      const pp = pose_transform.position;
      const o = tgt.orientation;
      o.a = po.x;
      o.b = po.y;
      o.c = po.z;
      o.d = po.w;
      const p = tgt.position;
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
    if (this.xr === undefined) {
      return;
    }
    const ref_space = this.xr.ref_space;
    const xr = this.xr;
    xr.session.requestAnimationFrame(this.xr.on_animation_frame);
    xr.pose = frame.getViewerPose(ref_space);
    if (!xr.pose || !xr.pose.views || xr.pose.views.length < 2) {
      return;
    }
    get_matrices(xr.layer, xr.pose.views[0], xr.left_eye);
    get_matrices(xr.layer, xr.pose.views[1], xr.right_eye);
    if (xr.xr_update === undefined) {
      xr.xr_update = {
        time: 0,
        head_transform: empty_transform(),
        inputs: [],
      };
    }
    const xr_update = xr.xr_update;
    xr_update.time = time / 1000.0;
    to_transform(this.xr.pose.transform, xr_update.head_transform);
    const inputs = xr_update.inputs;
    for (let i = 0; i < inputs.length; i++) {
      inputs[i].active = false;
    }
    const input_sources = this.xr.session.inputSources;
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
      const input = inputs[i];
      const input_source = input_sources[i];
      const grip_pose = frame.getPose(input_source.gripSpace, ref_space);
      const ray_pose = frame.getPose(input_source.targetRaySpace, ref_space);
      if (grip_pose == null || ray_pose == null) {
        input.active = false;
        continue;
      }
      to_transform(grip_pose.transform, input.grip);
      to_transform(ray_pose.transform, input.ray);
      const buttons = input.buttons;
      const input_buttons = input_source.gamepad.buttons;
      for (let j = 0; j < input_buttons.length; j++) {
        if (buttons[j] === undefined) {
          buttons[j] = { pressed: 0, value: 0 };
        }
        buttons[j].pressed = input_buttons[j].pressed ? 1 : 0;
        buttons[j].value = input_buttons[j].value;
      }
      const axes = input.axes;
      const input_axes = input_source.gamepad.axes;
      for (let j = 0; j < input_axes.length; j++) {
        axes[j] = input_axes[j];
      }
    }
    this.to_wasm.ToWasmXRUpdate(xr_update);
    this.to_wasm.ToWasmAnimationFrame({ time: time / 1000.0 });
    this.in_animation_frame = true;
    this.do_wasm_pump();
    this.in_animation_frame = false;
  }

  FromWasmXrStartPresenting(_args) {
    if (this.xr !== undefined || !this.device || !this.context) {
      return;
    }
    if (!navigator.xr) {
      return;
    }
    const LayerCtor = globalThis.XRWebGPULayer;
    if (!LayerCtor) {
      console.warn("[makepad] XRWebGPULayer not available; WebGPU XR unsupported in this browser");
      return;
    }
    navigator.xr
      .requestSession("immersive-vr", { requiredFeatures: ["local-floor"] })
      .then((session) => {
        let layer;
        try {
          layer = new LayerCtor(session, this.context, { device: this.device });
        } catch (_e) {
          try {
            layer = new LayerCtor(session, { device: this.device, context: this.context });
          } catch (e2) {
            console.warn("[makepad] XRWebGPULayer construction failed", e2);
            return;
          }
        }
        session.updateRenderState({ baseLayer: layer });
        session.requestReferenceSpace("local-floor").then((ref_space) => {
          window.localStorage.setItem("xr_presenting", "true");
          this.xr = {
            left_eye: {},
            right_eye: {},
            layer,
            ref_space,
            session,
            in_xr_pass: false,
            on_animation_frame: (t, f) => this.on_xr_animation_frame(t, f),
          };
          session.requestAnimationFrame(this.xr.on_animation_frame);
          session.addEventListener("end", () => {
            window.localStorage.setItem("xr_presenting", "false");
            this.xr = undefined;
            this.FromWasmRequestAnimationFrame();
          });
        });
      })
      .catch((e) => console.warn("[makepad] XR session request failed", e));
  }

  FromWasmXrStopPresenting() {}

  FromWasmPrepareVideoPlayback(args) {
    if (!this.device) {
      return;
    }
    const key = `${args.video_id_lo}_${args.video_id_hi}`;
    const video = document.createElement("video");
    video.crossOrigin = "anonymous";
    video.playsInline = true;
    video.preload = "auto";
    video.loop = args.should_loop;
    video.muted = args.autoplay;
    const player = {
      video,
      texture_id: args.texture_id,
      video_id_lo: args.video_id_lo,
      video_id_hi: args.video_id_hi,
      playing: false,
      use_video_frame_callback: typeof video.requestVideoFrameCallback === "function",
      video_frame_callback_id: 0,
      texture_initialized: false,
    };
    this.video_players[key] = player;
    video.addEventListener("loadedmetadata", () => {
      const duration_ms = Math.round(video.duration * 1000);
      this.to_wasm.ToWasmVideoPlaybackPrepared({
        video_id_lo: args.video_id_lo,
        video_id_hi: args.video_id_hi,
        video_width: video.videoWidth,
        video_height: video.videoHeight,
        duration_lo: duration_ms & 0xffffffff,
        duration_hi: Math.floor(duration_ms / 0x100000000),
      });
      this.do_wasm_pump();
    });
    video.addEventListener("ended", () => {
      player.playing = false;
      this.cancel_video_frame_callback(player);
      this.to_wasm.ToWasmVideoPlaybackCompleted({
        video_id_lo: args.video_id_lo,
        video_id_hi: args.video_id_hi,
      });
      this.do_wasm_pump();
    });
    video.addEventListener("play", () => {
      player.playing = true;
      this.schedule_video_texture_updates(player);
    });
    video.addEventListener("pause", () => {
      player.playing = false;
      this.cancel_video_frame_callback(player);
    });
    video.src = args.source_url;
    if (args.autoplay) {
      video.play().catch((e) => console.warn("Video autoplay failed:", e));
    }
  }

  FromWasmBeginVideoPlayback(args) {
    const key = `${args.video_id_lo}_${args.video_id_hi}`;
    const player = this.video_players[key];
    if (player) {
      player.video.play().catch((e) => console.warn("Video play failed:", e));
    }
  }

  FromWasmPauseVideoPlayback(args) {
    const key = `${args.video_id_lo}_${args.video_id_hi}`;
    const player = this.video_players[key];
    if (player) {
      player.video.pause();
    }
  }

  FromWasmResumeVideoPlayback(args) {
    const key = `${args.video_id_lo}_${args.video_id_hi}`;
    const player = this.video_players[key];
    if (player) {
      player.video.play().catch((e) => console.warn("Video resume failed:", e));
    }
  }

  FromWasmMuteVideoPlayback(args) {
    const key = `${args.video_id_lo}_${args.video_id_hi}`;
    const player = this.video_players[key];
    if (player) {
      player.video.muted = true;
    }
  }

  FromWasmUnmuteVideoPlayback(args) {
    const key = `${args.video_id_lo}_${args.video_id_hi}`;
    const player = this.video_players[key];
    if (player) {
      player.video.muted = false;
    }
  }

  FromWasmSeekVideoPlayback(args) {
    const key = `${args.video_id_lo}_${args.video_id_hi}`;
    const player = this.video_players[key];
    if (player) {
      const position_ms = args.position_ms_lo + args.position_ms_hi * 0x100000000;
      player.video.currentTime = position_ms / 1000.0;
    }
  }

  FromWasmCleanupVideoPlaybackResources(args) {
    const key = `${args.video_id_lo}_${args.video_id_hi}`;
    const player = this.video_players[key];
    if (player) {
      player.playing = false;
      this.cancel_video_frame_callback(player);
      player.video.pause();
      player.video.removeAttribute("src");
      player.video.load();
      delete this.video_players[key];
      this.to_wasm.ToWasmVideoPlaybackResourcesReleased({
        video_id_lo: args.video_id_lo,
        video_id_hi: args.video_id_hi,
      });
      this.do_wasm_pump();
    }
  }

  ensure_video_animation_frame() {
    if (this.video_anim_frame_id) {
      return;
    }
    this.video_anim_frame_id = window.requestAnimationFrame(() => {
      this.video_anim_frame_id = 0;
      this.update_video_textures();
    });
  }

  schedule_video_texture_updates(player) {
    if (!player || !player.playing) {
      return;
    }
    if (player.use_video_frame_callback) {
      if (player.video_frame_callback_id) {
        return;
      }
      const key = `${player.video_id_lo}_${player.video_id_hi}`;
      player.video_frame_callback_id = player.video.requestVideoFrameCallback(() => {
        player.video_frame_callback_id = 0;
        if (!player.playing || this.video_players[key] !== player) {
          return;
        }
        if (this.update_video_texture(player)) {
          this.do_wasm_pump();
        }
        if (player.playing && this.video_players[key] === player) {
          this.schedule_video_texture_updates(player);
        }
      });
      return;
    }
    this.ensure_video_animation_frame();
  }

  cancel_video_frame_callback(player) {
    if (!player || !player.video_frame_callback_id) {
      return;
    }
    if (
      player.use_video_frame_callback
      && typeof player.video.cancelVideoFrameCallback === "function"
    ) {
      player.video.cancelVideoFrameCallback(player.video_frame_callback_id);
    }
    player.video_frame_callback_id = 0;
  }

  update_video_texture(player) {
    const video = player.video;
    if (video.readyState < 2 || !this.device) {
      return false;
    }
    if (typeof this.queue.copyExternalImageToTexture !== "function") {
      return false;
    }
    const vw = video.videoWidth | 0;
    const vh = video.videoHeight | 0;
    if (vw < 1 || vh < 1) {
      return false;
    }
    let entry = this.textures[player.texture_id];
    if (
      !entry
      || !entry.texture
      || entry.w !== vw
      || entry.h !== vh
      || entry.format !== "rgba8unorm"
    ) {
      const texture = this.device.createTexture({
        size: [vw, vh, 1],
        format: "rgba8unorm",
        usage:
          GPUTextureUsage.TEXTURE_BINDING
          | GPUTextureUsage.COPY_DST
          | GPUTextureUsage.RENDER_ATTACHMENT,
      });
      entry = this.textures[player.texture_id] = {
        texture,
        view: texture.createView(),
        w: vw,
        h: vh,
        format: "rgba8unorm",
        version: (entry ? (entry.version | 0) + 1 : 1),
      };
      player.texture_initialized = true;
    }
    this.queue.copyExternalImageToTexture(
      { source: video },
      { texture: entry.texture },
      { width: vw, height: vh, depthOrArrayLayers: 1 },
    );
    const current_ms = Math.round(video.currentTime * 1000);
    this.to_wasm.ToWasmVideoTextureUpdated({
      video_id_lo: player.video_id_lo,
      video_id_hi: player.video_id_hi,
      current_position_lo: current_ms & 0xffffffff,
      current_position_hi: Math.floor(current_ms / 0x100000000),
    });
    return true;
  }

  update_video_textures() {
    let any_fallback_playing = false;
    let any_updated = false;
    for (const key in this.video_players) {
      const player = this.video_players[key];
      if (!player.playing) {
        continue;
      }
      if (player.use_video_frame_callback) {
        continue;
      }
      any_fallback_playing = true;
      if (this.update_video_texture(player)) {
        any_updated = true;
      }
    }
    if (any_updated) {
      this.do_wasm_pump();
    }
    if (any_fallback_playing) {
      this.ensure_video_animation_frame();
    }
  }

  _write_texture_2d_bytes(texture, w, h, bytes, bytesPerPixel) {
    const rowBytes = w * bytesPerPixel;
    const bytesPerRow = (rowBytes + 255) & ~255;
    if (bytesPerRow === rowBytes) {
      this.queue.writeTexture(
        { texture },
        bytes,
        { bytesPerRow, rowsPerImage: h },
        { width: w, height: h, depthOrArrayLayers: 1 },
      );
      return;
    }
    const staging = new Uint8Array(bytesPerRow * h);
    for (let row = 0; row < h; row++) {
      staging.set(bytes.subarray(row * rowBytes, row * rowBytes + rowBytes), row * bytesPerRow);
    }
    this.queue.writeTexture(
      { texture },
      staging,
      { bytesPerRow, rowsPerImage: h },
      { width: w, height: h, depthOrArrayLayers: 1 },
    );
  }

  _emitSingleDraw(
    shader,
    vao,
    pass_ptr, pass_len,
    list_ptr, list_len,
    call_ptr, call_len,
    user_ptr, user_len,
    live_ptr, live_len,
    depth_write,
    backface_culling,
    texIds
  ) {
    const NONE_TEX = 0xffffffff;
    
    if (!this._scratch_f32) {
      this._scratch_f32 = new Float32Array(4096);
    }
    if (!this._scratch_pass_f32) {
      this._scratch_pass_f32 = new Float32Array(1024);
    }

    // In WebGPU, queue.writeBuffer() resolves BEFORE the submitted command buffer.
    // This means if we reuse the same UBO for multiple draw calls within a render pass,
    // only the last writeBuffer wins and all draws see that data.
    // Fix: allocate a separate GPUBuffer per draw call from a pool, reset each frame.

    if (!shader._ubo_pool) {
      shader._ubo_pool = [];      // Array of {pass, list, call, user, live} buffer sets
      shader._ubo_pool_idx = 0;
    }
    // Reset pool index at start of each frame
    if (shader._ubo_pool_frame !== this._frame_id) {
      shader._ubo_pool_idx = 0;
      shader._ubo_pool_frame = this._frame_id;
    }

    const poolIdx = shader._ubo_pool_idx++;
    if (!shader._ubo_pool[poolIdx]) {
      shader._ubo_pool[poolIdx] = {
        pass: null, list: null, call: null, user: null, live: null,
      };
    }
    const poolSlot = shader._ubo_pool[poolIdx];

    const ensurePoolBuf = (slot, byteLen) => {
      let buf = poolSlot[slot];
      const needed = Math.max(256, (byteLen + 255) & ~255);
      if (!buf || buf.size < needed) {
        if (buf) buf.destroy();
        buf = this.device.createBuffer({
          size: needed,
          usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
        });
        poolSlot[slot] = buf;
      }
      return buf;
    };

    const writePoolUBO = (slot, ptr, len) => {
      const byteLen = len * 4;
      if (len === 0) return ensurePoolBuf(slot, 256);
      const buf = ensurePoolBuf(slot, byteLen);
      if (len > this._scratch_f32.length) {
        this._scratch_f32 = new Float32Array(Math.max(len, this._scratch_f32.length * 2));
      }
      const src = new Float32Array(this.memory.buffer, ptr, len);
      const dst = this._scratch_f32.subarray(0, len);
      dst.set(src);
      this.queue.writeBuffer(buf, 0, dst.buffer, dst.byteOffset, byteLen);
      return buf;
    };

    // Write pass uniforms into a dedicated scratch buffer (may be mutated for XR)
    if (pass_len > this._scratch_pass_f32.length) {
      this._scratch_pass_f32 = new Float32Array(Math.max(pass_len, this._scratch_pass_f32.length * 2));
    }
    const pass_u = this._scratch_pass_f32.subarray(0, pass_len);
    if (pass_len > 0) {
      pass_u.set(new Float32Array(this.memory.buffer, pass_ptr, pass_len));
    }

    const passBuf = ensurePoolBuf("pass", pass_len * 4);
    if (pass_len > 0) {
      this.queue.writeBuffer(passBuf, 0, pass_u.buffer, pass_u.byteOffset, pass_len * 4);
    }
    const listBuf = writePoolUBO("list", list_ptr, list_len);
    const callBuf = writePoolUBO("call", call_ptr, call_len);
    const userBuf = writePoolUBO("user", user_ptr, user_len);
    const liveBuf = writePoolUBO("live", live_ptr, live_len);

    // Build a direct bind group using the pool buffers for this draw call
    const textureViews = new Array(shader.texture_count);
    const textureEntries = new Array(shader.texture_count);
    for (let i = 0; i < shader.texture_count; i++) {
      const tid = texIds[i];
      const texId = tid === undefined || tid === null ? NONE_TEX : tid >>> 0;
      if (texId !== NONE_TEX) {
        const tex = this.textures[texId];
        textureViews[i] = tex ? tex.view : null;
        textureEntries[i] = tex || null;
      } else {
        textureViews[i] = null;
        textureEntries[i] = null;
      }
    }

    const variant = this.get_pipeline_variant(shader, textureEntries, depth_write, backface_culling);

    // Create bind group directly with pool buffers (no caching — each draw call is unique)
    const bindGroup = this._create_bind_group_with_pool_bufs(
      shader, variant, textureViews, textureEntries,
      passBuf, listBuf, callBuf, userBuf, liveBuf
    );

    this._pass.setPipeline(variant.pipeline);
    this._pass.setBindGroup(0, bindGroup);

    const geomRaw = this.array_buffers[vao.geom_vb_id];
    const instRaw = this.array_buffers[vao.inst_vb_id];
    const ib = this.index_buffers[vao.geom_ib_id];
    if (!geomRaw || !instRaw || !ib) {
      return;
    }
    const geom = this.get_packed_vertex_buffer(geomRaw, shader.geometry_slots, shader.geom_vec4s);
    const inst = this.get_packed_vertex_buffer(instRaw, shader.instance_slots, shader.inst_vec4s);
    this._pass.setVertexBuffer(0, geom.buf);
    this._pass.setVertexBuffer(1, inst.buf);
    this._pass.setIndexBuffer(ib.buf, "uint32");
    const indexCount = ib.length | 0;
    const instanceCount = shader.instance_slots > 0
      ? (((instRaw.length | 0) / shader.instance_slots) | 0)
      : 0;
    if (instanceCount <= 0 || indexCount <= 0) {
      return;
    }

    const xr = this.xr;
    if (xr !== undefined && xr.in_xr_pass && xr.left_eye && xr.left_eye.viewport) {
      const applyEye = (eye) => {
        const vp = eye.viewport;
        this._pass.setViewport(vp.x | 0, vp.y | 0, Math.max(1, vp.width | 0), Math.max(1, vp.height | 0), 0, 1);
        this._pass.setScissorRect(vp.x | 0, vp.y | 0, Math.max(1, vp.width | 0), Math.max(1, vp.height | 0));
        const m = pass_u;
        const mp = eye.projection_matrix;
        for (let i = 0; i < 16; i++) m[i] = mp[i];
        const mt = eye.transform_matrix;
        for (let i = 0; i < 16; i++) m[i + 16] = mt[i];
        const mi = eye.invtransform_matrix;
        for (let i = 0; i < 16; i++) m[i + 32] = mi[i];
        this.queue.writeBuffer(passBuf, 0, pass_u.buffer, pass_u.byteOffset, pass_u.byteLength);
        this._pass.drawIndexed(indexCount, instanceCount, 0, 0, 0);
      };
      applyEye(xr.left_eye);
      applyEye(xr.right_eye);
    } else {
      this._pass.drawIndexed(indexCount, instanceCount, 0, 0, 0);
    }
  }

  _create_bind_group_with_pool_bufs(shader, variant, textureViews, textureEntries, passBuf, listBuf, callBuf, userBuf, liveBuf) {
    const entries = [];

    // Map named UBO bindings to their pool buffers
    const uboMap = new Map();
    if (shader.ubo_binding_pass >= 0) uboMap.set(shader.ubo_binding_pass, passBuf);
    if (shader.ubo_binding_draw_list >= 0) uboMap.set(shader.ubo_binding_draw_list, listBuf);
    if (shader.ubo_binding_draw_call >= 0) uboMap.set(shader.ubo_binding_draw_call, callBuf);
    if (shader.ubo_binding_user >= 0) uboMap.set(shader.ubo_binding_user, userBuf);
    if (shader.ubo_binding_live >= 0) uboMap.set(shader.ubo_binding_live, liveBuf);

    for (const [binding, kind] of shader.binding_kinds.entries()) {
      if (kind !== "buffer") continue;
      const buf = uboMap.get(binding) || shader.ubos.get(binding) || passBuf;
      entries.push({ binding, resource: { buffer: buf } });
    }

    // Bind samplers
    for (const sb of shader.samplerBindings || []) {
      const samplerIndex = sb.samplerIndex | 0;
      const b = sb.binding | 0;
      if (shader.binding_kinds?.get(b) !== "sampler") continue;
      const bindingType = this.sampler_binding_type_for_index(shader, samplerIndex, textureEntries);
      const desc = shader.samplerDescs[samplerIndex];
      const useOriginal =
        (bindingType === "filtering" && desc && (desc.filter | 0) !== 0)
        || (bindingType === "non-filtering" && (!desc || (desc.filter | 0) === 0));
      entries.push({
        binding: b,
        resource: useOriginal
          ? (shader.samplers[samplerIndex] || this.get_fallback_sampler())
          : this.get_sampler_resource(desc, bindingType),
      });
    }

    // Bind textures
    for (const tb of shader.textureBindings || []) {
      const isDepth = tb.declaredSampleType === "depth";
      const entry = textureEntries[tb.textureIndex];
      const view = isDepth
        ? ((entry && entry.depthView) ? entry.depthView : this.get_fallback_depth_texture_view())
        : (textureViews[tb.textureIndex] || this.get_fallback_texture_view());
      const b = tb.binding | 0;
      if (shader.binding_kinds?.get(b) !== "texture") continue;
      entries.push({ binding: b, resource: view });
    }

    // Deduplicate bindings
    const byBinding = new Map();
    for (const e of entries) byBinding.set(e.binding | 0, e);
    const uniqueEntries = Array.from(byBinding.values()).sort((a, b) => (a.binding | 0) - (b.binding | 0));

    return this.device.createBindGroup({ layout: variant.bindGroupLayout, entries: uniqueEntries });
  }

  FromWasmDrawCall(args) {
    if (!this.device || !this._pass) {
      return;
    }
    const shader = this.draw_shaders[args.shader_id];
    const vao = this.vaos[args.vao_id];
    if (!shader || !vao) {
      return;
    }
    const pass_ptr = args.pass_uniforms.ptr; const pass_len = args.pass_uniforms.len;
    const list_ptr = args.draw_list_uniforms.ptr; const list_len = args.draw_list_uniforms.len;
    const call_ptr = args.draw_call_uniforms.ptr; const call_len = args.draw_call_uniforms.len;
    const user_ptr = args.user_uniforms.ptr; const user_len = args.user_uniforms.len;
    const live_ptr = args.live_uniforms.ptr; const live_len = args.live_uniforms.len;

    const texIds = [];
    for (let i = 0; i < shader.texture_count; i++) {
      const t = args.textures[i];
      texIds.push(t == null ? 0xffffffff : t >>> 0);
    }
    while (texIds.length < 16) {
      texIds.push(0xffffffff);
    }
    this._emitSingleDraw(
      shader,
      vao,
      pass_ptr, pass_len,
      list_ptr, list_len,
      call_ptr, call_len,
      user_ptr, user_len,
      live_ptr, live_len,
      !!args.depth_write,
      !!args.backface_culling,
      texIds,
    );
  }

  ensure_ubo(shader, field, requiredBytes) {
    const buf = shader[field];
    if (buf && buf.size >= requiredBytes) return;
    const nextSize = Math.max(256, (requiredBytes + 255) & ~255);
    const next = this.device.createBuffer({
      size: nextSize,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    });
    shader[field] = next;
    // Keep `shader.ubos` in sync, since bind groups are sourced from `ubos`.
    let binding = -1;
    switch (field) {
      case "ubo_pass": binding = shader.ubo_binding_pass; break;
      case "ubo_draw_list": binding = shader.ubo_binding_draw_list; break;
      case "ubo_draw_call": binding = shader.ubo_binding_draw_call; break;
      case "ubo_user": binding = shader.ubo_binding_user; break;
      case "ubo_live": binding = shader.ubo_binding_live; break;
      default: break;
    }
    if (binding >= 0) {
      shader.ubos.set(binding, next);
    }
    shader.bindGroups = new Map();
  }

  // --- Texture uploads from wasm ---

  FromWasmAllocTextureImage2D_BGRAu8_32(args) {
    if (!this.device) {
      this._pending_until_ready.push({ name: "FromWasmAllocTextureImage2D_BGRAu8_32", args });
      return;
    }
    // Input bytes are BGRA; store them in an RGBA texture and let the shader's
    // `sample*_bgra` helpers swizzle to RGBA (mirrors the WebGL path).
    const tid = args.texture_id | 0;
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
        version: (entry ? (entry.version | 0) + 1 : 1),
      };
    }
    this._write_texture_2d_bytes(entry.texture, w, h, bytes, 4);
  }

  FromWasmAllocTextureImage2D_Ru8(args) {
    if (!this.device) {
      this._pending_until_ready.push({ name: "FromWasmAllocTextureImage2D_Ru8", args });
      return;
    }
    const tid = args.texture_id | 0;
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
        version: (entry ? (entry.version | 0) + 1 : 1),
      };
    }
    this._write_texture_2d_bytes(entry.texture, w, h, bytes, 1);
  }

  FromWasmAllocTextureImage2D_RGBAf32(args) {
    if (!this.device) {
      this._pending_until_ready.push({ name: "FromWasmAllocTextureImage2D_RGBAf32", args });
      return;
    }
    const tid = args.texture_id | 0;
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
        version: (entry ? (entry.version | 0) + 1 : 1),
      };
    }
    this._write_texture_2d_bytes(entry.texture, w, h, new Uint8Array(f32.buffer), 16);
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

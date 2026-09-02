use {
    crate::{
        cx::Cx,
        draw_vars::{DrawVars, DRAW_CALL_UNIFORM_BUFFER_SLOTS},
        geometry::GeometryId,
        makepad_live_id::*,
        makepad_script::heap::ScriptHeap,
        makepad_script::pod::{ScriptPodTy, ScriptPodVec},
        makepad_script::shader::*,
        makepad_script::value::{ScriptIp, ScriptObject, ScriptPodType},
        makepad_script::NoTrap,
        makepad_script::ScriptObjectRef,
        os::CxOsDrawShader,
    },
    std::{
        collections::BTreeSet,
        collections::HashMap,
        ops::{Index, IndexMut},
    },
};

// Re-export UniformBufferBindings for use in other modules
pub use makepad_script::shader::UniformBufferBindings;

#[derive(Debug, Clone, PartialEq)]
pub struct CxDrawShaderOptions {
    pub draw_call_group: LiveId,
    pub debug_id: Option<LiveId>,
    pub depth_write: bool,
    pub alpha_blend: bool,
    pub backface_culling: bool,
}

impl Default for CxDrawShaderOptions {
    fn default() -> Self {
        Self {
            draw_call_group: LiveId(0),
            debug_id: None,
            depth_write: true,
            alpha_blend: true,
            backface_culling: false,
        }
    }
}

impl CxDrawShaderOptions {
    /*
    pub fn from_ptr(cx: &Cx, draw_shader_ptr: DrawShaderPtr) -> Self {
        let live_registry_cp = cx.live_registry.clone();
        let live_registry = live_registry_cp.borrow();
        let doc = live_registry.ptr_to_doc(draw_shader_ptr.0);
        let mut ret = Self::default();
        // copy in per-instance settings from the DSL
        let mut node_iter = doc.nodes.first_child(draw_shader_ptr.node_index());
        while let Some(node_index) = node_iter {
            let node = &doc.nodes[node_index];
            match node.id {
                live_id!(draw_call_group) => if let LiveValue::Id(id) = node.value {
                    ret.draw_call_group = id;
                }
                live_id!(debug_id) => if let LiveValue::Id(id) = node.value {
                    ret.debug_id = Some(id);
                }
                _ => ()
            }
            node_iter = doc.nodes.next_child(node_index);
        }
        ret
    }*/

    pub fn _appendable_drawcall(&self, other: &Self) -> bool {
        self == other
    }
}

/*
#[derive(Default)]
pub struct CxDrawShaderItem {
    pub draw_shader_id: usize,
    pub options: CxDrawShaderOptions
}*/

#[derive(Default)]
pub struct CxDrawShaders {
    pub shaders: Vec<CxDrawShader>,
    pub os_shaders: Vec<CxOsDrawShader>,
    pub compile_set: BTreeSet<usize>,
    /// Explicit const-table mode override; None = decide from the
    /// environment / remote bridge (see `Cx::shader_const_table_mode`).
    pub const_table_mode: Option<bool>,

    pub cache_object_reuse_epoch_seen: u64,
    /// Keyed by (heap key, object): an isolate's objects share index space
    /// with the app heap's and must never hit its entries.
    pub cache_object_id_to_shader: HashMap<(usize, ScriptObject), DrawShaderId>,
    pub cache_functions_to_shader: LiveIdMap<LiveId, DrawShaderId>,
    pub cache_code_to_shader: HashMap<CxDrawShaderCode, DrawShaderId>,
    //pub ptr_to_item: HashMap<DrawShaderPtr, CxDrawShaderItem>,
    //pub fingerprints: Vec<DrawShaderFingerprint>,
    //pub error_set: HashSet<DrawShaderPtr>,
    // pub error_fingerprints: Vec<Vec<LiveNode >>,
}

impl CxDrawShaders {
    pub fn reset_for_live_reload(&mut self) {
        self.cache_object_id_to_shader.clear();
        self.cache_functions_to_shader.clear();
    }
}

impl Cx {
    /// Whether draw shaders compile with the constant table: annotated float
    /// literals in fn bodies become hot-patchable scope-uniform slots
    /// instead of folded code. Explicit `set_shader_const_table_mode` wins;
    /// else `MAKEPAD_SHADER_CONST_TABLE=1|0`; else on exactly when the
    /// remote bridge (the tweaker's channel) is active. Off, codegen is
    /// byte-identical to a build without the feature.
    pub fn shader_const_table_mode(&self) -> bool {
        if let Some(on) = self.draw_shaders.const_table_mode {
            return on;
        }
        match std::env::var("MAKEPAD_SHADER_CONST_TABLE").as_deref() {
            Ok("1") | Ok("true") | Ok("on") => true,
            Ok("0") | Ok("false") | Ok("off") => false,
            _ => crate::remote::is_active(),
        }
    }

    /// Force the const-table mode. Takes effect for shaders compiled from
    /// now on: the shader caches are dropped so a re-applied draw object
    /// recompiles, but a DrawVars holding a compiled shader keeps it until
    /// it is re-applied (live edit / script reapply).
    pub fn set_shader_const_table_mode(&mut self, on: bool) {
        if self.draw_shaders.const_table_mode == Some(on) {
            return;
        }
        self.draw_shaders.const_table_mode = Some(on);
        self.draw_shaders.reset_for_live_reload();
        self.draw_shaders.cache_code_to_shader.clear();
    }

    /// The hot-patchable constants of a compiled shader (empty when it was
    /// compiled with the table off or has no annotated literals).
    pub fn shader_const_table(&self, shader_id: DrawShaderId) -> &[DrawShaderTableConst] {
        match self.draw_shaders.shaders.get(shader_id.index) {
            Some(sh) => &sh.mapping.table_consts,
            None => &[],
        }
    }

    /// Hot-patch one table constant: the new value reaches the GPU on the
    /// next frame through the scope-uniform buffer — no recompile, and the
    /// shader source is untouched. Every draw sharing this compiled shader
    /// changes together. Returns false for an unknown shader or index.
    pub fn shader_const_patch(&mut self, shader_id: DrawShaderId, index: usize, value: f32) -> bool {
        let Some(sh) = self.draw_shaders.shaders.get_mut(shader_id.index) else {
            return false;
        };
        if !sh.mapping.patch_table_const(index, value) {
            return false;
        }
        self.redraw_all();
        true
    }

    /// Put one table constant back to the literal in the source.
    pub fn shader_const_reset(&mut self, shader_id: DrawShaderId, index: usize) -> bool {
        let initial = match self
            .draw_shaders
            .shaders
            .get(shader_id.index)
            .and_then(|sh| sh.mapping.table_consts.get(index))
        {
            Some(tc) => tc.initial,
            None => return false,
        };
        self.shader_const_patch(shader_id, index, initial)
    }
}

impl Cx {
    pub fn flush_draw_shaders(&mut self) {
        /*
        self.shader_registry.flush_registry();
        self.draw_shaders.shaders.clear();
        self.draw_shaders.ptr_to_item.clear();
        self.draw_shaders.fingerprints.clear();
        self.draw_shaders.error_set.clear();
        self.draw_shaders.error_fingerprints.clear();*/
    }
}

impl Index<usize> for CxDrawShaders {
    type Output = CxDrawShader;
    fn index(&self, index: usize) -> &Self::Output {
        &self.shaders[index]
    }
}

impl IndexMut<usize> for CxDrawShaders {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.shaders[index]
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct DrawShaderId {
    pub index: usize,
    //pub draw_shader_ptr: DrawShaderPtr
}

impl DrawShaderId {
    pub fn false_compare_check(&self) -> u64 {
        (self.index as u64) << 32 //| self.draw_shader_ptr.0.index as u64
    }
}

pub struct CxDrawShader {
    pub debug_id: LiveId,
    pub os_shader_id: Option<usize>,
    pub mapping: CxDrawShaderMapping,
}

#[derive(Clone, Debug)]
pub struct DrawShaderInputs {
    pub inputs: Vec<DrawShaderInput>,
    pub packing_method: DrawShaderInputPacking,
    pub total_slots: usize,
}

#[derive(Clone, Copy, Debug)]
pub enum DrawShaderInputPacking {
    Attribute,
    UniformsGLSLTight,
    UniformsGLSL140,
    #[allow(dead_code)]
    UniformsHLSL,
    #[allow(dead_code)]
    UniformsMetal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawShaderAttrFormat {
    Float,
    UInt,
    SInt,
}

impl Default for DrawShaderAttrFormat {
    fn default() -> Self {
        Self::Float
    }
}

#[derive(Clone, Debug)]
pub struct DrawShaderInput {
    pub id: LiveId,
    //pub ty: ShaderTy,
    pub offset: usize,
    pub slots: usize,
    pub attr_format: DrawShaderAttrFormat,
    // pub live_ptr: Option<LivePtr>
}

fn uniform_packing() -> DrawShaderInputPacking {
    #[cfg(any(target_arch = "wasm32"))]
    {
        return DrawShaderInputPacking::UniformsGLSL140;
    }

    #[cfg(any(target_os = "android", target_os = "linux"))]
    {
        return DrawShaderInputPacking::UniformsGLSL140;
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    {
        return DrawShaderInputPacking::UniformsMetal;
    }

    #[cfg(target_os = "windows")]
    {
        return DrawShaderInputPacking::UniformsHLSL;
    }
}

impl DrawShaderInputs {
    pub fn new(packing_method: DrawShaderInputPacking) -> Self {
        Self {
            inputs: Vec::new(),
            packing_method,
            total_slots: 0,
        }
    }

    pub fn push(&mut self, id: LiveId, slots: usize, attr_format: DrawShaderAttrFormat) {
        match self.packing_method {
            DrawShaderInputPacking::Attribute => {
                let needs_int_align = attr_format != DrawShaderAttrFormat::Float && slots > 1;
                if needs_int_align && (self.total_slots & 3) != 0 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    attr_format,
                });
                self.total_slots += slots;
                if needs_int_align && (self.total_slots & 3) != 0 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
            }
            DrawShaderInputPacking::UniformsGLSLTight => {
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    attr_format,
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsGLSL140 => {
                // std140 alignment rules:
                // scalar (1 slot): no alignment requirement
                // vec2 (2 slots): 2-slot aligned
                // vec3/vec4 (3-4 slots): 4-slot aligned
                // larger (matrices, arrays): 4-slot aligned
                let alignment = match slots {
                    1 => 1,
                    2 => 2,
                    _ => 4,
                };
                if self.total_slots % alignment != 0 {
                    self.total_slots += alignment - (self.total_slots % alignment);
                }
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    attr_format,
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsHLSL => {
                if slots > 4 {
                    if (self.total_slots & 3) != 0 {
                        self.total_slots += 4 - (self.total_slots & 3);
                    }
                } else if (self.total_slots & 3) + slots > 4 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    attr_format,
                });
                self.total_slots += slots;
            }
            DrawShaderInputPacking::UniformsMetal => {
                // Metal struct alignment rules:
                // float (1 slot): 4-byte aligned (1-float)
                // float2 (2 slots): 8-byte aligned (2-float)
                // float3/float4 (3-4 slots): 16-byte aligned (4-float)
                // larger (matrices, arrays): 16-byte aligned (4-float)
                let aligned_slots = if slots == 3 { 4 } else { slots };
                let alignment = match aligned_slots {
                    1 => 1,
                    2 => 2,
                    _ => 4,
                };
                if self.total_slots % alignment != 0 {
                    self.total_slots += alignment - (self.total_slots % alignment);
                }
                self.inputs.push(DrawShaderInput {
                    id,
                    offset: self.total_slots,
                    slots,
                    attr_format,
                });
                self.total_slots += aligned_slots;
            }
        }
    }

    pub fn finalize(&mut self) {
        match self.packing_method {
            DrawShaderInputPacking::Attribute => (),
            DrawShaderInputPacking::UniformsGLSLTight => (),
            DrawShaderInputPacking::UniformsHLSL
            | DrawShaderInputPacking::UniformsMetal
            | DrawShaderInputPacking::UniformsGLSL140 => {
                if self.total_slots & 3 > 0 {
                    self.total_slots += 4 - (self.total_slots & 3);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct DrawShaderTextureInput {
    pub id: LiveId,
    pub tex_type: TextureType,
}

#[derive(Clone, Debug)]
pub struct DrawShaderUniformBufferInput {
    pub id: LiveId,
    pub block_name: String,
    pub ty: ScriptPodType,
    pub size: usize,
    pub align: usize,
    pub buffer_index: usize,
}

/// The pixel format of the color attachment a shader renders into. Backends
/// that bake the target format into their pipeline state (Metal, D3D,
/// Vulkan) read this when building the pipeline; GL-family backends ignore
/// it (the FBO's texture format decides). Declared in the shader DSL as
/// `color_format: @Rf32`; the default is the swapchain's BGRA8.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum DrawShaderColorFormat {
    #[default]
    Bgra8Unorm,
    /// BGRA8 with blending DISABLED: raw component writes for data passes
    /// whose alpha channel is payload, not opacity (an SDF byte, a coverage
    /// flag). Under the default premultiplied-over pipeline dst alpha can
    /// only ever grow (`a_out = a_src + a_dst·(1-a_src)`), so alpha-as-data
    /// is unwritable without this. Declared as `color_format: @Bgra8NoBlend`.
    Bgra8NoBlend,
    /// Single-channel 32-bit float (pairs with `TextureFormat::RenderRf32`).
    /// Blending is disabled on such pipelines — float blending is not
    /// universally supported and the consumers are data passes.
    Rf32,
    /// Four-channel 16-bit float (pairs with `TextureFormat::RenderRGBAf16`).
    /// Blending disabled — simulation/data passes write whole texels.
    /// Declared as `color_format: @Rgba16F`.
    Rgba16F,
    /// Four-channel 32-bit float (pairs with `TextureFormat::RenderRGBAf32`):
    /// the GPU-simulation state format (particle pos/vel, fluid fields).
    /// Blending disabled — simulation/data passes write whole texels.
    /// Declared as `color_format: @Rgba32F`.
    Rgba32F,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct DrawShaderFlags {
    pub debug_draw: bool,
    pub debug_layout: bool,
    pub debug_code: bool,
    pub draw_call_nocompare: bool,
    pub draw_call_always: bool,
    pub async_compile: bool,
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub enum CxDrawShaderCode {
    Separate { vertex: String, fragment: String },
    Combined { code: String },
}

/// What fills one scope-uniform slot: a script scope value read from the
/// heap, or one of the shader's table constants.
#[derive(Clone, Copy, Debug)]
pub enum ScopeUniformSlot {
    Scope(ScriptObject, LiveId),
    Const(usize),
}

/// A hot-patchable shader constant: a float literal in a shader fn body
/// that carried a `/** name [min..max] [step s] */` annotation and was
/// compiled under const-table mode. Patching writes the scope-uniform slot
/// (`input`) and never touches the source; `ip` names the literal for the
/// change ledger.
#[derive(Clone, Debug)]
pub struct DrawShaderTableConst {
    /// Scope-uniform io name (`ct<n>`).
    pub shader_name: LiveId,
    /// Annotation text as written.
    pub doc: String,
    /// Parsed hint: friendly name and optional range/step.
    pub name: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    /// The literal in the source.
    pub initial: f32,
    /// The live value (what the GPU reads).
    pub value: f32,
    /// Index into `scope_uniforms.inputs`.
    pub input: usize,
    /// The literal's immediate ip (file:line:col via the script code).
    pub ip: ScriptIp,
}

#[derive(Clone)]
pub struct CxDrawShaderMapping {
    pub source: ScriptObjectRef,
    pub code: CxDrawShaderCode,
    pub flags: DrawShaderFlags,
    pub instances: DrawShaderInputs,
    pub dyn_instances: DrawShaderInputs,
    pub dyn_uniforms: DrawShaderInputs,
    pub geometries: DrawShaderInputs,
    pub textures: Vec<DrawShaderTextureInput>,
    pub uniform_buffers: Vec<DrawShaderUniformBufferInput>,
    pub samplers: Vec<ShaderSampler>,
    pub texture_sampler_indices: Vec<usize>,
    pub uses_time: bool,
    pub rect_pos: Option<usize>,
    pub rect_size: Option<usize>,
    pub draw_clip: Option<usize>,
    pub uniform_buffer_bindings: UniformBufferBindings,
    pub scope_uniforms: DrawShaderInputs,
    pub scope_uniform_sources: Vec<ScopeUniformSlot>,
    pub scope_uniforms_buf: Vec<f32>,
    /// The shader's hot-patchable constants (annotated literals compiled
    /// under const-table mode), each backed by one scope-uniform slot.
    pub table_consts: Vec<DrawShaderTableConst>,
    /// Bumped by every patch of `scope_uniforms_buf`; backends that keep a
    /// GPU-side copy of the buffer re-upload when it moves.
    pub scope_uniforms_gen: u64,
    pub geometry_id: Option<GeometryId>,
    /// Total f32 slots in the varying buffer (instances + explicit varyings).
    /// Set by the headless backend during shader compilation.
    pub varying_total_slots: usize,
    /// The color-attachment format this shader's pipeline targets.
    pub color_format: DrawShaderColorFormat,
}

impl CxDrawShaderMapping {
    fn attr_format_from_pod_type(ty: &ScriptPodTy) -> DrawShaderAttrFormat {
        match ty {
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32 => DrawShaderAttrFormat::UInt,
            ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => DrawShaderAttrFormat::SInt,
            ScriptPodTy::Bool => DrawShaderAttrFormat::UInt,
            ScriptPodTy::Vec(vec_ty) => match vec_ty {
                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u => {
                    DrawShaderAttrFormat::UInt
                }
                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => {
                    DrawShaderAttrFormat::SInt
                }
                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b => {
                    DrawShaderAttrFormat::UInt
                }
                _ => DrawShaderAttrFormat::Float,
            },
            _ => DrawShaderAttrFormat::Float,
        }
    }

    pub fn debug_dump_shader_draw_call(
        backend: &str,
        draw_item_id: usize,
        draw_shader: &CxDrawShader,
        draw_call: &crate::draw_list::CxDrawCall,
        instance_data: &[f32],
        instances: usize,
    ) {
        let instance_slots = draw_shader.mapping.instances.total_slots;
        if instance_slots == 0 {
            crate::log!(
                "debug_draw [{}] item={} shader={} debug_id={:?}: no instance layout",
                backend,
                draw_item_id,
                draw_call.draw_shader_id.index,
                draw_call.options.debug_id.unwrap_or(draw_shader.debug_id)
            );
            return;
        }

        let dyn_slots = draw_shader
            .mapping
            .dyn_uniforms
            .total_slots
            .min(draw_call.dyn_uniforms.len());
        crate::log!(
        "debug_draw [{}] item={} shader={} debug_id={:?} instances={} instance_slots={} dyn_uniform_slots={}",
        backend,
        draw_item_id,
        draw_call.draw_shader_id.index,
        draw_call.options.debug_id.unwrap_or(draw_shader.debug_id),
        instances,
        instance_slots,
        dyn_slots
    );

        for input in &draw_shader.mapping.dyn_uniforms.inputs {
            if input.offset >= dyn_slots {
                continue;
            }
            let end = (input.offset + input.slots).min(dyn_slots);
            crate::log!(
                "debug_draw [{}]   u {:?}: {:?}",
                backend,
                input.id,
                &draw_call.dyn_uniforms[input.offset..end]
            );
        }

        for inst_idx in 0..instances {
            let base = inst_idx * instance_slots;
            if base + instance_slots > instance_data.len() {
                break;
            }
            let mut parts = Vec::new();
            for input in &draw_shader.mapping.instances.inputs {
                let start = base + input.offset;
                let end = start + input.slots;
                if end > instance_data.len() {
                    continue;
                }
                let vals = &instance_data[start..end];
                if input.slots == 1 {
                    parts.push(format!("{:?}={}", input.id, vals[0]));
                } else {
                    parts.push(format!("{:?}={:?}", input.id, vals));
                }
            }
            crate::log!(
                "debug_draw [{}]   i[{}] {}",
                backend,
                inst_idx,
                parts.join(" ")
            );
        }
    }

    pub fn from_shader_output(
        source: ScriptObjectRef,
        code: CxDrawShaderCode,
        heap: &ScriptHeap,
        output: &ShaderOutput,
        geometry_id: Option<GeometryId>,
    ) -> CxDrawShaderMapping {
        let debug_draw = heap
            .value(source.as_object(), id!(debug_draw).into(), NoTrap)
            .as_bool()
            == Some(true);
        let debug_layout = heap
            .value(source.as_object(), id!(debug_layout).into(), NoTrap)
            .as_bool()
            == Some(true);
        let debug_code = heap
            .value(source.as_object(), id!(debug_code).into(), NoTrap)
            .as_bool()
            == Some(true);
        let async_compile = heap
            .value(source.as_object(), id!(async_compile).into(), NoTrap)
            .as_bool()
            == Some(true);
        // Color-attachment format: `color_format: @Rf32` selects the float
        // render-target pipeline; anything else keeps the BGRA8 default.
        let color_format = match heap
            .value(source.as_object(), id!(color_format).into(), NoTrap)
            .as_id()
        {
            Some(id) if id == id!(Rf32) => DrawShaderColorFormat::Rf32,
            Some(id) if id == id!(Rgba16F) => DrawShaderColorFormat::Rgba16F,
            Some(id) if id == id!(Rgba32F) => DrawShaderColorFormat::Rgba32F,
            Some(id) if id == id!(Bgra8NoBlend) => DrawShaderColorFormat::Bgra8NoBlend,
            _ => DrawShaderColorFormat::Bgra8Unorm,
        };
        // Use attribute packing for instances (they're vertex attributes)
        // instances contains ALL instance fields (dyn first, then rust)
        let mut instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        // dyn_instances tracks just the dynamic portion for offset calculations
        let mut dyn_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        // Use platform-specific packing for uniforms
        let mut dyn_uniforms = DrawShaderInputs::new(uniform_packing());
        // Geometries for vertex buffer fields
        let mut geometries = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut textures = Vec::new();
        let mut uniform_buffers = Vec::new();
        let mut texture_sampler_indices = Vec::new();

        let mut rect_pos = None;
        let mut rect_size = None;
        let mut draw_clip = None;

        // Memory layout: DynInstance fields first, then RustInstance fields
        // This matches metal_create_instance_struct

        // 1. Process DynInstance fields first (added to both instances and dyn_instances)
        for io in &output.io {
            if let ShaderIoKind::DynInstance = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                let attr_format = Self::attr_format_from_pod_type(&pod_ty.ty);
                instances.push(io.name, slots, attr_format);
                dyn_instances.push(io.name, slots, attr_format);
            }
        }

        // 2. Process RustInstance fields after (already in correct order from pre_collect_rust_instance_io)
        for io in output
            .io
            .iter()
            .filter(|io| matches!(io.kind, ShaderIoKind::RustInstance))
        {
            let pod_ty = heap.pod_type_ref(io.ty);
            let slots = pod_ty.ty.slots();
            let attr_format = Self::attr_format_from_pod_type(&pod_ty.ty);

            // Track special field offsets
            if io.name == live_id!(rect_pos) {
                rect_pos = Some(instances.total_slots);
            }
            if io.name == live_id!(rect_size) {
                rect_size = Some(instances.total_slots);
            }
            if io.name == live_id!(draw_clip) {
                draw_clip = Some(instances.total_slots);
            }

            instances.push(io.name, slots, attr_format);
        }

        // Process Uniform fields
        for io in &output.io {
            if let ShaderIoKind::Uniform = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                dyn_uniforms.push(io.name, slots, DrawShaderAttrFormat::Float);
            }
        }

        // Process VertexBuffer (geometry) fields
        for io in &output.io {
            if let ShaderIoKind::VertexBuffer = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                let slots = pod_ty.ty.slots();
                let attr_format = Self::attr_format_from_pod_type(&pod_ty.ty);
                geometries.push(io.name, slots, attr_format);
            }
        }

        // Process texture fields.
        for io in &output.io {
            match &io.kind {
                ShaderIoKind::Texture(tex_type) => {
                    textures.push(DrawShaderTextureInput {
                        id: io.name,
                        tex_type: *tex_type,
                    });
                    let texture_name = format!("tex_{}", io.name);
                    let sampler_idx = output
                        .texture_sampler_bindings
                        .iter()
                        .find(|(bound_texture, _)| bound_texture == &texture_name)
                        .map(|(_, idx)| *idx)
                        .unwrap_or(0);
                    texture_sampler_indices.push(sampler_idx);
                }
                _ => (),
            }
        }

        for io in &output.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                let pod_ty = heap.pod_type_ref(io.ty);
                if matches!(
                    pod_ty.name,
                    Some(id!(DrawPassUniforms))
                        | Some(id!(DrawListUniforms))
                        | Some(id!(DrawCallUniforms))
                ) {
                    continue;
                }
                let io_name = output.backend.map_io_name(io.name);
                uniform_buffers.push(DrawShaderUniformBufferInput {
                    id: io.name,
                    block_name: format!("{}_Uniforms", io_name),
                    ty: io.ty,
                    size: pod_ty.ty.size_of(),
                    align: pod_ty.ty.align_of(),
                    buffer_index: io
                        .buffer_index
                        .expect("UniformBuffer must have buffer_index assigned"),
                });
            }
        }

        if uniform_buffers.len() > DRAW_CALL_UNIFORM_BUFFER_SLOTS {
            panic!(
                "shader {:?} declares {} custom uniform buffers but only {} draw-call slots are available",
                source.as_object(),
                uniform_buffers.len(),
                DRAW_CALL_UNIFORM_BUFFER_SLOTS
            );
        }

        instances.finalize();
        dyn_instances.finalize();
        dyn_uniforms.finalize();
        geometries.finalize();

        // Get uniform buffer bindings from the shader output
        // (must call assign_uniform_buffer_indices before from_shader_output)
        let uniform_buffer_bindings = output.get_uniform_buffer_bindings(heap);

        // Build scope uniforms layout using DrawShaderInputs (4-byte slot alignment)
        let mut scope_uniforms = DrawShaderInputs::new(uniform_packing());
        let mut scope_uniform_sources = Vec::new();

        // Process scope uniforms in order - same order as they appear in the io list
        let mut table_consts: Vec<DrawShaderTableConst> = Vec::new();
        for io in &output.io {
            if let ShaderIoKind::ScopeUniform = io.kind {
                // Find the corresponding ScopeUniformSource
                if let Some(source) = output
                    .scope_uniforms
                    .iter()
                    .find(|su| su.shader_name == io.name)
                {
                    let pod_ty = heap.pod_type_ref(source.ty);
                    let slots = pod_ty.ty.slots();
                    let input = scope_uniforms.inputs.len();
                    scope_uniforms.push(io.name, slots, DrawShaderAttrFormat::Float);
                    match source.table_const {
                        Some(ci) => {
                            let tc = &output.table_consts[ci];
                            let hint = crate::makepad_script::docs::parse_doc_hint(&tc.doc);
                            scope_uniform_sources.push(ScopeUniformSlot::Const(table_consts.len()));
                            table_consts.push(DrawShaderTableConst {
                                shader_name: tc.shader_name,
                                doc: tc.doc.clone(),
                                name: hint.name,
                                min: hint.min,
                                max: hint.max,
                                step: hint.step,
                                initial: tc.value as f32,
                                value: tc.value as f32,
                                input,
                                ip: tc.ip,
                            });
                        }
                        None => scope_uniform_sources
                            .push(ScopeUniformSlot::Scope(source.source_obj, source.key)),
                    }
                }
            }
        }
        scope_uniforms.finalize();

        // Allocate the buffer for scope uniforms (as f32 slots)
        let scope_uniforms_buf = vec![0.0f32; scope_uniforms.total_slots];

        if debug_layout {
            crate::log!(
                "debug_layout shader {:?}: flags draw={} layout={} code={} uniform_packing={:?}",
                source.as_object(),
                debug_draw,
                debug_layout,
                debug_code,
                dyn_uniforms.packing_method
            );

            for io in output
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::DynInstance))
            {
                let pod_ty = heap.pod_type_ref(io.ty);
                if let Some(input) = dyn_instances
                    .inputs
                    .iter()
                    .find(|input| input.id == io.name)
                {
                    crate::log!(
                        "debug_layout shader {:?}: dyn_instance {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        pod_ty.ty,
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in output
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::RustInstance))
            {
                let pod_ty = heap.pod_type_ref(io.ty);
                if let Some(input) = instances.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: rust_instance {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        pod_ty.ty,
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in output
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::Uniform))
            {
                let pod_ty = heap.pod_type_ref(io.ty);
                if let Some(input) = dyn_uniforms.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: dyn_uniform {:?} ty={:?} size={} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        pod_ty.ty,
                        pod_ty.ty.size_of(),
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for io in output
                .io
                .iter()
                .filter(|io| matches!(io.kind, ShaderIoKind::VertexBuffer))
            {
                let pod_ty = heap.pod_type_ref(io.ty);
                if let Some(input) = geometries.inputs.iter().find(|input| input.id == io.name) {
                    crate::log!(
                        "debug_layout shader {:?}: vertex_buffer {:?} ty={:?} slots={} offset={} attr={:?}",
                        source.as_object(),
                        io.name,
                        pod_ty.ty,
                        input.slots,
                        input.offset,
                        input.attr_format
                    );
                }
            }

            for input in &scope_uniforms.inputs {
                crate::log!(
                    "debug_layout shader {:?}: scope_uniform {:?} slots={} offset={} attr={:?}",
                    source.as_object(),
                    input.id,
                    input.slots,
                    input.offset,
                    input.attr_format
                );
            }

            for (type_name, idx) in &uniform_buffer_bindings.bindings {
                crate::log!(
                    "debug_layout shader {:?}: uniform_buffer {} -> b{}",
                    source.as_object(),
                    type_name,
                    idx
                );
            }

            crate::log!(
                "debug_layout shader {:?}: totals dyn_instances={} instances={} dyn_uniforms={} geometries={} scope_uniforms={} textures={}",
                source.as_object(),
                dyn_instances.total_slots,
                instances.total_slots,
                dyn_uniforms.total_slots,
                geometries.total_slots,
                scope_uniforms.total_slots,
                textures.len()
            );
        }

        // Check if shader uses draw_pass->time (requires repaint every frame)
        let uses_time = match &code {
            CxDrawShaderCode::Combined { code } => code.contains("draw_pass->time"),
            CxDrawShaderCode::Separate { vertex, fragment } => {
                vertex.contains("draw_pass->time") || fragment.contains("draw_pass->time")
            }
        };

        CxDrawShaderMapping {
            source,
            code,
            flags: DrawShaderFlags {
                debug_draw,
                debug_layout,
                debug_code,
                async_compile,
                ..DrawShaderFlags::default()
            },
            instances,
            dyn_instances,
            dyn_uniforms,
            geometries,
            textures,
            uniform_buffers,
            samplers: output.samplers.clone(),
            texture_sampler_indices,
            uses_time,
            rect_pos,
            rect_size,
            draw_clip,
            uniform_buffer_bindings,
            scope_uniforms,
            scope_uniform_sources,
            scope_uniforms_buf,
            table_consts,
            scope_uniforms_gen: 0,
            geometry_id,
            varying_total_slots: 0,
            color_format,
        }
    }

    /// Write one table constant's live value into its scope-uniform slot
    /// and bump the generation so GPU-side copies refresh. Returns false
    /// for an index the shader does not have.
    pub fn patch_table_const(&mut self, index: usize, value: f32) -> bool {
        let Some(tc) = self.table_consts.get_mut(index) else {
            return false;
        };
        tc.value = value;
        let input = &self.scope_uniforms.inputs[tc.input];
        if let Some(slot) = self.scope_uniforms_buf.get_mut(input.offset) {
            *slot = value;
        }
        self.scope_uniforms_gen = self.scope_uniforms_gen.wrapping_add(1);
        true
    }

    /// Fill the scope uniform buffer from script values.
    ///
    /// This reads values from the script heap using the source_obj and key for each entry,
    /// converts them to f32 slots, and writes to the buffer.
    pub fn fill_scope_uniforms_buffer(
        &mut self,
        heap: &ScriptHeap,
        trap: &crate::makepad_script::trap::ScriptTrap,
    ) {
        for (i, input) in self.scope_uniforms.inputs.iter().enumerate() {
            if i >= self.scope_uniform_sources.len() {
                break;
            }
            let (source_obj, key) = match self.scope_uniform_sources[i] {
                ScopeUniformSlot::Scope(obj, key) => (obj, key),
                ScopeUniformSlot::Const(ci) => {
                    // A table constant: its live value, never the heap.
                    if let Some(slot) = self.scope_uniforms_buf.get_mut(input.offset) {
                        *slot = self.table_consts[ci].value;
                    }
                    continue;
                }
            };

            // Read the value from the heap
            let value = heap.scope_value(source_obj, key, *trap);

            // Write value to buffer at the input's offset
            DrawVars::write_value_to_f32_slots(
                heap,
                value,
                &mut self.scope_uniforms_buf,
                input.offset,
                input.slots,
                DrawShaderAttrFormat::Float,
            );
        }
    }

    /*
    pub fn from_draw_shader_def(draw_shader_def: &DrawShaderDef, const_table: DrawShaderConstTable, uniform_packing: DrawShaderInputPacking) -> CxDrawShaderMapping { //}, options: ShaderCompileOptions, metal_uniform_packing:bool) -> Self {

        let mut geometries = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut var_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut live_instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut draw_call_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut live_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut draw_list_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut draw_call_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut pass_uniforms = DrawShaderInputs::new(uniform_packing);
        let mut textures = Vec::new();
        let mut instance_enums = Vec::new();
        let mut rect_pos = None;
        let mut rect_size = None;
        let mut draw_clip = None;
        for field in &draw_shader_def.fields {
            let ty = field.ty_expr.ty.borrow().as_ref().unwrap().clone();
            match &field.kind {
                DrawShaderFieldKind::Geometry {..} => {
                    geometries.push(field.ident.0, ty, None);
                }
                DrawShaderFieldKind::Instance {var_def_ptr, live_field_kind, ..} => {
                    if field.ident.0 == live_id!(rect_pos) {
                        rect_pos = Some(instances.total_slots);
                    }
                    if field.ident.0 == live_id!(rect_size) {
                        rect_size = Some(instances.total_slots);
                    }
                    if field.ident.0 == live_id!(draw_clip) {
                        draw_clip = Some(instances.total_slots);
                    }
                    if var_def_ptr.is_some() {
                        var_instances.push(field.ident.0, ty.clone(), None,);
                    }
                    if let ShaderTy::Enum{..} = ty{
                        instance_enums.push(instances.total_slots);
                    }
                    instances.push(field.ident.0, ty, None);
                    if let LiveFieldKind::Live = live_field_kind {
                        live_instances.inputs.push(instances.inputs.last().unwrap().clone());
                    }
                }
                DrawShaderFieldKind::Uniform {block_ident, ..} => {
                    match block_ident.0 {
                        live_id!(draw_call) => {
                            draw_call_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(draw_list) => {
                            draw_list_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(pass) => {
                            pass_uniforms.push(field.ident.0, ty, None);
                        }
                        live_id!(user) => {
                            draw_call_uniforms.push(field.ident.0, ty, None);
                        }
                        _ => ()
                    }
                }
                DrawShaderFieldKind::Texture {..} => {
                    textures.push(DrawShaderTextureInput {
                        ty:ty,
                        id: field.ident.0,
                    });
                }
                _ => ()
            }
        }

        geometries.finalize();
        instances.finalize();
        var_instances.finalize();
        draw_call_uniforms.finalize();
        live_uniforms.finalize();
        draw_list_uniforms.finalize();
        draw_call_uniforms.finalize();
        pass_uniforms.finalize();

        // fill up the default values for the user uniforms


        // ok now the live uniforms
        for (value_node_ptr, ty) in draw_shader_def.all_live_refs.borrow().iter() {
            live_uniforms.push(LiveId(0), ty.clone(), Some(value_node_ptr.0));
        }

        CxDrawShaderMapping {
            const_table,
            uses_time: draw_shader_def.uses_time.get(),
            flags: draw_shader_def.flags,
            geometries,
            instances,
            live_uniforms_buf: {let mut r = Vec::new(); r.resize(live_uniforms.total_slots, 0.0); r},
            var_instances,
            live_instances,
            draw_call_uniforms,
            live_uniforms,
            draw_list_uniforms,
            draw_call_uniforms,
            pass_uniforms,
            instance_enums,
            textures,
            rect_pos,
            rect_size,
            draw_clip,
        }
    }*/
    /*
    pub fn update_live_and_user_uniforms(&mut self, cx: &mut Cx, apply: &Apply) {
        // and write em into the live_uniforms buffer
        let live_registry = cx.live_registry.clone();
        let live_registry = live_registry.borrow();

        for input in &self.live_uniforms.inputs {
            let (nodes,index) = live_registry.ptr_to_nodes_index(input.live_ptr.unwrap());
            DrawVars::apply_slots(
                cx,
                input.slots,
                &mut self.live_uniforms_buf,
                input.offset,
                apply,
                index,
                nodes
            );
        }
    }*/
}

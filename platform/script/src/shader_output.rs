use crate::heap::*;
use crate::pod::*;
use crate::shader_backend::*;
use crate::value::*;
use crate::vm::*;
use makepad_live_id::*;
use std::collections::BTreeSet;
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerFilter {
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerAddress {
    Repeat,
    ClampToEdge,
    ClampToZero,
    MirroredRepeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerCoord {
    Normalized,
    Pixel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShaderSampler {
    pub filter: SamplerFilter,
    pub address: SamplerAddress,
    pub coord: SamplerCoord,
}

impl Default for ShaderSampler {
    fn default() -> Self {
        Self {
            filter: SamplerFilter::Linear,
            address: SamplerAddress::ClampToEdge,
            coord: SamplerCoord::Normalized,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct ShaderSamplerOptions {}

#[derive(Debug, Default, Clone)]
pub struct ShaderStorageFlags(u32);
impl ShaderStorageFlags {
    pub fn set_read(&mut self) {
        self.0 |= 1
    }
    pub fn set_write(&mut self) {
        self.0 |= 1
    }
    pub fn is_read(&self) -> bool {
        self.0 & 1 != 0
    }
    pub fn is_write(&self) -> bool {
        self.0 & 2 != 0
    }
    pub fn is_readwrite(&self) -> bool {
        self.0 & 3 == 3
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureType {
    Texture1d,
    Texture1dArray,
    Texture2d,
    Texture2dArray,
    Texture3d,
    Texture3dArray,
    TextureCube,
    TextureCubeArray,
    TextureDepth,
    TextureDepthArray,
    TextureVideo,
}

#[derive(Debug, Clone)]
pub enum ShaderIoKind {
    StorageBuffer(ShaderStorageFlags),
    UniformBuffer,
    Sampler(ShaderSamplerOptions),
    Texture(TextureType),
    Varying,
    VertexBuffer,
    VertexPosition,
    FragmentOutput(u8),
    RustInstance,
    Uniform,
    DynInstance,
    ScopeUniform,
}

/// Tracks the source of a scope uniform for buffer refresh after compilation.
/// Each scope uniform comes from either:
/// - A direct scope value: `test_p1` -> source_obj is the scope, key is `test_p1`
/// - An object property: `test_obj.p2` -> source_obj is `test_obj`, key is `p2`
#[derive(Debug, Clone)]
pub struct ScopeUniformSource {
    /// The source object to read the value from
    pub source_obj: ScriptObject,
    /// The key to read from the source object  
    pub key: LiveId,
    /// The name used in the shader (may be prefixed for collision avoidance)
    pub shader_name: LiveId,
    /// The pod type of this uniform
    pub ty: ScriptPodType,
}

/// Tracks a uniform buffer defined in the script scope (e.g., `let buf = shader.uniform_buffer(...)`)
/// This is distinct from uniform buffers on io_self - these come from the surrounding scope.
#[derive(Debug, Clone)]
pub struct ScopeUniformBufferSource {
    /// The uniform buffer ScriptObject itself (needed for runtime buffer binding)
    pub obj: ScriptObject,
    /// The pod type of the uniform buffer's prototype
    pub pod_ty: ScriptPodType,
    /// The name used in the shader code
    pub shader_name: LiveId,
}

/// Tracks a texture defined in the script scope (e.g., `let tex = shader.texture_2d(float)`)
/// This is distinct from textures on io_self - these come from the surrounding scope.
#[derive(Debug, Clone)]
pub struct ScopeTextureSource {
    /// The texture ScriptObject itself (needed for runtime texture binding)
    pub obj: ScriptObject,
    /// The texture type (2d, cube, etc.)
    pub tex_type: TextureType,
    /// The name used in the shader code
    pub shader_name: LiveId,
}

#[allow(unused)]
#[derive(Debug)]
pub struct ShaderIo {
    pub kind: ShaderIoKind,
    pub name: LiveId,
    pub ty: ScriptPodType,
    /// Buffer index assigned during Metal/backend code generation (for uniform buffers, etc.)
    pub buffer_index: Option<usize>,
}

impl ShaderIo {
    pub fn kind(&self) -> &ShaderIoKind {
        &self.kind
    }

    pub fn name(&self) -> LiveId {
        self.name
    }

    pub fn ty(&self) -> ScriptPodType {
        self.ty
    }

    pub fn buffer_index(&self) -> Option<usize> {
        self.buffer_index
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum ShaderMode {
    Vertex,
    #[default]
    Fragment,
    Compute,
}

#[derive(Default, Debug)]
pub struct ShaderOutput {
    pub mode: ShaderMode,
    pub backend: ShaderBackend,
    pub io: Vec<ShaderIo>,
    pub recur_block: Vec<ScriptObject>,
    pub structs: BTreeSet<ScriptPodType>,
    pub functions: Vec<ShaderFn>,
    pub samplers: Vec<ShaderSampler>,
    pub scope_uniforms: Vec<ScopeUniformSource>,
    pub scope_uniform_buffers: Vec<ScopeUniformBufferSource>,
    pub scope_textures: Vec<ScopeTextureSource>,
    /// Per-texture sampler bindings inferred during shader lowering.
    /// Entries are `(texture_expr, sampler_index)`.
    pub texture_sampler_bindings: Vec<(String, usize)>,
    /// HLSL helper: needs _mpTexSize helper function for texture.size()
    pub hlsl_needs_tex_size: bool,
    /// Set to true if any errors occurred during shader compilation
    pub has_errors: bool,
    /// True if this shader uses screen-space derivatives (dFdx/dFdy).
    pub uses_derivatives: bool,
    /// Monotonic temporary id source for Rust backend expression hoisting.
    pub rust_tmp_counter: usize,
}

/// Mapping of uniform buffer type names to their assigned buffer indices
#[derive(Default, Debug, Clone)]
pub struct UniformBufferBindings {
    /// Maps Pod type name (e.g. DrawCallUniforms) to buffer index
    pub bindings: Vec<(LiveId, usize)>,
    /// Buffer index for the IoScopeUniform struct (if any scope uniforms exist)
    pub scope_uniform_buffer_index: Option<usize>,
}

impl UniformBufferBindings {
    /// Look up buffer index by Pod type name
    pub fn get_by_type_name(&self, type_name: LiveId) -> Option<usize> {
        self.bindings
            .iter()
            .find(|(name, _)| *name == type_name)
            .map(|(_, idx)| *idx)
    }
}

#[derive(Debug)]
pub struct ShaderFn {
    pub call_sig: String,
    pub overload: usize,
    pub name: LiveId,
    pub args: Vec<ScriptPodType>,
    pub fnobj: ScriptObject,
    pub out: String,
    pub ret: ScriptPodType,
}

impl ShaderOutput {
    pub fn next_rust_tmp_id(&mut self) -> usize {
        let id = self.rust_tmp_counter;
        self.rust_tmp_counter = self.rust_tmp_counter.wrapping_add(1);
        id
    }

    /// Pre-collect ALL Rust instance fields in the correct order for struct layout.
    /// Uses recursion to process from deepest prototype to io_self, collecting all rust type properties.
    /// Dyn instance fields are NOT pre-collected - they are added during compilation
    /// as encountered, and their order doesn't matter.
    ///
    /// IoInstance struct layout: Dyn fields first (any order), Rust fields last (must match Repr(C))
    /// RustInstance fields are pushed in the correct order: deref parent fields first, then child fields.
    pub fn pre_collect_rust_instance_io(&mut self, vm: &mut ScriptVm, io_self: ScriptObject) {
        self.pre_collect_rust_instance_io_recursive(vm, io_self);
    }

    fn pre_collect_rust_instance_io_recursive(&mut self, vm: &mut ScriptVm, obj: ScriptObject) {
        // First, recurse to prototype (to process deepest ancestor first)
        // This ensures parent's RustInstance fields come before child's fields
        if let Some(proto_obj) = vm.bx.heap.proto(obj).as_object() {
            self.pre_collect_rust_instance_io_recursive(vm, proto_obj);
        }

        // Then process this object's type properties
        let obj_data = vm.bx.heap.object_data(obj);
        let ty_index = obj_data.tag.as_type_index();

        if let Some(ty_index) = ty_index {
            // Collect the ordered props - iter_rust_instance_ordered returns all instance fields
            // (parent fields + this type's fields after deref) because rust_instance_start is now
            // correctly left at 0 (config fields before deref are skipped during script_proto_props)
            let type_check = vm.bx.heap.type_check(ty_index);
            // Collect into a Vec first to avoid borrow issues with heap mutation below
            let ordered_props: Vec<_> = type_check.props.iter_rust_instance_ordered().collect();

            for (field_id, type_id) in ordered_props {
                // Get the pod type from the type_id
                if let Some(pod_ty) = vm
                    .bx
                    .heap
                    .type_id_to_pod_type(type_id, &vm.bx.code.builtins.pod)
                {
                    // Skip if already added (handles duplicate prototypes in chain)
                    if !self.io.iter().any(|io| io.name == field_id) {
                        vm.bx.heap.pod_type_name_if_not_set(pod_ty, field_id);
                        self.io.push(ShaderIo {
                            kind: ShaderIoKind::RustInstance,
                            name: field_id,
                            ty: pod_ty,
                            buffer_index: None,
                        });
                    }
                }
            }
        }
    }

    /// Pre-collect ALL shader IO (uniforms, textures, fragment outputs) in definition order.
    /// Walks from deepest prototype to io_self, collecting all shader IO properties in a single pass.
    /// This ensures IO appears in definition order, not access order during compilation.
    ///
    /// Note: RustInstance and DynInstance are handled separately - RustInstance via
    /// pre_collect_rust_instance_io (from Rust type properties), DynInstance during compilation.
    pub fn pre_collect_shader_io(&mut self, vm: &mut ScriptVm, io_self: ScriptObject) {
        // Use recursion to process from deepest prototype first (no temporary Vec needed)
        self.pre_collect_shader_io_recursive(vm, io_self);

        // Set pod type names for uniforms (requires mutable heap, done after iteration)
        for io in &self.io {
            if matches!(io.kind, ShaderIoKind::Uniform) {
                vm.bx.heap.pod_type_name_if_not_set(io.ty, io.name);
            }
        }
    }

    fn pre_collect_shader_io_recursive(&mut self, vm: &ScriptVm, obj: ScriptObject) {
        use crate::mod_shader::*;

        // First recurse to prototype (to process deepest first)
        if let Some(proto_obj) = vm.bx.heap.proto(obj).as_object() {
            self.pre_collect_shader_io_recursive(vm, proto_obj);
        }

        // Then process this object's map entries in insertion order
        let obj_data = vm.bx.heap.object_data(obj);
        obj_data.map_iter_ordered(|key, value| {
            if let Some(value_obj) = value.as_object() {
                if let Some(io_type) = vm.bx.heap.as_shader_io(value_obj) {
                    if let Some(field_id) = key.as_id() {
                        // Skip if already exists (derived class overrides)
                        if self.io.iter().any(|io| io.name == field_id) {
                            return;
                        }
                        
                        // Get the pod type from the prototype
                        let proto_value = vm.bx.heap.proto(value_obj);
                        let pod_ty = Self::get_pod_type_from_value(vm, proto_value);
                        
                        // Handle different shader IO types
                        match io_type {
                            // Uniforms
                            SHADER_IO_DYN_UNIFORM => {
                                if let Some(pod_ty) = pod_ty {
                                    self.io.push(ShaderIo {
                                        kind: ShaderIoKind::Uniform,
                                        name: field_id,
                                        ty: pod_ty,
                                        buffer_index: None,
                                    });
                                }
                            }
                            
                            // Fragment outputs
                            io_type if io_type.0 >= SHADER_IO_FRAGMENT_OUTPUT_0.0 
                                    && io_type.0 <= SHADER_IO_FRAGMENT_OUTPUT_MAX.0 => {
                                let index = (io_type.0 - SHADER_IO_FRAGMENT_OUTPUT_0.0) as u8;
                                // Check by index, not name
                                let already_exists = self.io.iter().any(|io| {
                                    matches!(io.kind, ShaderIoKind::FragmentOutput(idx) if idx == index)
                                });
                                if !already_exists {
                                    if let Some(pod_ty) = pod_ty {
                                        self.io.push(ShaderIo {
                                            kind: ShaderIoKind::FragmentOutput(index),
                                            name: field_id,
                                            ty: pod_ty,
                                            buffer_index: None,
                                        });
                                    }
                                }
                            }
                            
                            // Textures
                            SHADER_IO_TEXTURE_1D => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture1d), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_1D_ARRAY => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture1dArray), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_2D => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture2d), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_2D_ARRAY => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture2dArray), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_3D => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture3d), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_3D_ARRAY => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::Texture3dArray), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_CUBE => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureCube), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_CUBE_ARRAY => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureCubeArray), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_DEPTH => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureDepth), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_DEPTH_ARRAY => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureDepthArray), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),
                            SHADER_IO_TEXTURE_VIDEO => self.io.push(ShaderIo { kind: ShaderIoKind::Texture(TextureType::TextureVideo), name: field_id, ty: ScriptPodType::VOID, buffer_index: None }),

                            // Other IO types are handled during compilation or elsewhere
                            _ => {}
                        }
                    }
                }
            }
        });
    }

    pub(crate) fn get_pod_type_from_value(
        vm: &ScriptVm,
        value: ScriptValue,
    ) -> Option<ScriptPodType> {
        // Check if it's a primitive type (f32, f64, bool, etc.)
        if let Some(pod_ty) = vm.bx.code.builtins.pod.value_to_exact_type(value) {
            return Some(pod_ty);
        }
        // Check if it's a color - colors map to vec4f
        if value.is_color() {
            return Some(vm.bx.code.builtins.pod.pod_vec4f);
        }
        // Check if it's a pod type object
        if let Some(pod_ty) = vm.bx.heap.pod_type(value) {
            return Some(pod_ty);
        }
        // Check if it's a pod instance
        if let Some(pod) = value.as_pod() {
            let pod = &vm.bx.heap.pods[pod];
            return Some(pod.ty);
        }
        // Check if it's a pod type reference
        if let Some(pod_ty) = value.as_pod_type() {
            return Some(pod_ty);
        }
        None
    }

    pub fn create_struct_defs(&mut self, vm: &ScriptVm, out: &mut String) {
        let mut plain_structs = self.structs.clone();
        let mut packed_structs = BTreeSet::new();

        for io in &self.io {
            let ty = io.ty;
            if !matches!(vm.bx.heap.pod_type_ref(ty).ty, ScriptPodTy::Struct { .. }) {
                continue;
            }

            if matches!(self.backend, ShaderBackend::Metal)
                && matches!(
                    io.kind,
                    ShaderIoKind::UniformBuffer
                        | ShaderIoKind::VertexBuffer
                        | ShaderIoKind::RustInstance
                        | ShaderIoKind::DynInstance
                )
            {
                packed_structs.insert(ty);
            } else {
                plain_structs.insert(ty);
            }
        }

        for ty in &packed_structs {
            plain_structs.remove(ty);
        }

        self.structs.extend(plain_structs.iter().copied());
        self.structs.extend(packed_structs.iter().copied());

        if matches!(self.backend, ShaderBackend::Metal) {
            self.backend
                .pod_struct_defs_mixed(&vm.bx.heap, &plain_structs, &packed_structs, out);
        } else {
            self.backend.pod_struct_defs(&vm.bx.heap, &self.structs, out);
        }
    }

    pub fn create_functions(&self, out: &mut String) {
        for fns in &self.functions {
            writeln!(out, "{}{{", fns.call_sig).ok();
            if matches!(self.backend, ShaderBackend::Hlsl) {
                if fns.call_sig.contains("inout IoV _iov") {
                    // DXC requires explicit definite assignment for inout params.
                    writeln!(out, "    _iov = _iov;").ok();
                }
                if fns.call_sig.contains("inout IoF _iof") {
                    // DXC requires explicit definite assignment for inout params.
                    writeln!(out, "    _iof = _iof;").ok();
                }
            }
            writeln!(out, "{}", fns.out).ok();
            writeln!(out, "}}\n").ok();
        }
    }

    /// Find the vertex buffer object from io_self by looking for SHADER_IO_VERTEX_BUFFER type
    pub fn find_vertex_buffer_object(
        &self,
        vm: &ScriptVm,
        io_self: ScriptObject,
    ) -> Option<ScriptObject> {
        use crate::mod_shader::SHADER_IO_VERTEX_BUFFER;

        // Walk the prototype chain looking for vertex buffer properties
        let mut current = Some(io_self);
        while let Some(obj) = current {
            let obj_data = vm.bx.heap.object_data(obj);

            // Check map properties
            if let Some(ret) = obj_data.map_iter_ret(|_key, value| {
                if let Some(value_obj) = value.as_object() {
                    if let Some(io_type) = vm.bx.heap.as_shader_io(value_obj) {
                        if io_type == SHADER_IO_VERTEX_BUFFER {
                            return Some(value_obj);
                        }
                    }
                }
                None
            }) {
                return Some(ret);
            }

            // Move to next prototype
            current = vm.bx.heap.proto(obj).as_object();
        }
        None
    }

    /// Assign buffer indices to uniform buffers starting from `start_index`.
    /// Returns the UniformBufferBindings and the next available buffer index.
    /// Also sets the buffer_index field on each ShaderIo.
    pub fn assign_uniform_buffer_indices(
        &mut self,
        heap: &ScriptHeap,
        start_index: usize,
    ) -> (UniformBufferBindings, usize) {
        let mut bindings = UniformBufferBindings::default();
        let mut buf_idx = start_index;

        for io in &mut self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                // Get the Pod type name for this uniform buffer
                let pod_type = heap.pod_type_ref(io.ty);
                if let Some(type_name) = pod_type.name {
                    bindings.bindings.push((type_name, buf_idx));
                }
                io.buffer_index = Some(buf_idx);
                buf_idx += 1;
            }
        }

        // Assign scope uniform buffer index if we have any scope uniforms
        let has_scope_uniforms = self
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        if has_scope_uniforms {
            bindings.scope_uniform_buffer_index = Some(buf_idx);
            buf_idx += 1;
        }

        (bindings, buf_idx)
    }

    /// Get the UniformBufferBindings from the current IO state.
    /// This should be called after `assign_uniform_buffer_indices` has been called.
    pub fn get_uniform_buffer_bindings(&self, heap: &ScriptHeap) -> UniformBufferBindings {
        let mut bindings = UniformBufferBindings::default();

        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                if let Some(buf_idx) = io.buffer_index {
                    let pod_type = heap.pod_type_ref(io.ty);
                    if let Some(type_name) = pod_type.name {
                        bindings.bindings.push((type_name, buf_idx));
                    }
                }
            }
        }

        // Compute scope uniform buffer index (one past the max uniform buffer index, or start at 3)
        let has_scope_uniforms = self
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        if has_scope_uniforms {
            let max_idx = self
                .io
                .iter()
                .filter_map(|io| io.buffer_index)
                .max()
                .map(|m| m + 1)
                .unwrap_or(3);
            bindings.scope_uniform_buffer_index = Some(max_idx);
        }

        bindings
    }

    /// Get or create a sampler with the given properties, returns the sampler index
    pub fn get_or_create_sampler(&mut self, sampler: ShaderSampler) -> usize {
        // Check if we already have this sampler
        if let Some(idx) = self.samplers.iter().position(|s| *s == sampler) {
            return idx;
        }
        // Create new sampler
        let idx = self.samplers.len();
        self.samplers.push(sampler);
        idx
    }

    /// Record sampler usage for a texture expression.
    /// If the same texture is sampled multiple times, the first sampler wins.
    pub fn bind_texture_sampler(&mut self, texture_expr: &str, sampler_idx: usize) {
        if self
            .texture_sampler_bindings
            .iter()
            .any(|(expr, _)| expr == texture_expr)
        {
            return;
        }
        self.texture_sampler_bindings
            .push((texture_expr.to_string(), sampler_idx));
    }
}

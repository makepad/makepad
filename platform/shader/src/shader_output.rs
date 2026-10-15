use makepad_relax_api::relax_api;
use crate::shader_ids::*;
use crate::shader_types::ShaderTypes;

use crate::shader_schema::*;
use crate::shader_backend::*;

use makepad_live_id::*;

#[relax_api(plain)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerFilter {
    Nearest,
    Linear,
}

#[relax_api(plain)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerAddress {
    Repeat,
    ClampToEdge,
    ClampToZero,
    MirroredRepeat,
}

#[relax_api(plain)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplerCoord {
    Normalized,
    Pixel,
}

#[relax_api(plain)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShaderSampler {
    pub filter: SamplerFilter,
    pub address: SamplerAddress,
    pub coord: SamplerCoord,
    pub is_video: bool,
    /// Compare reference <= stored depth, then linearly filter visibility.
    pub compare: bool,
}

impl Default for ShaderSampler {
    fn default() -> Self {
        Self {
            filter: SamplerFilter::Linear,
            address: SamplerAddress::ClampToEdge,
            coord: SamplerCoord::Normalized,
            is_video: false,
            compare: false,
        }
    }
}

impl ShaderSampler {
    pub fn video() -> Self {
        Self {
            is_video: true,
            ..Self::default()
        }
    }
}

#[relax_api(plain)]
#[derive(Default, Debug, Clone)]
pub struct ShaderSamplerOptions {}

#[relax_api(plain)]
#[derive(Debug, Default, Clone)]
pub struct ShaderStorageFlags(u32);
impl ShaderStorageFlags {
    pub fn set_read(&mut self) {
        self.0 |= 1
    }
    pub fn set_write(&mut self) {
        self.0 |= 2
    }
    pub fn is_read(&self) -> bool {
        self.0 & 1 != 0
    }
    pub fn is_write(&self) -> bool {
        self.0 & 2 != 0
    }
}

#[relax_api(plain)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// `repr(C, u8)`: a fixed layout, so JIT code reads io kinds without crossing (relax_api.txt).
#[relax_api(plain)]
#[derive(Clone)]
#[repr(C, u8)]
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
    /// A compute kernel's workgroup-shared array.
    Workgroup,
}

#[relax_api(plain)]
#[derive(Clone, Debug)]
pub struct ShaderIo {
    pub kind: ShaderIoKind,
    pub name: LiveId,
    pub ty: ShaderTypeId,
    /// Buffer index assigned during Metal/backend code generation (for uniform buffers, etc.)
    pub buffer_index: Option<usize>,
    pub instance_lane: Option<usize>,
}

impl ShaderIo {
    pub fn kind(&self) -> &ShaderIoKind {
        &self.kind
    }

    pub fn name(&self) -> LiveId {
        self.name
    }

    pub fn ty(&self) -> ShaderTypeId {
        self.ty
    }

    pub fn buffer_index(&self) -> Option<usize> {
        self.buffer_index
    }
}

#[relax_api(plain)]
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum ShaderMode {
    Vertex,
    #[default]
    Fragment,
    Compute,
}

/// Backend input and reflection, independent of frontend provenance.
#[derive(Default, Debug)]
pub struct ShaderOutput {
    pub backend: ShaderBackend,
    pub use_vulkan: bool,
    pub glsl_webgl2: bool,
    pub io: Vec<ShaderIo>,
    pub samplers: Vec<ShaderSampler>,
    pub pick: bool,
    pub texture_sampler_bindings: Vec<(String, usize)>,
    pub has_errors: bool,
    pub errors: Vec<String>,
    pub uses_derivatives: bool,
    /// The shader reads `draw_pass.time`, so its pass repaints every frame.
    pub uses_time: bool,
    pub last_call_cost: u64,
    pub emitted_bytes: usize,
    pub size_exceeded: bool,
    pub ir: crate::shader_ir::IrModule,
}
/// Mapping of uniform buffer type names to their assigned buffer indices
#[relax_api(plain)]
#[derive(Default, Clone)]
pub struct UniformBufferBindings {
    /// Maps Pod type name (e.g. DrawCallUniforms) to buffer index (boxed: plain data JIT
    /// code reads without crossing; built once per compile)
    pub bindings: Box<[(LiveId, usize)]>,
    /// Buffer index for the IoScopeUniform struct (if any scope uniforms exist)
    pub scope_uniform_buffer_index: Option<usize>,
}

impl UniformBufferBindings {
    pub fn scope_uniform_buffer_index(&self) -> Option<usize> { self.scope_uniform_buffer_index }

    /// Look up buffer index by Pod type name
    pub fn get_by_type_name(&self, type_name: LiveId) -> Option<usize> {
        self.bindings
            .iter()
            .find(|(name, _)| *name == type_name)
            .map(|(_, idx)| *idx)
    }
}

impl ShaderOutput {
    pub fn validate_vertex_fetch_io(
        &mut self,
        types: &impl ShaderTypes,
        kind: &ShaderIoKind,
        name: LiveId,
        ty: ShaderTypeId,
    ) {
        let pod_ty = &types.type_data(ty).ty;
        let message = match kind {
            ShaderIoKind::DynInstance | ShaderIoKind::RustInstance
                if pod_ty.has_compact_format() =>
            {
                Some(format!(
                    "compact vertex format in instance field `{name}` is unsupported; instance buffers are f32-backed"
                ))
            }
            ShaderIoKind::VertexBuffer if !pod_ty.vertex_layout_supported() => {
                Some(format!(
                    "vertex format in `{name}` cannot contain nested structs, arrays, matrices, or non-float vectors"
                ))
            }
            _ => None,
        };
        if let Some(message) = message {
            if !self.errors.iter().any(|error| error == &message) {
                self.push_error(message);
            }
        }
    }

    /// The instance record as the draw list writes it (`DrawShaderInputs`
    /// with attribute packing): dyn instance fields, then Rust instance
    /// fields, 32-bit words back to back, an integer vector on a 4-word
    /// boundary with the words after it padded to the next one. Each entry is
    /// (index into `self.io`, word offset); the second value is the stride in
    /// words. Every emitter that reads instance records from a buffer
    /// addresses them by these offsets.
    pub fn instance_record(&self, types: &impl ShaderTypes) -> (Vec<(usize, usize)>, usize) {
        let mut fields = Vec::new();
        let mut at = 0usize;
        for rust in [false, true] {
            for (index, io) in self.io.iter().enumerate() {
                let wanted = match io.kind {
                    ShaderIoKind::DynInstance => !rust,
                    ShaderIoKind::RustInstance => rust,
                    _ => false,
                };
                if !wanted {
                    continue;
                }
                let ty = &types.type_data(io.ty).ty;
                let slots = ty.slots();
                if let Some(lane) = io.instance_lane {
                    fields.push((index, lane));
                    at = at.max(lane + slots);
                    continue;
                }
                let int_lanes = slots > 1 && Self::is_integer_word(ty);
                if int_lanes {
                    at = at.next_multiple_of(4);
                }
                fields.push((index, at));
                at += slots;
                if int_lanes {
                    at = at.next_multiple_of(4);
                }
            }
        }
        (fields, at)
    }

    /// Integer scalars and vectors: their words are read as integers.
    pub fn is_integer_word(ty: &ShaderTypeKind) -> bool {
        match ty {
            ShaderTypeKind::U32 | ShaderTypeKind::I32 | ShaderTypeKind::Bool | ShaderTypeKind::AtomicU32 | ShaderTypeKind::AtomicI32 => true,
            ShaderTypeKind::Vec(v) => !matches!(
                v,
                ShaderVec::Vec2f | ShaderVec::Vec3f | ShaderVec::Vec4f | ShaderVec::Vec2h | ShaderVec::Vec3h | ShaderVec::Vec4h
            ),
            _ => false,
        }
    }

    /// Find the vertex buffer object from io_self by looking for ShaderStorage::VertexBuffer type

    /// Assign buffer indices to uniform buffers starting from `start_index`.
    /// Returns the UniformBufferBindings and the next available buffer index.
    /// Also sets the buffer_index field on each ShaderIo.
    pub fn assign_uniform_buffer_indices(
        &mut self,
        types: &impl ShaderTypes,
        start_index: usize,
    ) -> (UniformBufferBindings, usize) {
        let mut bindings = UniformBufferBindings::default();
        let mut buf_idx = start_index;

        for io in &mut self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                // Get the Pod type name for this uniform buffer
                let pod_type = types.type_data(io.ty);
                if let Some(type_name) = pod_type.name {
                    bindings.push(type_name, buf_idx);
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
    pub fn get_uniform_buffer_bindings(&self, types: &impl crate::shader_types::ShaderTypes) -> UniformBufferBindings {
        let mut bindings = UniformBufferBindings::default();

        for io in &self.io {
            if let ShaderIoKind::UniformBuffer = io.kind {
                if let Some(buf_idx) = io.buffer_index {
                    let pod_type = types.type_data(io.ty);
                    if let Some(type_name) = pod_type.name {
                        bindings.push(type_name, buf_idx);
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

}


impl ShaderOutput {
    pub fn uses_derivatives(&self) -> bool { self.uses_derivatives }
    pub fn uses_time(&self) -> bool { self.uses_time }
    pub fn backend(&self) -> ShaderBackend { self.backend }
    pub fn set_backend(&mut self, backend: ShaderBackend) { self.backend = backend; }
    pub fn has_errors(&self) -> bool { self.has_errors }
    pub fn pick(&self) -> bool { self.pick }
    pub fn set_pick(&mut self, pick: bool) { self.pick = pick; }
    pub fn io(&self) -> &[ShaderIo] { &self.io }
    pub fn samplers(&self) -> &[ShaderSampler] { &self.samplers }
    pub fn texture_sampler_index(&self, name: &str) -> usize {
        self.texture_sampler_bindings.iter().find(|(bound, _)| bound == name).map(|(_, index)| *index).unwrap_or(0)
    }
    pub fn push_error(&mut self, msg: String) {
        self.has_errors = true;
        self.errors.push(msg);
    }
    pub fn error_report(&self) -> String {
        if self.errors.is_empty() {
            // has_errors was set without a message — still say something.
            "shader compile failed (no diagnostic captured; this is a compiler bug, please report)"
                .to_string()
        } else {
            self.errors.join("\n")
        }
    }
}
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderIoKindTag { StorageBuffer, UniformBuffer, Sampler, Texture, Varying, VertexBuffer, VertexPosition, FragmentOutput, RustInstance, Uniform, DynInstance, ScopeUniform, Workgroup }
impl ShaderIoKind {
    pub fn kind(&self) -> ShaderIoKindTag {
        match self {
            Self::StorageBuffer(_) => ShaderIoKindTag::StorageBuffer,
            Self::UniformBuffer => ShaderIoKindTag::UniformBuffer,
            Self::Sampler(_) => ShaderIoKindTag::Sampler,
            Self::Texture(_) => ShaderIoKindTag::Texture,
            Self::Varying => ShaderIoKindTag::Varying,
            Self::VertexBuffer => ShaderIoKindTag::VertexBuffer,
            Self::VertexPosition => ShaderIoKindTag::VertexPosition,
            Self::FragmentOutput(_) => ShaderIoKindTag::FragmentOutput,
            Self::RustInstance => ShaderIoKindTag::RustInstance,
            Self::Uniform => ShaderIoKindTag::Uniform,
            Self::DynInstance => ShaderIoKindTag::DynInstance,
            Self::ScopeUniform => ShaderIoKindTag::ScopeUniform,
            Self::Workgroup => ShaderIoKindTag::Workgroup,
        }
    }
}
impl ShaderIoKind {
    pub fn as_texture(&self) -> Option<TextureType> { if let Self::Texture(v) = self { Some(*v) } else { None } }
    pub fn fragment_output(&self) -> Option<u8> { if let Self::FragmentOutput(v) = self { Some(*v) } else { None } }
}
impl UniformBufferBindings {
    pub fn bindings(&self) -> &[(LiveId, usize)] { &self.bindings }
    pub fn new(scope_uniform_buffer_index: Option<usize>) -> Self { Self { bindings: Box::default(), scope_uniform_buffer_index } }
    pub fn push(&mut self, name: LiveId, index: usize) {
        let mut v = std::mem::take(&mut self.bindings).into_vec();
        v.push((name, index));
        self.bindings = v.into_boxed_slice();
    }
    pub fn len(&self) -> usize { self.bindings.len() }
    pub fn binding_name(&self, index: usize) -> LiveId { self.bindings[index].0 }
    pub fn binding_index(&self, index: usize) -> usize { self.bindings[index].1 }
}
impl ShaderIoKind {
    pub fn uniform_buffer() -> Self { Self::UniformBuffer }
    pub fn varying() -> Self { Self::Varying }
    pub fn vertex_buffer() -> Self { Self::VertexBuffer }
    pub fn vertex_position() -> Self { Self::VertexPosition }
    pub fn rust_instance() -> Self { Self::RustInstance }
    pub fn uniform() -> Self { Self::Uniform }
    pub fn dyn_instance() -> Self { Self::DynInstance }
    pub fn scope_uniform() -> Self { Self::ScopeUniform }
    pub fn storage_buffer(read: bool, write: bool) -> Self {
        let mut flags = ShaderStorageFlags::default();
        if read { flags.set_read(); }
        if write { flags.set_write(); }
        Self::StorageBuffer(flags)
    }
    pub fn sampler() -> Self { Self::Sampler(ShaderSamplerOptions::default()) }
    pub fn texture(ty: TextureType) -> Self { Self::Texture(ty) }
    pub fn fragment(index: u8) -> Self { Self::FragmentOutput(index) }
    pub fn storage_flags(&self) -> Option<&ShaderStorageFlags> { if let Self::StorageBuffer(flags) = self { Some(flags) } else { None } }
}
impl ShaderIoKind {
    pub fn sampler_options(&self) -> Option<&ShaderSamplerOptions> { if let Self::Sampler(options) = self { Some(options) } else { None } }
}
impl std::fmt::Debug for ShaderIoKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ShaderIoKindTag::*;
        match self.kind() {
            StorageBuffer => f.debug_tuple("StorageBuffer").field(self.storage_flags().unwrap()).finish(),
            Sampler => f.debug_tuple("Sampler").field(self.sampler_options().unwrap()).finish(),
            Texture => f.debug_tuple("Texture").field(&self.as_texture().unwrap()).finish(),
            FragmentOutput => f.debug_tuple("FragmentOutput").field(&self.fragment_output().unwrap()).finish(),
            UniformBuffer => f.write_str("UniformBuffer"), Varying => f.write_str("Varying"), VertexBuffer => f.write_str("VertexBuffer"),
            VertexPosition => f.write_str("VertexPosition"), RustInstance => f.write_str("RustInstance"), Uniform => f.write_str("Uniform"),
            DynInstance => f.write_str("DynInstance"), ScopeUniform => f.write_str("ScopeUniform"),
            Workgroup => f.write_str("Workgroup"),
        }
    }
}
impl std::fmt::Debug for UniformBufferBindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bindings: Vec<_> = (0..self.len()).map(|i| (self.binding_name(i), self.binding_index(i))).collect();
        f.debug_struct("UniformBufferBindings").field("bindings", &bindings).field("scope_uniform_buffer_index", &self.scope_uniform_buffer_index()).finish()
    }
}

use crate::shader_ir::{IrGlobalKind, IrIo};
pub fn global_kind_of(kind: &ShaderIoKind) -> Option<IrGlobalKind> {
    Some(match kind {
        ShaderIoKind::VertexBuffer => IrGlobalKind::Io(IrIo::Geometry),
        ShaderIoKind::DynInstance => IrGlobalKind::Io(IrIo::DynInstance),
        ShaderIoKind::RustInstance => IrGlobalKind::Io(IrIo::RustInstance),
        ShaderIoKind::Uniform => IrGlobalKind::Io(IrIo::Uniform),
        ShaderIoKind::ScopeUniform => IrGlobalKind::Io(IrIo::ScopeUniform),
        ShaderIoKind::UniformBuffer => IrGlobalKind::Io(IrIo::UniformBuffer),
        ShaderIoKind::Varying => IrGlobalKind::Io(IrIo::Varying),
        ShaderIoKind::FragmentOutput(i) => IrGlobalKind::Io(IrIo::FragmentOutput(*i)),
        ShaderIoKind::Texture(t) => IrGlobalKind::Io(IrIo::Texture(*t)),
        ShaderIoKind::VertexPosition => IrGlobalKind::VertexPosition,
        ShaderIoKind::StorageBuffer(flags) => IrGlobalKind::Io(IrIo::Storage(flags.is_write())),
        ShaderIoKind::Workgroup => IrGlobalKind::Io(IrIo::Workgroup),
        _ => return None,
    })
}

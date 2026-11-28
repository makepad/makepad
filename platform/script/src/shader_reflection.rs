//! Finished draw reflection shared by live compilation and shader packs.
use relax_api::relax_api;
use crate::{
    heap::ScriptHeap,
    literal::LiteralSite,
    pod::ScriptPodTy,
    shader::*,
    value::{ScriptObject, ScriptPodType},
};
use makepad_live_id::LiveId;
use makepad_shader::FnReturnFacts;
/// One io row. Plain data (fixed-layout fields, the pod type boxed): JIT code reads it
/// without crossing into the VM (relax_api.txt `jit`).
#[relax_api(plain, run)]
#[derive(Clone, Debug)]
pub struct DrawShaderDescIo {
    pub(crate) kind: ShaderIoKind,
    pub(crate) name: LiveId,
    pub(crate) ty: Box<ScriptPodTy>,
    /// The pod type's name (the draw system's own uniform buffers are told
    /// apart by it).
    pub(crate) pod_name: Option<LiveId>,
    /// The heap's pod type (custom uniform buffers keep it).
    pub(crate) pod_type: ScriptPodType,
    pub(crate) buffer_index: Option<usize>,
    /// Uniform buffers: the block name the code declares.
    pub(crate) block_name: Box<str>,
}

/// A combined GLSL sampler uniform and its logical texture, sampler, and stage mask.
#[relax_api(plain, run)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawShaderDescGlslTexture {
    pub(crate) uniform_name: Box<str>,
    pub(crate) texture_slot: u32,
    pub(crate) sampler_index: u32,
    pub(crate) stages: u8,
}

impl DrawShaderDescGlslTexture {
    pub fn uniform_name(&self) -> &str { &self.uniform_name }
    pub fn texture_slot(&self) -> u32 { self.texture_slot }
    pub fn sampler_index(&self) -> u32 { self.sampler_index }
    pub fn stages(&self) -> u8 { self.stages }
}

/// Where a scope uniform's value comes from.
#[relax_api(plain, run)]
#[derive(Clone, Debug)]
#[repr(C, u8)]
pub enum DrawShaderDescScope {
    /// Read from the heap: `key` looked up from `obj`'s scope.
    Scope(ScriptObject, LiveId),
    /// The table constant at this index.
    Const(usize),
    /// These slot values.
    Fixed(Box<[f32]>),
}

#[relax_api(plain, run)]
#[derive(Clone, Debug)]
pub struct DrawShaderDescScopeUniform {
    pub(crate) shader_name: LiveId,
    pub(crate) slots: usize,
    pub(crate) source: DrawShaderDescScope,
}

/// Everything a draw shader's mapping needs from its compile besides the
/// code: the io in the compiler's order, samplers, scope uniforms (in io
/// order), table constants and uniform buffer bindings. A compile makes one
/// ([`DrawShaderDesc::from_output`]); a prebuilt shader carries one.
#[relax_api(handle, run)]
#[derive(Clone, Debug, Default)]
pub struct DrawShaderDesc {
    pub(crate) io: Vec<DrawShaderDescIo>,
    pub(crate) samplers: Vec<ShaderSampler>,
    pub(crate) glsl_texture_bindings: Vec<DrawShaderDescGlslTexture>,
    pub(crate) scope_uniforms: Vec<DrawShaderDescScopeUniform>,
    pub(crate) table_consts: Vec<ShaderTableConst>,
    /// Live literals that stayed folded (see `ShaderOutput::folded_sites`).
    pub(crate) folded_sites: Vec<LiteralSite>,
    pub(crate) uniform_buffer_bindings: UniformBufferBindings,
    /// The shader reads `draw_pass.time`.
    pub(crate) uses_time: bool,
    pub(crate) return_facts: Box<[FnReturnFacts]>,
}

#[relax_api(run)]
impl DrawShaderDesc {
    /// The description of a finished compile (after
    /// `assign_uniform_buffer_indices`).
    #[relax_api(run)]
    pub fn from_output(heap: &ScriptHeap, output: &crate::shader_compile::CompiledShader) -> Self {
        let io = output
            .io()
            .iter()
            .map(|io| {
                let pod = heap.pod_type_ref(io.ty().into());
                DrawShaderDescIo {
                    kind: io.kind().clone(),
                    name: io.name(),
                    ty: Box::new(pod.ty().clone()),
                    pod_name: pod.name(),
                    pod_type: io.ty().into(),
                    buffer_index: io.buffer_index(),
                    block_name: match io.kind().kind() {
                        crate::shader_output::ShaderIoKindTag::UniformBuffer => {
                            format!("{}_Uniforms", output.backend().map_io_name(io.name())).into()
                        }
                        _ => Box::default(),
                    },
                }
            })
            .collect();
        let mut scope_uniforms = Vec::new();
        for io in output.io() {
            if let ShaderIoKindTag::ScopeUniform = io.kind().kind() {
                if let Some(source) = output
                    .scope_uniforms()
                    .iter()
                    .find(|su| su.shader_name() == io.name())
                {
                    scope_uniforms.push(DrawShaderDescScopeUniform {
                        shader_name: io.name(),
                        slots: heap.pod_type_ref(source.ty()).ty().slots(),
                        source: match source.table_const() {
                            Some(ci) => DrawShaderDescScope::Const(ci),
                            None => DrawShaderDescScope::Scope(source.source_obj(), source.key()),
                        },
                    });
                }
            }
        }
        DrawShaderDesc {
            io,
            samplers: output.samplers().to_vec(),
            glsl_texture_bindings: output.glsl_texture_bindings().iter().map(|b| DrawShaderDescGlslTexture {
                uniform_name: b.uniform_name.as_str().into(),
                texture_slot: b.texture_slot,
                sampler_index: b.sampler_index,
                stages: b.stages,
            }).collect(),
            scope_uniforms,
            table_consts: output.table_consts().to_vec(),
            folded_sites: output.folded_sites().to_vec(),
            uniform_buffer_bindings: output.uniform_buffer_bindings().clone(),
            uses_time: output.uses_time(),
            return_facts: output.return_facts().into(),
        }
    }
}

/// [`DrawShaderDesc::view`]: the description as slices of plain rows, borrowed.
#[relax_api(plain, run)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DrawShaderDescView<'a> {
    pub io: &'a [DrawShaderDescIo],
    pub samplers: &'a [ShaderSampler],
    pub glsl_texture_bindings: &'a [DrawShaderDescGlslTexture],
    pub scope_uniforms: &'a [DrawShaderDescScopeUniform],
    pub table_consts: &'a [ShaderTableConst],
    pub folded_sites: &'a [LiteralSite],
    pub uniform_buffer_bindings: &'a UniformBufferBindings,
    pub uses_time: bool,
    pub return_facts: &'a [FnReturnFacts],
}

impl<'a> DrawShaderDescView<'a> {
    pub fn glsl_texture_bindings(&self) -> &'a [DrawShaderDescGlslTexture] {
        self.glsl_texture_bindings
    }
}

#[relax_api(run)]
impl DrawShaderDesc {
    /// Every part of the description in one call (the accessors below read it; under Relax
    /// they are compiled on the caller's side).
    #[inline]
    #[relax_api(run)]
    pub fn view(&self) -> DrawShaderDescView<'_> {
        DrawShaderDescView {
            io: &self.io,
            samplers: &self.samplers,
            glsl_texture_bindings: &self.glsl_texture_bindings,
            scope_uniforms: &self.scope_uniforms,
            table_consts: &self.table_consts,
            folded_sites: &self.folded_sites,
            uniform_buffer_bindings: &self.uniform_buffer_bindings,
            uses_time: self.uses_time,
            return_facts: &self.return_facts,
        }
    }
    #[inline(always)]
    pub fn io(&self) -> &[DrawShaderDescIo] {
        self.view().io
    }
    #[inline(always)]
    pub fn samplers(&self) -> &[ShaderSampler] {
        self.view().samplers
    }
    #[inline(always)]
    pub fn glsl_texture_bindings(&self) -> &[DrawShaderDescGlslTexture] {
        self.view().glsl_texture_bindings()
    }
    #[inline(always)]
    pub fn scope_uniforms(&self) -> &[DrawShaderDescScopeUniform] {
        self.view().scope_uniforms
    }
    #[inline(always)]
    pub fn table_consts(&self) -> &[ShaderTableConst] {
        self.view().table_consts
    }
    #[inline(always)]
    pub fn folded_sites(&self) -> &[LiteralSite] {
        self.view().folded_sites
    }
    #[inline(always)]
    pub fn uniform_buffer_bindings(&self) -> &UniformBufferBindings {
        self.view().uniform_buffer_bindings
    }
    #[inline(always)]
    pub fn uses_time(&self) -> bool {
        self.view().uses_time
    }
    #[inline(always)]
    pub fn return_facts(&self) -> &[FnReturnFacts] {
        self.view().return_facts
    }
}
impl DrawShaderDescIo {
    pub fn kind(&self) -> &ShaderIoKind {
        &self.kind
    }
    pub fn name(&self) -> LiveId {
        self.name
    }
    pub fn ty(&self) -> &ScriptPodTy {
        &self.ty
    }
    pub fn pod_name(&self) -> Option<LiveId> {
        self.pod_name
    }
    pub fn pod_type(&self) -> ScriptPodType {
        self.pod_type
    }
    pub fn buffer_index(&self) -> Option<usize> {
        self.buffer_index
    }
    pub fn block_name(&self) -> &str {
        &self.block_name
    }
}
impl DrawShaderDescScopeUniform {
    pub fn shader_name(&self) -> LiveId {
        self.shader_name
    }
    pub fn slots(&self) -> usize {
        self.slots
    }
    pub fn table_const(&self) -> Option<usize> {
        match self.source {
            DrawShaderDescScope::Const(i) => Some(i),
            _ => None,
        }
    }
    pub fn source_obj(&self) -> Option<ScriptObject> {
        match self.source {
            DrawShaderDescScope::Scope(obj, _) => Some(obj),
            _ => None,
        }
    }
    pub fn source_key(&self) -> LiveId {
        match self.source {
            DrawShaderDescScope::Scope(_, key) => key,
            _ => LiveId(0),
        }
    }
    pub fn fixed(&self) -> &[f32] {
        match &self.source {
            DrawShaderDescScope::Fixed(values) => values,
            _ => &[],
        }
    }
}

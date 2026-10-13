//! Complete shader operations. Compiler state never leaves this module's result.
use makepad_relax_api::relax_api;
use crate::{
    heap::ScriptHeap,
    shader_backend::ShaderBackend,
    shader_ir_draw::{compile_draw_shader_ir_with, IrCompileOptions},
    shader_output::{
        ScopeUniformSource, ShaderIo, ShaderOutput, ShaderSampler, ShaderTableConst,
        UniformBufferBindings,
    },
    value::ScriptObject,
    vm::ScriptVm,
};
use makepad_live_id::*;

/// Platform policy; all compiler work executes on the owning side.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ShaderCompileOptions {
    pub backend: ShaderBackend,
    pub const_table: bool,
    pub use_vulkan: bool,
    pub glsl_webgl2: bool,
    pub gles_complete: bool,
    pub live_literals: bool,
    pub pick: bool,
    pub xr: bool,
    pub emit_source: bool,
    pub uniform_packing: makepad_shader::uniform_packing::UniformPacking,
}
impl ShaderCompileOptions {
    #[inline]
    pub fn new(
        backend: ShaderBackend,
        const_table: bool,
        use_vulkan: bool,
        glsl_webgl2: bool,
        live_literals: bool,
    ) -> Self {
        Self {
            backend,
            const_table,
            use_vulkan,
            glsl_webgl2,
            gles_complete: false,
            live_literals,
            pick: false,
            xr: false,
            emit_source: true,
            uniform_packing: makepad_shader::uniform_packing::UniformPacking::Std140,
        }
    }
}

/// Finished code and reflection, with no public path back to AST/IR state.
/// References returned by accessors borrow this owner.
#[relax_api(handle)]
pub struct CompiledShader {
    state: ShaderOutput,
    diagnostics: String,
    combined: String,
    pick_code: Option<String>,
    glsl: Option<makepad_shader::shader_ir_glsl::GlslDrawSources>,
    pick_glsl: Option<makepad_shader::shader_ir_glsl::GlslDrawSources>,
    spirv: Option<makepad_shader::shader_ir_draw::IrDrawSpirvResult>,
    pick_spirv: Option<makepad_shader::shader_ir_draw::IrDrawSpirvResult>,
    rust: Option<makepad_shader::shader_ir_rust::RustShaderSource>,
    bindings: UniformBufferBindings,
    varying_total_slots: usize,
}

/// [`CompiledShader::view`]: a finished compile as plain data (borrowed from the owner).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CompiledShaderView<'a> {
    pub has_errors: bool,
    pub error_report: &'a str,
    pub combined: &'a str,
    pub pick_code: Option<&'a str>,
    pub glsl: Option<&'a makepad_shader::shader_ir_glsl::GlslDrawSources>,
    pub pick_glsl: Option<&'a makepad_shader::shader_ir_glsl::GlslDrawSources>,
    pub spirv: Option<&'a makepad_shader::shader_ir_draw::IrDrawSpirvResult>,
    pub pick_spirv: Option<&'a makepad_shader::shader_ir_draw::IrDrawSpirvResult>,
    pub rust: Option<&'a makepad_shader::shader_ir_rust::RustShaderSource>,
    pub table_consts: &'a [ShaderTableConst],
    pub varying_total_slots: usize,
    /// GLSL backend: the two stages ("" for other backends).
    pub glsl_vertex: &'a str,
    pub glsl_fragment: &'a str,
}

#[inline]
#[relax_api]
pub fn compile_shader(
    vm: &mut ScriptVm,
    shader: ScriptObject,
    options: &ShaderCompileOptions,
) -> CompiledShader {
    let opts = IrCompileOptions::new(
        options.backend,
        options.const_table,
        options.use_vulkan,
        options.glsl_webgl2,
        options.live_literals,
    );
    let mut state = compile_draw_shader_ir_with(vm, shader, &opts);
    state.set_pick(options.pick);
    let mut combined = String::new();
    let mut pick_code = None;
    let mut glsl = None;
    let mut pick_glsl = None;
    let mut spirv = None;
    let mut pick_spirv = None;
    let mut rust = None;
    if !state.has_errors() && options.emit_source {
        match options.backend {
            ShaderBackend::Metal => {
                combined = makepad_shader::shader_ir_metal::metal_draw_source(&state, vm, false);
                if options.pick {
                    pick_code = Some(makepad_shader::shader_ir_metal::metal_draw_source(
                        &state, vm, true,
                    ));
                }
            }
            ShaderBackend::Hlsl => {
                combined = makepad_shader::shader_ir_hlsl::hlsl_draw_source(&state, vm);
                if options.pick {
                    pick_code = Some(makepad_shader::shader_ir_hlsl::hlsl_draw_pick_source(&state, vm));
                }
            }
            ShaderBackend::Glsl => {
                if options.use_vulkan {
                    spirv = Some(makepad_shader::shader_ir_draw::draw_spirv_result(
                        &state, vm,
                    ));
                    if options.pick {
                        pick_spirv = Some(makepad_shader::shader_ir_draw::draw_pick_spirv_result(&state, vm));
                    }
                }
                // Vulkan also uses the GLSL source as its existing platform cache key.
                glsl = Some(if options.gles_complete {
                    makepad_shader::shader_ir_glsl::gles_draw_sources(&mut state, vm, false)
                } else {
                    makepad_shader::shader_ir_glsl::glsl_draw_sources(&mut state, vm)
                });
                if options.gles_complete && options.pick {
                    pick_glsl = Some(makepad_shader::shader_ir_glsl::gles_draw_sources(&mut state, vm, true));
                }
            }
            ShaderBackend::Wgsl => {
                match makepad_shader::shader_ir_wgsl::draw_wgsl(&state, vm, options.xr) {
                    Ok((source, _)) => combined = source,
                    Err(error) => state.push_error(error),
                }
            }
            ShaderBackend::Rust => {
                rust = Some(makepad_shader::shader_ir_rust::rust_shader_source(
                    &state, vm,
                ))
            }
        }
    }
    let bindings = state.get_uniform_buffer_bindings(vm.heap());
    let diagnostics = if state.has_errors() {
        state.error_report()
    } else {
        String::new()
    };
    let result = CompiledShader {
        state,
        diagnostics,
        combined,
        pick_code,
        glsl,
        pick_glsl,
        spirv,
        pick_spirv,
        rust,
        bindings,
        varying_total_slots: 0,
    };
    result
}

#[relax_api]
impl CompiledShader {
    /// Say when a shader inlines to more source than a D3D compile handles
    /// quickly. ANGLE on D3D (Windows browsers) and native D3D inline every
    /// call, so a helper called at several sites is compiled once per site:
    /// measured cold on ANGLE D3D11, about 40 KB inlined links in under a
    /// second while 350-500 KB takes 5-13 s, which stalls the frame that
    /// first draws it. The fix is in the shader: one call site in a loop over
    /// its arguments instead of several (a normal from 4 offsets, two rays a
    /// pixel), so the heaviest functions are named with their copy counts.
    #[relax_api]
    pub fn warn_inline_size(&self, vm: &ScriptVm, io_self: ScriptObject) {
        const WARN_INLINED_BYTES: usize = 128 * 1024;
        let Some(src) = self.glsl() else {
            return;
        };
        let total = src.inline_bytes();
        if total <= WARN_INLINED_BYTES {
            return;
        }
        let name = vm
            .heap()
            .object_type_name_in_chain(io_self)
            .map(|id| format!("{}", id))
            .unwrap_or_else(|| format!("<script object {}>", io_self.index()));
        let heaviest: Vec<String> = (0..src.inline_function_count())
            .map(|i| {
                format!(
                    "{} inlined {}x ({} B each)",
                    src.inline_function_name(i),
                    src.inline_function_copies(i),
                    src.inline_function_bytes(i)
                )
            })
            .collect();
        makepad_error_log::warning!(
            "shader '{}' inlines to {} KB (D3D compiles every call site: seconds on Windows browsers; keep it under {} KB): {}",
            name,
            total / 1024,
            WARN_INLINED_BYTES / 1024,
            heaviest.join(", ")
        );
    }

    /// The finished compile as plain data, in one call: the code, diagnostics and table
    /// constants a platform needs (the accessors below read it; under Relax they are compiled on
    /// the caller's side, so this is the only crossing).
    #[inline]
    #[relax_api]
    pub fn view(&self) -> CompiledShaderView<'_> {
        CompiledShaderView {
            has_errors: self.state.has_errors(),
            error_report: &self.diagnostics,
            combined: &self.combined,
            pick_code: self.pick_code.as_deref(),
            glsl: self.glsl.as_ref(),
            pick_glsl: self.pick_glsl.as_ref(),
            spirv: self.spirv.as_ref(),
            pick_spirv: self.pick_spirv.as_ref(),
            rust: self.rust.as_ref(),
            table_consts: self.state.table_consts(),
            varying_total_slots: self.varying_total_slots,
            glsl_vertex: self.glsl.as_ref().map_or("", |g| g.vertex()),
            glsl_fragment: self.glsl.as_ref().map_or("", |g| g.fragment()),
        }
    }
    #[inline(always)]
    pub fn varying_total_slots(&self) -> usize {
        self.view().varying_total_slots
    }
    pub fn ast_function_count(&self) -> usize {
        self.state.ast_functions
    }
    pub fn function_count(&self) -> usize {
        self.state.ir.functions.len()
    }
    #[inline(always)]
    pub fn has_errors(&self) -> bool {
        self.view().has_errors
    }
    #[inline(always)]
    pub fn error_report(&self) -> &str {
        self.view().error_report
    }
    #[inline(always)]
    pub fn combined(&self) -> &str {
        self.view().combined
    }
    #[inline(always)]
    pub fn pick_code(&self) -> Option<&str> {
        self.view().pick_code
    }
    #[inline(always)]
    pub fn glsl(&self) -> Option<&makepad_shader::shader_ir_glsl::GlslDrawSources> {
        self.view().glsl
    }
    #[inline(always)]
    pub fn spirv(&self) -> Option<&makepad_shader::shader_ir_draw::IrDrawSpirvResult> {
        self.view().spirv
    }
    #[inline(always)]
    pub fn rust(&self) -> Option<&makepad_shader::shader_ir_rust::RustShaderSource> {
        self.view().rust
    }
    pub(crate) fn io(&self) -> &[ShaderIo] {
        self.state.io()
    }
    pub(crate) fn samplers(&self) -> &[ShaderSampler] {
        self.state.samplers()
    }
    pub(crate) fn scope_uniforms(&self) -> &[ScopeUniformSource] {
        self.state.scope_uniforms()
    }
    #[inline(always)]
    pub fn table_consts(&self) -> &[ShaderTableConst] {
        self.view().table_consts
    }
    pub(crate) fn folded_sites(&self) -> &[crate::literal::LiteralSite] {
        self.state.folded_sites()
    }
    pub(crate) fn uniform_buffer_bindings(&self) -> &UniformBufferBindings {
        &self.bindings
    }
    pub(crate) fn backend(&self) -> ShaderBackend {
        self.state.backend()
    }
    pub(crate) fn texture_sampler_index(&self, name: &str) -> usize {
        self.state.texture_sampler_index(name)
    }
    pub(crate) fn uses_time(&self) -> bool {
        self.state.uses_time()
    }
    #[relax_api]
    pub fn find_vertex_buffer_object(
        &self,
        vm: &ScriptVm,
        shader: ScriptObject,
    ) -> Option<ScriptObject> {
        self.state.find_vertex_buffer_object(vm, shader)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn reached_functions(&self) -> impl Iterator<Item = ScriptObject> + '_ {
        self.state
            .ir
            .functions
            .iter()
            .filter_map(|f| self.state.function_object(f.fnobj))
    }
}

#[relax_api]
pub fn shader_functions_hash(
    heap: &ScriptHeap,
    code: &crate::vm::ScriptCode,
    obj: ScriptObject,
) -> LiveId {
    let mut hash = LiveId(LiveId::SEED);
    for key in [id!(color_format), id!(blend_op)] {
        hash = hash.id_append(key);
        if let Some(v) = heap.value(obj, key.into(), crate::trap::NoTrap).as_id() {
            hash = hash.id_append(v);
        }
    }
    let mut current = Some(obj);
    while let Some(obj) = current {
        for entry in heap.map_entries(obj) {
            if let Some(fn_obj) = entry.value.as_object() {
                if let Some(fn_ptr) = heap.as_fn(fn_obj) {
                    if let Some(key) = entry.key.as_id() {
                        hash = hash.id_append(key);
                    }
                    if let Some(ip) = fn_ptr.script_ip() {
                        hash = hash.bytes_append(&code.fn_content_key(ip).to_le_bytes());
                    }
                }
            }
        }
        current = heap.proto(obj).as_object();
    }
    hash
}

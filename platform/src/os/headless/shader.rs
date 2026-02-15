use super::CxOsDrawShader;
use crate::{
    draw_shader::{
        CxDrawShader, CxDrawShaderCode, CxDrawShaderMapping, DrawShaderAttrFormat,
        DrawShaderId, DrawShaderInputPacking, DrawShaderInputs,
    },
    draw_vars::DrawVars,
    geometry::Geometry,
    makepad_live_id::*,
    makepad_script::{
        apply::Apply,
        shader::{ShaderFnCompiler, ShaderMode, ShaderOutput, ShaderType},
        shader_backend::ShaderBackend,
        trap::NoTrap,
        value::ScriptValue,
        ScriptVm,
    },
    script::vm::ScriptVmCx,
    Cx,
};
use std::fmt::Write;
use std::hash::{Hash, Hasher};

impl DrawVars {
    pub(crate) fn compile_shader(&mut self, vm: &mut ScriptVm, _apply: &Apply, value: ScriptValue) {
        if let Some(io_self) = value.as_object() {
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_object_id_to_shader.get(&io_self) {
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            let fnhash = DrawVars::compute_shader_functions_hash(&vm.bx.heap, io_self);
            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_functions_to_shader.get(&fnhash) {
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders
                        .cache_object_id_to_shader
                        .insert(io_self, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            let mut output = ShaderOutput::default();
            // Use the Rust backend so the shader compiler emits Rust syntax for
            // function signatures, bodies, struct defs, and type names.
            output.backend = ShaderBackend::Rust;
            output.pre_collect_rust_instance_io(vm, io_self);
            output.pre_collect_shader_io(vm, io_self);

            if let Some(fnobj) = vm
                .bx
                .heap
                .object_method(io_self, id!(vertex).into(), vm.thread().trap.pass())
                .as_object()
            {
                output.mode = ShaderMode::Vertex;
                ShaderFnCompiler::compile_shader_def(
                    vm,
                    &mut output,
                    NoTrap,
                    id!(vertex),
                    fnobj,
                    ShaderType::IoSelf(io_self),
                    vec![],
                );
            }
            if let Some(fnobj) = vm
                .bx
                .heap
                .object_method(io_self, id!(fragment).into(), vm.thread().trap.pass())
                .as_object()
            {
                output.mode = ShaderMode::Fragment;
                ShaderFnCompiler::compile_shader_def(
                    vm,
                    &mut output,
                    NoTrap,
                    id!(fragment),
                    fnobj,
                    ShaderType::IoSelf(io_self),
                    vec![],
                );
            }

            if output.has_errors {
                return;
            }

            output.assign_uniform_buffer_indices(&vm.bx.heap, 3);

            let gen_result = generate_headless_rust_shader_module(&mut output, vm, io_self);
            let varying_total_slots = gen_result.varying_total_slots;
            let code = CxDrawShaderCode::Combined {
                code: gen_result.source.clone(),
            };

            {
                let cx = vm.host.cx();
                if let Some(&shader_id) = cx.draw_shaders.cache_code_to_shader.get(&code) {
                    let cx = vm.host.cx_mut();
                    cx.draw_shaders
                        .cache_object_id_to_shader
                        .insert(io_self, shader_id);
                    cx.draw_shaders
                        .cache_functions_to_shader
                        .insert(fnhash, shader_id);
                    self.finalize_cached_shader(vm, shader_id);
                    return;
                }
            }

            let geometry_id = if let Some(vb_obj) = output.find_vertex_buffer_object(vm, io_self) {
                let buffer_value =
                    vm.bx
                        .heap
                        .value(vb_obj, id!(buffer).into(), vm.thread().trap.pass());
                if let Some(handle) = buffer_value.as_handle() {
                    vm.bx
                        .heap
                        .handle_ref::<Geometry>(handle)
                        .map(|g: &Geometry| g.geometry_id())
                } else {
                    None
                }
            } else {
                None
            };

            let source = vm.bx.heap.new_object_ref(io_self);
            let mut mapping = CxDrawShaderMapping::from_shader_output(
                source,
                code.clone(),
                &vm.bx.heap,
                &output,
                geometry_id,
            );
            mapping.fill_scope_uniforms_buffer(&vm.bx.heap, &vm.thread().trap.pass());
            mapping.varying_total_slots = varying_total_slots;

            let debug_value = vm.bx.heap.value(io_self, id!(debug).into(), NoTrap);
            if let Some(true) = debug_value.as_bool() {
                mapping.flags.debug = true;
            }

            self.dyn_instance_start = self.dyn_instances.len() - mapping.dyn_instances.total_slots;
            self.dyn_instance_slots = mapping.instances.total_slots;

            let cx = vm.host.cx_mut();
            let index = cx.draw_shaders.shaders.len();
            cx.draw_shaders.shaders.push(CxDrawShader {
                debug_id: LiveId(0),
                os_shader_id: None,
                mapping,
            });

            let shader_id = DrawShaderId { index };
            cx.draw_shaders
                .cache_object_id_to_shader
                .insert(io_self, shader_id);
            cx.draw_shaders
                .cache_functions_to_shader
                .insert(fnhash, shader_id);
            cx.draw_shaders.cache_code_to_shader.insert(code, shader_id);
            cx.draw_shaders.compile_set.insert(index);

            self.draw_shader_id = Some(shader_id);
            self.geometry_id = geometry_id;
        }
    }
}

impl Cx {
    pub(crate) fn headless_compile_shaders(&mut self) {
        let compile_set = std::mem::take(&mut self.draw_shaders.compile_set);
        for shader_index in compile_set {
            let cx_shader = &mut self.draw_shaders.shaders[shader_index];
            if cx_shader.os_shader_id.is_some() {
                continue;
            }

            let source = match &cx_shader.mapping.code {
                CxDrawShaderCode::Combined { code } => code.as_str(),
                CxDrawShaderCode::Separate { vertex, fragment } => {
                    crate::warning!(
                        "headless backend expected combined Rust source but got separate shaders; synthesizing module"
                    );
                    if vertex.len() > fragment.len() {
                        vertex.as_str()
                    } else {
                        fragment.as_str()
                    }
                }
            };
            let source_hash = hash_string(source);

            if let Some((existing_index, _)) = self
                .draw_shaders
                .os_shaders
                .iter()
                .enumerate()
                .find(|(_, os_shader)| os_shader.source_hash == source_hash)
            {
                cx_shader.os_shader_id = Some(existing_index);
                continue;
            }

            let mut os_shader = CxOsDrawShader {
                source_hash,
                ..Default::default()
            };
            let mut has_derivative_export = false;
            match self.os.shader_jit.compile_and_load(source_hash, source) {
                Ok(jit_output) => {
                    os_shader.dylib_path = Some(jit_output.dylib_path);
                    os_shader.shader_version = jit_output.shader_version;
                    os_shader.load_error = jit_output.load_error;
                    // Query RenderCx layout before storing module
                    if let Some(ref module) = jit_output.module {
                        type LayoutFn = extern "C" fn() -> u32;
                        if let Ok(f) = module.symbol::<LayoutFn>("makepad_headless_render_cx_size")
                        {
                            os_shader.rcx_size = f() as usize;
                        }
                        if let Ok(f) = module.symbol::<LayoutFn>("makepad_headless_rcx_vary_offset")
                        {
                            os_shader.rcx_vary_offset = f() as usize;
                        }
                        if let Ok(f) =
                            module.symbol::<LayoutFn>("makepad_headless_rcx_quad_mode_offset")
                        {
                            os_shader.rcx_quad_mode_offset = f() as usize;
                        }
                        if let Ok(f) = module.symbol::<LayoutFn>("makepad_headless_flat_varying_slots")
                        {
                            os_shader.flat_varying_slots = f() as usize;
                        }
                        if let Ok(f) = module.symbol::<LayoutFn>("makepad_headless_uses_derivatives")
                        {
                            os_shader.uses_derivatives = f() != 0;
                            has_derivative_export = true;
                        }
                        if let Ok(f) = module.symbol::<LayoutFn>("makepad_headless_rcx_frag_offset")
                        {
                            os_shader.rcx_frag_offset = f() as usize;
                        }
                        if let Ok(f) =
                            module.symbol::<LayoutFn>("makepad_headless_rcx_discard_offset")
                        {
                            os_shader.rcx_discard_offset = f() as usize;
                        }
                    }
                    os_shader.module = jit_output.module;
                }
                Err(err) => {
                    os_shader.load_error = Some(err.clone());
                    crate::error!("{err}");
                }
            }
            // Back-compat fallback: if an older JIT module is loaded without the
            // flat-varying export, keep previous behavior.
            if os_shader.flat_varying_slots == 0 {
                os_shader.flat_varying_slots = cx_shader
                    .mapping
                    .instances
                    .total_slots
                    .min(cx_shader.mapping.varying_total_slots);
            }
            if !has_derivative_export {
                // Conservative back-compat fallback for older JIT modules without
                // the derivative usage export.
                os_shader.uses_derivatives = true;
            }

            let os_shader_id = self.draw_shaders.os_shaders.len();
            self.draw_shaders.os_shaders.push(os_shader);
            cx_shader.os_shader_id = Some(os_shader_id);
        }
    }
}

fn hash_string(s: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

// ─────────────────────────────────────────────────────────────────────────────
// Rust shader module code generation
// ─────────────────────────────────────────────────────────────────────────────

struct HeadlessShaderGenResult {
    source: String,
    varying_total_slots: usize,
}

fn generate_headless_rust_shader_module(
    output: &mut ShaderOutput,
    vm: &ScriptVm,
    io_self: crate::ScriptObject,
) -> HeadlessShaderGenResult {
    let mut out = String::with_capacity(8192);
    let io_self_idx = io_self.index();

    // ── File header ──
    writeln!(out, "//! Auto-generated Makepad headless shader module.").ok();
    writeln!(out, "//! io_self object index: {io_self_idx}").ok();
    writeln!(
        out,
        "#![allow(unused_variables, unused_mut, unused_imports, dead_code,"
    )
    .ok();
    writeln!(
        out,
        "         unused_parens, unused_assignments, non_snake_case,"
    )
    .ok();
    writeln!(out, "         non_camel_case_types, unreachable_code,").ok();
    writeln!(
        out,
        "         unused_unsafe, redundant_semicolons, while_true)]"
    )
    .ok();
    writeln!(out).ok();

    // ── Inline runtime preamble ──
    out.push_str(SHADER_RUNTIME_PREAMBLE);
    writeln!(out).ok();

    // Compute total varying slots (needed for RenderCx derivative arrays)
    let varying_total_slots = count_varying_slots(output, vm);
    // Flat varying prefix: dyn/rust instance slots in the packed varying stream.
    let flat_varying_slots = count_flat_varying_slots(output, vm);
    let uses_derivatives = output.uses_derivatives;

    // ── Pod struct definitions from shader output (uniform buffer structs etc.) ──
    write_filtered_struct_defs(output, vm, &mut out);
    writeln!(out).ok();

    // ── Per-shader RenderCx struct (#[repr(C)] POD) ──
    write_render_cx_struct(output, vm, &mut out);
    writeln!(out).ok();

    // ── Per-shader dFdx/dFdy fallback functions ──
    write_dfdx_dfdy_fallbacks(&mut out);
    writeln!(out).ok();

    // ── Compiled shader functions (safe Rust, take rcx: &mut RenderCx) ──
    write_shader_functions(output, &mut out);
    writeln!(out).ok();

    // ── Entry points ──
    writeln!(out, "#[no_mangle]").ok();
    writeln!(
        out,
        "pub extern \"C\" fn makepad_headless_shader_version() -> u32 {{ 3 }}"
    )
    .ok();
    writeln!(out, "#[no_mangle]").ok();
    writeln!(
        out,
        "pub extern \"C\" fn makepad_headless_flat_varying_slots() -> u32 {{ {}u32 }}",
        flat_varying_slots
    )
    .ok();
    writeln!(out, "#[no_mangle]").ok();
    writeln!(
        out,
        "pub extern \"C\" fn makepad_headless_uses_derivatives() -> u32 {{ {}u32 }}",
        if uses_derivatives { 1 } else { 0 }
    )
    .ok();
    writeln!(out).ok();

    // Export RenderCx layout info so the host can fill the buffer correctly
    write_render_cx_layout_exports(output, vm, &mut out);
    writeln!(out).ok();

    write_fill_rcx_entry(output, vm, &mut out);
    writeln!(out).ok();

    write_vertex_entry(output, vm, &mut out);
    writeln!(out).ok();

    write_fragment_entry(output, vm, &mut out);

    HeadlessShaderGenResult {
        source: out,
        varying_total_slots,
    }
}

/// Write the varying fields (DynInstance, RustInstance, Varying) with an optional prefix.
/// Called 3 times: once for the main varyings (""), once for dx-shifted ("dx_"), once for dy-shifted ("dy_").
fn write_varying_fields(output: &ShaderOutput, vm: &ScriptVm, out: &mut String, prefix: &str) {
    use crate::makepad_script::shader::ShaderIoKind;
    for io in &output.io {
        if !matches!(io.kind, ShaderIoKind::DynInstance) {
            continue;
        }
        let io_name = output.backend.map_io_name(io.name);
        let ty = type_name(output, vm, io.ty);
        writeln!(out, "    {prefix}dyninst_{io_name}: {ty},").ok();
    }
    for io in &output.io {
        if !matches!(io.kind, ShaderIoKind::RustInstance) {
            continue;
        }
        let io_name = output.backend.map_io_name(io.name);
        let ty = type_name(output, vm, io.ty);
        writeln!(out, "    {prefix}rustinst_{io_name}: {ty},").ok();
    }
    for io in &output.io {
        if !matches!(io.kind, ShaderIoKind::Varying) {
            continue;
        }
        let io_name = output.backend.map_io_name(io.name);
        let ty = type_name(output, vm, io.ty);
        writeln!(out, "    {prefix}var_{io_name}: {ty},").ok();
    }
}

/// Generate the per-shader `RenderCx` struct — `#[repr(C)]` pure POD.
///
/// Field ordering is chosen so the host can fill contiguous regions:
///   1. Varyings  (dyninst + rustinst + var) — written per-pixel by rasterizer
///   2. Derivatives (dfdx, dfdy)             — written per-triangle
///   3. Uniforms   (uni + su)                — written per-draw-call
///   4. Uniform buffers (unibuf)             — written per-draw-call
///   5. Textures   (tex)                     — written per-draw-call
///   6. Geometry   (vb)                      — vertex shader only
///   7. Vertex position (vtx_pos)            — vertex shader only
///   8. Fragment output  (frag_fb)           — written by shader, read by host
///   9. Flags (discard)                      — written by shader
///
/// The host gets the f32 offsets of each group from exported `extern "C"` fns.
fn write_render_cx_struct(output: &ShaderOutput, vm: &ScriptVm, out: &mut String) {
    use crate::makepad_script::shader::ShaderIoKind;

    writeln!(out, "#[repr(C)]").ok();
    writeln!(out, "struct RenderCx {{").ok();

    // Group 1: Varyings — order MUST match vertex entry vary[] packing:
    // DynInstance first, then RustInstance, then Varying
    write_varying_fields(output, vm, out, "");

    // Group 1b: Quad derivative buffers for 3-pass dFdx/dFdy
    // quad_mode: 0=record_dx, 1=record_dy, 2=compute
    // quad_slot: auto-incrementing slot index per dFdx/dFdy call
    // quad_lane_x/quad_lane_y: 0 or 1 lane parity in the 2x2 pixel quad
    // quad_dx_buf/quad_dy_buf: stored intermediate values from neighbor pixels
    writeln!(out, "    quad_mode: u32,").ok();
    writeln!(out, "    quad_slot: u32,").ok();
    writeln!(out, "    quad_lane_x: u32,").ok();
    writeln!(out, "    quad_lane_y: u32,").ok();
    writeln!(out, "    quad_dx_buf: [f32; 32],").ok();
    writeln!(out, "    quad_dy_buf: [f32; 32],").ok();

    // Group 3: Uniforms
    for io in &output.io {
        let io_name = output.backend.map_io_name(io.name);
        match &io.kind {
            ShaderIoKind::Uniform => {
                let ty = type_name(output, vm, io.ty);
                writeln!(out, "    uni_{io_name}: {ty},").ok();
            }
            ShaderIoKind::ScopeUniform => {
                let ty = type_name(output, vm, io.ty);
                writeln!(out, "    su_{io_name}: {ty},").ok();
            }
            _ => {}
        }
    }

    // Group 4: Uniform buffer structs
    for io in &output.io {
        let io_name = output.backend.map_io_name(io.name);
        if let ShaderIoKind::UniformBuffer = &io.kind {
            let ty = type_name(output, vm, io.ty);
            writeln!(out, "    unibuf_{io_name}: {ty},").ok();
        }
    }

    // Group 5: Textures (POD — data_ptr, data_len, width, height as usize)
    for io in &output.io {
        let io_name = output.backend.map_io_name(io.name);
        if let ShaderIoKind::Texture(_) = &io.kind {
            writeln!(out, "    tex_{io_name}: Texture2D,").ok();
        }
    }

    // Group 6: Geometry (vertex buffer fields — vertex shader only)
    for io in &output.io {
        let io_name = output.backend.map_io_name(io.name);
        if let ShaderIoKind::VertexBuffer = &io.kind {
            let ty = type_name(output, vm, io.ty);
            writeln!(out, "    vb_{io_name}: {ty},").ok();
        }
    }

    // Group 7: Vertex position (vertex shader only)
    if output
        .io
        .iter()
        .any(|io| matches!(io.kind, ShaderIoKind::VertexPosition))
    {
        writeln!(out, "    vtx_pos: Vec4f,").ok();
    }

    // Group 8: Fragment output
    for io in &output.io {
        if let ShaderIoKind::FragmentOutput(idx) = &io.kind {
            let ty = type_name(output, vm, io.ty);
            writeln!(out, "    frag_fb{idx}: {ty},").ok();
        }
    }

    // Group 9: Discard flag (set by shader's discard → return pattern)
    writeln!(out, "    discard: f32,").ok();

    writeln!(out, "}}").ok();
}

/// Export `extern "C"` functions that tell the host the byte layout of RenderCx.
/// The host uses these to fill varyings, derivatives, uniforms, textures at the
/// correct byte offsets in its pre-allocated f32 buffer.
fn write_render_cx_layout_exports(output: &ShaderOutput, _vm: &ScriptVm, out: &mut String) {
    use crate::makepad_script::shader::ShaderIoKind;

    // Helper: generate an offset_of function using pointer arithmetic
    // (works on all Rust editions without nightly)
    let offset_of = |out: &mut String, fn_name: &str, field: &str| {
        writeln!(out, "#[no_mangle]").ok();
        writeln!(out, "pub extern \"C\" fn {fn_name}() -> u32 {{").ok();
        writeln!(out, "    let base = std::ptr::null::<RenderCx>();").ok();
        writeln!(
            out,
            "    unsafe {{ (std::ptr::addr_of!((*base).{field}) as usize) as u32 }}"
        )
        .ok();
        writeln!(out, "}}").ok();
    };

    // Total size in bytes
    writeln!(out, "#[no_mangle]").ok();
    writeln!(out, "pub extern \"C\" fn makepad_headless_render_cx_size() -> u32 {{ std::mem::size_of::<RenderCx>() as u32 }}").ok();

    // Varying region: byte offset of first varying field in Group 1.
    // Group 1 order is: DynInstance, RustInstance, Varying — must match vertex packing.
    let first_vary = output
        .io
        .iter()
        .find(|io| matches!(io.kind, ShaderIoKind::DynInstance))
        .or_else(|| {
            output
                .io
                .iter()
                .find(|io| matches!(io.kind, ShaderIoKind::RustInstance))
        })
        .or_else(|| {
            output
                .io
                .iter()
                .find(|io| matches!(io.kind, ShaderIoKind::Varying))
        });
    if let Some(io) = first_vary {
        let io_name = output.backend.map_io_name(io.name);
        let prefix = match io.kind {
            ShaderIoKind::DynInstance => "dyninst_",
            ShaderIoKind::RustInstance => "rustinst_",
            ShaderIoKind::Varying => "var_",
            _ => unreachable!(),
        };
        offset_of(
            out,
            "makepad_headless_rcx_vary_offset",
            &format!("{prefix}{io_name}"),
        );
    } else {
        // No varyings — export 0
        writeln!(
            out,
            "#[no_mangle]\npub extern \"C\" fn makepad_headless_rcx_vary_offset() -> u32 {{ 0 }}"
        )
        .ok();
    }

    // Quad mode field byte offset (for 3-pass dFdx/dFdy)
    offset_of(out, "makepad_headless_rcx_quad_mode_offset", "quad_mode");

    // Fragment output byte offset (frag_fb0)
    if output
        .io
        .iter()
        .any(|io| matches!(io.kind, ShaderIoKind::FragmentOutput(0)))
    {
        offset_of(out, "makepad_headless_rcx_frag_offset", "frag_fb0");
    }

    // Discard flag byte offset
    offset_of(out, "makepad_headless_rcx_discard_offset", "discard");
}

/// Count the total number of varying float slots (dyn_inst + rust_inst + varyings).
fn count_varying_slots(output: &ShaderOutput, vm: &ScriptVm) -> usize {
    use crate::makepad_script::shader::ShaderIoKind;
    let mut slots = 0usize;
    for io in &output.io {
        match io.kind {
            ShaderIoKind::DynInstance | ShaderIoKind::RustInstance | ShaderIoKind::Varying => {
                slots += vm.bx.heap.pod_type_ref(io.ty).ty.slots();
            }
            _ => {}
        }
    }
    slots
}

/// Count packed varying slots that correspond to dyn/rust instances.
fn count_flat_varying_slots(output: &ShaderOutput, vm: &ScriptVm) -> usize {
    use crate::makepad_script::shader::ShaderIoKind;
    let mut slots = 0usize;
    for io in &output.io {
        match io.kind {
            ShaderIoKind::DynInstance | ShaderIoKind::RustInstance => {
                slots += vm.bx.heap.pod_type_ref(io.ty).ty.slots();
            }
            _ => {}
        }
    }
    slots
}

/// Generate per-shader dFdx/dFdy fallback functions.
/// The real derivative logic is emitted inline by the shader compiler as
/// record/compute blocks that use rcx.quad_mode, quad_slot, quad_dx_buf, quad_dy_buf.
fn write_dfdx_dfdy_fallbacks(out: &mut String) {
    // Fallback dFdx/dFdy functions — should not be called since the compiler
    // emits inline quad-buffer blocks, but keeps compilation working.
    writeln!(
        out,
        "fn dFdx(_rcx: &mut RenderCx, _x: f32) -> f32 {{ 0.0 }}"
    )
    .ok();
    writeln!(
        out,
        "fn dFdy(_rcx: &mut RenderCx, _x: f32) -> f32 {{ 0.0 }}"
    )
    .ok();
    writeln!(
        out,
        "fn dFdx_2f(_rcx: &mut RenderCx, _v: Vec2f) -> Vec2f {{ vec2(0.0, 0.0) }}"
    )
    .ok();
    writeln!(
        out,
        "fn dFdy_2f(_rcx: &mut RenderCx, _v: Vec2f) -> Vec2f {{ vec2(0.0, 0.0) }}"
    )
    .ok();
}

/// Emit shader functions. Functions with `*mut` parameters (e.g. Sdf2d methods)
/// get their bodies wrapped in `unsafe` for raw pointer dereferences.
fn write_shader_functions(output: &ShaderOutput, out: &mut String) {
    for func in &output.functions {
        let needs_unsafe = func.call_sig.contains("*mut");
        if needs_unsafe {
            writeln!(out, "{} {{ unsafe {{", func.call_sig).ok();
            writeln!(out, "{}", func.out).ok();
            writeln!(out, "}} }}\n").ok();
        } else {
            writeln!(out, "{} {{", func.call_sig).ok();
            writeln!(out, "{}", func.out).ok();
            writeln!(out, "}}\n").ok();
        }
    }
}

/// Emit `makepad_headless_fill_rcx` — fills uniforms and textures into a RenderCx buffer.
/// Called once per draw call (cold path). The host passes its pre-allocated rcx buffer
/// plus uniform arrays and texture info. This entry writes the uniform/texture fields
/// at the correct byte offsets so the fragment entry can use them zero-copy.
fn write_fill_rcx_entry(output: &ShaderOutput, vm: &ScriptVm, out: &mut String) {
    use crate::makepad_script::shader::ShaderIoKind;

    writeln!(out, "#[no_mangle]").ok();
    writeln!(out, "pub extern \"C\" fn makepad_headless_fill_rcx(").ok();
    writeln!(out, "    rcx_ptr: *mut f32, rcx_f32s: u32,").ok();
    writeln!(
        out,
        "    uniform_ptrs: *const *const f32, uniform_lens: *const u32, uniform_count: u32,"
    )
    .ok();
    // tex_infos: array of [data_ptr, data_len, width, height] as usize
    writeln!(out, "    tex_infos_ptr: *const [usize; 4], tex_count: u32,").ok();
    writeln!(out, ") {{ unsafe {{").ok();

    // Validate
    writeln!(
        out,
        "    let expected = std::mem::size_of::<RenderCx>() / std::mem::size_of::<f32>();"
    )
    .ok();
    writeln!(out, "    if (rcx_f32s as usize) < expected {{ return; }}").ok();
    writeln!(out, "    let rcx = &mut *(rcx_ptr as *mut RenderCx);").ok();
    writeln!(out).ok();

    // Unpack uniforms
    write_uniform_unpack(output, vm, out, "rcx.");
    writeln!(out).ok();

    // Fill texture fields from tex_infos array
    {
        let mut tex_idx = 0usize;
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::Texture(_)) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            writeln!(out, "    if {tex_idx} < tex_count as usize {{").ok();
            writeln!(out, "        let ti = *tex_infos_ptr.add({tex_idx});").ok();
            writeln!(out, "        rcx.tex_{io_name} = Texture2D {{").ok();
            writeln!(out, "            data_ptr: ti[0],").ok();
            writeln!(out, "            data_len: ti[1],").ok();
            writeln!(out, "            width: ti[2],").ok();
            writeln!(out, "            height: ti[3],").ok();
            writeln!(out, "        }};").ok();
            writeln!(out, "    }}").ok();
            tex_idx += 1;
        }
    }

    writeln!(out, "}} }}").ok();
}

/// Emit the vertex shader entry point.
fn write_vertex_entry(output: &ShaderOutput, vm: &ScriptVm, out: &mut String) {
    use crate::makepad_script::shader::ShaderIoKind;

    writeln!(out, "#[no_mangle]").ok();
    writeln!(out, "pub extern \"C\" fn makepad_headless_vertex(").ok();
    writeln!(out, "    geom_ptr: *const f32, geom_len: u32,").ok();
    writeln!(out, "    inst_ptr: *const f32, inst_len: u32,").ok();
    writeln!(
        out,
        "    uniform_ptrs: *const *const f32, uniform_lens: *const u32, uniform_count: u32,"
    )
    .ok();
    writeln!(out, "    varying_out: *mut f32, varying_len: u32,").ok();
    writeln!(out, "    out_pos: *mut [f32; 4],").ok();
    writeln!(out, ") {{ unsafe {{").ok();
    writeln!(
        out,
        "    let geom = std::slice::from_raw_parts(geom_ptr, geom_len as usize);"
    )
    .ok();
    writeln!(
        out,
        "    let inst = std::slice::from_raw_parts(inst_ptr, inst_len as usize);"
    )
    .ok();
    writeln!(out).ok();

    // Build RenderCx — zeroed, then fill in fields
    writeln!(out, "    let mut rcx: RenderCx = std::mem::zeroed();").ok();
    writeln!(out, "    rcx.vtx_pos.w = 1.0;").ok();
    writeln!(out).ok();

    // Unpack geometry fields → rcx.vb_*
    {
        let mut slot = 0usize;
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::VertexBuffer) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            let ty = type_name(output, vm, io.ty);
            let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
            let is_struct = matches!(
                vm.bx.heap.pod_type_ref(io.ty).ty,
                crate::makepad_script::pod::ScriptPodTy::Struct { .. }
            );
            if is_struct {
                writeln!(
                    out,
                    "    rcx.vb_{io_name} = std::ptr::read(geom.as_ptr().add({slot}) as *const {ty});"
                )
                .ok();
            } else {
                write_static_assign(out, &format!("rcx.vb_{io_name}"), "geom", slot, slots);
            }
            slot += slots;
        }
    }

    // Unpack instance fields → rcx.dyninst_* / rcx.rustinst_*
    {
        let mut slot = 0usize;
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::DynInstance) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            let ty = type_name(output, vm, io.ty);
            let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
            write_static_assign_typed(
                out,
                &format!("rcx.dyninst_{io_name}"),
                "inst",
                slot,
                slots,
                &ty,
            );
            slot += slots;
        }
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::RustInstance) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            let ty = type_name(output, vm, io.ty);
            let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
            write_static_assign_typed(
                out,
                &format!("rcx.rustinst_{io_name}"),
                "inst",
                slot,
                slots,
                &ty,
            );
            slot += slots;
        }
    }

    // Unpack uniforms → rcx.unibuf_*, rcx.uni_*, rcx.su_*
    write_uniform_unpack(output, vm, out, "rcx.");
    writeln!(out).ok();

    // Call vertex function
    let vertex_returns_vec4f = output
        .functions
        .iter()
        .find(|f| f.name == id!(vertex))
        .map(|f| f.ret == vm.bx.code.builtins.pod.pod_vec4f)
        .unwrap_or(false);
    if vertex_returns_vec4f {
        writeln!(out, "    rcx.vtx_pos = io_vertex(&mut rcx);").ok();
    } else {
        writeln!(out, "    io_vertex(&mut rcx);").ok();
    }
    writeln!(out).ok();

    // Write output position
    writeln!(
        out,
        "    *out_pos = [rcx.vtx_pos.x, rcx.vtx_pos.y, rcx.vtx_pos.z, rcx.vtx_pos.w];"
    )
    .ok();
    writeln!(out).ok();

    // Pack varyings out
    writeln!(
        out,
        "    let vary = std::slice::from_raw_parts_mut(varying_out, varying_len as usize);"
    )
    .ok();
    {
        let mut slot = 0usize;
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::DynInstance) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            let ty = type_name(output, vm, io.ty);
            let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
            write_static_pack_typed(
                out,
                "vary",
                &format!("rcx.dyninst_{io_name}"),
                slot,
                slots,
                &ty,
            );
            slot += slots;
        }
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::RustInstance) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            let ty = type_name(output, vm, io.ty);
            let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
            write_static_pack_typed(
                out,
                "vary",
                &format!("rcx.rustinst_{io_name}"),
                slot,
                slots,
                &ty,
            );
            slot += slots;
        }
        for io in &output.io {
            if !matches!(io.kind, ShaderIoKind::Varying) {
                continue;
            }
            let io_name = output.backend.map_io_name(io.name);
            let ty = type_name(output, vm, io.ty);
            let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
            write_static_pack_typed(out, "vary", &format!("rcx.var_{io_name}"), slot, slots, &ty);
            slot += slots;
        }
    }

    writeln!(out, "}} }}").ok();
}

/// Emit the fragment shader entry point — zero-copy transmute from host buffer.
///
/// Signature: `fn(rcx_ptr: *mut f32, rcx_f32s: u32) -> u32`
///   - `rcx_ptr`: pointer to a host-allocated f32 buffer ≥ `size_of::<RenderCx>()` bytes
///   - `rcx_f32s`: buffer size in f32s (host gets this from `makepad_headless_render_cx_f32s()`)
///   - returns: 0 = discard, 1 = write pixel
///
/// The host pre-fills the buffer with varyings (group 1), derivatives (group 2),
/// uniforms (groups 3+4), and textures (group 5). Fragment output (group 8) and
/// discard flag (group 9) are zeroed. The entry transmutes, runs `io_fragment`,
/// and the host reads `frag_fb0` and `discard` directly from the buffer.
fn write_fragment_entry(output: &ShaderOutput, _vm: &ScriptVm, out: &mut String) {
    use crate::makepad_script::shader::ShaderIoKind;

    writeln!(out, "#[no_mangle]").ok();
    writeln!(out, "pub extern \"C\" fn makepad_headless_fragment(").ok();
    writeln!(out, "    rcx_ptr: *mut f32, rcx_f32s: u32,").ok();
    writeln!(out, ") -> u32 {{ unsafe {{").ok();
    // Validate buffer size
    writeln!(
        out,
        "    let expected = std::mem::size_of::<RenderCx>() / std::mem::size_of::<f32>();"
    )
    .ok();
    writeln!(out, "    if (rcx_f32s as usize) < expected {{ return 0; }}").ok();
    // Zero-copy transmute
    writeln!(out, "    let rcx = &mut *(rcx_ptr as *mut RenderCx);").ok();
    // Reset output fields before running shader
    writeln!(out, "    rcx.discard = 0.0;").ok();
    for io in &output.io {
        if let ShaderIoKind::FragmentOutput(idx) = &io.kind {
            writeln!(
                out,
                "    rcx.frag_fb{idx} = Vec4f {{ x: 0.0, y: 0.0, z: 0.0, w: 0.0 }};"
            )
            .ok();
        }
    }
    writeln!(out).ok();

    // Call fragment function
    writeln!(out, "    io_fragment(rcx);").ok();
    writeln!(out).ok();

    // Return 0 if discarded, 1 if pixel should be written
    writeln!(out, "    if rcx.discard != 0.0 {{ 0 }} else {{ 1 }}").ok();
    writeln!(out, "}} }}").ok();
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper functions for code generation
// ─────────────────────────────────────────────────────────────────────────────

/// Write struct definitions, skipping types already provided by the preamble.
fn write_filtered_struct_defs(output: &mut ShaderOutput, vm: &ScriptVm, out: &mut String) {
    use crate::makepad_script::pod::ScriptPodTy;

    // Collect struct types from IO
    for io in &output.io {
        let ty = io.ty;
        if let ScriptPodTy::Struct { .. } = vm.bx.heap.pod_type_ref(ty).ty {
            output.structs.insert(ty);
        }
    }

    let preamble_types: &[&str] = &[
        "Sdf2d",
        "Texture2D",
        "Vec2f",
        "Vec3f",
        "Vec4f",
        "Mat4f",
        "vec2f",
        "vec3f",
        "vec4f",
        "mat4x4f",
    ];

    let filtered: std::collections::BTreeSet<_> = output
        .structs
        .iter()
        .filter(|&&ty| {
            if let Some(name) = vm.bx.heap.pod_type_ref(ty).name {
                let name_str = format!("{}", name);
                !preamble_types.contains(&name_str.as_str())
            } else {
                true
            }
        })
        .copied()
        .collect();

    output.backend.pod_struct_defs(&vm.bx.heap, &filtered, out);
}

fn type_name(
    output: &ShaderOutput,
    vm: &ScriptVm,
    ty: crate::makepad_script::value::ScriptPodType,
) -> String {
    let mut s = String::new();
    output
        .backend
        .pod_type_name_from_ty(&vm.bx.heap, ty, &mut s);
    s
}

#[allow(unused)]
fn zero_val(_output: &ShaderOutput, ty_name: &str) -> String {
    match ty_name {
        "f32" => "0.0f32".to_string(),
        "u32" => "0u32".to_string(),
        "i32" => "0i32".to_string(),
        "bool" => "false".to_string(),
        "Vec2f" | "vec2f" => "Vec2f { x: 0.0, y: 0.0 }".to_string(),
        "Vec3f" | "vec3f" => "Vec3f { x: 0.0, y: 0.0, z: 0.0 }".to_string(),
        "Vec4f" | "vec4f" => "Vec4f { x: 0.0, y: 0.0, z: 0.0, w: 0.0 }".to_string(),
        "Mat4f" | "mat4x4f" => "unsafe { std::mem::zeroed() }".to_string(),
        _ => format!("unsafe {{ std::mem::zeroed() }}"),
    }
}

/// Keep headless uniform unpack offsets consistent with draw shader mapping.
fn headless_uniform_packing() -> DrawShaderInputPacking {
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

/// Assign a value from a float buffer to a variable.
fn write_static_assign(out: &mut String, var_name: &str, buf: &str, offset: usize, slots: usize) {
    write_static_assign_typed(out, var_name, buf, offset, slots, "f32");
}

fn write_static_assign_typed(
    out: &mut String,
    var_name: &str,
    buf: &str,
    offset: usize,
    slots: usize,
    ty: &str,
) {
    let read_elem = |out: &mut String, idx: usize| {
        if ty == "u32" {
            write!(out, "{buf}[{idx}].to_bits()").ok();
        } else {
            write!(out, "{buf}[{idx}]").ok();
        }
    };
    match slots {
        1 => {
            write!(out, "    {var_name} = ").ok();
            read_elem(out, offset);
            writeln!(out, ";").ok();
        }
        2 => {
            write!(out, "    {var_name} = vec2(").ok();
            read_elem(out, offset);
            write!(out, ", ").ok();
            read_elem(out, offset + 1);
            writeln!(out, ");").ok();
        }
        3 => {
            write!(out, "    {var_name} = vec3(").ok();
            read_elem(out, offset);
            write!(out, ", ").ok();
            read_elem(out, offset + 1);
            write!(out, ", ").ok();
            read_elem(out, offset + 2);
            writeln!(out, ");").ok();
        }
        4 => {
            write!(out, "    {var_name} = vec4(").ok();
            read_elem(out, offset);
            write!(out, ", ").ok();
            read_elem(out, offset + 1);
            write!(out, ", ").ok();
            read_elem(out, offset + 2);
            write!(out, ", ").ok();
            read_elem(out, offset + 3);
            writeln!(out, ");").ok();
        }
        n => {
            writeln!(
                out,
                "    {var_name} = unsafe {{ std::mem::transmute::<[f32; {n}], _>(["
            )
            .ok();
            for i in 0..n {
                if i > 0 {
                    write!(out, ", ").ok();
                }
                write!(out, "{buf}[{}]", offset + i).ok();
            }
            writeln!(out, "]) }};").ok();
        }
    }
}

fn write_static_pack_typed(
    out: &mut String,
    buf: &str,
    var_name: &str,
    offset: usize,
    slots: usize,
    ty: &str,
) {
    let wrap = |val: &str| -> String {
        if ty == "u32" {
            format!("f32::from_bits({val})")
        } else {
            val.to_string()
        }
    };
    match slots {
        1 => {
            let v = wrap(var_name);
            writeln!(out, "    {buf}[{offset}] = {v};").ok();
        }
        2 => {
            let vx = wrap(&format!("{var_name}.x"));
            let vy = wrap(&format!("{var_name}.y"));
            writeln!(
                out,
                "    {buf}[{offset}] = {vx}; {buf}[{}] = {vy};",
                offset + 1
            )
            .ok();
        }
        3 => {
            let vx = wrap(&format!("{var_name}.x"));
            let vy = wrap(&format!("{var_name}.y"));
            let vz = wrap(&format!("{var_name}.z"));
            writeln!(
                out,
                "    {buf}[{offset}] = {vx}; {buf}[{}] = {vy}; {buf}[{}] = {vz};",
                offset + 1,
                offset + 2
            )
            .ok();
        }
        4 => {
            let vx = wrap(&format!("{var_name}.x"));
            let vy = wrap(&format!("{var_name}.y"));
            let vz = wrap(&format!("{var_name}.z"));
            let vw = wrap(&format!("{var_name}.w"));
            writeln!(
                out,
                "    {buf}[{offset}] = {vx}; {buf}[{}] = {vy}; {buf}[{}] = {vz}; {buf}[{}] = {vw};",
                offset + 1,
                offset + 2,
                offset + 3
            )
            .ok();
        }
        n => {
            writeln!(
                out,
                "    {{ let b: [f32; {n}] = unsafe {{ std::mem::transmute({var_name}) }};"
            )
            .ok();
            for i in 0..n {
                writeln!(out, "    {buf}[{}] = b[{i}];", offset + i).ok();
            }
            writeln!(out, "    }}").ok();
        }
    }
}

/// Emit uniform buffer unpacking code. `prefix` is either "" or "rcx." to
/// target either local variables or RenderCx fields.
fn write_uniform_unpack(output: &ShaderOutput, vm: &ScriptVm, out: &mut String, prefix: &str) {
    use crate::makepad_script::shader::ShaderIoKind;

    // Uniform buffer structs (pass, draw_list, draw_call)
    for io in &output.io {
        if !matches!(io.kind, ShaderIoKind::UniformBuffer) {
            continue;
        }
        let io_name = output.backend.map_io_name(io.name);
        let buf_idx = io.buffer_index.unwrap_or(0);
        let ty = type_name(output, vm, io.ty);
        writeln!(out, "    if ({buf_idx} as u32) < uniform_count {{").ok();
        writeln!(out, "        let p = *uniform_ptrs.add({buf_idx});").ok();
        writeln!(
            out,
            "        {prefix}unibuf_{io_name} = std::ptr::read(p as *const {ty});"
        )
        .ok();
        writeln!(out, "    }}").ok();
    }

    // Per-draw dynamic uniforms
    {
        let max_buf = output
            .io
            .iter()
            .filter(|io| matches!(io.kind, ShaderIoKind::UniformBuffer))
            .filter_map(|io| io.buffer_index)
            .max();
        let dyn_buf = max_buf.map(|m| m + 1).unwrap_or(3);

        let has_dyn = output
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::Uniform));
        if has_dyn {
            let mut dyn_layout = DrawShaderInputs::new(headless_uniform_packing());
            for io in &output.io {
                if !matches!(io.kind, ShaderIoKind::Uniform) {
                    continue;
                }
                let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
                dyn_layout.push(io.name, slots, DrawShaderAttrFormat::Float);
            }
            dyn_layout.finalize();
            let mut dyn_inputs = dyn_layout.inputs.iter();

            writeln!(out, "    if ({dyn_buf} as u32) < uniform_count {{").ok();
            writeln!(out, "        let dp = *uniform_ptrs.add({dyn_buf});").ok();
            writeln!(
                out,
                "        let dl = *uniform_lens.add({dyn_buf}) as usize;"
            )
            .ok();
            writeln!(
                out,
                "        let dyn_uni = std::slice::from_raw_parts(dp, dl);"
            )
            .ok();
            for io in &output.io {
                if !matches!(io.kind, ShaderIoKind::Uniform) {
                    continue;
                }
                let io_name = output.backend.map_io_name(io.name);
                let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
                let offset = dyn_inputs.next().map(|i| i.offset).unwrap_or(0usize);
                write_static_assign(
                    out,
                    &format!("{prefix}uni_{io_name}"),
                    "dyn_uni",
                    offset,
                    slots,
                );
            }
            writeln!(out, "    }}").ok();
        }
    }

    // Scope uniforms
    {
        let scope_buf = output
            .io
            .iter()
            .filter(|io| matches!(io.kind, ShaderIoKind::UniformBuffer))
            .filter_map(|io| io.buffer_index)
            .max()
            .map(|m| m + 2)
            .unwrap_or(4);

        let has_scope = output
            .io
            .iter()
            .any(|io| matches!(io.kind, ShaderIoKind::ScopeUniform));
        if has_scope {
            let mut scope_layout = DrawShaderInputs::new(headless_uniform_packing());
            for io in &output.io {
                if !matches!(io.kind, ShaderIoKind::ScopeUniform) {
                    continue;
                }
                let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
                scope_layout.push(io.name, slots, DrawShaderAttrFormat::Float);
            }
            scope_layout.finalize();
            let mut scope_inputs = scope_layout.inputs.iter();

            writeln!(out, "    if ({scope_buf} as u32) < uniform_count {{").ok();
            writeln!(out, "        let sp = *uniform_ptrs.add({scope_buf});").ok();
            writeln!(
                out,
                "        let sl = *uniform_lens.add({scope_buf}) as usize;"
            )
            .ok();
            writeln!(
                out,
                "        let scope_uni = std::slice::from_raw_parts(sp, sl);"
            )
            .ok();
            for io in &output.io {
                if !matches!(io.kind, ShaderIoKind::ScopeUniform) {
                    continue;
                }
                let io_name = output.backend.map_io_name(io.name);
                let slots = vm.bx.heap.pod_type_ref(io.ty).ty.slots();
                let offset = scope_inputs.next().map(|i| i.offset).unwrap_or(0usize);
                write_static_assign(
                    out,
                    &format!("{prefix}su_{io_name}"),
                    "scope_uni",
                    offset,
                    slots,
                );
            }
            writeln!(out, "    }}").ok();
        }
    }
}

/// The inline runtime preamble embedded in every generated shader module.
const SHADER_RUNTIME_PREAMBLE: &str = include_str!("shader_runtime_preamble.rs");

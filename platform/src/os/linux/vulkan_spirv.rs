use crate::makepad_script::{
    shader::ShaderOutput,
    shader_spirv::compile_wgsl_to_spirv,
    shader_wgsl::{compile_draw_shader_wgsl_source, wgsl_instance_binding},
    value::ScriptObject,
    vm::ScriptVm,
};

#[derive(Clone)]
pub struct CxVulkanShaderBinary {
    pub vertex_spirv: Option<Vec<u32>>,
    pub fragment_spirv: Option<Vec<u32>>,
    pub dyn_uniform_binding: u32,
    pub texture_binding_base: u32,
    pub sampler_binding_base: u32,
    pub xr_depth_binding: u32,
    /// The storage buffer the vertex stage reads instance records from;
    /// None for a shader without instance fields.
    pub instance_binding: Option<u32>,
    pub geometry_slots: usize,
    pub instance_slots: usize,
}

/// A draw shader as Vulkan runs it: the module the WGSL emitter writes (the
/// one WebGPU runs), compiled to SPIR-V by the shader compiler's own
/// backend (`makepad_script::shader_spirv`).
pub(crate) fn compile_draw_shader_spirv(
    vm: &mut ScriptVm,
    io_self: ScriptObject,
    layout_source: &ShaderOutput,
    xr_multiview: bool,
) -> Result<CxVulkanShaderBinary, String> {
    let source = compile_draw_shader_wgsl_source(vm, io_self, layout_source, xr_multiview)?;

    if crate::makepad_error_log::trace_enabled("shader.wgsl") {
        let variant = if xr_multiview { "xr" } else { "window" };
        crate::trace!("shader.wgsl", "---- Vulkan WGSL ({}) ----\n{}", variant, source.wgsl);
    }

    let (vertex_spirv, fragment_spirv) = compile_wgsl_to_spirv(&source.wgsl)
        .map_err(|err| format!("{err}\nSet MAKEPAD_TRACE=shader.wgsl to dump the shader module."))?;

    Ok(CxVulkanShaderBinary {
        vertex_spirv,
        fragment_spirv,
        dyn_uniform_binding: source.dyn_uniform_binding,
        texture_binding_base: source.texture_binding_base,
        sampler_binding_base: source.sampler_binding_base,
        xr_depth_binding: source.xr_depth_binding,
        instance_binding: (source.instance_slots > 0).then(|| wgsl_instance_binding(source.xr_depth_binding)),
        geometry_slots: source.geometry_slots,
        instance_slots: source.instance_slots,
    })
}

use crate::makepad_script::{
    shader::ShaderOutput,
    shader_ir_draw::{compile_draw_shader_spirv as compile_ir_spirv, IrDrawSpirv},
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

impl CxVulkanShaderBinary {
    fn from_ir(s: IrDrawSpirv) -> Self {
        CxVulkanShaderBinary {
            vertex_spirv: Some(s.vertex),
            fragment_spirv: Some(s.fragment),
            dyn_uniform_binding: s.info.dyn_uniform_binding,
            texture_binding_base: s.info.texture_binding_base,
            sampler_binding_base: s.info.sampler_binding_base,
            xr_depth_binding: s.info.xr_depth_binding,
            instance_binding: s.info.instance_binding,
            geometry_slots: s.info.geometry_slots,
            instance_slots: s.info.instance_slots,
        }
    }
}

/// A draw shader as Vulkan runs it: lowered to the shader IR once and
/// printed as SPIR-V for the window and the XR variant
/// (`makepad_script::shader_ir_spirv`).
pub(crate) fn compile_draw_shader_spirv(
    vm: &mut ScriptVm,
    io_self: ScriptObject,
    layout_source: &ShaderOutput,
) -> Result<[CxVulkanShaderBinary; 2], String> {
    let [window, xr] = compile_ir_spirv(vm, io_self, layout_source)?;
    Ok([CxVulkanShaderBinary::from_ir(window), CxVulkanShaderBinary::from_ir(xr)])
}

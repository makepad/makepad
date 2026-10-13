pub use makepad_shader::shader_ir_wgsl::*;
use crate::{shader_ir_draw::{compile_draw_shader_ir_with, IrCompileOptions}, vm::ScriptVm, value::ScriptObject};
fn compile_draw_shader_wgsl_source_native(vm: &mut ScriptVm, io_self: ScriptObject, xr: bool) -> Result<WgslDrawShaderSource, String> {
    let opts = IrCompileOptions { use_vulkan: true, backend: crate::shader_backend::ShaderBackend::Wgsl, ..Default::default() };
    let output = compile_draw_shader_ir_with(vm, io_self, &opts);
    if output.has_errors {
        return Err(format!("shader IR lowering reported errors:\n{}", output.error_report()));
    }
    let (wgsl, info) = draw_wgsl(&output, vm, xr)?;
    Ok(WgslDrawShaderSource {
        wgsl,
        dyn_uniform_binding: info.dyn_uniform_binding,
        texture_binding_base: info.texture_binding_base,
        sampler_binding_base: info.sampler_binding_base,
        xr_depth_binding: info.xr_depth_binding,
        geometry_slots: info.geometry_slots,
        instance_slots: info.instance_slots,
    })
}

/// Opaque result: owned error strings stay with the compiler that creates them.
pub struct WgslDrawShaderResult(Result<WgslDrawShaderSource, String>);

impl WgslDrawShaderResult {
    pub fn source(&self) -> Option<&WgslDrawShaderSource> { self.0.as_ref().ok() }
    pub fn error(&self) -> Option<&str> { self.0.as_ref().err().map(String::as_str) }
}

pub fn compile_draw_shader_wgsl_result(vm: &mut ScriptVm, io_self: ScriptObject, xr: bool) -> WgslDrawShaderResult {
    WgslDrawShaderResult(compile_draw_shader_wgsl_source_native(vm, io_self, xr))
}

pub fn compile_draw_shader_wgsl_source(vm: &mut ScriptVm, io_self: ScriptObject, xr: bool) -> Result<WgslDrawShaderSource, String> {
    let result = compile_draw_shader_wgsl_result(vm, io_self, xr);
    match result.source() {
        Some(source) => Ok(source.clone()),
        None => Err(result.error().unwrap().to_owned()),
    }
}

//! Splash draw compilation into the shared shader IR.
pub use makepad_shader::shader_ir_draw::*;
use crate::{shader::{ShaderMode, ShaderType}, shader_lower::IrFnCompiler, shader_output::ShaderOutput, trap::NoTrap, value::ScriptObject, vm::ScriptVm};
use makepad_live_id::{id, LiveId};
/// How a draw shader is compiled (the platform's choices).
#[derive(Clone, Default)]
pub struct IrCompileOptions {
    pub const_table: bool,
    pub live_literals: Option<std::sync::Arc<crate::literal::LiveLiterals>>,
    /// The source is for Vulkan (GL keeps its clip z and video samplers).
    pub use_vulkan: bool,
    /// GL source for WebGL 2 (its instance attribute stride limit).
    pub glsl_webgl2: bool,
    /// The text backend the platform maps names with.
    pub backend: crate::shader_backend::ShaderBackend,
}

impl IrCompileOptions {
    pub fn new(backend: crate::shader_backend::ShaderBackend, const_table: bool, use_vulkan: bool, glsl_webgl2: bool, live_literals: bool) -> Self {
        Self { backend, const_table, use_vulkan, glsl_webgl2, live_literals: if live_literals { crate::literal::live() } else { None } }
    }
}

/// Compiles a draw shader's `vertex` and `fragment` to IR: the reflection
/// (io, scope uniforms, table constants, samplers ...) the platform maps
/// the shader by, and the IR every backend prints. Check `has_errors`.
pub fn compile_draw_shader_ir_with(vm: &mut ScriptVm, io_self: ScriptObject, opts: &IrCompileOptions) -> ShaderOutput {
    let mut output = ShaderOutput::default();
    output.backend = opts.backend;
    output.const_table = opts.const_table;
    output.live_literals = opts.live_literals.clone();
    output.use_vulkan = opts.use_vulkan;
    output.glsl_webgl2 = opts.glsl_webgl2;
    output.pre_collect_rust_instance_io(vm, io_self);
    output.pre_collect_shader_io(vm, io_self);
    for (entry, mode) in [(id!(vertex), ShaderMode::Vertex), (id!(fragment), ShaderMode::Fragment)] {
        let fnobj = vm.bx.heap.object_method(io_self, entry.into(), vm.thread().trap.pass()).as_object();
        if let Some(fnobj) = fnobj {
            output.mode = mode;
            IrFnCompiler::compile_shader_def(vm, &mut output, NoTrap, entry, fnobj, ShaderType::IoSelf(io_self), vec![]);
        }
    }
    if output.has_errors {
        return output;
    }
    output.assign_uniform_buffer_indices(&vm.bx.heap, 3);
    output.ir.refresh_struct_names(&vm.bx.heap);
    output.ir.precise = vm.bx.heap.value(io_self, id!(precise_math).into(), NoTrap).as_bool() == Some(true);
    if !output.ir.globals.iter().any(|g| matches!(g.kind, crate::shader_ir::IrGlobalKind::Io(crate::shader_ir::IrIo::Storage(_)))) {
        return output;
    }
    let mut names = Vec::new();
    storage_declarations(vm, io_self, &mut names);
    for (i, name) in names.iter().enumerate() {
        if let Some(g) = output.ir.find_global(crate::shader_ir::IrGlobalKind::Io(crate::shader_ir::IrIo::Storage(false)), *name) {
            output.ir.globals[g as usize].binding = Some(i as u32);
        }
    }
    use crate::shader_ir_compute::{draw_storage_layout, ComputeTarget};
    let target = match opts.backend {
        crate::shader_backend::ShaderBackend::Metal => ComputeTarget::Msl,
        crate::shader_backend::ShaderBackend::Hlsl => ComputeTarget::Hlsl,
        _ => ComputeTarget::Spirv,
    };
    match draw_storage_layout(&output.ir, target) {
        Ok(mut layout) => {
            let base = match target {
                ComputeTarget::Msl => output.io.iter().filter_map(|io| if matches!(io.kind, crate::shader_output::ShaderIoKind::UniformBuffer) { io.buffer_index } else { None }).max().unwrap_or(2) as u32 + 2,
                ComputeTarget::Hlsl => output.io.iter().filter(|io| matches!(io.kind, crate::shader_output::ShaderIoKind::Texture(_))).count() as u32 * 2 + 1,
                ComputeTarget::Spirv => 0,
            };
            for b in &mut layout.buffers { b.index += base; }
            if target == ComputeTarget::Msl && layout.buffers.iter().any(|b| b.index >= 30) {
                output.push_error("Metal draw storage buffers must fit below reserved pick buffer 30".into());
            }
            output.compiled.storage_push_size = if target == ComputeTarget::Spirv { layout.push_size } else { 0 };
            output.compiled.storage_buffers = layout.buffers;
        }
        Err(e) => output.push_error(e),
    }
    output
}

fn storage_declarations(vm: &ScriptVm, object: ScriptObject, names: &mut Vec<LiveId>) {
    if let Some(proto) = vm.bx.heap.proto(object).as_object() { storage_declarations(vm, proto, names); }
    for entry in vm.bx.heap.map_entries_ordered(object) {
        if let (Some(name), Some(value)) = (entry.key.as_id(), entry.value.as_object()) {
            if vm.bx.heap.as_shader_io(value) == Some(crate::mod_shader::SHADER_IO_BUFFER_R) && !names.contains(&name) {
                names.push(name);
            }
        }
    }
}

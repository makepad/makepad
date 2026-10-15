//! Splash compute kernels, compiled to SPIR-V.
//!
//! A kernel is a Splash object with a `main: fn() { ... }` body:
//!
//! ```text
//! let Scale = {
//!     workgroup_size: 64            // plain numbers are compile-time constants
//!     src: shader.buffer(f32)       // read-only device buffer
//!     dst: shader.buffer_rw(f32)    // read-write device buffer
//!     n: shader.uniform(u32)        // a param (push constant)
//!     scale: shader.uniform(f32)
//!     main: fn() {
//!         let i = global_id().x
//!         if i < self.n { self.dst[i] = self.src[i] * self.scale }
//!     }
//! }
//! ```
//!
//! Variants are prototypes: `Scale{workgroup_size: 128}` compiles the same
//! body with another constant. `workgroup_size_y` / `workgroup_size_z`
//! default to 1. Buffers are reached by device address, so the host binds a
//! kernel only through its push constants (`ComputeLayout`).

pub use makepad_shader::shader_ir_compute::*;
use makepad_shader::shader_ir::CooperativeMatrix;
use crate::{
    shader::{ShaderMode, ShaderType},
    shader_lower::IrFnCompiler,
    shader_output::ShaderOutput,
    trap::NoTrap,
    value::ScriptObject,
    vm::ScriptVm,
};
use makepad_live_id::{id, LiveId};
use makepad_relax_api::relax_api;

/// `compile_compute_kernel` as an op (makepad run): the result stays in this crate, JIT code
/// reads it through `view` and builds its own `CompiledKernel` (`CompiledKernelView::to_kernel`).
#[relax_api(handle)]
pub struct CompiledKernelResult {
    result: Result<CompiledKernel, String>,
}

#[relax_api]
pub fn compile_compute_kernel_result(vm: &mut ScriptVm, kernel: ScriptObject) -> CompiledKernelResult {
    CompiledKernelResult { result: compile_compute_kernel(vm, kernel) }
}

#[relax_api]
impl CompiledKernelResult {
    /// The kernel's SPIR-V and layout as plain data, or the error.
    #[relax_api]
    pub fn view(&self) -> CompiledKernelView<'_> {
        match &self.result {
            Ok(k) => CompiledKernelView { error: None, spirv: &k.spirv, slots: &k.layout.slots, cooperative_matrices: &k.layout.cooperative_matrices,
                push_size: k.layout.push_size, workgroup: k.layout.workgroup, shared_bytes: k.layout.shared_bytes },
            Err(e) => CompiledKernelView { error: Some(e), ..Default::default() },
        }
    }
}

/// [`CompiledKernelResult::view`].
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CompiledKernelView<'a> {
    pub error: Option<&'a str>,
    pub spirv: &'a [u32],
    pub slots: &'a [ComputeSlot],
    pub cooperative_matrices: &'a [CooperativeMatrix],
    pub push_size: u32,
    pub workgroup: [u32; 3],
    pub shared_bytes: u32,
}

impl CompiledKernelView<'_> {
    /// The compiled kernel (a copy on the caller's side), or the error.
    pub fn to_kernel(&self) -> Result<CompiledKernel, String> {
        if let Some(error) = self.error {
            return Err(error.to_string());
        }
        Ok(CompiledKernel { spirv: self.spirv.to_vec(), layout: ComputeLayout { slots: self.slots.to_vec(), push_size: self.push_size,
            workgroup: self.workgroup, shared_bytes: self.shared_bytes, cooperative_matrices: self.cooperative_matrices.to_vec() } })
    }
}

/// A compiled kernel: its SPIR-V (entry point `kernel_main`) and the
/// push-constant layout its host fills.
#[derive(Clone, Debug)]
pub struct CompiledKernel {
    pub spirv: Vec<u32>,
    pub layout: ComputeLayout,
}

fn size_field(vm: &ScriptVm, kernel: ScriptObject, name: LiveId) -> Result<u32, String> {
    let v = vm.bx.heap.value(kernel, name.into(), NoTrap);
    if v.is_nil() || v.is_err() {
        return Ok(1);
    }
    match v.as_number() {
        Some(n) if n >= 1.0 && n.fract() == 0.0 && n <= 1024.0 => Ok(n as u32),
        _ => Err(format!("kernel {} must be a whole number in 1..=1024", name)),
    }
}

/// Compiles `kernel` (an object with a `main` fn) to SPIR-V for Vulkan.
pub fn compile_compute_kernel(vm: &mut ScriptVm, kernel: ScriptObject) -> Result<CompiledKernel, String> {
    let workgroup = [
        size_field(vm, kernel, id!(workgroup_size))?,
        size_field(vm, kernel, id!(workgroup_size_y))?,
        size_field(vm, kernel, id!(workgroup_size_z))?,
    ];
    let mut output = ShaderOutput::default();
    output.compiled.use_vulkan = true;
    output.mode = ShaderMode::Compute;
    let main = vm
        .bx
        .heap
        .object_method(kernel, id!(main).into(), vm.thread().trap.pass())
        .as_object()
        .ok_or("a kernel needs a `main: fn() { ... }`")?;
    let (_, main) = IrFnCompiler::compile_shader_def(vm, &mut output, NoTrap, id!(main), main, ShaderType::IoSelf(kernel), vec![]);
    if output.has_errors {
        return Err(output.compiled.errors.join("\n"));
    }
    let main = main.ok_or("the kernel's main fn did not compile")?;
    output.compiled.ir.refresh_struct_names(&vm.bx.heap);
    let ir = &mut output.compiled.ir;
    ir.workgroup = workgroup;
    let entry = build_compute_entry(ir, main);
    let layout = compute_layout(ir)?;
    let spirv = makepad_shader::shader_ir_spirv::ir_to_spirv(ir, entry, false)?;
    Ok(CompiledKernel { spirv, layout })
}

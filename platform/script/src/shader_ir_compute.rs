//! Splash compute kernels, compiled to SPIR-V, MSL or HLSL cs_5_0.
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
//! default to 1. `ComputeLayout` describes the common parameter byte block
//! and every target resource binding; no backend infers offsets from source.

pub use makepad_shader::shader_ir_compute::*;
use makepad_shader::shader_ir::{CooperativeMatrix, FuncId, IrGlobalKind, IrModule, IrTy};
use crate::{
    shader::{ShaderMode, ShaderType},
    shader_lower::IrFnCompiler,
    shader_output::ShaderOutput,
    trap::NoTrap,
    value::ScriptObject,
    vm::ScriptVm,
};
use makepad_live_id::{id, LiveId};
use relax_api::relax_api;

/// `compile_compute_kernel` as an op (makepad run): the result stays in this crate, JIT code
/// reads it through `view` and builds its own `CompiledKernel` (`CompiledKernelView::to_kernel`).
#[relax_api(handle, run)]
pub struct CompiledKernelResult {
    result: Result<CompiledKernel, String>,
}

#[relax_api(run)]
pub fn compile_compute_kernel_result(vm: &mut ScriptVm, kernel: ScriptObject, target: ComputeTarget) -> CompiledKernelResult {
    CompiledKernelResult { result: compile_compute_kernel(vm, kernel, target) }
}

impl CompiledKernelResult {
    /// A result compiled on this side ([`compile_compute_ir`]), for JIT code to `view`.
    pub fn new(result: Result<CompiledKernel, String>) -> CompiledKernelResult {
        CompiledKernelResult { result }
    }
}
#[relax_api(run)]
impl CompiledKernelResult {
    /// Target code and complete binding/layout reflection as plain data, or the error.
    #[relax_api(run)]
    pub fn view(&self) -> CompiledKernelView<'_> {
        match &self.result {
            Ok(k) => CompiledKernelView {
                error: None,
                target: k.layout.target,
                spirv: k.code.as_spirv().unwrap_or(&[]),
                text: k.code.as_text().unwrap_or(""),
                buffers: &k.layout.buffers,
                storage_textures: &k.layout.storage_textures,
                textures: &k.layout.textures,
                samplers: &k.layout.samplers,
                param_binding: k.layout.param_binding,
                param_binding_size: k.layout.param_binding_size,
                group_count_offset: k.layout.group_count_offset,
                precise_math: k.layout.precise_math,
                slots: &k.layout.slots,
                cooperative_matrices: &k.layout.cooperative_matrices,
                push_size: k.layout.push_size,
                workgroup: k.layout.workgroup,
                shared_bytes: k.layout.shared_bytes,
            },
            Err(e) => CompiledKernelView { error: Some(e), ..Default::default() },
        }
    }
}

/// [`CompiledKernelResult::view`].
#[relax_api(plain, run)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CompiledKernelView<'a> {
    pub error: Option<&'a str>,
    pub spirv: &'a [u32],
    pub target: ComputeTarget,
    pub text: &'a str,
    pub buffers: &'a [ComputeBufferBinding],
    pub storage_textures: &'a [StorageTextureBinding],
    pub textures: &'a [SampledTextureBinding],
    pub samplers: &'a [ComputeSampler],
    pub param_binding: Option<u32>,
    pub param_binding_size: u32,
    pub group_count_offset: Option<u32>,
    pub precise_math: bool,
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
        Ok(CompiledKernel {
            code: match self.target {
                ComputeTarget::Spirv => KernelCode::Spirv(self.spirv.to_vec()),
                ComputeTarget::Msl => KernelCode::Msl(self.text.into()),
                ComputeTarget::Hlsl => KernelCode::Hlsl(self.text.into()),
            },
            layout: ComputeLayout {
                target: self.target,
                buffers: self.buffers.to_vec(),
                storage_textures: self.storage_textures.to_vec(),
                textures: self.textures.to_vec(),
                samplers: self.samplers.to_vec(),
                param_binding: self.param_binding,
                param_binding_size: self.param_binding_size,
                group_count_offset: self.group_count_offset,
                precise_math: self.precise_math,
                slots: self.slots.to_vec(),
                push_size: self.push_size,
                workgroup: self.workgroup,
                shared_bytes: self.shared_bytes,
                cooperative_matrices: self.cooperative_matrices.to_vec(),
            },
        })
    }
}

/// A compiled kernel, entry point `kernel_main`, and its complete host ABI.
#[derive(Clone, Debug)]
pub struct CompiledKernel {
    pub code: KernelCode,
    pub layout: ComputeLayout,
}

#[derive(Clone, Debug)]
pub enum KernelCode {
    Spirv(Vec<u32>),
    Msl(String),
    Hlsl(String),
}

impl KernelCode {
    pub fn as_spirv(&self) -> Option<&[u32]> {
        if let Self::Spirv(words) = self { Some(words) } else { None }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self { Self::Msl(text) | Self::Hlsl(text) => Some(text), _ => None }
    }
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

/// The samplers a kernel's texture samples use, in `_s<i>` order.
fn compute_samplers(samplers: &[makepad_shader::shader_output::ShaderSampler]) -> Result<Vec<ComputeSampler>, String> {
    use makepad_shader::shader_output::{SamplerAddress, SamplerCoord, SamplerFilter};
    samplers.iter().map(|s| {
        if s.compare || s.is_video || s.coord != SamplerCoord::Normalized {
            return Err("a kernel samples textures with plain samplers only (no depth compare, video or pixel coordinates)".to_string());
        }
        let filter = match s.filter { SamplerFilter::Nearest => 0, SamplerFilter::Linear => 1 };
        let address = match s.address { SamplerAddress::Repeat => 0, SamplerAddress::ClampToEdge => 1, SamplerAddress::ClampToZero => 2, SamplerAddress::MirroredRepeat => 3 };
        Ok(ComputeSampler { filter, address })
    }).collect()
}

/// Compiles `kernel` (an object with a `main` fn) to the explicit target.
pub fn compile_compute_kernel(vm: &mut ScriptVm, kernel: ScriptObject, target: ComputeTarget) -> Result<CompiledKernel, String> {
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
    ir.precise = vm.bx.heap.value(kernel, id!(precise_math).into(), NoTrap).as_bool().unwrap_or(false);
    let samplers = compute_samplers(&output.samplers)?;
    compile_compute_ir_sampled(&mut output.compiled.ir, main, target, samplers)
}

/// Compiles a kernel already in shader IR (`main`: its body, no params, void; the module's
/// workgroup and precision set) to the explicit target.
pub fn compile_compute_ir(ir: &mut IrModule, main: FuncId, target: ComputeTarget) -> Result<CompiledKernel, String> {
    compile_compute_ir_sampled(ir, main, target, Vec::new())
}

/// [`compile_compute_ir`] for a kernel whose texture samples use `samplers` (`_s<i>` order).
fn compile_compute_ir_sampled(ir: &mut IrModule, main: FuncId, target: ComputeTarget, samplers: Vec<ComputeSampler>) -> Result<CompiledKernel, String> {
    let entry = build_compute_entry(ir, main);
    if target == ComputeTarget::Spirv {
        // SPIR-V binds the samples' samplers as globals (the text targets name `_s<i>` in the text)
        let ty = ir.ty(IrTy::Sampler(false));
        for i in 0..samplers.len() as u32 { ir.global(IrGlobalKind::Sampler(i), LiveId(i as u64), ty); }
    }
    let mut layout = compute_target_layout(ir, target)?;
    layout.samplers = samplers;
    if target == ComputeTarget::Hlsl {
        let read = layout.buffers.iter().filter(|b| !b.writable).count() + layout.textures.len();
        if read > 128 { return Err(format!("D3D11 has 128 shader-resource slots; the kernel reads {read} buffers and textures")); }
        if layout.samplers.len() > 16 { return Err(format!("D3D11 has 16 sampler slots; the kernel samples with {}", layout.samplers.len())); }
    }
    makepad_shader::shader_ir_precise::lower_precise(ir, target != ComputeTarget::Msl);
    let code = match target {
        ComputeTarget::Spirv => KernelCode::Spirv(makepad_shader::shader_ir_spirv::ir_to_spirv(ir, entry, false)?),
        ComputeTarget::Msl => KernelCode::Msl(makepad_shader::shader_ir_compute_text::compute_text(ir, entry, &layout)?),
        ComputeTarget::Hlsl => KernelCode::Hlsl(makepad_shader::shader_ir_compute_text::compute_text(ir, entry, &layout)?),
    };
    Ok(CompiledKernel { code, layout })
}

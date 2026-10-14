//! The kernel library: the Splash kernel documents in `kernels/`, evaluated
//! once by a Splash VM and compiled to pipelines on first use.
//!
//! Each document evaluates to an object whose fields are kernels (objects
//! with a `main` fn); a variant is a prototype with other constants
//! (`gemv_q6k_wide: gemv_q6k{rows_per_group: 4}`).

use crate::device::{Device, Result};
use crate::kernel::Kernel;
use makepad_script::shader_ir_compute::compile_compute_kernel_result;
use makepad_script::*;
use std::collections::HashMap;
use std::sync::Arc;

/// The kernel documents compiled into the backend.
pub const KERNEL_SOURCES: &[(&str, &str)] = &[
    ("kernels/elementwise.splash", include_str!("../kernels/elementwise.splash")),
    ("kernels/llm.splash", include_str!("../kernels/llm.splash")),
    ("kernels/whisper.splash", include_str!("../kernels/whisper.splash")),
    ("kernels/attention_d64.splash", include_str!("../kernels/attention_d64.splash")),
    ("kernels/kokoro.splash", include_str!("../kernels/kokoro.splash")),
    ("kernels/vision.splash", include_str!("../kernels/vision.splash")),
    ("kernels/gemm.splash", include_str!("../kernels/gemm.splash")),
    ("kernels/gemm_phone.splash", include_str!("../kernels/gemm_phone.splash")),
    ("kernels/gemm_int8.splash", include_str!("../kernels/gemm_int8.splash")),
];

pub struct Library {
    vm: ScriptVm<'static>,
    kernels: HashMap<String, ScriptObject>,
    built: HashMap<(String, Vec<(String, u32)>, Option<u32>), Arc<Kernel>>,
}

impl Library {
    /// Evaluates `sources` (file name, Splash text).
    pub fn new(sources: &[(&str, &str)]) -> Result<Library> {
        let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        vm.capture_errors();
        let mut kernels = HashMap::new();
        for (file, text) in sources {
            let code = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{text}");
            let value = vm.with_instruction_limit(50_000_000, |vm| vm.eval(ScriptMod::new("", "", file, 0, 0, &code, &[])));
            let errors = vm.take_errors();
            if !errors.is_empty() {
                return Err(format!("{file}: {errors:?}"));
            }
            let obj = value.as_object().ok_or_else(|| format!("{file} must evaluate to an object of kernels"))?;
            for entry in vm.heap().map_entries_ordered(obj) {
                if let (Some(name), Some(k)) = (entry.key.as_id(), entry.value.as_object()) {
                    kernels.insert(format!("{}", name), k);
                }
            }
        }
        Ok(Library { vm, kernels, built: HashMap::new() })
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.kernels.keys().cloned().collect();
        v.sort();
        v
    }

    /// The SPIR-V and layout of kernel `name` (no device needed).
    pub fn compile(&mut self, name: &str) -> Result<makepad_script::shader_ir_compute::CompiledKernel> {
        self.compile_constants(name, &[])
    }

    /// Specializes a file-based kernel through a prototype's number fields.
    /// No source text is constructed and the library's base stays reusable.
    pub fn compile_constants(&mut self, name: &str, constants: &[(&str, u32)]) -> Result<makepad_script::shader_ir_compute::CompiledKernel> {
        let obj = *self.kernels.get(name).ok_or_else(|| format!("no kernel `{name}`"))?;
        let obj = if constants.is_empty() { obj } else {
            let variant = self.vm.heap_mut().new_with_proto(obj.into());
            for (field, value) in constants {
                self.vm.heap_mut().set_value(variant, LiveId::from_str(field).into(), (*value as f64).into(), NoTrap);
            }
            variant
        };
        let r = compile_compute_kernel_result(&mut self.vm, obj).view().to_kernel();
        let errors = self.vm.take_errors();
        let k = r.map_err(|e| format!("kernel {name}: {e}"))?;
        if !errors.is_empty() {
            return Err(format!("kernel {name}: {errors:?}"));
        }
        Ok(k)
    }

    /// The pipeline of kernel `name`, built on first use.
    pub fn get(&mut self, device: &Device, name: &str) -> Result<Arc<Kernel>> {
        self.get_constants(device, name, &[])
    }

    /// Retains specialized pipelines for recordings that outlive a prepared
    /// operation. Shapes sharing the same constants reuse the same pipeline.
    pub fn get_constants(&mut self, device: &Device, name: &str, constants: &[(&str, u32)]) -> Result<Arc<Kernel>> {
        self.get_constants_subgroup(device, name, constants, None)
    }

    /// The required subgroup is part of the cache key: a default pipeline
    /// must never satisfy a request for a specific full-subgroup size.
    pub fn get_constants_subgroup(&mut self, device: &Device, name: &str, constants: &[(&str, u32)], subgroup: Option<u32>) -> Result<Arc<Kernel>> {
        let key = (name.to_string(), constants.iter().map(|(k, v)| (k.to_string(), *v)).collect(), subgroup);
        if let Some(k) = self.built.get(&key) {
            return Ok(k.clone());
        }
        let compiled = self.compile_constants(name, constants)?;
        let k = Arc::new(device.kernel(name, &compiled, subgroup)?);
        self.built.insert(key, k.clone());
        Ok(k)
    }
}

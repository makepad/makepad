//! Kernels: Splash compute functions compiled to SPIR-V by our shader
//! compiler (`makepad_script::shader_ir_compute`), each a compute pipeline
//! bound through its push constants.

use crate::device::{vk_err, Device, Result, PUSH_CONSTANT_BYTES};
use makepad_script::makepad_live_id::LiveId;
use makepad_script::shader_ir::IrScalar;
use makepad_script::shader_ir_compute::{CompiledKernel, ComputeLayout, ComputeSlotKind};
use makepad_vulkan::vk;

pub struct Kernel {
    pub name: String,
    pub(crate) pipeline: vk::Pipeline,
    pub layout: ComputeLayout,
    device: makepad_vulkan::Device,
}

impl Drop for Kernel {
    fn drop(&mut self) {
        unsafe { self.device.destroy_pipeline(self.pipeline, None) };
    }
}

impl Kernel {
    pub fn workgroup(&self) -> [u32; 3] {
        self.layout.workgroup
    }

    /// The push-constant block for one dispatch, filled by name.
    pub fn args(&self) -> Args<'_> {
        Args { kernel: self, bytes: [0; PUSH_CONSTANT_BYTES as usize] }
    }
}

impl Device {
    /// A compute pipeline for `compiled`, optionally running with a
    /// required subgroup size (full subgroups).
    pub fn kernel(&self, name: &str, compiled: &CompiledKernel, subgroup_size: Option<u32>) -> Result<Kernel> {
        if !compiled.layout.cooperative_matrices.is_empty() && !self.features.cooperative_matrix {
            return Err(format!("kernel {name} requires cooperative matrices with the Vulkan memory model"));
        }
        if compiled.layout.shared_bytes > self.features.max_shared_bytes {
            return Err(format!("kernel {name} needs {} shared bytes; the device supports {}", compiled.layout.shared_bytes, self.features.max_shared_bytes));
        }
        unsafe {
            let module = self
                .device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&compiled.spirv), None)
                .map_err(vk_err("vkCreateShaderModule"))?;
            let mut required = vk::PipelineShaderStageRequiredSubgroupSizeCreateInfo::default();
            let mut stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"kernel_main");
            if let Some(size) = subgroup_size.filter(|_| self.features.subgroup_size_control) {
                required.required_subgroup_size = size;
                stage = stage
                    .flags(vk::PipelineShaderStageCreateFlags::REQUIRE_FULL_SUBGROUPS)
                    .next_pipeline_shader_stage_required_subgroup_size_create_info(&mut required);
            }
            let info = vk::ComputePipelineCreateInfo::default().stage(stage).layout(self.pipeline_layout);
            let pipelines = self.device.create_compute_pipelines(vk::PipelineCache::null(), &[info], None);
            self.device.destroy_shader_module(module, None);
            let pipeline = pipelines.map_err(|e| format!("kernel {name}: vkCreateComputePipelines: {e:?}"))?[0];
            Ok(Kernel { name: name.to_string(), pipeline, layout: compiled.layout.clone(), device: self.device.clone() })
        }
    }
}

/// One dispatch's push constants.
pub struct Args<'a> {
    kernel: &'a Kernel,
    pub(crate) bytes: [u8; PUSH_CONSTANT_BYTES as usize],
}

impl<'a> Args<'a> {
    /// The slot of `name`; None when the kernel never reads it (the
    /// compiler drops unused params, so a variant may lack one).
    fn slot(&self, name: &str) -> Option<makepad_script::shader_ir_compute::ComputeSlot> {
        self.kernel.layout.slot(LiveId::from_str(name)).cloned()
    }

    /// A buffer's device address.
    pub fn buffer(mut self, name: &str, address: u64) -> Self {
        if let Some(s) = self.slot(name) {
            assert!(matches!(s.kind, ComputeSlotKind::Buffer { .. }), "{name} is not a buffer of {}", self.kernel.name);
            let at = s.offset as usize;
            self.bytes[at..at + 8].copy_from_slice(&address.to_le_bytes());
        }
        self
    }

    pub fn u32(mut self, name: &str, v: u32) -> Self {
        if let Some(s) = self.slot(name) {
            assert!(matches!(s.kind, ComputeSlotKind::Param { scalar: IrScalar::U32 | IrScalar::I32, lanes: 1 }), "{name} of {} is not a u32", self.kernel.name);
            let at = s.offset as usize;
            self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        self
    }

    pub fn i32(self, name: &str, v: i32) -> Self {
        self.u32(name, v as u32)
    }

    pub fn f32(mut self, name: &str, v: f32) -> Self {
        if let Some(s) = self.slot(name) {
            assert!(matches!(s.kind, ComputeSlotKind::Param { scalar: IrScalar::F32, lanes: 1 }), "{name} of {} is not an f32", self.kernel.name);
            let at = s.offset as usize;
            self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        self
    }

    pub fn kernel(&self) -> &'a Kernel {
        self.kernel
    }
}

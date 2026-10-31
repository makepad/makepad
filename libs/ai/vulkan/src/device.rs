//! The compute device: one Vulkan instance, device and compute queue, with
//! the features the kernels rely on (device addresses, f16/int8 math,
//! 16-bit storage, subgroup size control, integer dot products) enabled
//! where the device has them and reported in `DeviceFeatures`.

use makepad_vulkan::vk;
use std::ffi::CStr;
use std::sync::{Arc, Mutex};

pub type Result<T> = std::result::Result<T, String>;

pub(crate) fn vk_err(what: &str) -> impl Fn(vk::Result) -> String + '_ {
    move |e| format!("{what}: {e:?}")
}

/// An exact multiply-add shape reported by the driver, without pNext pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CooperativeMatrixShape {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub a: vk::ComponentTypeKHR,
    pub b: vk::ComponentTypeKHR,
    pub c: vk::ComponentTypeKHR,
    pub result: vk::ComponentTypeKHR,
    pub scope: vk::ScopeKHR,
    pub saturating: bool,
}

/// What the bound device can do, reported truthfully from the driver.
#[derive(Clone, Debug)]
pub struct DeviceFeatures {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub discrete: bool,
    pub api_version: (u32, u32, u32),
    pub driver_version: u32,
    /// The subgroup size compute dispatches get by default, and the range
    /// `subgroup_size_control` can require.
    pub subgroup_size: u32,
    pub min_subgroup_size: u32,
    pub max_subgroup_size: u32,
    pub subgroup_size_control: bool,
    pub shader_f16: bool,
    pub shader_int8: bool,
    pub storage_16bit: bool,
    pub storage_8bit: bool,
    /// `dot4_u8` is accelerated (packed 4x8 unsigned integer dot product).
    pub dot4_u8_accelerated: bool,
    pub cooperative_matrix: bool,
    pub cooperative_matrix_shapes: Vec<CooperativeMatrixShape>,
    pub max_shared_bytes: u32,
    pub max_workgroup_invocations: u32,
    pub max_workgroup_size: [u32; 3],
    pub max_push_constant_bytes: u32,
    pub max_storage_buffer_range: u32,
    pub max_allocation_bytes: u64,
    /// Device memory the CPU can map directly (one heap on phones).
    pub unified_memory: bool,
    pub device_heap_bytes: u64,
    pub timestamp_period_ns: f32,
}

impl DeviceFeatures {
    pub fn description(&self) -> String {
        format!(
            "vulkan:{} {}.{}.{} subgroup {} ({}..{}) f16 {} int8 {} dot4 {} coopmat {} shared {}KB {} {:.1}GiB",
            self.name,
            self.api_version.0,
            self.api_version.1,
            self.api_version.2,
            self.subgroup_size,
            self.min_subgroup_size,
            self.max_subgroup_size,
            self.shader_f16,
            self.shader_int8,
            self.dot4_u8_accelerated,
            self.cooperative_matrix,
            self.max_shared_bytes / 1024,
            if self.unified_memory { "unified" } else { "discrete" },
            self.device_heap_bytes as f64 / (1u64 << 30) as f64,
        )
    }
}

/// One compute device. Models must obtain it through [`Device::shared`].
/// Everything the backend creates must be dropped before its device reference.
pub struct Device {
    _entry: makepad_vulkan::Entry,
    pub(crate) instance: makepad_vulkan::Instance,
    pub(crate) device: makepad_vulkan::Device,
    pub(crate) queue: vk::Queue,
    pub(crate) queue_lock: Arc<Mutex<()>>,
    pub(crate) queue_family: u32,
    pub(crate) memory: vk::PhysicalDeviceMemoryProperties,
    pub(crate) pipeline_layout: vk::PipelineLayout,
    pub features: DeviceFeatures,
}

// SAFETY: The instance/device handles, physical-device properties and pipeline
// layout are immutable after creation. Vulkan synchronizes memory allocation
// and buffer/pipeline creation internally; each allocation is owned by its
// buffer, and pipelines are created without a shared pipeline cache. There are
// no descriptor pools. Command pools/buffers, query pools and fences belong to
// individual streams, whose recording/submission methods require &mut self.
// All queue access (including device_wait_idle) holds queue_lock. The loader
// and its immutable command tables stay alive with the device.
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

/// The push-constant block every kernel declares (Vulkan's guaranteed
/// minimum; `shader_ir_compute::compute_layout` refuses more).
pub const PUSH_CONSTANT_BYTES: u32 = 128;

fn api_triplet(v: u32) -> (u32, u32, u32) {
    (v >> 22, (v >> 12) & 0x3ff, v & 0xfff)
}

impl Device {
    /// The process-wide compute device, created on the first successful call.
    /// Models must use this instead of `new`: the static reference is never
    /// dropped, so model/worker teardown cannot destroy the Vulkan instance or
    /// device (repeated destruction leaks resources in some drivers).
    /// Initialization failures are not cached; a later call may retry.
    pub fn shared() -> Result<Arc<Device>> {
        static SHARED: Mutex<Option<Arc<Device>>> = Mutex::new(None);
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(device) = shared.as_ref() {
            return Ok(Arc::clone(device));
        }
        let device = Arc::new(Self::new()?);
        *shared = Some(Arc::clone(&device));
        Ok(device)
    }

    /// Binds the best compute device (a discrete GPU first), or fails with
    /// the reason (no loader, no Vulkan 1.2 device, no device addresses).
    /// Only for this crate's tests that override device feature limits.
    /// Models must use [`Device::shared`] instead.
    #[doc(hidden)]
    pub fn new() -> Result<Device> {
        unsafe { Self::create() }
    }

    unsafe fn create() -> Result<Device> {
        let entry = makepad_vulkan::Entry::load()?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
        let validation = std::env::var("MAKEPAD_VULKAN_VALIDATION").is_ok();
        let layer_names = if validation { vec![c"VK_LAYER_KHRONOS_validation".as_ptr()] } else { vec![] };
        let info = vk::InstanceCreateInfo::default().application_info(&app).enabled_layer_names(&layer_names);
        let instance = entry.create_instance(&info, None).map_err(vk_err("vkCreateInstance"))?;
        // shared() permits retries, so failed discovery must release the
        // partial instance instead of leaking it on every attempt.
        match Self::create_device(entry, instance.clone()) {
            Ok(device) => Ok(device),
            Err(e) => {
                instance.destroy_instance(None);
                Err(e)
            }
        }
    }

    unsafe fn create_device(entry: makepad_vulkan::Entry, instance: makepad_vulkan::Instance) -> Result<Device> {
        let devices = instance.enumerate_physical_devices().map_err(vk_err("vkEnumeratePhysicalDevices"))?;
        let mut best: Option<(vk::PhysicalDevice, u32)> = None;
        for d in &devices {
            let p = instance.get_physical_device_properties(*d);
            if p.api_version < vk::API_VERSION_1_2 {
                continue;
            }
            let score = match p.device_type {
                vk::PhysicalDeviceType::DISCRETE_GPU => 3,
                vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
                _ => 1,
            };
            if best.map_or(true, |(_, s)| score > s) {
                best = Some((*d, score));
            }
        }
        let physical = best.ok_or("no Vulkan 1.2 device")?.0;
        let props = instance.get_physical_device_properties(physical);
        let family = instance
            .get_physical_device_queue_family_properties(physical)
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::COMPUTE))
            .ok_or("no compute queue")? as u32;

        let extensions = instance
            .enumerate_device_extension_properties(physical)
            .map_err(vk_err("vkEnumerateDeviceExtensionProperties"))?;
        let has_coopmat = extensions.iter().any(|e| CStr::from_ptr(e.extension_name.as_ptr()) == c"VK_KHR_cooperative_matrix");
        // What the device has. Extension presence alone does not imply
        // either the feature bit or compute-stage support.
        let mut has_coop = vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default();
        let mut has11 = vk::PhysicalDeviceVulkan11Features::default();
        let mut has12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut has13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut has2 = vk::PhysicalDeviceFeatures2::default()
            .next_physical_device_vulkan11_features(&mut has11)
            .next_physical_device_vulkan12_features(&mut has12)
            .next_physical_device_vulkan13_features(&mut has13);
        if has_coopmat {
            has2 = has2.next_physical_device_cooperative_matrix_features_khr(&mut has_coop);
        }
        instance.get_physical_device_features2(physical, &mut has2);
        let base = has2.features;
        if has12.buffer_device_address != vk::TRUE {
            return Err("the device lacks buffer device addresses".into());
        }
        let mut p11 = vk::PhysicalDeviceVulkan11Properties::default();
        let mut p13 = vk::PhysicalDeviceVulkan13Properties::default();
        let mut pcoop = vk::PhysicalDeviceCooperativeMatrixPropertiesKHR::default();
        let mut p2 = vk::PhysicalDeviceProperties2::default()
            .next_physical_device_vulkan11_properties(&mut p11)
            .next_physical_device_vulkan13_properties(&mut p13);
        if has_coopmat {
            p2 = p2.next_physical_device_cooperative_matrix_properties_khr(&mut pcoop);
        }
        instance.get_physical_device_properties2(physical, &mut p2);
        let v13 = props.api_version >= vk::API_VERSION_1_3;
        let coopmat = has_coopmat && has_coop.cooperative_matrix == vk::TRUE
            && has12.vulkan_memory_model == vk::TRUE
            && pcoop.cooperative_matrix_supported_stages.contains(vk::ShaderStageFlags::COMPUTE);
        let cooperative_matrix_shapes = if coopmat {
            instance.get_physical_device_cooperative_matrix_properties(physical)
                .map_err(vk_err("vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR"))?
                .into_iter().map(|s| CooperativeMatrixShape {
                    m: s.m_size, n: s.n_size, k: s.k_size,
                    a: s.a_type, b: s.b_type, c: s.c_type, result: s.result_type,
                    scope: s.scope, saturating: s.saturating_accumulation == vk::TRUE,
                }).collect()
        } else { Vec::new() };

        // What we enable: everything the kernels can use that is there.
        let mut en11 = vk::PhysicalDeviceVulkan11Features::default();
        en11.storage_buffer16_bit_access = has11.storage_buffer16_bit_access;
        let mut en12 = vk::PhysicalDeviceVulkan12Features::default();
        en12.buffer_device_address = vk::TRUE;
        en12.shader_float16 = has12.shader_float16;
        en12.shader_int8 = has12.shader_int8;
        en12.storage_buffer8_bit_access = has12.storage_buffer8_bit_access;
        en12.timeline_semaphore = has12.timeline_semaphore;
        en12.host_query_reset = has12.host_query_reset;
        en12.vulkan_memory_model = has12.vulkan_memory_model;
        en12.vulkan_memory_model_device_scope = has12.vulkan_memory_model_device_scope;
        en12.scalar_block_layout = has12.scalar_block_layout;
        let mut en13 = vk::PhysicalDeviceVulkan13Features::default();
        if v13 {
            en13.subgroup_size_control = has13.subgroup_size_control;
            en13.compute_full_subgroups = has13.compute_full_subgroups;
            en13.shader_integer_dot_product = has13.shader_integer_dot_product;
            en13.maintenance4 = has13.maintenance4;
            en13.synchronization2 = has13.synchronization2;
        }
        let mut basics = vk::PhysicalDeviceFeatures::default();
        basics.shader_int64 = base.shader_int64;
        basics.shader_int16 = base.shader_int16;
        let mut en2 = vk::PhysicalDeviceFeatures2::default()
            .features(basics)
            .next_physical_device_vulkan11_features(&mut en11)
            .next_physical_device_vulkan12_features(&mut en12);
        if v13 {
            en2 = en2.next_physical_device_vulkan13_features(&mut en13);
        }
        let mut ext_names = Vec::new();
        if coopmat {
            ext_names.push(c"VK_KHR_cooperative_matrix".as_ptr());
        }
        let mut coop_features = vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default();
        coop_features.cooperative_matrix = vk::TRUE;
        if coopmat {
            en2 = en2.next_physical_device_cooperative_matrix_features_khr(&mut coop_features);
        }
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
        let dinfo = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&ext_names)
            .next_physical_device_features2(&mut en2);
        let device = instance.create_device(physical, &dinfo, None).map_err(vk_err("vkCreateDevice"))?;
        let queue = device.get_device_queue(family, 0);
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(PUSH_CONSTANT_BYTES)];
        let pipeline_layout = match device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&ranges), None,
        ) {
            Ok(layout) => layout,
            Err(e) => {
                device.destroy_device(None);
                return Err(vk_err("vkCreatePipelineLayout")(e));
            }
        };
        let memory = instance.get_physical_device_memory_properties(physical);
        let mut device_heap_bytes = 0u64;
        let mut unified = false;
        for i in 0..memory.memory_type_count as usize {
            let t = memory.memory_types[i];
            let device_local = t.property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL);
            let host = t.property_flags.contains(vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT);
            if device_local {
                device_heap_bytes = device_heap_bytes.max(memory.memory_heaps[t.heap_index as usize].size);
                // Host-visible device memory as big as the device heap
                // (a phone's one heap), not a 256 MB BAR window.
                let heap = memory.memory_heaps[t.heap_index as usize].size;
                if host && heap >= (1u64 << 31) {
                    unified = true;
                }
            }
        }
        let features = DeviceFeatures {
            name: CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy().into_owned(),
            vendor_id: props.vendor_id,
            device_id: props.device_id,
            discrete: props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU,
            api_version: api_triplet(props.api_version),
            driver_version: props.driver_version,
            subgroup_size: p11.subgroup_size,
            min_subgroup_size: if v13 { p13.min_subgroup_size } else { p11.subgroup_size },
            max_subgroup_size: if v13 { p13.max_subgroup_size } else { p11.subgroup_size },
            subgroup_size_control: en13.subgroup_size_control == vk::TRUE && en13.compute_full_subgroups == vk::TRUE,
            shader_f16: en12.shader_float16 == vk::TRUE,
            shader_int8: en12.shader_int8 == vk::TRUE,
            storage_16bit: en11.storage_buffer16_bit_access == vk::TRUE,
            storage_8bit: en12.storage_buffer8_bit_access == vk::TRUE,
            dot4_u8_accelerated: en13.shader_integer_dot_product == vk::TRUE
                && p13.integer_dot_product4x8_bit_packed_unsigned_accelerated == vk::TRUE,
            cooperative_matrix: coopmat,
            cooperative_matrix_shapes,
            max_shared_bytes: props.limits.max_compute_shared_memory_size,
            max_workgroup_invocations: props.limits.max_compute_work_group_invocations,
            max_workgroup_size: props.limits.max_compute_work_group_size,
            max_push_constant_bytes: props.limits.max_push_constants_size,
            max_storage_buffer_range: props.limits.max_storage_buffer_range,
            max_allocation_bytes: p11.max_memory_allocation_size,
            unified_memory: unified,
            device_heap_bytes,
            timestamp_period_ns: props.limits.timestamp_period,
        };
        Ok(Device {
            _entry: entry,
            instance,
            device,
            queue,
            queue_lock: Arc::new(Mutex::new(())),
            queue_family: family,
            memory,
            pipeline_layout,
            features,
        })
    }

    /// The memory type for `bits` with `flags`, if any.
    pub(crate) fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        (0..self.memory.memory_type_count).find(|i| {
            bits & (1 << i) != 0 && self.memory.memory_types[*i as usize].property_flags.contains(flags)
        })
    }

    pub fn raw(&self) -> &makepad_vulkan::Device {
        &self.device
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _queue = self.queue_lock.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

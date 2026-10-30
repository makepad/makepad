//! Device creation and long-lived driver workers. Binding calls follow os/linux/vulkan.rs
//! and vulkan_linux.rs; no Cx objects or platform window handles enter this module.

use super::super::log_enabled;
use makepad_vulkan::{vk, Entry};
use std::collections::{HashSet, VecDeque};
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn error(e: vk::Result) -> String { format!("Vulkan: {e:?}") }

pub struct VideoQueue {
    pub family: u32,
    pub queue: vk::Queue,
    /// Its queue family's timestamp bits (0: no timestamps on it).
    pub timestamp_bits: u32,
    /// The family's further queues: one per further encode engine (a 4090 has two engines and
    /// exposes two queues), so tiles of a picture encode at once.
    pub more: Vec<(vk::Queue, Mutex<()>)>,
    pub lock: Mutex<()>,
    /// The encoder also takes H.265 (VK_KHR_video_encode_h265 enabled).
    pub h265: bool,
    /// The encoder takes a QP delta per region of a picture (VK_KHR_video_encode_quantization_map
    /// and its feature enabled): foveated encode.
    pub qp_map: bool,
}

pub struct Device {
    pub instance: makepad_vulkan::Instance,
    pub raw: makepad_vulkan::Device,
    pub physical: vk::PhysicalDevice,
    pub queue: vk::Queue,
    /// vkQueueSubmit/Present/WaitIdle need external synchronisation, and every Hal's submit worker
    /// (and swapchain retirement) drives this one queue.
    pub queue_lock: Mutex<()>,
    pub family: u32,
    pub properties: vk::PhysicalDeviceProperties,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    pub features: vk::PhysicalDeviceFeatures,
    pub formats: [vk::FormatProperties; crate::types::format::COUNT as usize],
    pub timestamp_bits: u32,
    /// The device's timestamps can be sampled together with the host clock
    /// (VK_KHR/EXT_calibrated_timestamps with the device and CLOCK_MONOTONIC domains): the GPU
    /// profiler places its pass times on the HAL clock with them ([`Device::calibrate`]).
    pub calibrated: bool,
    pub extensions: HashSet<String>,
    pub present_wait: bool,
    /// Multiview render passes (Vulkan 1.1 `multiview`, enabled when the device has it).
    pub multiview: bool,
    /// Fragment density maps (VK_EXT_fragment_density_map: a target's coarser shading where
    /// its map says, an XR runtime's foveation), enabled when the device has them.
    pub fdm: bool,
    /// The sample counts an MSAA pass may use (`HalInfo::msaa`: bit n = 2^n samples per pixel;
    /// 1: none but single sampling): the framebuffer colour and depth counts, up to 8, when the
    /// device resolves depth (VK_KHR_create_renderpass2 + VK_KHR_depth_stencil_resolve).
    pub msaa: u32,
    /// The device's pipeline cache, primed from its file of the last run (the driver checks the
    /// header: another device's or driver's data is ignored), so a relaunch does not compile
    /// every pipeline variant from SPIR-V again (`save_pipeline_cache` writes it back).
    pub pipeline_cache: vk::PipelineCache,
    pipeline_cache_file: Option<PathBuf>,
    /// Pipelines made since the cache was last saved, and when the last one was (clock_ns).
    pub pipelines_made: AtomicU32,
    pub pipeline_made_ns: AtomicU64,
    /// The queue of the GPU's hardware video encoder (VK_KHR_video_encode_queue + H.264), when
    /// it has one: `video_encode::Stream` submits there. Its own lock: video submits never wait
    /// for the graphics queue's.
    pub video_encode: Option<VideoQueue>,
    /// The instance, destroyed after the last device made on it (display peers and a device
    /// that replaced a lost one share it).
    life: Arc<InstanceLife>,
    pub allocated: AtomicU64,
    /// The driver reported VK_ERROR_DEVICE_LOST (or a simulated loss): the backend replaces
    /// the device. Nothing else marks a device lost; running out of memory never does.
    pub lost: AtomicBool,
    /// A submission or fence wait ran out of memory and waits for room: the backend raises
    /// GPU pressure at once (the host asks the AI hub to yield) instead of after a sustained
    /// shortage.
    pub starved: AtomicBool,
    /// An out-of-memory answer at the open-file limit was reported (`refused`); cleared by the
    /// next refusal below the limit, so each episode logs once.
    files_reported: AtomicBool,
    /// The last device budget read (`memory_budget`) and when.
    budget: Mutex<Option<(std::time::Instant, MemoryBudget)>>,
    /// Sampler Y'CbCr conversion (core 1.1 `samplerYcbcrConversion`, enabled when the device has
    /// it): a decoder's external-format pictures can be sampled.
    pub ycbcr: bool,
    /// The samplers of video textures (`Device::video_sampler`), by external format (0: an
    /// ordinary image's), destroyed with the device.
    video_samplers: Mutex<std::collections::HashMap<u64, VideoSampler>>,
}

/// The immutable sampler of a video texture slot: a linear clamp-to-edge one for an ordinary
/// image, or one through the Y'CbCr conversion of a decoder's external format (sampling then
/// gives the raw codes: .g = Y', .b = Cb, .r = Cr).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct VideoSampler {
    pub conversion: vk::SamplerYcbcrConversion,
    pub sampler: vk::Sampler,
}

/// The device-local heaps as the driver accounts them (VK_EXT_memory_budget): `budget` is what
/// this process may use with every other process's usage taken into account, `usage` what it
/// uses, `size` the heaps' total. Free for the process = `budget - usage`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBudget {
    pub budget: u64,
    pub usage: u64,
    pub size: u64,
}

impl MemoryBudget {
    pub fn free(&self) -> u64 { self.budget.saturating_sub(self.usage) }
}

/// The device-local heaps' free bytes for this process and their size (VK_EXT_memory_budget),
/// read before a device exists.
fn device_local_free(instance: &makepad_vulkan::Instance, physical: vk::PhysicalDevice) -> Option<(u64, u64)> {
    let mut heaps = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
    let mut props = vk::PhysicalDeviceMemoryProperties2::default().next_physical_device_memory_budget_properties_ext(&mut heaps);
    unsafe { instance.get_physical_device_memory_properties2(physical, &mut props); }
    let m = props.memory_properties;
    let (mut free, mut size) = (0u64, 0u64);
    for i in 0..(m.memory_heap_count as usize).min(m.memory_heaps.len()) {
        if m.memory_heaps[i].flags.as_raw() & 1 == 0 { continue; } // VK_MEMORY_HEAP_DEVICE_LOCAL_BIT
        free += heaps.heap_budget[i].saturating_sub(heaps.heap_usage[i]);
        size += m.memory_heaps[i].size;
    }
    (size > 0).then_some((free, size))
}

/// Tells the GPU profiler what this device can time: whole render passes and copies with
/// timestamp queries (no stage lanes), placed on the host clock by calibration where offered.
fn report_profiling(name: &CStr, properties: &vk::PhysicalDeviceProperties, bits: u32, calibrated: bool) {
    let period = properties.limits.timestamp_period;
    let mut notes = vec!["render passes and copies are timed whole with timestamp queries (no vertex / fragment lanes)".to_string(),
        "no shader cost hints: pipelines arrive as SPIR-V".to_string()];
    if bits == 0 { notes.push("the graphics queue has no timestamps: nothing is timed".into()); }
    if !calibrated { notes.push("no calibrated timestamps: GPU times are placed on the host clock where each frame's fence was seen (late by the completion worker's wake-up)".into()); }
    crate::gpu_prof::set_device(crate::gpu_prof::DeviceInfo {
        name: name.to_string_lossy().into_owned(),
        counter_sets: vec![format!("timestamp ({bits} valid bits, {period} ns per tick)")],
        sampling: vec!["pass boundary".into(), if calibrated { "calibrated device/host clock".into() } else { "fence-time clock anchor".into() }],
        notes,
    });
}

/// How long a device creation out of memory waits before it tries again.
const DEVICE_RETRY: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a budget read is reused: it answers every submit, and the driver call is not free.
const BUDGET_REUSE: std::time::Duration = std::time::Duration::from_millis(250);

static SHARED: Mutex<Option<(Vec<String>, Arc<Device>)>> = Mutex::new(None);

/// Owns a VkInstance: destroyed when the last device on it is gone. `creator` is the runtime
/// that made it (and makes its devices, a replacement after a loss too), with the loader's
/// `vkGetInstanceProcAddr` it was given.
struct InstanceLife(makepad_vulkan::Instance, Option<(Arc<dyn super::surface::VkCreator>, makepad_vulkan::commands::PfnGetInstanceProcAddr)>);
impl Drop for InstanceLife {
    fn drop(&mut self) { unsafe { self.0.destroy_instance(None) } }
}

/// What a logical device was made with on its physical device, to make it again after a loss.
struct Wanted<'a> {
    instance_extensions: &'a [&'a CStr],
    optional: &'a [&'a CStr],
}

impl Device {
    /// The process's Vulkan device if a backend made it already (code outside any session,
    /// such as the DMA-BUF copies of `crate::dmabuf::copy`, uses it and never makes one).
    #[cfg(target_os = "linux")]
    pub(crate) fn existing() -> Option<Arc<Self>> {
        lock(&SHARED).as_ref().map(|(_, device)| device.clone())
    }

    /// The process's one Vulkan device (HAL.md: one device per process, shared by every session
    /// and every Hal). The first backend chooses the instance/device extensions; a later one that
    /// needs an extension the device was not created with is an error, never a second device
    /// (concurrent instance creation crashed inside the loader).
    pub(crate) fn shared(instance_extensions: &[&CStr], optional: &[&CStr], source: &mut dyn super::surface::VkSurfaceSource) -> Result<Arc<Self>, String> {
        let mut shared = lock(&SHARED);
        if let Some((instance_names, device)) = shared.as_ref() {
            if let Some(missing) = instance_extensions.iter().map(|n| n.to_string_lossy().into_owned()).find(|n| !instance_names.contains(n)) {
                return Err(format!("the process's Vulkan device was created without instance extension {missing}"));
            }
            return Ok(device.clone());
        }
        let device = Device::new(instance_extensions, optional, source)?;
        *shared = Some((instance_extensions.iter().map(|n| n.to_string_lossy().into_owned()).collect(), device.clone()));
        Ok(device)
    }

    fn new(instance_extensions: &[&CStr], optional: &[&CStr], source: &mut dyn super::surface::VkSurfaceSource) -> Result<Arc<Self>, String> {
        if log_enabled() { eprintln!("[hal-vulkan] opening loader; required instance extensions={instance_extensions:?}"); }
        let entry = loader().map_err(|e| format!("Vulkan loader: {e}"))?;
        let available = unsafe { entry.enumerate_instance_extension_properties(None) }
            .map_err(|e| format!("vkEnumerateInstanceExtensionProperties: {e:?}"))?;
        for name in instance_extensions {
            if !available.iter().any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) == *name }) {
                return Err(format!("instance extension {} unavailable", name.to_string_lossy()));
            }
        }
        let names: Vec<_> = instance_extensions.iter().map(|n| n.as_ptr()).collect();
        let app = vk::ApplicationInfo::default().application_name(c"Makepad HAL").api_version(vk::API_VERSION_1_1);
        let info = vk::InstanceCreateInfo::default().application_info(&app).enabled_extension_names(&names);
        let creator = source.creator();
        let (instance, creator) = match creator {
            Some(creator) => {
                let gipa = entry.table.get_instance_proc_addr.ok_or("Vulkan loader lacks vkGetInstanceProcAddr")?;
                let handle = unsafe { creator.create_instance(gipa, &info) }?;
                (unsafe { makepad_vulkan::Instance::from_handle(&entry, handle) }, Some((creator, gipa)))
            }
            None => (unsafe { entry.create_instance(&info, None) }.map_err(|e| format!("vkCreateInstance: {e:?}"))?, None),
        };
        if log_enabled() { eprintln!("[hal-vulkan] vkCreateInstance succeeded"); }
        let life = Arc::new(InstanceLife(instance.clone(), creator));
        // a runtime that makes the device names its GPU
        let devices = match &life.1 {
            Some((creator, _)) => vec![unsafe { creator.physical_device(instance.handle()) }?],
            None => {
                let mut devices = unsafe { instance.enumerate_physical_devices() }
                    .map_err(|e| format!("vkEnumeratePhysicalDevices: {e:?}"))?;
                devices.sort_by_key(|p| match unsafe { instance.get_physical_device_properties(*p) }.device_type {
                    vk::PhysicalDeviceType::DISCRETE_GPU => 0,
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
                    _ => 2,
                });
                source.order_devices(&instance, &mut devices)?;
                devices
            }
        };
        let wanted = Wanted { instance_extensions, optional };
        for physical in devices {
            if let Some(device) = Self::open(&life, physical, &wanted)? { return Ok(device); }
        }
        Err("no Vulkan 1.1 graphics device".into())
    }

    /// A new logical device on the physical device of `lost`, with the same instance,
    /// extensions and features: the replacement after a device loss (`Vulkan::replace_device`).
    /// The process's shared device becomes the new one.
    pub(crate) fn replace(lost: &Arc<Self>, instance_extensions: &[&CStr], optional: &[&CStr]) -> Result<Arc<Self>, String> {
        let wanted = Wanted { instance_extensions, optional };
        let device = Self::open(&lost.life, lost.physical, &wanted)?.ok_or("the GPU no longer offers a Vulkan 1.1 graphics queue")?;
        if let Some((_, shared)) = lock(&SHARED).as_mut() { *shared = device.clone(); }
        Ok(device)
    }

    /// A logical device on `physical`; `None` when that GPU does not qualify.
    fn open(life: &Arc<InstanceLife>, physical: vk::PhysicalDevice, wanted_by: &Wanted) -> Result<Option<Arc<Self>>, String> {
        let instance = &life.0;
        let (instance_extensions, optional) = (wanted_by.instance_extensions, wanted_by.optional);
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        let name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) };
        if log_enabled() { eprintln!("[hal-vulkan] device candidate={name:?} handle={physical:?} type={:?} api={:#x} driver={:#x}", properties.device_type, properties.api_version, properties.driver_version); }
        if properties.api_version < vk::API_VERSION_1_1 {
            if log_enabled() { eprintln!("[hal-vulkan] skipping {name:?}: Vulkan 1.1 required"); }
            return Ok(None);
        }
        let queues = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let Some((family, queue)) = queues.iter().enumerate().find(|(_, q)| q.queue_count > 0 && q.queue_flags.contains(vk::QueueFlags::GRAPHICS)) else {
            if log_enabled() { eprintln!("[hal-vulkan] skipping {name:?}: no graphics queue"); }
            return Ok(None);
        };
        let available = unsafe { instance.enumerate_device_extension_properties(physical) }
            .map_err(|e| format!("vkEnumerateDeviceExtensionProperties device={name:?}: {e:?}"))?;
        let supported = |name: &CStr| available.iter().any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) == name });
        if instance_extensions.contains(&vk::KHR_SURFACE_NAME) && !supported(vk::KHR_SWAPCHAIN_NAME) {
            if log_enabled() { eprintln!("[hal-vulkan] skipping {name:?}: VK_KHR_swapchain unavailable"); }
            return Ok(None);
        }
        // External-image extensions form one capability, including their dependencies.
        let mut wanted = if optional.iter().all(|n| supported(n)) { optional.to_vec() } else { Vec::new() };
        #[cfg(target_os = "android")]
        if supported(vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME) { wanted.push(vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME); }
        #[cfg(target_os = "linux")]
        let mut timeline = vk::PhysicalDeviceTimelineSemaphoreFeatures::default();
        #[cfg(target_os = "linux")]
        {
            // OPAQUE_FD hosted imports and sync are independent of modifier support.
            if supported(vk::KHR_EXTERNAL_MEMORY_FD_NAME) { wanted.push(vk::KHR_EXTERNAL_MEMORY_FD_NAME); }
            if supported(vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME) { wanted.push(vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME); }
            if supported(vk::KHR_TIMELINE_SEMAPHORE_NAME) {
                let mut features = vk::PhysicalDeviceFeatures2::default().next_physical_device_timeline_semaphore_features(&mut timeline);
                unsafe { instance.get_physical_device_features2(physical, &mut features); }
                if timeline.timeline_semaphore != 0 { wanted.push(vk::KHR_TIMELINE_SEMAPHORE_NAME); }
            }
        }
        if instance_extensions.contains(&vk::KHR_SURFACE_NAME) { wanted.push(vk::KHR_SWAPCHAIN_NAME); }
        let mut present_id = vk::PhysicalDevicePresentIdFeaturesKHR::default();
        let mut present_wait = vk::PhysicalDevicePresentWaitFeaturesKHR::default();
        let wants_display_clock = instance_extensions.contains(&vk::KHR_DISPLAY_NAME);
        let pacing = wants_display_clock && supported(vk::KHR_PRESENT_ID_NAME) && supported(vk::KHR_PRESENT_WAIT_NAME);
        if pacing {
            let mut query = vk::PhysicalDeviceFeatures2::default()
                .next_physical_device_present_id_features_khr(&mut present_id)
                .next_physical_device_present_wait_features_khr(&mut present_wait);
            unsafe { instance.get_physical_device_features2(physical, &mut query); }
        }
        let pacing = pacing && present_id.present_id != 0 && present_wait.present_wait != 0;
        // These were chained for the feature query. Reusing that chain while
        // prepending the same structures would make a cyclic pNext list.
        present_id.p_next = std::ptr::null_mut();
        present_wait.p_next = std::ptr::null_mut();
        if pacing { wanted.extend([vk::KHR_PRESENT_ID_NAME, vk::KHR_PRESENT_WAIT_NAME]); }
        // the device's real budget (other processes included), for the UI's memory reserve
        if supported(vk::EXT_MEMORY_BUDGET_NAME) { wanted.push(vk::EXT_MEMORY_BUDGET_NAME); }
        // the GPU profiler's clock: device timestamps sampled with the host's CLOCK_MONOTONIC (the
        // HAL clock on Linux and Android); the KHR name first, the EXT one is the same functions
        let calibration = [vk::KHR_CALIBRATED_TIMESTAMPS_NAME, vk::EXT_CALIBRATED_TIMESTAMPS_NAME].into_iter().find(|n| supported(n))
            .filter(|_| instance.table.get_physical_device_calibrateable_time_domains_khr.is_some())
            .filter(|_| unsafe { instance.get_physical_device_calibrateable_time_domains(physical) }
                .is_ok_and(|d| d.contains(&vk::TimeDomainKHR::DEVICE) && d.contains(&vk::TimeDomainKHR::CLOCK_MONOTONIC)));
        if let Some(name) = calibration { wanted.push(name); }
        // The hardware video encoder: its queue family and the four extensions it takes, when the
        // GPU has all of them (nothing else changes on a GPU without them).
        let video_names = [vk::KHR_SYNCHRONIZATION_2_NAME, vk::KHR_VIDEO_QUEUE_NAME, vk::KHR_VIDEO_ENCODE_QUEUE_NAME, vk::KHR_VIDEO_ENCODE_H264_NAME];
        let video_family = if video_names.iter().all(|n| supported(n)) {
            queues.iter().position(|q| q.queue_count > 0 && q.queue_flags.contains(vk::QueueFlags::VIDEO_ENCODE_KHR))
                .filter(|f| *f != family)
        } else { None };
        if video_family.is_some() { wanted.extend(video_names); }
        let h265 = video_family.is_some() && supported(vk::KHR_VIDEO_ENCODE_H265_NAME);
        if h265 { wanted.push(vk::KHR_VIDEO_ENCODE_H265_NAME); }
        // 4:4:4 encode source images (G8_B8R8_2PLANE_444): an extension below Vulkan 1.3, and a
        // feature
        let mut plane444 = vk::PhysicalDeviceYcbcr2Plane444FormatsFeaturesEXT::default();
        if video_family.is_some() && supported(vk::EXT_YCBCR_2PLANE_444_FORMATS_NAME) {
            let mut query = vk::PhysicalDeviceFeatures2::default().next_physical_device_ycbcr2_plane444_formats_features_ext(&mut plane444);
            unsafe { instance.get_physical_device_features2(physical, &mut query); }
            plane444.p_next = std::ptr::null_mut();
            if plane444.ycbcr2plane444_formats != 0 { wanted.push(vk::EXT_YCBCR_2PLANE_444_FORMATS_NAME); }
        }
        // QP delta maps for the encoder (a QP per region of the picture)
        let mut qp_map = vk::PhysicalDeviceVideoEncodeQuantizationMapFeaturesKHR::default();
        if video_family.is_some() && supported(vk::KHR_VIDEO_ENCODE_QUANTIZATION_MAP_NAME) {
            let mut query = vk::PhysicalDeviceFeatures2::default().next_physical_device_video_encode_quantization_map_features_khr(&mut qp_map);
            unsafe { instance.get_physical_device_features2(physical, &mut query); }
            qp_map.p_next = std::ptr::null_mut();
            if qp_map.video_encode_quantization_map != 0 { wanted.push(vk::KHR_VIDEO_ENCODE_QUANTIZATION_MAP_NAME); }
        }
        let has_qp_map = qp_map.video_encode_quantization_map != 0;
        // fragment density maps (an XR runtime's foveation of its swapchains)
        let mut fdm = vk::PhysicalDeviceFragmentDensityMapFeaturesEXT::default();
        if supported(vk::EXT_FRAGMENT_DENSITY_MAP_NAME) && instance.table.get_physical_device_features2.is_some() {
            let mut query = vk::PhysicalDeviceFeatures2::default().next_physical_device_fragment_density_map_features_ext(&mut fdm);
            unsafe { instance.get_physical_device_features2(physical, &mut query); }
            fdm.p_next = std::ptr::null_mut();
            if fdm.fragment_density_map != 0 { wanted.push(vk::EXT_FRAGMENT_DENSITY_MAP_NAME); }
        }
        let has_fdm = fdm.fragment_density_map != 0;
        // MSAA passes: multisampled attachments resolved at the end of the subpass, depth (its
        // sample 0) too
        let msaa = if supported(vk::KHR_CREATE_RENDERPASS_2_NAME) && supported(vk::KHR_DEPTH_STENCIL_RESOLVE_NAME) {
            wanted.push(vk::KHR_CREATE_RENDERPASS_2_NAME);
            wanted.push(vk::KHR_DEPTH_STENCIL_RESOLVE_NAME);
            let l = &properties.limits;
            (l.framebuffer_color_sample_counts & l.framebuffer_depth_sample_counts).0 & 0b1111 | 1
        } else { 1 };
        let mut fdm = vk::PhysicalDeviceFragmentDensityMapFeaturesEXT::default().fragment_density_map(has_fdm);
        wanted.sort_by(|a, b| a.to_bytes().cmp(b.to_bytes()));
        wanted.dedup();
        if log_enabled() { eprintln!("[hal-vulkan] vkCreateDevice device={name:?} queue_family={family} extensions={wanted:?}"); }
        let extensions = wanted.iter().map(|n| n.to_string_lossy().into_owned()).collect();
        let wanted: Vec<_> = wanted.iter().map(|n| n.as_ptr()).collect();
        // multiview (core in 1.1): a stereo view renders both eyes in one pass
        let mut multiview = vk::PhysicalDeviceMultiviewFeatures::default();
        {
            let mut query = vk::PhysicalDeviceFeatures2::default().next_physical_device_multiview_features(&mut multiview);
            unsafe { instance.get_physical_device_features2(physical, &mut query); }
        }
        multiview.p_next = std::ptr::null_mut();
        let has_multiview = multiview.multiview != 0;
        let mut multiview = vk::PhysicalDeviceMultiviewFeatures::default().multiview(has_multiview);
        // sampler Y'CbCr conversion (core in 1.1): a decoder's pictures sampled where they are
        let mut ycbcr = vk::PhysicalDeviceSamplerYcbcrConversionFeatures::default();
        {
            let mut query = vk::PhysicalDeviceFeatures2::default().next_physical_device_sampler_ycbcr_conversion_features(&mut ycbcr);
            unsafe { instance.get_physical_device_features2(physical, &mut query); }
        }
        ycbcr.p_next = std::ptr::null_mut();
        let has_ycbcr = ycbcr.sampler_ycbcr_conversion != 0 && instance.table.get_physical_device_features2.is_some();
        let mut ycbcr = vk::PhysicalDeviceSamplerYcbcrConversionFeatures::default().sampler_ycbcr_conversion(has_ycbcr);
        let supported_features = unsafe { instance.get_physical_device_features(physical) };
        let features = vk::PhysicalDeviceFeatures::default()
            .independent_blend(supported_features.independent_blend == vk::TRUE)
            .texture_compression_bc(supported_features.texture_compression_bc == vk::TRUE)
            .texture_compression_astc_ldr(supported_features.texture_compression_astc_ldr == vk::TRUE)
            // line fill: frame capture's wireframe view (`Draw::WIREFRAME`)
            .fill_mode_non_solid(supported_features.fill_mode_non_solid == vk::TRUE)
            // an MSAA pass's alpha to coverage stores alpha 1 (the resolve then holds the coverage)
            .alpha_to_one(supported_features.alpha_to_one == vk::TRUE);
        let priorities = [1.0];
        // every encode engine's queue (up to 4)
        let video_count = video_family.map_or(0, |f| queues[f].queue_count.min(4));
        let video_priorities = [1.0; 4];
        let mut queue_infos = vec![vk::DeviceQueueCreateInfo::default().queue_family_index(family as u32).queue_priorities(&priorities)];
        if let Some(video) = video_family { queue_infos.push(vk::DeviceQueueCreateInfo::default().queue_family_index(video as u32).queue_priorities(&video_priorities[..video_count as usize])); }
        let video_timestamp_bits = video_family.map_or(0, |f| queues[f].timestamp_valid_bits);
        let queues = queue_infos;
        let mut create = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues).enabled_extension_names(&wanted).enabled_features(&features);
        #[cfg(target_os = "linux")]
        if timeline.timeline_semaphore != 0 { create = create.next_physical_device_timeline_semaphore_features(&mut timeline); }
        if pacing { create = create.next_physical_device_present_id_features_khr(&mut present_id)
            .next_physical_device_present_wait_features_khr(&mut present_wait); }
        if plane444.ycbcr2plane444_formats != 0 { create = create.next_physical_device_ycbcr2_plane444_formats_features_ext(&mut plane444); }
        if has_multiview { create = create.next_physical_device_multiview_features(&mut multiview); }
        if has_ycbcr { create = create.next_physical_device_sampler_ycbcr_conversion_features(&mut ycbcr); }
        if has_fdm { create = create.next_physical_device_fragment_density_map_features_ext(&mut fdm); }
        if has_qp_map { create = create.next_physical_device_video_encode_quantization_map_features_khr(&mut qp_map); }
        // A GPU full of other work (an AI model) has no room for a device yet: the app waits
        // for room -- the desktop's owner asks the hub to yield on the sustained shortage --
        // instead of falling back to another renderer. It never fails for want of memory.
        // (NVIDIA answers a full GPU with ERROR_INITIALIZATION_FAILED as well: that counts as
        // out of memory while the GPU's free memory is under the UI's reserve.)
        let budget = supported(vk::EXT_MEMORY_BUDGET_NAME);
        let short_of_memory = |e: vk::Result| match e {
            vk::Result::ERROR_OUT_OF_DEVICE_MEMORY | vk::Result::ERROR_OUT_OF_HOST_MEMORY => true,
            vk::Result::ERROR_INITIALIZATION_FAILED => budget && device_local_free(instance, physical)
                .is_some_and(|(free, size)| free < crate::gpu_pressure::ui_reserve(size)),
            _ => false,
        };
        let mut waited = 0u64;
        let raw = loop {
            let made = match &life.1 {
                Some((creator, gipa)) => unsafe { creator.create_device(*gipa, physical, &create) }
                    .map(|handle| unsafe { makepad_vulkan::Device::from_handle(instance, handle) }),
                None => unsafe { instance.create_device(physical, &create, None) },
            };
            match made {
                Ok(raw) => break raw,
                Err(e) if short_of_memory(e) => {
                    if waited == 0 { eprintln!("[hal-vulkan] vkCreateDevice device={name:?}: {e:?}; waiting for GPU memory"); }
                    waited += 1;
                    std::thread::sleep(DEVICE_RETRY);
                }
                Err(e) => return Err(format!("vkCreateDevice device={name:?} queue_family={family}: {e:?}")),
            }
        };
        if waited > 0 { eprintln!("[hal-vulkan] vkCreateDevice device={name:?}: made after {waited} tries out of GPU memory"); }
        if log_enabled() { eprintln!("[hal-vulkan] vkCreateDevice succeeded device={name:?} queue_family={family}"); }
        let memory = unsafe { instance.get_physical_device_memory_properties(physical) };
        let formats = std::array::from_fn(|f| unsafe {
            instance.get_physical_device_format_properties(physical, super::resources::vk_format(f as u32).unwrap())
        });
        let calibrated = calibration.is_some() && raw.table.get_calibrated_timestamps_khr.is_some();
        report_profiling(name, &properties, queue.timestamp_valid_bits, calibrated);
        let video_encode = video_family.map(|f| VideoQueue { family: f as u32, queue: unsafe { raw.get_device_queue(f as u32, 0) }, timestamp_bits: video_timestamp_bits,
            more: (1..video_count).map(|i| (unsafe { raw.get_device_queue(f as u32, i) }, Mutex::new(()))).collect(), lock: Mutex::new(()), h265, qp_map: has_qp_map });
        if let Some(v) = &video_encode { if log_enabled() { eprintln!("[hal-vulkan] hardware video encode queue family={}", v.family); } }
        let (pipeline_cache, pipeline_cache_file) = open_pipeline_cache(&raw, &properties);
        Ok(Some(Arc::new(Self {
            queue: unsafe { raw.get_device_queue(family as u32, 0) }, queue_lock: Mutex::new(()), raw, instance: instance.clone(),
            physical, family: family as u32, properties, memory, features, formats,
            timestamp_bits: queue.timestamp_valid_bits, calibrated, extensions, present_wait: pacing, multiview: has_multiview, fdm: has_fdm, msaa, life: life.clone(),
            video_encode,
            allocated: AtomicU64::new(0), lost: AtomicBool::new(false), starved: AtomicBool::new(false), files_reported: AtomicBool::new(false), budget: Mutex::new(None),
            ycbcr: has_ycbcr, video_samplers: Mutex::new(Default::default()),
            pipeline_cache, pipeline_cache_file, pipelines_made: AtomicU32::new(0), pipeline_made_ns: AtomicU64::new(0),
        })))
    }

    pub fn memory_type(&self, bits: u32, required: vk::MemoryPropertyFlags) -> Result<u32, String> {
        (0..self.memory.memory_type_count).find(|i| bits & (1 << i) != 0 && self.memory.memory_types[*i as usize].property_flags.contains(required))
            .ok_or_else(|| format!("no memory type for {required:?} ({bits:#x})"))
    }

    /// The sampler of a video texture slot: an ordinary image's (`external_format` 0) or one
    /// through the Y'CbCr conversion of a decoder's external format (`props`: its format's
    /// properties), made once per format. Model RGB identity, full range: sampling gives the raw
    /// codes (.g = Y', .b = Cb, .r = Cr); the shader converts with the picture's own colour.
    pub fn video_sampler(&self, external_format: u64, props: Option<&vk::AndroidHardwareBufferFormatPropertiesANDROID>) -> Result<VideoSampler, String> {
        let mut samplers = lock(&self.video_samplers);
        if let Some(v) = samplers.get(&external_format) { return Ok(*v); }
        let mut info = vk::SamplerCreateInfo::default().mag_filter(vk::Filter::LINEAR).min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST).address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE).address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .min_lod(0.0).max_lod(0.0);
        let mut conversion = vk::SamplerYcbcrConversion::null();
        let mut conversion_info = vk::SamplerYcbcrConversionInfo::default();
        if external_format != 0 {
            if !self.ycbcr { return Err("the device has no sampler Y'CbCr conversion".into()); }
            let props = props.ok_or("an external format's properties are needed")?;
            // VK_FORMAT_FEATURE_SAMPLED_IMAGE_YCBCR_CONVERSION_LINEAR_FILTER_BIT
            let linear = props.format_features.contains(vk::FormatFeatureFlags::from_raw(1 << 18));
            let mut external = vk::ExternalFormatANDROID { external_format, ..Default::default() };
            let create = vk::SamplerYcbcrConversionCreateInfo::default().next_external_format_android(&mut external).format(vk::Format::UNDEFINED)
                .ycbcr_model(vk::SamplerYcbcrModelConversion::from_raw(0 /* RGB_IDENTITY */)).ycbcr_range(vk::SamplerYcbcrRange::from_raw(0 /* ITU_FULL */))
                .components(vk::ComponentMapping::default()).x_chroma_offset(props.suggested_x_chroma_offset).y_chroma_offset(props.suggested_y_chroma_offset)
                .chroma_filter(if linear { vk::Filter::LINEAR } else { vk::Filter::NEAREST }).force_explicit_reconstruction(false);
            conversion = unsafe { self.raw.create_sampler_ycbcr_conversion(&create, None) }.map_err(error)?;
            conversion_info = conversion_info.conversion(conversion);
            info = info.next_sampler_ycbcr_conversion_info(&mut conversion_info);
            if !linear { info = info.mag_filter(vk::Filter::NEAREST).min_filter(vk::Filter::NEAREST); }
        }
        let sampler = match unsafe { self.raw.create_sampler(&info, None) } {
            Ok(s) => s,
            Err(e) => {
                if conversion != vk::SamplerYcbcrConversion::null() { unsafe { self.raw.destroy_sampler_ycbcr_conversion(conversion, None); } }
                return Err(error(e));
            }
        };
        let v = VideoSampler { conversion, sampler };
        samplers.insert(external_format, v);
        Ok(v)
    }

    pub fn supports(&self, extension: &CStr) -> bool { self.extensions.contains(extension.to_string_lossy().as_ref()) }

    /// A device timestamp and the HAL clock's ns sampled together (`None` without calibrated
    /// timestamps): the GPU profiler's anchor for a batch's query results.
    pub fn calibrate(&self) -> Option<(u64, u64)> {
        if !self.calibrated { return None; }
        let infos = [vk::CalibratedTimestampInfoKHR::default().time_domain(vk::TimeDomainKHR::DEVICE),
            vk::CalibratedTimestampInfoKHR::default().time_domain(vk::TimeDomainKHR::CLOCK_MONOTONIC)];
        let mut t = [0u64; 2];
        unsafe { self.raw.get_calibrated_timestamps(&infos, &mut t) }.ok()?;
        Some((t[0], t[1]))
    }

    /// The device-local budget; `fresh` reads the driver now (after a failed allocation),
    /// otherwise a read younger than `BUDGET_REUSE` is reused. `None` without the extension.
    pub fn memory_budget(&self, fresh: bool) -> Option<MemoryBudget> {
        if !self.supports(vk::EXT_MEMORY_BUDGET_NAME) { return None; }
        let now = std::time::Instant::now();
        let mut cached = lock(&self.budget);
        if let Some((at, b)) = *cached {
            if !fresh && now.duration_since(at) < BUDGET_REUSE { return Some(b); }
        }
        let mut heaps = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
        let mut props = vk::PhysicalDeviceMemoryProperties2::default().next_physical_device_memory_budget_properties_ext(&mut heaps);
        unsafe { self.instance.get_physical_device_memory_properties2(self.physical, &mut props); }
        let m = props.memory_properties;
        let mut b = MemoryBudget::default();
        for i in 0..(m.memory_heap_count as usize).min(m.memory_heaps.len()) {
            // VK_MEMORY_HEAP_DEVICE_LOCAL_BIT
            if m.memory_heaps[i].flags.as_raw() & 1 == 0 { continue; }
            b.budget += heaps.heap_budget[i];
            b.usage += heaps.heap_usage[i];
            b.size += m.memory_heaps[i].size;
        }
        *cached = Some((now, b));
        Some(b)
    }

    /// A create or wait on this device answered out of memory (`what` names it) while the
    /// process is at its open-file limit: the driver needs a descriptor per allocation and has
    /// none left, so it is not a full GPU, and asking the host and the AI hub for GPU memory
    /// frees nothing. Reported once per episode with the numbers; `true` at the limit.
    pub fn out_of_files(&self, what: &str) -> bool {
        if let Some((open, soft)) = crate::open_files().filter(|(open, soft)| crate::at_file_limit(*open, *soft)) {
            if !self.files_reported.swap(true, Ordering::AcqRel) {
                eprintln!("[hal-vulkan] {what}: out of memory at the open-file limit ({open} of {soft} descriptors; the driver holds one per allocation), not a full GPU");
            }
            return true;
        }
        self.files_reported.store(false, Ordering::Release);
        false
    }

    /// A submission, fence wait, arena or compile out of memory: below the open-file limit the
    /// backend asks for room at once (`starved`); at it, see `out_of_files`.
    pub fn refused(&self, what: &str) {
        if !self.out_of_files(what) { self.starved.store(true, Ordering::Release); }
    }

    pub fn charge(&self, bytes: u64) { self.allocated.fetch_add(bytes, Ordering::Relaxed); }
    pub fn uncharge(&self, bytes: u64) { self.allocated.fetch_sub(bytes, Ordering::Relaxed); }

    /// A display-only peer on another GPU. Rendering remains on the process's compositor
    /// device; this queue only reads imported images and drives that GPU's KMS outputs.
    #[cfg(target_os = "linux")]
    pub(crate) fn display_peer(owner: Arc<Self>, physical: vk::PhysicalDevice) -> Result<Arc<Self>, String> {
        if physical == owner.physical { return Ok(owner); }
        let instance = &owner.instance;
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        if properties.api_version < vk::API_VERSION_1_1 { return Err("display GPU requires Vulkan 1.1".into()); }
        let queues = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let (family, queue) = queues.iter().enumerate().find(|(_, q)| q.queue_count > 0 && q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or("display GPU has no graphics queue")?;
        let available = unsafe { instance.enumerate_device_extension_properties(physical) }.map_err(error)?;
        let supported = |n: &CStr| available.iter().any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) == n });
        if !supported(vk::KHR_SWAPCHAIN_NAME) { return Err("display GPU lacks VK_KHR_swapchain".into()); }
        let mut present_id = vk::PhysicalDevicePresentIdFeaturesKHR::default();
        let mut present_wait = vk::PhysicalDevicePresentWaitFeaturesKHR::default();
        if supported(vk::KHR_PRESENT_ID_NAME) && supported(vk::KHR_PRESENT_WAIT_NAME) {
            let mut query = vk::PhysicalDeviceFeatures2::default()
                .next_physical_device_present_id_features_khr(&mut present_id)
                .next_physical_device_present_wait_features_khr(&mut present_wait);
            unsafe { instance.get_physical_device_features2(physical, &mut query); }
        }
        let pacing = present_id.present_id != 0 && present_wait.present_wait != 0;
        present_id.p_next = std::ptr::null_mut();
        present_wait.p_next = std::ptr::null_mut();
        let sharing = <super::NativeImages as super::VkExternalImages>::device_extensions(&super::NativeImages::default());
        let mut wanted = vec![vk::KHR_SWAPCHAIN_NAME];
        if sharing.iter().all(|n| supported(n)) { wanted.extend(sharing); }
        if supported(vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME) { wanted.push(vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME); }
        if pacing { wanted.extend([vk::KHR_PRESENT_ID_NAME, vk::KHR_PRESENT_WAIT_NAME]); }
        let extensions = wanted.iter().map(|n| n.to_string_lossy().into_owned()).collect();
        let names: Vec<_> = wanted.iter().map(|n| n.as_ptr()).collect();
        let priorities = [1.0];
        let queue_info = [vk::DeviceQueueCreateInfo::default().queue_family_index(family as u32).queue_priorities(&priorities)];
        let features = vk::PhysicalDeviceFeatures::default();
        let mut create = vk::DeviceCreateInfo::default().queue_create_infos(&queue_info).enabled_extension_names(&names).enabled_features(&features);
        if pacing { create = create.next_physical_device_present_id_features_khr(&mut present_id)
            .next_physical_device_present_wait_features_khr(&mut present_wait); }
        let raw = unsafe { instance.create_device(physical, &create, None) }.map_err(error)?;
        let memory = unsafe { instance.get_physical_device_memory_properties(physical) };
        let formats = std::array::from_fn(|f| unsafe { instance.get_physical_device_format_properties(physical, super::resources::vk_format(f as u32).unwrap()) });
        let (pipeline_cache, pipeline_cache_file) = open_pipeline_cache(&raw, &properties);
        Ok(Arc::new(Self { instance: instance.clone(), queue: unsafe { raw.get_device_queue(family as u32, 0) },
            raw, physical, queue_lock: Mutex::new(()), family: family as u32, properties, memory, features, formats,
            // a display peer only scans out: the GPU profiler times the rendering device
            timestamp_bits: queue.timestamp_valid_bits, calibrated: false, extensions, present_wait: pacing, multiview: false, fdm: false, msaa: 1, life: owner.life.clone(), video_encode: None,
            allocated: AtomicU64::new(0), lost: AtomicBool::new(false), starved: AtomicBool::new(false), files_reported: AtomicBool::new(false), budget: Mutex::new(None),
            ycbcr: false, video_samplers: Mutex::new(Default::default()),
            pipeline_cache, pipeline_cache_file, pipelines_made: AtomicU32::new(0), pipeline_made_ns: AtomicU64::new(0) }))
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Every in-flight submission and resource owns an Arc<Device>. The last one is gone; the
        // instance goes with the last device made on it (`life`).
        unsafe {
            for (_, v) in lock(&self.video_samplers).drain() {
                self.raw.destroy_sampler(v.sampler, None);
                if v.conversion != vk::SamplerYcbcrConversion::null() { self.raw.destroy_sampler_ycbcr_conversion(v.conversion, None); }
            }
            if self.pipeline_cache != vk::PipelineCache::null() { self.raw.destroy_pipeline_cache(self.pipeline_cache, None); }
            self.raw.destroy_device(None);
        }
    }
}

/// The pipeline cache file of this device and driver, under the app's cache directory
/// (MAKEPAD_HAL_CACHE_DIR; Android: TMPDIR, the app's cache dir; Apple: Library/Caches; else
/// XDG_CACHE_HOME, LOCALAPPDATA or ~/.cache). None: pipelines are not kept across runs.
fn pipeline_cache_file(properties: &vk::PhysicalDeviceProperties) -> Option<PathBuf> {
    let dir = if let Some(d) = std::env::var_os("MAKEPAD_HAL_CACHE_DIR") { PathBuf::from(d) }
        else if cfg!(target_os = "android") { PathBuf::from(std::env::var_os("TMPDIR")?) }
        else if cfg!(target_vendor = "apple") { PathBuf::from(apple_cache_dir()?) }
        else if let Some(d) = std::env::var_os("XDG_CACHE_HOME") { PathBuf::from(d) }
        else if let Some(d) = std::env::var_os("LOCALAPPDATA") { PathBuf::from(d) }
        else { PathBuf::from(std::env::var_os("HOME")?).join(".cache") };
    Some(dir.join("makepad-hal").join(format!("pipelines-{:x}-{:x}-{:x}.bin", properties.vendor_id, properties.device_id, properties.driver_version)))
}
#[cfg(target_vendor = "apple")]
fn apple_cache_dir() -> Option<String> { crate::app_cache_dir() }
#[cfg(not(target_vendor = "apple"))]
fn apple_cache_dir() -> Option<String> { None }

/// The device's pipeline cache, primed from its file when there is one. On a Snapdragon 685
/// the WM phone's warm start spent seconds of its HAL thread in vkCreateGraphicsPipelines
/// (every variant compiled from SPIR-V, every launch) before this.
fn open_pipeline_cache(raw: &makepad_vulkan::Device, properties: &vk::PhysicalDeviceProperties) -> (vk::PipelineCache, Option<PathBuf>) {
    let file = pipeline_cache_file(properties);
    let data = file.as_ref().and_then(|f| std::fs::read(f).ok()).unwrap_or_default();
    let mut info = vk::PipelineCacheCreateInfo::default();
    if !data.is_empty() { info.initial_data_size = data.len(); info.p_initial_data = data.as_ptr() as *const std::ffi::c_void; }
    let made = unsafe { raw.create_pipeline_cache(&info, None) }
        .or_else(|_| unsafe { raw.create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None) });
    match made {
        Ok(cache) => {
            if log_enabled() { eprintln!("[hal-vulkan] pipeline cache: {} bytes from {}", data.len(), file.as_ref().map_or("nowhere".into(), |f| f.display().to_string())); }
            (cache, file)
        }
        Err(e) => { eprintln!("[hal-vulkan] pipeline cache: {e:?}; pipelines are compiled every run"); (vk::PipelineCache::null(), None) }
    }
}

impl Device {
    /// Writes the pipeline cache when pipelines were made since it was last saved (the file is
    /// replaced whole, through a rename). Called from the compile worker and at exit.
    pub fn save_pipeline_cache(&self) {
        if self.pipeline_cache == vk::PipelineCache::null() { return; }
        let Some(file) = &self.pipeline_cache_file else { return };
        if self.pipelines_made.swap(0, Ordering::AcqRel) == 0 { return; }
        let data = match unsafe { self.raw.get_pipeline_cache_data(self.pipeline_cache) } {
            Ok(d) => d,
            Err(e) => { eprintln!("[hal-vulkan] pipeline cache data: {e:?}"); return; }
        };
        let tmp = file.with_extension("tmp");
        let written = file.parent().map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&tmp, &data))
            .and_then(|_| std::fs::rename(&tmp, file));
        match written {
            Ok(()) => if log_enabled() { eprintln!("[hal-vulkan] pipeline cache saved: {} bytes", data.len()); },
            Err(e) => eprintln!("[hal-vulkan] pipeline cache {}: {e}", file.display()),
        }
    }
}

type Job = Box<dyn FnOnce() + Send>;
/// A job the worker runs at a time, on its own thread. `true` = the worker is closing: the job
/// runs now, early, and must not schedule again.
pub(super) type LaterJob = Box<dyn FnOnce(bool) + Send>;
/// The worker's timed jobs. Only jobs running on that worker add to it (no wakeup needed: the
/// worker reads the earliest time after every round).
pub(super) type Later = Arc<Mutex<Vec<(std::time::Instant, LaterJob)>>>;

/// The bounded channel carries wakeups only. Work spills into an owned FIFO and is never
/// discarded on saturation. No driver call or job runs while the FIFO lock is held.
pub(crate) struct Worker {
    jobs: Arc<Mutex<VecDeque<Job>>>,
    later: Later,
    /// the wakeup sender: `None` once the worker was let go (`Drop`) or stopped (`stop`)
    tx: Mutex<Option<SyncSender<()>>>,
    /// joined by `stop` (the end of the process); a dropped worker runs out on its own
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Worker {
    pub fn new(name: &str) -> Result<Self, String> {
        let jobs = Arc::new(Mutex::new(VecDeque::<Job>::new()));
        let work = jobs.clone();
        let later: Later = Arc::new(Mutex::new(Vec::new()));
        let timed = later.clone();
        let (tx, rx) = sync_channel(1);
        let thread = std::thread::Builder::new().name(name.into()).spawn(move || {
            loop {
                let next = lock(&timed).iter().map(|(at, _)| *at).min();
                let alive = match next {
                    None => rx.recv().is_ok(),
                    Some(at) => !matches!(rx.recv_timeout(at.saturating_duration_since(std::time::Instant::now())), Err(RecvTimeoutError::Disconnected)),
                };
                let ready = std::mem::take(&mut *lock(&work));
                for job in ready { job(); }
                let now = std::time::Instant::now();
                let due: Vec<LaterJob> = {
                    let mut list = lock(&timed);
                    let mut due = Vec::new();
                    let mut i = 0;
                    while i < list.len() {
                        if !alive || list[i].0 <= now { due.push(list.swap_remove(i).1); } else { i += 1; }
                    }
                    due
                };
                for job in due { job(!alive); }
                if !alive { break; }
            }
        }).map_err(|e| e.to_string())?;
        Ok(Self { jobs, later, tx: Mutex::new(Some(tx)), thread: Mutex::new(Some(thread)) })
    }

    /// The timed-job list, for jobs of this worker that continue later on it.
    pub fn later(&self) -> Later { self.later.clone() }

    /// Queues `job`. On a stopped worker (the process is ending, `stop`) the job is abandoned:
    /// dropped here, never run.
    pub fn push(&self, job: impl FnOnce() + Send + 'static) {
        let tx = lock(&self.tx);
        let Some(wake) = tx.as_ref() else {
            drop(tx);
            drop(job);
            return;
        };
        lock(&self.jobs).push_back(Box::new(job));
        match wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {} // an existing wake drains the FIFO
            Err(TrySendError::Disconnected(())) => panic!("Vulkan driver worker stopped"),
        }
    }

    /// Waits (up to `limit`) until the worker ran every job pushed before this call.
    pub fn settle(&self, limit: std::time::Duration) -> bool {
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        self.push(move || { let _ = done.send(()); });
        wait.recv_timeout(limit).is_ok()
    }

    /// The end of the process (`Vulkan::before_exit`): the worker runs every job queued so far,
    /// its timed jobs early (closing), and its thread ends; this returns once it has. Jobs pushed
    /// afterwards are abandoned (`push`). Never on the worker's own thread.
    pub fn stop(&self) {
        lock(&self.tx).take();
        let thread = lock(&self.thread).take();
        if let Some(thread) = thread {
            debug_assert!(thread.thread().id() != std::thread::current().id(), "a Vulkan worker cannot stop itself");
            if thread.join().is_err() { eprintln!("[hal-vulkan] a driver worker panicked"); }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        lock(&self.tx).take();
        // The worker drains the FIFO and exits. Jobs retain their device/resources; dropping
        // the HAL on a UI thread must not join a thread that can be waiting for a GPU fence
        // (only the end of the process joins it: `stop`).
    }
}

/// The Vulkan loader. makepad-vulkan opens it once per process and never closes it (ICDs register
/// process-exit and thread-exit handlers that must not outlive their library).
fn loader() -> Result<Entry, String> { unsafe { Entry::load() } }

#[cfg(test)]
mod tests {
    use super::Worker;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// The end of the process stops a driver worker: every job queued before `stop` ran, its
    /// timed jobs ran early (closing), its thread is gone when `stop` returns, and a job pushed
    /// afterwards is abandoned (dropped, never run, no panic).
    #[test]
    fn stop_runs_the_queued_jobs_and_joins_the_thread() {
        let worker = Worker::new("makepad-vulkan-test").expect("worker");
        let ran = Arc::new(AtomicUsize::new(0));
        let closing = Arc::new(AtomicUsize::new(0));
        let thread_name = Arc::new(std::sync::Mutex::new(None));
        {
            let (later, closing, name) = (worker.later(), closing.clone(), thread_name.clone());
            worker.push(move || {
                *name.lock().unwrap() = std::thread::current().name().map(str::to_owned);
                // a retry far in the future: only the stop runs it, as closing
                later.lock().unwrap().push((std::time::Instant::now() + std::time::Duration::from_secs(3600), Box::new(move |c: bool| {
                    if c { closing.fetch_add(1, Ordering::SeqCst); }
                })));
            });
        }
        for _ in 0..100 {
            let ran = ran.clone();
            worker.push(move || { ran.fetch_add(1, Ordering::SeqCst); });
        }
        worker.stop();
        assert_eq!(ran.load(Ordering::SeqCst), 100, "every queued job ran before stop returned");
        assert_eq!(closing.load(Ordering::SeqCst), 1, "the timed job ran early, closing");
        assert_eq!(thread_name.lock().unwrap().as_deref(), Some("makepad-vulkan-test"));
        assert!(super::lock(&worker.thread).is_none(), "the thread was joined");
        let late = ran.clone();
        worker.push(move || { late.fetch_add(1, Ordering::SeqCst); });
        assert_eq!(ran.load(Ordering::SeqCst), 100, "a job after stop never runs");
        assert_eq!(Arc::strong_count(&ran), 1, "the abandoned job was dropped");
        // stopping again (a second before_exit) is a no-op
        worker.stop();
    }
}

//! Device creation and long-lived driver workers. Binding calls follow os/linux/vulkan.rs
//! and vulkan_linux.rs; no Cx objects or platform window handles enter this module.

use super::super::log_enabled;
use makepad_vulkan::{vk, Entry};
use std::collections::{HashSet, VecDeque};
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn error(e: vk::Result) -> String { format!("Vulkan: {e:?}") }

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
    pub extensions: HashSet<String>,
    pub present_wait: bool,
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

/// How long a device creation out of memory waits before it tries again.
const DEVICE_RETRY: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a budget read is reused: it answers every submit, and the driver call is not free.
const BUDGET_REUSE: std::time::Duration = std::time::Duration::from_millis(250);

static SHARED: Mutex<Option<(Vec<String>, Arc<Device>)>> = Mutex::new(None);

/// Owns a VkInstance: destroyed when the last device on it is gone.
struct InstanceLife(makepad_vulkan::Instance);
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
        let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app).enabled_extension_names(&names), None) }
            .map_err(|e| format!("vkCreateInstance: {e:?}"))?;
        if log_enabled() { eprintln!("[hal-vulkan] vkCreateInstance succeeded"); }
        let life = Arc::new(InstanceLife(instance.clone()));
        let mut devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|e| format!("vkEnumeratePhysicalDevices: {e:?}"))?;
        devices.sort_by_key(|p| match unsafe { instance.get_physical_device_properties(*p) }.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 0,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
            _ => 2,
        });
        source.order_devices(&instance, &mut devices)?;
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
        wanted.sort_by(|a, b| a.to_bytes().cmp(b.to_bytes()));
        wanted.dedup();
        if log_enabled() { eprintln!("[hal-vulkan] vkCreateDevice device={name:?} queue_family={family} extensions={wanted:?}"); }
        let extensions = wanted.iter().map(|n| n.to_string_lossy().into_owned()).collect();
        let wanted: Vec<_> = wanted.iter().map(|n| n.as_ptr()).collect();
        let supported_features = unsafe { instance.get_physical_device_features(physical) };
        let features = vk::PhysicalDeviceFeatures::default()
            .independent_blend(supported_features.independent_blend == vk::TRUE)
            .texture_compression_bc(supported_features.texture_compression_bc == vk::TRUE)
            .texture_compression_astc_ldr(supported_features.texture_compression_astc_ldr == vk::TRUE)
            // line fill: frame capture's wireframe view (`Draw::WIREFRAME`)
            .fill_mode_non_solid(supported_features.fill_mode_non_solid == vk::TRUE);
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family as u32).queue_priorities(&priorities)];
        let mut create = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues).enabled_extension_names(&wanted).enabled_features(&features);
        #[cfg(target_os = "linux")]
        if timeline.timeline_semaphore != 0 { create = create.next_physical_device_timeline_semaphore_features(&mut timeline); }
        if pacing { create = create.next_physical_device_present_id_features_khr(&mut present_id)
            .next_physical_device_present_wait_features_khr(&mut present_wait); }
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
            match unsafe { instance.create_device(physical, &create, None) } {
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
        Ok(Some(Arc::new(Self {
            queue: unsafe { raw.get_device_queue(family as u32, 0) }, queue_lock: Mutex::new(()), raw, instance: instance.clone(),
            physical, family: family as u32, properties, memory, features, formats,
            timestamp_bits: queue.timestamp_valid_bits, extensions, present_wait: pacing, life: life.clone(),
            allocated: AtomicU64::new(0), lost: AtomicBool::new(false), starved: AtomicBool::new(false), files_reported: AtomicBool::new(false), budget: Mutex::new(None),
        })))
    }

    pub fn memory_type(&self, bits: u32, required: vk::MemoryPropertyFlags) -> Result<u32, String> {
        (0..self.memory.memory_type_count).find(|i| bits & (1 << i) != 0 && self.memory.memory_types[*i as usize].property_flags.contains(required))
            .ok_or_else(|| format!("no memory type for {required:?} ({bits:#x})"))
    }

    pub fn supports(&self, extension: &CStr) -> bool { self.extensions.contains(extension.to_string_lossy().as_ref()) }

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
        Ok(Arc::new(Self { instance: instance.clone(), queue: unsafe { raw.get_device_queue(family as u32, 0) },
            raw, physical, queue_lock: Mutex::new(()), family: family as u32, properties, memory, features, formats,
            timestamp_bits: queue.timestamp_valid_bits, extensions, present_wait: pacing, life: owner.life.clone(),
            allocated: AtomicU64::new(0), lost: AtomicBool::new(false), starved: AtomicBool::new(false), files_reported: AtomicBool::new(false), budget: Mutex::new(None) }))
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Every in-flight submission and resource owns an Arc<Device>. The last one is gone; the
        // instance goes with the last device made on it (`life`).
        unsafe { self.raw.destroy_device(None); }
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
pub(super) struct Worker {
    jobs: Arc<Mutex<VecDeque<Job>>>,
    later: Later,
    tx: Option<SyncSender<()>>,
}

impl Worker {
    pub fn new(name: &str) -> Result<Self, String> {
        let jobs = Arc::new(Mutex::new(VecDeque::<Job>::new()));
        let work = jobs.clone();
        let later: Later = Arc::new(Mutex::new(Vec::new()));
        let timed = later.clone();
        let (tx, rx) = sync_channel(1);
        std::thread::Builder::new().name(name.into()).spawn(move || {
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
        Ok(Self { jobs, later, tx: Some(tx) })
    }

    /// The timed-job list, for jobs of this worker that continue later on it.
    pub fn later(&self) -> Later { self.later.clone() }

    pub fn push(&self, job: impl FnOnce() + Send + 'static) {
        lock(&self.jobs).push_back(Box::new(job));
        match self.tx.as_ref().expect("live worker").try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {} // an existing wake drains the FIFO
            Err(TrySendError::Disconnected(())) => panic!("Vulkan driver worker stopped"),
        }
    }
}

impl Worker {
    /// Waits (up to `limit`) until the worker ran every job pushed before this call.
    pub fn settle(&self, limit: std::time::Duration) -> bool {
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        self.push(move || { let _ = done.send(()); });
        wait.recv_timeout(limit).is_ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx.take();
        // The worker drains the FIFO and exits. Jobs retain their device/resources; dropping
        // the HAL on a UI thread must not join a thread that can be waiting for a GPU fence.
    }
}

/// The Vulkan loader. makepad-vulkan opens it once per process and never closes it (ICDs register
/// process-exit and thread-exit handlers that must not outlive their library).
fn loader() -> Result<Entry, String> { unsafe { Entry::load() } }

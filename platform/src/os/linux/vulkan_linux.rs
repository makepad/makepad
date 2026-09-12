use super::*;
use crate::{os::linux::display::LINUX_VULKAN_DEVICE_ENV, WindowId};
// Direct (DRM/KMS) output support exists only in linux_direct builds; windowed
// Linux never constructs it.
#[cfg(linux_direct)]
use crate::{
    os::linux::display::{LinuxDisplayOutput, LinuxDisplaySnapshot},
    thread::{to_ui_bounded, TaskHandle, ThreadOptions, ThreadSpawner, ToUIReceiver, ToUISender},
};
#[cfg(linux_direct)]
use std::{
    fs::File,
    num::NonZeroUsize,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// Inactive windows own their WSI resources, while drawing uses the selected
/// window's resources in CxVulkan. Geometry, textures and pipelines stay shared.
#[derive(Default)]
struct DesktopWindow {
    native_surface: *mut c_void,
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    depth_targets: Vec<VulkanTextureResource>,
    readback: Option<VulkanBuffer>,
    format: vk::Format,
    depth_format: vk::Format,
    extent: vk::Extent2D,
    render_pass: vk::RenderPass,
    xr_render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    present_semaphores: Vec<vk::Semaphore>,
    width: u32,
    height: u32,
}

#[derive(Default)]
pub(super) struct DesktopState {
    pub(super) shared: super::shared::SharedState,
    wayland: Option<(ash::khr::wayland_surface::Instance, *mut c_void)>,
    windows: HashMap<(usize, u64), DesktopWindow>,
    selected: Option<WindowId>,
    native_surface: *mut c_void,
    #[cfg(linux_direct)]
    pub(super) direct: Option<DirectState>,
    /// Separate rendering and output devices. The display renderer retains
    /// ownership of DRM, WSI, output pacing and the final local image.
    #[cfg(linux_direct)]
    pub(super) routed: Option<RoutedCompositor>,
    #[cfg(linux_direct)]
    pub(super) gpu: super::transition::GpuState,
    #[cfg(linux_direct)]
    pub(super) hosted: Option<super::hosted_route::HostedRoute>,
    /// VK_KHR_present_wait entry points when the device enabled present ids.
    /// The direct backend paces its acquires on completed presents with them
    /// (see `DirectOutput::outstanding_presents`).
    #[cfg(linux_direct)]
    pub(super) present_wait: Option<ash::khr::present_wait::Device>,
}

/// Cleans up each successfully created parent when a later initialization step
/// fails. Ownership is transferred to CxVulkan only once the device exists.
struct DesktopInit {
    entry: Option<ash::Entry>,
    instance: Option<ash::Instance>,
    surface_loader: ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
    debug_utils: bool,
}

impl Drop for DesktopInit {
    fn drop(&mut self) {
        if let Some(instance) = self.instance.take() {
            unsafe { self.surface_loader.destroy_surface(self.surface, None) };
            unsafe { instance.destroy_instance(None) };
        }
    }
}

impl DesktopInit {
    fn new(platform_extensions: &[&CStr]) -> Result<Self, String> {
        let entry =
            unsafe { ash::Entry::load() }.map_err(|e| format!("Vulkan loader unavailable: {e}"))?;
        let available = unsafe { entry.enumerate_instance_extension_properties(None) }
            .map_err(|e| format!("enumerate Vulkan instance extensions: {e:?}"))?;
        let supports = |name: &CStr| {
            available
                .iter()
                .any(|ext| unsafe { CStr::from_ptr(ext.extension_name.as_ptr()) == name })
        };
        let mut extensions = vec![vk::KHR_SURFACE_NAME.as_ptr()];
        for extension in std::iter::once(&vk::KHR_SURFACE_NAME).chain(platform_extensions) {
            if !supports(extension) {
                return Err(format!(
                    "Vulkan driver is missing {}",
                    extension.to_string_lossy()
                ));
            }
        }
        extensions.extend(platform_extensions.iter().map(|name| name.as_ptr()));
        let debug_utils = supports(vk::EXT_DEBUG_UTILS_NAME);
        if debug_utils {
            extensions.push(vk::EXT_DEBUG_UTILS_NAME.as_ptr());
        }
        let validation = std::env::var_os("MAKEPAD_VULKAN_VALIDATION").is_some();
        let layers = if validation {
            let layers = unsafe { entry.enumerate_instance_layer_properties() }
                .map_err(|e| format!("enumerate Vulkan layers: {e:?}"))?;
            if !layers.iter().any(|layer| unsafe {
                CStr::from_ptr(layer.layer_name.as_ptr()).to_bytes()
                    == b"VK_LAYER_KHRONOS_validation"
            }) {
                return Err("MAKEPAD_VULKAN_VALIDATION requested but VK_LAYER_KHRONOS_validation is unavailable".into());
            }
            vec![c"VK_LAYER_KHRONOS_validation".as_ptr()]
        } else {
            Vec::new()
        };
        let application = vk::ApplicationInfo::default()
            .application_name(c"Makepad")
            .api_version(vk::API_VERSION_1_1);
        let mut info = vk::InstanceCreateInfo::default()
            .application_info(&application)
            .enabled_extension_names(&extensions)
            .enabled_layer_names(&layers);
        let mut debug_info = vulkan_debug_messenger_create_info();
        if debug_utils {
            info = info.push_next(&mut debug_info);
        }
        let instance = unsafe { entry.create_instance(&info, None) }
            .map_err(|e| format!("create Vulkan instance: {e:?}"))?;
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
        Ok(Self {
            entry: Some(entry),
            instance: Some(instance),
            surface_loader,
            surface: vk::SurfaceKHR::null(),
            debug_utils,
        })
    }

    fn devices(&self) -> Result<Vec<vk::PhysicalDevice>, String> {
        let instance = self.instance.as_ref().unwrap();
        let mut devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|e| format!("enumerate Vulkan devices: {e:?}"))?;
        devices.retain(|device| {
            let properties = unsafe { instance.get_physical_device_properties(*device) };
            properties.api_version >= vk::API_VERSION_1_1
                && unsafe { instance.enumerate_device_extension_properties(*device) }
                    .map(|extensions| {
                        extensions.iter().any(|extension| unsafe {
                            CStr::from_ptr(extension.extension_name.as_ptr())
                                == vk::KHR_SWAPCHAIN_NAME
                        })
                    })
                    .unwrap_or(false)
        });
        devices.sort_by_key(|device| device_type_rank(instance, *device));
        Ok(devices)
    }

    fn finish(
        mut self,
        physical_device: vk::PhysicalDevice,
        queue_family_index: u32,
        mut desktop: DesktopState,
        width: u32,
        height: u32,
    ) -> Result<CxVulkan, String> {
        let instance = self.instance.as_ref().unwrap();
        #[cfg(linux_direct)]
        {
            desktop.gpu.devices = self
                .devices()?
                .into_iter()
                .map(|device| gpu_identity(instance, device))
                .collect();
            desktop.gpu.device = gpu_identity(instance, physical_device);
            desktop.gpu.generation = 1;
        }
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&priorities)];
        desktop.shared.enabled = super::shared::device_supports_sharing(instance, physical_device);
        let mut extensions = vec![vk::KHR_SWAPCHAIN_NAME.as_ptr()];
        let mut timeline =
            vk::PhysicalDeviceTimelineSemaphoreFeatures::default().timeline_semaphore(true);
        if desktop.shared.enabled {
            extensions.extend([
                vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr(),
                vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME.as_ptr(),
                vk::KHR_TIMELINE_SEMAPHORE_NAME.as_ptr(),
            ]);
        }
        #[cfg(linux_direct)]
        if unsafe { super::dma_buf::supports_extensions(instance, physical_device) } {
            for name in super::dma_buf::REQUIRED_EXTENSIONS {
                if !extensions
                    .iter()
                    .any(|ptr| unsafe { CStr::from_ptr(*ptr) == name })
                {
                    extensions.push(name.as_ptr());
                }
            }
        }
        // Direct output only: with VK_KHR_present_id/present_wait the direct
        // backend asks for an image only after its previous present on that
        // swapchain completed, so a timeout-0 acquire never has to wait.
        #[cfg(linux_direct)]
        let present_wait =
            desktop.direct.is_some() && device_supports_present_wait(instance, physical_device);
        #[cfg(linux_direct)]
        let mut present_id_features =
            vk::PhysicalDevicePresentIdFeaturesKHR::default().present_id(true);
        #[cfg(linux_direct)]
        let mut present_wait_features =
            vk::PhysicalDevicePresentWaitFeaturesKHR::default().present_wait(true);
        #[cfg(linux_direct)]
        if present_wait {
            extensions.extend([
                vk::KHR_PRESENT_ID_NAME.as_ptr(),
                vk::KHR_PRESENT_WAIT_NAME.as_ptr(),
            ]);
        }
        let mut info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&extensions);
        if desktop.shared.enabled {
            info = info.push_next(&mut timeline);
        }
        #[cfg(linux_direct)]
        if present_wait {
            info = info.push_next(&mut present_id_features);
            info = info.push_next(&mut present_wait_features);
        }
        let device = match unsafe { instance.create_device(physical_device, &info, None) } {
            Ok(device) => device,
            Err(e) => {
                // Instance-level direct-display handles inside `desktop` must be
                // released while this instance is still alive.
                drop(desktop);
                return Err(format!("create Vulkan device: {e:?}"));
            }
        };
        let props = unsafe { instance.get_physical_device_properties(physical_device) };
        let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy();
        crate::log!("Vulkan: {name}, graphics/present queue {queue_family_index}");
        let recycle_pass_resources =
            std::env::var("MAKEPAD_VULKAN_RECYCLE").ok().as_deref() != Some("0");
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let swapchain_loader = ash::khr::swapchain::Device::new(instance, &device);
        #[cfg(linux_direct)]
        if present_wait {
            desktop.present_wait = Some(ash::khr::present_wait::Device::new(instance, &device));
            crate::log!(
                "Vulkan direct: VK_KHR_present_id/present_wait enabled; acquires wait for the previous present to complete"
            );
        }
        let mut renderer = CxVulkan {
            frame_serial_in_flight: 0,
            _entry: self.entry.take().unwrap(),
            instance: self.instance.take().unwrap(),
            desktop,
            surface_loader: self.surface_loader.clone(),
            surface: std::mem::take(&mut self.surface),
            physical_device,
            queue_family_index,
            min_uniform_buffer_offset_alignment: props
                .limits
                .min_uniform_buffer_offset_alignment
                .max(4),
            device,
            queue,
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            swapchain_images: Vec::new(),
            swapchain_image_views: Vec::new(),
            swapchain_depth_targets: Vec::new(),
            swapchain_readback_buffer: None,
            swapchain_format: vk::Format::UNDEFINED,
            depth_format: vk::Format::UNDEFINED,
            swapchain_extent: vk::Extent2D::default(),
            render_pass: vk::RenderPass::null(),
            xr_render_pass: vk::RenderPass::null(),
            framebuffers: Vec::new(),
            pipelines: HashMap::new(),
            offscreen_render_passes: HashMap::new(),
            geometries: HashMap::new(),
            textures: HashMap::new(),
            frame_resources: FrameResources::default(),
            command_pool: vk::CommandPool::null(),
            command_buffer: vk::CommandBuffer::null(),
            image_available_semaphore: vk::Semaphore::null(),
            render_finished_semaphores: Vec::new(),
            acquired_image_pending: false,
            in_flight_fence: vk::Fence::null(),
            requested_width: width,
            requested_height: height,
            texture_upload_count_this_frame: 0,
            texture_upload_bytes_this_frame: 0,
            xr_packet_buffer_count_this_frame: 0,
            xr_packet_buffer_bytes_this_frame: 0,
            xr_geometry_upload_bytes_this_frame: 0,
            xr_descriptor_set_count_this_frame: 0,
            debug_utils_enabled: self.debug_utils,
            debug_utils_loader: None,
            debug_messenger: vk::DebugUtilsMessengerEXT::null(),
            xr_depth_dummy: None,
            recycle_pass_resources,
            profile: vulkan_profile::VulkanProfile::from_env(recycle_pass_resources),
        };
        // From this point CxVulkan::drop covers every partially constructed resource.
        renderer.command_pool = unsafe {
            renderer.device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family_index)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }
        .map_err(|e| format!("create Vulkan command pool: {e:?}"))?;
        renderer.command_buffer = unsafe {
            renderer.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(renderer.command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .map_err(|e| format!("allocate Vulkan command buffer: {e:?}"))?[0];
        renderer.image_available_semaphore = unsafe {
            renderer
                .device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        }
        .map_err(|e| format!("create Vulkan acquire semaphore: {e:?}"))?;
        renderer.in_flight_fence = unsafe {
            renderer.device.create_fence(
                &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )
        }
        .map_err(|e| format!("create Vulkan fence: {e:?}"))?;
        renderer.try_enable_debug_messenger();
        renderer.initialize_depth_dummies()?;
        renderer.recreate_swapchain()?;
        Ok(renderer)
    }
}

fn device_type_rank(instance: &ash::Instance, device: vk::PhysicalDevice) -> u8 {
    let properties = unsafe { instance.get_physical_device_properties(device) };
    match properties.device_type {
        vk::PhysicalDeviceType::DISCRETE_GPU => 0,
        vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
        _ => 2,
    }
}

/// Both extensions offered and both features reported: present ids can be
/// attached to presents and waited for with vkWaitForPresentKHR.
#[cfg(linux_direct)]
fn device_supports_present_wait(instance: &ash::Instance, device: vk::PhysicalDevice) -> bool {
    let Ok(extensions) = (unsafe { instance.enumerate_device_extension_properties(device) }) else {
        return false;
    };
    let has = |name: &CStr| {
        extensions
            .iter()
            .any(|extension| unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) == name })
    };
    if !has(vk::KHR_PRESENT_ID_NAME) || !has(vk::KHR_PRESENT_WAIT_NAME) {
        return false;
    }
    let mut present_id = vk::PhysicalDevicePresentIdFeaturesKHR::default();
    let mut present_wait = vk::PhysicalDevicePresentWaitFeaturesKHR::default();
    let mut features = vk::PhysicalDeviceFeatures2::default()
        .push_next(&mut present_id)
        .push_next(&mut present_wait);
    unsafe { instance.get_physical_device_features2(device, &mut features) };
    present_id.present_id != 0 && present_wait.present_wait != 0
}

fn device_uuid(instance: &ash::Instance, device: vk::PhysicalDevice) -> [u8; 16] {
    let mut ids = vk::PhysicalDeviceIDProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
    unsafe { instance.get_physical_device_properties2(device, &mut properties) };
    ids.device_uuid
}

#[cfg(linux_direct)]
fn gpu_identity(
    instance: &ash::Instance,
    device: vk::PhysicalDevice,
) -> crate::linux_gpu::LinuxGpuDevice {
    let pci_supported = unsafe { instance.enumerate_device_extension_properties(device) }
        .map(|extensions| {
            extensions.iter().any(|extension| unsafe {
                CStr::from_ptr(extension.extension_name.as_ptr()) == vk::EXT_PCI_BUS_INFO_NAME
            })
        })
        .unwrap_or(false);
    let mut ids = vk::PhysicalDeviceIDProperties::default();
    let mut pci = vk::PhysicalDevicePCIBusInfoPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
    if pci_supported {
        properties = properties.push_next(&mut pci);
    }
    unsafe { instance.get_physical_device_properties2(device, &mut properties) };
    let base = properties.properties;
    crate::linux_gpu::LinuxGpuDevice {
        uuid: ids.device_uuid,
        driver_uuid: ids.driver_uuid,
        name: unsafe { CStr::from_ptr(base.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
        vendor_id: base.vendor_id,
        device_id: base.device_id,
        pci_address: pci_supported.then(|| {
            format!(
                "{:04x}:{:02x}:{:02x}.{:x}",
                pci.pci_domain, pci.pci_bus, pci.pci_device, pci.pci_function
            )
        }),
    }
}

fn uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_uuid_hex(text: &str) -> Option<[u8; 16]> {
    let text = text.trim();
    if text.len() != 32 {
        return None;
    }
    let mut uuid = [0u8; 16];
    for (index, chunk) in text.as_bytes().chunks(2).enumerate() {
        uuid[index] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(uuid)
}

impl CxVulkan {
    /// Hosted WM apps need a graphics queue, but no display or DRM master.
    /// They follow the direct renderer's GPU through `MAKEPAD_VULKAN_DEVICE_UUID`
    /// so their shared frames stay on the device that presents them.
    pub fn new_offscreen() -> Result<Self, String> {
        let pin = std::env::var(LINUX_VULKAN_DEVICE_ENV).ok();
        let uuid = pin
            .as_deref()
            .map(|pin| {
                parse_uuid_hex(pin).ok_or_else(|| {
                    format!("{LINUX_VULKAN_DEVICE_ENV}={pin:?} is not 32 hex digits")
                })
            })
            .transpose()?;
        Self::new_offscreen_on(uuid)
    }

    /// Explicit device selection also supports renderer recreation without
    /// mutating the environment of unrelated threads or child processes.
    pub(super) fn new_offscreen_on(uuid: Option<[u8; 16]>) -> Result<Self, String> {
        let init = DesktopInit::new(&[])?;
        let instance = init.instance.as_ref().unwrap();
        let mut devices = init.devices()?;
        // An explicit pin is a contract: a frame rendered on any other GPU
        // cannot be shared with the compositor that asked for this one, so a
        // malformed or unavailable pin is an error, never a silent fallback.
        if let Some(uuid) = uuid {
            let index = devices
                .iter()
                .position(|device| device_uuid(instance, *device) == uuid)
                .ok_or_else(|| {
                    format!(
                        "{LINUX_VULKAN_DEVICE_ENV}={} names no usable Vulkan 1.1 device with swapchain support",
                        uuid_hex(&uuid)
                    )
                })?;
            devices = vec![devices.remove(index)];
            crate::log!(
                "Vulkan: hosted renderer follows {LINUX_VULKAN_DEVICE_ENV}={}",
                uuid_hex(&uuid)
            );
        }
        for device in devices {
            let queues = unsafe { instance.get_physical_device_queue_family_properties(device) };
            if let Some(index) = queues
                .iter()
                .position(|queue| queue.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            {
                return init.finish(device, index as u32, DesktopState::default(), 0, 0);
            }
        }
        Err("No Vulkan 1.1 graphics queue for a hosted app".into())
    }

    pub fn new_wayland(display: *mut c_void) -> Result<Self, String> {
        if display.is_null() {
            return Err("Vulkan: null Wayland display".into());
        }
        let init = DesktopInit::new(&[vk::KHR_WAYLAND_SURFACE_NAME])?;
        let instance = init.instance.as_ref().unwrap();
        let loader =
            ash::khr::wayland_surface::Instance::new(init.entry.as_ref().unwrap(), instance);
        for physical_device in init.devices()? {
            let queues =
                unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
            for (index, queue) in queues.iter().enumerate() {
                let supports_present = queue.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                    && unsafe {
                        (loader
                            .fp()
                            .get_physical_device_wayland_presentation_support_khr)(
                            physical_device,
                            index as u32,
                            display.cast(),
                        ) != vk::FALSE
                    };
                if supports_present {
                    return init.finish(
                        physical_device,
                        index as u32,
                        DesktopState {
                            wayland: Some((loader, display)),
                            ..Default::default()
                        },
                        0,
                        0,
                    );
                }
            }
        }
        Err("No Vulkan 1.1 device with Wayland graphics/presentation support".into())
    }

    fn take_desktop_window(&mut self) -> DesktopWindow {
        DesktopWindow {
            native_surface: std::mem::take(&mut self.desktop.native_surface),
            surface: std::mem::take(&mut self.surface),
            swapchain: std::mem::take(&mut self.swapchain),
            images: std::mem::take(&mut self.swapchain_images),
            views: std::mem::take(&mut self.swapchain_image_views),
            depth_targets: std::mem::take(&mut self.swapchain_depth_targets),
            readback: self.swapchain_readback_buffer.take(),
            format: std::mem::take(&mut self.swapchain_format),
            depth_format: std::mem::take(&mut self.depth_format),
            extent: std::mem::take(&mut self.swapchain_extent),
            render_pass: std::mem::take(&mut self.render_pass),
            xr_render_pass: std::mem::take(&mut self.xr_render_pass),
            framebuffers: std::mem::take(&mut self.framebuffers),
            present_semaphores: std::mem::take(&mut self.render_finished_semaphores),
            width: std::mem::take(&mut self.requested_width),
            height: std::mem::take(&mut self.requested_height),
        }
    }

    fn put_desktop_window(&mut self, window: DesktopWindow) {
        self.desktop.native_surface = window.native_surface;
        self.surface = window.surface;
        self.swapchain = window.swapchain;
        self.swapchain_images = window.images;
        self.swapchain_image_views = window.views;
        self.swapchain_depth_targets = window.depth_targets;
        self.swapchain_readback_buffer = window.readback;
        self.swapchain_format = window.format;
        self.depth_format = window.depth_format;
        self.swapchain_extent = window.extent;
        self.render_pass = window.render_pass;
        self.xr_render_pass = window.xr_render_pass;
        self.framebuffers = window.framebuffers;
        self.render_finished_semaphores = window.present_semaphores;
        self.requested_width = window.width;
        self.requested_height = window.height;
    }

    pub fn ensure_wayland_window(
        &mut self,
        window_id: WindowId,
        surface: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if surface.is_null() {
            return Err("Vulkan: null Wayland surface".into());
        }
        if self.desktop.wayland.is_none() {
            return Err("Vulkan renderer is not using Wayland".into());
        }
        if self.desktop.selected != Some(window_id) {
            if let Some(previous) = self.desktop.selected.take() {
                let window = self.take_desktop_window();
                self.desktop
                    .windows
                    .insert((previous.0, previous.1), window);
            }
            let window = self
                .desktop
                .windows
                .remove(&(window_id.0, window_id.1))
                .unwrap_or_default();
            self.put_desktop_window(window);
            self.desktop.selected = Some(window_id);
        }
        if self.desktop.native_surface != surface || self.surface == vk::SurfaceKHR::null() {
            self.device_wait_idle();
            self.destroy_swapchain();
            self.destroy_surface();
            let (loader, display) = self.desktop.wayland.as_ref().unwrap();
            self.surface = unsafe {
                loader.create_wayland_surface(
                    &vk::WaylandSurfaceCreateInfoKHR::default()
                        .display((*display).cast())
                        .surface(surface.cast()),
                    None,
                )
            }
            .map_err(|e| format!("create Vulkan Wayland surface: {e:?}"))?;
            self.desktop.native_surface = surface;
            let supported = unsafe {
                self.surface_loader.get_physical_device_surface_support(
                    self.physical_device,
                    self.queue_family_index,
                    self.surface,
                )
            }
            .map_err(|e| format!("query Wayland Vulkan surface support: {e:?}"))?;
            if !supported {
                return Err("Selected Vulkan device cannot present to this Wayland surface".into());
            }
        }
        let resize = self.requested_width != width || self.requested_height != height;
        self.requested_width = width;
        self.requested_height = height;
        if width == 0 || height == 0 {
            self.device_wait_idle();
            self.destroy_swapchain();
        } else if resize || self.swapchain == vk::SwapchainKHR::null() {
            self.recreate_swapchain()?;
        }
        Ok(())
    }

    pub fn remove_wayland_window(&mut self, window_id: WindowId) {
        self.device_wait_idle();
        if self.desktop.selected == Some(window_id) {
            self.desktop.selected = None;
            self.destroy_swapchain();
            self.destroy_surface();
            self.desktop.native_surface = std::ptr::null_mut();
        } else if let Some(window) = self.desktop.windows.remove(&(window_id.0, window_id.1)) {
            let current = self.take_desktop_window();
            self.put_desktop_window(window);
            self.destroy_swapchain();
            self.destroy_surface();
            self.put_desktop_window(current);
        }
    }

    pub(super) fn destroy_desktop_windows(&mut self) {
        let windows: Vec<_> = self.desktop.windows.keys().copied().collect();
        for window in windows {
            self.remove_wayland_window(WindowId(window.0, window.1));
        }
    }

    /// The main render target size. In direct mode this is the composition:
    /// the render source output's native mode.
    pub fn size(&self) -> (u32, u32) {
        (self.swapchain_extent.width, self.swapchain_extent.height)
    }
}

// ---------------------------------------------------------------------------
// Direct display (DRM/KMS) output: one composition image rendered at the
// source output's native mode, presented to every acquired connector of the
// same GPU through a sampled fullscreen pass. Outputs acquire and present
// independently: a slow clone never gates the source, and a source that is
// not ready skips the frame (the pass stays dirty and is retried on a short,
// input-interruptible deadline).
// ---------------------------------------------------------------------------

/// Composition format. UNORM so the blit copies display-encoded values as-is;
/// an sRGB attachment would encode them a second time.
#[cfg(linux_direct)]
const COMPOSITION_FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;
#[cfg(linux_direct)]
const HOTPLUG_POLL: Duration = Duration::from_secs(1);
#[cfg(linux_direct)]
const RETRY_INITIAL: Duration = Duration::from_secs(2);
#[cfg(linux_direct)]
const RETRY_MAX: Duration = Duration::from_secs(30);
/// An acquired image whose fence has not signaled for this long is treated as
/// a lost presentation: the fence is retired and the swapchain recreated, so a
/// dead connector is detected through a fresh acquire instead of polled forever.
#[cfg(linux_direct)]
const ACQUIRE_WATCHDOG: Duration = Duration::from_secs(2);
#[cfg(linux_direct)]
const ACQUIRE_TEARDOWN_TIMEOUT_NS: u64 = 1_000_000_000;
#[cfg(linux_direct)]
const RETIRED_FENCE_TEARDOWN_NS: u64 = 500_000_000;
#[cfg(linux_direct)]
const SOURCE_SWITCH_RETRIES: u8 = 3;
#[cfg(linux_direct)]
const SOURCE_RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// Kernel events naming an output we just reacquired are our own modeset's
/// echo on some drivers; ignore them for this long to avoid a reacquire loop.
#[cfg(linux_direct)]
const REACQUIRE_SUPPRESS: Duration = Duration::from_secs(1);

#[cfg(linux_direct)]
const BLIT_WGSL: &str = r#"
struct BlitVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> BlitVertex {
    var out: BlitVertex;
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    out.uv = uv;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@fragment
fn fragment_main(input: BlitVertex) -> @location(0) vec4<f32> {
    return textureSample(source, source_sampler, input.uv);
}
"#;

/// Why the last frame attempt did not present; drives event-loop pacing.
#[cfg(linux_direct)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectWait {
    /// Presented, or nothing is blocking a presentation.
    Ready,
    /// The source output's image or the frame fence is still in flight; retry
    /// on a short, input-interruptible deadline.
    GpuBusy,
    /// No output can show a frame; keep the dirty pass and wait normally.
    NoOutput,
}

#[cfg(linux_direct)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct DrmConnector {
    card: PathBuf,
    name: String,
    id: u32,
    /// Hash of the sysfs EDID and mode list: a replaced monitor on the same
    /// connector changes it even though the name does not.
    fingerprint: u64,
}

/// A DRM hotplug/property uevent narrowed to what it names.
#[cfg(linux_direct)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct DrmEvent {
    card: Option<String>,
    connector_id: Option<u32>,
    /// `HOTPLUG=1`: a cable/link change, as opposed to a property update.
    hotplug: bool,
}

/// One worker publication: the current connector list plus the kernel events
/// seen since the previous publication (possibly none, possibly with an
/// unchanged list).
#[cfg(linux_direct)]
#[derive(Clone, Debug)]
struct DrmScan {
    connectors: Vec<DrmConnector>,
    events: Vec<DrmEvent>,
}

#[cfg(linux_direct)]
#[derive(Clone, Default)]
struct DrmFilter {
    card: Option<String>,
    connector: Option<String>,
}

#[cfg(linux_direct)]
impl DrmFilter {
    fn from_env() -> Self {
        Self {
            card: std::env::var("MAKEPAD_DRM_DEVICE")
                .ok()
                .filter(|v| !v.is_empty()),
            connector: std::env::var("MAKEPAD_DRM_CONNECTOR")
                .ok()
                .filter(|v| !v.is_empty()),
        }
    }
}

/// One DRM card node, opened once and shared by every connector it drives.
/// The descriptor must outlive vkReleaseDisplayEXT for each display on it.
#[cfg(linux_direct)]
struct DrmCard {
    path: PathBuf,
    file: File,
}

/// A display acquired and given a plane surface before the device exists.
#[cfg(linux_direct)]
struct PendingOutput {
    connector: DrmConnector,
    display: vk::DisplayKHR,
    surface: vk::SurfaceKHR,
    plane_index: u32,
    display_mode: vk::DisplayModeKHR,
    extent: vk::Extent2D,
    refresh_mhz: u32,
}

/// A fence handed to vkAcquireNextImageKHR that never signaled. It cannot be
/// reset or destroyed while pending, so it is parked here and destroyed once
/// it signals (or leaked with a warning at teardown).
#[cfg(linux_direct)]
struct RetiredFence {
    fence: vk::Fence,
    since: Instant,
}

#[cfg(linux_direct)]
struct DirectOutput {
    connector: DrmConnector,
    display: vk::DisplayKHR,
    surface: vk::SurfaceKHR,
    plane_index: u32,
    /// The selected mode, kept so the surface can move to another plane.
    display_mode: vk::DisplayModeKHR,
    mode_extent: vk::Extent2D,
    refresh_mhz: u32,
    swapchain: vk::SwapchainKHR,
    format: vk::Format,
    extent: vk::Extent2D,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    present_semaphores: Vec<vk::Semaphore>,
    /// Signaled by the presentation engine once the acquired image is free.
    /// Fence-only acquisition keeps every queue submission free of WSI
    /// semaphore waits, so one output's vblank cannot stall another's blit.
    acquire_fence: vk::Fence,
    /// Signaled when this output's last blit finished on the GPU; guards the
    /// command buffer and descriptor set reuse. Not presentation completion.
    submit_fence: vk::Fence,
    command_buffer: vk::CommandBuffer,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    descriptor_epoch: u64,
    acquired: Option<u32>,
    acquired_at: Option<Instant>,
    /// First NOT_READY while a newer composition is waiting; a presentation
    /// engine that never hands back an image is reacquired after the watchdog.
    no_image_since: Option<Instant>,
    /// VK_KHR_present_id pacing (Mesa display WSI). The id attached to the
    /// next present, and the presents not yet completed (id, when). Before
    /// an acquire, presents beyond `image_count - 2` outstanding are waited
    /// for with timeout 0: one image is being displayed, at most
    /// `image_count - 2` are queued, so an idle image always exists and the
    /// acquire never has to wait inside the driver. Reset per swapchain.
    next_present_id: u64,
    outstanding_presents: Vec<(u64, Instant)>,
    present_wait_logged: bool,
    presented_generation: u64,
    /// At least one vkQueuePresentKHR on the CURRENT swapchain succeeded.
    /// This is what the UI sees as `active`; reset on every recreation.
    presented_ok: bool,
    needs_recreate: bool,
    /// A kernel event named this connector: release and reacquire it.
    reacquire: bool,
    /// When this entry was (re)acquired after a kernel event or watchdog;
    /// kernel events inside REACQUIRE_SUPPRESS are treated as our own echo.
    reacquired_at: Option<Instant>,
    /// This output is the render source: the composition matches its mode.
    primary: bool,
    /// Internal readiness: the output owns a swapchain and can be presented to.
    active: bool,
    status: String,
    retry_at: Option<Instant>,
    retry_interval: Duration,
}

#[cfg(linux_direct)]
impl DirectOutput {
    fn inactive(connector: DrmConnector, status: String, retry_interval: Duration) -> Self {
        Self {
            connector,
            display: vk::DisplayKHR::null(),
            surface: vk::SurfaceKHR::null(),
            plane_index: 0,
            display_mode: vk::DisplayModeKHR::null(),
            mode_extent: vk::Extent2D::default(),
            refresh_mhz: 0,
            swapchain: vk::SwapchainKHR::null(),
            format: vk::Format::UNDEFINED,
            extent: vk::Extent2D::default(),
            images: Vec::new(),
            views: Vec::new(),
            framebuffers: Vec::new(),
            present_semaphores: Vec::new(),
            acquire_fence: vk::Fence::null(),
            submit_fence: vk::Fence::null(),
            command_buffer: vk::CommandBuffer::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            descriptor_set: vk::DescriptorSet::null(),
            descriptor_epoch: 0,
            acquired: None,
            acquired_at: None,
            no_image_since: None,
            next_present_id: 1,
            outstanding_presents: Vec::new(),
            present_wait_logged: false,
            presented_generation: 0,
            presented_ok: false,
            needs_recreate: false,
            reacquire: false,
            reacquired_at: None,
            primary: false,
            active: false,
            status,
            retry_at: Some(Instant::now() + retry_interval),
            retry_interval,
        }
    }

    fn refresh_hz(&self) -> f64 {
        self.refresh_mhz as f64 / 1000.0
    }

    fn card_name(&self) -> String {
        connector_card_name(&self.connector.card)
    }
}

#[cfg(linux_direct)]
struct BlitPipeline {
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
}

#[cfg(linux_direct)]
struct BlitResources {
    sampler: vk::Sampler,
    descriptor_set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    vertex_spirv: Vec<u32>,
    fragment_spirv: Vec<u32>,
    /// Keyed by the output's swapchain format (`vk::Format::as_raw`).
    pipelines: HashMap<i32, BlitPipeline>,
}

/// A fully built replacement composition, installed only once every part
/// exists so a failed allocation never leaves the renderer without a target.
#[cfg(linux_direct)]
struct CompositionTargets {
    resource: VulkanTextureResource,
    render_pass: vk::RenderPass,
    depth: VulkanTextureResource,
    framebuffer: vk::Framebuffer,
    readback: VulkanBuffer,
    depth_format: vk::Format,
    extent: vk::Extent2D,
}

/// The compositor and its apps use `CxVulkan`'s device. Display ownership stays
/// in a second context, so recreating the renderer never drops the Intel panel.
#[cfg(linux_direct)]
pub(super) struct RoutedCompositor {
    display: Box<CxVulkan>,
    composition: Option<VulkanTextureResource>,
    bridge: Option<super::gpu_bridge::GpuBridge>,
    composition_valid: bool,
    resend: bool,
    failure: Option<String>,
    retry_at: Option<Instant>,
    wait: DirectWait,
}

/// Candidate resources must retire while candidate and presenter are alive.
#[cfg(linux_direct)]
pub(super) struct PreparedOutputRoute {
    composition: VulkanTextureResource,
    bridge: super::gpu_bridge::GpuBridge,
}

#[cfg(linux_direct)]
impl CxVulkan {
    pub(super) fn gpu_display_identity(&self) -> Option<crate::linux_gpu::LinuxGpuDevice> {
        if let Some(routed) = &self.desktop.routed {
            return Some(routed.display.desktop.gpu.device.clone());
        }
        self.desktop
            .direct
            .as_ref()
            .map(|_| self.desktop.gpu.device.clone())
    }

    pub(super) fn freeze_gpu_output_size(&mut self, frozen: bool) {
        if let Some(direct) = &mut self.desktop.direct {
            direct.defer_composition_resize = frozen;
            if !frozen {
                direct.composition_resize_request = None;
            }
        }
    }

    pub(super) fn gpu_output_idle(&mut self) -> Result<bool, String> {
        let Some(mut routed) = self.desktop.routed.take() else {
            return Ok(true);
        };
        let result = match routed.bridge.as_mut() {
            Some(bridge) => unsafe { bridge.writable(self, &routed.display) },
            None => Err("compositor output bridge unavailable".into()),
        };
        self.desktop.routed = Some(routed);
        result
    }

    pub(super) fn prepare_gpu_output(
        &mut self,
        old: &CxVulkan,
    ) -> Result<Option<PreparedOutputRoute>, String> {
        let display = if let Some(routed) = &old.desktop.routed {
            &*routed.display
        } else if old.desktop.direct.is_some() {
            old
        } else {
            return Ok(None);
        };
        let extent = display.swapchain_extent;
        let targets = self.direct_build_composition(extent)?;
        // Candidate resource copying has completed. Install its unused target
        // before submitting the bridge initializer; installation's old-target
        // retirement must not wait on that newly submitted initialization.
        let composition = self.install_composition_targets(targets);
        let bridge = match super::gpu_bridge::GpuBridge::new(self, display, extent) {
            Ok(bridge) => bridge,
            Err(error) => {
                self.destroy_swapchain();
                self.destroy_texture_resource(composition);
                return Err(error);
            }
        };
        Ok(Some(PreparedOutputRoute {
            composition,
            bridge,
        }))
    }

    pub(super) fn destroy_prepared_gpu_output(
        &mut self,
        old: &CxVulkan,
        output: PreparedOutputRoute,
    ) {
        let display = old
            .desktop
            .routed
            .as_ref()
            .map(|routed| &*routed.display)
            .unwrap_or(old);
        if !unsafe { output.bridge.idle(self, display) }.unwrap_or(false) {
            self.device_wait_idle();
            display.device_wait_idle();
        }
        unsafe { output.bridge.destroy(self, display) };
        self.destroy_swapchain();
        self.destroy_texture_resource(output.composition);
    }

    pub(super) fn prepared_gpu_output_idle(
        &self,
        old: &CxVulkan,
        output: &PreparedOutputRoute,
    ) -> Result<bool, String> {
        let display = old
            .desktop
            .routed
            .as_ref()
            .map(|routed| &*routed.display)
            .unwrap_or(old);
        unsafe { output.bridge.idle(self, display) }
    }

    pub(super) fn prepare_gpu_output_ready(
        &self,
        old: &CxVulkan,
        output: &mut PreparedOutputRoute,
    ) -> Result<bool, String> {
        let display = old
            .desktop
            .routed
            .as_ref()
            .map(|routed| &*routed.display)
            .unwrap_or(old);
        unsafe { output.bridge.poll_initialization(self, display) }
    }

    pub(super) fn install_gpu_output(
        &mut self,
        mut old: CxVulkan,
        output: PreparedOutputRoute,
    ) -> Option<super::transition::RetiredRenderer> {
        use super::transition::RetiredRenderer;
        old.device_wait_idle();
        let (display, retired) = if let Some(mut routed) = old.desktop.routed.take() {
            routed.display.device_wait_idle();
            if let Some(bridge) = routed.bridge.take() {
                unsafe { bridge.destroy(&old, &routed.display) };
            }
            // Old framebuffers/views must retire before their composition.
            old.destroy_swapchain();
            if let Some(composition) = routed.composition.take() {
                old.destroy_texture_resource(composition);
            }
            (
                routed.display,
                Some(RetiredRenderer::Separate(Box::new(old))),
            )
        } else {
            old.desktop
                .direct
                .as_mut()
                .expect("prepared direct presenter")
                .defer_composition_resize = true;
            (Box::new(old), Some(RetiredRenderer::PresenterCache))
        };
        self.desktop.routed = Some(RoutedCompositor {
            display,
            composition: Some(output.composition),
            bridge: Some(output.bridge),
            composition_valid: false,
            resend: false,
            failure: None,
            retry_at: None,
            wait: DirectWait::GpuBusy,
        });
        retired
    }

    pub(super) fn retire_presenter_app_cache(&mut self) {
        if let Some(routed) = &mut self.desktop.routed {
            let display = &mut routed.display;
            display.device_wait_idle();
            display.destroy_frame_resources();
            display.destroy_pipelines();
            display.destroy_geometry_resources();
            display.destroy_shared_state();
            display.destroy_texture_resources();
        }
    }
}

#[cfg(linux_direct)]
struct HotplugWatch {
    receiver: ToUIReceiver<DrmScan>,
    stop: Arc<AtomicBool>,
    _handle: TaskHandle<()>,
}

#[cfg(linux_direct)]
impl Drop for HotplugWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(linux_direct)]
pub(super) struct DirectState {
    physical_device: vk::PhysicalDevice,
    display_loader: ash::khr::display::Instance,
    drm_loader: ash::ext::acquire_drm_display::Instance,
    release_loader: ash::ext::direct_mode_display::Instance,
    surface_loader: ash::khr::surface::Instance,
    filter: DrmFilter,
    cards: Vec<DrmCard>,
    /// Acquired before the device exists; converted to outputs afterwards.
    pending: Vec<PendingOutput>,
    outputs: Vec<DirectOutput>,
    /// Connected connectors this renderer cannot drive, with the reason.
    unsupported: Vec<(DrmConnector, String)>,
    /// Latest connector scan (init or hotplug worker).
    connected: Vec<DrmConnector>,
    scan_dirty: bool,
    /// The output the UI asked to render for; kept across reconnects.
    preferred_source: Option<String>,
    /// A source request accepted but not yet applied at a frame boundary.
    pending_source: Option<String>,
    source_name: Option<String>,
    source_switch_failures: u8,
    source_retry_at: Option<Instant>,
    /// A routed presenter proposes a new extent; both GPUs commit it together.
    defer_composition_resize: bool,
    composition_resize_request: Option<vk::Extent2D>,
    composition: Option<VulkanTextureResource>,
    /// Bumped when the composition image is recreated (descriptor rebinds).
    composition_epoch: u64,
    /// Bumped per rendered frame; outputs present when they are behind it.
    composition_generation: u64,
    composition_valid: bool,
    desktop_extent: vk::Extent2D,
    blit: Option<BlitResources>,
    retired_fences: Vec<RetiredFence>,
    hotplug: Option<HotplugWatch>,
    wait: DirectWait,
}

#[cfg(linux_direct)]
impl Drop for DirectState {
    /// Instance-level teardown only. Device-level resources were destroyed by
    /// `CxVulkan::destroy_direct_device_resources` before the device went away.
    fn drop(&mut self) {
        self.hotplug.take();
        for pending in std::mem::take(&mut self.pending) {
            self.release_surface_and_display(pending.surface, pending.display);
        }
        for output in std::mem::take(&mut self.outputs) {
            self.release_surface_and_display(output.surface, output.display);
        }
        // Card descriptors close after every display on them was released.
        self.cards.clear();
    }
}

#[cfg(linux_direct)]
impl DirectState {
    fn release_surface_and_display(&self, surface: vk::SurfaceKHR, display: vk::DisplayKHR) {
        if surface != vk::SurfaceKHR::null() {
            unsafe { self.surface_loader.destroy_surface(surface, None) };
        }
        if display != vk::DisplayKHR::null() {
            let result = unsafe {
                (self.release_loader.fp().release_display_ext)(self.physical_device, display)
            };
            if result != vk::Result::SUCCESS {
                crate::warning!("Vulkan: release DRM display failed: {result:?}");
            }
        }
    }

    fn card_fd(&mut self, path: &Path) -> Result<i32, String> {
        if let Some(card) = self.cards.iter().find(|card| card.path == path) {
            return Ok(card.file.as_raw_fd());
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let fd = file.as_raw_fd();
        self.cards.push(DrmCard {
            path: path.to_path_buf(),
            file,
        });
        Ok(fd)
    }

    /// Drop card descriptors no acquired display needs any more.
    fn prune_cards(&mut self) {
        let used: HashSet<PathBuf> = self
            .pending
            .iter()
            .map(|pending| pending.connector.card.clone())
            .chain(
                self.outputs
                    .iter()
                    .filter(|output| output.display != vk::DisplayKHR::null())
                    .map(|output| output.connector.card.clone()),
            )
            .collect();
        self.cards.retain(|card| used.contains(&card.path));
    }

    fn reserved_planes(&self) -> HashSet<u32> {
        self.pending
            .iter()
            .map(|pending| pending.plane_index)
            .chain(
                self.outputs
                    .iter()
                    .filter(|output| output.surface != vk::SurfaceKHR::null())
                    .map(|output| output.plane_index),
            )
            .collect()
    }

    fn source_index(&self) -> Option<usize> {
        self.outputs
            .iter()
            .position(|output| output.active && output.primary)
    }

    /// Queue family able to present to every pending surface, else to the
    /// first (best ranked) one; surfaces the chosen family cannot present to
    /// are released and reported.
    fn pick_queue_family(
        &mut self,
        instance: &ash::Instance,
        surface_loader: &ash::khr::surface::Instance,
    ) -> Result<u32, String> {
        let families =
            unsafe { instance.get_physical_device_queue_family_properties(self.physical_device) };
        let mut best: Option<(usize, u32)> = None;
        for (index, family) in families.iter().enumerate() {
            if !family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                continue;
            }
            let supports = |surface: vk::SurfaceKHR| unsafe {
                surface_loader
                    .get_physical_device_surface_support(
                        self.physical_device,
                        index as u32,
                        surface,
                    )
                    .unwrap_or(false)
            };
            if !supports(self.pending[0].surface) {
                continue;
            }
            let count = self
                .pending
                .iter()
                .filter(|pending| supports(pending.surface))
                .count();
            if best.is_none_or(|(best_count, _)| count > best_count) {
                best = Some((count, index as u32));
            }
            if count == self.pending.len() {
                break;
            }
        }
        let Some((_, family)) = best else {
            return Err("No Vulkan graphics queue family can present to the DRM display".into());
        };
        let pending = std::mem::take(&mut self.pending);
        for output in pending {
            let supported = unsafe {
                surface_loader
                    .get_physical_device_surface_support(
                        self.physical_device,
                        family,
                        output.surface,
                    )
                    .unwrap_or(false)
            };
            if supported {
                self.pending.push(output);
            } else {
                crate::warning!(
                    "Vulkan direct: {} cannot be presented from queue family {family}; skipped",
                    output.connector.name
                );
                self.release_surface_and_display(output.surface, output.display);
                self.unsupported.push((
                    output.connector,
                    format!("unsupported: graphics queue family {family} cannot present to this connector"),
                ));
            }
        }
        Ok(family)
    }
}

/// Built-in panels first (the laptop screen is the natural desktop), then by name.
#[cfg(linux_direct)]
fn connector_rank(name: &str) -> (u8, String) {
    let kind = name.split_once('-').map(|(_, rest)| rest).unwrap_or(name);
    let builtin = ["eDP", "LVDS", "DSI", "DPI"]
        .iter()
        .any(|prefix| kind.starts_with(prefix));
    (if builtin { 0 } else { 1 }, name.to_owned())
}

#[cfg(linux_direct)]
fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// EDID + mode list hash for one sysfs connector directory. A monitor swap
/// on the same connector changes it; an identical replug does not (that case
/// is covered by connector-level kernel events and the acquire watchdog).
#[cfg(linux_direct)]
fn connector_fingerprint(dir: &Path) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    if let Ok(edid) = std::fs::read(dir.join("edid")) {
        hash = fnv1a(hash, &edid);
    }
    if let Ok(modes) = std::fs::read(dir.join("modes")) {
        hash = fnv1a(hash, &modes);
    }
    hash
}

/// Connected connectors from sysfs, in primary-preference order. sysfs only
/// tells us the cable state; whether a connector can be driven is decided by
/// vkGetDrmDisplayEXT/vkAcquireDrmDisplayEXT and reported per output.
#[cfg(linux_direct)]
fn scan_drm_connectors(filter: &DrmFilter) -> Result<Vec<DrmConnector>, String> {
    let entries =
        std::fs::read_dir("/sys/class/drm").map_err(|e| format!("read DRM connectors: {e}"))?;
    let mut connectors = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read DRM connector entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((card, connector)) = name.split_once('-') else {
            continue;
        };
        if !card.starts_with("card") || !card[4..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let card_path = PathBuf::from("/dev/dri").join(card);
        if filter
            .card
            .as_ref()
            .is_some_and(|filter| filter != card && PathBuf::from(filter) != card_path)
        {
            continue;
        }
        if std::fs::read_to_string(entry.path().join("status"))
            .ok()
            .as_deref()
            .map(str::trim)
            != Some("connected")
        {
            continue;
        }
        let id = match std::fs::read_to_string(entry.path().join("connector_id"))
            .ok()
            .and_then(|id| id.trim().parse::<u32>().ok())
        {
            Some(id) => id,
            None => continue,
        };
        if filter.connector.as_ref().is_some_and(|filter| {
            filter != &name && filter != connector && filter.parse::<u32>().ok() != Some(id)
        }) {
            continue;
        }
        let fingerprint = connector_fingerprint(&entry.path());
        connectors.push(DrmConnector {
            card: card_path,
            name,
            id,
            fingerprint,
        });
    }
    connectors.sort_by_key(|connector| connector_rank(&connector.name));
    Ok(connectors)
}

#[cfg(linux_direct)]
fn connector_names(connectors: &[DrmConnector]) -> String {
    if connectors.is_empty() {
        return "(none)".into();
    }
    connectors
        .iter()
        .map(|connector| connector.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The EDID-preferred dimensions describe the panel, while that mode's refresh
/// may be a conservative compatibility timing. Rank every mode at these
/// dimensions by refresh; never pick an accepted 4K downscaling mode merely
/// because it has more pixels than an ultrawide panel.
#[cfg(linux_direct)]
fn drm_native_resolution(fd: i32, connector_id: u32) -> Option<vk::Extent2D> {
    use crate::os::linux::drm_sys;
    let connector = unsafe { drm_sys::drmModeGetConnector(fd, connector_id) };
    if connector.is_null() {
        return None;
    }
    let result = unsafe {
        let connector = &*connector;
        if connector.count_modes <= 0 || connector.modes.is_null() {
            None
        } else {
            std::slice::from_raw_parts(connector.modes, connector.count_modes as usize)
                .iter()
                .filter(|mode| mode.type_ & (1 << 3) != 0 && mode.flags & (1 << 4) == 0)
                .max_by_key(|mode| u32::from(mode.hdisplay) * u32::from(mode.vdisplay))
                .map(|mode| vk::Extent2D {
                    width: u32::from(mode.hdisplay),
                    height: u32::from(mode.vdisplay),
                })
        }
    };
    unsafe { drm_sys::drmModeFreeConnector(connector) };
    result
}

#[cfg(linux_direct)]
#[derive(Clone, Copy)]
struct DisplayMode {
    mode: vk::DisplayModeKHR,
    extent: vk::Extent2D,
    refresh_mhz: u32,
}

#[cfg(linux_direct)]
#[derive(Clone, Copy)]
struct PlaneCandidate {
    plane_index: u32,
    stack_index: u32,
    alpha: vk::DisplayPlaneAlphaFlagsKHR,
}

/// A live output vacated from its plane during a re-match, waiting to be
/// rebuilt on `candidate` once every moving output has released its old plane.
/// The display stays acquired throughout.
#[cfg(linux_direct)]
struct PlaneMove {
    index: usize,
    candidate: PlaneCandidate,
    was_primary: bool,
    connector: DrmConnector,
    display: vk::DisplayKHR,
    display_mode: vk::DisplayModeKHR,
    extent: vk::Extent2D,
    refresh_mhz: u32,
}

/// The fastest mode at the connector's native resolution (or the requested
/// override).
#[cfg(linux_direct)]
fn select_display_mode(
    loader: &ash::khr::display::Instance,
    device: vk::PhysicalDevice,
    display: vk::DisplayKHR,
    requested: Option<&str>,
    native_resolution: Option<vk::Extent2D>,
) -> Result<DisplayMode, String> {
    let properties = unsafe { loader.get_physical_device_display_properties(device) }
        .map_err(|e| format!("enumerate Vulkan displays: {e:?}"))?;
    let property = properties
        .iter()
        .find(|p| p.display == display)
        .ok_or("DRM display properties unavailable")?;
    if !property
        .supported_transforms
        .contains(vk::SurfaceTransformFlagsKHR::IDENTITY)
    {
        return Err("display does not support the identity transform".into());
    }
    let modes = unsafe { loader.get_display_mode_properties(device, display) }
        .map_err(|e| format!("enumerate display modes: {e:?}"))?;
    let selected = modes
        .iter()
        .filter(|mode| {
            let Some(requested) = requested else {
                return true;
            };
            let size = mode.parameters.visible_region;
            let resolution = format!("{}x{}", size.width, size.height);
            let hz = (mode.parameters.refresh_rate + 500) / 1000;
            requested == resolution
                || requested == format!("{resolution}-{hz}")
                || requested == format!("{resolution}@{hz}")
        })
        .max_by_key(|mode| {
            let size = mode.parameters.visible_region;
            (
                size == native_resolution.unwrap_or(property.physical_resolution),
                u64::from(size.width) * u64::from(size.height),
                mode.parameters.refresh_rate,
            )
        })
        .ok_or_else(|| {
            format!(
                "Requested display mode {:?} unavailable; modes: {}",
                requested,
                modes
                    .iter()
                    .map(|mode| format!(
                        "{}x{}-{}",
                        mode.parameters.visible_region.width,
                        mode.parameters.visible_region.height,
                        (mode.parameters.refresh_rate + 500) / 1000
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let extent = selected.parameters.visible_region;
    let refresh_mhz = selected.parameters.refresh_rate;
    crate::log!(
        "Vulkan direct mode: {}x{} @ {:.3} Hz ({})",
        extent.width,
        extent.height,
        refresh_mhz as f64 / 1000.0,
        if requested.is_some() {
            "explicit override"
        } else {
            "fastest native-resolution mode"
        }
    );
    Ok(DisplayMode {
        mode: selected.display_mode,
        extent,
        refresh_mhz,
    })
}

/// Every plane that can scan out `mode` on `display` at full size with a
/// usable alpha mode. `excluded` planes are skipped outright. A plane the
/// driver reports as currently driving another display is skipped unless it
/// is in `owned` (one of our own outputs, which a re-match may move); the
/// driver's `current_display` can also lag behind our own surface creation.
#[cfg(linux_direct)]
fn plane_candidates(
    loader: &ash::khr::display::Instance,
    device: vk::PhysicalDevice,
    display: vk::DisplayKHR,
    mode: &DisplayMode,
    excluded: &HashSet<u32>,
    owned: &HashSet<u32>,
) -> Result<Vec<PlaneCandidate>, String> {
    let planes = unsafe { loader.get_physical_device_display_plane_properties(device) }
        .map_err(|e| format!("enumerate display planes: {e:?}"))?;
    let extent = mode.extent;
    let mut candidates = Vec::new();
    for (index, plane) in planes.iter().enumerate() {
        let index = index as u32;
        if excluded.contains(&index) {
            continue;
        }
        if plane.current_display != vk::DisplayKHR::null()
            && plane.current_display != display
            && !owned.contains(&index)
        {
            continue;
        }
        let supported = unsafe { loader.get_display_plane_supported_displays(device, index) }
            .map_err(|e| format!("query plane displays: {e:?}"))?;
        if !supported.contains(&display) {
            continue;
        }
        let caps = unsafe { loader.get_display_plane_capabilities(device, mode.mode, index) }
            .map_err(|e| format!("query display plane capabilities: {e:?}"))?;
        if extent.width < caps.min_src_extent.width
            || extent.height < caps.min_src_extent.height
            || extent.width > caps.max_src_extent.width
            || extent.height > caps.max_src_extent.height
            || extent.width < caps.min_dst_extent.width
            || extent.height < caps.min_dst_extent.height
            || extent.width > caps.max_dst_extent.width
            || extent.height > caps.max_dst_extent.height
            || caps.min_src_position.x > 0
            || caps.min_src_position.y > 0
            || caps.max_src_position.x < 0
            || caps.max_src_position.y < 0
            || caps.min_dst_position.x > 0
            || caps.min_dst_position.y > 0
            || caps.max_dst_position.x < 0
            || caps.max_dst_position.y < 0
        {
            continue;
        }
        let Some(alpha) = [
            vk::DisplayPlaneAlphaFlagsKHR::OPAQUE,
            vk::DisplayPlaneAlphaFlagsKHR::GLOBAL,
            vk::DisplayPlaneAlphaFlagsKHR::PER_PIXEL,
            vk::DisplayPlaneAlphaFlagsKHR::PER_PIXEL_PREMULTIPLIED,
        ]
        .into_iter()
        .find(|alpha| caps.supported_alpha.contains(*alpha)) else {
            continue;
        };
        candidates.push(PlaneCandidate {
            plane_index: index,
            stack_index: plane.current_stack_index,
            alpha,
        });
    }
    Ok(candidates)
}

#[cfg(linux_direct)]
fn create_plane_surface(
    loader: &ash::khr::display::Instance,
    mode: &DisplayMode,
    candidate: &PlaneCandidate,
) -> Result<vk::SurfaceKHR, String> {
    let info = vk::DisplaySurfaceCreateInfoKHR::default()
        .display_mode(mode.mode)
        .plane_index(candidate.plane_index)
        .plane_stack_index(candidate.stack_index)
        .transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
        .global_alpha(1.0)
        .alpha_mode(candidate.alpha)
        .image_extent(mode.extent);
    unsafe { loader.create_display_plane_surface(&info, None) }
        .map_err(|e| format!("create Vulkan direct-display surface: {e:?}"))
}

/// Bipartite matching of displays (in rank order) to planes: augmenting paths
/// move an earlier display to another compatible plane rather than rejecting
/// a later one, and never unmatch a display already matched. Bounded by
/// displays × planes.
#[cfg(linux_direct)]
fn assign_planes(candidates: &[Vec<u32>]) -> Vec<Option<u32>> {
    fn augment(
        display: usize,
        candidates: &[Vec<u32>],
        owner: &mut HashMap<u32, usize>,
        visited: &mut HashSet<u32>,
    ) -> bool {
        for &plane in &candidates[display] {
            if !visited.insert(plane) {
                continue;
            }
            let free = match owner.get(&plane) {
                None => true,
                Some(&other) => augment(other, candidates, owner, visited),
            };
            if free {
                owner.insert(plane, display);
                return true;
            }
        }
        false
    }
    let mut owner: HashMap<u32, usize> = HashMap::new();
    for display in 0..candidates.len() {
        let mut visited = HashSet::new();
        augment(display, candidates, &mut owner, &mut visited);
    }
    let mut assignment = vec![None; candidates.len()];
    for (plane, display) in owner {
        assignment[display] = Some(plane);
    }
    assignment
}

/// UNORM with the sRGB-nonlinear colour space only: the composition already
/// holds display-encoded values, so an sRGB attachment would gamma-encode them
/// a second time. Rather than knowingly present wrong colours, an output with
/// no such format is reported unsupported.
#[cfg(linux_direct)]
fn choose_output_format(formats: &[vk::SurfaceFormatKHR]) -> Result<vk::SurfaceFormatKHR, String> {
    const PREFERRED: [vk::Format; 4] = [
        vk::Format::B8G8R8A8_UNORM,
        vk::Format::R8G8B8A8_UNORM,
        vk::Format::A8B8G8R8_UNORM_PACK32,
        vk::Format::A2B10G10R10_UNORM_PACK32,
    ];
    for preferred in PREFERRED {
        if let Some(format) = formats
            .iter()
            .find(|f| f.format == preferred && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        {
            return Ok(*format);
        }
    }
    if formats.len() == 1
        && formats[0].format == vk::Format::UNDEFINED
        && formats[0].color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
    {
        // The surface accepts any format in the sRGB-nonlinear colour space.
        return Ok(vk::SurfaceFormatKHR {
            format: vk::Format::B8G8R8A8_UNORM,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        });
    }
    Err(format!(
        "no colour-correct (UNORM, sRGB nonlinear) surface format; offered: {}",
        formats
            .iter()
            .map(|f| format!("{:?}/{:?}", f.format, f.color_space))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Letterbox the composition into an output: uniform scale, centered,
/// integer pixel rectangle. The render pass clears the rest to black.
#[cfg(linux_direct)]
fn letterbox(source: vk::Extent2D, target: vk::Extent2D) -> (vk::Viewport, vk::Rect2D) {
    let source_width = source.width.max(1) as f64;
    let source_height = source.height.max(1) as f64;
    let target_width = target.width.max(1) as f64;
    let target_height = target.height.max(1) as f64;
    let scale = (target_width / source_width).min(target_height / source_height);
    let width = (source_width * scale).round().clamp(1.0, target_width);
    let height = (source_height * scale).round().clamp(1.0, target_height);
    let x = ((target_width - width) / 2.0).floor();
    let y = ((target_height - height) / 2.0).floor();
    (
        vk::Viewport {
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        },
        vk::Rect2D {
            offset: vk::Offset2D {
                x: x as i32,
                y: y as i32,
            },
            extent: vk::Extent2D {
                width: width as u32,
                height: height as u32,
            },
        },
    )
}

#[cfg(linux_direct)]
fn compile_blit_shaders() -> Result<(Vec<u32>, Vec<u32>), String> {
    use naga::{back::spv, valid};
    let module = naga::front::wgsl::parse_str(BLIT_WGSL)
        .map_err(|e| format!("blit shader WGSL parse error: {e}"))?;
    let info = valid::Validator::new(valid::ValidationFlags::all(), valid::Capabilities::all())
        .validate(&module)
        .map_err(|e| format!("blit shader WGSL validation error: {e}"))?;
    let options = spv::Options {
        lang_version: (1, 3),
        flags: spv::WriterFlags::empty(),
        fake_missing_bindings: true,
        binding_map: spv::BindingMap::default(),
        capabilities: None,
        bounds_check_policies: naga::proc::BoundsCheckPolicies::default(),
        zero_initialize_workgroup_memory: spv::ZeroInitializeWorkgroupMemoryMode::None,
        force_loop_bounding: false,
        use_storage_input_output_16: false,
        debug_info: None,
    };
    let stage = |shader_stage, entry_point: &str| {
        let pipeline = spv::PipelineOptions {
            shader_stage,
            entry_point: entry_point.to_string(),
        };
        spv::write_vec(&module, &info, &options, Some(&pipeline))
            .map_err(|e| format!("blit shader SPIR-V write failed for {entry_point}: {e}"))
    };
    Ok((
        stage(naga::ShaderStage::Vertex, "vertex_main")?,
        stage(naga::ShaderStage::Fragment, "fragment_main")?,
    ))
}

/// Kernel uevent listener: DRM hotplug events wake the connector rescan
/// immediately and name the affected card/connector; the periodic rescan
/// covers systems where the socket is unavailable or its buffer overflowed.
#[cfg(linux_direct)]
mod uevent {
    use super::DrmEvent;
    // poll(2) and its pollfd come from the crate's existing libc surface
    // (nfds_t is unsigned long on Linux).
    use crate::os::linux::v4l2_sys::{poll, pollfd, POLLIN};
    use std::{
        os::raw::{c_int, c_uint, c_void},
        time::Duration,
    };

    #[repr(C)]
    struct SockaddrNl {
        family: u16,
        pad: u16,
        pid: u32,
        groups: u32,
    }

    extern "C" {
        fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
        fn bind(fd: c_int, addr: *const c_void, len: c_uint) -> c_int;
        fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize;
    }

    const AF_NETLINK: c_int = 16;
    const SOCK_RAW: c_int = 3;
    const SOCK_NONBLOCK: c_int = 0o4000;
    const SOCK_CLOEXEC: c_int = 0o2000000;
    const NETLINK_KOBJECT_UEVENT: c_int = 15;
    const KERNEL_EVENT_GROUP: u32 = 1;

    pub struct UeventSocket {
        fd: c_int,
    }

    impl UeventSocket {
        pub fn open() -> Option<Self> {
            let fd = unsafe {
                socket(
                    AF_NETLINK,
                    SOCK_RAW | SOCK_NONBLOCK | SOCK_CLOEXEC,
                    NETLINK_KOBJECT_UEVENT,
                )
            };
            if fd < 0 {
                return None;
            }
            let address = SockaddrNl {
                family: AF_NETLINK as u16,
                pad: 0,
                pid: 0,
                groups: KERNEL_EVENT_GROUP,
            };
            let bound = unsafe {
                bind(
                    fd,
                    (&address as *const SockaddrNl).cast(),
                    std::mem::size_of::<SockaddrNl>() as c_uint,
                )
            };
            if bound != 0 {
                unsafe { crate::os::linux::libc_sys::close(fd) };
                return None;
            }
            Some(Self { fd })
        }

        /// Sleeps up to `timeout` for DRM subsystem uevents and returns the
        /// hotplug/property events found (card and, on kernels that report
        /// it, the connector id). The caller rescans either way.
        pub fn wait_for_drm_change(&self, timeout: Duration) -> Vec<DrmEvent> {
            let mut poll_fd = pollfd {
                fd: self.fd,
                events: POLLIN,
                revents: 0,
            };
            let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as c_int;
            let ready = unsafe { poll(&mut poll_fd, 1, timeout_ms) };
            let mut events = Vec::new();
            if ready <= 0 {
                return events;
            }
            let mut buffer = [0u8; 4096];
            loop {
                let len = unsafe { recv(self.fd, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
                if len <= 0 {
                    break;
                }
                if let Some(event) = parse_drm_event(&buffer[..len as usize]) {
                    events.push(event);
                }
            }
            events
        }
    }

    impl Drop for UeventSocket {
        fn drop(&mut self) {
            unsafe { crate::os::linux::libc_sys::close(self.fd) };
        }
    }

    /// `change@/devices/…/drm/card1\0ACTION=change\0DEVPATH=…\0SUBSYSTEM=drm\0HOTPLUG=1\0CONNECTOR=95\0…`
    fn parse_drm_event(message: &[u8]) -> Option<DrmEvent> {
        let mut subsystem_drm = false;
        let mut hotplug = false;
        let mut card = None;
        let mut connector_id = None;
        for field in message.split(|byte| *byte == 0) {
            let Ok(field) = std::str::from_utf8(field) else {
                continue;
            };
            match field.split_once('=') {
                Some(("SUBSYSTEM", value)) => subsystem_drm = value == "drm",
                Some(("HOTPLUG", value)) => hotplug = value == "1",
                Some(("CONNECTOR", value)) => connector_id = value.trim().parse::<u32>().ok(),
                Some(("DEVPATH", value)) => {
                    card = value
                        .split('/')
                        .find(|segment| {
                            segment.len() > 4
                                && segment.starts_with("card")
                                && segment[4..].bytes().all(|byte| byte.is_ascii_digit())
                        })
                        .map(str::to_owned);
                }
                _ => {}
            }
        }
        if !subsystem_drm || (!hotplug && connector_id.is_none()) {
            return None;
        }
        Some(DrmEvent {
            card,
            connector_id,
            hotplug,
        })
    }
}

/// Long-lived worker: rescans sysfs on DRM uevents and once per second,
/// publishing changed connector lists and the events that triggered them
/// (an unchanged list still travels when a kernel event named a connector).
#[cfg(linux_direct)]
fn hotplug_worker(
    filter: DrmFilter,
    initial: Vec<DrmConnector>,
    sender: ToUISender<DrmScan>,
    stop: Arc<AtomicBool>,
) {
    let socket = uevent::UeventSocket::open();
    if socket.is_none() {
        crate::log!(
            "Direct display: no uevent socket; polling connectors every {:?}",
            HOTPLUG_POLL
        );
    }
    let mut last = initial;
    let mut pending: Option<DrmScan> = None;
    let mut events: Vec<DrmEvent> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        let connectors = match scan_drm_connectors(&filter) {
            Ok(scan) => scan,
            Err(_) => last.clone(),
        };
        let changed = connectors != last;
        if changed || !events.is_empty() {
            last = connectors.clone();
            match pending.as_mut() {
                Some(scan) => {
                    scan.connectors = connectors;
                    scan.events.append(&mut events);
                }
                None => {
                    pending = Some(DrmScan {
                        connectors,
                        events: std::mem::take(&mut events),
                    })
                }
            }
        }
        if let Some(snapshot) = pending.take() {
            match sender.try_send(snapshot) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(snapshot)) => pending = Some(snapshot),
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
            }
        }
        match &socket {
            Some(socket) => events.extend(socket.wait_for_drm_change(HOTPLUG_POLL)),
            None => std::thread::sleep(HOTPLUG_POLL),
        }
    }
}

#[cfg(linux_direct)]
impl CxVulkan {
    /// Direct-to-display renderer: acquire every connected connector of one
    /// GPU, render at the source output's native mode and clone it elsewhere.
    pub fn new_direct(mode: Option<&str>, spawner: &ThreadSpawner) -> Result<Self, String> {
        let init = DesktopInit::new(&[
            vk::KHR_DISPLAY_NAME,
            vk::EXT_DIRECT_MODE_DISPLAY_NAME,
            vk::EXT_ACQUIRE_DRM_DISPLAY_NAME,
        ])?;
        let filter = DrmFilter::from_env();
        let connected = scan_drm_connectors(&filter)?;
        if connected.is_empty() {
            return Err(
                "No connected DRM connector matches MAKEPAD_DRM_DEVICE / MAKEPAD_DRM_CONNECTOR"
                    .into(),
            );
        }
        let (physical_device, queue, desktop_extent, desktop) = {
            let instance = init.instance.as_ref().unwrap();
            let entry = init.entry.as_ref().unwrap();
            let devices = init.devices()?;
            let mut direct = DirectState {
                physical_device: vk::PhysicalDevice::null(),
                display_loader: ash::khr::display::Instance::new(entry, instance),
                drm_loader: ash::ext::acquire_drm_display::Instance::new(entry, instance),
                release_loader: ash::ext::direct_mode_display::Instance::new(entry, instance),
                surface_loader: init.surface_loader.clone(),
                filter,
                cards: Vec::new(),
                pending: Vec::new(),
                outputs: Vec::new(),
                unsupported: Vec::new(),
                connected: connected.clone(),
                scan_dirty: false,
                preferred_source: None,
                pending_source: None,
                source_name: None,
                source_switch_failures: 0,
                source_retry_at: None,
                defer_composition_resize: false,
                composition_resize_request: None,
                composition: None,
                composition_epoch: 0,
                composition_generation: 0,
                composition_valid: false,
                desktop_extent: vk::Extent2D::default(),
                blit: None,
                retired_fences: Vec::new(),
                hotplug: None,
                wait: DirectWait::Ready,
            };
            let mut diagnostics = Vec::new();

            // Pass 1: which GPU owns which connector. The GPU driving the best
            // ranked connector (built-in panel, then discrete, then name)
            // becomes the renderer; everything else on it is cloned.
            let mut owners: Vec<(DrmConnector, i32, vk::PhysicalDevice, vk::DisplayKHR)> =
                Vec::new();
            for connector in &connected {
                let fd = match direct.card_fd(&connector.card) {
                    Ok(fd) => fd,
                    Err(err) => {
                        diagnostics.push(err.clone());
                        direct
                            .unsupported
                            .push((connector.clone(), format!("unsupported: {err}")));
                        continue;
                    }
                };
                // vkGetDrmDisplayEXT verifies both the physical-device/fd match
                // and connector ownership. A mismatch returns an error or null.
                let owner = devices.iter().copied().find_map(|device| {
                    match unsafe { direct.drm_loader.get_drm_display(device, fd, connector.id) } {
                        Ok(display) if display != vk::DisplayKHR::null() => Some((device, display)),
                        _ => None,
                    }
                });
                match owner {
                    Some((device, display)) => {
                        owners.push((connector.clone(), fd, device, display))
                    }
                    None => {
                        diagnostics.push(format!(
                            "{}: no Vulkan device owns this connector",
                            connector.name
                        ));
                        direct.unsupported.push((
                            connector.clone(),
                            "unsupported: no Vulkan device with VK_EXT_acquire_drm_display owns this connector".into(),
                        ));
                    }
                }
            }
            let selected = owners
                .iter()
                .min_by_key(|(connector, _, device, _)| {
                    let (builtin, name) = connector_rank(&connector.name);
                    (builtin, device_type_rank(instance, *device), name)
                })
                .map(|(_, _, device, _)| *device);
            let Some(selected) = selected else {
                return Err(format!(
                    "No usable Vulkan DRM display. {}",
                    diagnostics.join("; ")
                ));
            };
            direct.physical_device = selected;

            // Pass 2: acquire every connector the selected GPU owns and pick
            // its mode; the mode override applies to the first (best ranked)
            // connector only.
            let mut plans: Vec<(
                DrmConnector,
                vk::DisplayKHR,
                DisplayMode,
                Vec<PlaneCandidate>,
            )> = Vec::new();
            for (connector, fd, device, display) in owners {
                if device != selected {
                    let status = format!(
                        "unsupported: driven by another GPU ({}); clone outputs must share the rendering GPU",
                        connector_card_name(&connector.card)
                    );
                    direct.unsupported.push((connector, status));
                    continue;
                }
                if let Err(err) =
                    unsafe { direct.drm_loader.acquire_drm_display(device, fd, display) }
                {
                    let status = format!(
                        "failed: acquire display {err:?} (requires DRM master on an active VT)"
                    );
                    diagnostics.push(format!("{}: {status}", connector.name));
                    direct
                        .outputs
                        .push(DirectOutput::inactive(connector, status, RETRY_INITIAL));
                    continue;
                }
                let requested = if plans.is_empty() { mode } else { None };
                let native_resolution = drm_native_resolution(fd, connector.id);
                let planned = select_display_mode(
                    &direct.display_loader,
                    device,
                    display,
                    requested,
                    native_resolution,
                )
                .and_then(|mode| {
                    plane_candidates(
                        &direct.display_loader,
                        device,
                        display,
                        &mode,
                        &HashSet::new(),
                        &HashSet::new(),
                    )
                    .map(|candidates| (mode, candidates))
                });
                match planned {
                    Ok((mode, candidates)) => plans.push((connector, display, mode, candidates)),
                    Err(err) => {
                        diagnostics.push(format!("{}: {err}", connector.name));
                        direct.release_surface_and_display(vk::SurfaceKHR::null(), display);
                        direct.outputs.push(DirectOutput::inactive(
                            connector,
                            format!("failed: {err}"),
                            RETRY_INITIAL,
                        ));
                    }
                }
            }

            // Pass 3: planes. Matching across all displays keeps a later
            // display from losing the only plane it can use to an earlier one
            // that had alternatives.
            let plane_sets: Vec<Vec<u32>> = plans
                .iter()
                .map(|(_, _, _, candidates)| candidates.iter().map(|c| c.plane_index).collect())
                .collect();
            let assignment = assign_planes(&plane_sets);
            for ((connector, display, mode, candidates), plane) in plans.into_iter().zip(assignment)
            {
                let Some(candidate) = plane
                    .and_then(|plane| candidates.iter().find(|c| c.plane_index == plane).copied())
                else {
                    let status =
                        "failed: no free display plane can present the selected mode".to_string();
                    diagnostics.push(format!("{}: {status}", connector.name));
                    direct.release_surface_and_display(vk::SurfaceKHR::null(), display);
                    direct
                        .outputs
                        .push(DirectOutput::inactive(connector, status, RETRY_INITIAL));
                    continue;
                };
                match create_plane_surface(&direct.display_loader, &mode, &candidate) {
                    Ok(surface) => {
                        crate::log!(
                            "Vulkan direct: {} on {} plane {}, {}x{} @ {:.3} Hz",
                            connector.name,
                            connector.card.display(),
                            candidate.plane_index,
                            mode.extent.width,
                            mode.extent.height,
                            mode.refresh_mhz as f64 / 1000.0
                        );
                        direct.pending.push(PendingOutput {
                            connector,
                            display,
                            surface,
                            plane_index: candidate.plane_index,
                            display_mode: mode.mode,
                            extent: mode.extent,
                            refresh_mhz: mode.refresh_mhz,
                        });
                    }
                    Err(err) => {
                        diagnostics.push(format!("{}: {err}", connector.name));
                        direct.release_surface_and_display(vk::SurfaceKHR::null(), display);
                        direct.outputs.push(DirectOutput::inactive(
                            connector,
                            format!("failed: {err}"),
                            RETRY_INITIAL,
                        ));
                    }
                }
            }
            if direct.pending.is_empty() {
                return Err(format!(
                    "No usable Vulkan DRM display. {}",
                    diagnostics.join("; ")
                ));
            }
            let queue = direct.pick_queue_family(instance, &init.surface_loader)?;
            let uuid = device_uuid(instance, selected);
            // Hosted children inherit the environment when spawned; they read
            // this before creating their offscreen renderer.
            std::env::set_var(LINUX_VULKAN_DEVICE_ENV, uuid_hex(&uuid));
            crate::log!(
                "Vulkan direct: GPU uuid {} exported as {LINUX_VULKAN_DEVICE_ENV} for hosted apps",
                uuid_hex(&uuid)
            );
            let desktop_extent = direct.pending[0].extent;
            (
                selected,
                queue,
                desktop_extent,
                DesktopState {
                    direct: Some(direct),
                    ..Default::default()
                },
            )
        };
        let mut renderer = init.finish(
            physical_device,
            queue,
            desktop,
            desktop_extent.width,
            desktop_extent.height,
        )?;
        renderer.direct_initialize(spawner)?;
        let compositor = if let Ok(pin) = std::env::var("MAKEPAD_VULKAN_COMPOSITOR_UUID") {
            Some(parse_uuid_hex(&pin).ok_or_else(|| {
                format!("MAKEPAD_VULKAN_COMPOSITOR_UUID={pin:?} is not 32 hex digits")
            })?)
        } else if let Ok(pci) = std::env::var("MAKEPAD_VULKAN_COMPOSITOR_PCI") {
            let uuid = renderer
                .desktop
                .gpu
                .devices
                .iter()
                .find(|device| device.pci_address.as_deref() == Some(pci.as_str()))
                .map(|device| device.uuid);
            if uuid.is_none() {
                crate::log!("Vulkan direct: saved compositor PCI {pci} is unavailable; using the display GPU");
            }
            uuid
        } else {
            None
        };
        if let Some(uuid) = compositor {
            if device_uuid(&renderer.instance, renderer.physical_device) != uuid {
                return Self::new_routed_compositor(renderer, uuid);
            }
        }
        Ok(renderer)
    }

    fn new_routed_compositor(mut display: CxVulkan, uuid: [u8; 16]) -> Result<Self, String> {
        let extent = display.swapchain_extent;
        display
            .desktop
            .direct
            .as_mut()
            .unwrap()
            .defer_composition_resize = true;
        let mut renderer = Self::new_offscreen_on(Some(uuid))?;
        renderer.requested_width = extent.width;
        renderer.requested_height = extent.height;
        renderer.desktop.routed = Some(RoutedCompositor {
            display: Box::new(display),
            composition: None,
            bridge: None,
            composition_valid: false,
            resend: false,
            failure: None,
            retry_at: None,
            wait: DirectWait::GpuBusy,
        });
        let targets = renderer.direct_build_composition(extent)?;
        let composition = renderer.install_composition_targets(targets);
        renderer.desktop.routed.as_mut().unwrap().composition = Some(composition);
        let bridge = super::gpu_bridge::GpuBridge::new(
            &renderer,
            &renderer.desktop.routed.as_ref().unwrap().display,
            extent,
        )?;
        renderer.desktop.routed.as_mut().unwrap().bridge = Some(bridge);
        // Initial children follow the composition GPU, not the connector GPU.
        // Later migration uses the explicit device constructor and protocol.
        std::env::set_var(LINUX_VULKAN_DEVICE_ENV, uuid_hex(&uuid));
        crate::log!("Vulkan direct: compositor {} renders {}x{}; separate display GPU receives GPU-only frames",
            uuid_hex(&uuid), extent.width, extent.height);
        Ok(renderer)
    }

    fn routed_reconcile(&mut self) -> Result<Option<vk::Extent2D>, String> {
        let Some(mut routed) = self.desktop.routed.take() else {
            return Ok(None);
        };
        let result = (|| {
            routed.display.direct_reconcile_outputs()?;
            if self.gpu_transition_pending() {
                return Ok(None);
            }
            let extent = routed
                .display
                .desktop
                .direct
                .as_mut()
                .unwrap()
                .composition_resize_request
                .take();
            let Some(extent) = extent else {
                return Ok(None);
            };
            // All three allocations succeed before either retained composition
            // is replaced. Slow/failed allocation leaves the old route usable.
            let prepared = (|| {
                let targets = self.direct_build_composition(extent)?;
                let display_targets = match routed.display.direct_build_composition(extent) {
                    Ok(targets) => targets,
                    Err(error) => {
                        self.destroy_composition_targets(targets);
                        return Err(error);
                    }
                };
                match super::gpu_bridge::GpuBridge::new(self, &routed.display, extent) {
                    Ok(bridge) => Ok((targets, display_targets, bridge)),
                    Err(error) => {
                        self.destroy_composition_targets(targets);
                        routed.display.destroy_composition_targets(display_targets);
                        Err(error)
                    }
                }
            })();
            let (targets, display_targets, bridge) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    let direct = routed.display.desktop.direct.as_mut().unwrap();
                    direct.source_switch_failures += 1;
                    let last = direct.source_switch_failures >= SOURCE_SWITCH_RETRIES;
                    direct.source_retry_at = if last {
                        None
                    } else {
                        Some(Instant::now() + SOURCE_RETRY_INTERVAL)
                    };
                    if last && direct.pending_source.is_some() {
                        direct.pending_source = None;
                        direct.preferred_source = direct.source_name.clone();
                    }
                    crate::warning!("Vulkan routed: resize {}x{} failed ({error}), attempt {}/{}; keeping retained desktop",
                        extent.width, extent.height, direct.source_switch_failures, SOURCE_SWITCH_RETRIES);
                    return Ok(None);
                }
            };
            self.device_wait_idle();
            routed.display.device_wait_idle();
            if let Some(old) = routed.bridge.take() {
                unsafe { old.destroy(self, &routed.display) };
            }
            let composition = self.install_composition_targets(targets);
            if let Some(old) = routed.composition.replace(composition) {
                self.destroy_texture_resource(old);
            }
            let mut direct = routed.display.desktop.direct.take().unwrap();
            routed
                .display
                .direct_install_composition(&mut direct, display_targets);
            direct.source_switch_failures = 0;
            direct.source_retry_at = None;
            direct.scan_dirty = true;
            let selected = routed.display.direct_select_source(&mut direct);
            routed.display.desktop.direct = Some(direct);
            routed.bridge = Some(bridge);
            routed.composition_valid = false;
            routed.resend = false;
            routed.failure = None;
            routed.retry_at = None;
            routed.wait = DirectWait::GpuBusy;
            selected?;
            Ok(Some(extent))
        })();
        self.desktop.routed = Some(routed);
        result
    }

    fn routed_service(&mut self, routed: &mut RoutedCompositor) -> Result<(), String> {
        if routed.failure.is_some() {
            routed.display.direct_present_retained()?;
            routed.wait = DirectWait::GpuBusy;
            if routed.retry_at.is_some_and(|at| at > Instant::now()) {
                return Ok(());
            }
            // Every foreign wait was admitted only after the owning endpoint's
            // fence signaled. Retirement cannot depend on a future UI message.
            self.device_wait_idle();
            routed.display.device_wait_idle();
            if let Some(old) = routed.bridge.take() {
                unsafe { old.destroy(self, &routed.display) };
            }
            match super::gpu_bridge::GpuBridge::new(self, &routed.display, self.swapchain_extent) {
                Ok(bridge) => {
                    routed.bridge = Some(bridge);
                    routed.failure = None;
                    routed.retry_at = None;
                    routed.resend = routed.composition_valid;
                    crate::log!("Vulkan routed: transfer buffers recovered");
                }
                Err(error) => {
                    if routed.failure.as_deref() != Some(error.as_str()) {
                        crate::error!("Vulkan routed: transfer recovery failed: {error}");
                    }
                    routed.failure = Some(error);
                    routed.retry_at = Some(Instant::now() + Duration::from_secs(1));
                    return Ok(());
                }
            }
        }
        if let Err(error) = self.routed_service_inner(routed) {
            self.routed_transfer_failed(routed, error);
        }
        Ok(())
    }

    fn routed_transfer_failed(&self, routed: &mut RoutedCompositor, error: String) {
        crate::error!("Vulkan routed: transfer failed ({error}); retaining desktop and retrying");
        routed.failure = Some(error);
        routed.retry_at = Some(Instant::now() + Duration::from_secs(1));
        routed.wait = DirectWait::GpuBusy;
    }

    fn routed_service_inner(&mut self, routed: &mut RoutedCompositor) -> Result<(), String> {
        let bridge = routed
            .bridge
            .as_mut()
            .ok_or("cross-GPU bridge unavailable")?;
        if !unsafe { bridge.poll_initialization(self, &routed.display) }? {
            routed.wait = DirectWait::GpuBusy;
            routed.display.direct_present_retained()?;
            return Ok(());
        }
        let (image, valid) = {
            let direct = routed
                .display
                .desktop
                .direct
                .as_ref()
                .ok_or("display context unavailable")?;
            (
                direct
                    .composition
                    .as_ref()
                    .ok_or("display composition unavailable")?
                    .image,
                direct.composition_valid,
            )
        };
        if self.swapchain_extent != routed.display.swapchain_extent {
            return Err("compositor/presenter extent mismatch".into());
        }
        if routed.resend && unsafe { bridge.writable(self, &routed.display) }? {
            let image = routed
                .composition
                .as_ref()
                .ok_or("compositor target unavailable")?
                .image;
            unsafe { bridge.send(self, image) }?;
            routed.resend = false;
        }
        if unsafe { bridge.receive(self, &routed.display, image, valid) }? {
            let direct = routed.display.desktop.direct.as_mut().unwrap();
            direct.composition_generation += 1;
            direct.composition_valid = true;
        }
        routed.display.direct_present_retained()?;
        let direct = routed.display.desktop.direct.as_ref().unwrap();
        // NoOutput permits deliberate captures, so it must not conceal a
        // pending main submission or a carrier still owned by the other GPU.
        routed.wait = if !self.fence_signaled(self.in_flight_fence)?
            || !unsafe { bridge.writable(self, &routed.display) }?
        {
            DirectWait::GpuBusy
        } else {
            match direct.source_index() {
                None => DirectWait::NoOutput,
                Some(index)
                    if routed
                        .display
                        .direct_output_can_take_frame(&direct.outputs[index])? =>
                {
                    DirectWait::Ready
                }
                Some(_) => DirectWait::GpuBusy,
            }
        };
        Ok(())
    }

    fn routed_frame(
        &mut self,
        cx: &mut Cx,
        routed: &mut RoutedCompositor,
        draw_pass_id: DrawPassId,
        before_present: impl FnOnce(),
    ) -> Result<bool, String> {
        self.routed_service(routed)?;
        let capture_window_id = cx
            .get_pass_window_id(draw_pass_id)
            .map(|window| window.id());
        let capture = !cx.screenshot_requests.is_empty()
            || crate::screen_capture::capture_wants_window(capture_window_id);
        if routed.wait != DirectWait::Ready && !(capture && routed.wait == DirectWait::NoOutput) {
            return Ok(false);
        }
        let bridge = routed
            .bridge
            .as_mut()
            .ok_or("cross-GPU bridge unavailable")?;
        if !unsafe { bridge.writable(self, &routed.display) }? {
            routed.wait = DirectWait::GpuBusy;
            return Ok(false);
        }
        let image = routed
            .composition
            .as_ref()
            .ok_or("compositor target unavailable")?
            .image;
        if !self.draw_direct_composition(cx, draw_pass_id, image)? {
            return Ok(false);
        }
        routed.composition_valid = true;
        if let Err(error) = unsafe { bridge.send(self, image) } {
            self.routed_transfer_failed(routed, error);
            return Ok(true);
        }
        routed.wait = DirectWait::GpuBusy;
        before_present();
        self.routed_service(routed)?;
        Ok(true)
    }

    fn direct_initialize(&mut self, spawner: &ThreadSpawner) -> Result<(), String> {
        let Some(mut direct) = self.desktop.direct.take() else {
            return Err("direct display state missing".into());
        };
        let result = self.direct_initialize_inner(&mut direct, spawner);
        self.desktop.direct = Some(direct);
        result
    }

    fn direct_initialize_inner(
        &mut self,
        direct: &mut DirectState,
        spawner: &ThreadSpawner,
    ) -> Result<(), String> {
        self.direct_ensure_blit(direct)?;
        let mut failures = Vec::new();
        {
            let DirectState {
                blit,
                pending,
                outputs,
                retired_fences,
                ..
            } = &mut *direct;
            let blit = blit.as_mut().ok_or("blit resources missing")?;
            for item in std::mem::take(pending) {
                match self.direct_create_output(blit, retired_fences, item) {
                    Ok(output) => outputs.push(output),
                    Err(failure) => failures.push(failure),
                }
            }
        }
        for (pending, err) in failures {
            crate::warning!("Vulkan direct: {} failed: {err}", pending.connector.name);
            direct.release_surface_and_display(pending.surface, pending.display);
            direct.outputs.push(DirectOutput::inactive(
                pending.connector,
                format!("failed: {err}"),
                RETRY_INITIAL,
            ));
        }
        if !direct.outputs.iter().any(|output| output.active) {
            let statuses = direct
                .outputs
                .iter()
                .map(|output| format!("{}: {}", output.connector.name, output.status))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "No DRM output could create a swapchain. {statuses}"
            ));
        }
        self.direct_select_source(direct)?;
        if direct.composition.is_none() {
            return Err("No composition target could be created for the render source".into());
        }
        direct.prune_cards();
        self.direct_start_hotplug(direct, spawner);
        Ok(())
    }

    fn direct_start_hotplug(&mut self, direct: &mut DirectState, spawner: &ThreadSpawner) {
        let (sender, receiver) = to_ui_bounded::<DrmScan>(NonZeroUsize::new(4).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let filter = direct.filter.clone();
        let initial = direct.connected.clone();
        let options = ThreadOptions {
            name: Some(Arc::from("makepad-drm-hotplug")),
            ..Default::default()
        };
        match spawner.spawn_worker(options, move || hotplug_worker(filter, initial, sender, worker_stop)) {
            Ok(handle) => direct.hotplug = Some(HotplugWatch { receiver, stop, _handle: handle }),
            Err(err) => crate::warning!(
                "Direct display: hotplug watcher unavailable ({err}); connectors are fixed for this session"
            ),
        }
    }

    fn direct_ensure_blit(&mut self, direct: &mut DirectState) -> Result<(), String> {
        if direct.blit.is_some() {
            return Ok(());
        }
        let (vertex_spirv, fragment_spirv) = compile_blit_shaders()?;
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .min_lod(0.0)
            .max_lod(0.0);
        let sampler = unsafe { self.device.create_sampler(&sampler_info, None) }
            .map_err(|e| format!("create blit sampler: {e:?}"))?;
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_count(1)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_count(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let descriptor_set_layout = match unsafe {
            self.device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        } {
            Ok(layout) => layout,
            Err(e) => {
                unsafe { self.device.destroy_sampler(sampler, None) };
                return Err(format!("create blit descriptor set layout: {e:?}"));
            }
        };
        let set_layouts = [descriptor_set_layout];
        let pipeline_layout = match unsafe {
            self.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
                None,
            )
        } {
            Ok(layout) => layout,
            Err(e) => {
                unsafe {
                    self.device
                        .destroy_descriptor_set_layout(descriptor_set_layout, None);
                    self.device.destroy_sampler(sampler, None);
                }
                return Err(format!("create blit pipeline layout: {e:?}"));
            }
        };
        direct.blit = Some(BlitResources {
            sampler,
            descriptor_set_layout,
            pipeline_layout,
            vertex_spirv,
            fragment_spirv,
            pipelines: HashMap::new(),
        });
        Ok(())
    }

    /// Render pass + pipeline presenting the composition into swapchain images
    /// of `format`; created once per format and shared by all outputs using it.
    fn direct_blit_render_pass(
        &mut self,
        blit: &mut BlitResources,
        format: vk::Format,
    ) -> Result<vk::RenderPass, String> {
        if let Some(pipeline) = blit.pipelines.get(&format.as_raw()) {
            return Ok(pipeline.render_pass);
        }
        let attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let color_refs = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs);
        // Orders this blit after the composition pass that wrote the image it
        // samples and after the previous use of the swapchain image.
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::SHADER_READ)
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::SHADER_READ,
            )];
        let attachments = [attachment];
        let subpasses = [subpass];
        let render_pass = unsafe {
            self.device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpasses)
                    .dependencies(&dependencies),
                None,
            )
        }
        .map_err(|e| format!("create blit render pass: {e:?}"))?;
        let modules = (|| -> Result<(vk::ShaderModule, vk::ShaderModule), String> {
            let vertex = unsafe {
                self.device.create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(&blit.vertex_spirv),
                    None,
                )
            }
            .map_err(|e| format!("create blit vertex module: {e:?}"))?;
            let fragment = match unsafe {
                self.device.create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(&blit.fragment_spirv),
                    None,
                )
            } {
                Ok(fragment) => fragment,
                Err(e) => {
                    unsafe { self.device.destroy_shader_module(vertex, None) };
                    return Err(format!("create blit fragment module: {e:?}"));
                }
            };
            Ok((vertex, fragment))
        })();
        let (vertex_module, fragment_module) = match modules {
            Ok(modules) => modules,
            Err(err) => {
                unsafe { self.device.destroy_render_pass(render_pass, None) };
                return Err(err);
            }
        };
        let vs_entry = c"vertex_main";
        let fs_entry = c"fragment_main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex_module)
                .name(vs_entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fragment_module)
                .name(fs_entry),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let create_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic)
            .layout(blit.pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);
        let result = unsafe {
            self.device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[create_info], None)
        };
        unsafe {
            self.device.destroy_shader_module(vertex_module, None);
            self.device.destroy_shader_module(fragment_module, None);
        }
        let pipeline = match result {
            Ok(pipelines) if !pipelines.is_empty() => pipelines[0],
            Ok(_) => {
                unsafe { self.device.destroy_render_pass(render_pass, None) };
                return Err("create blit pipeline returned nothing".into());
            }
            Err((pipelines, e)) => {
                unsafe {
                    for pipeline in pipelines {
                        self.device.destroy_pipeline(pipeline, None);
                    }
                    self.device.destroy_render_pass(render_pass, None);
                }
                return Err(format!("create blit pipeline: {e:?}"));
            }
        };
        blit.pipelines.insert(
            format.as_raw(),
            BlitPipeline {
                render_pass,
                pipeline,
            },
        );
        Ok(render_pass)
    }

    /// Device-level resources for an acquired display: fences, command
    /// buffer, descriptor set and the swapchain with its per-image targets.
    fn direct_create_output(
        &mut self,
        blit: &mut BlitResources,
        retired_fences: &mut Vec<RetiredFence>,
        pending: PendingOutput,
    ) -> Result<DirectOutput, (PendingOutput, String)> {
        let mut output = DirectOutput {
            connector: pending.connector,
            display: pending.display,
            surface: pending.surface,
            plane_index: pending.plane_index,
            display_mode: pending.display_mode,
            mode_extent: pending.extent,
            refresh_mhz: pending.refresh_mhz,
            swapchain: vk::SwapchainKHR::null(),
            format: vk::Format::UNDEFINED,
            extent: vk::Extent2D::default(),
            images: Vec::new(),
            views: Vec::new(),
            framebuffers: Vec::new(),
            present_semaphores: Vec::new(),
            acquire_fence: vk::Fence::null(),
            submit_fence: vk::Fence::null(),
            command_buffer: vk::CommandBuffer::null(),
            descriptor_pool: vk::DescriptorPool::null(),
            descriptor_set: vk::DescriptorSet::null(),
            descriptor_epoch: 0,
            acquired: None,
            acquired_at: None,
            no_image_since: None,
            next_present_id: 1,
            outstanding_presents: Vec::new(),
            present_wait_logged: false,
            presented_generation: 0,
            presented_ok: false,
            needs_recreate: false,
            reacquire: false,
            reacquired_at: None,
            primary: false,
            active: true,
            status: "ready".into(),
            retry_at: None,
            retry_interval: RETRY_INITIAL,
        };
        let result = (|| -> Result<(), String> {
            let descriptor_set_layout = blit.descriptor_set_layout;
            output.acquire_fence = unsafe {
                self.device
                    .create_fence(&vk::FenceCreateInfo::default(), None)
            }
            .map_err(|e| format!("create acquire fence: {e:?}"))?;
            output.submit_fence = unsafe {
                self.device.create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
            }
            .map_err(|e| format!("create submit fence: {e:?}"))?;
            output.command_buffer = unsafe {
                self.device.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.command_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
            }
            .map_err(|e| format!("allocate output command buffer: {e:?}"))?[0];
            let pool_sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: 1,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: 1,
                },
            ];
            output.descriptor_pool = unsafe {
                self.device.create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
            }
            .map_err(|e| format!("create output descriptor pool: {e:?}"))?;
            let set_layouts = [descriptor_set_layout];
            output.descriptor_set = unsafe {
                self.device.allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(output.descriptor_pool)
                        .set_layouts(&set_layouts),
                )
            }
            .map_err(|e| format!("allocate output descriptor set: {e:?}"))?[0];
            self.direct_create_output_swapchain(blit, retired_fences, &mut output)
        })();
        match result {
            Ok(()) => Ok(output),
            Err(err) => {
                self.direct_destroy_output_device_resources(retired_fences, &mut output);
                Err((
                    PendingOutput {
                        connector: output.connector,
                        display: output.display,
                        surface: output.surface,
                        plane_index: output.plane_index,
                        display_mode: output.display_mode,
                        extent: output.mode_extent,
                        refresh_mhz: output.refresh_mhz,
                    },
                    err,
                ))
            }
        }
    }

    fn direct_create_output_swapchain(
        &mut self,
        blit: &mut BlitResources,
        retired_fences: &mut Vec<RetiredFence>,
        output: &mut DirectOutput,
    ) -> Result<(), String> {
        let capabilities = unsafe {
            self.surface_loader
                .get_physical_device_surface_capabilities(self.physical_device, output.surface)
        }
        .map_err(|e| format!("get_surface_capabilities failed: {e:?}"))?;
        let formats = unsafe {
            self.surface_loader
                .get_physical_device_surface_formats(self.physical_device, output.surface)
        }
        .map_err(|e| format!("get_surface_formats failed: {e:?}"))?;
        if formats.is_empty() {
            return Err("No Vulkan surface formats available".into());
        }
        let format = choose_output_format(&formats)?;
        let extent = if capabilities.current_extent.width == u32::MAX {
            vk::Extent2D {
                width: output.mode_extent.width.clamp(
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: output.mode_extent.height.clamp(
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        } else {
            capabilities.current_extent
        };
        if extent.width == 0 || extent.height == 0 {
            return Err("display surface reports a zero extent".into());
        }
        let mut image_count = capabilities.min_image_count.saturating_add(1);
        if self.desktop.present_wait.is_some() {
            // One displaying, one queued, one idle to acquire (see
            // DirectOutput::outstanding_presents); fewer only if the driver caps it.
            image_count = image_count.max(3);
        }
        if capabilities.max_image_count > 0 {
            image_count = image_count.min(capabilities.max_image_count);
        }
        let present_modes = unsafe {
            self.surface_loader
                .get_physical_device_surface_present_modes(self.physical_device, output.surface)
        }
        .map_err(|e| format!("get_surface_present_modes failed: {e:?}"))?;
        let present_mode = if present_modes.contains(&vk::PresentModeKHR::FIFO) {
            vk::PresentModeKHR::FIFO
        } else {
            present_modes
                .first()
                .copied()
                .unwrap_or(vk::PresentModeKHR::FIFO)
        };
        if !capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        {
            return Err("Vulkan surface does not support COLOR_ATTACHMENT usage".into());
        }
        let pre_transform = if capabilities
            .supported_transforms
            .contains(vk::SurfaceTransformFlagsKHR::IDENTITY)
        {
            vk::SurfaceTransformFlagsKHR::IDENTITY
        } else {
            capabilities.current_transform
        };
        let composite_alpha = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::INHERIT,
        ]
        .into_iter()
        .find(|mode| capabilities.supported_composite_alpha.contains(*mode))
        .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);
        let render_pass = self.direct_blit_render_pass(blit, format.format)?;

        // Retire the previous swapchain. Presentation may still reference its
        // images after the blit fence signals; idle-and-retire is the
        // KHR_swapchain fallback without present fences.
        self.device_wait_idle();
        // A pending acquire must resolve (or be parked) before its swapchain
        // goes; without a valid acquire fence this output cannot continue.
        self.direct_wait_output_acquire(retired_fences, output)?;
        self.direct_destroy_output_targets(output);
        let old_swapchain = std::mem::take(&mut output.swapchain);
        let queue_family_indices = [self.queue_family_index];
        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(output.surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .queue_family_indices(&queue_family_indices)
            .pre_transform(pre_transform)
            .composite_alpha(composite_alpha)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old_swapchain);
        let new_swapchain = unsafe { self.swapchain_loader.create_swapchain(&create_info, None) };
        // oldSwapchain is retired by vkCreateSwapchainKHR even if creation fails.
        if old_swapchain != vk::SwapchainKHR::null() {
            unsafe { self.swapchain_loader.destroy_swapchain(old_swapchain, None) };
        }
        output.swapchain = new_swapchain.map_err(|e| format!("create_swapchain failed: {e:?}"))?;
        output.images = unsafe { self.swapchain_loader.get_swapchain_images(output.swapchain) }
            .map_err(|e| format!("get_swapchain_images failed: {e:?}"))?;
        output.format = format.format;
        output.extent = extent;
        for image in &output.images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format.format)
                .components(vk::ComponentMapping::default())
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            let view = unsafe { self.device.create_image_view(&view_info, None) }
                .map_err(|e| format!("create_image_view failed: {e:?}"))?;
            output.views.push(view);
            let attachments = [view];
            let framebuffer = unsafe {
                self.device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(&attachments)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1),
                    None,
                )
            }
            .map_err(|e| format!("create_framebuffer failed: {e:?}"))?;
            output.framebuffers.push(framebuffer);
            let semaphore = unsafe {
                self.device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|e| format!("create_semaphore(present) failed: {e:?}"))?;
            output.present_semaphores.push(semaphore);
        }
        if output.acquire_fence == vk::Fence::null() {
            return Err("output has no acquire fence".into());
        }
        output.acquired = None;
        output.acquired_at = None;
        // A fresh swapchain shows the retained composition as soon as it can
        // acquire; no UI redraw is needed for that. It is `ready`, not yet
        // `active`, until a present on it succeeds.
        output.presented_generation = 0;
        output.presented_ok = false;
        output.needs_recreate = false;
        output.next_present_id = 1;
        output.outstanding_presents.clear();
        output.status = "ready".into();
        Ok(())
    }

    /// Park a pending acquire fence and give the output a fresh one. The
    /// parked fence is destroyed once it signals; the output keeps working.
    fn direct_retire_pending_acquire(
        &mut self,
        retired_fences: &mut Vec<RetiredFence>,
        output: &mut DirectOutput,
    ) -> Result<(), String> {
        let stale = std::mem::take(&mut output.acquire_fence);
        output.acquired = None;
        output.acquired_at = None;
        if stale != vk::Fence::null() {
            retired_fences.push(RetiredFence {
                fence: stale,
                since: Instant::now(),
            });
        }
        output.acquire_fence = unsafe {
            self.device
                .create_fence(&vk::FenceCreateInfo::default(), None)
        }
        .map_err(|e| format!("replace acquire fence: {e:?}"))?;
        Ok(())
    }

    /// Complete a pending fence-only acquire before its swapchain goes away.
    /// The presentation engine normally releases the image within a refresh;
    /// a fence that does not signal is parked, never reset or destroyed while
    /// pending. Errors leave the output without a usable fence; the caller
    /// must retire it rather than reactivate it.
    fn direct_wait_output_acquire(
        &mut self,
        retired_fences: &mut Vec<RetiredFence>,
        output: &mut DirectOutput,
    ) -> Result<(), String> {
        if output.acquired.is_none() {
            return Ok(());
        }
        if output.acquire_fence == vk::Fence::null() {
            output.acquired = None;
            output.acquired_at = None;
            return Err("pending acquire without a fence".into());
        }
        let waited = unsafe {
            self.device
                .wait_for_fences(&[output.acquire_fence], true, ACQUIRE_TEARDOWN_TIMEOUT_NS)
        };
        match waited {
            Ok(()) => {
                output.acquired = None;
                output.acquired_at = None;
                unsafe { self.device.reset_fences(&[output.acquire_fence]) }
                    .map_err(|e| format!("reset acquire fence: {e:?}"))
            }
            Err(vk::Result::TIMEOUT) => {
                crate::warning!(
                    "Vulkan direct: {} acquire fence did not signal within 1 s; parking it",
                    output.connector.name
                );
                self.direct_retire_pending_acquire(retired_fences, output)
            }
            Err(result) => {
                output.acquired = None;
                output.acquired_at = None;
                Err(format!("wait for acquire fence: {result:?}"))
            }
        }
    }

    fn direct_destroy_output_targets(&self, output: &mut DirectOutput) {
        unsafe {
            for framebuffer in output.framebuffers.drain(..) {
                self.device.destroy_framebuffer(framebuffer, None);
            }
            for view in output.views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
            for semaphore in output.present_semaphores.drain(..) {
                self.device.destroy_semaphore(semaphore, None);
            }
        }
        output.images.clear();
    }

    /// Requires an idle device. Leaves the surface and display for the
    /// instance-level release. A fence still pending on the WSI is parked,
    /// never destroyed.
    fn direct_destroy_output_device_resources(
        &mut self,
        retired_fences: &mut Vec<RetiredFence>,
        output: &mut DirectOutput,
    ) {
        if let Err(err) = self.direct_wait_output_acquire(retired_fences, output) {
            crate::warning!("Vulkan direct: {} teardown: {err}", output.connector.name);
        }
        self.direct_destroy_output_targets(output);
        unsafe {
            if output.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_loader
                    .destroy_swapchain(output.swapchain, None);
                output.swapchain = vk::SwapchainKHR::null();
            }
            if output.descriptor_pool != vk::DescriptorPool::null() {
                self.device
                    .destroy_descriptor_pool(output.descriptor_pool, None);
                output.descriptor_pool = vk::DescriptorPool::null();
                output.descriptor_set = vk::DescriptorSet::null();
            }
            if output.command_buffer != vk::CommandBuffer::null() {
                self.device
                    .free_command_buffers(self.command_pool, &[output.command_buffer]);
                output.command_buffer = vk::CommandBuffer::null();
            }
            if output.acquire_fence != vk::Fence::null() {
                self.device.destroy_fence(output.acquire_fence, None);
                output.acquire_fence = vk::Fence::null();
            }
            if output.submit_fence != vk::Fence::null() {
                self.device.destroy_fence(output.submit_fence, None);
                output.submit_fence = vk::Fence::null();
            }
        }
        output.active = false;
        output.presented_ok = false;
        output.primary = false;
    }

    /// Destroy parked acquire fences that have signaled since.
    fn direct_drain_retired_fences(&mut self, direct: &mut DirectState) {
        let mut index = 0;
        while index < direct.retired_fences.len() {
            let signaled = unsafe {
                self.device
                    .get_fence_status(direct.retired_fences[index].fence)
            }
            .unwrap_or(false);
            if signaled {
                let retired = direct.retired_fences.swap_remove(index);
                unsafe { self.device.destroy_fence(retired.fence, None) };
            } else {
                index += 1;
            }
        }
    }

    /// Take an output offline after a failure: its GPU resources, surface and
    /// display ownership go away; the entry stays so the UI sees the reason
    /// and a later reconcile can try to reacquire it.
    fn direct_retire_output(&mut self, direct: &mut DirectState, index: usize, status: String) {
        self.device_wait_idle();
        {
            let DirectState {
                retired_fences,
                outputs,
                ..
            } = &mut *direct;
            let output = &mut outputs[index];
            crate::warning!("Vulkan direct: {} retired: {status}", output.connector.name);
            self.direct_destroy_output_device_resources(retired_fences, output);
        }
        let output = &mut direct.outputs[index];
        let surface = std::mem::take(&mut output.surface);
        let display = std::mem::take(&mut output.display);
        let interval = output.retry_interval;
        output.status = status;
        output.reacquire = false;
        output.retry_at = Some(Instant::now() + interval);
        output.retry_interval = (interval * 2).min(RETRY_MAX);
        direct.release_surface_and_display(surface, display);
        // The source may have gone; the next reconcile picks a fallback and
        // reports the geometry change.
        direct.scan_dirty = true;
    }

    /// Build a replacement composition (image, depth, render pass,
    /// framebuffer, readback) without touching the current one. Nothing is
    /// installed here; a failure cleans up only what this call created.
    fn direct_build_composition(&self, extent: vk::Extent2D) -> Result<CompositionTargets, String> {
        let depth_format = self.pick_depth_format()?;
        let resource = self.create_color_target_resource_with_usage(
            extent.width,
            extent.height,
            COMPOSITION_FORMAT,
            false,
            vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        let depth = match self.create_depth_target(extent.width, extent.height, depth_format) {
            Ok(depth) => depth,
            Err(err) => {
                self.destroy_texture_resource(resource);
                return Err(err);
            }
        };
        let render_pass = match self.direct_create_composition_render_pass(depth_format) {
            Ok(render_pass) => render_pass,
            Err(err) => {
                self.destroy_texture_resource(depth);
                self.destroy_texture_resource(resource);
                return Err(err);
            }
        };
        let attachments = [resource.view, depth.view];
        let framebuffer = match unsafe {
            self.device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass)
                    .attachments(&attachments)
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1),
                None,
            )
        } {
            Ok(framebuffer) => framebuffer,
            Err(e) => {
                unsafe { self.device.destroy_render_pass(render_pass, None) };
                self.destroy_texture_resource(depth);
                self.destroy_texture_resource(resource);
                return Err(format!("create composition framebuffer: {e:?}"));
            }
        };
        // Explicit `/g` captures read the composition; nothing else does.
        let readback = match self.create_host_buffer(
            vk::BufferUsageFlags::TRANSFER_DST,
            extent.width as vk::DeviceSize * extent.height as vk::DeviceSize * 4,
        ) {
            Ok(readback) => readback,
            Err(err) => {
                unsafe {
                    self.device.destroy_framebuffer(framebuffer, None);
                    self.device.destroy_render_pass(render_pass, None);
                }
                self.destroy_texture_resource(depth);
                self.destroy_texture_resource(resource);
                return Err(err);
            }
        };
        Ok(CompositionTargets {
            resource,
            render_pass,
            depth,
            framebuffer,
            readback,
            depth_format,
            extent,
        })
    }

    fn direct_create_composition_render_pass(
        &self,
        depth_format: vk::Format,
    ) -> Result<vk::RenderPass, String> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(COMPOSITION_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let depth_attachment = vk::AttachmentDescription::default()
            .format(depth_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_refs = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth_ref = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs)
            .depth_stencil_attachment(&depth_ref);
        // In: wait for every earlier blit/capture read of this image.
        // Out: make the new frame visible to the blits and the capture copy.
        let dependencies = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(
                    vk::PipelineStageFlags::FRAGMENT_SHADER
                        | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS
                        | vk::PipelineStageFlags::TRANSFER,
                )
                .src_access_mask(
                    vk::AccessFlags::SHADER_READ
                        | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE
                        | vk::AccessFlags::TRANSFER_READ,
                )
                .dst_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                )
                .dst_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                ),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::TRANSFER,
                )
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::TRANSFER_READ),
        ];
        let attachments = [color_attachment, depth_attachment];
        let subpasses = [subpass];
        unsafe {
            self.device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpasses)
                    .dependencies(&dependencies),
                None,
            )
        }
        .map_err(|e| format!("create composition render pass: {e:?}"))
    }

    /// Swap in a fully built composition at an idle boundary: the old
    /// targets and pipelines go (exactly as a swapchain recreation does) and
    /// the main-target fields describe the new image.
    fn direct_install_composition(
        &mut self,
        direct: &mut DirectState,
        targets: CompositionTargets,
    ) {
        let extent = targets.extent;
        let resource = self.install_composition_targets(targets);
        if let Some(old) = direct.composition.replace(resource) {
            self.destroy_texture_resource(old);
        }
        direct.desktop_extent = extent;
        direct.composition_epoch += 1;
        direct.composition_valid = false;
    }

    fn install_composition_targets(
        &mut self,
        targets: CompositionTargets,
    ) -> VulkanTextureResource {
        self.device_wait_idle();
        self.destroy_frame_resources();
        self.destroy_pipelines();
        self.destroy_swapchain_targets();
        self.swapchain_format = COMPOSITION_FORMAT;
        self.depth_format = targets.depth_format;
        self.swapchain_extent = targets.extent;
        self.render_pass = targets.render_pass;
        self.swapchain_depth_targets.push(targets.depth);
        self.framebuffers.push(targets.framebuffer);
        self.swapchain_readback_buffer = Some(targets.readback);
        targets.resource
    }

    fn destroy_composition_targets(&self, targets: CompositionTargets) {
        unsafe {
            self.device.destroy_framebuffer(targets.framebuffer, None);
            self.device.destroy_render_pass(targets.render_pass, None);
            self.device.destroy_buffer(targets.readback.buffer, None);
            self.device.free_memory(targets.readback.memory, None);
        }
        self.destroy_texture_resource(targets.depth);
        self.destroy_texture_resource(targets.resource);
    }

    /// Choose the render source and, when its mode differs from the desktop,
    /// replace the composition transactionally. Nothing observable (primary
    /// flags, source name, the pending request) changes unless the swap
    /// succeeded, except when no source is active at all: then the fallback
    /// output is published so rendering continues at the old desktop size
    /// while the swap is retried (bounded).
    /// Returns the new desktop extent when it changed.
    fn direct_select_source(
        &mut self,
        direct: &mut DirectState,
    ) -> Result<Option<vk::Extent2D>, String> {
        let now = Instant::now();
        let explicit = direct.pending_source.clone();
        let request = explicit.clone().or_else(|| direct.preferred_source.clone());
        let requested = request.as_deref().and_then(|name| {
            direct
                .outputs
                .iter()
                .position(|output| output.active && output.connector.name == name)
        });
        let current = direct.source_index();
        let fallback = direct
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, output)| output.active)
            .min_by_key(|(_, output)| connector_rank(&output.connector.name))
            .map(|(index, _)| index);
        let Some(target) = requested.or(current).or(fallback) else {
            for output in &mut direct.outputs {
                output.primary = false;
            }
            if direct.source_name.take().is_some() {
                crate::warning!("Vulkan direct: no active output; the desktop keeps its last size until one returns");
            }
            return Ok(None);
        };
        let extent = direct.outputs[target].mode_extent;
        let explicit_target = explicit.is_some() && requested == Some(target);
        let needs_swap = direct.composition.is_none() || direct.desktop_extent != extent;
        let mut installed = None;
        if needs_swap {
            let exhausted = direct.source_switch_failures >= SOURCE_SWITCH_RETRIES;
            let waiting = direct.source_retry_at.is_some_and(|at| at > now);
            if exhausted || waiting {
                // Exhausted explicit requests were finalized on their last
                // failed attempt below; what remains here is either a retry
                // interval still running, or a fallback with no source that
                // keeps the old desktop size until the connector list changes.
                if current.is_some() {
                    return Ok(None);
                }
            } else if direct.defer_composition_resize {
                direct.composition_resize_request = Some(extent);
                if current.is_some() {
                    return Ok(None);
                }
                // A newly connected output may show the old-sized retained
                // desktop while both replacement devices are prepared.
            } else {
                match self.direct_build_composition(extent) {
                    Ok(targets) => {
                        self.direct_install_composition(direct, targets);
                        direct.source_switch_failures = 0;
                        direct.source_retry_at = None;
                        installed = Some(extent);
                    }
                    Err(err) => {
                        direct.source_switch_failures += 1;
                        let last_attempt = direct.source_switch_failures >= SOURCE_SWITCH_RETRIES;
                        direct.source_retry_at = if last_attempt {
                            None
                        } else {
                            Some(now + SOURCE_RETRY_INTERVAL)
                        };
                        crate::warning!(
                            "Vulkan direct: composition {}x{} for {} failed ({err}); attempt {}/{}",
                            extent.width,
                            extent.height,
                            direct.outputs[target].connector.name,
                            direct.source_switch_failures,
                            SOURCE_SWITCH_RETRIES
                        );
                        if last_attempt && explicit_target {
                            // Final failure of an explicit request: reject it
                            // now and keep the working source, so no intent
                            // stays pending without a reconcile to clear it.
                            crate::error!(
                                "Vulkan direct: giving up rendering for {}: composition allocation failed {} times; keeping {}",
                                direct.outputs[target].connector.name,
                                SOURCE_SWITCH_RETRIES,
                                direct.source_name.as_deref().unwrap_or("the current source")
                            );
                            direct.pending_source = None;
                            direct.preferred_source = direct.source_name.clone();
                            direct.source_switch_failures = 0;
                            return Ok(None);
                        }
                        if current.is_some() {
                            // Keep the current source untouched; the request
                            // stays pending for the bounded retry.
                            return Ok(None);
                        }
                        // No source at all: publish the fallback at the old
                        // desktop size (retried until exhausted or the list changes).
                    }
                }
            }
        }
        for output in &mut direct.outputs {
            output.primary = false;
        }
        direct.outputs[target].primary = true;
        if explicit_target && !(needs_swap && direct.defer_composition_resize) {
            direct.pending_source = None;
        }
        let name = direct.outputs[target].connector.name.clone();
        if direct.source_name.as_deref() != Some(name.as_str()) {
            crate::log!(
                "Vulkan direct: rendering for {name} at {}x{} @ {:.3} Hz; other outputs clone it",
                extent.width,
                extent.height,
                direct.outputs[target].refresh_hz()
            );
            direct.source_name = Some(name);
        }
        Ok(installed)
    }

    /// UI request: render for `name` from the next frame boundary on. Only
    /// validates and queues; nothing waits or mode-sets here.
    pub(crate) fn direct_request_display_source(&mut self, name: &str) -> Result<(), String> {
        if let Some(routed) = &mut self.desktop.routed {
            return routed.display.direct_request_display_source(name);
        }
        let Some(direct) = self.desktop.direct.as_mut() else {
            return Err(
                "display source selection needs the direct Vulkan backend (DRM/KMS)".into(),
            );
        };
        if !direct
            .connected
            .iter()
            .any(|connector| connector.name == name)
        {
            return Err(format!("{name} is not a connected DRM connector"));
        }
        if let Some((_, status)) = direct
            .unsupported
            .iter()
            .find(|(connector, _)| connector.name == name)
        {
            return Err(format!("{name} cannot render the desktop: {status}"));
        }
        match direct
            .outputs
            .iter()
            .find(|output| output.connector.name == name)
        {
            Some(output) if output.active => {}
            Some(output) => return Err(format!("{name} is not ready: {}", output.status)),
            None => {
                return Err(format!(
                    "{name} is not driven by this renderer yet; retry after the next display scan"
                ))
            }
        }
        direct.preferred_source = Some(name.to_owned());
        direct.pending_source = Some(name.to_owned());
        direct.source_switch_failures = 0;
        direct.source_retry_at = None;
        direct.scan_dirty = true;
        Ok(())
    }

    /// Paint-time reconcile: hotplug snapshots, retries of failed outputs and
    /// pending source requests. Returns the new desktop extent when the
    /// composition size changed so the caller can update window geometry.
    pub(crate) fn direct_reconcile_outputs(&mut self) -> Result<Option<vk::Extent2D>, String> {
        if self.desktop.routed.is_some() {
            return self.routed_reconcile();
        }
        let Some(mut direct) = self.desktop.direct.take() else {
            return Ok(None);
        };
        let result = self.direct_reconcile(&mut direct);
        self.desktop.direct = Some(direct);
        result
    }

    fn direct_reconcile(
        &mut self,
        direct: &mut DirectState,
    ) -> Result<Option<vk::Extent2D>, String> {
        // Drain every worker publication: the newest list wins, all events count.
        let mut events = Vec::new();
        let mut latest = None;
        if let Some(watch) = &direct.hotplug {
            while let Ok(scan) = watch.receiver.try_recv() {
                events.extend(scan.events);
                latest = Some(scan.connectors);
            }
        }
        let mut list_changed = false;
        if let Some(connectors) = latest {
            if connectors != direct.connected {
                crate::log!(
                    "Direct display: connectors now {}",
                    connector_names(&connectors)
                );
                direct.connected = connectors;
                direct.scan_dirty = true;
                direct.source_switch_failures = 0;
                direct.source_retry_at = None;
                list_changed = true;
            }
        }
        let now = Instant::now();
        // Kernel events name outputs whose DRM ownership may have been lost
        // and regained even though sysfs looks identical: release and
        // reacquire exactly those. Connector-level events (CONNECTOR=) name
        // one output; a card-level HOTPLUG=1 with an unchanged connector list
        // (same monitor, fast replug) reacquires that card's outputs. When the
        // list changed, the card-level event is the plug/unplug already handled
        // by the diff and does not disturb the untouched outputs. Events inside
        // REACQUIRE_SUPPRESS of our own reacquire are our modeset's echo.
        for event in &events {
            let card = event.card.as_deref();
            for output in &mut direct.outputs {
                if !output.active || output.reacquire {
                    continue;
                }
                if output
                    .reacquired_at
                    .is_some_and(|at| now.duration_since(at) < REACQUIRE_SUPPRESS)
                {
                    continue;
                }
                let named = match event.connector_id {
                    Some(connector_id) => {
                        output.connector.id == connector_id
                            && card.is_none_or(|card| card == output.card_name())
                    }
                    None => {
                        event.hotplug
                            && !list_changed
                            && card.is_some_and(|card| card == output.card_name())
                    }
                };
                if named {
                    crate::log!(
                        "Direct display: kernel event for {}; reacquiring",
                        output.connector.name
                    );
                    output.reacquire = true;
                    direct.scan_dirty = true;
                }
            }
        }
        let retry_due = direct
            .outputs
            .iter()
            .any(|output| !output.active && output.retry_at.is_some_and(|at| at <= now));
        let source_retry_due = direct.source_retry_at.is_some_and(|at| at <= now)
            && direct.source_switch_failures < SOURCE_SWITCH_RETRIES;
        if !direct.scan_dirty && !retry_due && !source_retry_due {
            return Ok(None);
        }
        let scan_changed = std::mem::take(&mut direct.scan_dirty);
        let connected = direct.connected.clone();

        // Unplugged (or replaced: fingerprint differs) connectors: their DRM
        // ownership ends with the cable, so the entry is dropped and the
        // connector rejoins through the add path below if still connected.
        let mut index = 0;
        while index < direct.outputs.len() {
            let stale = !connected.contains(&direct.outputs[index].connector);
            let reacquire = direct.outputs[index].reacquire;
            if !stale && !reacquire {
                index += 1;
                continue;
            }
            if direct.outputs[index].active {
                let status = if stale {
                    "unplugged".to_string()
                } else {
                    "reacquiring after hotplug event".to_string()
                };
                self.direct_retire_output(direct, index, status);
            }
            let output = direct.outputs.remove(index);
            crate::log!("Vulkan direct: {} removed", output.connector.name);
            direct.release_surface_and_display(output.surface, output.display);
            if reacquire {
                if let Some(connector) = connected.iter().find(|c| c.name == output.connector.name)
                {
                    self.direct_try_add_output(direct, connector.clone(), RETRY_INITIAL);
                    if let Some(added) = direct
                        .outputs
                        .iter_mut()
                        .find(|o| o.connector.name == output.connector.name)
                    {
                        added.reacquired_at = Some(now);
                    }
                }
            }
        }
        if scan_changed {
            direct
                .unsupported
                .retain(|(connector, _)| connected.contains(connector));
        }

        // New connectors, and failed ones whose retry is due.
        for connector in &connected {
            if let Some(index) = direct
                .outputs
                .iter()
                .position(|output| &output.connector == connector)
            {
                if direct.outputs[index].active
                    || !direct.outputs[index].retry_at.is_some_and(|at| at <= now)
                {
                    continue;
                }
                let previous = direct.outputs.remove(index);
                let retry_interval = (previous.retry_interval * 2).min(RETRY_MAX);
                self.direct_try_add_output(direct, connector.clone(), retry_interval);
            } else if direct
                .unsupported
                .iter()
                .any(|(unsupported, _)| unsupported == connector)
            {
                if scan_changed {
                    direct
                        .unsupported
                        .retain(|(unsupported, _)| unsupported != connector);
                    self.direct_try_add_output(direct, connector.clone(), RETRY_INITIAL);
                }
            } else {
                self.direct_try_add_output(direct, connector.clone(), RETRY_INITIAL);
            }
        }
        direct.prune_cards();
        self.direct_select_source(direct)
    }

    /// Acquire one connector after init. Planes already held by live outputs
    /// stay theirs; the newcomer takes any remaining compatible plane.
    fn direct_try_add_output(
        &mut self,
        direct: &mut DirectState,
        connector: DrmConnector,
        retry_interval: Duration,
    ) {
        if let Err(err) = self.direct_ensure_blit(direct) {
            direct.outputs.push(DirectOutput::inactive(
                connector,
                format!("failed: {err}"),
                retry_interval,
            ));
            return;
        }
        let fd = match direct.card_fd(&connector.card) {
            Ok(fd) => fd,
            Err(err) => {
                direct
                    .unsupported
                    .push((connector, format!("unsupported: {err}")));
                return;
            }
        };
        let physical_device = direct.physical_device;
        let display = match unsafe {
            direct
                .drm_loader
                .get_drm_display(physical_device, fd, connector.id)
        } {
            Ok(display) if display != vk::DisplayKHR::null() => display,
            _ => {
                direct.unsupported.push((
                    connector,
                    "unsupported: driven by another GPU; clone outputs must share the rendering GPU".into(),
                ));
                return;
            }
        };
        if let Err(err) = unsafe {
            direct
                .drm_loader
                .acquire_drm_display(physical_device, fd, display)
        } {
            direct.outputs.push(DirectOutput::inactive(
                connector,
                format!("failed: acquire display {err:?} (requires DRM master on an active VT)"),
                retry_interval,
            ));
            return;
        }
        let native_resolution = drm_native_resolution(fd, connector.id);
        let mode = match select_display_mode(
            &direct.display_loader,
            physical_device,
            display,
            None,
            native_resolution,
        ) {
            Ok(mode) => mode,
            Err(err) => {
                direct.release_surface_and_display(vk::SurfaceKHR::null(), display);
                direct.outputs.push(DirectOutput::inactive(
                    connector,
                    format!("failed: {err}"),
                    retry_interval,
                ));
                return;
            }
        };
        let planned = self
            .direct_claim_plane(direct, display, &mode)
            .and_then(|candidate| {
                let surface = create_plane_surface(&direct.display_loader, &mode, &candidate)?;
                Ok((candidate, surface))
            });
        let (candidate, surface) = match planned {
            Ok(planned) => planned,
            Err(err) => {
                direct.release_surface_and_display(vk::SurfaceKHR::null(), display);
                direct.outputs.push(DirectOutput::inactive(
                    connector,
                    format!("failed: {err}"),
                    retry_interval,
                ));
                return;
            }
        };
        let pending = PendingOutput {
            connector,
            display,
            surface,
            plane_index: candidate.plane_index,
            display_mode: mode.mode,
            extent: mode.extent,
            refresh_mhz: mode.refresh_mhz,
        };
        let created = {
            let DirectState {
                blit,
                retired_fences,
                ..
            } = &mut *direct;
            match blit.as_mut() {
                Some(blit) => self.direct_create_output(blit, retired_fences, pending),
                None => Err((pending, "blit resources missing".to_string())),
            }
        };
        match created {
            Ok(output) => {
                crate::log!(
                    "Vulkan direct: {} added on plane {}, {}x{} @ {:.3} Hz",
                    output.connector.name,
                    candidate.plane_index,
                    mode.extent.width,
                    mode.extent.height,
                    output.refresh_hz()
                );
                direct.outputs.push(output);
            }
            Err((pending, err)) => {
                crate::warning!("Vulkan direct: {} failed: {err}", pending.connector.name);
                direct.release_surface_and_display(pending.surface, pending.display);
                direct.outputs.push(DirectOutput::inactive(
                    pending.connector,
                    format!("failed: {err}"),
                    retry_interval,
                ));
            }
        }
    }

    /// A plane for a newcomer display. First any compatible plane no live
    /// output holds; failing that, re-run the matching over the live outputs
    /// plus the newcomer (each live output lists its current plane first so
    /// the matching only moves what it must) and move the outputs whose
    /// assignment changed to their new plane. Display ownership and the
    /// composition are untouched by a move; only the surface and swapchain
    /// are rebuilt.
    fn direct_claim_plane(
        &mut self,
        direct: &mut DirectState,
        display: vk::DisplayKHR,
        mode: &DisplayMode,
    ) -> Result<PlaneCandidate, String> {
        let physical_device = direct.physical_device;
        let owned = direct.reserved_planes();
        let free = plane_candidates(
            &direct.display_loader,
            physical_device,
            display,
            mode,
            &owned,
            &HashSet::new(),
        )?;
        if let Some(candidate) = free.first() {
            return Ok(*candidate);
        }
        let none = HashSet::new();
        let mut plans: Vec<(Option<usize>, Vec<PlaneCandidate>)> = Vec::new();
        for (index, output) in direct.outputs.iter().enumerate() {
            if !output.active || output.surface == vk::SurfaceKHR::null() {
                continue;
            }
            let output_mode = DisplayMode {
                mode: output.display_mode,
                extent: output.mode_extent,
                refresh_mhz: output.refresh_mhz,
            };
            let mut candidates = plane_candidates(
                &direct.display_loader,
                physical_device,
                output.display,
                &output_mode,
                &none,
                &owned,
            )?;
            candidates.sort_by_key(|candidate| candidate.plane_index != output.plane_index);
            plans.push((Some(index), candidates));
        }
        let newcomer = plane_candidates(
            &direct.display_loader,
            physical_device,
            display,
            mode,
            &none,
            &owned,
        )?;
        if newcomer.is_empty() {
            return Err("no display plane can present the selected mode".into());
        }
        plans.push((None, newcomer));
        let plane_sets: Vec<Vec<u32>> = plans
            .iter()
            .map(|(_, candidates)| {
                candidates
                    .iter()
                    .map(|candidate| candidate.plane_index)
                    .collect()
            })
            .collect();
        let assignment = assign_planes(&plane_sets);
        let Some(new_plane) = assignment.last().copied().flatten() else {
            return Err("no free display plane can present the selected mode (no rearrangement of the running outputs frees one)".into());
        };
        // Phase 1: every output whose assignment changes releases its old
        // plane (swapchain, surface) while keeping its display and metadata.
        // Only then can any new surface be created: with chained moves
        // (A: 0→1, B: 1→2, newcomer→0) a plane is still occupied until its
        // previous owner has vacated it.
        let mut moves = Vec::new();
        for ((slot, candidates), plane) in plans.iter().zip(&assignment) {
            let (Some(index), Some(plane)) = (slot, plane) else {
                continue;
            };
            if direct.outputs[*index].plane_index == *plane {
                continue;
            }
            let Some(candidate) = candidates
                .iter()
                .find(|candidate| candidate.plane_index == *plane)
                .copied()
            else {
                continue;
            };
            moves.push(self.direct_vacate_output_plane(direct, *index, candidate));
        }
        // Phase 2: rebuild the vacated outputs on their planned planes, in
        // their original order. The newcomer's surface is created by the
        // caller afterwards, on a plane that is free by now.
        for plane_move in moves {
            self.direct_reoccupy_output_plane(direct, plane_move);
        }
        plans
            .last()
            .and_then(|(_, candidates)| {
                candidates
                    .iter()
                    .find(|candidate| candidate.plane_index == new_plane)
                    .copied()
            })
            .ok_or_else(|| "matched plane is not a candidate".to_string())
    }

    /// Phase 1 of a plane move: the output's swapchain and surface go at an
    /// idle boundary, its entry becomes a placeholder and everything needed
    /// to rebuild it is returned. The display stays acquired.
    fn direct_vacate_output_plane(
        &mut self,
        direct: &mut DirectState,
        index: usize,
        candidate: PlaneCandidate,
    ) -> PlaneMove {
        let placeholder = DirectOutput::inactive(
            direct.outputs[index].connector.clone(),
            "moving plane".into(),
            RETRY_INITIAL,
        );
        let mut output = std::mem::replace(&mut direct.outputs[index], placeholder);
        // Destroying the device resources clears the source role; keep it.
        let was_primary = output.primary;
        crate::log!(
            "Vulkan direct: {} moves from plane {} to plane {}",
            output.connector.name,
            output.plane_index,
            candidate.plane_index
        );
        self.device_wait_idle();
        {
            let DirectState { retired_fences, .. } = &mut *direct;
            self.direct_destroy_output_device_resources(retired_fences, &mut output);
        }
        // Only the surface changes plane; the display stays ours.
        direct.release_surface_and_display(
            std::mem::take(&mut output.surface),
            vk::DisplayKHR::null(),
        );
        PlaneMove {
            index,
            candidate,
            was_primary,
            connector: output.connector,
            display: output.display,
            display_mode: output.display_mode,
            extent: output.mode_extent,
            refresh_mhz: output.refresh_mhz,
        }
    }

    /// Phase 2 of a plane move: surface on the planned plane and a fresh
    /// swapchain. The entry keeps its index and source role and is `ready`
    /// until it presents; a failure leaves it inactive with the error and
    /// the display released, on the ordinary retry path.
    fn direct_reoccupy_output_plane(&mut self, direct: &mut DirectState, plane_move: PlaneMove) {
        let PlaneMove {
            index,
            candidate,
            was_primary,
            connector,
            display,
            display_mode,
            extent,
            refresh_mhz,
        } = plane_move;
        let mode = DisplayMode {
            mode: display_mode,
            extent,
            refresh_mhz,
        };
        let surface = match create_plane_surface(&direct.display_loader, &mode, &candidate) {
            Ok(surface) => surface,
            Err(err) => {
                direct.release_surface_and_display(vk::SurfaceKHR::null(), display);
                direct.outputs[index] =
                    DirectOutput::inactive(connector, format!("failed: {err}"), RETRY_INITIAL);
                return;
            }
        };
        let pending = PendingOutput {
            connector,
            display,
            surface,
            plane_index: candidate.plane_index,
            display_mode,
            extent,
            refresh_mhz,
        };
        let created = {
            let DirectState {
                blit,
                retired_fences,
                ..
            } = &mut *direct;
            match blit.as_mut() {
                Some(blit) => self.direct_create_output(blit, retired_fences, pending),
                None => Err((pending, "blit resources missing".to_string())),
            }
        };
        direct.outputs[index] = match created {
            Ok(mut rebuilt) => {
                rebuilt.primary = was_primary;
                rebuilt
            }
            Err((pending, err)) => {
                crate::warning!(
                    "Vulkan direct: {} failed after the plane move: {err}",
                    pending.connector.name
                );
                direct.release_surface_and_display(pending.surface, pending.display);
                DirectOutput::inactive(pending.connector, format!("failed: {err}"), RETRY_INITIAL)
            }
        };
    }

    fn fence_signaled(&self, fence: vk::Fence) -> Result<bool, String> {
        unsafe { self.device.get_fence_status(fence) }
            .map_err(|e| format!("get_fence_status failed: {e:?}"))
    }

    /// The source can take a new frame: it holds a released image and its
    /// previous blit finished, so the command buffer is free to re-record.
    fn direct_output_can_take_frame(&self, output: &DirectOutput) -> Result<bool, String> {
        Ok(output.acquired.is_some()
            && self.fence_signaled(output.acquire_fence)?
            && self.fence_signaled(output.submit_fence)?)
    }

    /// Present the retained composition to every output that is behind and
    /// can take it now. Runs on every paint tick, dirty or not, so a clone
    /// that was busy (NOT_READY) catches up without a UI redraw. Native
    /// readiness is recomputed from the live fence and source after service
    /// so a paint gate cannot keep Intel stuck on a stale Ready/GpuBusy.
    pub(crate) fn direct_present_retained(&mut self) -> Result<(), String> {
        if let Some(mut routed) = self.desktop.routed.take() {
            let result = self.routed_service(&mut routed);
            self.desktop.routed = Some(routed);
            return result;
        }
        let Some(mut direct) = self.desktop.direct.take() else {
            return Ok(());
        };
        let result = (|| {
            direct.wait = DirectWait::GpuBusy;
            self.direct_service_outputs(&mut direct)?;
            // Captures may render with NoOutput, but still need an idle
            // main command buffer before admitting the entire pass chain.
            direct.wait = if !self.fence_signaled(self.in_flight_fence)? {
                DirectWait::GpuBusy
            } else if direct.composition.is_none() {
                DirectWait::NoOutput
            } else {
                match direct.source_index() {
                    None => DirectWait::NoOutput,
                    Some(index) => {
                        if self.direct_output_can_take_frame(&direct.outputs[index])? {
                            DirectWait::Ready
                        } else {
                            DirectWait::GpuBusy
                        }
                    }
                }
            };
            Ok(())
        })();
        if result.is_err() {
            direct.wait = DirectWait::GpuBusy;
        }
        self.desktop.direct = Some(direct);
        result
    }

    /// What the event loop should do after the last frame attempt.
    pub(crate) fn direct_wait_reason(&self) -> DirectWait {
        if self.gpu_transition_pending() {
            return DirectWait::GpuBusy;
        }
        if let Some(routed) = &self.desktop.routed {
            return routed.wait;
        }
        self.desktop
            .direct
            .as_ref()
            .map(|direct| direct.wait)
            .unwrap_or(DirectWait::Ready)
    }

    fn direct_service_outputs(&mut self, direct: &mut DirectState) -> Result<(), String> {
        self.direct_drain_retired_fences(direct);
        let present_wait = self.desktop.present_wait.clone();
        let now = Instant::now();
        for index in 0..direct.outputs.len() {
            if !direct.outputs[index].active {
                continue;
            }
            if direct.outputs[index].needs_recreate {
                let result = {
                    let DirectState {
                        blit,
                        retired_fences,
                        outputs,
                        ..
                    } = &mut *direct;
                    match blit.as_mut() {
                        Some(blit) => self.direct_create_output_swapchain(
                            blit,
                            retired_fences,
                            &mut outputs[index],
                        ),
                        None => Err("blit resources missing".to_string()),
                    }
                };
                if let Err(err) = result {
                    self.direct_retire_output(direct, index, format!("lost: {err}"));
                }
                continue;
            }
            if direct.outputs[index].acquired.is_none() {
                if let Some(present_wait) = &present_wait {
                    // Present-wait pacing: only ask the WSI for an image when
                    // an idle one is guaranteed. Mesa 26.2's display WSI maps
                    // the internal wait of a timeout-0 acquire that finds no
                    // idle image to SURFACE_LOST and poisons the swapchain,
                    // so the acquire must never have to wait.
                    let mut blocked = false;
                    loop {
                        let output = &direct.outputs[index];
                        let limit = output.images.len().saturating_sub(2);
                        if output.outstanding_presents.len() <= limit {
                            break;
                        }
                        let (present_id, since) = output.outstanding_presents[0];
                        let swapchain = output.swapchain;
                        match unsafe { present_wait.wait_for_present(swapchain, present_id, 0) } {
                            Ok(()) => {
                                let output = &mut direct.outputs[index];
                                output.outstanding_presents.remove(0);
                                if !output.present_wait_logged {
                                    output.present_wait_logged = true;
                                    crate::log!(
                                        "Vulkan direct: {} paced by VK_KHR_present_wait; present id {present_id} completed with {} swapchain images",
                                        output.connector.name,
                                        output.images.len()
                                    );
                                }
                            }
                            Err(vk::Result::TIMEOUT) => {
                                // Still scanning out. A present that never
                                // completes is a real stall: reacquire.
                                if now.duration_since(since) > ACQUIRE_WATCHDOG
                                    && !direct.outputs[index].reacquire
                                {
                                    crate::warning!(
                                        "Vulkan direct: {} present id {present_id} did not complete within {:?}; releasing and reacquiring it",
                                        direct.outputs[index].connector.name,
                                        ACQUIRE_WATCHDOG
                                    );
                                    direct.outputs[index].reacquire = true;
                                    direct.scan_dirty = true;
                                }
                                blocked = true;
                                break;
                            }
                            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                                direct.outputs[index].needs_recreate = true;
                                direct.outputs[index].presented_ok = false;
                                blocked = true;
                                break;
                            }
                            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                                self.direct_retire_output(
                                    direct,
                                    index,
                                    "lost: display surface lost while waiting for a present".into(),
                                );
                                blocked = true;
                                break;
                            }
                            Err(err) => {
                                self.direct_retire_output(
                                    direct,
                                    index,
                                    format!("lost: wait_for_present {err:?}"),
                                );
                                blocked = true;
                                break;
                            }
                        }
                    }
                    if blocked {
                        continue;
                    }
                }
                let output = &direct.outputs[index];
                match unsafe {
                    self.swapchain_loader.acquire_next_image(
                        output.swapchain,
                        0,
                        vk::Semaphore::null(),
                        output.acquire_fence,
                    )
                } {
                    Ok((image, _suboptimal)) => {
                        let output = &mut direct.outputs[index];
                        output.acquired = Some(image);
                        output.acquired_at = Some(now);
                        output.no_image_since = None;
                    }
                    Err(vk::Result::NOT_READY | vk::Result::TIMEOUT) => {
                        // No-image watchdog: only while there is demand for
                        // an image. The source is demand by itself: at idle
                        // it already holds an acquired image and never gets
                        // here, so NOT_READY on it means a frame (the first
                        // one included) cannot be composed until it gets one.
                        // A clone is demand only while a newer composition is
                        // waiting for it; an idle clone is never timed out.
                        let behind = direct.outputs[index].primary
                            || (direct.composition_valid
                                && direct.outputs[index].presented_generation
                                    != direct.composition_generation);
                        let output = &mut direct.outputs[index];
                        if !behind {
                            output.no_image_since = None;
                            continue;
                        }
                        let since = *output.no_image_since.get_or_insert(now);
                        if now.duration_since(since) > ACQUIRE_WATCHDOG && !output.reacquire {
                            crate::warning!(
                                "Vulkan direct: {} handed back no image for over {:?}; releasing and reacquiring it",
                                output.connector.name,
                                ACQUIRE_WATCHDOG
                            );
                            output.reacquire = true;
                            direct.scan_dirty = true;
                        }
                        continue;
                    }
                    Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                        direct.outputs[index].needs_recreate = true;
                        direct.outputs[index].presented_ok = false;
                        continue;
                    }
                    Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                        self.direct_retire_output(
                            direct,
                            index,
                            "lost: display surface lost".into(),
                        );
                        continue;
                    }
                    Err(err) => {
                        self.direct_retire_output(
                            direct,
                            index,
                            format!("lost: acquire_next_image {err:?}"),
                        );
                        continue;
                    }
                }
            }
            let (acquire_fence, submit_fence, acquired_at) = {
                let output = &direct.outputs[index];
                (
                    output.acquire_fence,
                    output.submit_fence,
                    output.acquired_at,
                )
            };
            if !self.fence_signaled(acquire_fence)? {
                // Watchdog: a release that never comes means the display is
                // gone or wedged. Park the fence and go through a fresh
                // swapchain/acquire, which reports SURFACE_LOST properly.
                if acquired_at.is_some_and(|since| now.duration_since(since) > ACQUIRE_WATCHDOG) {
                    crate::warning!(
                        "Vulkan direct: {} acquire pending for over {:?}; recreating its swapchain",
                        direct.outputs[index].connector.name,
                        ACQUIRE_WATCHDOG
                    );
                    let parked = {
                        let DirectState {
                            retired_fences,
                            outputs,
                            ..
                        } = &mut *direct;
                        self.direct_retire_pending_acquire(retired_fences, &mut outputs[index])
                    };
                    match parked {
                        Ok(()) => {
                            direct.outputs[index].needs_recreate = true;
                            direct.outputs[index].presented_ok = false;
                        }
                        Err(err) => {
                            self.direct_retire_output(direct, index, format!("lost: {err}"))
                        }
                    }
                }
                continue;
            }
            if !direct.composition_valid
                || direct.outputs[index].presented_generation == direct.composition_generation
            {
                continue;
            }
            if !self.fence_signaled(submit_fence)? {
                continue;
            }
            match self.direct_present_output(direct, index) {
                Ok(false) => {}
                Ok(true) => {
                    direct.outputs[index].needs_recreate = true;
                    direct.outputs[index].presented_ok = false;
                }
                Err(err) => self.direct_retire_output(direct, index, format!("lost: {err}")),
            }
        }
        Ok(())
    }

    /// Blit the composition into the output's acquired image and present it.
    /// `Ok(true)` asks for a swapchain recreation (out of date/suboptimal).
    fn direct_present_output(
        &mut self,
        direct: &mut DirectState,
        index: usize,
    ) -> Result<bool, String> {
        let present_wait = self.desktop.present_wait.is_some();
        let blit = direct.blit.as_ref().ok_or("blit resources missing")?;
        let composition = direct
            .composition
            .as_ref()
            .ok_or("composition image missing")?;
        let output = &mut direct.outputs[index];
        let image = output.acquired.take().ok_or("no acquired image")?;
        output.acquired_at = None;
        let pipeline = blit
            .pipelines
            .get(&output.format.as_raw())
            .ok_or("blit pipeline missing for the output format")?;
        let framebuffer = *output
            .framebuffers
            .get(image as usize)
            .ok_or_else(|| format!("invalid swapchain image index {image}"))?;
        let present_semaphore = output.present_semaphores[image as usize];
        unsafe {
            self.device
                .reset_fences(&[output.acquire_fence, output.submit_fence])
                .map_err(|e| format!("reset output fences: {e:?}"))?;
        }
        if output.descriptor_epoch != direct.composition_epoch {
            let image_info = [vk::DescriptorImageInfo::default()
                .image_view(composition.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler_info = [vk::DescriptorImageInfo::default().sampler(blit.sampler)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(output.descriptor_set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(output.descriptor_set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&sampler_info),
            ];
            unsafe { self.device.update_descriptor_sets(&writes, &[]) };
            output.descriptor_epoch = direct.composition_epoch;
        }
        let (viewport, scissor) = letterbox(direct.desktop_extent, output.extent);
        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 1.0],
            },
        }];
        unsafe {
            self.device
                .reset_command_buffer(output.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset output command buffer: {e:?}"))?;
            self.device
                .begin_command_buffer(
                    output.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin output command buffer: {e:?}"))?;
            self.device.cmd_begin_render_pass(
                output.command_buffer,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(pipeline.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: output.extent,
                    })
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            self.device
                .cmd_set_viewport(output.command_buffer, 0, &[viewport]);
            self.device
                .cmd_set_scissor(output.command_buffer, 0, &[scissor]);
            self.device.cmd_bind_pipeline(
                output.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.pipeline,
            );
            self.device.cmd_bind_descriptor_sets(
                output.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                blit.pipeline_layout,
                0,
                &[output.descriptor_set],
                &[],
            );
            self.device.cmd_draw(output.command_buffer, 3, 1, 0, 0);
            self.device.cmd_end_render_pass(output.command_buffer);
            self.device
                .end_command_buffer(output.command_buffer)
                .map_err(|e| format!("end output command buffer: {e:?}"))?;
        }
        // No semaphore waits: the acquire fence already proved the image free
        // and submission order covers the composition write.
        let command_buffers = [output.command_buffer];
        let signal_semaphores = [present_semaphore];
        let submit = vk::SubmitInfo::default()
            .command_buffers(&command_buffers)
            .signal_semaphores(&signal_semaphores);
        unsafe {
            self.device
                .queue_submit(self.queue, &[submit], output.submit_fence)
                .map_err(|e| format!("submit output blit: {e:?}"))?;
        }
        output.presented_generation = direct.composition_generation;
        let swapchains = [output.swapchain];
        let image_indices = [image];
        let present_ids = [output.next_present_id];
        let mut present_id = vk::PresentIdKHR::default().present_ids(&present_ids);
        let mut present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        if present_wait {
            present_info = present_info.push_next(&mut present_id);
        }
        // vkQueuePresentKHR may block the host on some drivers; a fence-only
        // acquire limits, but does not rule out, that wait.
        let presented = unsafe {
            self.swapchain_loader
                .queue_present(self.queue, &present_info)
        };
        if present_wait && presented.is_ok() {
            // Ids are per swapchain and monotonic; recreation restarts them.
            output
                .outstanding_presents
                .push((output.next_present_id, Instant::now()));
            output.next_present_id += 1;
        }
        match presented {
            Ok(false) => {
                if !output.presented_ok {
                    output.presented_ok = true;
                    output.status = "active".into();
                }
                Ok(false)
            }
            Ok(true) => Ok(true),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(true),
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                Err("display surface lost during present".into())
            }
            Err(err) => Err(format!("queue_present failed: {err:?}")),
        }
    }

    /// Main pass entry for direct mode: render the composition when the
    /// source can take it, then present it to every ready output.
    pub(super) fn direct_draw_pass_and_present(
        &mut self,
        cx: &mut Cx,
        draw_pass_id: DrawPassId,
        before_present: impl FnOnce(),
    ) -> Result<bool, String> {
        if let Some(mut routed) = self.desktop.routed.take() {
            let result = self.routed_frame(cx, &mut routed, draw_pass_id, before_present);
            self.desktop.routed = Some(routed);
            return result;
        }
        let Some(mut direct) = self.desktop.direct.take() else {
            return Ok(false);
        };
        let result = self.direct_frame(cx, &mut direct, draw_pass_id, before_present);
        self.desktop.direct = Some(direct);
        result
    }

    fn direct_frame(
        &mut self,
        cx: &mut Cx,
        direct: &mut DirectState,
        draw_pass_id: DrawPassId,
        before_present: impl FnOnce(),
    ) -> Result<bool, String> {
        if self.in_flight_fence == vk::Fence::null() {
            return Err("Vulkan frame synchronization is unavailable".into());
        }
        // Clones that were busy last time take the retained frame first.
        self.direct_service_outputs(direct)?;
        let capture_window_id = cx.get_pass_window_id(draw_pass_id).map(|w| w.id());
        // Only a deliberate capture renders without a display; otherwise the
        // dirty pass is kept for the reconnect and the loop waits normally.
        let explicit_capture = !cx.screenshot_requests.is_empty()
            || crate::screen_capture::capture_wants_window(capture_window_id);
        match direct.source_index() {
            Some(index) => {
                if !self.direct_output_can_take_frame(&direct.outputs[index])? {
                    direct.wait = DirectWait::GpuBusy;
                    return Ok(false);
                }
            }
            None if !explicit_capture => {
                direct.wait = DirectWait::NoOutput;
                return Ok(false);
            }
            None => {}
        }
        let composition = match direct.composition.as_ref() {
            Some(composition) => composition.image,
            None => {
                direct.wait = DirectWait::NoOutput;
                return Ok(false);
            }
        };
        if !self.draw_direct_composition(cx, draw_pass_id, composition)? {
            direct.wait = if self.fence_signaled(self.in_flight_fence)? {
                DirectWait::Ready
            } else {
                DirectWait::GpuBusy
            };
            return Ok(false);
        }
        direct.composition_generation += 1;
        direct.composition_valid = true;
        direct.wait = DirectWait::Ready;
        before_present();
        self.direct_service_outputs(direct)?;
        Ok(true)
    }

    /// Rendering has no dependency on the device owning the outputs. Both
    /// direct and routed composition use the same shaders and capture path.
    fn draw_direct_composition(
        &mut self,
        cx: &mut Cx,
        draw_pass_id: DrawPassId,
        composition: vk::Image,
    ) -> Result<bool, String> {
        let capture_window_id = cx.get_pass_window_id(draw_pass_id).map(|w| w.id());
        let draw_list_id = if let Some(id) = cx.passes[draw_pass_id].main_draw_list_id {
            id
        } else {
            self.profile.note_composition_skipped();
            return Ok(false);
        };
        let dpi_factor = cx.passes[draw_pass_id].dpi_factor.unwrap_or(1.0);
        let pass_rect = match cx.get_pass_rect(draw_pass_id, dpi_factor) {
            Some(rect) => rect,
            None => {
                self.profile.note_composition_skipped();
                return Ok(false);
            }
        };
        if pass_rect.size.x < 0.5 || pass_rect.size.y < 0.5 {
            self.profile.note_composition_skipped();
            return Ok(false);
        }
        let ortho_uniforms_gen = cx.next_uniform_gen();
        let dpi_uniforms_gen = cx.next_uniform_gen();
        {
            let pass = &mut cx.passes[draw_pass_id];
            if !pass.keep_camera_matrix {
                pass.set_ortho_matrix(pass_rect.pos, pass_rect.size, ortho_uniforms_gen);
            }
            pass.set_dpi_factor(dpi_factor, dpi_uniforms_gen);
        }
        let clear_color = if cx.passes[draw_pass_id].color_textures.is_empty() {
            cx.passes[draw_pass_id].clear_color
        } else {
            match cx.passes[draw_pass_id].color_textures[0].clear_color {
                DrawPassClearColor::InitWith(color) => color,
                DrawPassClearColor::ClearWith(color) => color,
            }
        };
        if !self.fence_signaled(self.in_flight_fence)? {
            self.profile.note_composition_busy();
            return Ok(false);
        }
        self.profile.collect_pending(&self.device);
        let mut profile_sample = self.profile.begin_sample(
            cx,
            draw_pass_id,
            true,
            self.swapchain_extent.width,
            self.swapchain_extent.height,
            0.0,
        );
        let profile_encode_start = profile_sample.is_some().then(Instant::now);
        self.recycle_completed_frame_resources()?;

        let window_id = cx
            .get_pass_window_id(draw_pass_id)
            .map(|window| window.id())
            .unwrap_or(0);
        let screenshot_request_ids =
            cx.take_studio_screenshot_request_ids_for_window(0, Some(window_id));
        let run_view_request = cx.take_studio_run_view_frame_request(window_id);
        let wants_capture = crate::screen_capture::capture_wants_window(capture_window_id);
        let capture =
            !screenshot_request_ids.is_empty() || run_view_request.is_some() || wants_capture;
        if let Some(sample) = profile_sample.as_mut() {
            sample.mark_capture(capture);
        }
        if capture && self.swapchain_readback_buffer.is_none() {
            return Err(
                "frame capture requested but the composition readback buffer is unavailable".into(),
            );
        }

        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset_command_buffer failed: {e:?}"))?;
            self.device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin_command_buffer failed: {e:?}"))?;
        }
        if let Some(sample) = profile_sample.as_mut() {
            sample.wrote_timestamps = self.profile.begin_timestamps(
                &self.device,
                &self.instance,
                self.physical_device,
                self.queue_family_index,
                self.command_buffer,
            );
        }
        self.texture_upload_count_this_frame = 0;
        self.texture_upload_bytes_this_frame = 0;
        self.prune_stale_geometry_resources(cx);
        self.prepare_draw_list_textures(cx, draw_list_id)?;

        let mut zbias = 0.0f32;
        let zbias_step = cx.passes[draw_pass_id].zbias_step;
        let mut draw_stats = VulkanDrawStats::default();
        let clear_depth = match cx.passes[draw_pass_id].clear_depth {
            DrawPassClearDepth::InitWith(depth) | DrawPassClearDepth::ClearWith(depth) => depth,
        };
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [clear_color.x, clear_color.y, clear_color.z, clear_color.w],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: clear_depth,
                    stencil: 0,
                },
            },
        ];
        let framebuffer = *self
            .framebuffers
            .first()
            .ok_or("composition framebuffer missing")?;
        let extent = self.swapchain_extent;
        unsafe {
            self.device.cmd_begin_render_pass(
                self.command_buffer,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent,
                    })
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_set_viewport(
                self.command_buffer,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: extent.height as f32,
                    width: extent.width as f32,
                    height: -(extent.height as f32),
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            self.device.cmd_set_scissor(
                self.command_buffer,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent,
                }],
            );
        }
        let xr_depth_view = self.ensure_xr_depth_dummy()?;
        let render_pass_key = self.main_render_pass_key();
        self.record_draw_list(
            cx,
            draw_pass_id,
            draw_list_id,
            &render_pass_key,
            &mut zbias,
            zbias_step,
            &mut draw_stats,
            xr_depth_view,
        )?;
        unsafe {
            self.device.cmd_end_render_pass(self.command_buffer);
        }

        if capture {
            self.direct_record_capture_copy(composition, extent)?;
        }

        if profile_sample
            .as_ref()
            .is_some_and(|sample| sample.wrote_timestamps)
        {
            self.profile.end_timestamps(&self.device, self.command_buffer);
        }
        unsafe {
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|e| format!("end_command_buffer failed: {e:?}"))?;
        }
        let profile_encode_ms = profile_encode_start
            .map(|start| start.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        let command_buffers = [self.command_buffer];
        let profile_submit_start = profile_sample.is_some().then(Instant::now);
        self.submit_frame(&vk::SubmitInfo::default().command_buffers(&command_buffers))?;
        if let Some(mut sample) = profile_sample.take() {
            sample.encode_ms = profile_encode_ms;
            sample.submit_ms = profile_submit_start
                .map(|start| start.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            self.profile.admit_pending(sample);
        }
        if capture {
            let capture_result = (|| -> Result<(), String> {
                let profile_post_start = self.profile.enabled().then(Instant::now);
                unsafe {
                    self.device
                        .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                        .map_err(|e| {
                            format!("wait_for_fences(composition capture) failed: {e:?}")
                        })?;
                }
                self.profile.complete_after_fence(
                    &self.device,
                    profile_post_start
                        .map(|start| start.elapsed().as_secs_f64() * 1000.0)
                        .unwrap_or(0.0),
                );
                let width = extent.width.max(1);
                let height = extent.height.max(1);
                let rgba = self.read_readback_buffer_rgba()?;
                crate::screen_capture::deliver_capture_frame(
                    capture_window_id,
                    width,
                    height,
                    &rgba,
                );
                if !screenshot_request_ids.is_empty() {
                    let png = Cx::encode_rgba_as_png(width, height, &rgba)?;
                    Cx::send_studio_screenshot_response(screenshot_request_ids, width, height, png);
                }
                if let Some(request) = run_view_request {
                    cx.encode_studio_run_view_frame_async(request, width, height, rgba);
                }
                Ok(())
            })();
            if let Err(err) = capture_result {
                crate::warning!("Vulkan frame capture failed: {err}");
            }
        }

        crate::trace!(
            "gpu.present",
            "present time={:.6}",
            crate::cx_api::CxOsApi::seconds_since_app_start(cx)
        );
        cx.passes[draw_pass_id].paint_dirty = false;
        Ok(true)
    }

    /// Explicit capture only: copy the composition into the host-visible
    /// readback buffer inside the frame command buffer.
    fn direct_record_capture_copy(
        &mut self,
        image: vk::Image,
        extent: vk::Extent2D,
    ) -> Result<(), String> {
        let staging = self
            .swapchain_readback_buffer
            .ok_or_else(|| "composition readback buffer unavailable".to_string())?;
        let byte_len = extent.width as vk::DeviceSize * extent.height as vk::DeviceSize * 4;
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(1)
            .base_array_layer(0)
            .layer_count(1);
        let to_transfer = vk::ImageMemoryBarrier::default()
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(image)
            .subresource_range(range);
        let copy_region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            });
        let to_sampled = vk::ImageMemoryBarrier::default()
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(range);
        let buffer_ready = vk::BufferMemoryBarrier::default()
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .buffer(staging.buffer)
            .offset(0)
            .size(byte_len);
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            self.device.cmd_copy_image_to_buffer(
                self.command_buffer,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                staging.buffer,
                &[copy_region],
            );
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &[buffer_ready],
                &[to_sampled],
            );
        }
        Ok(())
    }

    /// Everything device-level the direct state owns; called once from
    /// `CxVulkan::drop` after the device went idle and the main targets were
    /// destroyed, before the device itself is destroyed.
    ///
    /// Returns whether the caller may destroy the device and instance. A
    /// presentation engine that never releases an acquired image leaves a
    /// fence pending forever; a device with such a live child must not be
    /// destroyed (VUID-vkDestroyDevice-device-05137) and waiting forever is
    /// not an option either, so in that case the whole direct state and a
    /// reference to the loaded Vulkan library are deliberately abandoned to
    /// process exit and `false` is returned.
    pub(super) fn destroy_direct_device_resources(&mut self) -> bool {
        if let Some(mut routed) = self.desktop.routed.take() {
            routed.display.device_wait_idle();
            if let Some(bridge) = routed.bridge.take() {
                unsafe { bridge.destroy(self, &routed.display) };
            }
            if let Some(composition) = routed.composition.take() {
                self.destroy_texture_resource(composition);
            }
            // Presenter Drop releases its WSI resources before either parent.
            drop(routed);
        }
        let Some(mut direct) = self.desktop.direct.take() else {
            return true;
        };
        direct.hotplug.take();
        {
            let DirectState {
                retired_fences,
                outputs,
                ..
            } = &mut direct;
            for output in outputs.iter_mut() {
                self.direct_destroy_output_device_resources(retired_fences, output);
            }
        }
        if !direct.retired_fences.is_empty() {
            let fences: Vec<vk::Fence> = direct
                .retired_fences
                .iter()
                .map(|retired| retired.fence)
                .collect();
            let _ = unsafe {
                self.device
                    .wait_for_fences(&fences, true, RETIRED_FENCE_TEARDOWN_NS)
            };
            let mut index = 0;
            while index < direct.retired_fences.len() {
                let signaled = unsafe {
                    self.device
                        .get_fence_status(direct.retired_fences[index].fence)
                }
                .unwrap_or(false);
                if signaled {
                    let retired = direct.retired_fences.swap_remove(index);
                    unsafe { self.device.destroy_fence(retired.fence, None) };
                } else {
                    index += 1;
                }
            }
            if !direct.retired_fences.is_empty() {
                let oldest = direct
                    .retired_fences
                    .iter()
                    .map(|retired| retired.since.elapsed())
                    .max()
                    .unwrap_or_default();
                crate::error!(
                    "Vulkan direct: {} acquire fence(s) never signaled (oldest pending {:?}): the presentation engine \
                     never released the image. Abandoning the Vulkan device, instance and display state to process \
                     exit instead of destroying a device with live children.",
                    direct.retired_fences.len(),
                    oldest
                );
                // Keep libvulkan mapped for the abandoned handles and skip
                // every instance-level release the state's drop would do.
                std::mem::forget(self._entry.clone());
                std::mem::forget(direct);
                return false;
            }
        }
        if let Some(resource) = direct.composition.take() {
            self.destroy_texture_resource(resource);
        }
        if let Some(blit) = direct.blit.take() {
            unsafe {
                for (_, pipeline) in blit.pipelines {
                    self.device.destroy_pipeline(pipeline.pipeline, None);
                    self.device.destroy_render_pass(pipeline.render_pass, None);
                }
                self.device
                    .destroy_pipeline_layout(blit.pipeline_layout, None);
                self.device
                    .destroy_descriptor_set_layout(blit.descriptor_set_layout, None);
                self.device.destroy_sampler(blit.sampler, None);
            }
        }
        // Surfaces and displays are instance-level; DirectState::drop releases
        // them before the instance is destroyed.
        self.desktop.direct = Some(direct);
        true
    }

    /// The UI's view of the outputs. Only observed state: `active` means a
    /// present on the current swapchain succeeded (`ready` before that);
    /// `primary` marks the applied render source.
    pub(crate) fn direct_display_snapshot(&self) -> LinuxDisplaySnapshot {
        if let Some(routed) = &self.desktop.routed {
            return routed.display.direct_display_snapshot();
        }
        let Some(direct) = self.desktop.direct.as_ref() else {
            return LinuxDisplaySnapshot::default();
        };
        let mut outputs: Vec<LinuxDisplayOutput> = direct
            .outputs
            .iter()
            .map(|output| LinuxDisplayOutput {
                name: output.connector.name.clone(),
                width: output.mode_extent.width,
                height: output.mode_extent.height,
                refresh_hz: output.refresh_hz(),
                primary: output.primary,
                active: output.active && output.presented_ok,
                status: output.status.clone(),
            })
            .chain(
                direct
                    .unsupported
                    .iter()
                    .map(|(connector, status)| LinuxDisplayOutput {
                        name: connector.name.clone(),
                        width: 0,
                        height: 0,
                        refresh_hz: 0.0,
                        primary: false,
                        active: false,
                        status: status.clone(),
                    }),
            )
            .collect();
        outputs.sort_by_key(|output| connector_rank(&output.name));
        LinuxDisplaySnapshot {
            direct: true,
            outputs,
        }
    }
}

#[cfg(linux_direct)]
fn connector_card_name(card: &Path) -> String {
    card.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| card.display().to_string())
}

#![cfg(target_os = "android")]

use crate::{
    cx::Cx,
    draw_list::DrawListId,
    draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
    draw_shader::DrawShaderAttrFormat,
    geometry::GeometryId,
    makepad_live_id::*,
    makepad_script::shader::TextureType,
    os::linux::{
        android::ndk_sys,
        openxr_sys::{
            LibOpenXr, VkDeviceCreateInfo, VkInstanceCreateInfo, XrInstance, XrResult, XrSystemId,
            XrVulkanDeviceCreateInfoKHR, XrVulkanGraphicsDeviceGetInfoKHR,
            XrVulkanInstanceCreateInfoKHR,
        },
    },
    texture::{TextureFormat, TextureId, TexturePixel, TextureUpdated},
};
use ash::vk::{self, Handle};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};

#[link(name = "nativewindow")]
extern "C" {
    fn ANativeWindow_acquire(window: *mut ndk_sys::ANativeWindow);
}

unsafe extern "system" fn vulkan_debug_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_types: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _p_user_data: *mut c_void,
) -> vk::Bool32 {
    let msg = if p_callback_data.is_null() {
        "<null debug callback data>".into()
    } else {
        CStr::from_ptr((*p_callback_data).p_message)
            .to_string_lossy()
            .into_owned()
    };
    if message_severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        crate::error!("Vulkan validation [{message_types:?}] {msg}");
    } else if message_severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        crate::warning!("Vulkan validation [{message_types:?}] {msg}");
    } else {
        crate::log!("Vulkan validation [{message_types:?}] {msg}");
    }
    vk::FALSE
}

fn vulkan_debug_messenger_create_info() -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::INFO,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(vulkan_debug_callback))
}

#[derive(Clone, Copy)]
struct VulkanBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: vk::DeviceSize,
}

#[derive(Clone, Copy)]
struct VulkanGeometryResource {
    vertex_buffer: VulkanBuffer,
    index_buffer: VulkanBuffer,
}

#[derive(Default)]
struct FrameResources {
    buffers: Vec<VulkanBuffer>,
    descriptor_pools: Vec<vk::DescriptorPool>,
}

struct VulkanPipeline {
    pipeline_write: vk::Pipeline,
    pipeline_no_write: vk::Pipeline,
    layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    has_descriptors: bool,
    sampler_handles: Vec<vk::Sampler>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct VulkanRenderPassKey {
    color_formats: Vec<i32>,
    depth_format: Option<i32>,
}

impl VulkanRenderPassKey {
    fn new(color_formats: &[vk::Format], depth_format: Option<vk::Format>) -> Self {
        Self {
            color_formats: color_formats.iter().map(|format| format.as_raw()).collect(),
            depth_format: depth_format.map(|format| format.as_raw()),
        }
    }

    fn color_vk_formats(&self) -> Vec<vk::Format> {
        self.color_formats
            .iter()
            .map(|format| vk::Format::from_raw(*format))
            .collect()
    }

    fn depth_vk_format(&self) -> Option<vk::Format> {
        self.depth_format.map(vk::Format::from_raw)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct VulkanPipelineKey {
    shader_index: usize,
    render_pass: VulkanRenderPassKey,
    backface_culling: bool,
}

struct VulkanDrawPacket {
    shader_index: usize,
    geometry_id: GeometryId,
    depth_write: bool,
    backface_culling: bool,
    instances: Vec<f32>,
    draw_call_uniforms: Vec<f32>,
    dyn_uniforms: Vec<f32>,
    scope_uniforms: Vec<f32>,
    uniform_bindings: Vec<(LiveId, usize)>,
    dyn_uniform_binding: u32,
    scope_uniform_binding: Option<usize>,
    texture_ids: Vec<TextureId>,
    texture_types: Vec<TextureType>,
}

struct VulkanTextureResource {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    width: u32,
    height: u32,
    layers: u32,
    is_cube: bool,
    format: vk::Format,
    layout: vk::ImageLayout,
    hardware_buffer: Option<*mut ndk_sys::AHardwareBuffer>,
    sampler: Option<vk::Sampler>,
    ycbcr_conversion: Option<vk::SamplerYcbcrConversion>,
    owns_image: bool,
}

#[derive(Clone, Copy)]
struct ImportedYuvPlaneLayout {
    biplanar: bool,
    plane0_view_format: vk::Format,
    plane1_view_format: vk::Format,
    plane2_view_format: Option<vk::Format>,
}

struct VulkanTextureUpload {
    data: Vec<u8>,
    offset_x: u32,
    offset_y: u32,
    width: u32,
    height: u32,
    layers: u32,
}

type VulkanTextureKey = usize;

pub(crate) struct CxVulkanOpenXrEyeTarget {
    framebuffer: vk::Framebuffer,
    color_view: vk::ImageView,
    depth_target: VulkanTextureResource,
}

pub(crate) struct CxVulkanOpenXrSwapchainImage {
    image: vk::Image,
    eyes: [CxVulkanOpenXrEyeTarget; 2],
}

pub(crate) struct CxVulkanOpenXrDepthImage {
    image: vk::Image,
    views: [vk::ImageView; 2],
}

pub(crate) struct CxVulkanOpenXrSessionData {
    width: u32,
    height: u32,
    color_format: vk::Format,
    pub(crate) depth_width: u32,
    pub(crate) depth_height: u32,
    color_images: Vec<CxVulkanOpenXrSwapchainImage>,
    depth_images: Vec<CxVulkanOpenXrDepthImage>,
    color_readback_buffer: Option<VulkanBuffer>,
    depth_readback_buffer: Option<VulkanBuffer>,
}

#[derive(Default)]
struct VulkanDrawStats {
    draw_items: usize,
    draw_calls: usize,
    packets_recorded: usize,
    skipped_non_draw_call: usize,
    skipped_no_os_shader: usize,
    skipped_no_vulkan_shader: usize,
    skipped_missing_spirv: usize,
    skipped_no_instance_slots: usize,
    skipped_no_instances_buffer: usize,
    skipped_instances_too_short: usize,
    skipped_zero_instances: usize,
    skipped_no_geometry_id: usize,
    skipped_empty_geometry: usize,
}

pub struct CxVulkan {
    instance: ash::Instance,
    surface_loader: ash::khr::surface::Instance,
    android_surface_loader: ash::khr::android_surface::Instance,
    surface: vk::SurfaceKHR,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    min_uniform_buffer_offset_alignment: vk::DeviceSize,
    device: ash::Device,
    external_memory_android_hardware_buffer:
        ash::android::external_memory_android_hardware_buffer::Device,
    queue: vk::Queue,
    swapchain_loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    swapchain_images: Vec<vk::Image>,
    swapchain_image_views: Vec<vk::ImageView>,
    swapchain_depth_targets: Vec<VulkanTextureResource>,
    swapchain_readback_buffer: Option<VulkanBuffer>,
    swapchain_format: vk::Format,
    depth_format: vk::Format,
    swapchain_extent: vk::Extent2D,
    render_pass: vk::RenderPass,
    xr_render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    pipelines: HashMap<VulkanPipelineKey, VulkanPipeline>,
    offscreen_render_passes: HashMap<VulkanRenderPassKey, vk::RenderPass>,
    geometries: HashMap<GeometryId, VulkanGeometryResource>,
    textures: HashMap<VulkanTextureKey, VulkanTextureResource>,
    frame_resources: FrameResources,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available_semaphore: vk::Semaphore,
    render_finished_semaphore: vk::Semaphore,
    in_flight_fence: vk::Fence,
    window: *mut ndk_sys::ANativeWindow,
    requested_width: u32,
    requested_height: u32,
    texture_upload_count_this_frame: u32,
    texture_upload_bytes_this_frame: u64,
    debug_utils_enabled: bool,
    debug_utils_loader: Option<ash::ext::debug_utils::Instance>,
    debug_messenger: vk::DebugUtilsMessengerEXT,
    xr_depth_dummy: Option<VulkanTextureResource>,
}

impl CxVulkan {
    pub fn new(
        window: *mut ndk_sys::ANativeWindow,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        if window.is_null() {
            return Err("Android Vulkan init failed: null ANativeWindow".to_string());
        }

        let entry = unsafe { ash::Entry::load() }
            .map_err(|e| format!("Android Vulkan init failed: Entry::load: {e:?}"))?;

        let available_layers = unsafe { entry.enumerate_instance_layer_properties() }
            .map_err(|e| format!("Android Vulkan init failed: enumerate layers: {e:?}"))?;
        let has_validation_layer = available_layers.iter().any(|layer| {
            let name = unsafe { CStr::from_ptr(layer.layer_name.as_ptr()) };
            name.to_bytes() == b"VK_LAYER_KHRONOS_validation"
        });

        let available_exts = unsafe { entry.enumerate_instance_extension_properties(None) }
            .map_err(|e| format!("Android Vulkan init failed: enumerate extensions: {e:?}"))?;
        let has_debug_utils_ext = available_exts.iter().any(|ext| {
            let name = unsafe { CStr::from_ptr(ext.extension_name.as_ptr()) };
            name.to_bytes() == vk::EXT_DEBUG_UTILS_NAME.to_bytes()
        });

        let mut instance_extensions = vec![
            vk::KHR_SURFACE_NAME.as_ptr(),
            vk::KHR_ANDROID_SURFACE_NAME.as_ptr(),
        ];
        if has_debug_utils_ext {
            instance_extensions.push(vk::EXT_DEBUG_UTILS_NAME.as_ptr());
        }
        let validation_layer_name = b"VK_LAYER_KHRONOS_validation\0";
        let enabled_layers: Vec<*const c_char> = if has_validation_layer {
            vec![validation_layer_name.as_ptr() as *const c_char]
        } else {
            Vec::new()
        };

        let app_info = vk::ApplicationInfo {
            api_version: vk::API_VERSION_1_1,
            ..Default::default()
        };
        let mut instance_create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&instance_extensions)
            .enabled_layer_names(&enabled_layers);
        let mut debug_create_info = vulkan_debug_messenger_create_info();
        if has_debug_utils_ext {
            instance_create_info = instance_create_info.push_next(&mut debug_create_info);
        }

        let instance = unsafe { entry.create_instance(&instance_create_info, None) }
            .map_err(|e| format!("Android Vulkan init failed: create_instance: {e:?}"))?;

        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
        let android_surface_loader = ash::khr::android_surface::Instance::new(&entry, &instance);

        unsafe { ANativeWindow_acquire(window) };

        let create_surface_result = Self::create_surface(&android_surface_loader, window);
        let surface = match create_surface_result {
            Ok(surface) => surface,
            Err(err) => {
                unsafe { ndk_sys::ANativeWindow_release(window) };
                unsafe { instance.destroy_instance(None) };
                return Err(err);
            }
        };

        let pick_result = Self::pick_device_and_queue_family(&instance, &surface_loader, surface);
        let (physical_device, queue_family_index) = match pick_result {
            Ok(pick) => pick,
            Err(err) => {
                unsafe {
                    surface_loader.destroy_surface(surface, None);
                    ndk_sys::ANativeWindow_release(window);
                    instance.destroy_instance(None);
                }
                return Err(err);
            }
        };

        let props = unsafe { instance.get_physical_device_properties(physical_device) };
        let device_name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        if device_name.contains("SwiftShader") || props.vendor_id == 0x1AE0 {
            crate::warning!(
                "Android Vulkan: SwiftShader/software device detected; expect very low performance"
            );
        }

        let queue_priorities = [1.0f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priorities)];
        let device_extensions = [
            vk::KHR_SWAPCHAIN_NAME.as_ptr(),
            vk::ANDROID_EXTERNAL_MEMORY_ANDROID_HARDWARE_BUFFER_NAME.as_ptr(),
        ];
        let mut sampler_ycbcr_features =
            vk::PhysicalDeviceSamplerYcbcrConversionFeatures::default()
                .sampler_ycbcr_conversion(true);
        let device_create_info = vk::DeviceCreateInfo::default()
            .push_next(&mut sampler_ycbcr_features)
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&device_extensions);

        let device =
            match unsafe { instance.create_device(physical_device, &device_create_info, None) } {
                Ok(device) => device,
                Err(err) => {
                    unsafe {
                        surface_loader.destroy_surface(surface, None);
                        ndk_sys::ANativeWindow_release(window);
                        instance.destroy_instance(None);
                    }
                    return Err(format!(
                        "Android Vulkan init failed: create_device: {err:?}"
                    ));
                }
            };

        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let external_memory_android_hardware_buffer =
            ash::android::external_memory_android_hardware_buffer::Device::new(&instance, &device);
        let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);

        let command_pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = match unsafe { device.create_command_pool(&command_pool_info, None) } {
            Ok(pool) => pool,
            Err(err) => {
                unsafe {
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    ndk_sys::ANativeWindow_release(window);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "Android Vulkan init failed: create_command_pool: {err:?}"
                ));
            }
        };

        let command_buffer_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer = match unsafe { device.allocate_command_buffers(&command_buffer_info) }
        {
            Ok(cmds) => cmds[0],
            Err(err) => {
                unsafe {
                    device.destroy_command_pool(command_pool, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    ndk_sys::ANativeWindow_release(window);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "Android Vulkan init failed: allocate_command_buffers: {err:?}"
                ));
            }
        };

        let semaphore_info = vk::SemaphoreCreateInfo::default();
        let image_available_semaphore =
            match unsafe { device.create_semaphore(&semaphore_info, None) } {
                Ok(semaphore) => semaphore,
                Err(err) => {
                    unsafe {
                        device.free_command_buffers(command_pool, &[command_buffer]);
                        device.destroy_command_pool(command_pool, None);
                        device.destroy_device(None);
                        surface_loader.destroy_surface(surface, None);
                        ndk_sys::ANativeWindow_release(window);
                        instance.destroy_instance(None);
                    }
                    return Err(format!(
                        "Android Vulkan init failed: create image semaphore: {err:?}"
                    ));
                }
            };

        let render_finished_semaphore =
            match unsafe { device.create_semaphore(&semaphore_info, None) } {
                Ok(semaphore) => semaphore,
                Err(err) => {
                    unsafe {
                        device.destroy_semaphore(image_available_semaphore, None);
                        device.free_command_buffers(command_pool, &[command_buffer]);
                        device.destroy_command_pool(command_pool, None);
                        device.destroy_device(None);
                        surface_loader.destroy_surface(surface, None);
                        ndk_sys::ANativeWindow_release(window);
                        instance.destroy_instance(None);
                    }
                    return Err(format!(
                        "Android Vulkan init failed: create render semaphore: {err:?}"
                    ));
                }
            };

        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let in_flight_fence = match unsafe { device.create_fence(&fence_info, None) } {
            Ok(fence) => fence,
            Err(err) => {
                unsafe {
                    device.destroy_semaphore(render_finished_semaphore, None);
                    device.destroy_semaphore(image_available_semaphore, None);
                    device.free_command_buffers(command_pool, &[command_buffer]);
                    device.destroy_command_pool(command_pool, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    ndk_sys::ANativeWindow_release(window);
                    instance.destroy_instance(None);
                }
                return Err(format!("Android Vulkan init failed: create_fence: {err:?}"));
            }
        };

        let mut vulkan = Self {
            instance,
            surface_loader,
            android_surface_loader,
            surface,
            physical_device,
            queue_family_index,
            min_uniform_buffer_offset_alignment: props
                .limits
                .min_uniform_buffer_offset_alignment
                .max(4),
            device,
            external_memory_android_hardware_buffer,
            queue,
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            swapchain_images: Vec::new(),
            swapchain_image_views: Vec::new(),
            swapchain_depth_targets: Vec::new(),
            swapchain_readback_buffer: None,
            swapchain_format: vk::Format::UNDEFINED,
            depth_format: vk::Format::UNDEFINED,
            swapchain_extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            render_pass: vk::RenderPass::null(),
            xr_render_pass: vk::RenderPass::null(),
            framebuffers: Vec::new(),
            pipelines: HashMap::new(),
            offscreen_render_passes: HashMap::new(),
            geometries: HashMap::new(),
            textures: HashMap::new(),
            frame_resources: FrameResources::default(),
            command_pool,
            command_buffer,
            image_available_semaphore,
            render_finished_semaphore,
            in_flight_fence,
            window,
            requested_width: width.max(1),
            requested_height: height.max(1),
            texture_upload_count_this_frame: 0,
            texture_upload_bytes_this_frame: 0,
            debug_utils_enabled: has_debug_utils_ext,
            debug_utils_loader: None,
            debug_messenger: vk::DebugUtilsMessengerEXT::null(),
            xr_depth_dummy: None,
        };

        if let Err(err) = vulkan.recreate_swapchain() {
            return Err(format!(
                "Android Vulkan init failed: recreate_swapchain: {err}"
            ));
        }

        vulkan.try_enable_debug_messenger(&entry);

        Ok(vulkan)
    }

    pub fn new_from_openxr(
        xr: &LibOpenXr,
        xr_instance: XrInstance,
        xr_system_id: XrSystemId,
        window: *mut ndk_sys::ANativeWindow,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        if window.is_null() {
            return Err("Android Vulkan XR init failed: null ANativeWindow".to_string());
        }

        let entry = unsafe { ash::Entry::load() }
            .map_err(|e| format!("Android Vulkan XR init failed: Entry::load: {e:?}"))?;

        let available_layers = unsafe { entry.enumerate_instance_layer_properties() }
            .map_err(|e| format!("Android Vulkan XR init failed: enumerate layers: {e:?}"))?;
        let has_validation_layer = available_layers.iter().any(|layer| {
            let name = unsafe { CStr::from_ptr(layer.layer_name.as_ptr()) };
            name.to_bytes() == b"VK_LAYER_KHRONOS_validation"
        });

        let available_exts = unsafe { entry.enumerate_instance_extension_properties(None) }
            .map_err(|e| format!("Android Vulkan XR init failed: enumerate extensions: {e:?}"))?;
        let has_debug_utils_ext = available_exts.iter().any(|ext| {
            let name = unsafe { CStr::from_ptr(ext.extension_name.as_ptr()) };
            name.to_bytes() == vk::EXT_DEBUG_UTILS_NAME.to_bytes()
        });

        let mut instance_extensions = vec![
            vk::KHR_SURFACE_NAME.as_ptr(),
            vk::KHR_ANDROID_SURFACE_NAME.as_ptr(),
        ];
        if has_debug_utils_ext {
            instance_extensions.push(vk::EXT_DEBUG_UTILS_NAME.as_ptr());
        }
        let validation_layer_name = b"VK_LAYER_KHRONOS_validation\0";
        let enabled_layers: Vec<*const c_char> = if has_validation_layer {
            vec![validation_layer_name.as_ptr() as *const c_char]
        } else {
            Vec::new()
        };

        let app_info = vk::ApplicationInfo {
            api_version: vk::API_VERSION_1_1,
            ..Default::default()
        };
        let mut instance_create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&instance_extensions)
            .enabled_layer_names(&enabled_layers);
        let mut debug_create_info = vulkan_debug_messenger_create_info();
        if has_debug_utils_ext {
            instance_create_info = instance_create_info.push_next(&mut debug_create_info);
        }

        let mut xr_vk_instance = std::ptr::null();
        let mut xr_vk_instance_result = 0;
        let xr_instance_create_info = XrVulkanInstanceCreateInfoKHR {
            system_id: xr_system_id,
            pfn_get_instance_proc_addr: Some(unsafe {
                std::mem::transmute(entry.static_fn().get_instance_proc_addr)
            }),
            vulkan_create_info: &instance_create_info as *const _ as *const VkInstanceCreateInfo,
            ..Default::default()
        };
        unsafe {
            (xr.xrCreateVulkanInstanceKHR)(
                xr_instance,
                &xr_instance_create_info,
                &mut xr_vk_instance,
                &mut xr_vk_instance_result,
            )
        }
        .to_result("xrCreateVulkanInstanceKHR")?;
        let xr_vk_instance_result = vk::Result::from_raw(xr_vk_instance_result);
        if xr_vk_instance_result != vk::Result::SUCCESS {
            return Err(format!(
                "Android Vulkan XR init failed: xrCreateVulkanInstanceKHR returned Vulkan error {xr_vk_instance_result:?}"
            ));
        }
        let instance = unsafe {
            ash::Instance::load(
                entry.static_fn(),
                vk::Instance::from_raw(xr_vk_instance as _),
            )
        };
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
        let android_surface_loader = ash::khr::android_surface::Instance::new(&entry, &instance);

        unsafe { ANativeWindow_acquire(window) };

        let create_surface_result = Self::create_surface(&android_surface_loader, window);
        let surface = match create_surface_result {
            Ok(surface) => surface,
            Err(err) => {
                unsafe { ndk_sys::ANativeWindow_release(window) };
                unsafe { instance.destroy_instance(None) };
                return Err(err);
            }
        };

        let get_info = XrVulkanGraphicsDeviceGetInfoKHR {
            system_id: xr_system_id,
            vulkan_instance: xr_vk_instance,
            ..Default::default()
        };
        let mut runtime_physical_device = std::ptr::null();
        let xr_get_device_result = unsafe {
            (xr.xrGetVulkanGraphicsDevice2KHR)(xr_instance, &get_info, &mut runtime_physical_device)
        };
        if xr_get_device_result != XrResult::SUCCESS {
            unsafe {
                surface_loader.destroy_surface(surface, None);
                ndk_sys::ANativeWindow_release(window);
                instance.destroy_instance(None);
            }
            return Err(format!(
                "OpenXR error in xrGetVulkanGraphicsDevice2KHR: {}",
                xr_get_device_result
            ));
        }
        let physical_device = vk::PhysicalDevice::from_raw(runtime_physical_device as _);

        let queue_family_index = match Self::pick_queue_family_for_device(
            &instance,
            &surface_loader,
            surface,
            physical_device,
        ) {
            Ok(index) => index,
            Err(err) => {
                unsafe {
                    surface_loader.destroy_surface(surface, None);
                    ndk_sys::ANativeWindow_release(window);
                    instance.destroy_instance(None);
                }
                return Err(err);
            }
        };

        let props = unsafe { instance.get_physical_device_properties(physical_device) };
        let device_name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        if device_name.contains("SwiftShader") || props.vendor_id == 0x1AE0 {
            crate::warning!(
                "Android Vulkan: SwiftShader/software device detected; expect very low performance"
            );
        }

        let queue_priorities = [1.0f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priorities)];
        let device_extensions = [
            vk::KHR_SWAPCHAIN_NAME.as_ptr(),
            vk::ANDROID_EXTERNAL_MEMORY_ANDROID_HARDWARE_BUFFER_NAME.as_ptr(),
        ];
        let mut sampler_ycbcr_features =
            vk::PhysicalDeviceSamplerYcbcrConversionFeatures::default()
                .sampler_ycbcr_conversion(true);
        let device_create_info = vk::DeviceCreateInfo::default()
            .push_next(&mut sampler_ycbcr_features)
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&device_extensions);

        let mut xr_vk_device = std::ptr::null();
        let mut xr_vk_device_result = 0;
        let xr_device_create_info = XrVulkanDeviceCreateInfoKHR {
            system_id: xr_system_id,
            pfn_get_instance_proc_addr: Some(unsafe {
                std::mem::transmute(entry.static_fn().get_instance_proc_addr)
            }),
            vulkan_physical_device: runtime_physical_device,
            vulkan_create_info: &device_create_info as *const _ as *const VkDeviceCreateInfo,
            ..Default::default()
        };
        unsafe {
            (xr.xrCreateVulkanDeviceKHR)(
                xr_instance,
                &xr_device_create_info,
                &mut xr_vk_device,
                &mut xr_vk_device_result,
            )
        }
        .to_result("xrCreateVulkanDeviceKHR")?;
        let xr_vk_device_result = vk::Result::from_raw(xr_vk_device_result);
        if xr_vk_device_result != vk::Result::SUCCESS {
            unsafe {
                surface_loader.destroy_surface(surface, None);
                ndk_sys::ANativeWindow_release(window);
                instance.destroy_instance(None);
            }
            return Err(format!(
                "Android Vulkan XR init failed: xrCreateVulkanDeviceKHR returned Vulkan error {xr_vk_device_result:?}"
            ));
        }
        let device = unsafe {
            ash::Device::load(instance.fp_v1_0(), vk::Device::from_raw(xr_vk_device as _))
        };
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let external_memory_android_hardware_buffer =
            ash::android::external_memory_android_hardware_buffer::Device::new(&instance, &device);
        let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);

        let command_pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = match unsafe { device.create_command_pool(&command_pool_info, None) } {
            Ok(pool) => pool,
            Err(err) => {
                unsafe {
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    ndk_sys::ANativeWindow_release(window);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "Android Vulkan XR init failed: create_command_pool: {err:?}"
                ));
            }
        };

        let command_buffer_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer = match unsafe { device.allocate_command_buffers(&command_buffer_info) }
        {
            Ok(cmds) => cmds[0],
            Err(err) => {
                unsafe {
                    device.destroy_command_pool(command_pool, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    ndk_sys::ANativeWindow_release(window);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "Android Vulkan XR init failed: allocate_command_buffers: {err:?}"
                ));
            }
        };

        let semaphore_info = vk::SemaphoreCreateInfo::default();
        let image_available_semaphore =
            match unsafe { device.create_semaphore(&semaphore_info, None) } {
                Ok(semaphore) => semaphore,
                Err(err) => {
                    unsafe {
                        device.free_command_buffers(command_pool, &[command_buffer]);
                        device.destroy_command_pool(command_pool, None);
                        device.destroy_device(None);
                        surface_loader.destroy_surface(surface, None);
                        ndk_sys::ANativeWindow_release(window);
                        instance.destroy_instance(None);
                    }
                    return Err(format!(
                        "Android Vulkan XR init failed: create image semaphore: {err:?}"
                    ));
                }
            };

        let render_finished_semaphore =
            match unsafe { device.create_semaphore(&semaphore_info, None) } {
                Ok(semaphore) => semaphore,
                Err(err) => {
                    unsafe {
                        device.destroy_semaphore(image_available_semaphore, None);
                        device.free_command_buffers(command_pool, &[command_buffer]);
                        device.destroy_command_pool(command_pool, None);
                        device.destroy_device(None);
                        surface_loader.destroy_surface(surface, None);
                        ndk_sys::ANativeWindow_release(window);
                        instance.destroy_instance(None);
                    }
                    return Err(format!(
                        "Android Vulkan XR init failed: create render semaphore: {err:?}"
                    ));
                }
            };

        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let in_flight_fence = match unsafe { device.create_fence(&fence_info, None) } {
            Ok(fence) => fence,
            Err(err) => {
                unsafe {
                    device.destroy_semaphore(render_finished_semaphore, None);
                    device.destroy_semaphore(image_available_semaphore, None);
                    device.free_command_buffers(command_pool, &[command_buffer]);
                    device.destroy_command_pool(command_pool, None);
                    device.destroy_device(None);
                    surface_loader.destroy_surface(surface, None);
                    ndk_sys::ANativeWindow_release(window);
                    instance.destroy_instance(None);
                }
                return Err(format!(
                    "Android Vulkan XR init failed: create_fence: {err:?}"
                ));
            }
        };

        let mut vulkan = Self {
            instance,
            surface_loader,
            android_surface_loader,
            surface,
            physical_device,
            queue_family_index,
            min_uniform_buffer_offset_alignment: props
                .limits
                .min_uniform_buffer_offset_alignment
                .max(4),
            device,
            external_memory_android_hardware_buffer,
            queue,
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            swapchain_images: Vec::new(),
            swapchain_image_views: Vec::new(),
            swapchain_depth_targets: Vec::new(),
            swapchain_readback_buffer: None,
            swapchain_format: vk::Format::UNDEFINED,
            depth_format: vk::Format::UNDEFINED,
            swapchain_extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            render_pass: vk::RenderPass::null(),
            xr_render_pass: vk::RenderPass::null(),
            framebuffers: Vec::new(),
            pipelines: HashMap::new(),
            offscreen_render_passes: HashMap::new(),
            geometries: HashMap::new(),
            textures: HashMap::new(),
            frame_resources: FrameResources::default(),
            command_pool,
            command_buffer,
            image_available_semaphore,
            render_finished_semaphore,
            in_flight_fence,
            window,
            requested_width: width.max(1),
            requested_height: height.max(1),
            texture_upload_count_this_frame: 0,
            texture_upload_bytes_this_frame: 0,
            debug_utils_enabled: has_debug_utils_ext,
            debug_utils_loader: None,
            debug_messenger: vk::DebugUtilsMessengerEXT::null(),
            xr_depth_dummy: None,
        };

        if let Err(err) = vulkan.recreate_swapchain() {
            return Err(format!(
                "Android Vulkan XR init failed: recreate_swapchain: {err}"
            ));
        }

        vulkan.try_enable_debug_messenger(&entry);

        Ok(vulkan)
    }

    fn try_enable_debug_messenger(&mut self, entry: &ash::Entry) {
        if !self.debug_utils_enabled {
            return;
        }
        let debug_loader = ash::ext::debug_utils::Instance::new(entry, &self.instance);
        let create_info = vulkan_debug_messenger_create_info();
        match unsafe { debug_loader.create_debug_utils_messenger(&create_info, None) } {
            Ok(messenger) => {
                self.debug_utils_loader = Some(debug_loader);
                self.debug_messenger = messenger;
            }
            Err(err) => {
                crate::warning!("Android Vulkan: failed to create debug messenger: {err:?}");
            }
        }
    }

    pub fn update_surface(
        &mut self,
        window: *mut ndk_sys::ANativeWindow,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if window.is_null() {
            return Err("Android Vulkan surface update failed: null ANativeWindow".to_string());
        }

        self.requested_width = width.max(1);
        self.requested_height = height.max(1);

        if self.window != window {
            unsafe { ANativeWindow_acquire(window) };

            self.device_wait_idle();
            self.destroy_swapchain();
            self.destroy_surface();

            unsafe { ndk_sys::ANativeWindow_release(self.window) };
            self.window = window;

            self.surface = Self::create_surface(&self.android_surface_loader, window)?;
        }

        self.recreate_swapchain()
    }

    pub fn suspend_surface(&mut self) {
        self.device_wait_idle();
        self.destroy_swapchain();
        self.destroy_surface();

        if !self.window.is_null() {
            unsafe { ndk_sys::ANativeWindow_release(self.window) };
            self.window = std::ptr::null_mut();
        }
    }

    pub(crate) fn swapchain_format(&self) -> vk::Format {
        self.swapchain_format
    }

    pub(crate) fn instance_handle(&self) -> vk::Instance {
        self.instance.handle()
    }

    pub(crate) fn physical_device_handle(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    pub(crate) fn device_handle(&self) -> vk::Device {
        self.device.handle()
    }

    pub(crate) fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    pub(crate) fn create_openxr_session_data(
        &mut self,
        color_images: &[vk::Image],
        depth_images: &[vk::Image],
        color_format: vk::Format,
        width: u32,
        height: u32,
        depth_width: u32,
        depth_height: u32,
    ) -> Result<CxVulkanOpenXrSessionData, String> {
        if self.xr_render_pass == vk::RenderPass::null() {
            return Err("Android Vulkan XR init failed: render pass is not ready".to_string());
        }
        if self.swapchain_format != color_format {
            return Err(format!(
                "Android Vulkan XR init failed: XR swapchain format {:?} does not match window render pass format {:?}",
                color_format, self.swapchain_format
            ));
        }

        self.ensure_xr_depth_dummy()?;

        let depth_readback_buffer = if depth_width > 0 && depth_height > 0 {
            let byte_len = depth_width as vk::DeviceSize
                * depth_height as vk::DeviceSize
                * std::mem::size_of::<u16>() as vk::DeviceSize;
            Some(self.create_host_buffer(vk::BufferUsageFlags::TRANSFER_DST, byte_len)?)
        } else {
            None
        };
        let color_readback_buffer = if width > 0 && height > 0 {
            let byte_len = width as vk::DeviceSize * height as vk::DeviceSize * 4;
            Some(self.create_host_buffer(vk::BufferUsageFlags::TRANSFER_DST, byte_len)?)
        } else {
            None
        };

        let mut xr_color_images = Vec::with_capacity(color_images.len());
        for &image in color_images {
            let eyes = [
                self.create_openxr_eye_target(image, 0, color_format, width, height)?,
                self.create_openxr_eye_target(image, 1, color_format, width, height)?,
            ];
            xr_color_images.push(CxVulkanOpenXrSwapchainImage { image, eyes });
        }

        let mut xr_depth_images = Vec::with_capacity(depth_images.len());
        let mut depth_view_error: Option<String> = None;
        for &image in depth_images {
            let views = match (
                self.create_openxr_depth_view(image, 0, vk::Format::D16_UNORM),
                self.create_openxr_depth_view(image, 1, vk::Format::D16_UNORM),
            ) {
                (Ok(left), Ok(right)) => [left, right],
                (left, right) => {
                    if let Ok(view) = left {
                        unsafe {
                            self.device.destroy_image_view(view, None);
                        }
                    }
                    if let Ok(view) = right {
                        unsafe {
                            self.device.destroy_image_view(view, None);
                        }
                    }
                    depth_view_error = Some(match (left.err(), right.err()) {
                        (Some(left_err), Some(right_err)) => {
                            format!("{left_err}; {right_err}")
                        }
                        (Some(err), None) | (None, Some(err)) => err,
                        (None, None) => "unknown depth-view creation failure".to_string(),
                    });
                    break;
                }
            };
            xr_depth_images.push(CxVulkanOpenXrDepthImage { image, views });
        }
        if let Some(err) = depth_view_error {
            crate::warning!(
                "OpenXR Vulkan: environment depth image views unavailable, disabling XR depth sampling: {}",
                err
            );
        }

        Ok(CxVulkanOpenXrSessionData {
            width: width.max(1),
            height: height.max(1),
            color_format,
            depth_width,
            depth_height,
            color_images: xr_color_images,
            depth_images: xr_depth_images,
            color_readback_buffer,
            depth_readback_buffer,
        })
    }

    fn create_openxr_eye_target(
        &self,
        image: vk::Image,
        eye: usize,
        color_format: vk::Format,
        width: u32,
        height: u32,
    ) -> Result<CxVulkanOpenXrEyeTarget, String> {
        let color_view = unsafe {
            self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(color_format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(eye as u32)
                            .layer_count(1),
                    ),
                None,
            )
        }
        .map_err(|e| format!("create_image_view(openxr color eye {eye}) failed: {e:?}"))?;

        let depth_target = match self.create_depth_target(width, height, self.depth_format) {
            Ok(depth_target) => depth_target,
            Err(err) => {
                unsafe {
                    self.device.destroy_image_view(color_view, None);
                }
                return Err(format!(
                    "create_depth_target(openxr eye {eye}) failed: {err}"
                ));
            }
        };

        let attachments = [color_view, depth_target.view];
        let framebuffer = match unsafe {
            self.device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(self.xr_render_pass)
                    .width(width.max(1))
                    .height(height.max(1))
                    .layers(1)
                    .attachments(&attachments),
                None,
            )
        } {
            Ok(framebuffer) => framebuffer,
            Err(e) => {
                unsafe {
                    self.device.destroy_image_view(color_view, None);
                }
                self.destroy_texture_resource(depth_target);
                return Err(format!(
                    "create_framebuffer(openxr eye {eye}) failed: {e:?}"
                ));
            }
        };

        Ok(CxVulkanOpenXrEyeTarget {
            framebuffer,
            color_view,
            depth_target,
        })
    }

    fn create_openxr_depth_view(
        &self,
        image: vk::Image,
        eye: usize,
        format: vk::Format,
    ) -> Result<vk::ImageView, String> {
        unsafe {
            self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::DEPTH)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(eye as u32)
                            .layer_count(1),
                    ),
                None,
            )
        }
        .map_err(|e| format!("create_image_view(openxr depth eye {eye}) failed: {e:?}"))
    }

    pub(crate) fn destroy_openxr_session_data(&mut self, session: CxVulkanOpenXrSessionData) {
        for image in session.color_images {
            for eye in image.eyes {
                unsafe {
                    if eye.framebuffer != vk::Framebuffer::null() {
                        self.device.destroy_framebuffer(eye.framebuffer, None);
                    }
                    if eye.color_view != vk::ImageView::null() {
                        self.device.destroy_image_view(eye.color_view, None);
                    }
                }
                self.destroy_texture_resource(eye.depth_target);
            }
        }
        for image in session.depth_images {
            for view in image.views {
                unsafe {
                    if view != vk::ImageView::null() {
                        self.device.destroy_image_view(view, None);
                    }
                }
            }
        }
        if let Some(buffer) = session.depth_readback_buffer {
            unsafe {
                if buffer.buffer != vk::Buffer::null() {
                    self.device.destroy_buffer(buffer.buffer, None);
                }
                if buffer.memory != vk::DeviceMemory::null() {
                    self.device.free_memory(buffer.memory, None);
                }
            }
        }
        if let Some(buffer) = session.color_readback_buffer {
            unsafe {
                if buffer.buffer != vk::Buffer::null() {
                    self.device.destroy_buffer(buffer.buffer, None);
                }
                if buffer.memory != vk::DeviceMemory::null() {
                    self.device.free_memory(buffer.memory, None);
                }
            }
        }
    }

    pub(crate) fn read_openxr_depth_image(
        &mut self,
        session: &CxVulkanOpenXrSessionData,
        depth_image_index: usize,
        eye_index: usize,
    ) -> Result<Vec<u16>, String> {
        let depth_image = session
            .depth_images
            .get(depth_image_index)
            .ok_or_else(|| format!("invalid OpenXR depth image index {depth_image_index}"))?;
        let staging = session
            .depth_readback_buffer
            .ok_or_else(|| "OpenXR depth readback buffer unavailable".to_string())?;
        if session.depth_width == 0 || session.depth_height == 0 {
            return Err("OpenXR depth swapchain has invalid dimensions".to_string());
        }

        let pixel_count = session.depth_width as usize * session.depth_height as usize;
        let byte_len = pixel_count as vk::DeviceSize * std::mem::size_of::<u16>() as vk::DeviceSize;

        unsafe {
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences(depth readback) failed: {e:?}"))?;
            self.device
                .reset_fences(&[self.in_flight_fence])
                .map_err(|e| format!("reset_fences(depth readback) failed: {e:?}"))?;
        }

        self.destroy_frame_resources();

        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset_command_buffer(depth readback) failed: {e:?}"))?;
            self.device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin_command_buffer(depth readback) failed: {e:?}"))?;
        }

        let to_transfer = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_READ)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .old_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(depth_image.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(eye_index as u32)
                    .layer_count(1),
            );
        let copy_region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .mip_level(0)
                    .base_array_layer(eye_index as u32)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: session.depth_width,
                height: session.depth_height,
                depth: 1,
            });
        let to_read_only = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .image(depth_image.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(eye_index as u32)
                    .layer_count(1),
            );
        let buffer_ready = vk::BufferMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .buffer(staging.buffer)
            .offset(0)
            .size(byte_len);

        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            self.device.cmd_copy_image_to_buffer(
                self.command_buffer,
                depth_image.image,
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
                &[to_read_only],
            );
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|e| format!("end_command_buffer(depth readback) failed: {e:?}"))?;
            self.device
                .queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&[self.command_buffer])],
                    self.in_flight_fence,
                )
                .map_err(|e| format!("queue_submit(depth readback) failed: {e:?}"))?;
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences(depth readback submit) failed: {e:?}"))?;
        }

        let depth = unsafe {
            let mapped = self
                .device
                .map_memory(staging.memory, 0, byte_len, vk::MemoryMapFlags::empty())
                .map_err(|e| format!("map_memory(depth readback) failed: {e:?}"))?;
            let data = std::slice::from_raw_parts(mapped as *const u16, pixel_count).to_vec();
            self.device.unmap_memory(staging.memory);
            data
        };

        Ok(depth)
    }

    pub(crate) fn read_openxr_color_image_rgba(
        &mut self,
        session: &CxVulkanOpenXrSessionData,
        color_image_index: usize,
        eye_index: usize,
    ) -> Result<Vec<u8>, String> {
        let color_image = session
            .color_images
            .get(color_image_index)
            .ok_or_else(|| format!("invalid OpenXR color image index {color_image_index}"))?;
        let staging = session
            .color_readback_buffer
            .ok_or_else(|| "OpenXR color readback buffer unavailable".to_string())?;
        if session.width == 0 || session.height == 0 {
            return Err("OpenXR color readback dimensions are zero".to_string());
        }

        let pixel_count = session.width as usize * session.height as usize;
        let byte_len = pixel_count as vk::DeviceSize * 4;

        unsafe {
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences(color readback) failed: {e:?}"))?;
            self.device
                .reset_fences(&[self.in_flight_fence])
                .map_err(|e| format!("reset_fences(color readback) failed: {e:?}"))?;
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset_command_buffer(color readback) failed: {e:?}"))?;
            self.device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin_command_buffer(color readback) failed: {e:?}"))?;
        }

        let to_transfer = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(color_image.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(eye_index as u32)
                    .layer_count(1),
            );
        let copy_region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(eye_index as u32)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: session.width,
                height: session.height,
                depth: 1,
            });
        let to_color_attachment = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(color_image.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(eye_index as u32)
                    .layer_count(1),
            );
        let buffer_ready = vk::BufferMemoryBarrier::default()
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
                color_image.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                staging.buffer,
                &[copy_region],
            );
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &[buffer_ready],
                &[to_color_attachment],
            );
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|e| format!("end_command_buffer(color readback) failed: {e:?}"))?;
            self.device
                .queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&[self.command_buffer])],
                    self.in_flight_fence,
                )
                .map_err(|e| format!("queue_submit(color readback) failed: {e:?}"))?;
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences(color readback submit) failed: {e:?}"))?;
        }

        let mut rgba = unsafe {
            let mapped = self
                .device
                .map_memory(staging.memory, 0, byte_len, vk::MemoryMapFlags::empty())
                .map_err(|e| format!("map_memory(color readback) failed: {e:?}"))?;
            let bytes = std::slice::from_raw_parts(mapped as *const u8, byte_len as usize).to_vec();
            self.device.unmap_memory(staging.memory);
            bytes
        };

        match session.color_format {
            vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB => {
                for px in rgba.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
            }
            vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB => {}
            other => {
                return Err(format!(
                    "OpenXR color readback does not support format {:?}",
                    other
                ));
            }
        }

        Ok(rgba)
    }

    fn swapchain_readback_supported(&self) -> bool {
        matches!(
            self.swapchain_format,
            vk::Format::B8G8R8A8_UNORM
                | vk::Format::B8G8R8A8_SRGB
                | vk::Format::R8G8B8A8_UNORM
                | vk::Format::R8G8B8A8_SRGB
        )
    }

    fn read_swapchain_color_image_rgba(&mut self, image_index: usize) -> Result<Vec<u8>, String> {
        let _image = *self
            .swapchain_images
            .get(image_index)
            .ok_or_else(|| format!("invalid swapchain image index {image_index}"))?;
        let staging = self
            .swapchain_readback_buffer
            .ok_or_else(|| "swapchain color readback buffer unavailable".to_string())?;
        let width = self.swapchain_extent.width;
        let height = self.swapchain_extent.height;
        if width == 0 || height == 0 {
            return Err("swapchain color readback dimensions are zero".to_string());
        }

        let byte_len = width as vk::DeviceSize * height as vk::DeviceSize * 4;
        let mut rgba = unsafe {
            let mapped = self
                .device
                .map_memory(staging.memory, 0, byte_len, vk::MemoryMapFlags::empty())
                .map_err(|e| format!("map_memory(swapchain color readback) failed: {e:?}"))?;
            let bytes = std::slice::from_raw_parts(mapped as *const u8, byte_len as usize).to_vec();
            self.device.unmap_memory(staging.memory);
            bytes
        };

        match self.swapchain_format {
            vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB => {
                for px in rgba.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
            }
            vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB => {}
            other => {
                return Err(format!(
                    "swapchain color readback does not support format {:?}",
                    other
                ));
            }
        }

        Ok(rgba)
    }

    pub(crate) fn draw_openxr_view(
        &mut self,
        cx: &mut Cx,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        session: &CxVulkanOpenXrSessionData,
        color_image_index: usize,
        eye_index: usize,
        depth_image_index: Option<usize>,
    ) -> Result<(), String> {
        let color_target = session
            .color_images
            .get(color_image_index)
            .ok_or_else(|| format!("invalid OpenXR color image index {color_image_index}"))?;
        let xr_depth_view = depth_image_index
            .and_then(|index| session.depth_images.get(index))
            .map(|image| image.views[eye_index])
            .unwrap_or(self.ensure_xr_depth_dummy()?);

        unsafe {
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences(openxr) failed: {e:?}"))?;
            self.device
                .reset_fences(&[self.in_flight_fence])
                .map_err(|e| format!("reset_fences(openxr) failed: {e:?}"))?;
        }

        self.destroy_frame_resources();
        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset_command_buffer(openxr) failed: {e:?}"))?;
            self.device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin_command_buffer(openxr) failed: {e:?}"))?;
        }

        self.texture_upload_count_this_frame = 0;
        self.texture_upload_bytes_this_frame = 0;
        self.prepare_draw_list_textures(cx, draw_list_id)?;

        let clear_color = if cx.passes[draw_pass_id].color_textures.is_empty() {
            cx.passes[draw_pass_id].clear_color
        } else {
            match cx.passes[draw_pass_id].color_textures[0].clear_color {
                DrawPassClearColor::InitWith(color) => color,
                DrawPassClearColor::ClearWith(color) => color,
            }
        };
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

        unsafe {
            self.device.cmd_begin_render_pass(
                self.command_buffer,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.xr_render_pass)
                    .framebuffer(color_target.eyes[eye_index].framebuffer)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: vk::Extent2D {
                            width: session.width,
                            height: session.height,
                        },
                    })
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_set_viewport(
                self.command_buffer,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: session.height as f32,
                    width: session.width as f32,
                    height: -(session.height as f32),
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            self.device.cmd_set_scissor(
                self.command_buffer,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D {
                        width: session.width,
                        height: session.height,
                    },
                }],
            );
        }

        let render_pass_key = self.main_render_pass_key();
        let mut zbias = 0.0f32;
        let zbias_step = cx.passes[draw_pass_id].zbias_step;
        let mut draw_stats = VulkanDrawStats::default();
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
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|e| format!("end_command_buffer(openxr) failed: {e:?}"))?;
            self.device
                .queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&[self.command_buffer])],
                    self.in_flight_fence,
                )
                .map_err(|e| format!("queue_submit(openxr) failed: {e:?}"))?;
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences(openxr submit) failed: {e:?}"))?;
        }

        Ok(())
    }

    pub fn draw_pass_and_present(
        &mut self,
        cx: &mut Cx,
        draw_pass_id: DrawPassId,
    ) -> Result<(), String> {
        if self.surface == vk::SurfaceKHR::null() || self.swapchain == vk::SwapchainKHR::null() {
            return Ok(());
        }

        let draw_list_id = if let Some(id) = cx.passes[draw_pass_id].main_draw_list_id {
            id
        } else {
            return Ok(());
        };

        let dpi_factor = cx.passes[draw_pass_id].dpi_factor.unwrap_or(1.0);
        let pass_rect = match cx.get_pass_rect(draw_pass_id, dpi_factor) {
            Some(rect) => rect,
            None => return Ok(()),
        };
        if pass_rect.size.x < 0.5 || pass_rect.size.y < 0.5 {
            return Ok(());
        }

        {
            let pass = &mut cx.passes[draw_pass_id];
            pass.paint_dirty = false;
            pass.set_ortho_matrix(pass_rect.pos, pass_rect.size);
            pass.set_dpi_factor(dpi_factor);
        }

        let clear_color = if cx.passes[draw_pass_id].color_textures.is_empty() {
            cx.passes[draw_pass_id].clear_color
        } else {
            match cx.passes[draw_pass_id].color_textures[0].clear_color {
                DrawPassClearColor::InitWith(color) => color,
                DrawPassClearColor::ClearWith(color) => color,
            }
        };

        unsafe {
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences failed: {e:?}"))?;
            self.device
                .reset_fences(&[self.in_flight_fence])
                .map_err(|e| format!("reset_fences failed: {e:?}"))?;
        }

        self.destroy_frame_resources();

        let (image_index, acquire_suboptimal) = match unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain,
                u64::MAX,
                self.image_available_semaphore,
                vk::Fence::null(),
            )
        } {
            Ok(v) => v,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate_swapchain()?;
                return Ok(());
            }
            Err(err) => {
                return Err(format!("acquire_next_image failed: {err:?}"));
            }
        };
        if self.swapchain_images.get(image_index as usize).is_none() {
            return Err(format!("invalid swapchain image index {image_index}"));
        }
        let screenshot_request_ids = cx.take_studio_screenshot_request_ids(0);
        let run_view_request = cx.take_studio_run_view_frame_request(0);
        let capture_swapchain =
            !screenshot_request_ids.is_empty() || run_view_request.is_some();
        if capture_swapchain && self.swapchain_readback_buffer.is_none() {
            return Err("swapchain capture requested but readback buffer is unavailable".to_string());
        }

        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset_command_buffer failed: {e:?}"))?;
        }

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe {
            self.device
                .begin_command_buffer(self.command_buffer, &begin_info)
                .map_err(|e| format!("begin_command_buffer failed: {e:?}"))?;
        }

        self.texture_upload_count_this_frame = 0;
        self.texture_upload_bytes_this_frame = 0;
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
            .get(image_index as usize)
            .ok_or_else(|| format!("invalid framebuffer index {image_index}"))?;
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain_extent,
            })
            .clear_values(&clear_values);

        unsafe {
            self.device.cmd_begin_render_pass(
                self.command_buffer,
                &render_pass_info,
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_set_viewport(
                self.command_buffer,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: self.swapchain_extent.height as f32,
                    width: self.swapchain_extent.width as f32,
                    height: -(self.swapchain_extent.height as f32),
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            self.device.cmd_set_scissor(
                self.command_buffer,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain_extent,
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

        if capture_swapchain {
            let width = self.swapchain_extent.width;
            let height = self.swapchain_extent.height;
            let byte_len = width as vk::DeviceSize * height as vk::DeviceSize * 4;
            let staging = self
                .swapchain_readback_buffer
                .ok_or_else(|| "swapchain color readback buffer unavailable".to_string())?;
            let image = self.swapchain_images[image_index as usize];
            let to_transfer = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(image)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                );
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
                    width,
                    height,
                    depth: 1,
                });
            let to_present = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::empty())
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .image(image)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                );
            let buffer_ready = vk::BufferMemoryBarrier::default()
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
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE | vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[buffer_ready],
                    &[to_present],
                );
            }
        }

        unsafe {
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|e| format!("end_command_buffer failed: {e:?}"))?;
        }

        let wait_semaphores = [self.image_available_semaphore];
        let signal_semaphores = [self.render_finished_semaphore];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let cmd_buffers = [self.command_buffer];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&cmd_buffers)
            .signal_semaphores(&signal_semaphores);

        unsafe {
            self.device
                .queue_submit(self.queue, &[submit_info], self.in_flight_fence)
                .map_err(|e| format!("queue_submit failed: {e:?}"))?;
        }

        if capture_swapchain {
            unsafe {
                self.device
                    .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                    .map_err(|e| format!("wait_for_fences(swapchain capture) failed: {e:?}"))?;
            }
            let width = self.swapchain_extent.width.max(1);
            let height = self.swapchain_extent.height.max(1);
            let rgba = self.read_swapchain_color_image_rgba(image_index as usize)?;

            if !screenshot_request_ids.is_empty() {
                let png = Cx::encode_rgba_as_png(width, height, &rgba)?;
                Cx::send_studio_screenshot_response(screenshot_request_ids, width, height, png);
            }

            if let Some(request) = run_view_request {
                cx.encode_studio_run_view_frame_async(request, width, height, rgba);
            }
        }

        let swapchains = [self.swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);

        let present_suboptimal = match unsafe {
            self.swapchain_loader
                .queue_present(self.queue, &present_info)
        } {
            Ok(suboptimal) => suboptimal,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate_swapchain()?;
                return Ok(());
            }
            Err(err) => {
                return Err(format!("queue_present failed: {err:?}"));
            }
        };

        if acquire_suboptimal || present_suboptimal {
            self.recreate_swapchain()?;
        }

        Ok(())
    }

    fn ensure_pass_color_target(
        &mut self,
        cx: &mut Cx,
        texture_id: TextureId,
        width: usize,
        height: usize,
    ) -> Result<(), String> {
        let texture_key = Self::texture_key(texture_id);
        let (alloc_changed, alloc) = {
            let cxtexture = &mut cx.textures[texture_id];
            let alloc_changed = cxtexture.alloc_render(width, height);
            let alloc = cxtexture.alloc.clone().ok_or_else(|| {
                format!(
                    "render target texture {} missing allocation metadata",
                    texture_key
                )
            })?;
            (alloc_changed, alloc)
        };

        let format = Self::vk_color_format_from_texture_pixel(alloc.pixel).ok_or_else(|| {
            format!(
                "unsupported Vulkan render target pixel format for texture {}",
                texture_key
            )
        })?;
        let target_width = alloc.width.max(1) as u32;
        let target_height = alloc.height.max(1) as u32;
        let needs_recreate = match self.textures.get(&texture_key) {
            Some(resource) => {
                alloc_changed
                    || resource.width != target_width
                    || resource.height != target_height
                    || resource.format != format
                    || resource.layers != 1
                    || resource.is_cube
            }
            None => true,
        };

        if needs_recreate {
            if let Some(old_resource) = self.textures.remove(&texture_key) {
                self.destroy_texture_resource(old_resource);
            }
            let resource =
                self.create_color_target_resource(target_width, target_height, format)?;
            self.textures.insert(texture_key, resource);
        }
        Ok(())
    }

    fn ensure_pass_depth_target(
        &mut self,
        cx: &mut Cx,
        texture_id: TextureId,
        width: usize,
        height: usize,
    ) -> Result<(), String> {
        let texture_key = Self::texture_key(texture_id);
        let (alloc_changed, alloc) = {
            let cxtexture = &mut cx.textures[texture_id];
            let alloc_changed = cxtexture.alloc_depth(width, height);
            let alloc = cxtexture.alloc.clone().ok_or_else(|| {
                format!(
                    "depth target texture {} missing allocation metadata",
                    texture_key
                )
            })?;
            (alloc_changed, alloc)
        };

        let format = match alloc.pixel {
            TexturePixel::D32 => vk::Format::D32_SFLOAT,
            _ => {
                return Err(format!(
                    "unsupported Vulkan depth target pixel format for texture {}",
                    texture_key
                ));
            }
        };
        let target_width = alloc.width.max(1) as u32;
        let target_height = alloc.height.max(1) as u32;
        let needs_recreate = match self.textures.get(&texture_key) {
            Some(resource) => {
                alloc_changed
                    || resource.width != target_width
                    || resource.height != target_height
                    || resource.format != format
                    || resource.layers != 1
                    || resource.is_cube
            }
            None => true,
        };

        if needs_recreate {
            if let Some(old_resource) = self.textures.remove(&texture_key) {
                self.destroy_texture_resource(old_resource);
            }
            let resource = self.create_depth_target(target_width, target_height, format)?;
            self.textures.insert(texture_key, resource);
        }
        Ok(())
    }

    fn main_render_pass_key(&self) -> VulkanRenderPassKey {
        VulkanRenderPassKey::new(&[self.swapchain_format], Some(self.depth_format))
    }

    fn get_or_create_pipeline_render_pass(
        &mut self,
        key: &VulkanRenderPassKey,
    ) -> Result<vk::RenderPass, String> {
        if *key == self.main_render_pass_key() {
            return Ok(self.render_pass);
        }
        if let Some(render_pass) = self.offscreen_render_passes.get(key) {
            return Ok(*render_pass);
        }

        let color_formats = key.color_vk_formats();
        let depth_format = key.depth_vk_format();
        let mut attachments =
            Vec::with_capacity(color_formats.len() + depth_format.is_some() as usize);
        let mut color_refs = Vec::with_capacity(color_formats.len());
        for (index, format) in color_formats.iter().enumerate() {
            attachments.push(
                vk::AttachmentDescription::default()
                    .format(*format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::LOAD)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
            );
            color_refs.push(
                vk::AttachmentReference::default()
                    .attachment(index as u32)
                    .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
            );
        }
        let depth_ref = if let Some(format) = depth_format {
            attachments.push(
                vk::AttachmentDescription::default()
                    .format(format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::LOAD)
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                    .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            );
            Some(
                vk::AttachmentReference::default()
                    .attachment(color_formats.len() as u32)
                    .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            )
        } else {
            None
        };

        let mut subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs);
        if let Some(depth_ref) = depth_ref.as_ref() {
            subpass = subpass.depth_stencil_attachment(depth_ref);
        }
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .src_access_mask(
                vk::AccessFlags::SHADER_READ
                    | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_READ
                    | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )];
        let subpasses = [subpass];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        let render_pass = unsafe { self.device.create_render_pass(&render_pass_info, None) }
            .map_err(|e| format!("create_render_pass(pipeline-cache) failed: {e:?}"))?;
        self.offscreen_render_passes
            .insert(key.clone(), render_pass);
        Ok(render_pass)
    }

    pub fn draw_pass_to_texture(
        &mut self,
        cx: &mut Cx,
        draw_pass_id: DrawPassId,
    ) -> Result<(), String> {
        let draw_list_id = if let Some(id) = cx.passes[draw_pass_id].main_draw_list_id {
            id
        } else {
            return Ok(());
        };

        let dpi_factor = cx.passes[draw_pass_id].dpi_factor.unwrap_or(1.0);
        let pass_rect = match cx.get_pass_rect(draw_pass_id, dpi_factor) {
            Some(rect) => rect,
            None => return Ok(()),
        };
        if pass_rect.size.x < 0.5 || pass_rect.size.y < 0.5 {
            return Ok(());
        }

        {
            let pass = &mut cx.passes[draw_pass_id];
            pass.paint_dirty = false;
            pass.set_ortho_matrix(pass_rect.pos, pass_rect.size);
            pass.set_dpi_factor(dpi_factor);
        }

        let target_width = (dpi_factor * pass_rect.size.x).max(1.0) as usize;
        let target_height = (dpi_factor * pass_rect.size.y).max(1.0) as usize;

        #[derive(Clone, Copy)]
        struct ColorAttachmentState {
            texture_id: TextureId,
            view: vk::ImageView,
            image: vk::Image,
            format: vk::Format,
            old_layout: vk::ImageLayout,
            should_clear: bool,
        }

        #[derive(Clone, Copy)]
        struct DepthAttachmentState {
            texture_id: TextureId,
            view: vk::ImageView,
            image: vk::Image,
            format: vk::Format,
            old_layout: vk::ImageLayout,
            should_clear: bool,
        }

        let pass_dont_clear = cx.passes[draw_pass_id].dont_clear;
        let color_targets: Vec<_> = cx.passes[draw_pass_id]
            .color_textures
            .iter()
            .map(|color_texture| {
                (
                    color_texture.texture.texture_id(),
                    color_texture.clear_color.clone(),
                )
            })
            .collect();
        if color_targets.is_empty() {
            return Ok(());
        }
        let depth_target = cx.passes[draw_pass_id]
            .depth_texture
            .as_ref()
            .map(|texture| texture.texture_id());
        let clear_depth_value = match cx.passes[draw_pass_id].clear_depth {
            DrawPassClearDepth::InitWith(depth) | DrawPassClearDepth::ClearWith(depth) => depth,
        };

        for (texture_id, _) in &color_targets {
            self.ensure_pass_color_target(cx, *texture_id, target_width, target_height)?;
        }
        if let Some(texture_id) = depth_target {
            self.ensure_pass_depth_target(cx, texture_id, target_width, target_height)?;
        }

        unsafe {
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences(offscreen) failed: {e:?}"))?;
            self.device
                .reset_fences(&[self.in_flight_fence])
                .map_err(|e| format!("reset_fences(offscreen) failed: {e:?}"))?;
        }

        self.destroy_frame_resources();
        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset_command_buffer(offscreen) failed: {e:?}"))?;
            self.device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin_command_buffer(offscreen) failed: {e:?}"))?;
        }

        self.texture_upload_count_this_frame = 0;
        self.texture_upload_bytes_this_frame = 0;
        self.prepare_draw_list_textures(cx, draw_list_id)?;

        let mut color_attachments = Vec::with_capacity(color_targets.len());
        let mut clear_values = Vec::with_capacity(color_targets.len() + 1);
        for (texture_id, clear_color) in &color_targets {
            let should_clear = match clear_color {
                DrawPassClearColor::InitWith(_) => {
                    !pass_dont_clear && cx.textures[*texture_id].take_initial()
                }
                DrawPassClearColor::ClearWith(_) => !pass_dont_clear,
            };
            let clear = match clear_color {
                DrawPassClearColor::InitWith(color) | DrawPassClearColor::ClearWith(color) => {
                    *color
                }
            };
            let resource = self
                .textures
                .get(&Self::texture_key(*texture_id))
                .ok_or_else(|| {
                    format!("missing Vulkan color target for texture {:?}", texture_id)
                })?;
            color_attachments.push(ColorAttachmentState {
                texture_id: *texture_id,
                view: resource.view,
                image: resource.image,
                format: resource.format,
                old_layout: resource.layout,
                should_clear,
            });
            clear_values.push(vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [clear.x, clear.y, clear.z, clear.w],
                },
            });
        }

        let depth_attachment = if let Some(texture_id) = depth_target {
            let should_clear = match cx.passes[draw_pass_id].clear_depth {
                DrawPassClearDepth::InitWith(_) => {
                    !pass_dont_clear && cx.textures[texture_id].take_initial()
                }
                DrawPassClearDepth::ClearWith(_) => !pass_dont_clear,
            };
            let resource = self
                .textures
                .get(&Self::texture_key(texture_id))
                .ok_or_else(|| {
                    format!("missing Vulkan depth target for texture {:?}", texture_id)
                })?;
            clear_values.push(vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: clear_depth_value,
                    stencil: 0,
                },
            });
            Some(DepthAttachmentState {
                texture_id,
                view: resource.view,
                image: resource.image,
                format: resource.format,
                old_layout: resource.layout,
                should_clear,
            })
        } else {
            None
        };

        for attachment in &color_attachments {
            self.transition_image_layout(
                attachment.image,
                vk::ImageAspectFlags::COLOR,
                1,
                attachment.old_layout,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            );
        }
        if let Some(depth) = depth_attachment {
            self.transition_image_layout(
                depth.image,
                vk::ImageAspectFlags::DEPTH,
                1,
                depth.old_layout,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            );
        }

        let color_attachment_descriptions: Vec<_> = color_attachments
            .iter()
            .map(|attachment| {
                vk::AttachmentDescription::default()
                    .format(attachment.format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(if attachment.should_clear {
                        vk::AttachmentLoadOp::CLEAR
                    } else {
                        vk::AttachmentLoadOp::LOAD
                    })
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            })
            .collect();
        let mut attachments = color_attachment_descriptions;
        let color_refs: Vec<_> = (0..color_attachments.len())
            .map(|index| {
                vk::AttachmentReference::default()
                    .attachment(index as u32)
                    .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            })
            .collect();
        let depth_ref = depth_attachment.as_ref().map(|_| {
            vk::AttachmentReference::default()
                .attachment(color_attachments.len() as u32)
                .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
        });
        if let Some(depth) = depth_attachment {
            attachments.push(
                vk::AttachmentDescription::default()
                    .format(depth.format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(if depth.should_clear {
                        vk::AttachmentLoadOp::CLEAR
                    } else {
                        vk::AttachmentLoadOp::LOAD
                    })
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                    .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            );
        }

        let mut subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs);
        if let Some(depth_ref) = depth_ref.as_ref() {
            subpass = subpass.depth_stencil_attachment(depth_ref);
        }
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .src_access_mask(
                vk::AccessFlags::SHADER_READ
                    | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_READ
                    | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )];
        let subpasses = [subpass];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        let render_pass = unsafe { self.device.create_render_pass(&render_pass_info, None) }
            .map_err(|e| format!("create_render_pass(offscreen) failed: {e:?}"))?;

        let mut framebuffer_attachments: Vec<vk::ImageView> = color_attachments
            .iter()
            .map(|attachment| attachment.view)
            .collect();
        if let Some(depth) = depth_attachment {
            framebuffer_attachments.push(depth.view);
        }
        let framebuffer_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&framebuffer_attachments)
            .width(target_width as u32)
            .height(target_height as u32)
            .layers(1);
        let framebuffer = unsafe { self.device.create_framebuffer(&framebuffer_info, None) }
            .map_err(|e| format!("create_framebuffer(offscreen) failed: {e:?}"))?;

        unsafe {
            self.device.cmd_begin_render_pass(
                self.command_buffer,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(render_pass)
                    .framebuffer(framebuffer)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: vk::Extent2D {
                            width: target_width as u32,
                            height: target_height as u32,
                        },
                    })
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_set_viewport(
                self.command_buffer,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: target_height as f32,
                    width: target_width as f32,
                    height: -(target_height as f32),
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            self.device.cmd_set_scissor(
                self.command_buffer,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D {
                        width: target_width as u32,
                        height: target_height as u32,
                    },
                }],
            );
        }

        let xr_depth_view = self.ensure_xr_depth_dummy()?;
        let render_pass_key = VulkanRenderPassKey::new(
            &color_attachments
                .iter()
                .map(|attachment| attachment.format)
                .collect::<Vec<_>>(),
            depth_attachment.map(|depth| depth.format),
        );
        let mut zbias = 0.0f32;
        let zbias_step = cx.passes[draw_pass_id].zbias_step;
        let mut draw_stats = VulkanDrawStats::default();
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
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|e| format!("end_command_buffer(offscreen) failed: {e:?}"))?;
            self.device
                .queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&[self.command_buffer])],
                    self.in_flight_fence,
                )
                .map_err(|e| format!("queue_submit(offscreen) failed: {e:?}"))?;
            self.device
                .wait_for_fences(&[self.in_flight_fence], true, u64::MAX)
                .map_err(|e| format!("wait_for_fences(offscreen submit) failed: {e:?}"))?;
            self.device.destroy_framebuffer(framebuffer, None);
            self.device.destroy_render_pass(render_pass, None);
        }

        for attachment in &color_attachments {
            if let Some(resource) = self
                .textures
                .get_mut(&Self::texture_key(attachment.texture_id))
            {
                resource.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
            }
        }
        if let Some(depth) = depth_attachment {
            if let Some(resource) = self.textures.get_mut(&Self::texture_key(depth.texture_id)) {
                resource.layout = vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
            }
        }

        Ok(())
    }

    fn prepare_draw_list_textures(
        &mut self,
        cx: &mut Cx,
        draw_list_id: DrawListId,
    ) -> Result<(), String> {
        let mut seen = HashSet::<VulkanTextureKey>::new();
        self.prepare_draw_list_textures_inner(cx, draw_list_id, &mut seen)
    }

    fn prepare_draw_list_textures_inner(
        &mut self,
        cx: &mut Cx,
        draw_list_id: DrawListId,
        seen: &mut HashSet<VulkanTextureKey>,
    ) -> Result<(), String> {
        let draw_order_len = cx.draw_lists[draw_list_id].draw_item_order_len();
        for order_index in 0..draw_order_len {
            let Some(draw_item_id) =
                cx.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
            else {
                continue;
            };
            let (sub_list_id, texture_ids) = {
                let draw_list = &cx.draw_lists[draw_list_id];
                let draw_item = &draw_list.draw_items[draw_item_id];
                if let Some(sub_list_id) = draw_item.kind.sub_list() {
                    (Some(sub_list_id), Vec::new())
                } else if let Some(draw_call) = draw_item.kind.draw_call() {
                    let sh = &cx.draw_shaders.shaders[draw_call.draw_shader_id.index];
                    let null_texture_id = cx.null_texture.texture_id();
                    let null_cube_texture_id = cx.null_cube_texture.texture_id();
                    let texture_ids = (0..sh.mapping.textures.len())
                        .map(|i| {
                            draw_call.texture_slots[i]
                                .as_ref()
                                .map(|texture| texture.texture_id())
                                .unwrap_or_else(|| {
                                    if matches!(
                                        sh.mapping.textures[i].tex_type,
                                        TextureType::TextureCube | TextureType::TextureCubeArray
                                    ) {
                                        null_cube_texture_id
                                    } else {
                                        null_texture_id
                                    }
                                })
                        })
                        .collect();
                    (None, texture_ids)
                } else {
                    (None, Vec::new())
                }
            };

            if let Some(sub_list_id) = sub_list_id {
                self.prepare_draw_list_textures_inner(cx, sub_list_id, seen)?;
                continue;
            }

            for texture_id in texture_ids {
                if seen.insert(Self::texture_key(texture_id)) {
                    self.ensure_texture_uploaded(cx, texture_id)?;
                }
            }
        }
        Ok(())
    }

    fn vec_texture_meta(format: &TextureFormat) -> Option<(u32, u32, u32, bool, vk::Format)> {
        match format {
            TextureFormat::VecBGRAu8_32 { width, height, .. } => Some((
                *width as u32,
                *height as u32,
                1,
                false,
                vk::Format::B8G8R8A8_UNORM,
            )),
            TextureFormat::VecCubeBGRAu8_32 { width, height, .. } => Some((
                *width as u32,
                *height as u32,
                6,
                true,
                vk::Format::B8G8R8A8_UNORM,
            )),
            TextureFormat::VecMipBGRAu8_32 { width, height, .. } => Some((
                *width as u32,
                *height as u32,
                1,
                false,
                vk::Format::B8G8R8A8_UNORM,
            )),
            TextureFormat::VecRGBAf32 { width, height, .. } => Some((
                *width as u32,
                *height as u32,
                1,
                false,
                vk::Format::R32G32B32A32_SFLOAT,
            )),
            TextureFormat::VecRu8 { width, height, .. } => Some((
                *width as u32,
                *height as u32,
                1,
                false,
                vk::Format::R8_UNORM,
            )),
            TextureFormat::VecRGu8 { width, height, .. } => Some((
                *width as u32,
                *height as u32,
                1,
                false,
                vk::Format::R8G8_UNORM,
            )),
            TextureFormat::VecRf32 { width, height, .. } => Some((
                *width as u32,
                *height as u32,
                1,
                false,
                vk::Format::R32_SFLOAT,
            )),
            _ => None,
        }
    }

    fn texture_upload_rect(
        width: usize,
        height: usize,
        updated: TextureUpdated,
        force_full: bool,
    ) -> Option<(usize, usize, usize, usize)> {
        if width == 0 || height == 0 {
            return None;
        }
        if force_full {
            return Some((0, 0, width, height));
        }
        match updated {
            TextureUpdated::Empty => None,
            TextureUpdated::Full => Some((0, 0, width, height)),
            TextureUpdated::Partial(rect) => {
                let x0 = rect.origin.x.min(width);
                let y0 = rect.origin.y.min(height);
                let x1 = rect.origin.x.saturating_add(rect.size.width).min(width);
                let y1 = rect.origin.y.saturating_add(rect.size.height).min(height);
                if x1 <= x0 || y1 <= y0 {
                    None
                } else {
                    Some((x0, y0, x1 - x0, y1 - y0))
                }
            }
        }
    }

    fn pack_texture_region_bytes(
        src: &[u8],
        src_row_pixels: usize,
        bytes_per_pixel: usize,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    ) -> Vec<u8> {
        let row_bytes = width.saturating_mul(bytes_per_pixel);
        let mut out = vec![0u8; row_bytes.saturating_mul(height)];
        for row in 0..height {
            let src_offset = (y + row)
                .saturating_mul(src_row_pixels)
                .saturating_add(x)
                .saturating_mul(bytes_per_pixel);
            let dst_offset = row.saturating_mul(row_bytes);
            let src_end = src_offset.saturating_add(row_bytes);
            if src_end <= src.len() && dst_offset + row_bytes <= out.len() {
                out[dst_offset..dst_offset + row_bytes].copy_from_slice(&src[src_offset..src_end]);
            }
        }
        out
    }

    fn vec_texture_upload(
        format: &TextureFormat,
        updated: TextureUpdated,
        force_full: bool,
    ) -> Option<VulkanTextureUpload> {
        match format {
            TextureFormat::VecBGRAu8_32 {
                width,
                height,
                data,
                ..
            }
            | TextureFormat::VecMipBGRAu8_32 {
                width,
                height,
                data,
                ..
            } => {
                let (x, y, w, h) = Self::texture_upload_rect(*width, *height, updated, force_full)?;
                let out = if let Some(data) = data.as_ref() {
                    let src = unsafe {
                        std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
                    };
                    Self::pack_texture_region_bytes(src, *width, 4, x, y, w, h)
                } else {
                    vec![0u8; w.saturating_mul(h).saturating_mul(4)]
                };
                Some(VulkanTextureUpload {
                    data: out,
                    offset_x: x as u32,
                    offset_y: y as u32,
                    width: w as u32,
                    height: h as u32,
                    layers: 1,
                })
            }
            TextureFormat::VecCubeBGRAu8_32 {
                width,
                height,
                data,
                ..
            } => {
                let w = *width;
                let h = *height;
                if w == 0 || h == 0 {
                    return None;
                }
                let out = if let Some(data) = data.as_ref() {
                    let src = unsafe {
                        std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
                    };
                    let expected = w.saturating_mul(h).saturating_mul(4).saturating_mul(6);
                    if src.len() >= expected {
                        src[..expected].to_vec()
                    } else {
                        vec![0u8; expected]
                    }
                } else {
                    vec![0u8; w.saturating_mul(h).saturating_mul(4).saturating_mul(6)]
                };
                Some(VulkanTextureUpload {
                    data: out,
                    offset_x: 0,
                    offset_y: 0,
                    width: w as u32,
                    height: h as u32,
                    layers: 6,
                })
            }
            TextureFormat::VecRGBAf32 {
                width,
                height,
                data,
                ..
            } => {
                let (x, y, w, h) = Self::texture_upload_rect(*width, *height, updated, force_full)?;
                let out = if let Some(data) = data.as_ref() {
                    let src = unsafe {
                        std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
                    };
                    Self::pack_texture_region_bytes(src, *width, 16, x, y, w, h)
                } else {
                    vec![0u8; w.saturating_mul(h).saturating_mul(16)]
                };
                Some(VulkanTextureUpload {
                    data: out,
                    offset_x: x as u32,
                    offset_y: y as u32,
                    width: w as u32,
                    height: h as u32,
                    layers: 1,
                })
            }
            TextureFormat::VecRf32 {
                width,
                height,
                data,
                ..
            } => {
                let (x, y, w, h) = Self::texture_upload_rect(*width, *height, updated, force_full)?;
                let out = if let Some(data) = data.as_ref() {
                    let src = unsafe {
                        std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
                    };
                    Self::pack_texture_region_bytes(src, *width, 4, x, y, w, h)
                } else {
                    vec![0u8; w.saturating_mul(h).saturating_mul(4)]
                };
                Some(VulkanTextureUpload {
                    data: out,
                    offset_x: x as u32,
                    offset_y: y as u32,
                    width: w as u32,
                    height: h as u32,
                    layers: 1,
                })
            }
            TextureFormat::VecRu8 {
                width,
                height,
                data,
                unpack_row_length,
                ..
            } => {
                let (x, y, w, h) = Self::texture_upload_rect(*width, *height, updated, force_full)?;
                let row_len = unpack_row_length.unwrap_or(*width);
                let out = if let Some(data) = data.as_ref() {
                    Self::pack_texture_region_bytes(data, row_len, 1, x, y, w, h)
                } else {
                    vec![0u8; w.saturating_mul(h)]
                };
                Some(VulkanTextureUpload {
                    data: out,
                    offset_x: x as u32,
                    offset_y: y as u32,
                    width: w as u32,
                    height: h as u32,
                    layers: 1,
                })
            }
            TextureFormat::VecRGu8 {
                width,
                height,
                data,
                unpack_row_length,
                ..
            } => {
                let (x, y, w, h) = Self::texture_upload_rect(*width, *height, updated, force_full)?;
                let row_len = unpack_row_length.unwrap_or(*width);
                let out = if let Some(data) = data.as_ref() {
                    Self::pack_texture_region_bytes(data, row_len, 2, x, y, w, h)
                } else {
                    vec![0u8; w.saturating_mul(h).saturating_mul(2)]
                };
                Some(VulkanTextureUpload {
                    data: out,
                    offset_x: x as u32,
                    offset_y: y as u32,
                    width: w as u32,
                    height: h as u32,
                    layers: 1,
                })
            }
            _ => None,
        }
    }

    fn create_texture_resource(
        &self,
        width: u32,
        height: u32,
        layers: u32,
        is_cube: bool,
        format: vk::Format,
    ) -> Result<VulkanTextureResource, String> {
        let image_flags = if is_cube {
            vk::ImageCreateFlags::CUBE_COMPATIBLE
        } else {
            vk::ImageCreateFlags::empty()
        };
        let image_info = vk::ImageCreateInfo::default()
            .flags(image_flags)
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(layers.max(1))
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { self.device.create_image(&image_info, None) }
            .map_err(|e| format!("create_image failed: {e:?}"))?;
        let memory_req = unsafe { self.device.get_image_memory_requirements(image) };
        let memory_type_index = self
            .find_memory_type(
                memory_req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|_| {
                self.find_memory_type(
                    memory_req.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
            })?;
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(memory_req.size)
            .memory_type_index(memory_type_index);
        let memory = match unsafe { self.device.allocate_memory(&alloc_info, None) } {
            Ok(memory) => memory,
            Err(e) => {
                unsafe {
                    self.device.destroy_image(image, None);
                }
                return Err(format!("allocate_memory(image) failed: {e:?}"));
            }
        };
        unsafe {
            if let Err(e) = self.device.bind_image_memory(image, memory, 0) {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
                return Err(format!("bind_image_memory failed: {e:?}"));
            }
        }
        let view_type = if is_cube {
            vk::ImageViewType::CUBE
        } else {
            vk::ImageViewType::TYPE_2D
        };
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(view_type)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(layers.max(1)),
            );
        let view = match unsafe { self.device.create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(e) => {
                unsafe {
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                }
                return Err(format!("create_image_view(texture) failed: {e:?}"));
            }
        };

        Ok(VulkanTextureResource {
            image,
            memory,
            view,
            width: width.max(1),
            height: height.max(1),
            layers: layers.max(1),
            is_cube,
            format,
            layout: vk::ImageLayout::UNDEFINED,
            hardware_buffer: None,
            sampler: None,
            ycbcr_conversion: None,
            owns_image: true,
        })
    }

    fn vk_color_format_from_texture_pixel(pixel: TexturePixel) -> Option<vk::Format> {
        match pixel {
            TexturePixel::BGRAu8 => Some(vk::Format::B8G8R8A8_UNORM),
            TexturePixel::RGBAf16 => Some(vk::Format::R16G16B16A16_SFLOAT),
            TexturePixel::RGBAf32 => Some(vk::Format::R32G32B32A32_SFLOAT),
            _ => None,
        }
    }

    fn create_color_target_resource(
        &self,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Result<VulkanTextureResource, String> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { self.device.create_image(&image_info, None) }
            .map_err(|e| format!("create_image(render_target) failed: {e:?}"))?;
        let memory_req = unsafe { self.device.get_image_memory_requirements(image) };
        let memory_type_index = self
            .find_memory_type(
                memory_req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|_| {
                self.find_memory_type(
                    memory_req.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
            })?;
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(memory_req.size)
            .memory_type_index(memory_type_index);
        let memory = match unsafe { self.device.allocate_memory(&alloc_info, None) } {
            Ok(memory) => memory,
            Err(e) => {
                unsafe {
                    self.device.destroy_image(image, None);
                }
                return Err(format!("allocate_memory(render_target) failed: {e:?}"));
            }
        };
        unsafe {
            if let Err(e) = self.device.bind_image_memory(image, memory, 0) {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
                return Err(format!("bind_image_memory(render_target) failed: {e:?}"));
            }
        }
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        let view = match unsafe { self.device.create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(e) => {
                unsafe {
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                }
                return Err(format!("create_image_view(render_target) failed: {e:?}"));
            }
        };

        Ok(VulkanTextureResource {
            image,
            memory,
            view,
            width: width.max(1),
            height: height.max(1),
            layers: 1,
            is_cube: false,
            format,
            layout: vk::ImageLayout::UNDEFINED,
            hardware_buffer: None,
            sampler: None,
            ycbcr_conversion: None,
            owns_image: true,
        })
    }

    fn transition_image_layout(
        &self,
        image: vk::Image,
        aspect_mask: vk::ImageAspectFlags,
        layer_count: u32,
        old_layout: vk::ImageLayout,
        new_layout: vk::ImageLayout,
    ) {
        let (src_stage, src_access) = Self::layout_stage_access(old_layout);
        let (dst_stage, dst_access) = Self::layout_stage_access(new_layout);
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(old_layout)
            .new_layout(new_layout)
            .src_access_mask(src_access)
            .dst_access_mask(dst_access)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(aspect_mask)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(layer_count.max(1)),
            );
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                src_stage,
                dst_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }

    fn has_stencil_component(format: vk::Format) -> bool {
        matches!(
            format,
            vk::Format::D24_UNORM_S8_UINT | vk::Format::D32_SFLOAT_S8_UINT
        )
    }

    fn pick_depth_format(&self) -> Result<vk::Format, String> {
        let candidates = [
            vk::Format::D32_SFLOAT,
            vk::Format::D24_UNORM_S8_UINT,
            vk::Format::D16_UNORM,
        ];
        for format in candidates {
            let props = unsafe {
                self.instance
                    .get_physical_device_format_properties(self.physical_device, format)
            };
            if props
                .optimal_tiling_features
                .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
            {
                return Ok(format);
            }
        }
        Err("No supported Vulkan depth format found".to_string())
    }

    fn create_depth_target(
        &self,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Result<VulkanTextureResource, String> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        let image = unsafe { self.device.create_image(&image_info, None) }
            .map_err(|e| format!("create_image(depth) failed: {e:?}"))?;
        let memory_req = unsafe { self.device.get_image_memory_requirements(image) };
        let memory_type_index = self
            .find_memory_type(
                memory_req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|_| {
                self.find_memory_type(
                    memory_req.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
            })?;
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(memory_req.size)
            .memory_type_index(memory_type_index);
        let memory = match unsafe { self.device.allocate_memory(&alloc_info, None) } {
            Ok(memory) => memory,
            Err(e) => {
                unsafe {
                    self.device.destroy_image(image, None);
                }
                return Err(format!("allocate_memory(depth) failed: {e:?}"));
            }
        };
        unsafe {
            if let Err(e) = self.device.bind_image_memory(image, memory, 0) {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
                return Err(format!("bind_image_memory(depth) failed: {e:?}"));
            }
        }

        let mut aspect = vk::ImageAspectFlags::DEPTH;
        if Self::has_stencil_component(format) {
            aspect |= vk::ImageAspectFlags::STENCIL;
        }
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(aspect)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        let view = match unsafe { self.device.create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(e) => {
                unsafe {
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                }
                return Err(format!("create_image_view(depth) failed: {e:?}"));
            }
        };

        Ok(VulkanTextureResource {
            image,
            memory,
            view,
            width: width.max(1),
            height: height.max(1),
            layers: 1,
            is_cube: false,
            format,
            layout: vk::ImageLayout::UNDEFINED,
            hardware_buffer: None,
            sampler: None,
            ycbcr_conversion: None,
            owns_image: true,
        })
    }

    fn create_sampled_depth_resource(
        &self,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Result<VulkanTextureResource, String> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        let image = unsafe { self.device.create_image(&image_info, None) }
            .map_err(|e| format!("create_image(sampled_depth) failed: {e:?}"))?;
        let memory_req = unsafe { self.device.get_image_memory_requirements(image) };
        let memory_type_index = self
            .find_memory_type(
                memory_req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|_| {
                self.find_memory_type(
                    memory_req.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
            })?;
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(memory_req.size)
            .memory_type_index(memory_type_index);
        let memory = match unsafe { self.device.allocate_memory(&alloc_info, None) } {
            Ok(memory) => memory,
            Err(e) => {
                unsafe {
                    self.device.destroy_image(image, None);
                }
                return Err(format!("allocate_memory(sampled_depth) failed: {e:?}"));
            }
        };
        unsafe {
            if let Err(e) = self.device.bind_image_memory(image, memory, 0) {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
                return Err(format!("bind_image_memory(sampled_depth) failed: {e:?}"));
            }
        }

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        let view = match unsafe { self.device.create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(e) => {
                unsafe {
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                }
                return Err(format!("create_image_view(sampled_depth) failed: {e:?}"));
            }
        };

        Ok(VulkanTextureResource {
            image,
            memory,
            view,
            width: width.max(1),
            height: height.max(1),
            layers: 1,
            is_cube: false,
            format,
            layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
            hardware_buffer: None,
            sampler: None,
            ycbcr_conversion: None,
            owns_image: true,
        })
    }

    fn ensure_xr_depth_dummy(&mut self) -> Result<vk::ImageView, String> {
        if self.xr_depth_dummy.is_none() {
            self.xr_depth_dummy =
                Some(self.create_sampled_depth_resource(1, 1, vk::Format::D16_UNORM)?);
        }
        Ok(self.xr_depth_dummy.as_ref().unwrap().view)
    }

    fn destroy_texture_resource(&self, resource: VulkanTextureResource) {
        unsafe {
            if let Some(sampler) = resource.sampler {
                self.device.destroy_sampler(sampler, None);
            }
            if let Some(conversion) = resource.ycbcr_conversion {
                self.device
                    .destroy_sampler_ycbcr_conversion(conversion, None);
            }
            if resource.view != vk::ImageView::null() {
                self.device.destroy_image_view(resource.view, None);
            }
            if resource.owns_image && resource.image != vk::Image::null() {
                self.device.destroy_image(resource.image, None);
            }
            if resource.owns_image && resource.memory != vk::DeviceMemory::null() {
                self.device.free_memory(resource.memory, None);
            }
            if resource.owns_image {
                if let Some(hardware_buffer) = resource.hardware_buffer {
                    if !hardware_buffer.is_null() {
                        ndk_sys::AHardwareBuffer_release(hardware_buffer);
                    }
                }
            }
        }
    }

    fn create_imported_hardware_buffer_texture_resource(
        &mut self,
        hardware_buffer: *mut ndk_sys::AHardwareBuffer,
        width: u32,
        height: u32,
    ) -> Result<VulkanTextureResource, String> {
        if hardware_buffer.is_null() {
            return Err("Android Vulkan camera import failed: null AHardwareBuffer".to_string());
        }

        let (vk_format, external_format, allocation_size, android_memory_type_bits) = {
            let mut format_properties = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
            let (allocation_size, android_memory_type_bits) = {
                let mut properties = vk::AndroidHardwareBufferPropertiesANDROID::default()
                    .push_next(&mut format_properties);
                unsafe {
                    self.external_memory_android_hardware_buffer
                        .get_android_hardware_buffer_properties(
                            hardware_buffer.cast(),
                            &mut properties,
                        )
                        .map_err(|e| {
                            format!(
                                "Android Vulkan camera import failed: get_android_hardware_buffer_properties: {e:?}"
                            )
                        })?;
                }
                (properties.allocation_size, properties.memory_type_bits)
            };
            (
                format_properties.format,
                format_properties.external_format,
                allocation_size,
                android_memory_type_bits,
            )
        };
        if vk_format == vk::Format::UNDEFINED {
            return Err(format!(
                "Android Vulkan camera import failed: hardware buffer reported undefined Vulkan format (external_format={external_format})"
            ));
        }

        let mut external_memory = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
        let image_info = vk::ImageCreateInfo::default()
            .push_next(&mut external_memory)
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { self.device.create_image(&image_info, None) }
            .map_err(|e| format!("Android Vulkan camera import failed: create_image: {e:?}"))?;
        let memory_req = unsafe { self.device.get_image_memory_requirements(image) };
        let compatible_memory_bits = memory_req.memory_type_bits & android_memory_type_bits;
        let memory_type_index = self
            .find_memory_type(
                compatible_memory_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|_| {
                self.find_memory_type(compatible_memory_bits, vk::MemoryPropertyFlags::empty())
            })
            .map_err(|err| {
                unsafe {
                    self.device.destroy_image(image, None);
                }
                err
            })?;

        let mut import_info =
            vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(hardware_buffer.cast());
        let alloc_info = vk::MemoryAllocateInfo::default()
            .push_next(&mut import_info)
            .allocation_size(allocation_size.max(memory_req.size))
            .memory_type_index(memory_type_index);
        let memory = unsafe { self.device.allocate_memory(&alloc_info, None) }.map_err(|e| {
            unsafe {
                self.device.destroy_image(image, None);
            }
            format!("Android Vulkan camera import failed: allocate_memory: {e:?}")
        })?;

        if let Err(e) = unsafe { self.device.bind_image_memory(image, memory, 0) } {
            unsafe {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
            }
            return Err(format!(
                "Android Vulkan camera import failed: bind_image_memory: {e:?}"
            ));
        }

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk_format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        let view = match unsafe { self.device.create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(e) => {
                unsafe {
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                }
                return Err(format!(
                    "Android Vulkan camera import failed: create_image_view: {e:?}"
                ));
            }
        };

        unsafe {
            ndk_sys::AHardwareBuffer_acquire(hardware_buffer);
        }
        crate::warning!(
            "Android Vulkan camera import: size={}x{} vk_format={:?} external_format={} alloc_size={}",
            width.max(1),
            height.max(1),
            vk_format,
            external_format,
            allocation_size.max(memory_req.size),
        );

        let resource = VulkanTextureResource {
            image,
            memory,
            view,
            width: width.max(1),
            height: height.max(1),
            layers: 1,
            is_cube: false,
            format: vk_format,
            // Imported camera contents are produced externally; keeping the image in GENERAL
            // avoids discarding those contents via an UNDEFINED->SHADER_READ transition.
            layout: vk::ImageLayout::GENERAL,
            hardware_buffer: Some(hardware_buffer),
            sampler: None,
            ycbcr_conversion: None,
            owns_image: true,
        };
        Ok(resource)
    }

    fn create_imported_external_hardware_buffer_texture_resource(
        &mut self,
        hardware_buffer: *mut ndk_sys::AHardwareBuffer,
        width: u32,
        height: u32,
    ) -> Result<VulkanTextureResource, String> {
        if hardware_buffer.is_null() {
            return Err("Android Vulkan camera import failed: null AHardwareBuffer".to_string());
        }

        let (vk_format, external_format, allocation_size, android_memory_type_bits, format_props) = {
            let mut format_properties = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
            let (allocation_size, android_memory_type_bits) = {
                let mut properties = vk::AndroidHardwareBufferPropertiesANDROID::default()
                    .push_next(&mut format_properties);
                unsafe {
                    self.external_memory_android_hardware_buffer
                        .get_android_hardware_buffer_properties(
                            hardware_buffer.cast(),
                            &mut properties,
                        )
                        .map_err(|e| {
                            format!(
                                "Android Vulkan camera import failed: get_android_hardware_buffer_properties: {e:?}"
                            )
                        })?;
                }
                (properties.allocation_size, properties.memory_type_bits)
            };
            (
                format_properties.format,
                format_properties.external_format,
                allocation_size,
                android_memory_type_bits,
                format_properties,
            )
        };

        if external_format == 0 {
            if vk_format != vk::Format::UNDEFINED {
                return self.create_imported_hardware_buffer_texture_resource(
                    hardware_buffer,
                    width,
                    height,
                );
            }
            return Err(
                "Android Vulkan camera import failed: external-format camera buffer missing external format"
                    .to_string(),
            );
        }

        let mut external_memory = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
        let mut external_format_info =
            vk::ExternalFormatANDROID::default().external_format(external_format);
        let image_info = vk::ImageCreateInfo::default()
            .push_next(&mut external_memory)
            .push_next(&mut external_format_info)
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::UNDEFINED)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { self.device.create_image(&image_info, None) }.map_err(|e| {
            format!("Android Vulkan camera import failed: create_image(external): {e:?}")
        })?;
        let memory_req = unsafe { self.device.get_image_memory_requirements(image) };
        let compatible_memory_bits = memory_req.memory_type_bits & android_memory_type_bits;
        let memory_type_index = self
            .find_memory_type(
                compatible_memory_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|_| {
                self.find_memory_type(compatible_memory_bits, vk::MemoryPropertyFlags::empty())
            })
            .map_err(|err| {
                unsafe {
                    self.device.destroy_image(image, None);
                }
                err
            })?;

        let mut import_info =
            vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(hardware_buffer.cast());
        let alloc_info = vk::MemoryAllocateInfo::default()
            .push_next(&mut import_info)
            .allocation_size(allocation_size.max(memory_req.size))
            .memory_type_index(memory_type_index);
        let memory = unsafe { self.device.allocate_memory(&alloc_info, None) }.map_err(|e| {
            unsafe {
                self.device.destroy_image(image, None);
            }
            format!("Android Vulkan camera import failed: allocate_memory(external): {e:?}")
        })?;

        if let Err(e) = unsafe { self.device.bind_image_memory(image, memory, 0) } {
            unsafe {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
            }
            return Err(format!(
                "Android Vulkan camera import failed: bind_image_memory(external): {e:?}"
            ));
        }

        let mut conversion_external_format =
            vk::ExternalFormatANDROID::default().external_format(external_format);
        let conversion_info = vk::SamplerYcbcrConversionCreateInfo::default()
            .push_next(&mut conversion_external_format)
            .format(vk::Format::UNDEFINED)
            .ycbcr_model(format_props.suggested_ycbcr_model)
            .ycbcr_range(format_props.suggested_ycbcr_range)
            .components(format_props.sampler_ycbcr_conversion_components)
            .x_chroma_offset(format_props.suggested_x_chroma_offset)
            .y_chroma_offset(format_props.suggested_y_chroma_offset)
            .chroma_filter(vk::Filter::LINEAR)
            .force_explicit_reconstruction(false);
        let ycbcr_conversion = unsafe {
            self.device
                .create_sampler_ycbcr_conversion(&conversion_info, None)
        }
        .map_err(|e| {
            unsafe {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
            }
            format!("Android Vulkan camera import failed: create_sampler_ycbcr_conversion: {e:?}")
        })?;

        let mut view_conversion =
            vk::SamplerYcbcrConversionInfo::default().conversion(ycbcr_conversion);
        let view_info = vk::ImageViewCreateInfo::default()
            .push_next(&mut view_conversion)
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::UNDEFINED)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        let view = match unsafe { self.device.create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(e) => {
                unsafe {
                    self.device
                        .destroy_sampler_ycbcr_conversion(ycbcr_conversion, None);
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                }
                return Err(format!(
                    "Android Vulkan camera import failed: create_image_view(external): {e:?}"
                ));
            }
        };

        let mut sampler_conversion =
            vk::SamplerYcbcrConversionInfo::default().conversion(ycbcr_conversion);
        let sampler_info = vk::SamplerCreateInfo::default()
            .push_next(&mut sampler_conversion)
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .unnormalized_coordinates(false)
            .compare_enable(false)
            .min_lod(0.0)
            .max_lod(vk::LOD_CLAMP_NONE);
        let sampler = unsafe { self.device.create_sampler(&sampler_info, None) }.map_err(|e| {
            unsafe {
                self.device.destroy_image_view(view, None);
                self.device
                    .destroy_sampler_ycbcr_conversion(ycbcr_conversion, None);
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
            }
            format!("Android Vulkan camera import failed: create_sampler(external): {e:?}")
        })?;

        unsafe {
            ndk_sys::AHardwareBuffer_acquire(hardware_buffer);
        }

        Ok(VulkanTextureResource {
            image,
            memory,
            view,
            width: width.max(1),
            height: height.max(1),
            layers: 1,
            is_cube: false,
            format: vk::Format::UNDEFINED,
            layout: vk::ImageLayout::GENERAL,
            hardware_buffer: Some(hardware_buffer),
            sampler: Some(sampler),
            ycbcr_conversion: Some(ycbcr_conversion),
            owns_image: true,
        })
    }

    fn imported_yuv_plane_layout(vk_format: vk::Format) -> Option<ImportedYuvPlaneLayout> {
        match vk_format {
            vk::Format::G8_B8_R8_3PLANE_420_UNORM => Some(ImportedYuvPlaneLayout {
                biplanar: false,
                plane0_view_format: vk::Format::R8_UNORM,
                plane1_view_format: vk::Format::R8_UNORM,
                plane2_view_format: Some(vk::Format::R8_UNORM),
            }),
            vk::Format::G8_B8R8_2PLANE_420_UNORM => Some(ImportedYuvPlaneLayout {
                biplanar: true,
                plane0_view_format: vk::Format::R8_UNORM,
                plane1_view_format: vk::Format::R8G8_UNORM,
                plane2_view_format: None,
            }),
            _ => None,
        }
    }

    fn create_imported_hardware_buffer_plane_view(
        &self,
        image: vk::Image,
        view_format: vk::Format,
        aspect_mask: vk::ImageAspectFlags,
    ) -> Result<vk::ImageView, String> {
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(view_format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(aspect_mask)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        unsafe { self.device.create_image_view(&view_info, None) }
            .map_err(|e| format!("Android Vulkan camera import failed: create_plane_view: {e:?}"))
    }

    pub fn update_video_yuv_hardware_buffer_textures(
        &mut self,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        hardware_buffer: *mut ndk_sys::AHardwareBuffer,
        width: u32,
        height: u32,
    ) -> Result<crate::event::video_playback::VideoYuvMetadata, String> {
        if hardware_buffer.is_null() {
            return Err("Android Vulkan camera import failed: null AHardwareBuffer".to_string());
        }

        let tex_y_key = Self::texture_key(tex_y_id);
        let tex_u_key = Self::texture_key(tex_u_id);
        let tex_v_key = Self::texture_key(tex_v_id);

        let same_source = self
            .textures
            .get(&tex_y_key)
            .and_then(|resource| resource.hardware_buffer)
            == Some(hardware_buffer);
        if same_source {
            let biplanar = self
                .textures
                .get(&tex_u_key)
                .map(|resource| resource.format == vk::Format::R8G8_UNORM)
                .unwrap_or(false);
            return Ok(crate::event::video_playback::VideoYuvMetadata {
                enabled: true,
                matrix: 0.0,
                biplanar,
                rotation_steps: 0.0,
            });
        }

        let (vk_format, external_format, allocation_size, android_memory_type_bits) = {
            let mut format_properties = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
            let (allocation_size, android_memory_type_bits) = {
                let mut properties = vk::AndroidHardwareBufferPropertiesANDROID::default()
                    .push_next(&mut format_properties);
                unsafe {
                    self.external_memory_android_hardware_buffer
                        .get_android_hardware_buffer_properties(
                            hardware_buffer.cast(),
                            &mut properties,
                        )
                        .map_err(|e| {
                            format!(
                                "Android Vulkan camera import failed: get_android_hardware_buffer_properties: {e:?}"
                            )
                        })?;
                }
                (properties.allocation_size, properties.memory_type_bits)
            };
            (
                format_properties.format,
                format_properties.external_format,
                allocation_size,
                android_memory_type_bits,
            )
        };

        let plane_layout = Self::imported_yuv_plane_layout(vk_format).ok_or_else(|| {
            if vk_format == vk::Format::UNDEFINED {
                format!(
                    "Android Vulkan camera import failed: YUV hardware buffer reported undefined Vulkan format (external_format={external_format})"
                )
            } else {
                format!(
                    "Android Vulkan camera import failed: unsupported YUV Vulkan format {vk_format:?}"
                )
            }
        })?;

        let mut external_memory = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
        let image_info = vk::ImageCreateInfo::default()
            .push_next(&mut external_memory)
            .flags(vk::ImageCreateFlags::MUTABLE_FORMAT)
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { self.device.create_image(&image_info, None) }.map_err(|e| {
            format!("Android Vulkan camera import failed: create_image(yuv): {e:?}")
        })?;
        let memory_req = unsafe { self.device.get_image_memory_requirements(image) };
        let compatible_memory_bits = memory_req.memory_type_bits & android_memory_type_bits;
        let memory_type_index = self
            .find_memory_type(
                compatible_memory_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|_| {
                self.find_memory_type(compatible_memory_bits, vk::MemoryPropertyFlags::empty())
            })
            .map_err(|err| {
                unsafe {
                    self.device.destroy_image(image, None);
                }
                err
            })?;

        let mut import_info =
            vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(hardware_buffer.cast());
        let alloc_info = vk::MemoryAllocateInfo::default()
            .push_next(&mut import_info)
            .allocation_size(allocation_size.max(memory_req.size))
            .memory_type_index(memory_type_index);
        let memory = unsafe { self.device.allocate_memory(&alloc_info, None) }.map_err(|e| {
            unsafe {
                self.device.destroy_image(image, None);
            }
            format!("Android Vulkan camera import failed: allocate_memory(yuv): {e:?}")
        })?;

        if let Err(e) = unsafe { self.device.bind_image_memory(image, memory, 0) } {
            unsafe {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
            }
            return Err(format!(
                "Android Vulkan camera import failed: bind_image_memory(yuv): {e:?}"
            ));
        }

        let y_view = match self.create_imported_hardware_buffer_plane_view(
            image,
            plane_layout.plane0_view_format,
            vk::ImageAspectFlags::PLANE_0,
        ) {
            Ok(view) => view,
            Err(err) => {
                unsafe {
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                }
                return Err(err);
            }
        };
        let u_view = match self.create_imported_hardware_buffer_plane_view(
            image,
            plane_layout.plane1_view_format,
            vk::ImageAspectFlags::PLANE_1,
        ) {
            Ok(view) => view,
            Err(err) => {
                unsafe {
                    self.device.destroy_image_view(y_view, None);
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                }
                return Err(err);
            }
        };
        let v_view = match plane_layout.plane2_view_format {
            Some(view_format) => match self.create_imported_hardware_buffer_plane_view(
                image,
                view_format,
                vk::ImageAspectFlags::PLANE_2,
            ) {
                Ok(view) => view,
                Err(err) => {
                    unsafe {
                        self.device.destroy_image_view(u_view, None);
                        self.device.destroy_image_view(y_view, None);
                        self.device.free_memory(memory, None);
                        self.device.destroy_image(image, None);
                    }
                    return Err(err);
                }
            },
            None => match self.create_imported_hardware_buffer_plane_view(
                image,
                plane_layout.plane1_view_format,
                vk::ImageAspectFlags::PLANE_1,
            ) {
                Ok(view) => view,
                Err(err) => {
                    unsafe {
                        self.device.destroy_image_view(u_view, None);
                        self.device.destroy_image_view(y_view, None);
                        self.device.free_memory(memory, None);
                        self.device.destroy_image(image, None);
                    }
                    return Err(err);
                }
            },
        };

        unsafe {
            ndk_sys::AHardwareBuffer_acquire(hardware_buffer);
        }
        crate::warning!(
            "Android Vulkan camera import: YUV size={}x{} vk_format={:?} external_format={} biplanar={}",
            width.max(1),
            height.max(1),
            vk_format,
            external_format,
            plane_layout.biplanar,
        );

        let chroma_width = width.div_ceil(2).max(1);
        let chroma_height = height.div_ceil(2).max(1);
        let y_resource = VulkanTextureResource {
            image,
            memory,
            view: y_view,
            width: width.max(1),
            height: height.max(1),
            layers: 1,
            is_cube: false,
            format: plane_layout.plane0_view_format,
            layout: vk::ImageLayout::GENERAL,
            hardware_buffer: Some(hardware_buffer),
            sampler: None,
            ycbcr_conversion: None,
            owns_image: true,
        };
        let u_resource = VulkanTextureResource {
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            view: u_view,
            width: chroma_width,
            height: chroma_height,
            layers: 1,
            is_cube: false,
            format: plane_layout.plane1_view_format,
            layout: vk::ImageLayout::GENERAL,
            hardware_buffer: None,
            sampler: None,
            ycbcr_conversion: None,
            owns_image: false,
        };
        let v_resource = VulkanTextureResource {
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            view: v_view,
            width: chroma_width,
            height: chroma_height,
            layers: 1,
            is_cube: false,
            format: plane_layout
                .plane2_view_format
                .unwrap_or(plane_layout.plane1_view_format),
            layout: vk::ImageLayout::GENERAL,
            hardware_buffer: None,
            sampler: None,
            ycbcr_conversion: None,
            owns_image: false,
        };

        if let Some(old_resource) = self.textures.remove(&tex_v_key) {
            self.destroy_texture_resource(old_resource);
        }
        if let Some(old_resource) = self.textures.remove(&tex_u_key) {
            self.destroy_texture_resource(old_resource);
        }
        if let Some(old_resource) = self.textures.remove(&tex_y_key) {
            self.destroy_texture_resource(old_resource);
        }
        self.textures.insert(tex_y_key, y_resource);
        self.textures.insert(tex_u_key, u_resource);
        self.textures.insert(tex_v_key, v_resource);

        Ok(crate::event::video_playback::VideoYuvMetadata {
            enabled: true,
            matrix: 0.0,
            biplanar: plane_layout.biplanar,
            rotation_steps: 0.0,
        })
    }

    pub fn update_video_external_hardware_buffer_texture(
        &mut self,
        texture_id: TextureId,
        hardware_buffer: *mut ndk_sys::AHardwareBuffer,
        width: u32,
        height: u32,
    ) -> Result<crate::event::video_playback::VideoYuvMetadata, String> {
        let texture_key = Self::texture_key(texture_id);
        let same_source = self
            .textures
            .get(&texture_key)
            .and_then(|resource| resource.hardware_buffer)
            == Some(hardware_buffer);
        if !same_source {
            if let Some(old_resource) = self.textures.remove(&texture_key) {
                self.destroy_texture_resource(old_resource);
            }
            let resource = self.create_imported_external_hardware_buffer_texture_resource(
                hardware_buffer,
                width,
                height,
            )?;
            self.textures.insert(texture_key, resource);
        }

        Ok(crate::event::video_playback::VideoYuvMetadata::disabled())
    }

    pub fn update_video_rgba_hardware_buffer_texture(
        &mut self,
        texture_id: TextureId,
        hardware_buffer: *mut ndk_sys::AHardwareBuffer,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let texture_key = Self::texture_key(texture_id);
        let same_source = self
            .textures
            .get(&texture_key)
            .and_then(|resource| resource.hardware_buffer)
            == Some(hardware_buffer);
        if same_source {
            return Ok(());
        }

        if let Some(old_resource) = self.textures.remove(&texture_key) {
            self.destroy_texture_resource(old_resource);
        }
        let resource =
            self.create_imported_hardware_buffer_texture_resource(hardware_buffer, width, height)?;
        self.textures.insert(texture_key, resource);
        Ok(())
    }

    fn layout_stage_access(layout: vk::ImageLayout) -> (vk::PipelineStageFlags, vk::AccessFlags) {
        match layout {
            vk::ImageLayout::UNDEFINED => (
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::AccessFlags::empty(),
            ),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL => (
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::TRANSFER_WRITE,
            ),
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => (
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::VERTEX_SHADER,
                vk::AccessFlags::SHADER_READ,
            ),
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL => (
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            ),
            vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL => (
                vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            ),
            vk::ImageLayout::GENERAL => (
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
            ),
            _ => (
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
            ),
        }
    }

    fn texture_key(texture_id: TextureId) -> VulkanTextureKey {
        texture_id.0
    }

    fn ensure_texture_uploaded(
        &mut self,
        cx: &mut Cx,
        texture_id: TextureId,
    ) -> Result<(), String> {
        let texture_key = Self::texture_key(texture_id);
        let (alloc_changed, updated, width, height, layers, is_cube, format) = {
            let cxtexture = &mut cx.textures[texture_id];
            if !cxtexture.format.is_vec() {
                return Ok(());
            }
            let alloc_changed = cxtexture.alloc_vec();
            let updated = cxtexture.take_updated();
            let (width, height, layers, is_cube, format) =
                Self::vec_texture_meta(&cxtexture.format).ok_or_else(|| {
                    format!("unsupported Vulkan texture format: {:?}", cxtexture.format)
                })?;
            (
                alloc_changed,
                updated,
                width,
                height,
                layers,
                is_cube,
                format,
            )
        };

        let needs_recreate = match self.textures.get(&texture_key) {
            Some(resource) => {
                alloc_changed
                    || resource.width != width.max(1)
                    || resource.height != height.max(1)
                    || resource.layers != layers.max(1)
                    || resource.is_cube != is_cube
                    || resource.format != format
            }
            None => true,
        };

        if needs_recreate {
            if let Some(old_resource) = self.textures.remove(&texture_key) {
                self.destroy_texture_resource(old_resource);
            }
            let resource = self.create_texture_resource(width, height, layers, is_cube, format)?;
            self.textures.insert(texture_key, resource);
        }

        if matches!(updated, TextureUpdated::Empty) && !needs_recreate {
            return Ok(());
        }

        let force_full_upload = needs_recreate && !matches!(updated, TextureUpdated::Partial(_));
        let clear_before_partial_upload =
            needs_recreate && matches!(updated, TextureUpdated::Partial(_));
        let upload = {
            let cxtexture = &cx.textures[texture_id];
            Self::vec_texture_upload(&cxtexture.format, updated, force_full_upload)
                .ok_or_else(|| format!("texture {} has unsupported upload format", texture_key))?
        };
        if upload.data.is_empty() || upload.width == 0 || upload.height == 0 {
            return Ok(());
        }
        let layer_count = upload.layers.max(1);
        self.texture_upload_count_this_frame += 1;
        self.texture_upload_bytes_this_frame += upload.data.len() as u64;

        let staging =
            self.create_host_buffer_with_data(vk::BufferUsageFlags::TRANSFER_SRC, &upload.data)?;
        self.frame_resources.buffers.push(staging);

        let (image, old_layout) = {
            let texture = self
                .textures
                .get(&texture_key)
                .ok_or_else(|| format!("missing Vulkan texture resource for {}", texture_key))?;
            (texture.image, texture.layout)
        };
        let (src_stage, src_access) = Self::layout_stage_access(old_layout);

        let to_transfer = vk::ImageMemoryBarrier::default()
            .src_access_mask(src_access)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .old_layout(old_layout)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(layer_count),
            );
        let copy_region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(layer_count),
            )
            .image_offset(vk::Offset3D {
                x: upload.offset_x as i32,
                y: upload.offset_y as i32,
                z: 0,
            })
            .image_extent(vk::Extent3D {
                width: upload.width,
                height: upload.height,
                depth: 1,
            });
        let to_shader = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(layer_count),
            );
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                src_stage,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            if clear_before_partial_upload {
                let clear_value = vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                };
                let clear_range = vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(layer_count);
                self.device.cmd_clear_color_image(
                    self.command_buffer,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &clear_value,
                    &[clear_range],
                );
            }
            self.device.cmd_copy_buffer_to_image(
                self.command_buffer,
                staging.buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy_region],
            );
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::VERTEX_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_shader],
            );
        }
        if let Some(texture) = self.textures.get_mut(&texture_key) {
            texture.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        }

        Ok(())
    }

    fn record_draw_list(
        &mut self,
        cx: &mut Cx,
        draw_pass_id: DrawPassId,
        draw_list_id: DrawListId,
        render_pass_key: &VulkanRenderPassKey,
        zbias: &mut f32,
        zbias_step: f32,
        draw_stats: &mut VulkanDrawStats,
        xr_depth_view: vk::ImageView,
    ) -> Result<(), String> {
        let draw_order_len = cx.draw_lists[draw_list_id].draw_item_order_len();
        for order_index in 0..draw_order_len {
            let Some(draw_item_id) =
                cx.draw_lists[draw_list_id].draw_item_id_at_order_index(order_index)
            else {
                continue;
            };
            let null_texture_id = cx.null_texture.texture_id();
            let null_cube_texture_id = cx.null_cube_texture.texture_id();
            draw_stats.draw_items += 1;
            if let Some(sub_list_id) = cx.draw_lists[draw_list_id].draw_items[draw_item_id]
                .kind
                .sub_list()
            {
                self.record_draw_list(
                    cx,
                    draw_pass_id,
                    sub_list_id,
                    render_pass_key,
                    zbias,
                    zbias_step,
                    draw_stats,
                    xr_depth_view,
                )?;
                continue;
            }

            let packet = {
                let draw_list = &mut cx.draw_lists[draw_list_id];
                let draw_item = &mut draw_list.draw_items[draw_item_id];
                let draw_call = if let Some(draw_call) = draw_item.kind.draw_call_mut() {
                    draw_stats.draw_calls += 1;
                    draw_call
                } else {
                    draw_stats.skipped_non_draw_call += 1;
                    continue;
                };

                let sh = &cx.draw_shaders.shaders[draw_call.draw_shader_id.index];
                let os_shader_id = if let Some(id) = sh.os_shader_id {
                    id
                } else {
                    draw_stats.skipped_no_os_shader += 1;
                    continue;
                };
                let os_shader = &cx.draw_shaders.os_shaders[os_shader_id];
                let vk_shader = if let Some(vk) = &os_shader.vulkan_shader {
                    vk
                } else {
                    draw_stats.skipped_no_vulkan_shader += 1;
                    continue;
                };
                if vk_shader.vertex_spirv.is_none() || vk_shader.fragment_spirv.is_none() {
                    draw_stats.skipped_missing_spirv += 1;
                    continue;
                }
                if sh.mapping.instances.total_slots == 0 {
                    draw_stats.skipped_no_instance_slots += 1;
                    continue;
                }
                let instances = if let Some(instances) = draw_item.instances.as_ref() {
                    instances.clone()
                } else {
                    draw_stats.skipped_no_instances_buffer += 1;
                    continue;
                };
                if instances.len() < sh.mapping.instances.total_slots {
                    draw_stats.skipped_instances_too_short += 1;
                    continue;
                }
                let instance_count = instances.len() / sh.mapping.instances.total_slots;
                if instance_count == 0 {
                    draw_stats.skipped_zero_instances += 1;
                    continue;
                }
                let geometry_id = if let Some(geometry_id) = draw_call.geometry_id {
                    geometry_id
                } else {
                    draw_stats.skipped_no_geometry_id += 1;
                    continue;
                };

                if sh.mapping.uses_time {
                    cx.demo_time_repaint = true;
                }

                draw_call.draw_call_uniforms.set_zbias(*zbias);
                *zbias += zbias_step;
                draw_call.instance_dirty = false;
                draw_call.uniforms_dirty = false;
                let texture_ids = (0..sh.mapping.textures.len())
                    .map(|i| {
                        draw_call.texture_slots[i]
                            .as_ref()
                            .map(|texture| texture.texture_id())
                            .unwrap_or_else(|| {
                                if matches!(
                                    sh.mapping.textures[i].tex_type,
                                    TextureType::TextureCube | TextureType::TextureCubeArray
                                ) {
                                    null_cube_texture_id
                                } else {
                                    null_texture_id
                                }
                            })
                    })
                    .collect();
                let texture_types = sh.mapping.textures.iter().map(|t| t.tex_type).collect();

                VulkanDrawPacket {
                    shader_index: draw_call.draw_shader_id.index,
                    geometry_id,
                    depth_write: draw_call.options.depth_write,
                    backface_culling: draw_call.options.backface_culling,
                    instances,
                    draw_call_uniforms: draw_call.draw_call_uniforms.as_slice().to_vec(),
                    dyn_uniforms: draw_call.dyn_uniforms[..sh
                        .mapping
                        .dyn_uniforms
                        .total_slots
                        .min(draw_call.dyn_uniforms.len())]
                        .to_vec(),
                    scope_uniforms: sh.mapping.scope_uniforms_buf.clone(),
                    uniform_bindings: sh.mapping.uniform_buffer_bindings.bindings.clone(),
                    dyn_uniform_binding: vk_shader.dyn_uniform_binding,
                    scope_uniform_binding: sh
                        .mapping
                        .uniform_buffer_bindings
                        .scope_uniform_buffer_index,
                    texture_ids,
                    texture_types,
                }
            };

            let geometry = &mut cx.geometries[packet.geometry_id];
            if geometry.indices.is_empty() || geometry.vertices.is_empty() {
                draw_stats.skipped_empty_geometry += 1;
                continue;
            }
            self.ensure_geometry_resource(packet.geometry_id, geometry)?;
            let geometry_resource = self
                .geometries
                .get(&packet.geometry_id)
                .copied()
                .ok_or_else(|| {
                    format!(
                        "missing Vulkan geometry resource for {:?}",
                        packet.geometry_id
                    )
                })?;
            let index_count = geometry.indices.len() as u32;
            let pass_uniforms = cx.passes[draw_pass_id].pass_uniforms.as_slice().to_vec();
            let draw_list_uniforms = cx.draw_lists[draw_list_id]
                .draw_list_uniforms
                .as_slice()
                .to_vec();

            self.record_draw_packet(
                cx,
                &packet,
                render_pass_key,
                geometry_resource,
                index_count,
                &pass_uniforms,
                &draw_list_uniforms,
                xr_depth_view,
            )?;
            draw_stats.packets_recorded += 1;
        }
        Ok(())
    }

    fn record_draw_packet(
        &mut self,
        cx: &Cx,
        packet: &VulkanDrawPacket,
        render_pass_key: &VulkanRenderPassKey,
        geometry_resource: VulkanGeometryResource,
        index_count: u32,
        pass_uniforms: &[f32],
        draw_list_uniforms: &[f32],
        xr_depth_view: vk::ImageView,
    ) -> Result<(), String> {
        self.ensure_pipeline(
            cx,
            packet.shader_index,
            render_pass_key,
            packet.backface_culling,
        )?;
        let (
            pipeline_handle,
            pipeline_layout,
            descriptor_set_layout,
            pipeline_has_descriptors,
            pipeline_samplers,
        ) = {
            let pipeline_key = VulkanPipelineKey {
                shader_index: packet.shader_index,
                render_pass: render_pass_key.clone(),
                backface_culling: packet.backface_culling,
            };
            let pipeline = self.pipelines.get(&pipeline_key).ok_or_else(|| {
                format!("missing Vulkan pipeline for shader {}", packet.shader_index)
            })?;
            (
                if packet.depth_write {
                    pipeline.pipeline_write
                } else {
                    pipeline.pipeline_no_write
                },
                pipeline.layout,
                pipeline.descriptor_set_layout,
                pipeline.has_descriptors,
                pipeline.sampler_handles.clone(),
            )
        };

        let sh = &cx.draw_shaders.shaders[packet.shader_index];
        let os_shader_id = sh
            .os_shader_id
            .ok_or_else(|| format!("shader {} missing os_shader_id", packet.shader_index))?;
        let os_shader = &cx.draw_shaders.os_shaders[os_shader_id];
        let vk_shader = os_shader
            .vulkan_shader
            .as_ref()
            .ok_or_else(|| format!("shader {} missing Vulkan binary", packet.shader_index))?;
        let geometry_stride =
            (sh.mapping.geometries.total_slots * std::mem::size_of::<f32>()) as u64;
        let instance_stride =
            (sh.mapping.instances.total_slots * std::mem::size_of::<f32>()) as u64;
        if geometry_stride == 0 || instance_stride == 0 {
            return Ok(());
        }
        let instance_count = (packet.instances.len() as u64
            / (instance_stride / std::mem::size_of::<f32>() as u64))
            as u32;
        if instance_count == 0 || index_count == 0 {
            return Ok(());
        }

        struct UniformUpload<'a> {
            binding: u32,
            src: &'a [f32],
            offset: vk::DeviceSize,
            size: vk::DeviceSize,
        }

        let mut uniform_uploads: Vec<UniformUpload<'_>> = Vec::new();
        for (type_name, binding_idx) in &packet.uniform_bindings {
            let src: &[f32] = if *type_name == id!(DrawPassUniforms) {
                pass_uniforms
            } else if *type_name == id!(DrawListUniforms) {
                draw_list_uniforms
            } else if *type_name == id!(DrawCallUniforms) {
                packet.draw_call_uniforms.as_slice()
            } else {
                &[]
            };
            if src.is_empty() {
                continue;
            }
            uniform_uploads.push(UniformUpload {
                binding: *binding_idx as u32,
                src,
                offset: 0,
                size: 0,
            });
        }
        if !packet.dyn_uniforms.is_empty() {
            uniform_uploads.push(UniformUpload {
                binding: packet.dyn_uniform_binding,
                src: packet.dyn_uniforms.as_slice(),
                offset: 0,
                size: 0,
            });
        }
        if let Some(scope_binding) = packet.scope_uniform_binding {
            if !packet.scope_uniforms.is_empty() {
                uniform_uploads.push(UniformUpload {
                    binding: scope_binding as u32,
                    src: packet.scope_uniforms.as_slice(),
                    offset: 0,
                    size: 0,
                });
            }
        }
        uniform_uploads.sort_by_key(|uniform| uniform.binding);
        uniform_uploads.dedup_by_key(|uniform| uniform.binding);

        let mut cursor: vk::DeviceSize = 0;
        let instances_offset = Self::align_device_size(cursor, 4);
        let instances_bytes = std::mem::size_of_val(packet.instances.as_slice()) as vk::DeviceSize;
        cursor = instances_offset + instances_bytes;

        let uniform_alignment = self.min_uniform_buffer_offset_alignment.max(4);
        for uniform in &mut uniform_uploads {
            let size = std::mem::size_of_val(uniform.src) as vk::DeviceSize;
            if size == 0 {
                continue;
            }
            let offset = Self::align_device_size(cursor, uniform_alignment);
            cursor = offset + size;
            uniform.offset = offset;
            uniform.size = size;
        }
        uniform_uploads.retain(|uniform| uniform.size != 0);

        let packet_buffer_usage =
            vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::UNIFORM_BUFFER;
        let packet_buffer = self.create_host_buffer(packet_buffer_usage, cursor.max(4))?;
        unsafe {
            let mapped = self
                .device
                .map_memory(
                    packet_buffer.memory,
                    0,
                    packet_buffer.size,
                    vk::MemoryMapFlags::empty(),
                )
                .map_err(|e| format!("map_memory(packet_buffer) failed: {e:?}"))?;
            let mapped_ptr = mapped as *mut u8;
            if instances_bytes != 0 {
                std::ptr::copy_nonoverlapping(
                    packet.instances.as_ptr() as *const u8,
                    mapped_ptr.add(instances_offset as usize),
                    instances_bytes as usize,
                );
            }
            for uniform in &uniform_uploads {
                std::ptr::copy_nonoverlapping(
                    uniform.src.as_ptr() as *const u8,
                    mapped_ptr.add(uniform.offset as usize),
                    uniform.size as usize,
                );
            }
            self.device.unmap_memory(packet_buffer.memory);
        }
        self.frame_resources.buffers.push(packet_buffer);

        let mut texture_bindings = Vec::new();
        let mut texture_descriptor_types = Vec::new();
        let mut texture_infos = Vec::new();
        let mut video_sampler_overrides = std::collections::HashMap::<usize, vk::Sampler>::new();
        let null_texture_key = Self::texture_key(cx.null_texture.texture_id());
        let null_cube_texture_key = Self::texture_key(cx.null_cube_texture.texture_id());
        let null_texture_resource = self.textures.get(&null_texture_key);
        let null_cube_texture_resource = self.textures.get(&null_cube_texture_key);
        for (slot, texture_id) in packet.texture_ids.iter().enumerate() {
            let expected_cube = packet
                .texture_types
                .get(slot)
                .map(|tex_type| {
                    matches!(
                        tex_type,
                        TextureType::TextureCube | TextureType::TextureCubeArray
                    )
                })
                .unwrap_or(false);
            let fallback = if expected_cube {
                null_cube_texture_resource
            } else {
                null_texture_resource
            };
            let resource = self
                .textures
                .get(&Self::texture_key(*texture_id))
                .or(fallback);
            let Some(resource) = resource else {
                return Ok(());
            };
            let sampler_index = sh
                .mapping
                .texture_sampler_indices
                .get(slot)
                .copied()
                .unwrap_or(0);
            texture_bindings.push(vk_shader.texture_binding_base + slot as u32);
            let descriptor_type = vk::DescriptorType::SAMPLED_IMAGE;
            texture_descriptor_types.push(descriptor_type);

            let image_info = vk::DescriptorImageInfo::default()
                .image_view(resource.view)
                .image_layout(resource.layout);
            if let Some(video_sampler) = resource.sampler {
                video_sampler_overrides.insert(sampler_index, video_sampler);
            }
            texture_infos.push(image_info);
        }

        let mut sampler_bindings = Vec::new();
        let mut sampler_infos = Vec::new();
        for (sampler_index, sampler) in pipeline_samplers.iter().enumerate() {
            sampler_bindings.push(vk_shader.sampler_binding_base + sampler_index as u32);
            let sampler = video_sampler_overrides
                .get(&sampler_index)
                .copied()
                .unwrap_or(*sampler);
            sampler_infos.push(vk::DescriptorImageInfo::default().sampler(sampler));
        }

        let xr_depth_info = vk::DescriptorImageInfo::default()
            .image_view(xr_depth_view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);

        let descriptor_set = if pipeline_has_descriptors {
            if uniform_uploads.is_empty() && texture_infos.is_empty() && sampler_infos.is_empty() {
                return Err(format!(
                    "shader {} expects descriptors but no descriptor payloads were built",
                    packet.shader_index
                ));
            }

            let descriptor_set = self.alloc_frame_descriptor_set(descriptor_set_layout)?;

            let mut buffer_infos = Vec::with_capacity(uniform_uploads.len());
            for uniform in &uniform_uploads {
                buffer_infos.push(
                    vk::DescriptorBufferInfo::default()
                        .buffer(packet_buffer.buffer)
                        .offset(uniform.offset)
                        .range(uniform.size),
                );
            }

            let mut writes = Vec::with_capacity(
                uniform_uploads.len() + texture_infos.len() + sampler_infos.len(),
            );
            for (index, uniform) in uniform_uploads.iter().enumerate() {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(uniform.binding)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(std::slice::from_ref(&buffer_infos[index])),
                );
            }
            for (index, binding) in texture_bindings.iter().enumerate() {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(*binding)
                        .descriptor_type(texture_descriptor_types[index])
                        .image_info(std::slice::from_ref(&texture_infos[index])),
                );
            }
            for (index, binding) in sampler_bindings.iter().enumerate() {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(*binding)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(std::slice::from_ref(&sampler_infos[index])),
                );
            }
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(descriptor_set)
                    .dst_binding(vk_shader.xr_depth_binding)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(std::slice::from_ref(&xr_depth_info)),
            );
            unsafe {
                self.device.update_descriptor_sets(&writes, &[]);
            }
            Some(descriptor_set)
        } else {
            None
        };
        let vertex_buffers = [geometry_resource.vertex_buffer.buffer, packet_buffer.buffer];
        let vertex_offsets = [0, instances_offset];

        unsafe {
            self.device.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline_handle,
            );
            if let Some(set) = descriptor_set {
                self.device.cmd_bind_descriptor_sets(
                    self.command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipeline_layout,
                    0,
                    &[set],
                    &[],
                );
            }
            self.device.cmd_bind_vertex_buffers(
                self.command_buffer,
                0,
                &vertex_buffers,
                &vertex_offsets,
            );
            self.device.cmd_bind_index_buffer(
                self.command_buffer,
                geometry_resource.index_buffer.buffer,
                0,
                vk::IndexType::UINT32,
            );
            self.device
                .cmd_draw_indexed(self.command_buffer, index_count, instance_count, 0, 0, 0);
        }

        Ok(())
    }

    fn ensure_pipeline(
        &mut self,
        cx: &Cx,
        shader_index: usize,
        render_pass_key: &VulkanRenderPassKey,
        backface_culling: bool,
    ) -> Result<(), String> {
        let pipeline_key = VulkanPipelineKey {
            shader_index,
            render_pass: render_pass_key.clone(),
            backface_culling,
        };
        if self.pipelines.contains_key(&pipeline_key) {
            return Ok(());
        }

        let sh = &cx.draw_shaders.shaders[shader_index];
        let os_shader_id = sh
            .os_shader_id
            .ok_or_else(|| format!("shader {} missing os_shader_id", shader_index))?;
        let os_shader = &cx.draw_shaders.os_shaders[os_shader_id];
        let vk_shader = os_shader
            .vulkan_shader
            .as_ref()
            .ok_or_else(|| format!("shader {} missing Vulkan binary", shader_index))?;
        let vs_spv = vk_shader
            .vertex_spirv
            .as_ref()
            .ok_or_else(|| format!("shader {} missing vertex SPIR-V", shader_index))?;
        let fs_spv = vk_shader
            .fragment_spirv
            .as_ref()
            .ok_or_else(|| format!("shader {} missing fragment SPIR-V", shader_index))?;

        if vk_shader.geometry_slots != sh.mapping.geometries.total_slots
            || vk_shader.instance_slots != sh.mapping.instances.total_slots
        {
            crate::warning!(
                "Android Vulkan slot mismatch: shader={}, wgsl_geom_slots={}, map_geom_slots={}, wgsl_inst_slots={}, map_inst_slots={}",
                shader_index,
                vk_shader.geometry_slots,
                sh.mapping.geometries.total_slots,
                vk_shader.instance_slots,
                sh.mapping.instances.total_slots
            );
        }

        let has_descriptors = !sh.mapping.uniform_buffer_bindings.bindings.is_empty()
            || !sh.mapping.dyn_uniforms.inputs.is_empty()
            || !sh.mapping.scope_uniforms.inputs.is_empty()
            || !sh.mapping.textures.is_empty()
            || !sh.mapping.samplers.is_empty()
            || vk_shader.xr_depth_binding != 0;

        let mut descriptor_bindings: Vec<(u32, vk::DescriptorType)> = Vec::new();
        for (_, idx) in &sh.mapping.uniform_buffer_bindings.bindings {
            descriptor_bindings.push((*idx as u32, vk::DescriptorType::UNIFORM_BUFFER));
        }
        if !sh.mapping.dyn_uniforms.inputs.is_empty() {
            descriptor_bindings.push((
                vk_shader.dyn_uniform_binding,
                vk::DescriptorType::UNIFORM_BUFFER,
            ));
        }
        if !sh.mapping.scope_uniforms.inputs.is_empty() {
            if let Some(idx) = sh
                .mapping
                .uniform_buffer_bindings
                .scope_uniform_buffer_index
            {
                descriptor_bindings.push((idx as u32, vk::DescriptorType::UNIFORM_BUFFER));
            }
        }
        for (slot, _) in sh.mapping.textures.iter().enumerate() {
            descriptor_bindings.push((
                vk_shader.texture_binding_base + slot as u32,
                vk::DescriptorType::SAMPLED_IMAGE,
            ));
        }
        for sampler_index in 0..sh.mapping.samplers.len() {
            descriptor_bindings.push((
                vk_shader.sampler_binding_base + sampler_index as u32,
                vk::DescriptorType::SAMPLER,
            ));
        }
        descriptor_bindings.push((
            vk_shader.xr_depth_binding,
            vk::DescriptorType::SAMPLED_IMAGE,
        ));
        descriptor_bindings.sort_by_key(|(binding, _)| *binding);
        descriptor_bindings.dedup_by_key(|(binding, _)| *binding);

        let descriptor_set_layout = {
            let mut dsl_bindings = Vec::new();
            for (binding, descriptor_type) in &descriptor_bindings {
                dsl_bindings.push(
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(*binding)
                        .descriptor_count(1)
                        .descriptor_type(*descriptor_type)
                        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
                );
            }
            let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&dsl_bindings);
            unsafe { self.device.create_descriptor_set_layout(&info, None) }
                .map_err(|e| format!("create_descriptor_set_layout failed: {e:?}"))?
        };

        let set_layouts = [descriptor_set_layout];
        let pipeline_layout_info =
            vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
        let pipeline_layout = match unsafe {
            self.device
                .create_pipeline_layout(&pipeline_layout_info, None)
        } {
            Ok(pipeline_layout) => pipeline_layout,
            Err(e) => {
                unsafe {
                    self.device
                        .destroy_descriptor_set_layout(descriptor_set_layout, None);
                }
                return Err(format!("create_pipeline_layout failed: {e:?}"));
            }
        };

        let vs_module_info = vk::ShaderModuleCreateInfo::default().code(vs_spv);
        let fs_module_info = vk::ShaderModuleCreateInfo::default().code(fs_spv);
        let vs_module = match unsafe { self.device.create_shader_module(&vs_module_info, None) } {
            Ok(vs_module) => vs_module,
            Err(e) => {
                unsafe {
                    self.device.destroy_pipeline_layout(pipeline_layout, None);
                    self.device
                        .destroy_descriptor_set_layout(descriptor_set_layout, None);
                }
                return Err(format!("create_shader_module(vertex) failed: {e:?}"));
            }
        };
        let fs_module = match unsafe { self.device.create_shader_module(&fs_module_info, None) } {
            Ok(fs_module) => fs_module,
            Err(e) => {
                unsafe {
                    self.device.destroy_shader_module(vs_module, None);
                    self.device.destroy_pipeline_layout(pipeline_layout, None);
                    self.device
                        .destroy_descriptor_set_layout(descriptor_set_layout, None);
                }
                return Err(format!("create_shader_module(fragment) failed: {e:?}"));
            }
        };

        let vs_entry = std::ffi::CString::new("vertex_main").unwrap();
        let fs_entry = std::ffi::CString::new("fragment_main").unwrap();
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs_module)
                .name(&vs_entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs_module)
                .name(&fs_entry),
        ];

        let geometry_formats =
            Self::collect_attribute_chunk_formats(sh.mapping.geometries.total_slots);
        let instance_formats =
            Self::collect_attribute_chunk_formats(sh.mapping.instances.total_slots);

        let mut vertex_bindings = Vec::new();
        vertex_bindings.push(
            vk::VertexInputBindingDescription::default()
                .binding(0)
                .stride((sh.mapping.geometries.total_slots * std::mem::size_of::<f32>()) as u32)
                .input_rate(vk::VertexInputRate::VERTEX),
        );
        vertex_bindings.push(
            vk::VertexInputBindingDescription::default()
                .binding(1)
                .stride((sh.mapping.instances.total_slots * std::mem::size_of::<f32>()) as u32)
                .input_rate(vk::VertexInputRate::INSTANCE),
        );

        let mut vertex_attributes = Vec::new();
        let mut location = 0u32;
        for (chunk_index, format) in geometry_formats.iter().enumerate() {
            let remaining = sh
                .mapping
                .geometries
                .total_slots
                .saturating_sub(chunk_index * 4);
            let components = remaining.min(4);
            vertex_attributes.push(
                vk::VertexInputAttributeDescription::default()
                    .location(location)
                    .binding(0)
                    .format(Self::vk_vertex_format(*format, components))
                    .offset((chunk_index * 4 * std::mem::size_of::<f32>()) as u32),
            );
            location += 1;
        }
        for (chunk_index, format) in instance_formats.iter().enumerate() {
            let remaining = sh
                .mapping
                .instances
                .total_slots
                .saturating_sub(chunk_index * 4);
            let components = remaining.min(4);
            vertex_attributes.push(
                vk::VertexInputAttributeDescription::default()
                    .location(location)
                    .binding(1)
                    .format(Self::vk_vertex_format(*format, components))
                    .offset((chunk_index * 4 * std::mem::size_of::<f32>()) as u32),
            );
            location += 1;
        }

        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&vertex_bindings)
            .vertex_attribute_descriptions(&vertex_attributes);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(if backface_culling {
                vk::CullModeFlags::BACK
            } else {
                vk::CullModeFlags::NONE
            })
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        let color_blend_attachments = [color_blend_attachment];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);
        let has_depth = render_pass_key.depth_format.is_some();
        let make_depth_stencil = |depth_write| {
            vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(has_depth)
                .depth_write_enable(has_depth && depth_write)
                .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL)
                .depth_bounds_test_enable(false)
                .stencil_test_enable(false)
        };
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        let render_pass = self.get_or_create_pipeline_render_pass(render_pass_key)?;
        let depth_stencil_write = make_depth_stencil(true);
        let depth_stencil_no_write = make_depth_stencil(false);
        let create_info_write = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil_write)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic)
            .layout(pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);
        let create_info_no_write = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil_no_write)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic)
            .layout(pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);

        let pipeline_result = unsafe {
            self.device.create_graphics_pipelines(
                vk::PipelineCache::null(),
                &[create_info_write, create_info_no_write],
                None,
            )
        };

        unsafe {
            self.device.destroy_shader_module(vs_module, None);
            self.device.destroy_shader_module(fs_module, None);
        }
        let (pipeline_write, pipeline_no_write) = match pipeline_result {
            Ok(pipelines) if pipelines.len() >= 2 => (pipelines[0], pipelines[1]),
            Ok(pipelines) => {
                unsafe {
                    for pipeline in pipelines {
                        self.device.destroy_pipeline(pipeline, None);
                    }
                    self.device.destroy_pipeline_layout(pipeline_layout, None);
                    self.device
                        .destroy_descriptor_set_layout(descriptor_set_layout, None);
                }
                return Err("create_graphics_pipelines returned fewer than 2 pipelines".to_string());
            }
            Err((pipelines, e)) => {
                unsafe {
                    for pipeline in pipelines {
                        self.device.destroy_pipeline(pipeline, None);
                    }
                    self.device.destroy_pipeline_layout(pipeline_layout, None);
                    self.device
                        .destroy_descriptor_set_layout(descriptor_set_layout, None);
                }
                return Err(format!("create_graphics_pipelines failed: {e:?}"));
            }
        };

        let mut sampler_handles = Vec::with_capacity(sh.mapping.samplers.len());
        for sampler_desc in &sh.mapping.samplers {
            let filter = match sampler_desc.filter {
                crate::makepad_script::shader::SamplerFilter::Nearest => vk::Filter::NEAREST,
                crate::makepad_script::shader::SamplerFilter::Linear => vk::Filter::LINEAR,
            };
            let (address_mode, border_color) = match sampler_desc.address {
                crate::makepad_script::shader::SamplerAddress::Repeat => (
                    vk::SamplerAddressMode::REPEAT,
                    vk::BorderColor::FLOAT_TRANSPARENT_BLACK,
                ),
                crate::makepad_script::shader::SamplerAddress::ClampToEdge => (
                    vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    vk::BorderColor::FLOAT_TRANSPARENT_BLACK,
                ),
                crate::makepad_script::shader::SamplerAddress::ClampToZero => (
                    vk::SamplerAddressMode::CLAMP_TO_BORDER,
                    vk::BorderColor::FLOAT_TRANSPARENT_BLACK,
                ),
                crate::makepad_script::shader::SamplerAddress::MirroredRepeat => (
                    vk::SamplerAddressMode::MIRRORED_REPEAT,
                    vk::BorderColor::FLOAT_TRANSPARENT_BLACK,
                ),
            };
            let mut sampler_info = vk::SamplerCreateInfo::default()
                .mag_filter(filter)
                .min_filter(filter)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                .address_mode_u(address_mode)
                .address_mode_v(address_mode)
                .address_mode_w(address_mode)
                .border_color(border_color)
                .unnormalized_coordinates(false)
                .compare_enable(false)
                .min_lod(0.0)
                .max_lod(vk::LOD_CLAMP_NONE);
            if sampler_desc.coord == crate::makepad_script::shader::SamplerCoord::Pixel {
                sampler_info = sampler_info.unnormalized_coordinates(true);
            }
            let sampler = match unsafe { self.device.create_sampler(&sampler_info, None) } {
                Ok(sampler) => sampler,
                Err(e) => {
                    unsafe {
                        for sampler in sampler_handles.drain(..) {
                            self.device.destroy_sampler(sampler, None);
                        }
                        self.device.destroy_pipeline(pipeline_write, None);
                        self.device.destroy_pipeline(pipeline_no_write, None);
                        self.device.destroy_pipeline_layout(pipeline_layout, None);
                        self.device
                            .destroy_descriptor_set_layout(descriptor_set_layout, None);
                    }
                    return Err(format!("create_sampler failed: {e:?}"));
                }
            };
            sampler_handles.push(sampler);
        }

        self.pipelines.insert(
            pipeline_key,
            VulkanPipeline {
                pipeline_write,
                pipeline_no_write,
                layout: pipeline_layout,
                descriptor_set_layout,
                has_descriptors,
                sampler_handles,
            },
        );

        Ok(())
    }

    fn collect_attribute_chunk_formats(total_slots: usize) -> Vec<DrawShaderAttrFormat> {
        vec![DrawShaderAttrFormat::Float; (total_slots + 3) / 4]
    }

    fn vk_vertex_format(attr_format: DrawShaderAttrFormat, components: usize) -> vk::Format {
        match (attr_format, components.max(1).min(4)) {
            (DrawShaderAttrFormat::Float, 1) => vk::Format::R32_SFLOAT,
            (DrawShaderAttrFormat::Float, 2) => vk::Format::R32G32_SFLOAT,
            (DrawShaderAttrFormat::Float, 3) => vk::Format::R32G32B32_SFLOAT,
            (DrawShaderAttrFormat::Float, _) => vk::Format::R32G32B32A32_SFLOAT,
            (DrawShaderAttrFormat::UInt, 1) => vk::Format::R32_UINT,
            (DrawShaderAttrFormat::UInt, 2) => vk::Format::R32G32_UINT,
            (DrawShaderAttrFormat::UInt, 3) => vk::Format::R32G32B32_UINT,
            (DrawShaderAttrFormat::UInt, _) => vk::Format::R32G32B32A32_UINT,
            (DrawShaderAttrFormat::SInt, 1) => vk::Format::R32_SINT,
            (DrawShaderAttrFormat::SInt, 2) => vk::Format::R32G32_SINT,
            (DrawShaderAttrFormat::SInt, 3) => vk::Format::R32G32B32_SINT,
            (DrawShaderAttrFormat::SInt, _) => vk::Format::R32G32B32A32_SINT,
        }
    }

    fn align_device_size(value: vk::DeviceSize, alignment: vk::DeviceSize) -> vk::DeviceSize {
        if alignment <= 1 {
            value
        } else {
            value.div_ceil(alignment) * alignment
        }
    }

    fn create_host_buffer(
        &self,
        usage: vk::BufferUsageFlags,
        byte_len: vk::DeviceSize,
    ) -> Result<VulkanBuffer, String> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(byte_len.max(4))
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { self.device.create_buffer(&buffer_info, None) }
            .map_err(|e| format!("create_buffer failed: {e:?}"))?;
        let mem_req = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let memory_type_index = match self.find_memory_type(
            mem_req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        ) {
            Ok(memory_type_index) => memory_type_index,
            Err(err) => {
                unsafe {
                    self.device.destroy_buffer(buffer, None);
                }
                return Err(err);
            }
        };
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_req.size)
            .memory_type_index(memory_type_index);
        let memory = match unsafe { self.device.allocate_memory(&alloc_info, None) } {
            Ok(memory) => memory,
            Err(e) => {
                unsafe {
                    self.device.destroy_buffer(buffer, None);
                }
                return Err(format!("allocate_memory failed: {e:?}"));
            }
        };
        unsafe {
            if let Err(e) = self.device.bind_buffer_memory(buffer, memory, 0) {
                self.device.free_memory(memory, None);
                self.device.destroy_buffer(buffer, None);
                return Err(format!("bind_buffer_memory failed: {e:?}"));
            }
        }

        Ok(VulkanBuffer {
            buffer,
            memory,
            size: byte_len.max(4),
        })
    }

    fn destroy_buffer(&self, buffer: VulkanBuffer) {
        unsafe {
            if buffer.buffer != vk::Buffer::null() {
                self.device.destroy_buffer(buffer.buffer, None);
            }
            if buffer.memory != vk::DeviceMemory::null() {
                self.device.free_memory(buffer.memory, None);
            }
        }
    }

    fn destroy_geometry_resource(&self, resource: VulkanGeometryResource) {
        self.destroy_buffer(resource.vertex_buffer);
        self.destroy_buffer(resource.index_buffer);
    }

    fn ensure_geometry_resource(
        &mut self,
        geometry_id: GeometryId,
        geometry: &mut crate::geometry::CxGeometry,
    ) -> Result<(), String> {
        if geometry.vertices.is_empty() || geometry.indices.is_empty() {
            if let Some(old) = self.geometries.remove(&geometry_id) {
                self.destroy_geometry_resource(old);
            }
            geometry.dirty_vertices = false;
            geometry.dirty_indices = false;
            geometry.dirty = false;
            return Ok(());
        }

        let existing = self.geometries.remove(&geometry_id);
        let vertex_needs_upload = existing.is_none() || geometry.dirty_vertices;
        let index_needs_upload = existing.is_none() || geometry.dirty_indices;

        let new_vertex_buffer = if vertex_needs_upload {
            Some(self.create_host_buffer_with_data(
                vk::BufferUsageFlags::VERTEX_BUFFER,
                &geometry.vertices,
            )?)
        } else {
            None
        };

        let new_index_buffer = if index_needs_upload {
            match self
                .create_host_buffer_with_data(vk::BufferUsageFlags::INDEX_BUFFER, &geometry.indices)
            {
                Ok(buffer) => Some(buffer),
                Err(err) => {
                    if let Some(buffer) = new_vertex_buffer {
                        self.destroy_buffer(buffer);
                    }
                    if let Some(existing) = existing {
                        self.geometries.insert(geometry_id, existing);
                    }
                    return Err(err);
                }
            }
        } else {
            None
        };

        let resource = match existing {
            Some(existing) => {
                if vertex_needs_upload {
                    self.destroy_buffer(existing.vertex_buffer);
                }
                if index_needs_upload {
                    self.destroy_buffer(existing.index_buffer);
                }
                VulkanGeometryResource {
                    vertex_buffer: new_vertex_buffer.unwrap_or(existing.vertex_buffer),
                    index_buffer: new_index_buffer.unwrap_or(existing.index_buffer),
                }
            }
            None => VulkanGeometryResource {
                vertex_buffer: new_vertex_buffer
                    .ok_or_else(|| "missing Vulkan vertex buffer upload".to_string())?,
                index_buffer: new_index_buffer
                    .ok_or_else(|| "missing Vulkan index buffer upload".to_string())?,
            },
        };

        self.geometries.insert(geometry_id, resource);
        geometry.dirty_vertices = false;
        geometry.dirty_indices = false;
        geometry.dirty = false;
        Ok(())
    }

    fn create_frame_descriptor_pool(&self) -> Result<vk::DescriptorPool, String> {
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 8192,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: 4096,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1024,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: 4096,
            },
        ];
        let info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(2048)
            .pool_sizes(&pool_sizes);
        unsafe { self.device.create_descriptor_pool(&info, None) }
            .map_err(|e| format!("create_descriptor_pool failed: {e:?}"))
    }

    fn alloc_frame_descriptor_set(
        &mut self,
        descriptor_set_layout: vk::DescriptorSetLayout,
    ) -> Result<vk::DescriptorSet, String> {
        if self.frame_resources.descriptor_pools.is_empty() {
            let pool = self.create_frame_descriptor_pool()?;
            self.frame_resources.descriptor_pools.push(pool);
        }
        let try_alloc = |device: &ash::Device, pool: vk::DescriptorPool| {
            let set_layouts = [descriptor_set_layout];
            let alloc_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(&set_layouts);
            unsafe { device.allocate_descriptor_sets(&alloc_info) }.map(|sets| sets[0])
        };

        let pool = *self.frame_resources.descriptor_pools.last().unwrap();
        match try_alloc(&self.device, pool) {
            Ok(set) => Ok(set),
            Err(vk::Result::ERROR_OUT_OF_POOL_MEMORY) | Err(vk::Result::ERROR_FRAGMENTED_POOL) => {
                let pool = self.create_frame_descriptor_pool()?;
                self.frame_resources.descriptor_pools.push(pool);
                try_alloc(&self.device, pool)
                    .map_err(|e| format!("allocate_descriptor_sets failed: {e:?}"))
            }
            Err(e) => Err(format!("allocate_descriptor_sets failed: {e:?}")),
        }
    }

    fn create_host_buffer_with_data<T: Copy>(
        &self,
        usage: vk::BufferUsageFlags,
        data: &[T],
    ) -> Result<VulkanBuffer, String> {
        let byte_len = std::mem::size_of_val(data) as vk::DeviceSize;
        let buffer = self.create_host_buffer(usage, byte_len)?;

        if !data.is_empty() {
            unsafe {
                let mapped = self
                    .device
                    .map_memory(buffer.memory, 0, buffer.size, vk::MemoryMapFlags::empty())
                    .map_err(|e| format!("map_memory failed: {e:?}"))?;
                std::ptr::copy_nonoverlapping(
                    data.as_ptr() as *const u8,
                    mapped as *mut u8,
                    std::mem::size_of_val(data),
                );
                self.device.unmap_memory(buffer.memory);
            }
        }

        Ok(buffer)
    }

    fn find_memory_type(
        &self,
        type_filter: u32,
        properties: vk::MemoryPropertyFlags,
    ) -> Result<u32, String> {
        let memory_props = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical_device)
        };
        for i in 0..memory_props.memory_type_count {
            let bit = 1u32 << i;
            if (type_filter & bit) == 0 {
                continue;
            }
            let flags = memory_props.memory_types[i as usize].property_flags;
            if flags.contains(properties) {
                return Ok(i);
            }
        }
        Err(format!(
            "failed to find memory type matching {:?} for filter 0x{:X}",
            properties, type_filter
        ))
    }

    fn create_surface(
        android_surface_loader: &ash::khr::android_surface::Instance,
        window: *mut ndk_sys::ANativeWindow,
    ) -> Result<vk::SurfaceKHR, String> {
        let surface_create_info = vk::AndroidSurfaceCreateInfoKHR::default().window(window.cast());
        unsafe { android_surface_loader.create_android_surface(&surface_create_info, None) }
            .map_err(|e| format!("create_android_surface failed: {e:?}"))
    }

    fn pick_device_and_queue_family(
        instance: &ash::Instance,
        surface_loader: &ash::khr::surface::Instance,
        surface: vk::SurfaceKHR,
    ) -> Result<(vk::PhysicalDevice, u32), String> {
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|e| format!("enumerate_physical_devices failed: {e:?}"))?;

        for physical_device in physical_devices {
            if let Ok(queue_family_index) = Self::pick_queue_family_for_device(
                instance,
                surface_loader,
                surface,
                physical_device,
            ) {
                return Ok((physical_device, queue_family_index));
            }
        }

        Err("No Vulkan physical device with graphics+present support found".to_string())
    }

    fn pick_queue_family_for_device(
        instance: &ash::Instance,
        surface_loader: &ash::khr::surface::Instance,
        surface: vk::SurfaceKHR,
        physical_device: vk::PhysicalDevice,
    ) -> Result<u32, String> {
        let queue_families =
            unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
        for (index, family) in queue_families.iter().enumerate() {
            if !family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                continue;
            }
            let supports_surface = unsafe {
                surface_loader.get_physical_device_surface_support(
                    physical_device,
                    index as u32,
                    surface,
                )
            }
            .map_err(|e| format!("get_physical_device_surface_support failed: {e:?}"))?;
            if supports_surface {
                return Ok(index as u32);
            }
        }
        Err("No Vulkan queue family with graphics+present support found".to_string())
    }

    fn recreate_swapchain(&mut self) -> Result<(), String> {
        if self.surface == vk::SurfaceKHR::null() {
            return Ok(());
        }

        let capabilities = unsafe {
            self.surface_loader
                .get_physical_device_surface_capabilities(self.physical_device, self.surface)
        }
        .map_err(|e| format!("get_surface_capabilities failed: {e:?}"))?;

        let formats = unsafe {
            self.surface_loader
                .get_physical_device_surface_formats(self.physical_device, self.surface)
        }
        .map_err(|e| format!("get_surface_formats failed: {e:?}"))?;
        if formats.is_empty() {
            return Err("No Vulkan surface formats available".to_string());
        }

        let format = formats
            .iter()
            .copied()
            .find(|f| {
                f.format == vk::Format::B8G8R8A8_UNORM
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .unwrap_or(formats[0]);

        let extent = if capabilities.current_extent.width == u32::MAX {
            vk::Extent2D {
                width: self.requested_width.clamp(
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: self.requested_height.clamp(
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        } else {
            capabilities.current_extent
        };

        let mut image_count = capabilities.min_image_count + 1;
        if capabilities.max_image_count > 0 {
            image_count = image_count.min(capabilities.max_image_count);
        }

        let present_modes = unsafe {
            self.surface_loader
                .get_physical_device_surface_present_modes(self.physical_device, self.surface)
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

        let usage = capabilities.supported_usage_flags;
        if !usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) {
            return Err("Vulkan surface does not support COLOR_ATTACHMENT usage".to_string());
        }
        let mut image_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
        if usage.contains(vk::ImageUsageFlags::TRANSFER_DST) {
            image_usage |= vk::ImageUsageFlags::TRANSFER_DST;
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

        let old_swapchain = self.swapchain;
        self.destroy_swapchain_targets();
        self.destroy_pipelines();

        let queue_family_indices = [self.queue_family_index];
        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(image_usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .queue_family_indices(&queue_family_indices)
            .pre_transform(pre_transform)
            .composite_alpha(composite_alpha)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old_swapchain);

        let new_swapchain = unsafe { self.swapchain_loader.create_swapchain(&create_info, None) }
            .map_err(|e| format!("create_swapchain failed: {e:?}"))?;
        let new_images = unsafe { self.swapchain_loader.get_swapchain_images(new_swapchain) }
            .map_err(|e| format!("get_swapchain_images failed: {e:?}"))?;

        if old_swapchain != vk::SwapchainKHR::null() {
            unsafe { self.swapchain_loader.destroy_swapchain(old_swapchain, None) };
        }

        self.swapchain = new_swapchain;
        self.swapchain_images = new_images;
        self.swapchain_format = format.format;
        self.depth_format = self.pick_depth_format()?;
        self.swapchain_extent = extent;

        let color_attachment = vk::AttachmentDescription::default()
            .format(self.swapchain_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let depth_attachment = vk::AttachmentDescription::default()
            .format(self.depth_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let depth_ref = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_refs = [color_ref];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs)
            .depth_stencil_attachment(&depth_ref);
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )];
        let attachments = [color_attachment, depth_attachment];
        let subpasses = [subpass];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.render_pass = unsafe { self.device.create_render_pass(&render_pass_info, None) }
            .map_err(|e| format!("create_render_pass failed: {e:?}"))?;
        let xr_color_attachment =
            color_attachment.final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let xr_attachments = [xr_color_attachment, depth_attachment];
        let xr_render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&xr_attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.xr_render_pass = unsafe { self.device.create_render_pass(&xr_render_pass_info, None) }
            .map_err(|e| format!("create_render_pass(openxr) failed: {e:?}"))?;

        for image in &self.swapchain_images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(self.swapchain_format)
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
            self.swapchain_image_views.push(view);
        }

        self.swapchain_depth_targets
            .reserve(self.swapchain_images.len());
        for _ in &self.swapchain_images {
            let depth_target = self.create_depth_target(
                self.swapchain_extent.width,
                self.swapchain_extent.height,
                self.depth_format,
            )?;
            self.swapchain_depth_targets.push(depth_target);
        }

        self.swapchain_readback_buffer = if self.swapchain_readback_supported()
            && self.swapchain_extent.width > 0
            && self.swapchain_extent.height > 0
        {
            Some(self.create_host_buffer(
                vk::BufferUsageFlags::TRANSFER_DST,
                self.swapchain_extent.width as vk::DeviceSize
                    * self.swapchain_extent.height as vk::DeviceSize
                    * 4,
            )?)
        } else {
            None
        };

        for (index, view) in self.swapchain_image_views.iter().enumerate() {
            let depth_view = self
                .swapchain_depth_targets
                .get(index)
                .ok_or_else(|| format!("missing depth target for framebuffer {index}"))?
                .view;
            let attachments = [*view, depth_view];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(self.render_pass)
                .attachments(&attachments)
                .width(self.swapchain_extent.width)
                .height(self.swapchain_extent.height)
                .layers(1);
            let framebuffer = unsafe { self.device.create_framebuffer(&framebuffer_info, None) }
                .map_err(|e| format!("create_framebuffer failed: {e:?}"))?;
            self.framebuffers.push(framebuffer);
        }

        Ok(())
    }

    fn destroy_frame_resources(&mut self) {
        unsafe {
            for pool in self.frame_resources.descriptor_pools.drain(..) {
                self.device.destroy_descriptor_pool(pool, None);
            }
            for buffer in self.frame_resources.buffers.drain(..) {
                self.device.destroy_buffer(buffer.buffer, None);
                self.device.free_memory(buffer.memory, None);
            }
        }
    }

    fn destroy_pipelines(&mut self) {
        unsafe {
            for (_, pipeline) in self.pipelines.drain() {
                for sampler in pipeline.sampler_handles {
                    self.device.destroy_sampler(sampler, None);
                }
                self.device.destroy_pipeline(pipeline.pipeline_write, None);
                self.device
                    .destroy_pipeline(pipeline.pipeline_no_write, None);
                self.device.destroy_pipeline_layout(pipeline.layout, None);
                self.device
                    .destroy_descriptor_set_layout(pipeline.descriptor_set_layout, None);
            }
            for (_, render_pass) in self.offscreen_render_passes.drain() {
                self.device.destroy_render_pass(render_pass, None);
            }
        }
    }

    fn destroy_swapchain_targets(&mut self) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                self.device.destroy_framebuffer(framebuffer, None);
            }
            let depth_targets: Vec<VulkanTextureResource> =
                self.swapchain_depth_targets.drain(..).collect();
            for depth in depth_targets {
                self.destroy_texture_resource(depth);
            }
            for image_view in self.swapchain_image_views.drain(..) {
                self.device.destroy_image_view(image_view, None);
            }
            if self.render_pass != vk::RenderPass::null() {
                self.device.destroy_render_pass(self.render_pass, None);
                self.render_pass = vk::RenderPass::null();
            }
            if self.xr_render_pass != vk::RenderPass::null() {
                self.device.destroy_render_pass(self.xr_render_pass, None);
                self.xr_render_pass = vk::RenderPass::null();
            }
            if let Some(buffer) = self.swapchain_readback_buffer.take() {
                self.device.destroy_buffer(buffer.buffer, None);
                self.device.free_memory(buffer.memory, None);
            }
            self.depth_format = vk::Format::UNDEFINED;
        }
    }

    fn destroy_swapchain(&mut self) {
        self.destroy_frame_resources();
        self.destroy_pipelines();
        self.destroy_swapchain_targets();
        if self.swapchain != vk::SwapchainKHR::null() {
            unsafe {
                self.swapchain_loader
                    .destroy_swapchain(self.swapchain, None)
            };
            self.swapchain = vk::SwapchainKHR::null();
        }
        self.swapchain_images.clear();
    }

    fn destroy_texture_resources(&mut self) {
        let mut resources: Vec<VulkanTextureResource> =
            self.textures.drain().map(|(_, r)| r).collect();
        resources.sort_by_key(|resource| resource.owns_image);
        for resource in resources {
            self.destroy_texture_resource(resource);
        }
        if let Some(resource) = self.xr_depth_dummy.take() {
            self.destroy_texture_resource(resource);
        }
    }

    fn destroy_geometry_resources(&mut self) {
        let resources: Vec<VulkanGeometryResource> = self
            .geometries
            .drain()
            .map(|(_, resource)| resource)
            .collect();
        for resource in resources {
            self.destroy_geometry_resource(resource);
        }
    }

    fn destroy_surface(&mut self) {
        if self.surface != vk::SurfaceKHR::null() {
            unsafe { self.surface_loader.destroy_surface(self.surface, None) };
            self.surface = vk::SurfaceKHR::null();
        }
    }

    fn device_wait_idle(&self) {
        let _ = unsafe { self.device.device_wait_idle() };
    }
}

impl Drop for CxVulkan {
    fn drop(&mut self) {
        self.device_wait_idle();
        self.destroy_swapchain();
        self.destroy_geometry_resources();
        self.destroy_texture_resources();

        unsafe {
            if self.in_flight_fence != vk::Fence::null() {
                self.device.destroy_fence(self.in_flight_fence, None);
            }
            if self.render_finished_semaphore != vk::Semaphore::null() {
                self.device
                    .destroy_semaphore(self.render_finished_semaphore, None);
            }
            if self.image_available_semaphore != vk::Semaphore::null() {
                self.device
                    .destroy_semaphore(self.image_available_semaphore, None);
            }
            if self.command_pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.command_pool, None);
            }
            self.device.destroy_device(None);
        }

        self.destroy_surface();
        if let Some(loader) = &self.debug_utils_loader {
            if self.debug_messenger != vk::DebugUtilsMessengerEXT::null() {
                unsafe { loader.destroy_debug_utils_messenger(self.debug_messenger, None) };
                self.debug_messenger = vk::DebugUtilsMessengerEXT::null();
            }
        }
        unsafe { self.instance.destroy_instance(None) };

        if !self.window.is_null() {
            unsafe { ndk_sys::ANativeWindow_release(self.window) };
            self.window = std::ptr::null_mut();
        }
    }
}

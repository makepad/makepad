#![cfg(target_os = "android")]

use crate::{
    cx::Cx,
    draw_list::DrawListId,
    draw_pass::{DrawPassClearColor, DrawPassId},
    draw_shader::DrawShaderAttrFormat,
    geometry::GeometryId,
    makepad_live_id::*,
    os::linux::android::ndk_sys,
    texture::{TextureFormat, TextureId, TextureUpdated},
};
use ash::vk;
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

#[derive(Default)]
struct FrameResources {
    buffers: Vec<VulkanBuffer>,
    descriptor_pools: Vec<vk::DescriptorPool>,
}

struct VulkanPipeline {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    has_descriptors: bool,
    sampler_handles: Vec<vk::Sampler>,
}

struct VulkanDrawPacket {
    shader_index: usize,
    geometry_id: GeometryId,
    instances: Vec<f32>,
    draw_call_uniforms: Vec<f32>,
    dyn_uniforms: Vec<f32>,
    scope_uniforms: Vec<f32>,
    uniform_bindings: Vec<(LiveId, usize)>,
    dyn_uniform_binding: u32,
    scope_uniform_binding: Option<usize>,
    texture_ids: Vec<TextureId>,
}

struct VulkanTextureResource {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    width: u32,
    height: u32,
    format: vk::Format,
    layout: vk::ImageLayout,
}

struct VulkanTextureUpload {
    data: Vec<u8>,
    offset_x: u32,
    offset_y: u32,
    width: u32,
    height: u32,
}

type VulkanTextureKey = usize;

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
    queue: vk::Queue,
    swapchain_loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    swapchain_images: Vec<vk::Image>,
    swapchain_image_views: Vec<vk::ImageView>,
    swapchain_format: vk::Format,
    swapchain_extent: vk::Extent2D,
    render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    pipelines: HashMap<usize, VulkanPipeline>,
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
        if has_validation_layer {
            crate::log!("Android Vulkan: VK_LAYER_KHRONOS_validation available");
        } else {
            crate::warning!("Android Vulkan: VK_LAYER_KHRONOS_validation not available");
        }

        let available_exts = unsafe { entry.enumerate_instance_extension_properties(None) }
            .map_err(|e| format!("Android Vulkan init failed: enumerate extensions: {e:?}"))?;
        let has_debug_utils_ext = available_exts.iter().any(|ext| {
            let name = unsafe { CStr::from_ptr(ext.extension_name.as_ptr()) };
            name.to_bytes() == vk::EXT_DEBUG_UTILS_NAME.to_bytes()
        });
        if has_debug_utils_ext {
            crate::log!("Android Vulkan: VK_EXT_debug_utils available");
        } else {
            crate::warning!("Android Vulkan: VK_EXT_debug_utils not available");
        }

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
        crate::log!(
            "Android Vulkan device: name='{}' vendor=0x{:04X} device=0x{:04X} api={}.{}.{} driver=0x{:X} queue_family={}",
            device_name,
            props.vendor_id,
            props.device_id,
            vk::api_version_major(props.api_version),
            vk::api_version_minor(props.api_version),
            vk::api_version_patch(props.api_version),
            props.driver_version,
            queue_family_index
        );
        if device_name.contains("SwiftShader") || props.vendor_id == 0x1AE0 {
            crate::warning!(
                "Android Vulkan: SwiftShader/software device detected; expect very low performance"
            );
        }

        let queue_priorities = [1.0f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priorities)];
        let device_extensions = [vk::KHR_SWAPCHAIN_NAME.as_ptr()];
        let device_create_info = vk::DeviceCreateInfo::default()
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
            queue,
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            swapchain_images: Vec::new(),
            swapchain_image_views: Vec::new(),
            swapchain_format: vk::Format::UNDEFINED,
            swapchain_extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            render_pass: vk::RenderPass::null(),
            framebuffers: Vec::new(),
            pipelines: HashMap::new(),
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
        };

        if let Err(err) = vulkan.recreate_swapchain() {
            return Err(format!(
                "Android Vulkan init failed: recreate_swapchain: {err}"
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
                crate::log!("Android Vulkan: debug messenger enabled");
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
        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [clear_color.x, clear_color.y, clear_color.z, clear_color.w],
            },
        }];
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

        self.record_draw_list(
            cx,
            draw_pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
            &mut draw_stats,
        )?;

        unsafe {
            self.device.cmd_end_render_pass(self.command_buffer);
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
        let draw_items_len = cx.draw_lists[draw_list_id].draw_items.len();
        for draw_item_id in 0..draw_items_len {
            let (sub_list_id, texture_ids) = {
                let draw_list = &cx.draw_lists[draw_list_id];
                let draw_item = &draw_list.draw_items[draw_item_id];
                if let Some(sub_list_id) = draw_item.kind.sub_list() {
                    (Some(sub_list_id), Vec::new())
                } else if let Some(draw_call) = draw_item.kind.draw_call() {
                    let sh = &cx.draw_shaders.shaders[draw_call.draw_shader_id.index];
                    let null_texture_id = cx.null_texture.texture_id();
                    let texture_ids = (0..sh.mapping.textures.len())
                        .map(|i| {
                            draw_call.texture_slots[i]
                                .as_ref()
                                .map(|texture| texture.texture_id())
                                .unwrap_or(null_texture_id)
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

    fn vec_texture_meta(format: &TextureFormat) -> Option<(u32, u32, vk::Format)> {
        match format {
            TextureFormat::VecBGRAu8_32 { width, height, .. } => {
                Some((*width as u32, *height as u32, vk::Format::B8G8R8A8_UNORM))
            }
            TextureFormat::VecMipBGRAu8_32 { width, height, .. } => {
                Some((*width as u32, *height as u32, vk::Format::B8G8R8A8_UNORM))
            }
            TextureFormat::VecRGBAf32 { width, height, .. } => Some((
                *width as u32,
                *height as u32,
                vk::Format::R32G32B32A32_SFLOAT,
            )),
            TextureFormat::VecRu8 { width, height, .. } => {
                Some((*width as u32, *height as u32, vk::Format::R8_UNORM))
            }
            TextureFormat::VecRGu8 { width, height, .. } => {
                Some((*width as u32, *height as u32, vk::Format::R8G8_UNORM))
            }
            TextureFormat::VecRf32 { width, height, .. } => {
                Some((*width as u32, *height as u32, vk::Format::R32_SFLOAT))
            }
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
                width, height, data, ..
            }
            | TextureFormat::VecMipBGRAu8_32 {
                width, height, data, ..
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
                })
            }
            TextureFormat::VecRGBAf32 {
                width, height, data, ..
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
                })
            }
            TextureFormat::VecRf32 {
                width, height, data, ..
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
                })
            }
            _ => None,
        }
    }

    fn create_texture_resource(
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
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { self.device.create_image(&image_info, None) }
            .map_err(|e| format!("create_image failed: {e:?}"))?;
        let memory_req = unsafe { self.device.get_image_memory_requirements(image) };
        let memory_type_index = self
            .find_memory_type(memory_req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
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
                return Err(format!("create_image_view(texture) failed: {e:?}"));
            }
        };

        Ok(VulkanTextureResource {
            image,
            memory,
            view,
            width: width.max(1),
            height: height.max(1),
            format,
            layout: vk::ImageLayout::UNDEFINED,
        })
    }

    fn destroy_texture_resource(&self, resource: VulkanTextureResource) {
        unsafe {
            if resource.view != vk::ImageView::null() {
                self.device.destroy_image_view(resource.view, None);
            }
            if resource.image != vk::Image::null() {
                self.device.destroy_image(resource.image, None);
            }
            if resource.memory != vk::DeviceMemory::null() {
                self.device.free_memory(resource.memory, None);
            }
        }
    }

    fn layout_stage_access(
        layout: vk::ImageLayout,
    ) -> (vk::PipelineStageFlags, vk::AccessFlags) {
        match layout {
            vk::ImageLayout::UNDEFINED => {
                (vk::PipelineStageFlags::TOP_OF_PIPE, vk::AccessFlags::empty())
            }
            vk::ImageLayout::TRANSFER_DST_OPTIMAL => (
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::TRANSFER_WRITE,
            ),
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => (
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::VERTEX_SHADER,
                vk::AccessFlags::SHADER_READ,
            ),
            _ => (
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::AccessFlags::empty(),
            ),
        }
    }

    fn texture_key(texture_id: TextureId) -> VulkanTextureKey {
        texture_id.0
    }

    fn ensure_texture_uploaded(&mut self, cx: &mut Cx, texture_id: TextureId) -> Result<(), String> {
        let texture_key = Self::texture_key(texture_id);
        let (alloc_changed, updated, width, height, format) = {
            let cxtexture = &mut cx.textures[texture_id];
            if !cxtexture.format.is_vec() {
                return Ok(());
            }
            let alloc_changed = cxtexture.alloc_vec();
            let updated = cxtexture.take_updated();
            let (width, height, format) = Self::vec_texture_meta(&cxtexture.format)
                .ok_or_else(|| format!("unsupported Vulkan texture format: {:?}", cxtexture.format))?;
            (alloc_changed, updated, width, height, format)
        };

        let needs_recreate = match self.textures.get(&texture_key) {
            Some(resource) => {
                alloc_changed
                    || resource.width != width.max(1)
                    || resource.height != height.max(1)
                    || resource.format != format
            }
            None => true,
        };

        if needs_recreate {
            if let Some(old_resource) = self.textures.remove(&texture_key) {
                self.destroy_texture_resource(old_resource);
            }
            let resource = self.create_texture_resource(width, height, format)?;
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
            Self::vec_texture_upload(&cxtexture.format, updated, force_full_upload).ok_or_else(|| {
                format!("texture {} has unsupported upload format", texture_key)
            })?
        };
        if upload.data.is_empty() || upload.width == 0 || upload.height == 0 {
            return Ok(());
        }
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
                    .layer_count(1),
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
                    .layer_count(1);
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
        zbias: &mut f32,
        zbias_step: f32,
        draw_stats: &mut VulkanDrawStats,
    ) -> Result<(), String> {
        let draw_items_len = cx.draw_lists[draw_list_id].draw_items.len();
        for draw_item_id in 0..draw_items_len {
            let null_texture_id = cx.null_texture.texture_id();
            draw_stats.draw_items += 1;
            if let Some(sub_list_id) = cx.draw_lists[draw_list_id].draw_items[draw_item_id]
                .kind
                .sub_list()
            {
                self.record_draw_list(
                    cx,
                    draw_pass_id,
                    sub_list_id,
                    zbias,
                    zbias_step,
                    draw_stats,
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
                            .unwrap_or(null_texture_id)
                    })
                    .collect();

                VulkanDrawPacket {
                    shader_index: draw_call.draw_shader_id.index,
                    geometry_id,
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
                }
            };

            let geometry = &cx.geometries[packet.geometry_id];
            if geometry.indices.is_empty() || geometry.vertices.is_empty() {
                draw_stats.skipped_empty_geometry += 1;
                continue;
            }
            let pass_uniforms = cx.passes[draw_pass_id].pass_uniforms.as_slice().to_vec();
            let draw_list_uniforms = cx.draw_lists[draw_list_id]
                .draw_list_uniforms
                .as_slice()
                .to_vec();

            self.record_draw_packet(
                cx,
                &packet,
                &geometry.vertices,
                &geometry.indices,
                &pass_uniforms,
                &draw_list_uniforms,
            )?;
            draw_stats.packets_recorded += 1;
        }
        Ok(())
    }

    fn record_draw_packet(
        &mut self,
        cx: &Cx,
        packet: &VulkanDrawPacket,
        geometry_vertices: &[f32],
        geometry_indices: &[u32],
        pass_uniforms: &[f32],
        draw_list_uniforms: &[f32],
    ) -> Result<(), String> {
        self.ensure_pipeline(cx, packet.shader_index)?;
        let (
            pipeline_handle,
            pipeline_layout,
            descriptor_set_layout,
            pipeline_has_descriptors,
            pipeline_samplers,
        ) = {
            let pipeline = self.pipelines.get(&packet.shader_index).ok_or_else(|| {
                format!("missing Vulkan pipeline for shader {}", packet.shader_index)
            })?;
            (
                pipeline.pipeline,
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
        let index_count = geometry_indices.len() as u32;
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
        let geometry_offset = Self::align_device_size(cursor, 4);
        let geometry_bytes = std::mem::size_of_val(geometry_vertices) as vk::DeviceSize;
        cursor = geometry_offset + geometry_bytes;

        let instances_offset = Self::align_device_size(cursor, 4);
        let instances_bytes = std::mem::size_of_val(packet.instances.as_slice()) as vk::DeviceSize;
        cursor = instances_offset + instances_bytes;

        let indices_offset = Self::align_device_size(cursor, 4);
        let indices_bytes = std::mem::size_of_val(geometry_indices) as vk::DeviceSize;
        cursor = indices_offset + indices_bytes;

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

        let packet_buffer_usage = vk::BufferUsageFlags::VERTEX_BUFFER
            | vk::BufferUsageFlags::INDEX_BUFFER
            | vk::BufferUsageFlags::UNIFORM_BUFFER;
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
            if geometry_bytes != 0 {
                std::ptr::copy_nonoverlapping(
                    geometry_vertices.as_ptr() as *const u8,
                    mapped_ptr.add(geometry_offset as usize),
                    geometry_bytes as usize,
                );
            }
            if instances_bytes != 0 {
                std::ptr::copy_nonoverlapping(
                    packet.instances.as_ptr() as *const u8,
                    mapped_ptr.add(instances_offset as usize),
                    instances_bytes as usize,
                );
            }
            if indices_bytes != 0 {
                std::ptr::copy_nonoverlapping(
                    geometry_indices.as_ptr() as *const u8,
                    mapped_ptr.add(indices_offset as usize),
                    indices_bytes as usize,
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
        let mut texture_infos = Vec::new();
        let null_texture_key = Self::texture_key(cx.null_texture.texture_id());
        let null_texture_resource = self.textures.get(&null_texture_key);
        for (slot, texture_id) in packet.texture_ids.iter().enumerate() {
            let resource = self
                .textures
                .get(&Self::texture_key(*texture_id))
                .or(null_texture_resource);
            let Some(resource) = resource else {
                return Ok(());
            };
            texture_bindings.push(vk_shader.texture_binding_base + slot as u32);
            texture_infos.push(
                vk::DescriptorImageInfo::default()
                    .image_view(resource.view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
            );
        }

        let mut sampler_bindings = Vec::new();
        let mut sampler_infos = Vec::new();
        for (sampler_index, sampler) in pipeline_samplers.iter().enumerate() {
            sampler_bindings.push(vk_shader.sampler_binding_base + sampler_index as u32);
            sampler_infos.push(vk::DescriptorImageInfo::default().sampler(*sampler));
        }

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

            let mut writes =
                Vec::with_capacity(uniform_uploads.len() + texture_infos.len() + sampler_infos.len());
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
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
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
            unsafe {
                self.device.update_descriptor_sets(&writes, &[]);
            }
            Some(descriptor_set)
        } else {
            None
        };
        let vertex_buffers = [packet_buffer.buffer, packet_buffer.buffer];
        let vertex_offsets = [geometry_offset, instances_offset];

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
                packet_buffer.buffer,
                indices_offset,
                vk::IndexType::UINT32,
            );
            self.device
                .cmd_draw_indexed(self.command_buffer, index_count, instance_count, 0, 0, 0);
        }

        Ok(())
    }

    fn ensure_pipeline(&mut self, cx: &Cx, shader_index: usize) -> Result<(), String> {
        if self.pipelines.contains_key(&shader_index) {
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
            || !sh.mapping.samplers.is_empty();

        let mut descriptor_bindings: Vec<(u32, vk::DescriptorType)> = Vec::new();
        for (_, idx) in &sh.mapping.uniform_buffer_bindings.bindings {
            descriptor_bindings.push((*idx as u32, vk::DescriptorType::UNIFORM_BUFFER));
        }
        if !sh.mapping.dyn_uniforms.inputs.is_empty() {
            descriptor_bindings.push((vk_shader.dyn_uniform_binding, vk::DescriptorType::UNIFORM_BUFFER));
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
        for slot in 0..sh.mapping.textures.len() {
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
        let pipeline_layout = match unsafe { self.device.create_pipeline_layout(&pipeline_layout_info, None) } {
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

        let geometry_formats = Self::collect_attribute_chunk_formats(
            sh.mapping.geometries.total_slots,
            &sh.mapping.geometries.inputs,
        );
        let instance_formats = Self::collect_attribute_chunk_formats(
            sh.mapping.instances.total_slots,
            &sh.mapping.instances.inputs,
        );

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
            .cull_mode(vk::CullModeFlags::NONE)
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
            .layout(pipeline_layout)
            .render_pass(self.render_pass)
            .subpass(0);

        let pipeline_result = unsafe {
            self.device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[create_info], None)
        };

        unsafe {
            self.device.destroy_shader_module(vs_module, None);
            self.device.destroy_shader_module(fs_module, None);
        }
        let pipeline = match pipeline_result {
            Ok(pipelines) => pipelines[0],
            Err(e) => {
                unsafe {
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
                crate::makepad_script::shader::SamplerAddress::Repeat => {
                    (vk::SamplerAddressMode::REPEAT, vk::BorderColor::FLOAT_TRANSPARENT_BLACK)
                }
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
                        self.device.destroy_pipeline(pipeline, None);
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
            shader_index,
            VulkanPipeline {
                pipeline,
                layout: pipeline_layout,
                descriptor_set_layout,
                has_descriptors,
                sampler_handles,
            },
        );

        Ok(())
    }

    fn collect_attribute_chunk_formats(
        total_slots: usize,
        inputs: &[crate::draw_shader::DrawShaderInput],
    ) -> Vec<DrawShaderAttrFormat> {
        let mut out = vec![DrawShaderAttrFormat::Float; (total_slots + 3) / 4];
        for input in inputs {
            if input.attr_format == DrawShaderAttrFormat::Float {
                continue;
            }
            for slot in input.offset..(input.offset + input.slots) {
                out[slot / 4] = input.attr_format;
            }
        }
        out
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
                    return Ok((physical_device, index as u32));
                }
            }
        }

        Err("No Vulkan physical device with graphics+present support found".to_string())
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
        self.swapchain_extent = extent;

        crate::log!(
            "Android Vulkan swapchain: format={:?} color_space={:?} extent={}x{} images={} present_mode={:?}",
            format.format,
            format.color_space,
            extent.width,
            extent.height,
            self.swapchain_images.len(),
            present_mode
        );

        let color_attachment = vk::AttachmentDescription::default()
            .format(self.swapchain_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let color_refs = [color_ref];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs);
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
        let attachments = [color_attachment];
        let subpasses = [subpass];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        self.render_pass = unsafe { self.device.create_render_pass(&render_pass_info, None) }
            .map_err(|e| format!("create_render_pass failed: {e:?}"))?;

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

        for view in &self.swapchain_image_views {
            let attachments = [*view];
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
                self.device.destroy_pipeline(pipeline.pipeline, None);
                self.device.destroy_pipeline_layout(pipeline.layout, None);
                self.device
                    .destroy_descriptor_set_layout(pipeline.descriptor_set_layout, None);
            }
        }
    }

    fn destroy_swapchain_targets(&mut self) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                self.device.destroy_framebuffer(framebuffer, None);
            }
            for image_view in self.swapchain_image_views.drain(..) {
                self.device.destroy_image_view(image_view, None);
            }
            if self.render_pass != vk::RenderPass::null() {
                self.device.destroy_render_pass(self.render_pass, None);
                self.render_pass = vk::RenderPass::null();
            }
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
        let resources: Vec<VulkanTextureResource> = self.textures.drain().map(|(_, r)| r).collect();
        for resource in resources {
            self.destroy_texture_resource(resource);
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

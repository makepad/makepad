use crate::{
    cx::Cx,
    draw_list::DrawListId,
    draw_pass::{DrawPassClearColor, DrawPassId},
    draw_shader::DrawShaderAttrFormat,
    geometry::GeometryId,
    makepad_live_id::*,
    os::linux::android::ndk_sys,
};
use ash::vk;
use std::ffi::CStr;
use std::collections::HashMap;

#[link(name = "nativewindow")]
extern "C" {
    fn ANativeWindow_acquire(window: *mut ndk_sys::ANativeWindow);
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
    skipped_textured_shader: usize,
    skipped_no_instance_slots: usize,
    skipped_no_instances_buffer: usize,
    skipped_instances_too_short: usize,
    skipped_zero_instances: usize,
    skipped_no_geometry_id: usize,
    skipped_empty_geometry: usize,
}

pub struct CxAndroidVulkan {
    instance: ash::Instance,
    surface_loader: ash::khr::surface::Instance,
    android_surface_loader: ash::khr::android_surface::Instance,
    surface: vk::SurfaceKHR,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
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
    frame_resources: FrameResources,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    image_available_semaphore: vk::Semaphore,
    render_finished_semaphore: vk::Semaphore,
    in_flight_fence: vk::Fence,
    window: *mut ndk_sys::ANativeWindow,
    requested_width: u32,
    requested_height: u32,
    present_count: u64,
    present_debug_remaining: u32,
    has_logged_present: bool,
    has_logged_draw_stats: bool,
}

impl CxAndroidVulkan {
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

        let instance_extensions = [vk::KHR_SURFACE_NAME.as_ptr(), vk::KHR_ANDROID_SURFACE_NAME.as_ptr()];
        let app_info = vk::ApplicationInfo {
            api_version: vk::API_VERSION_1_1,
            ..Default::default()
        };
        let instance_create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&instance_extensions);

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
                    return Err(format!("Android Vulkan init failed: create_device: {err:?}"));
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
        let command_buffer = match unsafe { device.allocate_command_buffers(&command_buffer_info) } {
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
        let image_available_semaphore = match unsafe { device.create_semaphore(&semaphore_info, None) } {
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

        let render_finished_semaphore = match unsafe { device.create_semaphore(&semaphore_info, None) } {
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
            device,
            queue,
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            swapchain_images: Vec::new(),
            swapchain_image_views: Vec::new(),
            swapchain_format: vk::Format::UNDEFINED,
            swapchain_extent: vk::Extent2D { width: 0, height: 0 },
            render_pass: vk::RenderPass::null(),
            framebuffers: Vec::new(),
            pipelines: HashMap::new(),
            frame_resources: FrameResources::default(),
            command_pool,
            command_buffer,
            image_available_semaphore,
            render_finished_semaphore,
            in_flight_fence,
            window,
            requested_width: width.max(1),
            requested_height: height.max(1),
            present_count: 0,
            present_debug_remaining: 120,
            has_logged_present: false,
            has_logged_draw_stats: false,
        };

        if let Err(err) = vulkan.recreate_swapchain() {
            return Err(format!("Android Vulkan init failed: recreate_swapchain: {err}"));
        }

        Ok(vulkan)
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

    pub fn draw_pass_and_present(&mut self, cx: &mut Cx, draw_pass_id: DrawPassId) -> Result<(), String> {
        const FORCE_TRANSFER_CLEAR_DEBUG: bool = false;
        const FORCE_RENDERPASS_CLEAR_ONLY_DEBUG: bool = false;

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
        let dont_clear = cx.passes[draw_pass_id].dont_clear;

        static PASS_LOG_ONCE: std::sync::Once = std::sync::Once::new();
        PASS_LOG_ONCE.call_once(|| {
            crate::log!(
                "Android Vulkan first pass: rect=({}, {}) {}x{}, dpi_factor={}, clear=({}, {}, {}, {})",
                pass_rect.pos.x,
                pass_rect.pos.y,
                pass_rect.size.x,
                pass_rect.size.y,
                dpi_factor,
                clear_color.x,
                clear_color.y,
                clear_color.z,
                clear_color.w
            );
        });

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

        let mut zbias = 0.0f32;
        let zbias_step = cx.passes[draw_pass_id].zbias_step;
        let mut draw_stats = VulkanDrawStats::default();
        if FORCE_TRANSFER_CLEAR_DEBUG {
            static TRANSFER_CLEAR_LOG_ONCE: std::sync::Once = std::sync::Once::new();
            TRANSFER_CLEAR_LOG_ONCE.call_once(|| {
                crate::warning!(
                    "Android Vulkan debug: FORCE_TRANSFER_CLEAR_DEBUG enabled (render pass + shaders bypassed)"
                );
            });

            let image = *self
                .swapchain_images
                .get(image_index as usize)
                .ok_or_else(|| format!("invalid swapchain image index {image_index}"))?;

            let range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let to_transfer = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(image)
                .subresource_range(range);
            let clear = vk::ClearColorValue {
                float32: [0.0, 1.0, 0.0, 1.0],
            };
            let to_present = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .image(image)
                .subresource_range(range);
            unsafe {
                self.device.cmd_pipeline_barrier(
                    self.command_buffer,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_transfer],
                );
                self.device.cmd_clear_color_image(
                    self.command_buffer,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &clear,
                    &[range],
                );
                self.device.cmd_pipeline_barrier(
                    self.command_buffer,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_present],
                );
            }
        } else {
            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 1.0, 0.0, 1.0],
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
                        y: 0.0,
                        width: self.swapchain_extent.width as f32,
                        height: self.swapchain_extent.height as f32,
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

            if FORCE_RENDERPASS_CLEAR_ONLY_DEBUG {
                static RENDERPASS_CLEAR_ONLY_LOG_ONCE: std::sync::Once = std::sync::Once::new();
                RENDERPASS_CLEAR_ONLY_LOG_ONCE.call_once(|| {
                    crate::warning!(
                        "Android Vulkan debug: FORCE_RENDERPASS_CLEAR_ONLY_DEBUG enabled (render pass clear only, draw list skipped)"
                    );
                });
            } else {
                self.record_draw_list(
                    cx,
                    draw_pass_id,
                    draw_list_id,
                    &mut zbias,
                    zbias_step,
                    &mut draw_stats,
                )?;
                if !self.has_logged_draw_stats {
                    self.has_logged_draw_stats = true;
                    crate::log!(
                        "Android Vulkan draw stats: draw_items={}, draw_calls={}, packets={}, skip_non_draw_call={}, skip_no_os_shader={}, skip_no_vulkan_shader={}, skip_missing_spirv={}, skip_textured={}, skip_no_instance_slots={}, skip_no_instances_buffer={}, skip_instances_too_short={}, skip_zero_instances={}, skip_no_geometry_id={}, skip_empty_geometry={}",
                        draw_stats.draw_items,
                        draw_stats.draw_calls,
                        draw_stats.packets_recorded,
                        draw_stats.skipped_non_draw_call,
                        draw_stats.skipped_no_os_shader,
                        draw_stats.skipped_no_vulkan_shader,
                        draw_stats.skipped_missing_spirv,
                        draw_stats.skipped_textured_shader,
                        draw_stats.skipped_no_instance_slots,
                        draw_stats.skipped_no_instances_buffer,
                        draw_stats.skipped_instances_too_short,
                        draw_stats.skipped_zero_instances,
                        draw_stats.skipped_no_geometry_id,
                        draw_stats.skipped_empty_geometry
                    );
                }
            }

            unsafe {
                self.device.cmd_end_render_pass(self.command_buffer);
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

        let swapchains = [self.swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);

        let present_suboptimal = match unsafe { self.swapchain_loader.queue_present(self.queue, &present_info) } {
            Ok(suboptimal) => suboptimal,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate_swapchain()?;
                return Ok(());
            }
            Err(err) => {
                return Err(format!("queue_present failed: {err:?}"));
            }
        };

        if !self.has_logged_present {
            self.has_logged_present = true;
            crate::log!("Android Vulkan: native swapchain present path is active");
        }

        self.present_count += 1;
        if self.present_debug_remaining > 0 {
            self.present_debug_remaining -= 1;
            crate::log!(
                "Android Vulkan present#{}: pass={} clear=({}, {}, {}, {}) dont_clear={} packets={} draw_calls={} skip_textured={} acquire_suboptimal={} present_suboptimal={}",
                self.present_count,
                draw_pass_id.0,
                clear_color.x,
                clear_color.y,
                clear_color.z,
                clear_color.w,
                dont_clear,
                draw_stats.packets_recorded,
                draw_stats.draw_calls,
                draw_stats.skipped_textured_shader,
                acquire_suboptimal,
                present_suboptimal
            );
        }

        if acquire_suboptimal || present_suboptimal {
            self.recreate_swapchain()?;
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
        const MAX_DRAW_PACKETS_DEBUG: usize = 2;

        let draw_items_len = cx.draw_lists[draw_list_id].draw_items.len();
        for draw_item_id in 0..draw_items_len {
            if MAX_DRAW_PACKETS_DEBUG != 0 && draw_stats.packets_recorded >= MAX_DRAW_PACKETS_DEBUG {
                return Ok(());
            }
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
                if !sh.mapping.textures.is_empty() {
                    draw_stats.skipped_textured_shader += 1;
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

                VulkanDrawPacket {
                    shader_index: draw_call.draw_shader_id.index,
                    geometry_id,
                    instances,
                    draw_call_uniforms: draw_call.draw_call_uniforms.as_slice().to_vec(),
                    dyn_uniforms: draw_call.dyn_uniforms
                        [..sh.mapping.dyn_uniforms.total_slots.min(draw_call.dyn_uniforms.len())]
                        .to_vec(),
                    scope_uniforms: sh.mapping.scope_uniforms_buf.clone(),
                    uniform_bindings: sh.mapping.uniform_buffer_bindings.bindings.clone(),
                    dyn_uniform_binding: vk_shader.dyn_uniform_binding,
                    scope_uniform_binding: sh.mapping.uniform_buffer_bindings.scope_uniform_buffer_index,
                }
            };

            let geometry = &cx.geometries[packet.geometry_id];
            if geometry.indices.is_empty() || geometry.vertices.is_empty() {
                draw_stats.skipped_empty_geometry += 1;
                continue;
            }
            let pass_uniforms = cx.passes[draw_pass_id].pass_uniforms.as_slice().to_vec();
            let draw_list_uniforms = cx.draw_lists[draw_list_id].draw_list_uniforms.as_slice().to_vec();

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
        let pipeline = self
            .pipelines
            .get(&packet.shader_index)
            .ok_or_else(|| format!("missing Vulkan pipeline for shader {}", packet.shader_index))?;

        let sh = &cx.draw_shaders.shaders[packet.shader_index];
        let geometry_stride = (sh.mapping.geometries.total_slots * std::mem::size_of::<f32>()) as u64;
        let instance_stride = (sh.mapping.instances.total_slots * std::mem::size_of::<f32>()) as u64;
        if geometry_stride == 0 || instance_stride == 0 {
            return Ok(());
        }

        let vb_geometry = self.create_host_buffer_with_data(
            vk::BufferUsageFlags::VERTEX_BUFFER,
            geometry_vertices,
        )?;
        let vb_instances =
            self.create_host_buffer_with_data(vk::BufferUsageFlags::VERTEX_BUFFER, &packet.instances)?;
        let ib_indices =
            self.create_host_buffer_with_data(vk::BufferUsageFlags::INDEX_BUFFER, geometry_indices)?;

        self.frame_resources.buffers.push(vb_geometry);
        self.frame_resources.buffers.push(vb_instances);
        self.frame_resources.buffers.push(ib_indices);

        let mut binding_payloads: Vec<(u32, VulkanBuffer)> = Vec::new();
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
            let ub =
                self.create_host_buffer_with_data(vk::BufferUsageFlags::UNIFORM_BUFFER, src)?;
            self.frame_resources.buffers.push(ub);
            binding_payloads.push((*binding_idx as u32, ub));
        }

        if !packet.dyn_uniforms.is_empty() {
            let ub = self.create_host_buffer_with_data(
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                &packet.dyn_uniforms,
            )?;
            self.frame_resources.buffers.push(ub);
            binding_payloads.push((packet.dyn_uniform_binding, ub));
        }

        if let Some(scope_binding) = packet.scope_uniform_binding {
            if !packet.scope_uniforms.is_empty() {
                let ub = self.create_host_buffer_with_data(
                    vk::BufferUsageFlags::UNIFORM_BUFFER,
                    &packet.scope_uniforms,
                )?;
                self.frame_resources.buffers.push(ub);
                binding_payloads.push((scope_binding as u32, ub));
            }
        }

        binding_payloads.sort_by_key(|(binding, _)| *binding);
        binding_payloads.dedup_by_key(|(binding, _)| *binding);

        static FIRST_PACKET_LOG_ONCE: std::sync::Once = std::sync::Once::new();
        FIRST_PACKET_LOG_ONCE.call_once(|| {
            let pass_preview: Vec<f32> = pass_uniforms.iter().take(16).copied().collect();
            let list_preview: Vec<f32> = draw_list_uniforms.iter().take(16).copied().collect();
            let call_preview: Vec<f32> = packet.draw_call_uniforms.iter().take(16).copied().collect();
            let inst_preview: Vec<f32> = packet.instances.iter().take(24).copied().collect();
            crate::log!(
                "Android Vulkan first packet: shader={}, geom_vertices_f32={}, geom_indices_u32={}, instances_f32={}, pass_uniforms_f32={}, draw_list_uniforms_f32={}, draw_call_uniforms_f32={}, dyn_uniforms_f32={}, scope_uniforms_f32={}, ub_bindings={:?}, pass_preview={:?}, draw_list_preview={:?}, draw_call_preview={:?}, instance_preview={:?}",
                packet.shader_index,
                geometry_vertices.len(),
                geometry_indices.len(),
                packet.instances.len(),
                pass_uniforms.len(),
                draw_list_uniforms.len(),
                packet.draw_call_uniforms.len(),
                packet.dyn_uniforms.len(),
                packet.scope_uniforms.len(),
                packet.uniform_bindings,
                pass_preview,
                list_preview,
                call_preview,
                inst_preview
            );
        });

        let descriptor_set = if pipeline.has_descriptors && !binding_payloads.is_empty() {
            let descriptor_pool = {
                let pool_sizes = [vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::UNIFORM_BUFFER,
                    descriptor_count: binding_payloads.len() as u32,
                }];
                let info = vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&pool_sizes);
                unsafe { self.device.create_descriptor_pool(&info, None) }
                    .map_err(|e| format!("create_descriptor_pool failed: {e:?}"))?
            };
            self.frame_resources.descriptor_pools.push(descriptor_pool);

            let set_layouts = [pipeline.descriptor_set_layout];
            let alloc_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(descriptor_pool)
                .set_layouts(&set_layouts);
            let descriptor_set = unsafe { self.device.allocate_descriptor_sets(&alloc_info) }
                .map_err(|e| format!("allocate_descriptor_sets failed: {e:?}"))?
                [0];

            let mut buffer_infos = Vec::with_capacity(binding_payloads.len());
            for (_, ub) in &binding_payloads {
                buffer_infos.push(
                    vk::DescriptorBufferInfo::default()
                        .buffer(ub.buffer)
                        .offset(0)
                        .range(ub.size),
                );
            }

            let mut writes = Vec::with_capacity(binding_payloads.len());
            for (index, (binding, _)) in binding_payloads.iter().enumerate() {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(*binding)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(std::slice::from_ref(&buffer_infos[index])),
                );
            }
            unsafe {
                self.device.update_descriptor_sets(&writes, &[]);
            }
            Some(descriptor_set)
        } else {
            None
        };

        let instance_count = (packet.instances.len() / sh.mapping.instances.total_slots) as u32;
        if instance_count == 0 {
            return Ok(());
        }

        unsafe {
            self.device
                .cmd_bind_pipeline(self.command_buffer, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
            let vbs = [vb_geometry.buffer, vb_instances.buffer];
            let offsets = [0u64, 0u64];
            self.device
                .cmd_bind_vertex_buffers(self.command_buffer, 0, &vbs, &offsets);
            self.device.cmd_bind_index_buffer(
                self.command_buffer,
                ib_indices.buffer,
                0,
                vk::IndexType::UINT32,
            );
            if let Some(set) = descriptor_set {
                self.device.cmd_bind_descriptor_sets(
                    self.command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipeline.layout,
                    0,
                    &[set],
                    &[],
                );
            }
            self.device.cmd_draw_indexed(
                self.command_buffer,
                geometry_indices.len() as u32,
                instance_count,
                0,
                0,
                0,
            );
        }

        Ok(())
    }

    fn ensure_pipeline(&mut self, cx: &Cx, shader_index: usize) -> Result<(), String> {
        if self.pipelines.contains_key(&shader_index) {
            return Ok(());
        }

        let sh = &cx.draw_shaders.shaders[shader_index];
        if !sh.mapping.textures.is_empty() {
            return Err(format!(
                "shader {} uses textures; Vulkan textured path is not wired yet",
                shader_index
            ));
        }
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
            || !sh.mapping.scope_uniforms.inputs.is_empty();

        let mut descriptor_bindings = Vec::new();
        for (_, idx) in &sh.mapping.uniform_buffer_bindings.bindings {
            descriptor_bindings.push(*idx as u32);
        }
        if !sh.mapping.dyn_uniforms.inputs.is_empty() {
            descriptor_bindings.push(vk_shader.dyn_uniform_binding);
        }
        if !sh.mapping.scope_uniforms.inputs.is_empty() {
            if let Some(idx) = sh.mapping.uniform_buffer_bindings.scope_uniform_buffer_index {
                descriptor_bindings.push(idx as u32);
            }
        }
        descriptor_bindings.sort_unstable();
        descriptor_bindings.dedup();

        let descriptor_set_layout = {
            let mut dsl_bindings = Vec::new();
            for binding in &descriptor_bindings {
                dsl_bindings.push(
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(*binding)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
                );
            }
            let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&dsl_bindings);
            unsafe { self.device.create_descriptor_set_layout(&info, None) }
                .map_err(|e| format!("create_descriptor_set_layout failed: {e:?}"))?
        };

        let set_layouts = [descriptor_set_layout];
        let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
        let pipeline_layout = unsafe { self.device.create_pipeline_layout(&pipeline_layout_info, None) }
            .map_err(|e| format!("create_pipeline_layout failed: {e:?}"))?;

        let vs_module_info = vk::ShaderModuleCreateInfo::default().code(vs_spv);
        let fs_module_info = vk::ShaderModuleCreateInfo::default().code(fs_spv);
        let vs_module = unsafe { self.device.create_shader_module(&vs_module_info, None) }
            .map_err(|e| format!("create_shader_module(vertex) failed: {e:?}"))?;
        let fs_module = unsafe { self.device.create_shader_module(&fs_module_info, None) }
            .map_err(|e| format!("create_shader_module(fragment) failed: {e:?}"))?;

        let main_name = std::ffi::CString::new("main").unwrap();
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs_module)
                .name(&main_name),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs_module)
                .name(&main_name),
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
            let remaining = sh.mapping.geometries.total_slots.saturating_sub(chunk_index * 4);
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
            let remaining = sh.mapping.instances.total_slots.saturating_sub(chunk_index * 4);
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
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&color_blend_attachments);
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

        let pipeline = unsafe {
            self.device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[create_info], None)
        }
        .map_err(|e| format!("create_graphics_pipelines failed: {e:?}"))?[0];

        crate::log!(
            "Android Vulkan pipeline created: shader={}, geom_slots={}, inst_slots={}, vertex_attrs={}, descriptor_bindings={:?}",
            shader_index,
            sh.mapping.geometries.total_slots,
            sh.mapping.instances.total_slots,
            vertex_attributes.len(),
            descriptor_bindings
        );

        unsafe {
            self.device.destroy_shader_module(vs_module, None);
            self.device.destroy_shader_module(fs_module, None);
        }

        self.pipelines.insert(
            shader_index,
            VulkanPipeline {
                pipeline,
                layout: pipeline_layout,
                descriptor_set_layout,
                has_descriptors,
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

    fn create_host_buffer_with_data<T: Copy>(
        &self,
        usage: vk::BufferUsageFlags,
        data: &[T],
    ) -> Result<VulkanBuffer, String> {
        let byte_len = (std::mem::size_of_val(data)).max(4) as vk::DeviceSize;
        let buffer_info = vk::BufferCreateInfo::default()
            .size(byte_len)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { self.device.create_buffer(&buffer_info, None) }
            .map_err(|e| format!("create_buffer failed: {e:?}"))?;
        let mem_req = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let memory_type_index = self.find_memory_type(
            mem_req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_req.size)
            .memory_type_index(memory_type_index);
        let memory = unsafe { self.device.allocate_memory(&alloc_info, None) }
            .map_err(|e| format!("allocate_memory failed: {e:?}"))?;
        unsafe {
            self.device
                .bind_buffer_memory(buffer, memory, 0)
                .map_err(|e| format!("bind_buffer_memory failed: {e:?}"))?;
        }

        if !data.is_empty() {
            unsafe {
                let mapped = self
                    .device
                    .map_memory(memory, 0, byte_len, vk::MemoryMapFlags::empty())
                    .map_err(|e| format!("map_memory failed: {e:?}"))?;
                std::ptr::copy_nonoverlapping(
                    data.as_ptr() as *const u8,
                    mapped as *mut u8,
                    std::mem::size_of_val(data),
                );
                self.device.unmap_memory(memory);
            }
        }

        Ok(VulkanBuffer {
            buffer,
            memory,
            size: byte_len,
        })
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
                width: self
                    .requested_width
                    .clamp(capabilities.min_image_extent.width, capabilities.max_image_extent.width),
                height: self
                    .requested_height
                    .clamp(capabilities.min_image_extent.height, capabilities.max_image_extent.height),
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
                self.device.destroy_pipeline(pipeline.pipeline, None);
                self.device
                    .destroy_pipeline_layout(pipeline.layout, None);
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
            unsafe { self.swapchain_loader.destroy_swapchain(self.swapchain, None) };
            self.swapchain = vk::SwapchainKHR::null();
        }
        self.swapchain_images.clear();
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

impl Drop for CxAndroidVulkan {
    fn drop(&mut self) {
        self.device_wait_idle();
        self.destroy_swapchain();

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
        unsafe { self.instance.destroy_instance(None) };

        if !self.window.is_null() {
            unsafe { ndk_sys::ANativeWindow_release(self.window) };
            self.window = std::ptr::null_mut();
        }
    }
}

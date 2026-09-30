//! GPU tests on a real Vulkan device (KERNELS.md P0-S and P0 gates), the
//! Vulkan half of `os/apple/metal_gpu_tests.rs`. Headless: no surface, its own
//! instance and device (with the validation layer when installed; any
//! validation error fails the test).
//!
//! - MRT pixel test: one render pass with RGBA16F, RG16F and R32Uint
//!   attachments, drawn through a pipeline whose blend state is
//!   `mrt_blend_attachments` (the function the renderer builds its pipelines
//!   with), then a one-output shader drawn into the same pass leaving the
//!   other attachments untouched;
//! - the hostile-shader suite: Splash shaders with out-of-range indices and
//!   runaway nested loops, lowered by the WGSL emitter and compiled to SPIR-V
//!   by the same path the renderer uses, run on the GPU, finishing quickly
//!   with in-range (clamped) results.
//!
//! Run with
//! `MAKEPAD=vulkan cargo test --release -p makepad-platform --lib linux::vulkan::gpu_tests -- --test-threads=1`.
//! With no Vulkan device the tests pass without running.

use super::*;
use crate::makepad_script::shader::*;
use crate::makepad_script::shader_backend::*;
use crate::makepad_script::*;
use crate::os::linux::vulkan_naga::compile_wgsl_to_spirv;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static VALIDATION_ERRORS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn count_validation_errors(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut c_void,
) -> vk::Bool32 {
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        VALIDATION_ERRORS.fetch_add(1, Ordering::SeqCst);
        let msg = if data.is_null() { "<null>".into() } else { CStr::from_ptr((*data).p_message).to_string_lossy().into_owned() };
        eprintln!("VALIDATION ERROR: {msg}");
    }
    vk::FALSE
}

struct Target {
    image: vk::Image,
    _memory: vk::DeviceMemory,
    view: vk::ImageView,
    format: vk::Format,
    size: u32,
}

struct Gpu {
    _entry: ash::Entry,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    validation: bool,
    _messenger: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
}

impl Gpu {
    fn new() -> Option<Gpu> {
        let entry = unsafe { ash::Entry::load() }.ok()?;
        let layers = unsafe { entry.enumerate_instance_layer_properties() }.ok()?;
        let validation = layers.iter().any(|l| unsafe { CStr::from_ptr(l.layer_name.as_ptr()) }.to_bytes() == b"VK_LAYER_KHRONOS_validation");
        let extensions = unsafe { entry.enumerate_instance_extension_properties(None) }.ok()?;
        let debug_utils = extensions.iter().any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) } == vk::EXT_DEBUG_UTILS_NAME);
        let layer_names = if validation { vec![c"VK_LAYER_KHRONOS_validation".as_ptr()] } else { vec![] };
        let extension_names = if debug_utils { vec![vk::EXT_DEBUG_UTILS_NAME.as_ptr()] } else { vec![] };
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let info = vk::InstanceCreateInfo::default()
            .application_info(&app)
            .enabled_layer_names(&layer_names)
            .enabled_extension_names(&extension_names);
        let instance = unsafe { entry.create_instance(&info, None) }.ok()?;
        let messenger = if debug_utils && validation {
            let utils = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let create = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR)
                .message_type(vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION | vk::DebugUtilsMessageTypeFlagsEXT::GENERAL)
                .pfn_user_callback(Some(count_validation_errors));
            let m = unsafe { utils.create_debug_utils_messenger(&create, None) }.ok()?;
            Some((utils, m))
        } else {
            None
        };
        let devices = unsafe { instance.enumerate_physical_devices() }.ok()?;
        // Prefer a discrete GPU.
        let physical = devices
            .iter()
            .copied()
            .find(|d| unsafe { instance.get_physical_device_properties(*d) }.device_type == vk::PhysicalDeviceType::DISCRETE_GPU)
            .or_else(|| devices.first().copied())?;
        let props = unsafe { instance.get_physical_device_properties(physical) };
        eprintln!(
            "vulkan_gpu_tests: {} (validation layer: {})",
            unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy(),
            messenger.is_some()
        );
        let family = unsafe { instance.get_physical_device_queue_family_properties(physical) }
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))? as u32;
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
        // The features the renderer's device enables (vulkan_linux.rs).
        let supported = unsafe { instance.get_physical_device_features(physical) };
        let features = vk::PhysicalDeviceFeatures::default().independent_blend(supported.independent_blend == vk::TRUE);
        let device = unsafe { instance.create_device(physical, &vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_features(&features), None) }.ok()?;
        let queue = unsafe { device.get_device_queue(family, 0) };
        let pool = unsafe { device.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(family), None) }.ok()?;
        Some(Gpu { _entry: entry, instance, physical, device, queue, pool, validation, _messenger: messenger })
    }

    fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> u32 {
        let props = unsafe { self.instance.get_physical_device_memory_properties(self.physical) };
        (0..props.memory_type_count)
            .find(|i| bits & (1 << i) != 0 && props.memory_types[*i as usize].property_flags.contains(flags))
            .expect("memory type")
    }

    fn format_features(&self, format: vk::Format) -> vk::FormatFeatureFlags {
        unsafe { self.instance.get_physical_device_format_properties(self.physical, format) }.optimal_tiling_features
    }

    fn target(&self, format: vk::Format, size: u32) -> Target {
        let need = vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::TRANSFER_SRC;
        assert!(self.format_features(format).contains(need), "{format:?} is not a colour attachment here");
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: size, height: size, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        unsafe {
            let image = self.device.create_image(&info, None).unwrap();
            let req = self.device.get_image_memory_requirements(image);
            let memory = self
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(self.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)),
                    None,
                )
                .unwrap();
            self.device.bind_image_memory(image, memory, 0).unwrap();
            let view = self
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(color_range()),
                    None,
                )
                .unwrap();
            Target { image, _memory: memory, view, format, size }
        }
    }

    fn host_buffer(&self, size: u64, usage: vk::BufferUsageFlags) -> (vk::Buffer, vk::DeviceMemory) {
        unsafe {
            let buffer = self.device.create_buffer(&vk::BufferCreateInfo::default().size(size).usage(usage), None).unwrap();
            let req = self.device.get_buffer_memory_requirements(buffer);
            let memory = self
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(self.memory_type(
                        req.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                    None,
                )
                .unwrap();
            self.device.bind_buffer_memory(buffer, memory, 0).unwrap();
            let p = self.device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()).unwrap() as *mut u8;
            std::ptr::write_bytes(p, 0, size as usize);
            self.device.unmap_memory(memory);
            (buffer, memory)
        }
    }

    fn shader_module(&self, spirv: &[u32]) -> vk::ShaderModule {
        unsafe { self.device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(spirv), None).unwrap() }
    }

    /// A render pass over `targets`: each cleared or loaded, and left in
    /// TRANSFER_SRC layout for readback.
    fn render_pass(&self, targets: &[&Target], load: bool) -> vk::RenderPass {
        let attachments: Vec<_> = targets
            .iter()
            .map(|t| {
                vk::AttachmentDescription::default()
                    .format(t.format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(if load { vk::AttachmentLoadOp::LOAD } else { vk::AttachmentLoadOp::CLEAR })
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .initial_layout(if load { vk::ImageLayout::TRANSFER_SRC_OPTIMAL } else { vk::ImageLayout::UNDEFINED })
                    .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            })
            .collect();
        let refs: Vec<_> = (0..targets.len() as u32)
            .map(|i| vk::AttachmentReference::default().attachment(i).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL))
            .collect();
        let subpasses = [vk::SubpassDescription::default().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&refs)];
        let deps = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
        ];
        unsafe {
            self.device
                .create_render_pass(&vk::RenderPassCreateInfo::default().attachments(&attachments).subpasses(&subpasses).dependencies(&deps), None)
                .unwrap()
        }
    }

    /// A full-screen-triangle pipeline (vertex from `gl_VertexIndex`, no
    /// vertex input) with the given fragment SPIR-V and blend state.
    fn pipeline(
        &self,
        render_pass: vk::RenderPass,
        layout: vk::PipelineLayout,
        fragment: &[u32],
        blend: &[vk::PipelineColorBlendAttachmentState],
        size: u32,
    ) -> vk::Pipeline {
        let (vertex, _) = compile_wgsl_to_spirv(TRIANGLE_WGSL).unwrap();
        let vertex = self.shader_module(&vertex.unwrap());
        let fragment = self.shader_module(fragment);
        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vertex).name(c"vertex_main"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fragment).name(c"fragment_main"),
        ];
        let input = vk::PipelineVertexInputStateCreateInfo::default();
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewports = [vk::Viewport { x: 0.0, y: 0.0, width: size as f32, height: size as f32, min_depth: 0.0, max_depth: 1.0 }];
        let scissors = [vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: size, height: size } }];
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewports(&viewports).scissors(&scissors);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(blend);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);
        unsafe { self.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e).unwrap()[0] }
    }

    /// One pass over `targets` (cleared with `clears`, or loaded when
    /// `clears` is empty), drawing the triangle with `pipeline`; returns the
    /// GPU wall time.
    fn draw(
        &self,
        targets: &[&Target],
        clears: &[vk::ClearValue],
        pipeline: vk::Pipeline,
        render_pass: vk::RenderPass,
        layout: vk::PipelineLayout,
        set: Option<vk::DescriptorSet>,
    ) -> Duration {
        unsafe {
            let views: Vec<_> = targets.iter().map(|t| t.view).collect();
            let size = targets[0].size;
            let fb = self
                .device
                .create_framebuffer(&vk::FramebufferCreateInfo::default().render_pass(render_pass).attachments(&views).width(size).height(size).layers(1), None)
                .unwrap();
            let cb = self
                .device
                .allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(self.pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1))
                .unwrap()[0];
            self.device.begin_command_buffer(cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).unwrap();
            let area = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: size, height: size } };
            self.device.cmd_begin_render_pass(
                cb,
                &vk::RenderPassBeginInfo::default().render_pass(render_pass).framebuffer(fb).render_area(area).clear_values(clears),
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
            if let Some(set) = set {
                self.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, layout, 0, &[set], &[]);
            }
            self.device.cmd_draw(cb, 3, 1, 0, 0);
            self.device.cmd_end_render_pass(cb);
            self.device.end_command_buffer(cb).unwrap();
            let fence = self.device.create_fence(&vk::FenceCreateInfo::default(), None).unwrap();
            let cbs = [cb];
            let started = Instant::now();
            self.device.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], fence).unwrap();
            // A hung shader is a failed test, not a hung test run.
            let waited = self.device.wait_for_fences(&[fence], true, 20_000_000_000);
            let elapsed = started.elapsed();
            assert!(waited.is_ok(), "GPU did not finish within 20 s: {waited:?}");
            self.device.destroy_fence(fence, None);
            self.device.destroy_framebuffer(fb, None);
            elapsed
        }
    }

    /// The first pixel of `target` (`bytes` per pixel) as raw bytes, plus the
    /// second pixel of the first row when `bytes` allows (2x2 targets).
    fn read(&self, target: &Target, bytes_per_pixel: usize) -> Vec<u8> {
        unsafe {
            let total = bytes_per_pixel * (target.size * target.size) as usize;
            let (buffer, memory) = self.host_buffer(total as u64, vk::BufferUsageFlags::TRANSFER_DST);
            let cb = self
                .device
                .allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(self.pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1))
                .unwrap()[0];
            self.device.begin_command_buffer(cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).unwrap();
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 })
                .image_extent(vk::Extent3D { width: target.size, height: target.size, depth: 1 });
            self.device.cmd_copy_image_to_buffer(cb, target.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, buffer, &[region]);
            self.device.end_command_buffer(cb).unwrap();
            let cbs = [cb];
            let fence = self.device.create_fence(&vk::FenceCreateInfo::default(), None).unwrap();
            self.device.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], fence).unwrap();
            self.device.wait_for_fences(&[fence], true, 20_000_000_000).unwrap();
            let p = self.device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()).unwrap() as *const u8;
            let out = std::slice::from_raw_parts(p, total).to_vec();
            self.device.unmap_memory(memory);
            self.device.destroy_fence(fence, None);
            self.device.destroy_buffer(buffer, None);
            self.device.free_memory(memory, None);
            out
        }
    }

    fn finish(self) {
        let errors = VALIDATION_ERRORS.load(Ordering::SeqCst);
        // Objects are left for process exit: destroying the device with live
        // pipelines would itself be a validation error.
        unsafe { self.device.device_wait_idle().unwrap() };
        assert_eq!(errors, 0, "Vulkan validation reported errors (validation layer: {})", self.validation);
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 }
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x3ff) as f32;
    sign * match exp {
        0 => mant * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + mant / 1024.0) * 2f32.powi(e - 15),
    }
}

fn halves(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(2).map(|b| f16_to_f32(u16::from_le_bytes([b[0], b[1]]))).collect()
}

fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn clear_f(c: [f32; 4]) -> vk::ClearValue {
    vk::ClearValue { color: vk::ClearColorValue { float32: c } }
}

fn clear_u(c: [u32; 4]) -> vk::ClearValue {
    vk::ClearValue { color: vk::ClearColorValue { uint32: c } }
}

const TRIANGLE_WGSL: &str = "
@vertex fn vertex_main(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vid], 0.0, 1.0);
}
";

// ---------------------------------------------------------------------------
// MRT.

const MRT_WGSL: &str = "
struct Out3 {
    @location(0) c0: vec4<f32>,
    @location(1) c1: vec2<f32>,
    @location(2) c2: u32,
};
@fragment fn fragment_main() -> Out3 {
    var o: Out3;
    o.c0 = vec4<f32>(0.25, 0.5, 0.75, 0.5);
    o.c1 = vec2<f32>(-2.5, 0.125);
    o.c2 = 0xDEADBEEFu;
    return o;
}
";

const ONE_WGSL: &str = "
@fragment fn fragment_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }
";

#[test]
fn mrt_blend_state_follows_format_and_declared_outputs() {
    let formats = [vk::Format::R16G16B16A16_SFLOAT, vk::Format::R16G16_SFLOAT, vk::Format::R32_UINT, vk::Format::B8G8R8A8_UNORM, vk::Format::R32G32B32A32_SFLOAT];
    let blend = mrt_blend_attachments(&formats, 0b01111, true, false);
    let enabled: Vec<bool> = blend.iter().map(|b| b.blend_enable == vk::TRUE).collect();
    assert_eq!(enabled, [true, false, false, true, false]);
    let masks: Vec<bool> = blend.iter().map(|b| !b.color_write_mask.is_empty()).collect();
    assert_eq!(masks, [true, true, true, true, false], "an undeclared output has an empty write mask");
    let opaque = mrt_blend_attachments(&formats, 0b11111, false, false);
    assert!(opaque.iter().all(|b| b.blend_enable == vk::FALSE));
    // The texture pixel to Vulkan format mapping of the new formats.
    assert_eq!(CxVulkan::vk_color_format_from_texture_pixel(TexturePixel::RGf16), Some(vk::Format::R16G16_SFLOAT));
    assert_eq!(CxVulkan::vk_color_format_from_texture_pixel(TexturePixel::Ru32), Some(vk::Format::R32_UINT));
}

#[test]
fn mrt_attachments_get_their_own_formats_blend_and_write_masks() {
    let Some(gpu) = Gpu::new() else { return };
    let formats = [
        CxVulkan::vk_color_format_from_texture_pixel(TexturePixel::RGBAf16).unwrap(),
        CxVulkan::vk_color_format_from_texture_pixel(TexturePixel::RGf16).unwrap(),
        CxVulkan::vk_color_format_from_texture_pixel(TexturePixel::Ru32).unwrap(),
    ];
    assert_eq!(formats, [vk::Format::R16G16B16A16_SFLOAT, vk::Format::R16G16_SFLOAT, vk::Format::R32_UINT]);
    assert!(gpu.format_features(formats[0]).contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND), "RGBA16F must be blendable for the over blend");
    let targets = formats.map(|f| gpu.target(f, 2));
    let refs: Vec<&Target> = targets.iter().collect();
    let layout = unsafe { gpu.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default(), None).unwrap() };
    let clear_pass = gpu.render_pass(&refs, false);
    let load_pass = gpu.render_pass(&refs, true);
    let (three, _) = (compile_wgsl_to_spirv(MRT_WGSL).unwrap().1.unwrap(), ());
    let one = compile_wgsl_to_spirv(ONE_WGSL).unwrap().1.unwrap();

    // Blending on: only the RGBA16F attachment blends; the RG16F and R32Uint
    // ones write raw. Outputs 0..2 are declared.
    let three_pipe = gpu.pipeline(clear_pass, layout, &three, &mrt_blend_attachments(&formats, 0b111, true, false), 2);
    // One output declared (bit 0), no blending: the other attachments keep
    // their contents.
    let one_pipe = gpu.pipeline(load_pass, layout, &one, &mrt_blend_attachments(&formats, 0b001, false, false), 2);

    // Pass 1: clear c0 to blue, then the three-output shader blends over it.
    gpu.draw(&refs, &[clear_f([0.0, 0.0, 1.0, 1.0]), clear_f([7.0, 7.0, 0.0, 0.0]), clear_u([0; 4])], three_pipe, clear_pass, layout, None);
    assert_eq!(halves(&gpu.read(&targets[0], 8))[..4], [0.25, 0.5, 1.25, 1.0], "premultiplied over onto the blue clear");
    assert_eq!(halves(&gpu.read(&targets[1], 4))[..2], [-2.5, 0.125]);
    let c2 = gpu.read(&targets[2], 4);
    assert_eq!(u32::from_le_bytes([c2[0], c2[1], c2[2], c2[3]]), 0xDEADBEEF);

    // Pass 2 (load): a one-output shader writes c0 only.
    gpu.draw(&refs, &[], one_pipe, load_pass, layout, None);
    assert_eq!(halves(&gpu.read(&targets[0], 8))[..4], [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(halves(&gpu.read(&targets[1], 4))[..2], [-2.5, 0.125], "unwritten attachment kept");
    let c2 = gpu.read(&targets[2], 4);
    assert_eq!(u32::from_le_bytes([c2[0], c2[1], c2[2], c2[3]]), 0xDEADBEEF, "unwritten attachment kept");
    gpu.finish();
}

// ---------------------------------------------------------------------------
// Hostile shaders.

/// Compile a Splash draw shader (one `vec4f` output, `u_n` a uniform the GPU
/// reads as 0) to the WGSL the Vulkan backend feeds naga.
fn splash_wgsl(fragment_body: &str, extra: &str) -> String {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{{\n\
         vertex_pos: shader.vertex_position(vec4f)\n\
         pixel: shader.fragment_output(0, vec4f)\n\
         u_n: shader.uniform(0.0)\n\
         {extra}\n\
         vertex: fn() {{ self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0) }}\n\
         fragment: fn() {{\n{fragment_body}\n}}\n}}"
    );
    let value = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod { file: "vulkan_gpu_tests".into(), code, ..Default::default() }));
    let io_self = value.as_object().expect("shader object");
    // The layout compile the renderer does first (GLSL backend, Vulkan IO).
    let mut layout = ShaderOutput::default();
    layout.backend = ShaderBackend::Glsl;
    layout.use_vulkan = true;
    layout.pre_collect_rust_instance_io(&mut vm, io_self);
    layout.pre_collect_shader_io(&mut vm, io_self);
    for (entry, mode) in [(id!(vertex), ShaderMode::Vertex), (id!(fragment), ShaderMode::Fragment)] {
        let fnobj = vm.bx.heap.object_method(io_self, entry.into(), NoTrap).as_object().expect("entry");
        layout.mode = mode;
        ShaderFnCompiler::compile_shader_def(&mut vm, &mut layout, NoTrap, entry, fnobj, ShaderType::IoSelf(io_self), vec![]);
    }
    assert!(!layout.has_errors, "{}", layout.error_report());
    layout.assign_uniform_buffer_indices(&vm.bx.heap, 3);
    let wgsl = crate::makepad_script::shader_wgsl::compile_draw_shader_wgsl_source(&mut vm, io_self, &layout, false).expect("WGSL").wgsl;
    // The pass uniforms every draw shader declares (`draw.DrawPassUniforms`,
    // registered by the draw crate); this bare VM has no such type, so the
    // block the emitted helpers read is declared here.
    let pass = "struct TestPassUniforms { camera_projection: mat4x4f, camera_view: mat4x4f, depth_projection: mat4x4f, depth_view: mat4x4f, camera_inv: mat4x4f }\n\
                @group(0) @binding(20) var<uniform> unibuf_draw_pass: TestPassUniforms;\n";
    format!("{pass}{wgsl}")
}

/// Run a Splash fragment on a 4x4 RGBA32Float target: the pixel it wrote and
/// the GPU time.
fn run_splash(gpu: &Gpu, fragment_body: &str, extra: &str) -> ([f32; 4], Duration) {
    let wgsl = splash_wgsl(fragment_body, extra);
    let (_, fragment) = compile_wgsl_to_spirv(&wgsl).unwrap_or_else(|e| panic!("{e}\n{wgsl}"));
    let fragment = fragment.expect("fragment_main");
    // One zeroed buffer per binding the module declares (the uniforms all
    // read as 0).
    let module = naga::front::wgsl::parse_str(&wgsl).unwrap();
    let mut bindings = Vec::new();
    for (_, g) in module.global_variables.iter() {
        let Some(b) = &g.binding else { continue };
        let ty = match g.space {
            naga::AddressSpace::Uniform => vk::DescriptorType::UNIFORM_BUFFER,
            naga::AddressSpace::Storage { .. } => vk::DescriptorType::STORAGE_BUFFER,
            // Textures and samplers (the XR depth texture is always declared):
            // no shader here samples one, so the entry point does not use it.
            naga::AddressSpace::Handle => continue,
            other => panic!("the hostile suite does not bind {other:?} (binding {}, {:?}, {:?})", b.binding, g.name, module.types[g.ty]),
        };
        bindings.push((b.binding, ty));
    }
    unsafe {
        let layout_bindings: Vec<_> = bindings
            .iter()
            .map(|(b, ty)| vk::DescriptorSetLayoutBinding::default().binding(*b).descriptor_type(*ty).descriptor_count(1).stage_flags(vk::ShaderStageFlags::ALL_GRAPHICS))
            .collect();
        let set_layout = gpu.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&layout_bindings), None).unwrap();
        let set_layouts = [set_layout];
        let layout = gpu.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts), None).unwrap();
        let pool_sizes = [
            vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_BUFFER, descriptor_count: 16 },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 16 },
        ];
        let pool = gpu.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&pool_sizes), None).unwrap();
        let set = gpu.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&set_layouts)).unwrap()[0];
        let mut buffers = Vec::new();
        let mut infos = Vec::new();
        for (_, ty) in &bindings {
            let usage = if *ty == vk::DescriptorType::UNIFORM_BUFFER { vk::BufferUsageFlags::UNIFORM_BUFFER } else { vk::BufferUsageFlags::STORAGE_BUFFER };
            let (buffer, memory) = gpu.host_buffer(16384, usage);
            buffers.push((buffer, memory));
            infos.push([vk::DescriptorBufferInfo { buffer, offset: 0, range: 16384 }]);
        }
        let writes: Vec<_> = bindings
            .iter()
            .zip(infos.iter())
            .map(|((b, ty), info)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*b).descriptor_type(*ty).buffer_info(info))
            .collect();
        gpu.device.update_descriptor_sets(&writes, &[]);

        let target = gpu.target(vk::Format::R32G32B32A32_SFLOAT, 4);
        let refs = [&target];
        let pass = gpu.render_pass(&refs, false);
        let pipeline = gpu.pipeline(pass, layout, &fragment, &mrt_blend_attachments(&[target.format], 1, false, false), 4);
        let time = gpu.draw(&refs, &[clear_f([0.0; 4])], pipeline, pass, layout, Some(set));
        let px = floats(&gpu.read(&target, 16));
        ([px[0], px[1], px[2], px[3]], time)
    }
}

#[test]
fn hostile_indices_stay_in_bounds_on_the_gpu() {
    let Some(gpu) = Gpu::new() else { return };
    // i = 1000000, j = -5, k = 7u: every access is clamped into range.
    let body = "var arr = array(1f, 2f, 3f, 4f)\n\
                let i = int(self.u_n) + 1000000\n\
                let j = int(self.u_n) - 5\n\
                let a = arr[i]\n\
                arr[i] = 9f\n\
                arr[j] += 10f\n\
                var v = vec3(5f, 6f, 7f)\n\
                let k = uint(self.u_n) + 7u\n\
                v[k] *= 2f\n\
                self.pixel = vec4(a, arr[0], arr[3], v[k])";
    let (px, _) = run_splash(&gpu, body, "");
    assert_eq!(px, [4.0, 11.0, 9.0, 14.0]);
    gpu.finish();
}

#[test]
fn runaway_nested_loops_finish_within_the_budget_on_the_gpu() {
    let Some(gpu) = Gpu::new() else { return };
    let huge = "uint(self.u_n) + 4000000000u";
    let budget = crate::makepad_script::shader_control::SHADER_ITERATION_BUDGET as f32;
    // Two nested runtime loops of ~4e9 each: 1.6e19 passes unguarded.
    let (px, time) = run_splash(&gpu, &format!("var s = 0f\nfor i in 0..{huge} {{ for j in 0..{huge} {{ s += 1f }} }}\nself.pixel = vec4(s, 0.0, 0.0, 1.0)"), "");
    assert!(px[0] > 1000.0 && px[0] <= budget, "{px:?}");
    assert!(time < Duration::from_secs(10), "{time:?}");
    eprintln!("nested runtime loops: {} passes in {time:?}", px[0]);
    // A loop that never breaks, and one called from inside another loop.
    let (px, time) = run_splash(&gpu, "var s = 0f\nloop { s += 1f }\nself.pixel = vec4(s, 0.0, 0.0, 1.0)", "");
    assert_eq!(px[0], crate::makepad_script::shader_control::LOOP_GUARD_MAX_ITERS as f32);
    assert!(time < Duration::from_secs(10), "{time:?}");
    eprintln!("endless loop: {} passes in {time:?}", px[0]);
    let extra = "spin: fn() { var t = 0f\n loop { t += 1f }\n return t }";
    let (px, time) = run_splash(&gpu, &format!("var s = 0f\nfor i in 0..{huge} {{ s += self.spin() }}\nself.pixel = vec4(s, 0.0, 0.0, 1.0)"), extra);
    assert!(px[0] > 1000.0 && px[0] <= budget + 65536.0, "{px:?}");
    assert!(time < Duration::from_secs(10), "{time:?}");
    eprintln!("spinning callee: {} passes in {time:?}", px[0]);
    gpu.finish();
}

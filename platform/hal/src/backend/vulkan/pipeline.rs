//! SPIR-V modules, descriptor layouts and render-pass-compatible pipeline variants. Vertex
//! fetch and instance storage match native vulkan.rs, including packed geometry word lanes.

use super::device::{error, lock, Device, VideoSampler};
use super::resources::vk_format;
use crate::spirv::SpirvPipeline;
use crate::types::*;
use makepad_vulkan::vk;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, OnceLock};

pub(super) const ABSENT: u32 = u32::MAX;
/// A video texture slot's shape (`SpirvPipeline::textures`): a combined image sampler.
pub(super) const VIDEO_SHAPE: u32 = 6;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(super) struct PassKey { pub colors: [u32; 4], pub depth: u32, pub load: [u8; 5], /** 1, or 2: a multiview pass */ pub views: u8, /** a fragment density map attachment after the others */ pub fdm: bool,
    /** samples per pixel: 1, or an MSAA pass (`Pass::samples`) rendering into multisampled attachments resolved into the pass's own at its end */ pub samples: u8 }

pub(super) struct RenderPass { pub dev: Arc<Device>, pub raw: vk::RenderPass, pub key: PassKey }

impl RenderPass {
    pub fn new(dev: Arc<Device>, key: PassKey) -> Result<Arc<Self>, String> {
        if key.samples > 1 { return Self::multisampled(dev, key); }
        let mut attachments = Vec::new();
        let mut colors = Vec::new();
        let count = key.colors.iter().rposition(|f| *f != ABSENT).map_or(0, |i| i + 1);
        let load = |i: usize| match key.load[i] {
            pass_load::LOAD => vk::AttachmentLoadOp::LOAD,
            pass_load::CLEAR => vk::AttachmentLoadOp::CLEAR,
            _ => vk::AttachmentLoadOp::DONT_CARE,
        };
        for i in 0..count {
            if key.colors[i] == ABSENT {
                // VK_ATTACHMENT_UNUSED is the all-ones attachment index.
                colors.push(vk::AttachmentReference::default().attachment(u32::MAX));
                continue;
            }
            colors.push(vk::AttachmentReference::default().attachment(attachments.len() as u32).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL));
            attachments.push(vk::AttachmentDescription::default().format(vk_format(key.colors[i]).ok_or("unknown color format")?)
                .samples(vk::SampleCountFlags::TYPE_1).load_op(load(i)).store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE).stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL).final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL));
        }
        let depth = vk::AttachmentReference::default().attachment(attachments.len() as u32).layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        if key.depth != ABSENT {
            attachments.push(vk::AttachmentDescription::default().format(vk_format(key.depth).ok_or("unknown depth format")?)
                .samples(vk::SampleCountFlags::TYPE_1).load_op(load(4)).store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(load(4)).stencil_store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL).final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL));
        }
        let mut subpass = vk::SubpassDescription::default().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&colors);
        if key.depth != ABSENT { subpass = subpass.depth_stencil_attachment(&depth); }
        let subpasses = [subpass];
        // the fragment density map (an XR runtime's foveation): read only, in its own layout
        let mut density = vk::RenderPassFragmentDensityMapCreateInfoEXT::default()
            .fragment_density_map_attachment(vk::AttachmentReference::default().attachment(attachments.len() as u32).layout(vk::ImageLayout::FRAGMENT_DENSITY_MAP_OPTIMAL_EXT));
        if key.fdm {
            attachments.push(vk::AttachmentDescription::default().format(vk::Format::R8G8_UNORM).samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::DONT_CARE).store_op(vk::AttachmentStoreOp::DONT_CARE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE).stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::FRAGMENT_DENSITY_MAP_OPTIMAL_EXT).final_layout(vk::ImageLayout::FRAGMENT_DENSITY_MAP_OPTIMAL_EXT));
        }
        let mut info = vk::RenderPassCreateInfo::default().attachments(&attachments).subpasses(&subpasses);
        if key.fdm { info = info.next_render_pass_fragment_density_map_create_info_ext(&mut density); }
        // a multiview pass renders every view's layer (view mask: the low `views` bits); the
        // views are correlated (the eyes see nearly the same scene)
        let mask = [(1u32 << key.views.max(1)) - 1];
        let mut multiview = vk::RenderPassMultiviewCreateInfo { subpass_count: 1, p_view_masks: mask.as_ptr(), correlation_mask_count: 1,
            p_correlation_masks: mask.as_ptr(), ..Default::default() };
        if key.views > 1 { info = info.next_render_pass_multiview_create_info(&mut multiview); }
        let raw = unsafe { dev.raw.create_render_pass(&info, None) }.map_err(error)?;
        Ok(Arc::new(Self { dev, raw, key }))
    }
}

impl RenderPass {
    /// An MSAA pass (`key.samples` > 1, `VK_KHR_create_renderpass2` + `VK_KHR_depth_stencil_resolve`):
    /// the attachments are the multisampled colours, their resolve targets (the pass's own
    /// colour textures), the multisampled depth and its resolve target (the pass's depth texture,
    /// sample 0 of each pixel). The multisampled images load as the pass says and are stored
    /// (a pass that loads continues them: the second eye of a side-by-side target, a profiler's
    /// split); the resolve targets are written over in the render area only.
    fn multisampled(dev: Arc<Device>, key: PassKey) -> Result<Arc<Self>, String> {
        if key.fdm { return Err("an MSAA pass with a fragment density map".into()); }
        let samples = vk::SampleCountFlags(key.samples as u32);
        let load = |i: usize| match key.load[i] {
            pass_load::LOAD => vk::AttachmentLoadOp::LOAD,
            pass_load::CLEAR => vk::AttachmentLoadOp::CLEAR,
            _ => vk::AttachmentLoadOp::DONT_CARE,
        };
        let count = key.colors.iter().rposition(|f| *f != ABSENT).map_or(0, |i| i + 1);
        let used: Vec<usize> = (0..count).filter(|i| key.colors[*i] != ABSENT).collect();
        let mut attachments = Vec::new();
        let colour = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
        for i in &used {
            let fmt = vk_format(key.colors[*i]).ok_or("unknown color format")?;
            attachments.push(vk::AttachmentDescription2::default().format(fmt).samples(samples).load_op(load(*i)).store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE).stencil_store_op(vk::AttachmentStoreOp::DONT_CARE).initial_layout(colour).final_layout(colour));
        }
        for i in &used {
            let fmt = vk_format(key.colors[*i]).ok_or("unknown color format")?;
            attachments.push(vk::AttachmentDescription2::default().format(fmt).samples(vk::SampleCountFlags::TYPE_1).load_op(vk::AttachmentLoadOp::DONT_CARE)
                .store_op(vk::AttachmentStoreOp::STORE).stencil_load_op(vk::AttachmentLoadOp::DONT_CARE).stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(colour).final_layout(colour));
        }
        // the colour references in slot order (an unused slot: VK_ATTACHMENT_UNUSED), and their resolves
        let unused = vk::AttachmentReference2::default().attachment(u32::MAX).aspect_mask(vk::ImageAspectFlags::COLOR);
        let mut colors = Vec::new();
        let mut resolves = Vec::new();
        for i in 0..count {
            match used.iter().position(|u| *u == i) {
                Some(k) => {
                    colors.push(vk::AttachmentReference2::default().attachment(k as u32).layout(colour).aspect_mask(vk::ImageAspectFlags::COLOR));
                    resolves.push(vk::AttachmentReference2::default().attachment((used.len() + k) as u32).layout(colour).aspect_mask(vk::ImageAspectFlags::COLOR));
                }
                None => { colors.push(unused); resolves.push(unused); }
            }
        }
        let depth_layout = vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
        let depth_at = attachments.len() as u32;
        let depth_aspect = if key.depth == crate::types::format::DEPTH24_STENCIL8 { vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL } else { vk::ImageAspectFlags::DEPTH };
        if key.depth != ABSENT {
            let fmt = vk_format(key.depth).ok_or("unknown depth format")?;
            attachments.push(vk::AttachmentDescription2::default().format(fmt).samples(samples).load_op(load(4)).store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(load(4)).stencil_store_op(vk::AttachmentStoreOp::STORE).initial_layout(depth_layout).final_layout(depth_layout));
            attachments.push(vk::AttachmentDescription2::default().format(fmt).samples(vk::SampleCountFlags::TYPE_1).load_op(vk::AttachmentLoadOp::DONT_CARE)
                .store_op(vk::AttachmentStoreOp::STORE).stencil_load_op(vk::AttachmentLoadOp::DONT_CARE).stencil_store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(depth_layout).final_layout(depth_layout));
        }
        let depth = vk::AttachmentReference2::default().attachment(depth_at).layout(depth_layout).aspect_mask(depth_aspect);
        let depth_resolve = vk::AttachmentReference2::default().attachment(depth_at + 1).layout(depth_layout).aspect_mask(depth_aspect);
        // depth: sample 0 of each pixel (every device's mode)
        let mut resolve = vk::SubpassDescriptionDepthStencilResolve::default().depth_resolve_mode(vk::ResolveModeFlags::SAMPLE_ZERO)
            .stencil_resolve_mode(if depth_aspect.contains(vk::ImageAspectFlags::STENCIL) { vk::ResolveModeFlags::SAMPLE_ZERO } else { vk::ResolveModeFlags::empty() })
            .depth_stencil_resolve_attachment(&depth_resolve);
        let mask = (1u32 << key.views.max(1)) - 1;
        let mut subpass = vk::SubpassDescription2::default().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&colors).resolve_attachments(&resolves)
            .view_mask(if key.views > 1 { mask } else { 0 });
        if key.depth != ABSENT { subpass = subpass.depth_stencil_attachment(&depth).next_subpass_description_depth_stencil_resolve(&mut resolve); }
        let subpasses = [subpass];
        let masks = [mask];
        let mut info = vk::RenderPassCreateInfo2::default().attachments(&attachments).subpasses(&subpasses);
        if key.views > 1 { info = info.correlated_view_masks(&masks); }
        let raw = unsafe { dev.raw.create_render_pass2(&info, None) }.map_err(error)?;
        Ok(Arc::new(Self { dev, raw, key }))
    }
}

impl Drop for RenderPass { fn drop(&mut self) { unsafe { self.dev.raw.destroy_render_pass(self.raw, None); } } }

pub(super) struct Program {
    pub dev: Arc<Device>,
    pub desc: PipelineDesc,
    pub code: SpirvPipeline,
    pub bindings: Vec<u32>,
    pub vertex_stride: u32,
    pub layout: vk::PipelineLayout,
    pub descriptors: vk::DescriptorSetLayout,
    pub descriptor_types: Vec<(u32, vk::DescriptorType)>,
    /// The bindings fed from the batch's upload arena (the uniform blocks, the pick block and
    /// the instance storage), by binding number: dynamic, so one written set serves every draw
    /// with the same buffers and textures and a draw binds it with its own offsets.
    pub dynamic: Vec<u32>,
    pub samplers: Vec<vk::Sampler>,
    /// The immutable samplers of its video texture slots (shape 6), in slot order: the layout
    /// was made with them (`State::select_video` picks the program whose samplers are the
    /// bound images').
    pub video: Vec<VideoSampler>,
    vertex: vk::ShaderModule,
    fragment: vk::ShaderModule,
    /// the multiview stages (null without them)
    xr_vertex: vk::ShaderModule,
    xr_fragment: vk::ShaderModule,
}

impl Program {
    /// `video`: the samplers of its video slots (`None`: an ordinary image's for each).
    pub fn new(dev: Arc<Device>, desc: PipelineDesc, code: SpirvPipeline, bindings: Vec<u32>, vertex_stride: u32, video: Option<&[VideoSampler]>) -> Result<Arc<Self>, String> {
        if bindings.len() < pipeline_binding::BUFFERS || bindings.len() > pipeline_binding::BUFFERS + 4
            || bindings[pipeline_binding::TEXTURES] as usize != code.textures.len() || vertex_stride % 4 != 0 {
            return Err("SPIR-V pipeline binding/layout mismatch".into());
        }
        let slots = code.textures.iter().filter(|t| **t == VIDEO_SHAPE).count();
        let video = match video {
            Some(v) if v.len() == slots => v.to_vec(),
            Some(_) => return Err("video samplers for a different number of video slots".into()),
            None if slots == 0 => Vec::new(),
            None => vec![dev.video_sampler(0, None)?; slots],
        };
        let mut out = Self { dev, desc, code, bindings, vertex_stride, layout: vk::PipelineLayout::null(),
            descriptors: vk::DescriptorSetLayout::null(), descriptor_types: Vec::new(), dynamic: Vec::new(), samplers: Vec::new(), video,
            vertex: vk::ShaderModule::null(), fragment: vk::ShaderModule::null(),
            xr_vertex: vk::ShaderModule::null(), xr_fragment: vk::ShaderModule::null() };
        for (i, slot) in out.bindings.iter().enumerate() {
            if i != pipeline_binding::TEXTURES && *slot != pipeline_binding::NONE {
                // the fixed blocks come from the arena; custom uniform buffers are bound whole
                let ty = if i < pipeline_binding::BUFFERS { vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC } else { vk::DescriptorType::UNIFORM_BUFFER };
                out.descriptor_types.push((*slot, ty));
            }
        }
        for i in 0..out.code.textures.len() as u32 {
            // a video slot is a combined image sampler (its sampler immutable in the layout)
            let ty = if out.code.textures[i as usize] == VIDEO_SHAPE { vk::DescriptorType::COMBINED_IMAGE_SAMPLER } else { vk::DescriptorType::SAMPLED_IMAGE };
            out.descriptor_types.push((out.code.texture_base.checked_add(i).ok_or("texture binding overflow")?, ty));
            if out.code.pick_texture_base != ABSENT {
                out.descriptor_types.push((out.code.pick_texture_base.checked_add(i).ok_or("pick binding overflow")?, vk::DescriptorType::SAMPLED_IMAGE));
            }
        }
        for i in 0..out.code.samplers.len() as u32 {
            out.descriptor_types.push((out.code.sampler_base.checked_add(i).ok_or("sampler binding overflow")?, vk::DescriptorType::SAMPLER));
        }
        if out.code.depth_binding != ABSENT { out.descriptor_types.push((out.code.depth_binding, vk::DescriptorType::SAMPLED_IMAGE)); }
        if out.code.instance_binding != ABSENT { out.descriptor_types.push((out.code.instance_binding, vk::DescriptorType::STORAGE_BUFFER_DYNAMIC)); }
        if out.code.pick_binding != ABSENT { out.descriptor_types.push((out.code.pick_binding, vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)); }
        out.descriptor_types.sort_by_key(|b| b.0);
        out.dynamic = out.descriptor_types.iter().filter(|(_, ty)| matches!(*ty, vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC | vk::DescriptorType::STORAGE_BUFFER_DYNAMIC))
            .map(|(slot, _)| *slot).collect();
        if out.descriptor_types.windows(2).any(|p| p[0].0 == p[1].0) { return Err("overlapping Vulkan descriptor bindings".into()); }
        // every binding is visible to both stages: the per-stage limits apply to the whole set (a
        // driver that is not told refuses the pipeline with an unexplained error: PowerVR, ERROR_UNKNOWN)
        let limits = &out.dev.properties.limits;
        let count = |t: &[vk::DescriptorType]| out.descriptor_types.iter().filter(|(_, ty)| t.contains(ty)).count() as u32;
        let (uniform, storage) = ([vk::DescriptorType::UNIFORM_BUFFER, vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC], [vk::DescriptorType::STORAGE_BUFFER, vk::DescriptorType::STORAGE_BUFFER_DYNAMIC]);
        for (t, max, what) in [(&[vk::DescriptorType::SAMPLED_IMAGE][..], limits.max_per_stage_descriptor_sampled_images, "sampled images"),
            (&[vk::DescriptorType::SAMPLER][..], limits.max_per_stage_descriptor_samplers, "samplers"),
            (&uniform[..], limits.max_per_stage_descriptor_uniform_buffers, "uniform buffers"),
            (&storage[..], limits.max_per_stage_descriptor_storage_buffers, "storage buffers"),
            (&uniform[1..], limits.max_descriptor_set_uniform_buffers_dynamic, "dynamic uniform buffers"),
            (&storage[1..], limits.max_descriptor_set_storage_buffers_dynamic, "dynamic storage buffers")] {
            let n = count(t);
            if n > max { return Err(format!("the shader binds {n} {what} per stage, the device allows {max}")); }
        }
        let total = out.descriptor_types.len() as u32;
        if total > limits.max_per_stage_resources { return Err(format!("the shader binds {total} resources per stage, the device allows {}", limits.max_per_stage_resources)); }
        // the video slots' samplers, one array each (the layout binding points at it)
        let video_slots: Vec<u32> = (0..out.code.textures.len() as u32).filter(|i| out.code.textures[*i as usize] == VIDEO_SHAPE).map(|i| out.code.texture_base + i).collect();
        let immutable: Vec<[vk::Sampler; 1]> = out.video.iter().map(|v| [v.sampler]).collect();
        let bindings: Vec<_> = out.descriptor_types.iter().map(|(slot, ty)| {
            let b = vk::DescriptorSetLayoutBinding::default().binding(*slot).descriptor_type(*ty).descriptor_count(1).stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT);
            match video_slots.iter().position(|v| v == slot) {
                Some(k) if *ty == vk::DescriptorType::COMBINED_IMAGE_SAMPLER => b.immutable_samplers(&immutable[k]),
                _ => b,
            }
        }).collect();
        unsafe {
            out.descriptors = out.dev.raw.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None).map_err(error)?;
            let layouts = [out.descriptors];
            out.layout = out.dev.raw.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts), None).map_err(error)?;
            out.vertex = out.dev.raw.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&out.code.vertex), None).map_err(error)?;
            out.fragment = out.dev.raw.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&out.code.fragment), None).map_err(error)?;
            if out.dev.multiview && !out.code.xr_vertex.is_empty() {
                out.xr_vertex = out.dev.raw.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&out.code.xr_vertex), None).map_err(error)?;
                out.xr_fragment = out.dev.raw.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&out.code.xr_fragment), None).map_err(error)?;
            }
            for s in &out.code.samplers {
                let filter = if s.filter == 0 { vk::Filter::NEAREST } else { vk::Filter::LINEAR };
                let address = match s.address { 0 => vk::SamplerAddressMode::REPEAT, 1 => vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    2 => vk::SamplerAddressMode::CLAMP_TO_BORDER, _ => vk::SamplerAddressMode::MIRRORED_REPEAT };
                if s.pixel && (s.compare || !matches!(s.address, 1 | 2)) { return Err("invalid unnormalized sampler".into()); }
                out.samplers.push(out.dev.raw.create_sampler(&vk::SamplerCreateInfo::default().mag_filter(filter).min_filter(filter)
                    .mipmap_mode(if s.pixel { vk::SamplerMipmapMode::NEAREST } else { vk::SamplerMipmapMode::LINEAR })
                    .address_mode_u(address).address_mode_v(address).address_mode_w(address)
                    .border_color(vk::BorderColor::FLOAT_TRANSPARENT_BLACK).unnormalized_coordinates(s.pixel)
                    .compare_enable(s.compare).compare_op(vk::CompareOp::LESS_OR_EQUAL).min_lod(0.0)
                    .max_lod(if s.pixel { 0.0 } else { vk::LOD_CLAMP_NONE }), None).map_err(error)?);
            }
        }
        Ok(Arc::new(out))
    }
}

impl Drop for Program {
    fn drop(&mut self) { unsafe {
        for s in self.samplers.drain(..) { self.dev.raw.destroy_sampler(s, None); }
        self.dev.raw.destroy_shader_module(self.vertex, None);
        self.dev.raw.destroy_shader_module(self.fragment, None);
        if self.xr_vertex != vk::ShaderModule::null() { self.dev.raw.destroy_shader_module(self.xr_vertex, None); }
        if self.xr_fragment != vk::ShaderModule::null() { self.dev.raw.destroy_shader_module(self.xr_fragment, None); }
        self.dev.raw.destroy_pipeline_layout(self.layout, None);
        self.dev.raw.destroy_descriptor_set_layout(self.descriptors, None);
    } }
}

pub(super) struct Pipeline { pub program: Arc<Program>, pub raw: vk::Pipeline }
impl Drop for Pipeline { fn drop(&mut self) { unsafe { self.program.dev.raw.destroy_pipeline(self.raw, None); } } }

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
/// A pipeline's render pass compatibility (formats, views, samples per pixel: a pipeline made
/// for an MSAA pass rasterises at its sample count and must never draw in another) and its
/// draw state.
struct VariantKey { colors: [u32; 4], depth: u32, flags: u32, views: u8, samples: u8 }

impl VariantKey {
    fn new(pass: PassKey, flags: u32) -> Self {
        let mut flags = flags & (Draw::BLEND | Draw::CULL | Draw::DEPTH_WRITE | Draw::WIREFRAME);
        if pass.depth == ABSENT { flags &= !Draw::DEPTH_WRITE; } // no depth attachment: the bit changes nothing
        Self { colors: pass.colors, depth: pass.depth, flags, views: pass.views, samples: pass.samples.max(1) }
    }
}

/// A driver call failed for want of host or device memory (as formatted by `device::error`):
/// it may succeed once there is room, so it is not a final failure.
pub(super) fn out_of_memory(e: &str) -> bool { e.contains("OUT_OF_DEVICE_MEMORY") || e.contains("OUT_OF_HOST_MEMORY") }

/// A pipeline's create, as the app sent it: the program is made from it (again on a device that
/// replaced a lost one). `key` names identical creates, which share one program.
pub(super) struct Source {
    pub desc: PipelineDesc,
    pub code: SpirvPipeline,
    pub bindings: Vec<u32>,
    pub stride: u32,
    pub key: Vec<u8>,
}

/// A program and the pipelines made from it. The likeliest pipeline (`predict`) is built with
/// the program; any other (a target layout plus the blend, depth-write and cull state) when a
/// draw first needs it (`select`). Only what is drawn with gets compiled (a driver pipeline
/// compile is milliseconds and tens of thousands of allocations on NVIDIA).
pub(super) struct State {
    /// what the program is made from: compiled again on a replacement device
    pub src: Arc<Source>,
    pub program: OnceLock<Result<Arc<Program>, String>>,
    variants: Mutex<HashMap<VariantKey, Result<Arc<Pipeline>, String>>>,
    /// Programs for video slots bound to other samplers than an ordinary image's (a decoder's
    /// pictures), and their pipelines; made at the first draw that needs them.
    video: Mutex<HashMap<Vec<VideoSampler>, Result<Arc<Program>, String>>>,
    video_variants: Mutex<HashMap<(Vec<VideoSampler>, VariantKey), Result<Arc<Pipeline>, String>>>,
    pub epoch: AtomicU64,
    /// the sessions whose pipelines wait for this program while it compiles (programs are shared
    /// by shader content, so every session that created a pipeline with it is told); `None` once
    /// the compile has answered
    waiters: Mutex<Option<Vec<u32>>>,
}

impl State {
    pub fn new(src: Arc<Source>) -> Arc<Self> {
        Arc::new(Self { src, program: OnceLock::new(), variants: Mutex::new(HashMap::new()), video: Mutex::new(HashMap::new()), video_variants: Mutex::new(HashMap::new()),
            epoch: AtomicU64::new(0), waiters: Mutex::new(Some(Vec::new())) })
    }

    /// Session `sid` has a pipeline on this program: its pump is woken when the compile answers.
    /// A program that has already answered needs no wake (the session's own turn sees it).
    pub fn wait(&self, sid: u32) {
        if let Some(waiters) = self.waiters.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            if !waiters.contains(&sid) { waiters.push(sid); }
        }
    }

    /// The compile answered: the sessions that waited for it.
    pub fn answered(&self) -> Vec<u32> {
        self.waiters.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default()
    }

    /// Builds, with the program and before it is announced ready, the pipeline its draws most
    /// often need: its own colour layout over a depth target, writing depth, blending into a
    /// colour format (scene, UI and shadow-value passes). That is the first pipeline of about
    /// two thirds of the programs an app draws with, and nearly every program draws with one
    /// pipeline only; those draws are complete in the first frame after PipelineReady. A
    /// raw-write program (pick ids, data passes) is not guessed: its layout is the pass's.
    pub fn predict(&self, program: &Arc<Program>) {
        if program.desc.blend & pipeline_blend::RAW != 0 { return; }
        let mut colors = [ABSENT; 4];
        for i in 0..4 { if program.desc.outputs & (1 << i) != 0 { colors[i] = program.desc.color_formats[i]; } }
        let key = PassKey { colors, depth: format::DEPTH32F, load: [pass_load::DONT_CARE; 5], views: 1, fdm: false, samples: 1 };
        // a data target (R32F depth/shadow values, ids) is drawn without blending
        let blends = matches!(colors[0], format::BGRA8 | format::RGBA8 | format::RGBA16F);
        let flags = Draw::DEPTH_WRITE | if blends { Draw::BLEND } else { 0 };
        let made = RenderPass::new(program.dev.clone(), key).and_then(|pass| create_variant(program.clone(), &pass, flags));
        if let Err(e) = &made { eprintln!("[hal-vulkan] pipeline variant: {e}"); }
        // out of memory is not this pipeline's failure: the first draw that needs it tries again
        if made.as_ref().is_err_and(|e| out_of_memory(e)) { return; }
        lock(&self.variants).insert(VariantKey::new(key, flags), made);
    }

    /// Whether a draw into a pass of `key` with `flags` would compile its pipeline now: the
    /// program is built and that variant is not.
    pub fn lacks(&self, key: PassKey, flags: u32) -> bool {
        matches!(self.program.get(), Some(Ok(_))) && !lock(&self.variants).contains_key(&VariantKey::new(key, flags))
    }

    pub fn failure(&self) -> Option<String> {
        if let Some(Err(e)) = self.program.get() { return Some(e.clone()); }
        lock(&self.variants).values().find_map(|made| made.as_ref().err().cloned())
    }

    /// The pipeline for a draw into `pass` with `flags`. One the program was not built with is
    /// compiled here, before the frame is encoded: the frame waits for one driver compile
    /// rather than showing without the draw. None while the program itself compiles (the app
    /// paints again at PipelineReady) or when the pipeline failed.
    pub fn select(&self, pass: &RenderPass, flags: u32) -> Option<Arc<Pipeline>> {
        let program = self.program.get()?.as_ref().ok()?.clone();
        let key = VariantKey::new(pass.key, flags);
        if let Some(made) = lock(&self.variants).get(&key) { return made.as_ref().ok().cloned(); }
        let made = create_variant(program, pass, flags);
        if let Err(e) = &made { eprintln!("[hal-vulkan] pipeline variant: {e}"); }
        // out of memory is not kept: the next draw tries again (the frame skips it meanwhile)
        if made.as_ref().is_err_and(|e| out_of_memory(e)) { return None; }
        lock(&self.variants).entry(key).or_insert(made).as_ref().ok().cloned()
    }
}

impl State {
    /// [`State::select`] for a draw whose video slots are bound to images needing `video`
    /// samplers: the program made with those samplers (compiled here, at the first draw that
    /// needs it, as an unknown pass variant is) and its pipeline for `pass` and `flags`.
    pub fn select_video(&self, pass: &RenderPass, flags: u32, video: &[VideoSampler]) -> Option<Arc<Pipeline>> {
        let base = self.program.get()?.as_ref().ok()?.clone();
        if base.video == video { return self.select(pass, flags); }
        let program = {
            let mut programs = lock(&self.video);
            programs.entry(video.to_vec()).or_insert_with(|| {
                let made = Program::new(base.dev.clone(), self.src.desc, self.src.code.clone(), self.src.bindings.clone(), self.src.stride, Some(video));
                if let Err(e) = &made { eprintln!("[hal-vulkan] video pipeline: {e}"); }
                made
            }).as_ref().ok()?.clone()
        };
        let key = (video.to_vec(), VariantKey::new(pass.key, flags));
        if let Some(made) = lock(&self.video_variants).get(&key) { return made.as_ref().ok().cloned(); }
        let made = create_variant(program, pass, flags);
        if let Err(e) = &made { eprintln!("[hal-vulkan] video pipeline variant: {e}"); }
        if made.as_ref().is_err_and(|e| out_of_memory(e)) { return None; }
        lock(&self.video_variants).entry(key).or_insert(made).as_ref().ok().cloned()
    }
}

fn create_variant(program: Arc<Program>, pass: &RenderPass, flags: u32) -> Result<Arc<Pipeline>, String> {
    let dev = &program.dev;
    // a multiview pass runs the multiview stages (each view's matrices by view index)
    let (vertex_module, fragment_module) = if pass.key.views > 1 {
        if program.xr_vertex == vk::ShaderModule::null() { return Err("the shader has no multiview stages for a multiview pass".into()); }
        (program.xr_vertex, program.xr_fragment)
    } else { (program.vertex, program.fragment) };
    let stages = [vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vertex_module).name(c"vertex_main"),
        vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fragment_module).name(c"fragment_main")];
    let words = program.vertex_stride / 4;
    let bindings = [vk::VertexInputBindingDescription::default().binding(0).stride(program.vertex_stride).input_rate(vk::VertexInputRate::VERTEX)];
    let attrs: Vec<_> = (0..words.div_ceil(4)).map(|i| vk::VertexInputAttributeDescription::default().location(i).binding(0).offset(i * 16)
        .format(match (words - i * 4).min(4) { 1 => vk::Format::R32_SFLOAT, 2 => vk::Format::R32G32_SFLOAT,
            3 => vk::Format::R32G32B32_SFLOAT, _ => vk::Format::R32G32B32A32_SFLOAT })).collect();
    let vertex = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(if words == 0 { &[] } else { &bindings }).vertex_attribute_descriptions(&attrs);
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
    let lines = flags & Draw::WIREFRAME != 0 && program.dev.features.fill_mode_non_solid == vk::TRUE;
    let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(if lines { vk::PolygonMode::LINE } else { vk::PolygonMode::FILL })
        .cull_mode(if flags & Draw::CULL != 0 { vk::CullModeFlags::BACK } else { vk::CullModeFlags::NONE })
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE).line_width(1.0);
    // an MSAA pass: its sample count, per-sample shading off (one fragment shader run per pixel);
    // an opaque draw's alpha is its coverage of the pixel (alpha to coverage: a cut-out's edge,
    // `cut_coverage` in the scene shaders), stored as 1 where the device can (alpha to one)
    let msaa = pass.key.samples > 1;
    let a2c = msaa && flags & Draw::BLEND == 0 && program.desc.blend & pipeline_blend::RAW == 0;
    let samples = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags(pass.key.samples.max(1) as u32))
        .alpha_to_coverage_enable(a2c).alpha_to_one_enable(a2c && dev.features.alpha_to_one == vk::TRUE);
    let count = pass.key.colors.iter().rposition(|f| *f != ABSENT).map_or(0, |i| i + 1);
    let attachments: Vec<_> = (0..count).map(|i| {
        let writes = program.desc.outputs & (1 << i) != 0 && pass.key.colors[i] != ABSENT;
        let blend = writes && flags & Draw::BLEND != 0 && program.desc.blend & pipeline_blend::RAW == 0 && pass.key.colors[i] != format::R32UINT;
        let op = if program.desc.blend & pipeline_blend::MAX != 0 { vk::BlendOp::MAX } else { vk::BlendOp::ADD };
        vk::PipelineColorBlendAttachmentState::default().blend_enable(blend)
            .src_color_blend_factor(vk::BlendFactor::ONE).dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA).color_blend_op(op)
            .src_alpha_blend_factor(vk::BlendFactor::ONE).dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA).alpha_blend_op(op)
            .color_write_mask(if writes { vk::ColorComponentFlags::R | vk::ColorComponentFlags::G | vk::ColorComponentFlags::B | vk::ColorComponentFlags::A } else { vk::ColorComponentFlags::empty() })
    }).collect();
    if dev.features.independent_blend == vk::FALSE && attachments.windows(2).any(|a| a[0].blend_enable != a[1].blend_enable || a[0].color_write_mask != a[1].color_write_mask) {
        return Err("device lacks independent blend for this MRT layout".into());
    }
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let depth = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(pass.key.depth != ABSENT)
        .depth_write_enable(pass.key.depth != ABSENT && flags & Draw::DEPTH_WRITE != 0).depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let info = vk::GraphicsPipelineCreateInfo::default().stages(&stages).vertex_input_state(&vertex).input_assembly_state(&assembly)
        .viewport_state(&viewport).rasterization_state(&raster).multisample_state(&samples).depth_stencil_state(&depth)
        .color_blend_state(&blend).dynamic_state(&dynamic).layout(program.layout).render_pass(pass.raw).subpass(0);
    let raw = match unsafe { dev.raw.create_graphics_pipelines(dev.pipeline_cache, &[info], None) } {
        Ok(p) => {
            dev.pipelines_made.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            dev.pipeline_made_ns.store(crate::clock_ns(), std::sync::atomic::Ordering::Relaxed);
            p[0]
        }
        Err((partial, e)) => { for p in partial { unsafe { dev.raw.destroy_pipeline(p, None); } } return Err(error(e)); }
    };
    Ok(Arc::new(Pipeline { program, raw }))
}

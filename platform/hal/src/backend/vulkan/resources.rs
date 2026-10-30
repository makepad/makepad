//! Dedicated image memory and host-visible buffer memory, ported from Vulkan's native
//! allocation paths. Arc leases, held by each submission, retire storage after its fence.

use super::device::{error, lock, Device};
use crate::session::TextureObj;
use crate::types::{format, usage};
use makepad_vulkan::vk;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

pub fn vk_format(f: u32) -> Option<vk::Format> {
    Some(match f {
        format::RGBA8 => vk::Format::R8G8B8A8_UNORM,
        format::RGBA8_SRGB => vk::Format::R8G8B8A8_SRGB,
        format::BGRA8 => vk::Format::B8G8R8A8_UNORM,
        format::R8 => vk::Format::R8_UNORM,
        format::RG8 => vk::Format::R8G8_UNORM,
        format::RGBA16F => vk::Format::R16G16B16A16_SFLOAT,
        format::RGBA32F => vk::Format::R32G32B32A32_SFLOAT,
        format::DEPTH32F => vk::Format::D32_SFLOAT,
        format::DEPTH24_STENCIL8 => vk::Format::D24_UNORM_S8_UINT,
        format::R32F => vk::Format::R32_SFLOAT,
        format::RG16F => vk::Format::R16G16_SFLOAT,
        format::R16F => vk::Format::R16_SFLOAT,
        format::R32UINT => vk::Format::R32_UINT,
        format::BC7 => vk::Format::BC7_UNORM_BLOCK,
        format::BC5 => vk::Format::BC5_UNORM_BLOCK,
        format::ASTC4X4 => vk::Format::ASTC_4X4_UNORM_BLOCK,
        _ => return None,
    })
}

/// The fragment density maps of render targets another owner made (an XR runtime's foveated
/// swapchain images: their density map image's view, by the target's raw image): a pass into
/// such a target shades coarser where its map says ([`set_density_map`]).
static DENSITY_MAPS: Mutex<Vec<(u64, vk::ImageView)>> = Mutex::new(Vec::new());

/// Target `image` renders with the fragment density map `view` (a 2D array view, a layer per
/// view of a multiview pass); `None`: without one again. The view's owner keeps it alive while
/// registered.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn set_density_map(image: vk::Image, view: Option<vk::ImageView>) {
    let mut maps = DENSITY_MAPS.lock().unwrap_or_else(|e| e.into_inner());
    maps.retain(|(i, _)| *i != image.as_raw());
    if let Some(v) = view { maps.push((image.as_raw(), v)); }
}

/// The density map registered for `image`, if any.
pub(super) fn density_map(image: vk::Image) -> Option<vk::ImageView> {
    DENSITY_MAPS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(i, _)| *i == image.as_raw()).map(|(_, v)| *v)
}

pub fn image_usage(t: &TextureObj) -> vk::ImageUsageFlags {
    let mut flags = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED;
    // a depth image is sampled in DEPTH_STENCIL_READ_ONLY layout, which needs the attachment usage too
    if format::is_depth(t.format) { flags |= vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT; }
    else if t.usage & usage::RENDER_TARGET != 0 { flags |= vk::ImageUsageFlags::COLOR_ATTACHMENT; }
    flags
}

/// All fields except the recording layout are immutable. Only the serialized HAL encoder
/// changes that layout. Worker threads merely retain the image until the GPU finishes.
pub struct Image {
    pub dev: Arc<Device>,
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    array_view: vk::ImageView,
    cube_view: vk::ImageView,
    pub layers: Vec<vk::ImageView>,
    pub desc: TextureObj,
    pub bytes: u64,
    pub aspect: vk::ImageAspectFlags,
    layout: Mutex<vk::ImageLayout>,
    pub(super) external_owner: AtomicBool,
    /// Made and kept by another owner (an XR runtime's swapchain image): never destroyed here.
    borrowed: bool,
    /// A decoder's external-format picture: the video sampler its views were made with (the
    /// sampler a video texture slot needs for it); the default (null) for an ordinary image.
    pub video: super::device::VideoSampler,
    /// The external allocation/handle owner, if any (released after image and memory).
    pub external: Option<Arc<dyn std::any::Any + Send + Sync>>,
    #[cfg(target_os = "linux")]
    pub(crate) timeline: Option<Arc<super::share::sync::Timeline>>,
}

impl Image {
    /// XRGB external buffers have undefined alpha. Sampling them must still be opaque.
    #[cfg(target_os = "linux")]
    pub(super) fn sample_alpha_one(&mut self) -> Result<(), String> {
        let info = vk::ImageViewCreateInfo::default().image(self.image)
            .format(vk_format(self.desc.format).ok_or("unknown image format")?)
            .components(vk::ComponentMapping::default().a(vk::ComponentSwizzle::from_raw(2))) // VK_COMPONENT_SWIZZLE_ONE
            .view_type(vk::ImageViewType::TYPE_2D_ARRAY).subresource_range(vk::ImageSubresourceRange::default()
                .aspect_mask(self.aspect).level_count(self.desc.mips).layer_count(self.desc.depth));
        unsafe {
            let array = self.dev.raw.create_image_view(&info, None).map_err(error)?;
            let view = match self.dev.raw.create_image_view(&info.view_type(vk::ImageViewType::TYPE_2D)
                .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(self.aspect).level_count(self.desc.mips).layer_count(1)), None) {
                Ok(view) => view,
                Err(e) => { self.dev.raw.destroy_image_view(array, None); return Err(error(e)); }
            };
            self.dev.raw.destroy_image_view(std::mem::replace(&mut self.view, view), None);
            self.dev.raw.destroy_image_view(std::mem::replace(&mut self.array_view, array), None);
        }
        Ok(())
    }
    pub fn new(dev: Arc<Device>, desc: TextureObj) -> Result<Arc<Self>, String> {
        let usage = image_usage(&desc);
        Self::new_with(dev, desc, vk::SampleCountFlags::TYPE_1, usage)
    }

    /// A multisampled render target of `samples` per pixel shaped like `like` (format, size,
    /// layers): an MSAA pass renders into it and resolves into `like`'s image. Only an
    /// attachment, never sampled or copied.
    pub fn multisampled(dev: Arc<Device>, like: &TextureObj, samples: u32) -> Result<Arc<Self>, String> {
        let desc = TextureObj { mips: 1, lent: None, imported: false, ..like.clone() };
        let usage = if format::is_depth(desc.format) { vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT } else { vk::ImageUsageFlags::COLOR_ATTACHMENT };
        Self::new_with(dev, desc, vk::SampleCountFlags(samples), usage)
    }

    fn new_with(dev: Arc<Device>, desc: TextureObj, samples: vk::SampleCountFlags, usage: vk::ImageUsageFlags) -> Result<Arc<Self>, String> {
        let fmt = vk_format(desc.format).ok_or("unknown texture format")?;
        if desc.w == 0 || desc.h == 0 || desc.depth == 0 || desc.mips == 0
            || desc.mips > 32 - desc.w.max(desc.h).leading_zeros()
            || desc.w > dev.properties.limits.max_image_dimension2_d || desc.h > dev.properties.limits.max_image_dimension2_d
            || desc.depth > dev.properties.limits.max_image_array_layers {
            return Err("invalid texture dimensions or mip count".into());
        }
        let features = dev.formats[desc.format as usize].optimal_tiling_features;
        let mut needed = if samples == vk::SampleCountFlags::TYPE_1 { vk::FormatFeatureFlags::SAMPLED_IMAGE } else { vk::FormatFeatureFlags::empty() };
        if format::is_depth(desc.format) { needed |= vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT; }
        if desc.usage & usage::RENDER_TARGET != 0 {
            if !format::is_depth(desc.format) { needed |= vk::FormatFeatureFlags::COLOR_ATTACHMENT; }
            if desc.mips > 1 && !format::is_depth(desc.format) { needed |= vk::FormatFeatureFlags::BLIT_SRC | vk::FormatFeatureFlags::BLIT_DST; }
        }
        if !features.contains(needed) { return Err(format!("texture format {} lacks device features {needed:?}", desc.format)); }
        let info = vk::ImageCreateInfo::default()
            .flags(if desc.is_cube() { vk::ImageCreateFlags::CUBE_COMPATIBLE } else { vk::ImageCreateFlags::empty() })
            .image_type(vk::ImageType::TYPE_2D).format(fmt)
            .extent(vk::Extent3D { width: desc.w, height: desc.h, depth: 1 })
            .mip_levels(desc.mips).array_layers(desc.depth).samples(samples)
            .tiling(vk::ImageTiling::OPTIMAL).usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE).initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { dev.raw.create_image(&info, None) }.map_err(error)?;
        let req = unsafe { dev.raw.get_image_memory_requirements(image) };
        let memory = (|| {
            let ty = dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                .or_else(|_| dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::empty()))?;
            unsafe { dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None) }.map_err(error)
        })();
        let memory = match memory {
            Ok(m) => m,
            Err(e) => { unsafe { dev.raw.destroy_image(image, None); } return Err(e); }
        };
        if let Err(e) = unsafe { dev.raw.bind_image_memory(image, memory, 0) } {
            unsafe { dev.raw.destroy_image(image, None); dev.raw.free_memory(memory, None); }
            return Err(error(e));
        }
        // from_bound owns both handles even if a view cannot be created.
        unsafe { Self::from_bound(dev, desc, image, memory, req.size, vk::ImageLayout::UNDEFINED, None) }
    }

    /// Adopt a bound image and its dedicated allocation. The caller transfers both handles;
    /// no pending commands may use them except through the returned lease.
    pub unsafe fn from_bound(dev: Arc<Device>, desc: TextureObj, image: vk::Image, memory: vk::DeviceMemory,
        bytes: u64, layout: vk::ImageLayout, external: Option<Arc<dyn std::any::Any + Send + Sync>>) -> Result<Arc<Self>, String> {
        Self::adopt(dev, desc, image, memory, bytes, layout, external, false)
    }

    /// An image another owner made and keeps (an XR runtime's swapchain image), in `layout`
    /// now: views of it in `desc`'s format (the image must allow that format), never destroyed
    /// here.
    pub unsafe fn borrowed(dev: Arc<Device>, desc: TextureObj, image: vk::Image, layout: vk::ImageLayout) -> Result<Arc<Self>, String> {
        Self::adopt(dev, desc, image, vk::DeviceMemory::null(), 0, layout, None, true)
    }

    /// A depth image another owner made and keeps (an XR runtime's environment depth swapchain
    /// image, in its depth read-only layout), of a format the HAL has no texture format for
    /// (`format`): `desc` names a depth stand-in (DEPTH32F) for the bookkeeping, its views are
    /// `format`'s. Sampled as a plain 2D array it reads the depth in .r; never destroyed here.
    pub unsafe fn borrowed_depth(dev: Arc<Device>, desc: TextureObj, image: vk::Image, format: vk::Format) -> Result<Arc<Self>, String> {
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::DEPTH).level_count(1).layer_count(desc.depth);
        let info = vk::ImageViewCreateInfo::default().image(image).format(format).view_type(vk::ImageViewType::TYPE_2D_ARRAY).subresource_range(range);
        let array_view = dev.raw.create_image_view(&info, None).map_err(error)?;
        let view = match dev.raw.create_image_view(&info.view_type(vk::ImageViewType::TYPE_2D).subresource_range(range.layer_count(1)), None) {
            Ok(v) => v,
            Err(e) => { dev.raw.destroy_image_view(array_view, None); return Err(error(e)); }
        };
        Ok(Arc::new(Self { dev, image, memory: vk::DeviceMemory::null(), view, array_view, cube_view: vk::ImageView::null(), layers: Vec::new(), desc, bytes: 0,
            aspect: vk::ImageAspectFlags::DEPTH, layout: Mutex::new(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL), external_owner: AtomicBool::new(false), borrowed: true,
            video: Default::default(), external: None, #[cfg(target_os = "linux")] timeline: None }))
    }

    /// A bound external-format image (a decoder's picture) sampled through `video`'s Y'CbCr
    /// conversion: its views carry the conversion.
    pub unsafe fn external_video(dev: Arc<Device>, desc: TextureObj, image: vk::Image, memory: vk::DeviceMemory, video: super::device::VideoSampler,
        external: Option<Arc<dyn std::any::Any + Send + Sync>>) -> Result<Arc<Self>, String> {
        let mut conversion = vk::SamplerYcbcrConversionInfo::default().conversion(video.conversion);
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let info = vk::ImageViewCreateInfo::default().next_sampler_ycbcr_conversion_info(&mut conversion).image(image).format(vk::Format::UNDEFINED)
            .view_type(vk::ImageViewType::TYPE_2D).subresource_range(range);
        let view = dev.raw.create_image_view(&info, None).map_err(error)?;
        Ok(Arc::new(Self { dev, image, memory, view, array_view: vk::ImageView::null(), cube_view: vk::ImageView::null(), layers: Vec::new(), desc, bytes: 0,
            // (the decoder's contents, acquired from the foreign queue in GENERAL: an acquire from
            // UNDEFINED lets the driver drop them; a compressed (UBWC) picture read so was noise)
            aspect: vk::ImageAspectFlags::COLOR, layout: Mutex::new(vk::ImageLayout::GENERAL), external_owner: AtomicBool::new(false), borrowed: false, video, external,
            #[cfg(target_os = "linux")] timeline: None }))
    }

    /// The layout the image is in now, as its owner says (an acquired swapchain image).
    pub fn set_layout(&self, layout: vk::ImageLayout) {
        *lock(&self.layout) = layout;
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn adopt(dev: Arc<Device>, desc: TextureObj, image: vk::Image, memory: vk::DeviceMemory,
        bytes: u64, layout: vk::ImageLayout, external: Option<Arc<dyn std::any::Any + Send + Sync>>, borrowed: bool) -> Result<Arc<Self>, String> {
        let aspect = if desc.format == format::DEPTH24_STENCIL8 { vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL }
            else if format::is_depth(desc.format) { vk::ImageAspectFlags::DEPTH } else { vk::ImageAspectFlags::COLOR };
        dev.charge(bytes);
        let mut out = Self { dev, image, memory, view: vk::ImageView::null(), array_view: vk::ImageView::null(), cube_view: vk::ImageView::null(),
            layers: Vec::new(), desc, bytes, aspect, layout: Mutex::new(layout), external_owner: AtomicBool::new(false), borrowed, video: Default::default(), external,
            #[cfg(target_os = "linux")] timeline: None };
        let sampled_aspect = if format::is_depth(out.desc.format) { vk::ImageAspectFlags::DEPTH } else { aspect };
        let info = vk::ImageViewCreateInfo::default().image(image).format(vk_format(out.desc.format).ok_or("unknown image format")?)
            .view_type(vk::ImageViewType::TYPE_2D_ARRAY).subresource_range(vk::ImageSubresourceRange::default()
                .aspect_mask(sampled_aspect).level_count(out.desc.mips).layer_count(out.desc.depth));
        out.array_view = out.dev.raw.create_image_view(&info, None).map_err(error)?;
        out.view = out.dev.raw.create_image_view(&info.view_type(vk::ImageViewType::TYPE_2D)
            .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(sampled_aspect).level_count(out.desc.mips).layer_count(1)), None).map_err(error)?;
        if out.desc.is_cube() {
            out.cube_view = out.dev.raw.create_image_view(&info.view_type(vk::ImageViewType::CUBE), None).map_err(error)?;
        }
        if out.desc.usage & usage::RENDER_TARGET != 0 {
            for layer in 0..out.desc.depth {
                out.layers.push(out.dev.raw.create_image_view(&info.view_type(vk::ImageViewType::TYPE_2D)
                    .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(aspect)
                        .level_count(1).base_array_layer(layer).layer_count(1)), None).map_err(error)?);
            }
        }
        Ok(Arc::new(out))
    }

    pub(super) fn transition(&self, cb: vk::CommandBuffer, to: vk::ImageLayout) {
        let old = std::mem::replace(&mut *lock(&self.layout), to);
        let foreign = self.external_owner.swap(false, Ordering::AcqRel);
        let barrier = vk::ImageMemoryBarrier::default().image(self.image).old_layout(old).new_layout(to)
            .src_queue_family_index(if foreign { self.foreign_family() } else { vk::QUEUE_FAMILY_IGNORED })
            .dst_queue_family_index(if foreign { self.dev.family } else { vk::QUEUE_FAMILY_IGNORED })
            .src_access_mask(if foreign || old == vk::ImageLayout::UNDEFINED { vk::AccessFlags::empty() } else { vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE })
            .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
            .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(self.aspect).level_count(self.desc.mips).layer_count(self.desc.depth));
        unsafe { self.dev.raw.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(), &[], &[], &[barrier]); }
    }

    /// Readies the image for a draw that samples it. A barrier is recorded only when its layout
    /// changes (or another queue family owns it): every write to an image (render pass,
    /// copy, mip chain) leaves it in its sampled layout through a barrier of its own, which
    /// already orders the reads that follow on this queue; a read after a read needs none. A
    /// barrier per sampled texture per draw was a full pipeline drain for each one (a scene of
    /// 470 draws recorded 8418 of them: 20 ms in vkQueueSubmit on NVIDIA, and GPU stalls).
    pub(super) fn to_sampled(&self, cb: vk::CommandBuffer) {
        let to = self.sampled_layout();
        if *lock(&self.layout) == to && !self.external_owner.load(Ordering::Acquire) {
            return;
        }
        self.transition(cb, to);
    }

    pub(super) fn supports_transfer(&self) -> bool {
        #[cfg(target_os = "linux")]
        if self.external.as_ref().and_then(|e| e.downcast_ref::<super::NativeHandle>()).is_some_and(|n| n.sample_only) {
            return false;
        }
        true
    }

    pub(super) fn sampled_layout(&self) -> vk::ImageLayout {
        if format::is_depth(self.desc.format) { vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL } else { vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL }
    }

    pub(super) fn sample_view(&self, shape: u32) -> Result<vk::ImageView, String> {
        // A depth target sampled as a plain 2D/array texture reads its depth in .r (what SSAO
        // and soft particles do with the scene depth, as on Metal, D3D11 and GL); only a depth
        // shape (a comparison) needs a depth image.
        let depth = format::is_depth(self.desc.format);
        if (matches!(shape, 3 | 4) && !depth) || (matches!(shape, 1 | 5 | 6) && depth) || (shape == 5) != (self.desc.format == format::R32UINT)
            || (self.video.conversion != vk::SamplerYcbcrConversion::null() && shape != 6) {
            return Err("sampled image format differs from shader texture type".into());
        }
        match shape {
            0 | 3 | 5 | 6 => Ok(self.view),
            2 | 4 => Ok(self.array_view),
            1 if self.cube_view != vk::ImageView::null() => Ok(self.cube_view),
            _ => Err("sampled image has no matching cube/array view".into()),
        }
    }

    /// The view of every array layer (one mip): a multiview pass's attachment.
    pub(super) fn array_view(&self) -> vk::ImageView { self.array_view }

    pub(super) fn release_external(&self, cb: vk::CommandBuffer) {
        self.transition(cb, vk::ImageLayout::GENERAL);
        let barrier = vk::ImageMemoryBarrier::default().image(self.image)
            .old_layout(vk::ImageLayout::GENERAL).new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(self.dev.family).dst_queue_family_index(self.foreign_family())
            .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
            .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(self.aspect).level_count(self.desc.mips).layer_count(self.desc.depth));
        unsafe { self.dev.raw.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::DependencyFlags::empty(), &[], &[], &[barrier]); }
        self.external_owner.store(true, Ordering::Release);
    }

    fn foreign_family(&self) -> u32 {
        #[cfg(target_os = "linux")]
        if self.external.as_ref().and_then(|e| e.downcast_ref::<super::NativeHandle>()).is_some_and(|n| n.opaque.is_some()) {
            return vk::QUEUE_FAMILY_EXTERNAL;
        }
        vk::QUEUE_FAMILY_FOREIGN_EXT
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            for view in self.layers.drain(..) { self.dev.raw.destroy_image_view(view, None); }
            if self.view != vk::ImageView::null() { self.dev.raw.destroy_image_view(self.view, None); }
            if self.array_view != vk::ImageView::null() { self.dev.raw.destroy_image_view(self.array_view, None); }
            if self.cube_view != vk::ImageView::null() { self.dev.raw.destroy_image_view(self.cube_view, None); }
            if !self.borrowed {
                self.dev.raw.destroy_image(self.image, None);
                self.dev.raw.free_memory(self.memory, None);
            }
        }
        self.dev.uncharge(self.bytes);
    }
}

pub(crate) struct Buffer {
    pub dev: Arc<Device>,
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: u64,
    pub bytes: u64,
    mapped: usize,
    coherent: bool,
}

impl Buffer {
    /// A zeroed buffer: a retained buffer is read where the app has not written yet.
    pub fn new(dev: Arc<Device>, size: u64, usage: vk::BufferUsageFlags) -> Result<Arc<Self>, String> {
        let out = Self::scratch(dev, size, usage)?;
        unsafe { std::ptr::write_bytes(out.mapped as *mut u8, 0, out.size.max(16) as usize); }
        out.flush()?;
        Ok(out)
    }

    /// A buffer whose bytes are undefined until written: upload arenas and readback staging,
    /// which are read only where they were written (by the CPU or a GPU copy).
    pub fn scratch(dev: Arc<Device>, size: u64, usage: vk::BufferUsageFlags) -> Result<Arc<Self>, String> {
        let logical_size = size;
        let size = size.max(16);
        let buffer = unsafe { dev.raw.create_buffer(&vk::BufferCreateInfo::default().size(size).usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE), None) }.map_err(error)?;
        let req = unsafe { dev.raw.get_buffer_memory_requirements(buffer) };
        let result = (|| {
            // a buffer the GPU writes for the CPU to read (a readback) is CPU-cached where the
            // device has such memory: reading uncached (write-combined) memory runs at a few
            // hundred MB/s, a frame's readback took ~90 ms of the copy-out alone
            let readback = usage.contains(vk::BufferUsageFlags::TRANSFER_DST) && !usage.contains(vk::BufferUsageFlags::TRANSFER_SRC);
            let cached = if readback { dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_CACHED).ok() } else { None };
            let ty = match cached {
                Some(ty) => ty,
                None => dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)
                    .or_else(|_| dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE))?,
            };
            let memory = unsafe { dev.raw.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None) }.map_err(error)?;
            Ok::<_, String>((memory, ty))
        })();
        let (memory, ty) = match result { Ok(v) => v, Err(e) => { unsafe { dev.raw.destroy_buffer(buffer, None); } return Err(e); } };
        dev.charge(req.size);
        let mut out = Self { dev, buffer, memory, size: logical_size, bytes: req.size, mapped: 0,
            coherent: false };
        out.coherent = out.dev.memory.memory_types[ty as usize].property_flags.contains(vk::MemoryPropertyFlags::HOST_COHERENT);
        unsafe {
            out.dev.raw.bind_buffer_memory(buffer, memory, 0).map_err(error)?;
            out.mapped = out.dev.raw.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()).map_err(error)? as usize;
        }
        Ok(Arc::new(out))
    }

    /// Only the HAL encoder writes, to fresh storage or an unleased retained buffer.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<(), String> {
        self.write_unflushed(offset, data)?;
        self.flush()
    }

    pub(super) fn write_unflushed(&self, offset: u64, data: &[u8]) -> Result<(), String> {
        if offset.checked_add(data.len() as u64).is_none_or(|end| end > self.size) { return Err("buffer write exceeds storage".into()); }
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), (self.mapped as *mut u8).add(offset as usize), data.len()); }
        Ok(())
    }

    pub(super) fn flush(&self) -> Result<(), String> {
        if !self.coherent {
            unsafe { self.dev.raw.flush_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(self.memory).size(vk::WHOLE_SIZE)]) }.map_err(error)?;
        }
        Ok(())
    }

    pub fn read(&self, length: usize) -> Result<Vec<u8>, String> {
        if length as u64 > self.size { return Err("read exceeds staging buffer".into()); }
        if !self.coherent {
            unsafe { self.dev.raw.invalidate_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(self.memory).size(vk::WHOLE_SIZE)]) }.map_err(error)?;
        }
        Ok(unsafe { std::slice::from_raw_parts(self.mapped as *const u8, length) }.to_vec())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn invalidate(&self) -> Result<(), String> {
        if !self.coherent {
            unsafe { self.dev.raw.invalidate_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(self.memory).size(vk::WHOLE_SIZE)]) }.map_err(error)?;
        }
        Ok(())
    }

    /// Caller holds the completed allocation exclusively against writers for the slice's life.
    #[cfg(target_os = "linux")]
    pub(super) unsafe fn completed_bytes(&self, length: usize) -> &[u8] {
        std::slice::from_raw_parts(self.mapped as *const u8, length.min(self.size as usize))
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            if self.mapped != 0 { self.dev.raw.unmap_memory(self.memory); }
            self.dev.raw.destroy_buffer(self.buffer, None);
            self.dev.raw.free_memory(self.memory, None);
        }
        self.dev.uncharge(self.bytes);
    }
}

pub(crate) fn buffer_usage() -> vk::BufferUsageFlags {
    vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::UNIFORM_BUFFER
        | vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST
}

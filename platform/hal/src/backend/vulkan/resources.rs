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
        format::BGRA8 => vk::Format::B8G8R8A8_UNORM,
        format::R8 => vk::Format::R8_UNORM,
        format::RG8 => vk::Format::R8G8_UNORM,
        format::RGBA16F => vk::Format::R16G16B16A16_SFLOAT,
        format::RGBA32F => vk::Format::R32G32B32A32_SFLOAT,
        format::DEPTH32F => vk::Format::D32_SFLOAT,
        format::DEPTH24_STENCIL8 => vk::Format::D24_UNORM_S8_UINT,
        format::R32F => vk::Format::R32_SFLOAT,
        format::RG16F => vk::Format::R16G16_SFLOAT,
        format::R32UINT => vk::Format::R32_UINT,
        format::BC7 => vk::Format::BC7_UNORM_BLOCK,
        format::BC5 => vk::Format::BC5_UNORM_BLOCK,
        format::ASTC4X4 => vk::Format::ASTC_4X4_UNORM_BLOCK,
        _ => return None,
    })
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
    /// The external allocation/handle owner, if any (released after image and memory).
    pub external: Option<Arc<dyn std::any::Any + Send + Sync>>,
    #[cfg(target_os = "linux")]
    pub(crate) timeline: Option<Arc<super::share::sync::Timeline>>,
}

impl Image {
    pub fn new(dev: Arc<Device>, desc: TextureObj) -> Result<Arc<Self>, String> {
        let fmt = vk_format(desc.format).ok_or("unknown texture format")?;
        if desc.w == 0 || desc.h == 0 || desc.depth == 0 || desc.mips == 0
            || desc.mips > 32 - desc.w.max(desc.h).leading_zeros()
            || desc.w > dev.properties.limits.max_image_dimension2_d || desc.h > dev.properties.limits.max_image_dimension2_d
            || desc.depth > dev.properties.limits.max_image_array_layers {
            return Err("invalid texture dimensions or mip count".into());
        }
        let features = dev.formats[desc.format as usize].optimal_tiling_features;
        let mut needed = vk::FormatFeatureFlags::SAMPLED_IMAGE;
        if format::is_depth(desc.format) { needed |= vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT; }
        if desc.usage & usage::RENDER_TARGET != 0 {
            if !format::is_depth(desc.format) { needed |= vk::FormatFeatureFlags::COLOR_ATTACHMENT; }
            if desc.mips > 1 && !format::is_depth(desc.format) { needed |= vk::FormatFeatureFlags::BLIT_SRC | vk::FormatFeatureFlags::BLIT_DST; }
        }
        if !features.contains(needed) { return Err(format!("texture format {} lacks device features {needed:?}", desc.format)); }
        let info = vk::ImageCreateInfo::default()
            .flags(if desc.depth == 6 && desc.w == desc.h && !format::is_depth(desc.format) { vk::ImageCreateFlags::CUBE_COMPATIBLE } else { vk::ImageCreateFlags::empty() })
            .image_type(vk::ImageType::TYPE_2D).format(fmt)
            .extent(vk::Extent3D { width: desc.w, height: desc.h, depth: 1 })
            .mip_levels(desc.mips).array_layers(desc.depth).samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL).usage(image_usage(&desc))
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
        let aspect = if desc.format == format::DEPTH24_STENCIL8 { vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL }
            else if format::is_depth(desc.format) { vk::ImageAspectFlags::DEPTH } else { vk::ImageAspectFlags::COLOR };
        dev.charge(bytes);
        let mut out = Self { dev, image, memory, view: vk::ImageView::null(), array_view: vk::ImageView::null(), cube_view: vk::ImageView::null(),
            layers: Vec::new(), desc, bytes, aspect, layout: Mutex::new(layout), external_owner: AtomicBool::new(false), external,
            #[cfg(target_os = "linux")] timeline: None };
        let sampled_aspect = if format::is_depth(out.desc.format) { vk::ImageAspectFlags::DEPTH } else { aspect };
        let info = vk::ImageViewCreateInfo::default().image(image).format(vk_format(out.desc.format).ok_or("unknown image format")?)
            .view_type(vk::ImageViewType::TYPE_2D_ARRAY).subresource_range(vk::ImageSubresourceRange::default()
                .aspect_mask(sampled_aspect).level_count(out.desc.mips).layer_count(out.desc.depth));
        out.array_view = out.dev.raw.create_image_view(&info, None).map_err(error)?;
        out.view = out.dev.raw.create_image_view(&info.view_type(vk::ImageViewType::TYPE_2D)
            .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(sampled_aspect).level_count(out.desc.mips).layer_count(1)), None).map_err(error)?;
        if out.desc.depth == 6 && out.desc.w == out.desc.h && !format::is_depth(out.desc.format) {
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
        if (matches!(shape, 3 | 4) && !depth) || (matches!(shape, 1 | 5) && depth) || (shape == 5) != (self.desc.format == format::R32UINT) {
            return Err("sampled image format differs from shader texture type".into());
        }
        match shape {
            0 | 3 | 5 => Ok(self.view),
            2 | 4 => Ok(self.array_view),
            1 if self.cube_view != vk::ImageView::null() => Ok(self.cube_view),
            _ => Err("sampled image has no matching cube/array view".into()),
        }
    }

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
            self.dev.raw.destroy_image_view(self.array_view, None);
            self.dev.raw.destroy_image_view(self.cube_view, None);
            self.dev.raw.destroy_image(self.image, None);
            self.dev.raw.free_memory(self.memory, None);
        }
        self.dev.uncharge(self.bytes);
    }
}

pub(super) struct Buffer {
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
            let ty = dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)
                .or_else(|_| dev.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE))?;
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

pub(super) fn buffer_usage() -> vk::BufferUsageFlags {
    vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::UNIFORM_BUFFER
        | vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST
}

//! The game's images stay on its GPU. A reusable transfer command copies each
//! submitted eye into a DMA-BUF image on the bridge's GPU, then releases it to
//! foreign ownership. Only the game's render thread waits for this completion.
use crate::{abi::*, protocol};
use super::state::*;
use makepad_vulkan::{vk, Device, Entry, Instance};
use std::{os::fd::{FromRawFd, OwnedFd}, sync::{Arc, Mutex}};

fn error(e: vk::Result) -> Result {
    eprintln!("[makepad-openxr] Vulkan transfer: {e:?}");
    match e { vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => OUT_OF_MEMORY,
        vk::Result::ERROR_DEVICE_LOST => SESSION_LOST, _ => RUNTIME_FAILURE }
}
pub struct Gpu {
    pub instance: Instance,
    pub device: Device,
    pub physical: vk::PhysicalDevice,
    pub family: u32,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    pub max_size: u32,
    commands: Mutex<Commands>,
}
struct Commands { queue: vk::Queue, pool: vk::CommandPool, cb: vk::CommandBuffer, fence: vk::Fence }
pub struct Image {
    gpu: Arc<Gpu>,
    pub image: vk::Image,
    memory: vk::DeviceMemory,
    pub format: vk::Format,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}
pub struct Export {
    pub image: Image,
    pub fd: OwnedFd,
    pub descriptor: protocol::Image,
    pub published: bool,
}
impl Drop for Image {
    fn drop(&mut self) {
        if trace_errors() { eprintln!("[makepad-openxr] destroy image {:#x}", self.image.0); }
        unsafe { self.gpu.device.destroy_image(self.image, None); self.gpu.device.free_memory(self.memory, None); }
    }
}
impl Drop for Gpu {
    fn drop(&mut self) {
        let cmd = lock(&self.commands);
        unsafe { self.device.destroy_fence(cmd.fence, None); self.device.destroy_command_pool(cmd.pool, None); }
    }
}
impl Gpu {
    pub unsafe fn new(binding: &GraphicsBindingVulkan, render_device: u64) -> Xr<Arc<Self>> {
        if binding.instance.is_null() || binding.physical_device.is_null() || binding.device.is_null() { return Err(GRAPHICS_DEVICE_INVALID); }
        let entry = Entry::load().map_err(io_error)?;
        let instance = Instance::from_handle(&entry, vk::Instance(binding.instance as usize));
        let physical = super::core::graphics::select_device(&instance, render_device)?;
        if physical.0 != binding.physical_device as usize { return Err(GRAPHICS_DEVICE_INVALID); }
        let families = instance.get_physical_device_queue_family_properties(physical);
        let family = families.get(binding.queue_family as usize).ok_or(GRAPHICS_DEVICE_INVALID)?;
        if binding.queue_index >= family.queue_count || !family.queue_flags.contains(vk::QueueFlags::GRAPHICS) { return Err(GRAPHICS_DEVICE_INVALID); }
        let device = Device::from_handle(&instance, vk::Device(binding.device as usize));
        if device.table.get_memory_fd_khr.is_none() || device.table.get_image_drm_format_modifier_properties_ext.is_none() { return Err(GRAPHICS_DEVICE_INVALID); }
        let queue = device.get_device_queue(binding.queue_family, binding.queue_index);
        let pool = device.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(binding.queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None).map_err(error)?;
        let result = (|| {
            let cb = device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(error)?[0];
            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None).map_err(error)?;
            let memory = instance.get_physical_device_memory_properties(physical);
            let max_size = instance.get_physical_device_properties(physical).limits.max_image_dimension2_d;
            Ok((cb, fence, memory, max_size))
        })();
        let (cb, fence, memory, max_size) = match result { Ok(v) => v, Err(e) => { device.destroy_command_pool(pool, None); return Err(e); } };
        Ok(Arc::new(Self { instance, device, physical, family: binding.queue_family, memory, max_size,
            commands: Mutex::new(Commands { queue, pool, cb, fence }) }))
    }
    fn memory_type(&self, bits: u32) -> Xr<u32> {
        self.memory.memory_types[..self.memory.memory_type_count as usize].iter().enumerate()
            .find(|(n, t)| bits & (1 << n) != 0 && t.property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                && !t.property_flags.contains(vk::MemoryPropertyFlags::PROTECTED))
            .map(|(n, _)| n as u32).ok_or(OUT_OF_MEMORY)
    }
    unsafe fn allocate(self: &Arc<Self>, info: &vk::ImageCreateInfo, export: bool) -> Xr<Image> {
        let image = self.device.create_image(info, None).map_err(error)?;
        let req = self.device.get_image_memory_requirements(image);
        let result = (|| {
            let ty = self.memory_type(req.memory_type_bits)?;
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut external = vk::ExportMemoryAllocateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let mut info = vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty).next_memory_dedicated_allocate_info(&mut dedicated);
            if export { info = info.next_export_memory_allocate_info(&mut external); }
            let memory = self.device.allocate_memory(&info, None).map_err(error)?;
            if let Err(e) = self.device.bind_image_memory(image, memory, 0) { self.device.free_memory(memory, None); return Err(error(e)); }
            Ok(memory)
        })();
        match result {
            Ok(memory) => Ok(Image { gpu: self.clone(), image, memory, format: info.format, width: info.extent.width, height: info.extent.height, layers: info.array_layers }),
            Err(e) => { self.device.destroy_image(image, None); Err(e) }
        }
    }
    pub unsafe fn image(self: &Arc<Self>, format: vk::Format, width: u32, height: u32, layers: u32, mips: u32, usage: u64) -> Xr<Image> {
        let mut flags = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::COLOR_ATTACHMENT;
        if usage & 4 != 0 { flags |= vk::ImageUsageFlags::from_raw(0x8); }
        if usage & 16 != 0 { flags |= vk::ImageUsageFlags::TRANSFER_DST; }
        if usage & 32 != 0 { flags |= vk::ImageUsageFlags::SAMPLED; }
        if usage & 128 != 0 { flags |= vk::ImageUsageFlags::from_raw(0x80); }
        let info = vk::ImageCreateInfo::default().image_type(vk::ImageType::TYPE_2D).format(format)
            .flags(if usage & 64 != 0 { vk::ImageCreateFlags::MUTABLE_FORMAT } else { vk::ImageCreateFlags::empty() })
            .extent(vk::Extent3D { width, height, depth: 1 }).mip_levels(mips).array_layers(layers).samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL).usage(flags).sharing_mode(vk::SharingMode::EXCLUSIVE).initial_layout(vk::ImageLayout::UNDEFINED);
        if trace_errors() { eprintln!("[makepad-openxr] swapchain {width}x{height}, {layers} layers, {mips} mips, {format:?}, usage {usage:#x}"); }
        let image = self.allocate(&info, false)?;
        if trace_errors() { eprintln!("[makepad-openxr] allocated image {:#x}", image.image.0); }
        self.submit(|cb| {
            let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(mips).layer_count(layers);
            let barrier = vk::ImageMemoryBarrier::default().image(image.image).subresource_range(range)
                .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
            self.device.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::DependencyFlags::empty(), &[], &[], &[barrier]);
        })?;
        Ok(image)
    }
    pub unsafe fn export(self: &Arc<Self>, id: u64, size: [u32; 2], srgb: bool, formats: &[(u32, u64)]) -> Xr<Export> {
        let format = if srgb { vk::Format::B8G8R8A8_SRGB } else { vk::Format::B8G8R8A8_UNORM };
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut props = vk::FormatProperties2::default().next_drm_format_modifier_properties_list_ext(&mut list);
        self.instance.get_physical_device_format_properties2(self.physical, format, &mut props);
        let mut values = vec![vk::DrmFormatModifierPropertiesEXT::default(); list.drm_format_modifier_count as usize];
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default().drm_format_modifier_properties(&mut values);
        let mut props = vk::FormatProperties2::default().next_drm_format_modifier_properties_list_ext(&mut list);
        self.instance.get_physical_device_format_properties2(self.physical, format, &mut props);
        values.truncate(list.drm_format_modifier_count as usize);
        let fourcc = u32::from_le_bytes(*b"AR24");
        for choice in values {
            if choice.drm_format_modifier_plane_count != 1 || !formats.contains(&(fourcc, choice.drm_format_modifier)) { continue; }
            let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let mut drm = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default().drm_format_modifier(choice.drm_format_modifier).sharing_mode(vk::SharingMode::EXCLUSIVE);
            let query = vk::PhysicalDeviceImageFormatInfo2::default().format(format).ty(vk::ImageType::TYPE_2D).tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
                .next_physical_device_external_image_format_info(&mut external).next_physical_device_image_drm_format_modifier_info_ext(&mut drm);
            let mut ext = vk::ExternalImageFormatProperties::default();
            let mut out = vk::ImageFormatProperties2::default().next_external_image_format_properties(&mut ext);
            if self.instance.get_physical_device_image_format_properties2(self.physical, &query, &mut out).is_err()
                || !ext.external_memory_properties.external_memory_features.contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE)
                || size[0] > out.image_format_properties.max_extent.width || size[1] > out.image_format_properties.max_extent.height { continue; }
            let choices = [choice.drm_format_modifier];
            let mut drm = vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&choices);
            let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let info = vk::ImageCreateInfo::default().image_type(vk::ImageType::TYPE_2D).format(format)
                .extent(vk::Extent3D { width: size[0], height: size[1], depth: 1 }).mip_levels(1).array_layers(1).samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT).usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
                .sharing_mode(vk::SharingMode::EXCLUSIVE).initial_layout(vk::ImageLayout::UNDEFINED)
                .next_image_drm_format_modifier_list_create_info_ext(&mut drm).next_external_memory_image_create_info(&mut external);
            let image = self.allocate(&info, true)?;
            let layout = self.device.get_image_subresource_layout(image.image, &vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT));
            let raw = self.device.get_memory_fd(&vk::MemoryGetFdInfoKHR::default().memory(image.memory).handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)).map_err(error)?;
            if raw < 0 { return Err(RUNTIME_FAILURE); }
            let fd = OwnedFd::from_raw_fd(raw);
            let descriptor = protocol::Image { id, width: size[0], height: size[1], fourcc, modifier: choice.drm_format_modifier,
                offset: u32::try_from(layout.offset).map_err(|_| RUNTIME_FAILURE)?, stride: u32::try_from(layout.row_pitch).map_err(|_| RUNTIME_FAILURE)? };
            return Ok(Export { image, fd, descriptor, published: false });
        }
        Err(SWAPCHAIN_FORMAT_UNSUPPORTED)
    }
    unsafe fn submit(&self, record: impl FnOnce(vk::CommandBuffer)) -> Xr<()> {
        let cmd = lock(&self.commands);
        self.device.reset_command_buffer(cmd.cb, vk::CommandBufferResetFlags::empty()).map_err(error)?;
        self.device.reset_fences(&[cmd.fence]).map_err(error)?;
        self.device.begin_command_buffer(cmd.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(error)?;
        record(cmd.cb);
        self.device.end_command_buffer(cmd.cb).map_err(error)?;
        let buffers = [cmd.cb];
        self.device.queue_submit(cmd.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], cmd.fence).map_err(error)?;
        self.device.wait_for_fences(&[cmd.fence], true, u64::MAX).map_err(error)
    }
    pub unsafe fn copy_eye(&self, source: &Image, rect: Rect2i, layer: u32, target: &mut Export) -> Xr<()> {
        let sub = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let src = vk::ImageMemoryBarrier::default().image(source.image).subresource_range(sub.base_array_layer(layer))
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL).new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE).dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        let dst = vk::ImageMemoryBarrier::default().image(target.image.image).subresource_range(sub)
            .src_queue_family_index(if target.published { vk::QUEUE_FAMILY_FOREIGN_EXT } else { vk::QUEUE_FAMILY_IGNORED })
            .dst_queue_family_index(if target.published { self.family } else { vk::QUEUE_FAMILY_IGNORED })
            .old_layout(if target.published { vk::ImageLayout::GENERAL } else { vk::ImageLayout::UNDEFINED })
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL).dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        self.submit(|cb| {
            self.device.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[src, dst]);
            let [x,y] = rect.offset; let [w,h] = rect.extent;
            let slice = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
            let region = vk::ImageBlit::default().src_subresource(slice.base_array_layer(layer))
                .src_offsets([vk::Offset3D { x, y, z: 0 }, vk::Offset3D { x: x+w, y: y+h, z: 1 }])
                .dst_subresource(slice).dst_offsets([vk::Offset3D { x: 0, y: 0, z: 0 }, vk::Offset3D { x: w, y: h, z: 1 }]);
            self.device.cmd_blit_image(cb, source.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, target.image.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region], vk::Filter::NEAREST);
            let restore = src.old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL).new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ).dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
            let release = dst.old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL).new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(self.family).dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::empty());
            self.device.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::ALL_COMMANDS, vk::DependencyFlags::empty(), &[], &[], &[restore, release]);
        })?;
        target.published = true; Ok(())
    }
}

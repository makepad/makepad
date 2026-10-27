//! Environment depth (XR_META_environment_depth) for the app: each display frame the XR thread
//! acquires the runtime's newest depth image. A new one goes to the XR window at once as an
//! `XrDepth` header (no texels) naming the image itself (`texture_handle::XR_DEPTH`: the
//! runtime's swapchain images are lent to the app's GPU work, so its per-pixel tests read the
//! newest image directly, both eyes' layers, with no CPU on the way), and its copy into a
//! host-visible buffer starts; when that copy's fence has signalled (polled, never waited on) the
//! texels follow as a second `XrDepth` with the same header. The app integrates those into its
//! TSDF on its own thread (makepad-tsdf's depth writer).
//!
//! The retired backend read the depth back on its frame thread and waited on the GPU twice a
//! frame for it (`openxr_depth.rs` / `vulkan.rs` `read_openxr_depth_image`); here the copy rides
//! in a slot with its own fence, two slots deep, so a copy still in flight is skipped, not
//! waited for.
use super::sys::*;
use super::log;
use crate::backend::vulkan::Buffer;
use crate::backend::vulkan::Device;
use crate::{XrDepth, XrEye};
use makepad_vulkan::vk;
use std::sync::Arc;

/// The eyes whose depth layers are read back (both: eye 0's first).
const LAYERS: u32 = 2;
const SLOTS: usize = 2;

/// The depth swapchain's images lent to the app's GPU work: their tickets, by chain index.
struct Lent(Vec<u64>);

impl Drop for Lent {
    fn drop(&mut self) {
        for t in &self.0 { crate::backend::vulkan::share::end_lend(*t); }
    }
}

struct DepthSlot {
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    buffer: Arc<Buffer>,
    /// the copy in flight: what it holds
    pending: Option<XrDepth>,
}

pub(super) struct DepthSource {
    images: Vec<vk::Image>,
    /// The image last read back (its chain index and its view's pose): the runtime hands the
    /// same image out again until it has a newer one (it makes them at its own rate, below the
    /// display's), and that one is not read back again.
    last: Option<(u32, [f32; 7])>,
    size: [u32; 2],
    slots: Vec<DepthSlot>,
    pub(super) delivered: u64,
    pub(super) acquired: u64,
    lent: Lent,
}

impl DepthSource {
    /// The depth swapchain's images and the readback slots, and starts the provider. `None`
    /// without a provider (no USE_SCENE permission, or a headset without depth).
    pub(super) fn new(xr: &LibOpenXr, dev: &Arc<Device>, provider: XrEnvironmentDepthProviderMETA, swapchain: XrEnvironmentDepthSwapchainMETA) -> Result<Option<DepthSource>, String> {
        if provider.0 == 0 || swapchain.0 == 0 {
            return Ok(None);
        }
        let mut state = XrEnvironmentDepthSwapchainStateMETA::default();
        unsafe { (xr.xrGetEnvironmentDepthSwapchainStateMETA)(swapchain, &mut state) }.to_result("xrGetEnvironmentDepthSwapchainStateMETA")?;
        let images = xr_array_fetch(XrSwapchainImageVulkanKHR::default(), |cap, len, buf| {
            unsafe { (xr.xrEnumerateEnvironmentDepthSwapchainImagesMETA)(swapchain, cap, len, buf as *mut std::ffi::c_void) }.to_result("xrEnumerateEnvironmentDepthSwapchainImagesMETA")
        })?;
        let images: Vec<vk::Image> = images.iter().map(|i| vk::Image::from_raw(i.image)).collect();
        let size = [state.width, state.height];
        // (each image as a 2-layer depth texture of the runtime's D16 format; a DEPTH32F stand-in names it)
        let desc = crate::session::TextureObj { format: crate::format::DEPTH32F, w: size[0], h: size[1], depth: LAYERS, mips: 1, usage: crate::usage::SAMPLED,
            bytes: 0, imported: true, uploaded: 0, lent: None };
        let lent = Lent(images.iter().map(|i| unsafe { crate::backend::vulkan::Image::borrowed_depth(dev.clone(), desc.clone(), *i, vk::Format::D16_UNORM) }
            .map(crate::backend::vulkan::share::lend_image)).collect::<Result<_, _>>()?);
        let bytes = size[0] as u64 * size[1] as u64 * 2 * LAYERS as u64;
        let mut slots = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let buffer = Buffer::scratch(dev.clone(), bytes, vk::BufferUsageFlags::TRANSFER_DST)?;
            unsafe {
                let pool = dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(dev.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None).map_err(|e| format!("XR depth command pool: {e:?}"))?;
                let cb = dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(|e| format!("XR depth command buffer: {e:?}"))?[0];
                let fence = dev.raw.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None).map_err(|e| format!("XR depth fence: {e:?}"))?;
                slots.push(DepthSlot { pool, cb, fence, buffer, pending: None });
            }
        }
        unsafe { (xr.xrStartEnvironmentDepthProviderMETA)(provider) }.to_result("xrStartEnvironmentDepthProviderMETA")?;
        // the hands stay in the image (occlusion needs them): set again on the started provider
        let keep_hands = XrEnvironmentDepthHandRemovalSetInfoMETA { enabled: XrBool32::from_bool(false), ..Default::default() };
        let r = unsafe { (xr.xrSetEnvironmentDepthHandRemovalMETA)(provider, &keep_hands) };
        xr_log!("environment depth provider {:#x} started; hand removal off: {r:?}", provider.0);
        xr_log!("environment depth: {}x{} D16, {} images, read back to the app", size[0], size[1], images.len());
        Ok(Some(DepthSource { images, last: None, size, slots, delivered: 0, acquired: 0, lent }))
    }

    /// A finished readback: the `XrDepth` payload (header + texels), or None.
    pub(super) fn poll(&mut self, dev: &Device) -> Option<Vec<u8>> {
        // the oldest finished copy first
        for slot in self.slots.iter_mut() {
            let Some(header) = slot.pending else { continue };
            if !unsafe { dev.raw.get_fence_status(slot.fence) }.unwrap_or(false) {
                continue;
            }
            slot.pending = None;
            let texels = (header.width * header.height * header.layers) as usize * 2;
            let Ok(data) = slot.buffer.read(texels) else { continue };
            let mut payload = Vec::with_capacity(<XrDepth as crate::buffers::Wire>::SIZE + texels);
            crate::buffers::Wire::write(&header, &mut payload);
            payload.extend_from_slice(&data);
            self.delivered += 1;
            return Some(payload);
        }
        None
    }

    /// Acquires the newest depth image for `time`: a new one's header (the app's GPU reads it
    /// from now on), and its copy started when a slot is free.
    pub(super) fn acquire(&mut self, xr: &LibOpenXr, dev: &Device, provider: XrEnvironmentDepthProviderMETA, space: XrSpace, time: XrTime) -> Option<XrDepth> {
        let info = XrEnvironmentDepthImageAcquireInfoMETA { space, display_time: time, ..Default::default() };
        let mut image = XrEnvironmentDepthImageMETA::default();
        if unsafe { (xr.xrAcquireEnvironmentDepthImageMETA)(provider, &info, &mut image) } != XrResult::SUCCESS {
            return None;
        }
        let &src = self.images.get(image.swapchain_index as usize)?;
        let p = image.views[0].pose;
        let id = (image.swapchain_index, [p.orientation.x, p.orientation.y, p.orientation.z, p.orientation.w, p.position.x, p.position.y, p.position.z]);
        if self.last == Some(id) {
            return None;
        }
        self.last = Some(id);
        let pose = |p: XrPosef| crate::XrPose { orientation: [p.orientation.x, p.orientation.y, p.orientation.z, p.orientation.w], position: [p.position.x, p.position.y, p.position.z] };
        let view = |v: &XrEnvironmentDepthImageViewMETA| XrEye { pose: pose(v.pose), fov: [v.fov.angle_left, v.fov.angle_right, v.fov.angle_up, v.fov.angle_down] };
        let header = XrDepth { width: self.size[0], height: self.size[1], layers: LAYERS, near: image.near_z, far: if image.far_z.is_finite() { image.far_z } else { 0.0 },
            views: [view(&image.views[0]), view(&image.views[1])], image: self.lent.0.get(image.swapchain_index as usize).copied().unwrap_or(0) };
        // the texels for the room field: copied when the app wants it (`hal_xr_depth_readback`)
        // and a slot is free (a busy one skips this image)
        if !super::DEPTH_READBACK.load(std::sync::atomic::Ordering::Acquire) { return Some(header); }
        // (a delivered slot's fence has signalled (poll checked it); a slot never used is created signalled)
        let Some(slot) = self.slots.iter_mut().find(|s| s.pending.is_none()) else { return Some(header) };
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::DEPTH).level_count(1).base_array_layer(0).layer_count(LAYERS);
        let barrier = |old, new, from, to| vk::ImageMemoryBarrier::default().image(src).old_layout(old).new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED).src_access_mask(from).dst_access_mask(to).subresource_range(range);
        // (one region per layer: each eye's texels after the one before)
        let layer_bytes = self.size[0] as u64 * self.size[1] as u64 * 2;
        let copies: Vec<vk::BufferImageCopy> = (0..LAYERS).map(|l| vk::BufferImageCopy::default().buffer_offset(l as u64 * layer_bytes)
            .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::DEPTH).base_array_layer(l).layer_count(1))
            .image_extent(vk::Extent3D { width: self.size[0], height: self.size[1], depth: 1 })).collect();
        let ready = vk::BufferMemoryBarrier::default().src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::HOST_READ).buffer(slot.buffer.buffer).size(vk::WHOLE_SIZE);
        let recorded = unsafe {
            (|| -> Result<(), vk::Result> {
                dev.raw.reset_fences(&[slot.fence])?;
                dev.raw.reset_command_buffer(slot.cb, vk::CommandBufferResetFlags::empty())?;
                dev.raw.begin_command_buffer(slot.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
                dev.raw.cmd_pipeline_barrier(slot.cb, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[],
                    &[barrier(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::SHADER_READ, vk::AccessFlags::TRANSFER_READ)]);
                dev.raw.cmd_copy_image_to_buffer(slot.cb, src, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, slot.buffer.buffer, &copies);
                dev.raw.cmd_pipeline_barrier(slot.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::HOST, vk::DependencyFlags::empty(), &[], &[ready],
                    &[barrier(vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL, vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::SHADER_READ)]);
                dev.raw.end_command_buffer(slot.cb)?;
                let cbs = [slot.cb];
                let _queue = super::lock(&dev.queue_lock);
                dev.raw.queue_submit(dev.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], slot.fence)
            })()
        };
        match recorded {
            Ok(()) => {
                slot.pending = Some(header);
                self.acquired += 1;
            }
            Err(e) => {
                xr_log!("environment depth copy: {e:?}");
                // the fence was reset but nothing will signal it: put it back
                unsafe { let _ = dev.raw.destroy_fence(slot.fence, None); }
                slot.fence = unsafe { dev.raw.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None) }.unwrap_or(vk::Fence::null());
            }
        }
        Some(header)
    }

    pub(super) fn destroy(&mut self, dev: &Device) {
        for slot in self.slots.drain(..) {
            unsafe {
                let _ = dev.raw.wait_for_fences(&[slot.fence], true, u64::MAX);
                dev.raw.destroy_fence(slot.fence, None);
                dev.raw.destroy_command_pool(slot.pool, None);
            }
        }
    }
}

//! The frame tap's GPU work (`crate::frame_tap`): a finished frame image (BGRA8 UNORM in
//! TRANSFER_SRC_OPTIMAL, as the core hands frames to presenters) blitted, linear filtered, from a
//! region down to the stream size into an RGBA8 image (the blit also swaps the channels), copied
//! to host memory and waited for. Its own command pool, buffer and fence: the device's queue lock
//! is taken only around the submit, so the core's queue worker never waits for a tap.

use super::device::{error, Device};
use super::resources::{Buffer, Image};
use crate::session::TextureObj;
use crate::{format, usage};
use makepad_vulkan::vk;
use std::sync::Arc;

/// How long one wait on the copy's fence lasts before it is tried again (a long wait is a GPU
/// stall, never a reason to give up the image while the GPU may still read it).
const FENCE_SLICE_NS: u64 = 1_000_000_000;

pub struct Reader {
    dev: Arc<Device>,
    size: [u32; 2],
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    scaled: Arc<Image>,
    staging: Arc<Buffer>,
    /// A copy was submitted and its fence not seen signalled.
    in_flight: bool,
}

impl Reader {
    /// A reader of frames scaled to `size` on `dev`.
    pub fn new(dev: Arc<Device>, size: [u32; 2]) -> Result<Reader, String> {
        let scaled = Image::new(dev.clone(), TextureObj { format: format::RGBA8, w: size[0], h: size[1], depth: 1, mips: 1,
            usage: usage::RENDER_TARGET | usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?;
        let staging = Buffer::scratch(dev.clone(), size[0] as u64 * size[1] as u64 * 4, vk::BufferUsageFlags::TRANSFER_DST)?;
        unsafe {
            let pool = dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(dev.family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None).map_err(|e| format!("tap command pool: {e:?}"))?;
            let made = (|| {
                let cb = dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(|e| format!("tap command buffer: {e:?}"))?[0];
                let fence = dev.raw.create_fence(&vk::FenceCreateInfo::default(), None).map_err(|e| format!("tap fence: {e:?}"))?;
                Ok::<_, String>((cb, fence))
            })();
            let (cb, fence) = match made {
                Ok(v) => v,
                Err(e) => {
                    dev.raw.destroy_command_pool(pool, None);
                    return Err(e);
                }
            };
            Ok(Reader { dev, size, pool, cb, fence, scaled, staging, in_flight: false })
        }
    }

    /// Whether this reader's objects are on `dev` (a replaced device needs a new reader).
    pub fn on_device(&self, dev: &Arc<Device>) -> bool {
        Arc::ptr_eq(&self.dev, dev)
    }

    /// `frame` blitted part by part (`crate::frame_tap::parts`: a rectangle of an array layer into
    /// a rectangle of the picture) into this reader's size, as tightly packed RGBA8. Returns once
    /// the GPU is done with `frame`.
    pub fn read(&mut self, frame: &Image, parts: &[crate::frame_tap::Part]) -> Result<Vec<u8>, String> {
        if !Arc::ptr_eq(&frame.dev, &self.dev) {
            return Err("the frame is on another device".into());
        }
        if frame.desc.format != format::BGRA8 && frame.desc.format != format::RGBA8 {
            return Err(format!("tap of a frame in texture format {}", frame.desc.format));
        }
        let [w, h] = self.size;
        let dev = &self.dev;
        let color = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let layers = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        let barrier = |image: vk::Image, old, new, src_access, dst_access| vk::ImageMemoryBarrier::default().image(image).old_layout(old).new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(src_access).dst_access_mask(dst_access).subresource_range(color);
        unsafe {
            dev.raw.reset_command_buffer(self.cb, vk::CommandBufferResetFlags::empty()).map_err(error)?;
            dev.raw.begin_command_buffer(self.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(error)?;
            // the frame's writes (earlier submissions on this queue) before the blit reads it;
            // the scaled image's old contents are not needed
            dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::MEMORY_WRITE).dst_access_mask(vk::AccessFlags::TRANSFER_READ)], &[],
                &[barrier(self.scaled.image, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)]);
            let blits: Vec<vk::ImageBlit> = parts.iter().filter(|p| p.layer < frame.desc.depth.max(1)).map(|p| {
                let (src, dst) = (p.src, p.dst);
                let x1 = (src[0] + src[2]).min(frame.desc.w);
                let y1 = (src[1] + src[3]).min(frame.desc.h);
                vk::ImageBlit::default().src_subresource(layers.base_array_layer(p.layer)).dst_subresource(layers)
                    .src_offsets([vk::Offset3D { x: src[0] as i32, y: src[1] as i32, z: 0 }, vk::Offset3D { x: x1 as i32, y: y1 as i32, z: 1 }])
                    .dst_offsets([vk::Offset3D { x: dst[0].min(w) as i32, y: dst[1].min(h) as i32, z: 0 }, vk::Offset3D { x: (dst[0] + dst[2]).min(w) as i32, y: (dst[1] + dst[3]).min(h) as i32, z: 1 }])
            }).collect();
            dev.raw.cmd_blit_image(self.cb, frame.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, self.scaled.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &blits, vk::Filter::LINEAR);
            dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[],
                &[barrier(self.scaled.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_READ)]);
            let copy = vk::BufferImageCopy::default().image_subresource(layers).image_extent(vk::Extent3D { width: w, height: h, depth: 1 });
            dev.raw.cmd_copy_image_to_buffer(self.cb, self.scaled.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, self.staging.buffer, &[copy]);
            dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::HOST, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::HOST_READ)], &[], &[]);
            dev.raw.end_command_buffer(self.cb).map_err(error)?;
            dev.raw.reset_fences(&[self.fence]).map_err(error)?;
            let cbs = [self.cb];
            {
                let _queue = super::device::lock(&dev.queue_lock);
                dev.raw.queue_submit(dev.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], self.fence).map_err(error)?;
            }
            self.in_flight = true;
            loop {
                match dev.raw.wait_for_fences(&[self.fence], true, FENCE_SLICE_NS) {
                    Ok(()) => {
                        self.in_flight = false;
                        break;
                    }
                    Err(vk::Result::TIMEOUT) if !dev.lost.load(std::sync::atomic::Ordering::Acquire) => continue,
                    Err(e) => return Err(error(e)),
                }
            }
        }
        self.staging.read(w as usize * h as usize * 4)
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            // a copy whose wait failed (a lost device) may still be in flight
            if self.in_flight {
                let _ = self.dev.raw.wait_for_fences(&[self.fence], true, FENCE_SLICE_NS);
            }
            self.dev.raw.destroy_fence(self.fence, None);
            self.dev.raw.destroy_command_pool(self.pool, None);
        }
    }
}

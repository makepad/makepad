//! Native Vulkan capture port: one GPU copy into persistently mapped storage, delivered only
//! after the submission fence. Compiled under backend::vulkan, sharing its allocation ledger.
use super::{
    resources::{Buffer, Image},
    submit::{Batch, Ticket},
    Vulkan,
};
use crate::{
    backend::linux::media::capture_surface::{Mapping, Surface},
    session::Session,
    types::*,
};
use makepad_vulkan::vk;
use std::sync::Arc;

struct Completed {
    buffer: Arc<Buffer>,
    length: usize,
}
impl Mapping for Completed {
    fn bytes(&self) -> &[u8] {
        // Constructed after the GPU fence and invalidation. No path writes this allocation
        // again while a Surface owns it.
        unsafe { self.buffer.completed_bytes(self.length) }
    }
}
pub(super) fn completed(
    buffer: Arc<Buffer>,
    size: [u32; 2],
    fmt: u32,
) -> Result<Arc<Surface>, String> {
    let length = size[0] as usize * size[1] as usize * 4;
    buffer.invalidate()?;
    Ok(Arc::new(Surface {
        width: size[0],
        height: size[1],
        bgra: fmt == format::BGRA8,
        mapping: Arc::new(Completed { buffer, length }),
    }))
}
impl Vulkan {
    pub(super) fn capture_frame(
        &mut self,
        s: &Session,
        window: Id,
        image: Arc<Image>,
        batch: &mut Batch,
    ) -> Result<(), String> {
        if !matches!(image.desc.format, format::BGRA8 | format::RGBA8) {
            return Err("capture requires RGBA/BGRA window storage".into());
        }
        // Match the native recorder contract: even dimensions, cropped at right/bottom.
        let size = [image.desc.w & !1, image.desc.h & !1];
        if size[0] == 0 || size[1] == 0 {
            return Ok(());
        }
        let length = size[0] as u64 * size[1] as u64 * 4;
        let pool = self.capture_pool.entry((s.id, window)).or_default();
        // A recorder's Arc lease prevents reuse. Resize retires old allocations; pending
        // fences and recorder leases keep them alive. Grow only when every buffer is in use.
        pool.retain(|buffer| buffer.size == length);
        let buffer = if let Some(buffer) = pool.iter().find(|b| Arc::strong_count(b) == 1) {
            buffer.clone()
        } else {
            let buffer = Buffer::new(
                batch.dev.clone(),
                length,
                vk::BufferUsageFlags::TRANSFER_DST,
            )?;
            pool.push(buffer.clone());
            buffer
        };
        image.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: size[0],
                height: size[1],
                depth: 1,
            });
        unsafe {
            batch.dev.raw.cmd_copy_image_to_buffer(
                batch.cb,
                image.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer.buffer,
                &[region],
            );
            batch.dev.raw.cmd_pipeline_barrier(
                batch.cb,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)],
                &[],
                &[],
            );
        }
        image.transition(batch.cb, image.sampled_layout());
        batch.tickets.push(Ticket::Capture {
            token: Self::token(s),
            window,
            size,
            format: image.desc.format,
            buffer,
        });
        batch.images.push(image);
        Ok(())
    }
}

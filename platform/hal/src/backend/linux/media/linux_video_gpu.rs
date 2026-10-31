//! Vulkan storage for Linux decoded frames. Compiled as a child of backend::vulkan so the
//! media service uses the same image table, queue and fence retirement as ordinary textures.
use super::{
    device::error,
    resources::{vk_format, Image},
    Texture, Vulkan,
};
use crate::{
    backend::linux::media::{
        video::{DmaPlane, Frame, Plane, Storage},
        Sink,
    },
    ledger::{Kind, LedgerEntry},
    session::{Session, TextureObj},
    types::*,
};
use makepad_vulkan::vk;
use std::{
    os::fd::{AsRawFd, IntoRawFd},
    sync::{atomic::Ordering, Arc},
};

fn descriptor(plane: &Plane) -> TextureObj {
    TextureObj {
        format: plane.format,
        w: plane.width,
        h: plane.height,
        depth: 1,
        mips: 1,
        usage: usage::SAMPLED,
        bytes: 0,
        imported: true,
        uploaded: 0,
        lent: None,
    }
}
fn row(s: &mut Session, id: Id, desc: &TextureObj) {
    if let Some(row) = s
        .ledger
        .get_mut(Kind::Texture, id.index)
        .filter(|r| r.gen == id.gen)
    {
        row.bytes = desc.bytes;
        row.width = desc.w;
        row.height = desc.h;
        row.format = desc.format;
    } else {
        s.ledger.add(LedgerEntry {
            kind: Kind::Texture,
            detail: "video frame",
            index: id.index,
            gen: id.gen,
            bytes: desc.bytes,
            format: desc.format,
            width: desc.w,
            height: desc.h,
            mips: 1,
            attr: Attr::default(),
            created_ns: crate::clock_ns(),
            created_frame: s.frame,
            last_used_frame: 0,
            last_used_ns: 0,
        });
    }
}

impl Sink for Vulkan {
    fn declare(&mut self, s: &mut Session, id: Id, fmt: u32) {
        if s.textures.get(s.id, id).is_ok() {
            return;
        }
        let desc = TextureObj {
            format: fmt,
            w: 1,
            h: 1,
            depth: 1,
            mips: 1,
            usage: usage::SAMPLED,
            bytes: 0,
            imported: true,
            uploaded: 0,
            lent: None,
        };
        if s.textures.insert(s.id, id, s.id, desc.clone()).is_ok() {
            row(s, id, &desc);
        } else {
            s.stale();
        }
    }
    fn present(&mut self, s: &mut Session, ids: [Id; 4], frame: &Frame) -> Result<bool, String> {
        let targets: Vec<_> = if frame.yuv & crate::media::yuv::ENABLED != 0 {
            ids[1..].to_vec()
        } else {
            vec![ids[0]]
        };
        if targets.len() != frame.planes.len() {
            return Err("video frame plane count differs from texture targets".into());
        }
        // Validate all generations before allocating or replacing any plane.
        for id in &targets {
            if s.textures.get(s.id, *id).is_err() {
                return Ok(false);
            }
        }
        let mut images = Vec::new();
        for (&id, plane) in targets.iter().zip(&frame.planes) {
            let image = match &plane.storage {
                Storage::Dma(dma) => import_plane(self.dev.clone(), plane, dma.clone())?,
                Storage::Bytes(_) => {
                    if let Some(t) = self.tex.get(&(s.id, id.index)).filter(|t| {
                        t.gen == id.gen
                            && t.image.desc.format == plane.format
                            && t.image.desc.w == plane.width
                            && t.image.desc.h == plane.height
                            && t.image.external.is_none()
                            && Arc::strong_count(&t.image) == 1
                    }) {
                        t.image.clone()
                    } else {
                        Image::new(self.dev.clone(), descriptor(plane))?
                    }
                }
            };
            images.push(image);
        }
        if frame
            .planes
            .iter()
            .any(|p| matches!(&p.storage, Storage::Bytes(_)))
        {
            let batch = self.ensure_batch()?;
            let mut copies = Vec::new();
            // Allocate every staging slice before changing a texture or recording a write.
            // An allocation failure leaves the previously presented frame intact.
            for (plane, image) in frame.planes.iter().zip(&images) {
                if let Storage::Bytes(bytes) = &plane.storage {
                    if bytes.len() as u64
                        != format::level_bytes(plane.format, plane.width, plane.height)
                    {
                        return Err("video plane byte count differs from its extent".into());
                    }
                    let (staging, offset) = batch.put(bytes)?;
                    copies.push((image.clone(), staging, offset));
                }
            }
            for (image, staging, offset) in copies {
                image.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
                let copy = vk::BufferImageCopy::default()
                    .buffer_offset(offset)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(image.aspect)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: image.desc.w,
                        height: image.desc.h,
                        depth: 1,
                    });
                unsafe {
                    batch.dev.raw.cmd_copy_buffer_to_image(
                        batch.cb,
                        staging.buffer,
                        image.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[copy],
                    );
                }
                image.transition(batch.cb, image.sampled_layout());
                batch.images.push(image);
            }
        }
        for ((&id, plane), image) in targets.iter().zip(&frame.planes).zip(images) {
            let mut desc = descriptor(plane);
            desc.bytes = image.bytes;
            s.textures
                .get_mut(s.id, id)
                .map_err(|_| "stale video texture")?
                .val = desc.clone();
            self.tex
                .insert((s.id, id.index), Texture { gen: id.gen, image });
            row(s, id, &desc);
        }
        Ok(true)
    }
    fn release(&mut self, s: &mut Session, ids: [Id; 4]) {
        for id in ids {
            if self
                .tex
                .get(&(s.id, id.index))
                .is_some_and(|t| t.gen == id.gen)
            {
                self.tex.remove(&(s.id, id.index));
            }
            if let Ok(entry) = s.textures.get_mut(s.id, id) {
                entry.val.bytes = 0;
                entry.val.imported = true;
            }
            if let Some(row) = s
                .ledger
                .get_mut(Kind::Texture, id.index)
                .filter(|r| r.gen == id.gen)
            {
                row.bytes = 0;
            }
        }
    }
}

fn import_plane(
    dev: Arc<super::Device>,
    plane: &Plane,
    dma: Arc<DmaPlane>,
) -> Result<Arc<Image>, String> {
    for extension in [
        vk::KHR_EXTERNAL_MEMORY_FD_NAME,
        vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
        vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME,
        vk::EXT_QUEUE_FAMILY_FOREIGN_NAME,
    ] {
        if !dev.supports(extension) {
            return Err("Vulkan device cannot import DMA-BUF video planes".into());
        }
    }
    let desc = descriptor(plane);
    let fmt = vk_format(desc.format).ok_or("unsupported video plane format")?;
    if plane.width == 0 || plane.height == 0 || dma.offset >= dma.size || dma.pitch == 0 {
        return Err("invalid DMA-BUF plane extent".into());
    }
    let mut drm = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(dma.modifier)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let query = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(fmt)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .next_physical_device_image_drm_format_modifier_info_ext(&mut drm)
        .next_physical_device_external_image_format_info(&mut external);
    let mut props = vk::ExternalImageFormatProperties::default();
    let mut result =
        vk::ImageFormatProperties2::default().next_external_image_format_properties(&mut props);
    unsafe {
        dev.instance
            .get_physical_device_image_format_properties2(dev.physical, &query, &mut result)
    }
    .map_err(error)?;
    if !props
        .external_memory_properties
        .external_memory_features
        .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
    {
        return Err("DMA-BUF video modifier cannot be sampled".into());
    }
    let layouts = [vk::SubresourceLayout::default()
        .offset(dma.offset)
        .row_pitch(dma.pitch)];
    let mut layout = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(dma.modifier)
        .plane_layouts(&layouts);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(fmt)
        .extent(vk::Extent3D {
            width: desc.w,
            height: desc.h,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .next_image_drm_format_modifier_explicit_create_info_ext(&mut layout)
        .next_external_memory_image_create_info(&mut external);
    let image = unsafe { dev.raw.create_image(&info, None) }.map_err(error)?;
    let mut memory = vk::DeviceMemory::null();
    let bind = (|| {
        let req = unsafe { dev.raw.get_image_memory_requirements(image) };
        if req.size > dma.size {
            return Err("DMA-BUF allocation smaller than Vulkan requirements".into());
        }
        let fd = dma.fd.try_clone().map_err(|e| e.to_string())?;
        let mut properties = vk::MemoryFdPropertiesKHR::default();
        unsafe {
            dev.raw.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                fd.as_raw_fd(),
                &mut properties,
            )
        }
        .map_err(error)?;
        let ty = dev.memory_type(
            req.memory_type_bits & properties.memory_type_bits,
            vk::MemoryPropertyFlags::empty(),
        )?;
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(fd.as_raw_fd());
        memory = unsafe {
            dev.raw.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(dma.size)
                    .memory_type_index(ty)
                    .next_memory_dedicated_allocate_info(&mut dedicated)
                    .next_import_memory_fd_info_khr(&mut import),
                None,
            )
        }
        .map_err(error)?;
        let _ = fd.into_raw_fd();
        unsafe { dev.raw.bind_image_memory(image, memory, 0) }.map_err(error)
    })();
    if let Err(e) = bind {
        unsafe {
            dev.raw.destroy_image(image, None);
            dev.raw.free_memory(memory, None);
        }
        return Err(e);
    }
    let image = unsafe {
        Image::from_bound(
            dev,
            desc,
            image,
            memory,
            0,
            vk::ImageLayout::GENERAL,
            Some(dma),
        )
    }?;
    image.external_owner.store(true, Ordering::Release);
    Ok(image)
}

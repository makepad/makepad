//! Linux transport decoding and Cx's external-image facade. No Vulkan object lives here.
use crate::{foreign_dmabuf::*, os::shared_framebuf::{LinuxOwnedImage, LinuxOwnedImagePlane}, Texture, TextureFormat, Cx};
use makepad_hal::{dmabuf::{DmaBuf, OpaqueMemory}, format, usage, texture_handle, Attr, Hal};

pub(super) fn hosted_descriptor(image: LinuxOwnedImage, width: u32, height: u32) -> Result<DmaBuf, String> {
    let (opaque, timeline_fd, allocation_size) = if let Some(vulkan) = image.vulkan {
        (Some(OpaqueMemory { memory_type_index: vulkan.info.memory_type_index,
            device_uuid: vulkan.info.device_uuid, driver_uuid: vulkan.info.driver_uuid }),
            Some(vulkan.semaphore_fd), vulkan.info.allocation_size)
    } else {
        let size = DmaBuf::size(&image.plane.dma_buf_fd)?;
        (None, image.dma_buf_timeline, size)
    };
    if timeline_fd.is_none() { return Err("hosted image needs an explicit writer/read lease timeline".into()); }
    Ok(DmaBuf { fd: image.plane.dma_buf_fd, width, height, modifier: image.drm_format.modifiers,
        offset: image.plane.offset as u64, row_pitch: image.plane.stride as u64, allocation_size,
        fourcc: if opaque.is_some() { u32::from_le_bytes(*b"AR24") } else { image.drm_format.fourcc },
        sample_only: false, opaque, timeline_fd })
}

impl Cx {
    /// A shared texture's DMA-BUF (`create_shared_texture`, once `shared_texture_handle` is
    /// known): a new descriptor the caller owns and its layout as
    /// `makepad_hal::dmabuf::copy::Layout` words. For handing the image to another process
    /// (frame rings); the texture must outlive every use of it there.
    pub fn shared_texture_dmabuf(&self, texture: &Texture) -> Result<(std::os::fd::OwnedFd, [u64; 6]), String> {
        use std::os::fd::FromRawFd;
        let ticket = self.shared_texture_handle(texture).filter(|h| *h != 0).ok_or("shared image export is not ready")?;
        let mut layout = [0u64; 6];
        let fd = makepad_hal::dmabuf::hal_dmabuf_export(self.hal.frames.w.session(), ticket, &mut layout);
        if fd < 0 {
            return Err("shared image ticket expired".into());
        }
        Ok((unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }, layout))
    }

    pub(crate) fn hal_export_presentable_image(&self, texture: &Texture) -> Result<LinuxOwnedImage, String> {
        let ticket = self.shared_texture_handle(texture).filter(|h| *h != 0).ok_or("shared image export is not ready")?;
        let native = Hal::global().dmabuf_handle(self.hal.frames.w.session(), ticket).ok_or("shared image ticket expired")?;
        let timeline = native.timeline_fd.as_ref().ok_or("hosted DMA-BUF sharing needs external timelines")?;
        Ok(LinuxOwnedImage {
            vulkan: None,
            dma_buf_timeline: Some(timeline.try_clone().map_err(|e| e.to_string())?),
            drm_format: crate::os::shared_framebuf::DrmFormat { fourcc: native.fourcc, modifiers: native.modifier },
            plane: LinuxOwnedImagePlane { dma_buf_fd: native.fd.try_clone().map_err(|e| e.to_string())?,
                offset: native.offset.try_into().map_err(|_| "DMA-BUF offset exceeds wire format")?,
                stride: native.row_pitch.try_into().map_err(|_| "DMA-BUF stride exceeds wire format")? },
        })
    }

    /// Copies a hosted child's finished frame out of its shared image into `into` (the
    /// snapshot of its previous frame, when the size matches) or else into a new snapshot.
    /// Reusing the snapshot keeps a hosted window at one allocation however many frames it
    /// presents: the copy is ordered after the frames that sampled the previous contents.
    pub(crate) fn hal_copy_presentable_frame(&mut self, source: &Texture, sequence: u64, into: Option<&Texture>) -> Result<Texture, String> {
        let (width, height) = match source.get_format(self) {
            TextureFormat::SharedBGRAu8 { width, height, .. } => (*width, *height),
            _ => return Err("expected a shared hosted texture".into()),
        };
        let source_id = source.texture_id();
        let source_id = self.hal.frames.texture_hal_id(source_id.0, source_id.generation()).ok_or("hosted image has no HAL allocation")?;
        if let Some(texture) = into {
            let same_size = matches!(texture.get_format(self), TextureFormat::SharedBGRAu8 { width: w, height: h, .. } if *w == width && *h == height);
            let id = texture.texture_id();
            if let Some(destination) = self.hal.frames.texture_hal_id(id.0, id.generation()).filter(|_| same_size) {
                // what this Cx recorded so far (draws sampling the old contents) goes first
                self.hal_submit_batch();
                Hal::global().dmabuf_copy_frame(self.hal.frames.w.session(), source_id, destination, sequence)?;
                return Ok(texture.clone());
            }
        }
        let texture = Texture::new_with_format(self, TextureFormat::SharedBGRAu8 {
            id: crate::studio::PresentableImageId::alloc(), width, height, initial: false,
        });
        let id = texture.texture_id();
        self.textures[id].alloc_shared();
        let destination = self.hal.frames.external_texture(id.0, id.generation());
        self.hal.frames.w.texture_create(destination, usage::SAMPLED | usage::RENDER_TARGET, format::BGRA8,
            width as u32, height as u32, [1, 1], Attr::default());
        self.hal_submit_batch();
        Hal::global().dmabuf_copy_frame(self.hal.frames.w.session(), source_id, destination, sequence)?;
        Ok(texture)
    }

    pub fn foreign_dmabuf_support(&mut self) -> Option<ForeignDmabufSupport> {
        let support = Hal::global().dmabuf_support(self.hal.frames.w.session())?;
        Some(ForeignDmabufSupport { render_device: support.render_device, render_node: support.render_node, formats: support.formats })
    }

    pub fn foreign_dmabuf_import(&mut self, buffer: ForeignDmabuf) -> Result<Texture, String> {
        let [plane] = <[_; 1]>::try_from(buffer.planes).map_err(|_| "only advertised single-plane DMA-BUFs are imported")?;
        let allocation_size = DmaBuf::size(&plane.fd)?;
        let descriptor = DmaBuf { fd: plane.fd, width: buffer.width, height: buffer.height, fourcc: buffer.fourcc,
            modifier: buffer.modifier, offset: plane.offset as u64, row_pitch: plane.stride as u64,
            allocation_size, sample_only: true, opaque: None, timeline_fd: None };
        let texture = Texture::new_with_format(self, TextureFormat::SharedBGRAu8 {
            id: crate::studio::PresentableImageId::alloc(), width: buffer.width as usize, height: buffer.height as usize, initial: false,
        });
        let id = texture.texture_id();
        self.textures[id].alloc_shared();
        let id = self.hal.frames.external_texture(id.0, id.generation());
        let session = self.hal.frames.w.session();
        // The Wayland server gates both implicit and explicit acquire fences before
        // forwarding the commit. The image is acquired when the next frame uses it.
        let ticket = unsafe { Hal::global().dmabuf_register(session, descriptor) }?;
        self.hal.frames.w.texture_import(id, ticket, texture_handle::NATIVE);
        self.hal_submit_batch();
        Hal::global().dmabuf_release(session, ticket);
        Hal::global().dmabuf_acquire(session, id)?;
        Ok(texture)
    }

    pub fn foreign_dmabuf_acquire(&mut self, texture: &Texture) -> Result<(), String> {
        let id = texture.texture_id();
        let id = self.hal.frames.texture_hal_id(id.0, id.generation()).ok_or("missing foreign texture")?;
        Hal::global().dmabuf_acquire(self.hal.frames.w.session(), id)
    }

    pub fn foreign_dmabuf_fence(&mut self) -> Result<ForeignDmabufFence, String> {
        self.hal_submit_batch();
        Hal::global().dmabuf_fence(self.hal.frames.w.session(), std::sync::Arc::new(crate::thread::SignalToUI::set_ui_signal)).map(ForeignDmabufFence::from_hal)
    }
}

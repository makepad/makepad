//! Linux transport decoding and Cx's external-image facade. No Vulkan object lives here.
use crate::{foreign_dmabuf::*, os::shared_framebuf::{LinuxOwnedImage, LinuxOwnedImagePlane}, Texture, TextureFormat, Cx};
use makepad_hal::{dmabuf::{self, DmabufDesc}, format, usage, texture_handle, Attr};

/// The text of this thread's last failed DMA-BUF op (`makepad_hal::dmabuf::hal_dmabuf_error`).
pub(crate) fn dmabuf_error() -> String {
    let mut text = vec![0u8; 256];
    let n = dmabuf::hal_dmabuf_error(&mut text) as usize;
    if n > text.len() {
        text.resize(n, 0);
        dmabuf::hal_dmabuf_error(&mut text);
    }
    text.truncate(n);
    String::from_utf8_lossy(&text).into_owned()
}

/// `dmabuf_error` as the error of a failed op.
pub(crate) fn dmabuf_ok(ok: bool) -> Result<(), String> {
    if ok { Ok(()) } else { Err(dmabuf_error()) }
}

extern "C" fn wake_ui() {
    crate::thread::SignalToUI::set_ui_signal()
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
        use std::os::fd::{FromRawFd, OwnedFd};
        let mut desc = DmabufDesc::default();
        let mut fds = [-1i32; 2];
        dmabuf_ok(dmabuf::hal_dmabuf_presentable(self.hal.frames.w.session(), ticket, &mut desc, &mut fds))?;
        let fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        if fds[1] < 0 {
            return Err("hosted DMA-BUF sharing needs external timelines".into());
        }
        let timeline = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        Ok(LinuxOwnedImage {
            vulkan: None,
            dma_buf_timeline: Some(timeline),
            drm_format: crate::os::shared_framebuf::DrmFormat { fourcc: desc.fourcc, modifiers: desc.modifier },
            plane: LinuxOwnedImagePlane { dma_buf_fd: fd,
                offset: desc.offset.try_into().map_err(|_| "DMA-BUF offset exceeds wire format")?,
                stride: desc.row_pitch.try_into().map_err(|_| "DMA-BUF stride exceeds wire format")? },
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
                dmabuf_ok(dmabuf::hal_dmabuf_copy_frame(self.hal.frames.w.session(), source_id, destination, sequence))?;
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
        dmabuf_ok(dmabuf::hal_dmabuf_copy_frame(self.hal.frames.w.session(), source_id, destination, sequence))?;
        Ok(texture)
    }

    pub fn foreign_dmabuf_support(&mut self) -> Option<ForeignDmabufSupport> {
        use std::os::unix::ffi::OsStrExt;
        let session = self.hal.frames.w.session();
        let (mut words, mut node) = (vec![0u64; 128], vec![0u8; 256]);
        let mut info = dmabuf::hal_dmabuf_support(session, &mut words, &mut node);
        if info.format_count as usize * 2 > words.len() || info.node_len as usize > node.len() {
            words.resize(words.len().max(info.format_count as usize * 2), 0);
            node.resize(node.len().max(info.node_len as usize), 0);
            info = dmabuf::hal_dmabuf_support(session, &mut words, &mut node);
        }
        if !info.found {
            return None;
        }
        node.truncate(info.node_len as usize);
        let formats = words.chunks_exact(2).take(info.format_count as usize).map(|p| (p[0] as u32, p[1])).collect();
        Some(ForeignDmabufSupport { render_device: info.render_device, render_node: std::ffi::OsStr::from_bytes(&node).into(), formats })
    }

    pub fn foreign_dmabuf_import(&mut self, buffer: ForeignDmabuf) -> Result<Texture, String> {
        let [plane] = <[_; 1]>::try_from(buffer.planes).map_err(|_| "only advertised single-plane DMA-BUFs are imported")?;
        let allocation_size = dmabuf::DmaBuf::size(&plane.fd)?;
        let desc = DmabufDesc { width: buffer.width, height: buffer.height, fourcc: buffer.fourcc,
            modifier: buffer.modifier, offset: plane.offset as u64, row_pitch: plane.stride as u64,
            allocation_size, sample_only: true, ..DmabufDesc::default() };
        let texture = Texture::new_with_format(self, TextureFormat::SharedBGRAu8 {
            id: crate::studio::PresentableImageId::alloc(), width: buffer.width as usize, height: buffer.height as usize, initial: false,
        });
        let id = texture.texture_id();
        self.textures[id].alloc_shared();
        let id = self.hal.frames.external_texture(id.0, id.generation());
        let session = self.hal.frames.w.session();
        // The Wayland server gates both implicit and explicit acquire fences before
        // forwarding the commit. The image is acquired when the next frame uses it.
        use std::os::fd::IntoRawFd;
        let ticket = dmabuf::hal_dmabuf_import(session, plane.fd.into_raw_fd(), -1, &desc);
        if ticket == 0 {
            return Err(dmabuf_error());
        }
        self.hal.frames.w.texture_import(id, ticket, texture_handle::NATIVE);
        self.hal_submit_batch();
        dmabuf::hal_dmabuf_unregister(session, ticket);
        dmabuf_ok(dmabuf::hal_dmabuf_acquire(session, id))?;
        Ok(texture)
    }

    pub fn foreign_dmabuf_acquire(&mut self, texture: &Texture) -> Result<(), String> {
        let id = texture.texture_id();
        let id = self.hal.frames.texture_hal_id(id.0, id.generation()).ok_or("missing foreign texture")?;
        dmabuf_ok(dmabuf::hal_dmabuf_acquire(self.hal.frames.w.session(), id))
    }

    pub fn foreign_dmabuf_fence(&mut self) -> Result<ForeignDmabufFence, String> {
        self.hal_submit_batch();
        match dmabuf::hal_dmabuf_fence(self.hal.frames.w.session(), wake_ui) {
            0 => Err(dmabuf_error()),
            fence => Ok(ForeignDmabufFence::from_hal(fence)),
        }
    }
}

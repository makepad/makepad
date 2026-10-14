//! Native external image storage, isolated from the executor. Handles in command buffers are
//! registry tickets, never unchecked pointers or process-local FD numbers. A shell moves an
//! owned DMA-BUF/AHardwareBuffer descriptor through this registry, then sends its ticket.

#[cfg(target_os = "linux")]
mod dma_buf;
#[cfg(target_os = "linux")]
pub(super) mod sync;
#[cfg(target_os = "linux")]
pub mod copy;
#[cfg(target_os = "android")]
mod hardware_buffer;

use super::device::{lock, Device};
use super::resources::Image;
use crate::session::TextureObj;
use crate::types::texture_handle;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::{Arc, Mutex, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "linux")]
pub use dma_buf::DmaBuf as NativeHandle;
#[cfg(target_os = "linux")]
pub use dma_buf::{DmaBufSupport, OpaqueMemory};
#[cfg(target_os = "android")]
pub use hardware_buffer::HardwareBuffer as NativeHandle;

/// A shell may supply its own handle transport. The image lease owns the native allocation;
/// any external producer must have completed and released ownership before import is used.
/// External providers enable EXT_queue_family_foreign. Batches release their images to that
/// family in GENERAL layout before delivering completion; the shell supplies fence transport.
pub trait VkExternalImages: Send {
    fn registry(&self) -> Option<&NativeImages> { None }
    fn device_extensions(&self) -> Vec<&'static CStr>;
    fn create(&mut self, dev: Arc<Device>, desc: TextureObj) -> Result<Arc<Image>, String>;
    fn export(&mut self, session: u32, image: Arc<Image>) -> Result<u64, String>;
    fn import(&mut self, dev: Arc<Device>, handle: u64, kind: u32) -> Result<Arc<Image>, String>;
    fn session_closed(&mut self, session: u32);
}

struct Entry { session: u32, handle: Arc<NativeHandle>, image: Weak<Image> }

#[derive(Clone, Default)]
pub struct NativeImages { entries: Arc<Mutex<HashMap<u64, Entry>>> }
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

impl NativeImages {
    /// Register an externally synchronized descriptor. The shell keeps the native transport's
    /// fence/ownership protocol; a raw handle alone never grants permission for simultaneous
    /// external writes. The registry holds the descriptor until release or session close.
    ///
    /// # Safety
    /// The descriptor must match the native allocation. The producer must have finished its
    /// writes and released the image in GENERAL layout to foreign ownership.
    pub unsafe fn register(&self, session: u32, handle: NativeHandle) -> u64 {
        let id = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        lock(&self.entries).insert(id, Entry { session, handle: Arc::new(handle), image: Weak::new() }); id
    }
    /// Export tickets become externally readable only after TextureShared is delivered (the
    /// release-to-foreign barrier has completed). The shell must finish its external use before
    /// allowing further HAL access, or register the finished producer handle for import.
    pub fn handle(&self, ticket: u64) -> Option<Arc<NativeHandle>> { lock(&self.entries).get(&ticket).map(|e| e.handle.clone()) }
    pub fn release(&self, ticket: u64) { lock(&self.entries).remove(&ticket); }
    pub(super) fn retire(&self, session: u32, image: &Arc<Image>) {
        let image = Arc::downgrade(image);
        lock(&self.entries).retain(|_, entry| entry.session != session || !entry.image.ptr_eq(&image));
    }
}

impl VkExternalImages for NativeImages {
    fn registry(&self) -> Option<&NativeImages> { Some(self) }
    fn device_extensions(&self) -> Vec<&'static CStr> { NativeHandle::extensions() }
    fn create(&mut self, dev: Arc<Device>, desc: TextureObj) -> Result<Arc<Image>, String> { NativeHandle::allocate(dev, desc) }
    fn export(&mut self, session: u32, image: Arc<Image>) -> Result<u64, String> {
        let native = image.external.as_ref().ok_or("texture was not created SHARED")?.clone().downcast::<NativeHandle>().map_err(|_| "unknown external allocation")?;
        let id = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        lock(&self.entries).insert(id, Entry { session, handle: native, image: Arc::downgrade(&image) }); Ok(id)
    }
    fn import(&mut self, dev: Arc<Device>, handle: u64, kind: u32) -> Result<Arc<Image>, String> {
        if kind != texture_handle::NATIVE { return Err("unsupported Vulkan handle kind".into()); }
        let (native, image) = {
            let entries = lock(&self.entries); let entry = entries.get(&handle).ok_or("unknown native image ticket")?;
            (entry.handle.clone(), entry.image.upgrade())
        };
        // In-process sharing needs neither a second VkImage nor an ownership transfer.
        if let Some(image) = image.filter(|i| Arc::ptr_eq(&i.dev, &dev)) { return Ok(image); }
        NativeHandle::import(dev, native)
    }
    fn session_closed(&mut self, session: u32) { lock(&self.entries).retain(|_, e| e.session != session); }
}

#[cfg(target_os = "linux")]
impl crate::dmabuf::Api for super::Vulkan {
    fn supported(&self) -> Result<(), String> {
        NativeHandle::supported(&self.dev)?;
        let desc = TextureObj { format: crate::format::BGRA8, w: 1, h: 1, depth: 1, mips: 1,
            usage: crate::usage::SAMPLED | crate::usage::RENDER_TARGET | crate::usage::SHARED,
            bytes: 0, imported: false, uploaded: 0, lent: None };
        if NativeHandle::modifiers(&self.dev).iter().any(|m| m.drm_format_modifier_plane_count == 1
            && NativeHandle::properties(&self.dev, &desc, m.drm_format_modifier).is_ok_and(|p| p.external_memory_features.contains(
                makepad_vulkan::vk::ExternalMemoryFeatureFlags::EXPORTABLE | makepad_vulkan::vk::ExternalMemoryFeatureFlags::IMPORTABLE))) {
            Ok(())
        } else { Err("no shared BGRA8 DMA-BUF modifier".into()) }
    }
    fn support(&self) -> Option<DmaBufSupport> { self.foreign_support.clone() }
    fn register(&self, session: u32, descriptor: NativeHandle) -> Result<u64, String> {
        let registry = self.sharing.registry().ok_or("external provider owns its descriptor transport")?;
        Ok(unsafe { registry.register(session, descriptor) })
    }
    fn handle(&self, session: u32, ticket: u64) -> Option<Arc<NativeHandle>> {
        lock(&self.sharing.registry()?.entries).get(&ticket).filter(|e| e.session == session).map(|e| e.handle.clone())
    }
    fn release(&self, session: u32, ticket: u64) {
        if let Some(registry) = self.sharing.registry() {
            let mut entries = lock(&registry.entries);
            if entries.get(&ticket).is_some_and(|e| e.session == session) { entries.remove(&ticket); }
        }
    }
    fn acquire(&mut self, session: u32, id: crate::Id) -> Result<(), String> {
        let image = self.tex.get(&(session, id.index)).filter(|t| t.gen == id.gen).ok_or("missing DMA-BUF texture")?;
        if image.image.external.is_none() { return Err("texture is not external".into()); }
        // Every submitted use releases to FOREIGN/GENERAL. The next use's transition
        // acquires it again, after the compositor has observed the producer's fence.
        Ok(())
    }
    fn fence(&mut self, session: u32, notify: Arc<dyn Fn() + Send + Sync>) -> Result<crate::dmabuf::Fence, String> {
        let semaphore = sync::Semaphore::sync_file(self.dev.clone())?;
        self.flush();
        let look = self.mailbox.notify.clone();
        let (fence, reply) = crate::dmabuf::FenceReply::new(Arc::new(move || { notify(); look(session, false); }));
        self.ensure_batch()?.release_fence = Some((semaphore, reply));
        self.flush();
        Ok(fence)
    }
    fn available(&self, session: u32, id: crate::Id) -> Result<bool, String> {
        let image = self.tex.get(&(session, id.index)).filter(|t| t.gen == id.gen).ok_or("missing hosted texture")?;
        match &image.image.timeline { Some(t) if t.producer => t.available(), _ => Err("hosted image has no writer timeline".into()) }
    }
    fn sequence(&self, session: u32, id: crate::Id) -> Result<u64, String> {
        let image = self.tex.get(&(session, id.index)).filter(|t| t.gen == id.gen).ok_or("missing hosted texture")?;
        image.image.timeline.as_ref().filter(|t| t.producer).map(|t| t.sequence()).ok_or("hosted image has no writer timeline".into())
    }
    fn copy_frame(&mut self, session: &mut crate::session::Session, source: crate::Id, destination: crate::Id, sequence: u64) -> Result<(), String> {
        use makepad_vulkan::vk;
        let source = self.resolve(session, source).ok_or("missing hosted source")?;
        let destination = self.resolve(session, destination).ok_or("missing hosted snapshot")?;
        if source.desc.w != destination.desc.w || source.desc.h != destination.desc.h || source.desc.format != destination.desc.format
            || !source.supports_transfer() || !destination.supports_transfer()
            || destination.external.is_some() { return Err("hosted snapshot dimensions/format/storage differ".into()); }
        let timeline = source.timeline.clone().ok_or("hosted source has no timeline")?;
        self.flush();
        let batch = self.ensure_batch()?;
        timeline.accept(sequence)?;
        batch.external_waits.push((timeline.semaphore.clone(), sequence));
        batch.external_signals.push((timeline.semaphore.clone(), sequence + 1));
        source.transition(batch.cb, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        destination.transition(batch.cb, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        let sub = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        let copy = vk::ImageCopy::default().src_subresource(sub).dst_subresource(sub)
            .extent(vk::Extent3D { width: source.desc.w, height: source.desc.h, depth: 1 });
        unsafe { batch.dev.raw.cmd_copy_image(batch.cb, source.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            destination.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[copy]); }
        destination.transition(batch.cb, destination.sampled_layout());
        batch.images.extend([source, destination]);
        self.flush();
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub(super) fn foreign_support(dev: &Device) -> Option<DmaBufSupport> { NativeHandle::support(dev) }

/// A host compositor's shared image (a hosted window's swapchain image) as a render target of
/// this device, with the host's timeline: [`Image::hosted_writable`], [`Image::hosted_sequence`].
#[cfg(target_os = "linux")]
pub(crate) fn import_hosted(dev: Arc<Device>, native: Arc<NativeHandle>) -> Result<Arc<Image>, String> {
    let image = NativeHandle::import(dev, native)?;
    if image.timeline.as_ref().is_none_or(|t| !t.producer) { return Err("hosted image has no writer timeline".into()); }
    Ok(image)
}

#[cfg(target_os = "linux")]
impl Image {
    /// The host released this hosted image: the next frame may render into it.
    pub(crate) fn hosted_writable(&self) -> bool {
        self.timeline.as_ref().is_some_and(|t| t.producer && t.available().unwrap_or(false))
    }
    /// The timeline value that publishes the newest frame rendered into this hosted image.
    pub(crate) fn hosted_sequence(&self) -> Option<u64> {
        self.timeline.as_ref().filter(|t| t.producer).map(|t| t.sequence())
    }
}

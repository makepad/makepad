//! The shell supplies native surfaces; Vulkan owns swapchains. A surface generation changes
//! on loss/restoration, independently of every session resource and the device.

use super::device::{lock, Device, Worker};
use super::super::log_enabled;
use crate::Id;
use makepad_vulkan::vk;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Longest wait for a swapchain image (see `Swapchain::show`).
const ACQUIRE_WAIT_NS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceState {
    Offscreen,
    /// A presentation provider owns scanout and fans the complete window image out to outputs.
    Managed,
    Lost,
    Native { generation: u64, size: [u32; 2] },
}

/// An Android/Wayland/X11 shell implements this trait using its own synchronized handle
/// registry. `state` never waits; all sizes are physical pixels. The core calls `create` for a
/// new native generation or after VK_ERROR_SURFACE_LOST_KHR, keeping the native handle leased.
/// Creation is lazy, from FramePacket acquisition; the shell must deliver startup FrameReady
/// events before a surface/swapchain or a compositor frame callback exists.
pub trait VkSurfaceSource: Send {
    fn instance_extensions(&self) -> Vec<&'static CStr>;
    /// Called during device discovery, before any logical device is created.
    fn order_devices(&mut self, _instance: &makepad_vulkan::Instance, _devices: &mut Vec<vk::PhysicalDevice>) -> Result<(), String> { Ok(()) }
    fn device_ready(&mut self, _device: Arc<Device>) -> Result<(), String> { Ok(()) }
    fn presenter(&self, _session: u32, _window: Id) -> Option<Arc<dyn FramePresenter>> { None }
    fn state(&mut self, session: u32, window: Id, requested: [u32; 2]) -> SurfaceState;
    /// Whether the desktop compositor should blend this window's premultiplied alpha.
    fn transparent(&self, _session: u32, _window: Id) -> bool { false }
    /// A shell that hands out the window's render targets itself (a host compositor's shared
    /// images): the frame renders straight into the image returned here and the presenter gets
    /// that same image, with no window target and no copy in between. `None` (no free image of
    /// at least `size`, or a shell without its own targets) renders into the window target.
    fn target(&mut self, _session: u32, _window: Id, _size: [u32; 2]) -> Option<Arc<super::Image>> { None }
    /// Whether `image` is one of the targets [`VkSurfaceSource::target`] handed out.
    fn is_target(&self, _image: &Arc<super::Image>) -> bool { false }
    /// Create a surface for this instance. The returned lease must keep the native window
    /// alive until Vulkan destroys the surface (including pending presentation).
    ///
    /// # Safety
    /// The surface must belong to `instance` and be exclusively owned by the returned value.
    unsafe fn create(&mut self, _instance: &makepad_vulkan::Instance, _session: u32, _window: Id) -> Result<NativeSurface, String> {
        Err("this source owns presentation instead of a window-system surface".into())
    }
}

/// A native scanout provider receives an immutable whole-target image after its rendering
/// fence signals. It owns presentation pacing and reports actual presentation separately
/// from GPU completion. The image lease must survive all display reads, including peers.
pub trait FramePresenter: Send + Sync {
    fn submit(&self, session: u32, window: Id, image: Arc<super::Image>, presented: Box<dyn FnOnce(u64) + Send>);
}

pub struct NativeSurface {
    pub handle: vk::SurfaceKHR,
    pub keep: Arc<dyn std::any::Any + Send + Sync>,
}

/// GPU-real windows backed by render targets, with no display-server or Activity dependency.
pub struct Offscreen;
impl VkSurfaceSource for Offscreen {
    fn instance_extensions(&self) -> Vec<&'static CStr> { Vec::new() }
    #[cfg(target_os = "linux")]
    fn order_devices(&mut self, instance: &makepad_vulkan::Instance, devices: &mut Vec<vk::PhysicalDevice>) -> Result<(), String> {
        // Hosted processes inherit the display owner's render GPU, including a GPU
        // with no connected outputs. An unavailable explicit pin must not silently move.
        if let Ok(pin) = std::env::var("MAKEPAD_VULKAN_DEVICE_UUID") {
            if pin.len() != 32 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("MAKEPAD_VULKAN_DEVICE_UUID must contain 32 hex digits".into());
            }
            devices.retain(|physical| {
                let mut id = vk::PhysicalDeviceIDProperties::default();
                let mut properties = vk::PhysicalDeviceProperties2::default().next_physical_device_id_properties(&mut id);
                unsafe { instance.get_physical_device_properties2(*physical, &mut properties); }
                let uuid: String = id.device_uuid.iter().map(|b| format!("{b:02x}")).collect();
                pin.eq_ignore_ascii_case(&uuid)
            });
            if devices.is_empty() { return Err("inherited compositor GPU is unavailable".into()); }
        }
        Ok(())
    }
    fn state(&mut self, _: u32, _: Id, _: [u32; 2]) -> SurfaceState { SurfaceState::Offscreen }
    unsafe fn create(&mut self, _: &makepad_vulkan::Instance, _: u32, _: Id) -> Result<NativeSurface, String> {
        Err("offscreen windows have no VkSurfaceKHR".into())
    }
}

struct Surface { dev: Arc<Device>, native: NativeSurface }
impl Drop for Surface { fn drop(&mut self) { unsafe { self.dev.instance.destroy_surface(self.native.handle, None); } } }

pub(super) struct Semaphore { dev: Arc<Device>, pub raw: vk::Semaphore }
impl Semaphore {
    pub fn new(dev: Arc<Device>) -> Result<Arc<Self>, String> {
        let raw = unsafe { dev.raw.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }.map_err(|e| format!("vkCreateSemaphore: {e:?}"))?;
        Ok(Arc::new(Self { dev, raw }))
    }
}
impl Drop for Semaphore { fn drop(&mut self) { unsafe { self.dev.raw.destroy_semaphore(self.raw, None); } } }

pub(super) struct Swapchain {
    dev: Arc<Device>,
    surface: Arc<Surface>,
    pub raw: vk::SwapchainKHR,
    pub images: Vec<vk::Image>,
    ready: Vec<Arc<Semaphore>>,
    pub size: [u32; 2],
    // WSI host access is externally synchronized. The encoder only tries this lock;
    // it never waits for the submission worker's presentation call.
    host: Mutex<()>,
    key: (u32, Id),
    logged_acquire: AtomicBool,
    logged_present: AtomicBool,
    generation: u64,
    requested: [u32; 2],
    transparent: bool,
    pub dirty: AtomicBool,
    lost: AtomicBool,
    /// Replaced by a new swapchain; a frame still waiting for an image here is dropped.
    retired: AtomicBool,
    /// The newest frame queued for presentation here (set when the frame is encoded).
    pub latest: AtomicU64,
    /// A frame that found no free image, waiting on the queue worker for its next try
    /// (`Batch::show`). Only the queue worker takes or fills it.
    pub pending: Mutex<Option<super::device::LaterJob>>,
    /// Per swapchain image: the copy of the window target into it, recorded on the queue worker.
    blits: Mutex<Vec<Blit>>,
}

struct Blit { pool: vk::CommandPool, cb: vk::CommandBuffer, fence: vk::Fence, available: Option<Arc<Semaphore>> }

impl Drop for Swapchain {
    fn drop(&mut self) {
        unsafe {
            for b in lock(&self.blits).drain(..) {
                let _ = self.dev.raw.wait_for_fences(&[b.fence], true, u64::MAX);
                self.dev.raw.destroy_fence(b.fence, None);
                self.dev.raw.destroy_command_pool(b.pool, None);
            }
            self.dev.raw.destroy_swapchain(self.raw, None);
        }
    }
}

/// A rendered window target waiting to be shown in its window's swapchain.
pub(super) struct Present {
    pub chain: Arc<Swapchain>,
    /// The frame's window target, left in TRANSFER_SRC_OPTIMAL by the frame's command buffer.
    pub image: Arc<super::Image>,
    pub frame: u64,
}

pub(super) enum Shown {
    Presented,
    /// No swapchain image is free yet: try again later.
    NotReady,
    /// The swapchain is out of date, lost or replaced: this frame is not shown.
    Dropped,
}

impl Swapchain {
    /// Queue worker only, after the frame's own submission: acquire an image, copy the window
    /// target into it and present it. Acquisition, recording and presentation all happen here,
    /// never on the HAL thread.
    ///
    /// The acquire waits at most ACQUIRE_WAIT_NS (1 ms), not 0: the NVIDIA Wayland WSI learns
    /// that the compositor released a buffer only while it may wait in vkAcquireNextImageKHR
    /// (it reads its event queue there). With 0, after the third present every acquire was
    /// NOT_READY/TIMEOUT and every later frame was dropped (149, weston, video at ~0 fps shown).
    pub fn show(&self, image: &super::Image) -> Result<Shown, String> {
        if self.retired.load(Ordering::Acquire) || self.dirty.load(Ordering::Acquire) { return Ok(Shown::Dropped); }
        let _host = lock(&self.host);
        let available = Semaphore::new(self.dev.clone()).map_err(|e| format!("acquire semaphore key={:?}: {e}", self.key))?;
        let result = unsafe { self.dev.raw.acquire_next_image(self.raw, ACQUIRE_WAIT_NS, available.raw, vk::Fence::null()) };
        if log_enabled() && !self.logged_acquire.load(Ordering::Relaxed) && !self.logged_acquire.swap(true, Ordering::Relaxed) {
            eprintln!("[hal-vulkan] first vkAcquireNextImageKHR key={:?} swapchain={:?}: {result:?}", self.key, self.raw);
        }
        let index = match result {
            Ok((index, suboptimal)) => {
                if suboptimal {
                    if log_enabled() { eprintln!("[hal-vulkan] vkAcquireNextImageKHR key={:?}: SUBOPTIMAL_KHR; rebuilding swapchain", self.key); }
                    self.dirty.store(true, Ordering::Release);
                }
                index
            }
            Err(vk::Result::NOT_READY) | Err(vk::Result::TIMEOUT) => return Ok(Shown::NotReady),
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                eprintln!("[hal-vulkan] vkAcquireNextImageKHR key={:?}: ERROR_SURFACE_LOST_KHR; recreating surface", self.key);
                self.lost.store(true, Ordering::Release);
                self.dirty.store(true, Ordering::Release);
                return Ok(Shown::Dropped);
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                if log_enabled() { eprintln!("[hal-vulkan] vkAcquireNextImageKHR key={:?}: ERROR_OUT_OF_DATE_KHR; rebuilding swapchain", self.key); }
                self.dirty.store(true, Ordering::Release);
                return Ok(Shown::Dropped);
            }
            Err(e) => return Err(format!("vkAcquireNextImageKHR key={:?}: {e:?}", self.key)),
        };
        let dev = &self.dev.raw;
        let mut blits = lock(&self.blits);
        let b = &mut blits[index as usize];
        let dst = self.images[index as usize];
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let barrier = |old, new, src, dest| vk::ImageMemoryBarrier::default().image(dst).old_layout(old).new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(src).dst_access_mask(dest).subresource_range(range);
        let sub = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        let blit = vk::ImageBlit::default().src_subresource(sub).dst_subresource(sub)
            .src_offsets([vk::Offset3D::default(), vk::Offset3D { x: image.desc.w as i32, y: image.desc.h as i32, z: 1 }])
            .dst_offsets([vk::Offset3D::default(), vk::Offset3D { x: self.size[0] as i32, y: self.size[1] as i32, z: 1 }]);
        unsafe {
            // This image's previous copy has finished (its present waited on it); the fence says so.
            dev.wait_for_fences(&[b.fence], true, u64::MAX).map_err(|e| format!("blit fence: {e:?}"))?;
            dev.reset_fences(&[b.fence]).map_err(|e| format!("blit fence reset: {e:?}"))?;
            dev.reset_command_buffer(b.cb, vk::CommandBufferResetFlags::empty()).map_err(|e| format!("blit reset: {e:?}"))?;
            dev.begin_command_buffer(b.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(|e| format!("blit begin: {e:?}"))?;
            // The first scope covers the frame's own submission, earlier on this queue.
            dev.cmd_pipeline_barrier(b.cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::MEMORY_WRITE).dst_access_mask(vk::AccessFlags::TRANSFER_READ)], &[],
                &[barrier(vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)]);
            dev.cmd_blit_image(b.cb, image.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[blit], vk::Filter::NEAREST);
            dev.cmd_pipeline_barrier(b.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[],
                &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::PRESENT_SRC_KHR, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::empty())]);
            dev.end_command_buffer(b.cb).map_err(|e| format!("blit end: {e:?}"))?;
            let cbs = [b.cb];
            let waits = [available.raw];
            let stages = [vk::PipelineStageFlags::TRANSFER];
            let signals = [self.ready[index as usize].raw];
            let info = vk::SubmitInfo::default().command_buffers(&cbs).wait_semaphores(&waits).wait_dst_stage_mask(&stages).signal_semaphores(&signals);
            let _queue = lock(&self.dev.queue_lock);
            dev.queue_submit(self.dev.queue, &[info], b.fence).map_err(|e| format!("blit submit: {e:?}"))?;
        }
        b.available = Some(available); // waited on by that copy: kept until its fence is waited
        drop(blits);
        let waits = [self.ready[index as usize].raw];
        let chains = [self.raw];
        let indices = [index];
        let result = {
            let _queue = lock(&self.dev.queue_lock);
            unsafe { self.dev.raw.queue_present(self.dev.queue, &vk::PresentInfoKHR::default()
                .wait_semaphores(&waits).swapchains(&chains).image_indices(&indices)) }
        };
        if result.is_ok() && log_enabled() && !self.logged_present.load(Ordering::Relaxed)
            && !self.logged_present.swap(true, Ordering::Relaxed) {
            eprintln!("[hal-vulkan] first frame presented key={:?} generation={} swapchain={:?} image={index} size={:?} (vkQueuePresentKHR succeeded)",
                self.key, self.generation, self.raw, self.size);
        }
        match result {
            Ok(false) => Ok(Shown::Presented),
            Ok(true) => {
                if log_enabled() { eprintln!("[hal-vulkan] vkQueuePresentKHR key={:?}: SUBOPTIMAL_KHR; rebuilding swapchain", self.key); }
                self.dirty.store(true, Ordering::Release); Ok(Shown::Presented)
            }
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                eprintln!("[hal-vulkan] vkQueuePresentKHR key={:?}: ERROR_SURFACE_LOST_KHR; recreating surface", self.key);
                self.lost.store(true, Ordering::Release);
                self.dirty.store(true, Ordering::Release); Ok(Shown::Dropped)
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                if log_enabled() { eprintln!("[hal-vulkan] vkQueuePresentKHR key={:?}: ERROR_OUT_OF_DATE_KHR; rebuilding swapchain", self.key); }
                self.dirty.store(true, Ordering::Release); Ok(Shown::Dropped)
            }
            Err(e) => Err(format!("vkQueuePresentKHR key={:?}: {e:?}", self.key)),
        }
    }
}

pub(super) struct Windows { pub source: Box<dyn VkSurfaceSource>, chains: HashMap<(u32, Id), Arc<Swapchain>> }
impl Windows {
    pub fn new(source: Box<dyn VkSurfaceSource>) -> Self { Self { source, chains: HashMap::new() } }

    pub fn remove(&mut self, key: (u32, Id), worker: &Worker) {
        if let Some(old) = self.chains.remove(&key) { Self::retire(old, worker); }
    }

    fn retire(old: Arc<Swapchain>, worker: &Worker) {
        // Submission fences do not cover presentation. Vulkan 1.1's portable fallback is
        // queue idle, off the HAL/UI thread. Only this swapchain and surface are retired.
        if log_enabled() { eprintln!("[hal-vulkan] retire swapchain key={:?} generation={}; queuing presentation drain", old.key, old.generation); }
        old.retired.store(true, Ordering::Release);
        worker.push(move || {
            // a frame still waiting for an image of this swapchain completes unshown
            let waiting = lock(&old.pending).take();
            if let Some(waiting) = waiting { waiting(true); }
            if log_enabled() { eprintln!("[hal-vulkan] retire swapchain key={:?}: waiting for presentation queue idle", old.key); }
            let result = {
                let _queue = lock(&old.dev.queue_lock);
                unsafe { old.dev.raw.queue_wait_idle(old.dev.queue) }
            };
            if let Err(e) = result {
                eprintln!("[hal-vulkan] retire swapchain key={:?}: vkQueueWaitIdle failed: {e:?}", old.key);
            } else if log_enabled() {
                eprintln!("[hal-vulkan] retire swapchain key={:?}: presentation queue drained", old.key);
            }
            drop(old);
        });
    }

    pub fn close_session(&mut self, sid: u32, worker: &Worker) {
        let keys: Vec<_> = self.chains.keys().filter(|(s, _)| *s == sid).copied().collect();
        for key in keys { self.remove(key, worker); }
    }

    pub fn clear(&mut self, worker: &Worker) { for (_, old) in self.chains.drain() { Self::retire(old, worker); } }

    /// `true` means an offscreen window. The window's current swapchain, rebuilt here when its
    /// surface, size or alpha changed; the queue worker acquires and presents (`Swapchain::show`).
    pub fn swapchain(&mut self, dev: &Arc<Device>, key: (u32, Id), requested: [u32; 2], worker: &Worker) -> Result<(bool, Option<Arc<Swapchain>>), String> {
        let state = self.source.state(key.0, key.1, requested);
        if log_enabled() && !self.chains.contains_key(&key) {
            eprintln!("[hal-vulkan] surface acquisition without a swapchain key={key:?} requested={requested:?} state={state:?}; native surface creation is driven by FramePacket");
        }
        let (generation, size) = match state {
            SurfaceState::Offscreen => { self.remove(key, worker); return Ok((true, None)); }
            SurfaceState::Managed => { self.remove(key, worker); return Ok((false, None)); }
            SurfaceState::Lost => { self.remove(key, worker); return Ok((false, None)); }
            SurfaceState::Native { generation, size } => (generation, size),
        };
        if size.contains(&0) { self.remove(key, worker); return Ok((false, None)); }
        let transparent = self.source.transparent(key.0, key.1);
        let rebuild = self.chains.get(&key).is_none_or(|c| c.generation != generation || c.requested != size || c.transparent != transparent || c.dirty.load(Ordering::Acquire));
        if rebuild {
            if log_enabled() {
                eprintln!("[hal-vulkan] swapchain needed key={key:?} generation={generation} size={size:?} transparent={transparent}");
            }
            if self.chains.get(&key).is_some_and(|c| c.host.try_lock().is_err()) { return Ok((false, None)); }
            let old = self.chains.remove(&key);
            let same_surface = old.as_ref().filter(|o| o.generation == generation && !o.lost.load(Ordering::Acquire));
            let made = {
                let guard = old.as_ref().map(|o| o.host.try_lock());
                if guard.as_ref().is_some_and(|g| g.is_err()) {
                    drop(guard);
                    if let Some(old) = old { self.chains.insert(key, old); }
                    return Ok((false, None));
                }
                (|| {
                    let surface = if let Some(old) = same_surface { old.surface.clone() } else {
                        let native = unsafe { self.source.create(&dev.instance, key.0, key.1) }?;
                        Arc::new(Surface { dev: dev.clone(), native })
                    };
                    Self::create(dev.clone(), surface, key, generation, size, transparent, same_surface.map_or(vk::SwapchainKHR::null(), |o| o.raw))
                })()
            };
            if let Some(old) = old { Self::retire(old, worker); }
            self.chains.insert(key, made.map_err(|e| format!("swapchain creation key={key:?} generation={generation}: {e}"))?);
        }
        Ok((false, Some(self.chains.get(&key).expect("created swapchain").clone())))
    }

    fn create(dev: Arc<Device>, surface: Arc<Surface>, key: (u32, Id), generation: u64, requested: [u32; 2], transparent: bool, old: vk::SwapchainKHR) -> Result<Arc<Swapchain>, String> {
        if !unsafe { dev.instance.get_physical_device_surface_support(dev.physical, dev.family, surface.native.handle) }
            .map_err(|e| format!("vkGetPhysicalDeviceSurfaceSupportKHR: {e:?}"))? {
            return Err(format!("surface {:?} cannot present on selected device {:?} graphics queue family {}; check the Vulkan ICD and compositor WSI support", surface.native.handle, dev.physical, dev.family));
        }
        let caps = unsafe { dev.instance.get_physical_device_surface_capabilities(dev.physical, surface.native.handle) }
            .map_err(|e| format!("vkGetPhysicalDeviceSurfaceCapabilitiesKHR: {e:?}"))?;
        let mut formats = unsafe { dev.instance.get_physical_device_surface_formats(dev.physical, surface.native.handle) }
            .map_err(|e| format!("vkGetPhysicalDeviceSurfaceFormatsKHR: {e:?}"))?;
        let modes = unsafe { dev.instance.get_physical_device_surface_present_modes(dev.physical, surface.native.handle) }
            .map_err(|e| format!("vkGetPhysicalDeviceSurfacePresentModesKHR: {e:?}"))?;
        if log_enabled() {
            eprintln!("[hal-vulkan] surface key={key:?} queue_family={} present_supported=true caps={caps:?} formats={formats:?} present_modes={modes:?}", dev.family);
        }
        if !modes.contains(&vk::PresentModeKHR::FIFO) {
            return Err(format!("surface does not advertise required FIFO present mode: {modes:?}"));
        }
        if !caps.supported_usage_flags.contains(vk::ImageUsageFlags::TRANSFER_DST) {
            return Err(format!("surface lacks TRANSFER_DST usage needed for window blits; supported={:?}", caps.supported_usage_flags));
        }
        for f in &mut formats {
            if f.format == vk::Format::UNDEFINED { f.format = vk::Format::B8G8R8A8_UNORM; }
        }
        formats.sort_by_key(|f| if f.format == vk::Format::B8G8R8A8_UNORM { 0 } else { 1 });
        // A preferred format is not necessarily blit-capable on the chosen ICD. Try all
        // advertised formats before rejecting a compositor (including software Weston).
        let fmt = formats.iter().copied().find(|f| {
            let features = unsafe { dev.instance.get_physical_device_format_properties(dev.physical, f.format) }.optimal_tiling_features;
            if log_enabled() { eprintln!("[hal-vulkan] surface format candidate key={key:?} format={:?} features={features:?}", f.format); }
            features.contains(vk::FormatFeatureFlags::BLIT_DST)
        }).ok_or_else(|| format!("no advertised surface format supports BLIT_DST: {formats:?}"))?;
        let extent = if caps.current_extent.width != u32::MAX { caps.current_extent } else { vk::Extent2D {
            width: requested[0].clamp(caps.min_image_extent.width, caps.max_image_extent.width),
            height: requested[1].clamp(caps.min_image_extent.height, caps.max_image_extent.height),
        } };
        if extent.width == 0 || extent.height == 0 {
            return Err(format!("surface has zero drawable extent {extent:?}; requested={requested:?}"));
        }
        // The window image is rendered upright and blitted, so the chain is created un-rotated and
        // the compositor applies the display rotation (a chain pre-rotated by `current_transform`
        // would need the blit to rotate too). A pre-rotated surface reports its extent swapped.
        let (pre_transform, extent) = if caps.supported_transforms.contains(vk::SurfaceTransformFlagsKHR::IDENTITY) {
            // ROTATE_90, ROTATE_270 and their mirrored forms
            let swapped = caps.current_transform.as_raw() & 0xAA != 0;
            let extent = if swapped && caps.current_extent.width != u32::MAX { vk::Extent2D { width: extent.height, height: extent.width } } else { extent };
            (vk::SurfaceTransformFlagsKHR::IDENTITY, extent)
        } else { (caps.current_transform, extent) };
        let count = if caps.max_image_count == 0 { caps.min_image_count + 1 } else { (caps.min_image_count + 1).min(caps.max_image_count) };
        let alpha = [if transparent { vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED } else { vk::CompositeAlphaFlagsKHR::OPAQUE }, vk::CompositeAlphaFlagsKHR::OPAQUE, vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED, vk::CompositeAlphaFlagsKHR::INHERIT].into_iter()
            .find(|a| caps.supported_composite_alpha.contains(*a))
            .ok_or_else(|| format!("surface has no usable composite alpha mode: {:?}", caps.supported_composite_alpha))?;
        if log_enabled() {
            eprintln!("[hal-vulkan] vkCreateSwapchainKHR key={key:?} size={extent:?} images={count} format={:?} color_space={:?} alpha={alpha:?} present_mode=FIFO old={old:?}", fmt.format, fmt.color_space);
        }
        let raw = unsafe { dev.raw.create_swapchain(&vk::SwapchainCreateInfoKHR::default().surface(surface.native.handle)
            .min_image_count(count).image_format(fmt.format).image_color_space(fmt.color_space).image_extent(extent)
            .image_array_layers(1).image_usage(vk::ImageUsageFlags::TRANSFER_DST).image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(pre_transform).composite_alpha(alpha).present_mode(vk::PresentModeKHR::FIFO).clipped(true).old_swapchain(old), None) }
            .map_err(|e| format!("vkCreateSwapchainKHR size={extent:?} format={:?} color_space={:?} alpha={alpha:?} present_mode=FIFO: {e:?}", fmt.format, fmt.color_space))?;
        let mut out = Swapchain { dev, surface, raw, images: Vec::new(), ready: Vec::new(), size: [extent.width, extent.height],
            host: Mutex::new(()), key, logged_acquire: AtomicBool::new(false), logged_present: AtomicBool::new(false), generation, requested, transparent, dirty: AtomicBool::new(false), lost: AtomicBool::new(false),
            retired: AtomicBool::new(false), latest: AtomicU64::new(0), pending: Mutex::new(None), blits: Mutex::new(Vec::new()) };
        out.images = unsafe { out.dev.raw.get_swapchain_images(raw) }.map_err(|e| format!("vkGetSwapchainImagesKHR: {e:?}"))?;
        if out.images.is_empty() { return Err("vkGetSwapchainImagesKHR returned no images".into()); }
        for _ in &out.images {
            out.ready.push(Semaphore::new(out.dev.clone())?);
            let blit = unsafe {
                let pool = out.dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(out.dev.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None)
                    .map_err(|e| format!("blit command pool: {e:?}"))?;
                let cb = out.dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(|e| format!("blit command buffer: {e:?}"))?[0];
                let fence = out.dev.raw.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None)
                    .map_err(|e| format!("blit fence: {e:?}"))?;
                Blit { pool, cb, fence, available: None }
            };
            lock(&out.blits).push(blit);
        }
        if log_enabled() { eprintln!("[hal-vulkan] swapchain created key={key:?} generation={generation} handle={raw:?} images={} size={:?}", out.images.len(), out.size); }
        Ok(Arc::new(out))
    }
}

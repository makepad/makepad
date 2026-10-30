//! Vulkan HAL core. Commands, generations, font jobs and lending use the shared executor;
//! native surfaces are supplied by a shell through VkSurfaceSource. Linux desktop's CxVulkan
//! backend remains independent until its HAL switch.

#[cfg(target_os = "linux")]
#[path = "../linux/media/linux_video_gpu.rs"]
mod media_gpu;
#[cfg(target_os = "linux")]
#[path = "../linux/media/capture_gpu.rs"]
mod capture_gpu;
#[cfg(target_os = "linux")]
mod native_app;
mod device;
mod frame;
#[cfg(target_os = "android")]
mod native_layer;
mod pipeline;
mod prof;
mod readback;
pub(crate) mod resources;
pub(crate) mod share;
mod submit;
pub mod surface;
pub(crate) mod tap;
#[cfg(target_os = "linux")]
pub(crate) mod video_encode;

pub use device::Device;
pub use resources::Image;
pub use share::{NativeHandle, NativeImages, VkExternalImages};
#[cfg(target_os = "android")]
pub use share::{picture_ticket, release_picture_ticket, shared_images};
pub use surface::{Offscreen, VkSurfaceSource};

use super::exec::{self, Gpu, PipeStatus};
use super::{Backend, Pixels};
use crate::buffers::{CmdView, PipelineBlob};
use crate::ledger::{Kind, LedgerEntry};
use crate::session::*;
use crate::types::*;
pub(crate) use device::Worker;
use pipeline::{PassKey, RenderPass, State};
pub(crate) use resources::{buffer_usage, Buffer};
use submit::{Batch, Event, Mailbox, Token};
use crate::FastMap;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

struct Texture { gen: u32, image: Arc<Image> }
struct Retained { gen: u32, buffer: Arc<Buffer>, evictable: bool }
/// A session's pipeline. `desc` is its own create's: the program may be shared with an identical
/// shader of another session, whose names (string ids) are that session's; the GPU profiler names
/// the pipeline by this one.
struct Pipe { gen: u32, state: Arc<State>, epoch: u64, announced: bool, desc: PipelineDesc }

/// Native window-system work stays outside the GPU core.
pub trait Shell: Send {
    fn command(&mut self, session: u32, cmd: exec::OsCmd) -> bool;
    fn lent(&mut self, session: u32, alias: Id, host: Id, attach: bool);
    fn auto_adopt(&self) -> bool { false }
}

pub struct Vulkan {
    #[cfg(target_os = "linux")]
    native_apps: native_app::NativeApps,
    #[cfg(target_os = "linux")]
    media: super::linux::media::Media,
    #[cfg(target_os = "android")]
    media: super::android::media::AndroidMedia,
    #[cfg(target_os = "android")]
    native_layers: native_layer::Layers,
    #[cfg(target_os = "linux")]
    capture_pool: FastMap<(u32, Id), Vec<Arc<Buffer>>>,
    #[cfg(target_os = "linux")]
    shell: Option<&'static super::linux::Shared>,
    /// the Android shell (Activity surface, lifecycle, input)
    #[cfg(target_os = "android")]
    shell: Option<Box<dyn Shell>>,
    dev: Arc<Device>,
    info: HalInfo,
    tex: FastMap<(u32, u32), Texture>,
    buf: FastMap<(u32, u32), Retained>,
    pipe: FastMap<(u32, u32), Pipe>,
    programs: FastMap<Vec<u8>, Weak<State>>,
    /// Scratch of `prepare`: one draw's buffer and image descriptors, cleared per draw.
    draw_bins: Vec<(u32, makepad_vulkan::vk::DescriptorType, makepad_vulkan::vk::DescriptorBufferInfo)>,
    draw_images: Vec<(u32, makepad_vulkan::vk::DescriptorType, makepad_vulkan::vk::DescriptorImageInfo)>,
    passes: FastMap<PassKey, Arc<RenderPass>>,
    /// Framebuffers kept across frames by (render pass, attachment views, size): on the Adreno
    /// driver vkCreateFramebuffer is a kernel round trip (GPU memory), and one made and
    /// destroyed per pass per frame took 14 % of a swipe frame's CPU (`frame::encode_pass`).
    fbs: FastMap<frame::FbKey, frame::Fb>,
    /// The multisampled images of MSAA passes, by the attachment they resolve into (its raw
    /// image) and sample count; dropped with that attachment (`frame::msaa_image`).
    msaa: FastMap<(u64, u8), (std::sync::Weak<resources::Image>, Arc<resources::Image>)>,
    win: FastMap<(u32, u32), Texture>,
    /// A hosted window's newest frame size: its host's shared image is allocated larger than
    /// the window (resizes reuse it), and a snapshot or copy reads only what the frame drew.
    hosted_sizes: FastMap<(u32, u32), [u32; 2]>,
    scanout_images: FastMap<(u32, Id), Vec<Arc<Image>>>,
    /// MAKEPAD_FRAME_STAMP: the red and blue 8x8 blocks stamped into each frame handed to a host
    /// (`frame::stamp_frame`), and the size the last window pass drew.
    #[cfg(target_os = "linux")]
    stamp: Option<[Arc<Image>; 2]>,
    #[cfg(target_os = "linux")]
    window_px: [u32; 2],
    /// session 0's windows with a badge layer (by index): the images their layer was composed
    /// into (`frame::Badged`)
    badged: FastMap<u32, frame::Badged>,
    /// session 0's windows (by index) whose newest frame went straight to the shell's target, so
    /// their retained image holds an older one: a badge layer is never composed over it (the
    /// stale picture showed in the WM for a frame each time a badge came up)
    retained_stale: std::collections::HashSet<u32>,
    /// the composed image the badge frame being encoded draws into
    compose: Option<Arc<Image>>,
    aliases: FastMap<(u32, Id), Id>,
    pick_ids: FastMap<(u32, Id), Id>,
    pick_pending: HashSet<(u32, Id)>,
    fallback: FastMap<u32, Arc<Image>>,
    windows: surface::Windows,
    sharing: Box<dyn VkExternalImages>,
    #[cfg(target_os = "linux")]
    foreign_support: Option<share::DmaBufSupport>,
    batch: Option<Batch>,
    spares: submit::Spares,
    mailbox: Mailbox,
    queue: Option<Worker>,
    complete: Arc<Worker>,
    compiler: Worker,
    /// The UI's memory reserve and the creates waiting for room (`crate::gpu_pressure`).
    pressure: crate::gpu_pressure::Pressure,
    /// The instance extensions the device was made with (a replacement device gets the same).
    instance_extensions: Vec<&'static std::ffi::CStr>,
    /// the sessions heard `DeviceLost` and the replacement device is not made yet
    lost_posted: bool,
    /// when the next attempt to make a replacement device may run (HAL clock ns), and how
    /// many attempts failed (the wait doubles, up to `RESET_RETRY_MAX_NS`)
    retry_ns: u64,
    reset_failures: u32,
    /// `simulate_device_loss` asked for a loss: the next step treats the device as lost
    simulate: bool,
    /// `MAKEPAD_HAL_SIMULATE_DEVICE_LOSS_MS` (`super::simulated_loss_at`): a loss is simulated
    /// this long after the backend was made (app-level checks of the reset path, e.g. a grab
    /// before and after)
    simulate_at_ns: Option<u64>,
}

/// The first wait before another attempt at a replacement device, and the longest.
const RESET_RETRY_NS: u64 = 250_000_000;
const RESET_RETRY_MAX_NS: u64 = 8_000_000_000;

impl Vulkan {
    /// `sessions` makes a completion's session look; `notify` (the shell's events) every pump.
    pub fn new(flags: u32, sessions: super::SessionNotify, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        #[cfg(not(target_os = "linux"))]
        drop(notify);
        #[cfg(target_os = "linux")]
        if flags & open_flags::OFFSCREEN_WINDOWS == 0 {
            let shell = super::linux::Shared::global();
            shell.start(flags, notify)?;
            return Self::on_shell(flags, sessions, shell).inspect_err(|_| super::linux::stop_main_loop());
        }
        Self::with_sources(flags, sessions, Box::new(Offscreen), Box::new(NativeImages::default()))
    }

    /// On a started Linux shell (X11, Wayland or the DRM/KMS direct shell).
    #[cfg(target_os = "linux")]
    pub fn on_shell(flags: u32, sessions: super::SessionNotify, shell: &'static super::linux::Shared) -> Result<Self, String> {
        let mut backend = Self::with_sources(flags, sessions, super::linux::surface_source(shell), Box::new(NativeImages::default()))?;
        backend.info.flags |= info_flags::DISPLAY_CLOCK | info_flags::WINDOW_BADGES;
        if shell.client_chrome() { backend.info.flags |= info_flags::CLIENT_CHROME; }
        if shell.window_manager() { backend.info.flags |= info_flags::WM_HOSTED; }
        if shell.direct_display() { backend.info.flags |= info_flags::DIRECT_DISPLAY; }
        backend.info.dpi = shell.dpi();
        backend.shell = Some(shell);
        Ok(backend)
    }

    /// Shell construction seam. Native surface loss only retires Windows' swapchain objects.
    pub fn with_sources(flags: u32, sessions: super::SessionNotify, mut source: Box<dyn VkSurfaceSource>, sharing: Box<dyn VkExternalImages>) -> Result<Self, String> {
        if flags & open_flags::OFFSCREEN_WINDOWS != 0 { source = Box::new(Offscreen); }
        let mut extensions = source.instance_extensions();
        if !extensions.is_empty() && !extensions.contains(&makepad_vulkan::vk::KHR_SURFACE_NAME) { extensions.push(makepad_vulkan::vk::KHR_SURFACE_NAME); }
        let instance_extensions = extensions.clone();
        let dev = Device::shared(&extensions, &sharing.device_extensions(), &mut *source)?;
        source.device_ready(dev.clone())?;
        #[cfg(target_os = "linux")]
        let foreign_support = share::foreign_support(&dev);
        let formats = (0..format::COUNT).fold(0, |bits, f| {
            let properties = dev.formats[f as usize];
            if properties.optimal_tiling_features.contains(makepad_vulkan::vk::FormatFeatureFlags::SAMPLED_IMAGE) { bits | (1u64 << f) } else { bits }
        });
        let working_set = dev.memory.memory_heaps[..dev.memory.memory_heap_count as usize].iter().enumerate()
            .filter(|(heap, _)| dev.memory.memory_types[..dev.memory.memory_type_count as usize].iter().any(|ty|
                ty.heap_index as usize == *heap && ty.property_flags.contains(makepad_vulkan::vk::MemoryPropertyFlags::DEVICE_LOCAL)))
            .map(|(_, h)| h.size).sum();
        let info = HalInfo { backend: backend_id::VULKAN, dialect: dialect::SPIRV, formats, msaa: dev.msaa, dpi: 1.0,
            flags: if dev.timestamp_bits > 0 { info_flags::GPU_TIMESTAMPS } else { 0 } | if dev.multiview { info_flags::MULTIVIEW } else { 0 },
            max_texture: dev.properties.limits.max_image_dimension2_d,
            max_window: [dev.properties.limits.max_framebuffer_width, dev.properties.limits.max_framebuffer_height],
            max_buffer: dev.properties.limits.max_storage_buffer_range as u64, working_set,
            vendor_id: dev.properties.vendor_id,
            device_kind: match dev.properties.device_type {
                makepad_vulkan::vk::PhysicalDeviceType::INTEGRATED_GPU => device_kind::INTEGRATED,
                makepad_vulkan::vk::PhysicalDeviceType::DISCRETE_GPU => device_kind::DISCRETE,
                makepad_vulkan::vk::PhysicalDeviceType::CPU => device_kind::SOFTWARE,
                _ => device_kind::UNKNOWN,
            },
            ..Default::default() };
        let mut info = info;
        info.set_device_name(&unsafe { std::ffi::CStr::from_ptr(dev.properties.device_name.as_ptr()) }.to_string_lossy());
        crate::gpu_limits::set_max_gpu_buffer_bytes(info.max_buffer);
        crate::gpu_limits::set_gpu_working_set_bytes(info.working_set);
        Ok(Self { #[cfg(target_os = "linux")] native_apps: Default::default(), #[cfg(any(target_os = "linux", target_os = "android"))] media: Default::default(), #[cfg(target_os = "linux")] capture_pool: FastMap::default(), #[cfg(any(target_os = "linux", target_os = "android"))] shell: None, dev, info, tex: FastMap::default(), buf: FastMap::default(), pipe: FastMap::default(), programs: FastMap::default(), draw_bins: Vec::new(), draw_images: Vec::new(),
            #[cfg(target_os = "android")] native_layers: Default::default(),
            passes: FastMap::default(), fbs: FastMap::default(), msaa: FastMap::default(), win: FastMap::default(), hosted_sizes: FastMap::default(), scanout_images: FastMap::default(), #[cfg(target_os = "linux")] stamp: None, #[cfg(target_os = "linux")] window_px: [0, 0], badged: FastMap::default(), retained_stale: Default::default(), compose: None, aliases: FastMap::default(), pick_ids: FastMap::default(), pick_pending: HashSet::new(),
            fallback: FastMap::default(), windows: surface::Windows::new(source), sharing,
            #[cfg(target_os = "linux")] foreign_support,
            batch: None, spares: Default::default(), mailbox: Mailbox::new(sessions),
            queue: Some(Worker::new("makepad-vulkan-submit")?), complete: Arc::new(Worker::new("makepad-vulkan-complete")?),
            compiler: Worker::new("makepad-vulkan-compile")?, pressure: Default::default(),
            instance_extensions, lost_posted: false, retry_ns: 0, reset_failures: 0, simulate: false,
            simulate_at_ns: super::simulated_loss_at() })
    }

    #[cfg(target_os = "android")]
    pub fn set_shell(&mut self, shell: Box<dyn Shell>) { self.shell = Some(shell); }

    fn token(s: &Session) -> Token { Token { sid: s.id, incarnation: s.font_frames.clone() } }
    fn window_key(&self, sid: u32, id: Id) -> (u32, Id) { self.aliases.get(&(sid, id)).map_or((sid, id), |host| (HOST, *host)) }

    fn ensure_batch(&mut self) -> Result<&mut Batch, String> {
        if self.dev.lost.load(Ordering::Acquire) { return Err("Vulkan device lost".into()); }
        if self.batch.is_none() { self.batch = Some(Batch::new(self.dev.clone(), &self.spares, 0, 0)?); }
        Ok(self.batch.as_mut().unwrap())
    }

    fn flush(&mut self) {
        if let Some(batch) = self.batch.take() { batch.finish(self.queue.as_ref().unwrap(), self.complete.clone(), self.mailbox.clone()); }
    }

    #[cfg(target_os = "linux")]
    fn native_drain(&mut self, world: &mut World) {
        let mut native = std::mem::take(&mut self.native_apps);
        native.drain(self, world);
        self.native_apps = native;
    }

    fn resolve(&mut self, s: &mut Session, id: Id) -> Option<Arc<Image>> {
        let object = s.textures.get(s.id, id).ok()?.val.clone();
        let (sid, real) = object.lent.map_or((s.id, id), |l| (l.child, l.tex));
        if let Some(t) = self.tex.get(&(sid, real.index)).filter(|t| t.gen == real.gen && t.image.desc.w == object.w && t.image.desc.h == object.h) {
            return Some(t.image.clone());
        }
        if object.lent.is_some() || object.imported { return None; }
        // Embedded windows are host table entries whose storage is made at first render.
        let image = Image::new(self.dev.clone(), object).map_err(|e| eprintln!("[hal-vulkan] target allocation: {e}")).ok()?;
        // a create that starved has its storage now: it is no longer handed back to its app
        // (that released a texture already drawn into -- a host's copy of a hosted frame)
        if self.pressure.is_starved(s.id, id) { self.pressure.forget(s.id, id); }
        if let Ok(entry) = s.textures.get_mut(s.id, id) { entry.val.bytes = image.bytes; }
        if let Some(row) = s.ledger.get_mut(Kind::Texture, id.index) { row.bytes = image.bytes; }
        self.tex.insert((sid, id.index), Texture { gen: id.gen, image: image.clone() });
        Some(image)
    }

    fn drain(&mut self, world: &mut World) {
        for done in self.mailbox.take() {
            let sid = done.token.sid;
            if !world.session(sid).is_some_and(|s| Arc::ptr_eq(&s.font_frames, &done.token.incarnation)) { continue; }
            match done.event {
                #[cfg(target_os = "linux")]
                Event::Capture { window, surface } => {
                    let size = [surface.width as u16, surface.height as u16];
                    let handle = super::linux::media::capture_surface::publish(sid, surface);
                    world.to_session(sid, |q| q.screen_surface(window, size, handle));
                }
                Event::Presented { target, frame } => {
                    world.window_presented(sid, target, frame, 0, done.time);
                    #[cfg(target_os = "linux")] self.native_apps.presented(sid, target, frame);
                    #[cfg(target_os = "linux")]
                    if let Some(shell) = self.shell { shell.presented(self.window_key(sid, target)); }
                }
                Event::Frame { target, kind, frame, window, gpu_ns } => {
                    if let Some(s) = world.session_mut(sid) {
                        s.stats.frames += 1;
                        if gpu_ns > 0 { s.stats.gpu_frames += 1; s.stats.gpu_ns_total += gpu_ns; s.stats.gpu_ns_max = s.stats.gpu_ns_max.max(gpu_ns); }
                    }
                    if window {
                        #[cfg(target_os = "linux")] self.native_apps.presented(sid, target, frame);
                        world.window_presented(sid, target, frame, gpu_ns, done.time);
                        #[cfg(target_os = "linux")]
                        if let Some(shell) = self.shell { shell.presented(self.window_key(sid, target)); }
                    }
                    world.frame_done(sid, target, kind, frame, gpu_ns, done.time, [0.0; 2]);
                }
                Event::Readback { req, status, rect, pixels } => world.to_session(sid, |q| q.readback(req, status, rect, &pixels)),
                Event::Pick { req, ids } => world.to_session(sid, |q| q.pick_result(req, &ids.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>())),
                Event::Shared { id, handle } => world.to_session(sid, |q| q.texture_shared(id, handle)),
            }
        }
        for ((sid, index), pipe) in &mut self.pipe {
            let epoch = pipe.state.epoch.load(Ordering::Acquire);
            if epoch == pipe.epoch { continue; }
            pipe.epoch = epoch;
            let id = Id::new(*index, pipe.gen);
            let Some(s) = world.session_mut(*sid) else { continue };
            if !pipe.announced { pipe.announced = true; s.pipelines_pending = s.pipelines_pending.saturating_sub(1); }
            if let Some(e) = pipe.state.failure() {
                world.to_session(*sid, |q| q.pipeline_failed(id, e.as_bytes()));
            } else if matches!(pipe.state.program.get(), Some(Ok(_))) {
                world.to_session(*sid, |q| q.pipeline_ready(id));
            }
        }
        let allocated = self.dev.allocated.load(Ordering::Relaxed);
        for ((sid, index), texture) in &self.win {
            let id = Id::new(*index, texture.gen);
            if *sid != HOST && !self.scanout_images.contains_key(&(*sid,id)) { continue; }
            let Some(owner) = world.session_mut(*sid) else { continue };
            let extra: u64 = self.scanout_images.get(&(*sid,id)).into_iter().flatten().map(|image| image.bytes).sum();
            let bytes = texture.image.bytes + extra;
            if owner.ledger.get(Kind::Window, *index).is_none_or(|r| r.gen != texture.gen || r.bytes != bytes
                || r.width != texture.image.desc.w || r.height != texture.image.desc.h) {
                let mut row = window_row(id, &texture.image, crate::clock_ns(), owner.frame);
                row.bytes = bytes;
                owner.ledger.add(row);
            }
        }
        for sid in world.session_ids() { if let Some(s) = world.session_mut(sid) { s.ledger.device_allocated = Some(allocated); } }
    }

    /// A CPU implementation (lavapipe, SwiftShader): the default selection prefers hardware GL.
    pub fn software(&self) -> bool {
        self.dev.properties.device_type == makepad_vulkan::vk::PhysicalDeviceType::CPU
    }

    /// One look at the device's memory (`crate::gpu_pressure`): creates that waited for room
    /// go back to their apps once it is there; while free memory is under the UI's reserve the
    /// HAL first gives back what it holds for nothing (idle transient targets, evictable
    /// buffers), then tells every session how much is still missing.
    fn watch_memory(&mut self, world: &mut World) {
        // a submission or fence wait out of memory waits on its worker: ask for room now
        if self.dev.starved.swap(false, Ordering::AcqRel) { self.pressure.submit_starved(); }
        let now = crate::clock_ns();
        if !self.pressure.due(now) { return; }
        let Some(b) = self.dev.memory_budget(true) else {
            // No budget from the driver: a failed create goes back to its app at once.
            let (back, _) = self.pressure.check(now, u64::MAX / 4, 0, 0, 0);
            for (sid, id) in back { world.to_session(sid, |q| q.texture_evicted(id)); }
            return;
        };
        let reserve = crate::gpu_pressure::ui_reserve(b.size);
        let short = self.pressure.shortfall(b.free(), reserve);
        let freed = if self.pressure.sustained(now, short) {
            let world_now = world.now();
            let targets = self.trim_idle_targets(world, world_now);
            targets + if targets < short { self.evict_buffers(world, short - targets) } else { 0 }
        } else { 0 };
        let (back, signal) = self.pressure.check(now, b.free(), b.budget, reserve, freed);
        for (sid, id) in back { world.to_session(sid, |q| q.texture_evicted(id)); }
        if let Some(e) = signal {
            eprintln!("[hal-vulkan] GPU memory: {} MB free of {} MB, UI reserve {} MB: {}", e.free_mb, e.budget_mb, e.reserve_mb,
                if e.wanted_mb == 0 { "enough again".to_string() } else { format!("{} MB short{}, asking for room", e.wanted_mb, if e.failed { " (a create failed)" } else { "" }) });
            for sid in world.session_ids() {
                world.to_session(sid, |q| q.gpu_pressure(e.wanted_mb, e.free_mb, e.budget_mb, e.reserve_mb, e.failed));
            }
        }
    }

    /// Transient render targets nothing drew with for `IDLE_TARGET_NS` give their storage back
    /// (their apps make them again when next needed, as on Metal). Bytes given back.
    fn trim_idle_targets(&mut self, world: &mut World, now: u64) -> u64 {
        const IDLE_TARGET_NS: u64 = 3_000_000_000;
        let mut freed = 0;
        for sid in world.session_ids() {
            let Some(s) = world.session_mut(sid) else { continue };
            let idle: Vec<Id> = s.textures.keys_of(sid).into_iter().filter(|id| {
                s.textures.get(sid, *id).is_ok_and(|e| e.val.usage & usage::TRANSIENT != 0)
                    && s.ledger.get(Kind::Texture, id.index).is_some_and(|l| now.saturating_sub(l.last_used_ns.max(l.created_ns)) >= IDLE_TARGET_NS)
                    && self.tex.get(&(sid, id.index)).is_some_and(|t| t.gen == id.gen)
            }).collect();
            for id in idle {
                if let Some(t) = self.tex.remove(&(sid, id.index)) {
                    // frames still in flight hold the image; its memory goes when they finish
                    freed += t.image.bytes;
                    if let Some(l) = s.ledger.get_mut(Kind::Texture, id.index) { l.bytes = 0; }
                    s.events.texture_evicted(id);
                }
            }
        }
        freed
    }

    /// Explicit host memory-pressure input; only completed, evictable buffers are removed.
    /// The caller supplies the measured pressure target, rather than an arbitrary byte cap.
    pub fn evict_buffers(&mut self, world: &mut World, bytes_needed: u64) -> u64 {
        let mut victims: Vec<_> = self.buf.iter().filter(|(_, b)| b.evictable && Arc::strong_count(&b.buffer) == 1)
            .map(|(key, b)| (world.session(key.0).and_then(|s| s.ledger.get(Kind::Buffer, key.1)).map_or(0, |l| l.last_used_ns), *key, b.gen, b.buffer.bytes)).collect();
        victims.sort_by_key(|v| v.0);
        let mut freed = 0;
        for (_, key, gen, bytes) in victims {
            if freed >= bytes_needed { break; }
            self.buf.remove(&key);
            let id = Id::new(key.1, gen);
            if let Some(s) = world.session_mut(key.0) { let _ = s.buffers.remove(key.0, id); s.ledger.remove(Kind::Buffer, key.1); }
            world.to_session(key.0, |q| q.buffer_evicted(id));
            freed += bytes;
        }
        freed
    }
}

impl Backend for Vulkan {
    #[cfg(target_os = "linux")]
    fn dmabuf(&mut self) -> Option<&mut dyn crate::dmabuf::Api> { Some(self) }
    /// A sync_file semaphore signalled by the batch that takes the session's submitted work:
    /// answered once the batch is on the queue (the reader's GPU waits on it).
    #[cfg(target_os = "linux")]
    fn frame_completion(&mut self, session: u32, notify: Arc<dyn Fn() + Send + Sync>) -> Result<crate::completion::Completion, String> {
        let semaphore = share::sync::Semaphore::sync_file(self.dev.clone())?;
        self.flush();
        let look = self.mailbox.notify.clone();
        let (completion, reply) = crate::completion::CompletionReply::new(Arc::new(move || { notify(); look(session, false); }));
        self.ensure_batch()?.release_fence = Some((semaphore, reply));
        self.flush();
        Ok(completion)
    }
    fn info(&self) -> HalInfo { self.info }
    fn submit(&mut self, world: &mut World, sid: u32, view: &CmdView) { self.recover(world); self.drain(world); #[cfg(target_os = "linux")] self.native_drain(world); exec::submit(self, world, sid, view); self.flush(); }

    /// The pipeline variants this batch's frames draw with first (a target layout and blend /
    /// depth-write / cull state the program was not built with), compiled before the batch runs:
    /// a driver compile is milliseconds to tens of milliseconds, an app's first frame needs
    /// dozens, and under the HAL's lock every other session (the run's boot window among them)
    /// would wait for them. A pass whose layout is not known yet (a target this batch does not
    /// create) compiles in the frame as before.
    fn work_ahead(&mut self, world: &World, sid: u32, view: &CmdView) -> Option<Box<dyn FnOnce() + Send>> {
        use crate::cmds::HalCmd;
        let s = world.session(sid)?;
        let mut created = FastMap::default();
        for cmd in view.cmds {
            if let Some(HalCmd::TextureCreate { id, format, .. }) = cmd.decode(view.arena) { created.insert(id, format); }
        }
        let format_of = |id: Id| s.textures.get(sid, id).ok().map(|t| t.val.format).or_else(|| created.get(&id).copied());
        let mut needed: Vec<(Arc<State>, PassKey, u32)> = Vec::new();
        for cmd in view.cmds {
            let Some(HalCmd::Frame { target, kind, packet }) = cmd.decode(view.arena) else { continue };
            let Some(packet) = view.frames.get(packet as usize) else { continue };
            let Some(passes) = frame::slice(packet.passes, view.passes.len()) else { continue };
            let is_window = kind & !target::FLAGS == target::WINDOW;
            'pass: for pass in &view.passes[passes] {
                let mut key = PassKey { colors: [pipeline::ABSENT; 4], depth: pipeline::ABSENT, load: pass.load, views: frame::pass_views(pass), fdm: false,
                    samples: frame::pass_samples(pass, self.dev.msaa) };
                for (i, id) in pass.color.iter().enumerate().filter(|(_, id)| !id.is_none()) {
                    let Some(format) = format_of(*id) else { continue 'pass };
                    key.colors[i] = format;
                }
                if key.colors.iter().all(|c| *c == pipeline::ABSENT) && is_window {
                    // the window's own target: its embedded texture, else a BGRA8 image
                    key.colors[0] = match s.windows.get(sid, target).ok().and_then(|w| w.val.embedded) {
                        Some(em) => match format_of(em.tex) { Some(format) => format, None => continue 'pass },
                        None => { let (wsid, wid) = self.window_key(sid, target); self.windows.source.window_format(wsid, wid) }
                    };
                }
                if !pass.depth.is_none() {
                    let Some(format) = format_of(pass.depth) else { continue 'pass };
                    key.depth = format;
                }
                let Some(draws) = frame::slice(pass.draws, view.draws.len()) else { continue };
                for d in &view.draws[draws] {
                    let Some(p) = self.pipe.get(&(sid, d.pipeline.index)).filter(|p| p.gen == d.pipeline.gen) else { continue };
                    if p.state.lacks(key, d.flags) && !needed.iter().any(|(state, k, f)| Arc::ptr_eq(state, &p.state) && k.colors == key.colors && k.depth == key.depth && k.views == key.views && k.samples == key.samples && *f == d.flags) {
                        needed.push((p.state.clone(), key, d.flags));
                    }
                }
            }
        }
        if needed.is_empty() { return None; }
        let dev = self.dev.clone();
        Some(Box::new(move || {
            for (state, key, flags) in needed {
                match RenderPass::new(dev.clone(), key) {
                    Ok(pass) => { state.select(&pass, flags); }
                    Err(e) => eprintln!("[hal-vulkan] render pass for a pipeline variant: {e}"),
                }
            }
        }))
    }
    fn host_gpu(&mut self) -> Option<&mut dyn Gpu> { Some(self) }
    fn step(&mut self, world: &mut World, sid: u32) {
        self.flush(); self.drain(world);
        self.recover(world);
        #[cfg(target_os = "linux")] self.native_drain(world);
        #[cfg(target_os = "linux")]
        if let Some(shell) = self.shell {
            for (owner, id) in shell.drain_events(sid, world) {
                // A parent close also closes native popups, and an X server can destroy a
                // window independently. Retire their WSI even without a WindowClose command.
                // Command acknowledgements have already freed their table/chain and are harmless.
                let host = self.aliases.get(&(owner, id)).copied();
                let key = self.window_key(owner, id);
                self.window_closed(key.0, key.1);
                let _ = world.release_window(owner, id);
                if let Some(host) = host {
                    self.aliases.retain(|_, parent| *parent != host);
                    let _ = world.release_window(HOST, host);
                    world.to_session(HOST, |q| q.closed(host));
                }
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(s) = world.session_mut(sid) {
            let mut media = std::mem::take(&mut self.media);
            media.step(self, s);
            self.media = media;
            self.flush();
        }
        #[cfg(target_os = "android")]
        if let Some(s) = world.session_mut(sid) {
            let waiting: Vec<_> = self.native_layers.pending.keys().filter(|(owner, _)| *owner == sid).copied().collect();
            for key in waiting {
                if let Some((kind, frame)) = self.native_layers.pending.remove(&key) {
                    match self.native_layers.prepare(&self.dev, s, key.1, 0, &frame.view()) {
                        Ok(native_layer::Plan::Busy) => { self.native_layers.pending.insert(key, (kind, frame)); }
                        Ok(plan) => {
                            if let native_layer::Plan::Ready(plan) = plan { self.native_layers.frame = Some(plan); }
                            self.encode_frame(s, sid, key.1, kind, 0, &frame.view(), crate::clock_ns());
                        }
                        Err(e) => { eprintln!("[hal-android-layer] retry: {e}"); s.bad(); }
                    }
                }
            }
            let mut media = std::mem::take(&mut self.media);
            media.drain(self, s);
            self.media = media;
            self.flush();
        }
        self.watch_memory(world);
        exec::step_timers(world, sid);
    }
    fn next_due(&self, world: &World, sid: u32) -> Option<u64> {
        let watch = self.pressure.waiting().then(|| world.now() + crate::gpu_pressure::CHECK_NS);
        // a lost device is tried again (`recover`) without anything else to wake the HAL
        let reset = self.lost_posted.then(|| world.now() + RESET_RETRY_NS);
        let timer = exec::next_timer(world, sid).into_iter().chain(watch).chain(reset).min();
        #[cfg(target_os = "android")]
        let timer = timer.into_iter().chain(self.native_layers.pending.keys().any(|(s, _)| *s == sid).then(|| world.now() + 8_000_000)).min();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { return timer.into_iter().chain(self.media.next_due(sid, world.now())).min(); }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        timer
    }
    fn session_closed(&mut self, world: &mut World, sid: u32) {
        #[cfg(target_os = "android")]
        self.native_layers.close(sid);
        self.flush();
        #[cfg(target_os = "linux")]
        { let mut native = std::mem::take(&mut self.native_apps); native.close(self, world, sid); self.native_apps = native; }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        self.media.session_closed(sid);
        #[cfg(target_os = "linux")]
        self.capture_pool.retain(|key, _| key.0 != sid);
        self.windows.close_session(sid, self.queue.as_ref().unwrap());
        self.tex.retain(|k, _| k.0 != sid); self.buf.retain(|k, _| k.0 != sid); self.pipe.retain(|k, _| k.0 != sid);
        if sid == HOST { self.badged.clear(); self.retained_stale.clear(); }
        self.win.retain(|k, _| k.0 != sid); self.hosted_sizes.retain(|k, _| k.0 != sid); self.aliases.retain(|k, _| k.0 != sid);
        self.scanout_images.retain(|k, _| k.0 != sid);
        self.pick_ids.retain(|k, _| k.0 != sid); self.pick_pending.retain(|k| k.0 != sid);
        self.programs.retain(|_, state| state.strong_count() > 0);
        self.sharing.session_closed(sid);
        self.pressure.forget_session(sid);
        exec::free_session(world, sid);
        #[cfg(target_os = "linux")]
        if let Some(shell) = self.shell { shell.session_closed(sid); }
    }
    fn window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> { self.host_window_pixels(world, sid, window) }
    fn simulate_device_loss(&mut self) { self.simulate = true; }
    /// The process ends next: exit() runs the driver's exit handlers (NVIDIA's EGL teardown
    /// unloads its core), and a worker still in a driver call faulted inside it (a SIGSEGV in
    /// `submit_until_room` at a SIGTERM mid-frame; on makepad-vulkan-compile at a quit during
    /// boot). What was recorded is submitted, every swapchain retires, and the three workers
    /// stop in dependency order -- compiles; the queue (its submits, presents and retirements,
    /// which hand completions on); the completions (fence waits, frees) -- each joined.
    fn before_exit(&mut self) {
        self.dev.save_pipeline_cache();
        self.flush();
        if let Some(queue) = self.queue.as_ref() { self.windows.clear(queue); }
        self.compiler.stop();
        if let Some(queue) = self.queue.as_ref() { queue.stop(); }
        self.complete.stop();
    }
}

impl Vulkan {
    /// Compiles `state`'s program on the compile worker; its answer wakes the pumps of every
    /// session waiting on it (`State::wait`).
    fn compile(&self, state: &Arc<State>) {
        let job = Arc::downgrade(state); let dev = self.dev.clone(); let notify = self.mailbox.notify.clone();
        self.compiler.push(move || {
            // the pipelines that wanted it are gone (their session closed, they were
            // destroyed): no compile. A quit while an app's first pipelines were queued
            // compiled them all before the last session's close could settle the worker.
            let Some(job) = job.upgrade() else { return };
            let src = &job.src;
            // out of memory waits for room (asked for through `Device::starved`) and compiles
            // again: a program never fails for want of memory
            let mut tries = 0u64;
            let program = loop {
                match pipeline::Program::new(dev.clone(), src.desc, src.code.clone(), src.bindings.clone(), src.stride, None) {
                    Err(e) if pipeline::out_of_memory(&e) && !dev.lost.load(Ordering::Acquire) => {
                        if tries == 0 { eprintln!("[hal-vulkan] pipeline: {e}; waiting for GPU memory"); }
                        tries += 1;
                        dev.refused("pipeline compile");
                        // the host's pump raises the pressure (`watch_memory`)
                        notify(HOST, false);
                        std::thread::sleep(std::time::Duration::from_millis(16));
                    }
                    made => break made,
                }
            };
            if let Ok(program) = &program { job.predict(program); }
            let _ = job.program.set(program); job.epoch.fetch_add(1, Ordering::Release);
            for sid in job.answered() { notify(sid, false); }
        });
    }

    /// A lost device (the driver's VK_ERROR_DEVICE_LOST, or a simulated loss) is replaced
    /// here: every session hears `DeviceLost` once, then `exec::device_reset` runs until a
    /// device can be made (tried again after a doubling wait, logged once per loss).
    fn recover(&mut self, world: &mut World) {
        if self.simulate_at_ns.is_some_and(|t| crate::clock_ns() >= t) {
            self.simulate_at_ns = None;
            self.simulate = true;
        }
        // a simulated loss is this backend's alone: the process's device (other Hals share it)
        // keeps working, so its work in flight completes
        let simulated = std::mem::take(&mut self.simulate);
        if !(simulated || self.lost_posted || self.dev.lost.load(Ordering::Acquire)) { return; }
        if !self.lost_posted {
            let generation = exec::device_lost(world);
            eprintln!("[hal-vulkan] device generation {generation} lost; making a new one");
            self.lost_posted = true;
            self.retry_ns = 0;
            self.reset_failures = 0;
        }
        let now = crate::clock_ns();
        if now < self.retry_ns { return; }
        if exec::device_reset(self, world) {
            self.lost_posted = false;
            let name = unsafe { std::ffi::CStr::from_ptr(self.dev.properties.device_name.as_ptr()) };
            eprintln!("[hal-vulkan] device generation {} on {name:?}{}", world.device_generation,
                if self.reset_failures > 0 { format!(" after {} attempts", self.reset_failures + 1) } else { String::new() });
        } else {
            self.retry_ns = now + (RESET_RETRY_NS << self.reset_failures.min(5)).min(RESET_RETRY_MAX_NS);
            self.reset_failures += 1;
        }
    }
}

impl exec::DeviceReset for Vulkan {
    fn replace_device(&mut self, world: &mut World) -> Result<(), String> {
        let optional = self.sharing.device_extensions();
        let dev = Device::replace(&self.dev, &self.instance_extensions, &optional)?;
        // The old device's work: what was recorded is submitted (on a lost device it fails at
        // once), its workers answer every batch (readbacks and picks as failed, frames as
        // finished) and run out; those answers are delivered before the tables are emptied.
        self.flush();
        let queue = Worker::new("makepad-vulkan-submit")?;
        let complete = Arc::new(Worker::new("makepad-vulkan-complete")?);
        let compiler = Worker::new("makepad-vulkan-compile")?;
        let old_queue = self.queue.replace(queue).expect("live submit worker");
        let old_complete = std::mem::replace(&mut self.complete, complete);
        let old_compiler = std::mem::replace(&mut self.compiler, compiler);
        self.windows.clear(&old_queue);
        let limit = std::time::Duration::from_secs(2);
        if !(old_compiler.settle(limit) && old_queue.settle(limit) && old_complete.settle(limit)) {
            eprintln!("[hal-vulkan] device reset: the old device's work still runs after {limit:?}; its answers come later");
        }
        drop((old_queue, old_complete, old_compiler));
        self.drain(world);
        #[cfg(target_os = "linux")]
        self.capture_pool.clear();
        self.tex.clear();
        #[cfg(target_os = "android")]
        self.native_layers.reset();
        self.buf.clear();
        self.win.clear();
        self.hosted_sizes.clear();
        for b in self.badged.values_mut() { *b = Default::default(); }
        self.scanout_images.clear();
        self.fallback.clear();
        for (_, f) in self.fbs.drain() { unsafe { self.dev.raw.destroy_framebuffer(f.raw, None) }; }
        self.passes.clear();
        self.pick_ids.clear();
        self.pick_pending.clear();
        self.spares = Default::default();
        self.windows.source.device_ready(dev.clone())?;
        #[cfg(target_os = "linux")]
        {
            self.foreign_support = share::foreign_support(&dev);
            share::copy::device_replaced();
        }
        self.dev = dev;
        Ok(())
    }

    fn rebuild_pipelines(&mut self, world: &mut World) {
        self.programs.clear();
        let mut rebuilt: FastMap<*const State, Arc<State>> = FastMap::default();
        let keys: Vec<(u32, u32)> = self.pipe.keys().copied().collect();
        for key in keys {
            let old = self.pipe[&key].state.clone();
            let state = match rebuilt.get(&Arc::as_ptr(&old)) {
                Some(state) => state.clone(),
                None => {
                    let state = State::new(old.src.clone());
                    state.wait(key.0);
                    self.compile(&state);
                    self.programs.insert(state.src.key.clone(), Arc::downgrade(&state));
                    rebuilt.insert(Arc::as_ptr(&old), state.clone());
                    state
                }
            };
            state.wait(key.0);
            let pipe = self.pipe.get_mut(&key).expect("listed above");
            pipe.state = state;
            pipe.epoch = 0;
            // pending again until the new program answers
            if std::mem::replace(&mut pipe.announced, false) {
                if let Some(s) = world.session_mut(key.0) { s.pipelines_pending += 1; }
            }
        }
    }
}

impl Drop for Vulkan {
    fn drop(&mut self) {
        self.flush();
        for (_, f) in self.fbs.drain() { unsafe { self.dev.raw.destroy_framebuffer(f.raw, None) }; }
        self.windows.clear(self.queue.as_ref().unwrap());
        // Queue jobs retain the completion worker; both drain after the HAL lets them go.
        self.queue.take();
    }
}

impl Gpu for Vulkan {
    #[cfg(target_os = "linux")]
    fn native_layer(&mut self, world: &mut World, sid: u32, command: &crate::cmds::HalCmd) -> bool {
        let mut native = std::mem::take(&mut self.native_apps);
        let handled = native.command(self, world, sid, command);
        self.native_apps = native;
        handled
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn media(&mut self, s: &mut Session, c: &crate::cmds::HalCmd) -> bool {
        #[cfg(target_os = "android")]
        if self.native_layers.views.command(s, c) { return true; }
        let mut media = std::mem::take(&mut self.media);
        let handled = media.command(self, s, c);
        self.media = media;
        handled
    }
    #[cfg(target_os = "linux")]
    fn badges(&self) -> bool { self.shell.is_some() }
    fn badge_layer(&mut self, window: Id, on: bool) {
        if on { self.badged.entry(window.index).or_default(); } else { self.badged.remove(&window.index); }
    }

    #[cfg(target_os = "android")]
    fn auto_adopt(&self) -> bool { self.shell.as_ref().is_some_and(|s| s.auto_adopt()) }
    #[cfg(target_os = "android")]
    fn os(&mut self, sid: u32, cmd: exec::OsCmd) -> bool {
        if matches!(&cmd, exec::OsCmd::FileDialog { .. }) { return self.media.dialog(sid, cmd); }
        self.shell.as_mut().is_some_and(|s| s.command(sid, cmd))
    }
    #[cfg(target_os = "linux")]
    fn os(&mut self, sid: u32, command: exec::OsCmd) -> bool {
        if matches!(&command, exec::OsCmd::FileDialog { .. }) { return self.media.dialog(sid, command); }
        if let Some(shell) = self.shell { shell.command(sid, command); true } else { false }
    }
    fn gpu_budget(&self) -> Option<(u64, u64)> {
        self.dev.memory_budget(false).map(|b| (b.budget, b.usage))
    }
    fn texture_starved(&mut self, sid: u32, id: Id, t: &TextureObj, error: &str) -> bool {
        // only a device out of memory waits; any other failure is final for this create
        if !error.contains("OUT_OF_DEVICE_MEMORY") { return false; }
        let bytes = t.bytes.max(t.w as u64 * t.h as u64 * t.depth.max(1) as u64 * 4);
        let budget = self.dev.memory_budget(true);
        let free = budget.map_or(0, |b| b.free());
        // Refused with the UI's whole reserve (and the texture) free: not a full GPU but a device
        // the driver crippled -- one made while the GPU was full refused every later image of
        // its process even with tens of GB free again. It is replaced like a lost device.
        // (At the open-file limit every create is refused whatever the memory: that is not a
        // crippled device, and a new device would need more descriptors, not fewer.)
        let out_of_files = self.dev.out_of_files("texture create");
        let crippled = !out_of_files && budget.is_some_and(|b| b.free() >= crate::gpu_pressure::ui_reserve(b.size).saturating_add(bytes));
        if crippled && !self.dev.lost.swap(true, Ordering::AcqRel) {
            eprintln!("[hal-vulkan] a {bytes}-byte texture was refused with {} MB free: the device is replaced", free >> 20);
        }
        // (on a replaced device the room is there: it goes back to its app at the next look)
        self.pressure.starve(sid, id, bytes, crate::clock_ns(), if crippled { 0 } else { free });
        true
    }
    fn texture_create(&mut self, s: &mut Session, id: Id, t: &mut TextureObj) -> Result<(), String> {
        let image = if t.usage & usage::SHARED != 0 { self.sharing.create(self.dev.clone(), t.clone()) } else { Image::new(self.dev.clone(), t.clone()) }
            .map_err(|e| { eprintln!("[hal-vulkan] texture create {}x{}x{} format {} usage {:#x}: {e}", t.w, t.h, t.depth, t.format, t.usage); e })?;
        t.bytes = image.bytes;
        if let Some(old) = self.tex.insert((s.id, id.index), Texture { gen: id.gen, image }) {
            if let Some(registry) = self.sharing.registry() { registry.retire(s.id, &old.image); }
        }
        Ok(())
    }
    fn texture_update(&mut self, s: &mut Session, id: Id, t: &TextureObj, mip: u16, rect: [u32; 4], layer: u32, data: &[u8]) {
        if let Err(e) = self.upload(s, id, t, mip, rect, layer, data) {
            if pipeline::out_of_memory(&e) && self.dev.out_of_files("texture upload") { s.bad(); return; }
            eprintln!("[hal-vulkan] texture upload: {e}"); s.bad();
        }
    }
    fn texture_destroy(&mut self, sid: u32, id: Id) {
        #[cfg(target_os = "linux")] self.native_apps.forget_texture(sid, id);
        if self.tex.get(&(sid, id.index)).is_some_and(|t| t.gen <= id.gen) {
            if let Some(texture) = self.tex.remove(&(sid, id.index)) {
                if let Some(registry) = self.sharing.registry() { registry.retire(sid, &texture.image); }
            }
        }
        self.pick_pending.remove(&(sid, id));
        if self.pressure.is_starved(sid, id) { self.pressure.forget(sid, id); }
    }
    fn capture_texture(&mut self, s: &mut Session, id: Id, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        self.capture_storage(s, id, done);
    }
    fn capture_buffer(&mut self, sid: u32, id: Id) -> Option<Vec<u8>> {
        let r = self.buf.get(&(sid, id.index)).filter(|r| r.gen == id.gen)?;
        r.buffer.read(r.buffer.size as usize).ok()
    }
    fn texture_readback(&mut self, s: &mut Session, id: Id, req: u32, rect: [u32; 4]) {
        if self.pick_pending.contains(&(s.id, id)) { s.events.readback(req, crate::types::readback::INCOMPLETE, rect, &[]); return; }
        let image = self.resolve(s, id); self.read(s, image, req, rect, None);
    }
    fn texture_share(&mut self, s: &mut Session, id: Id) {
        let result = (|| {
            let image = self.tex.get(&(s.id, id.index)).filter(|t| t.gen == id.gen).ok_or("missing shared image")?.image.clone();
            let handle = self.sharing.export(s.id, image.clone())?;
            let batch = self.ensure_batch()?;
            batch.images.push(image);
            batch.tickets.push(submit::Ticket::Shared { token: Self::token(s), id, handle }); Ok::<_, String>(())
        })();
        if let Err(e) = result { eprintln!("[hal-vulkan] share: {e}"); s.events.texture_shared(id, 0); }
    }
    fn texture_import(&mut self, s: &mut Session, id: Id, handle: u64, kind: u32) -> Option<TextureObj> {
        let image = self.sharing.import(self.dev.clone(), handle, kind).map_err(|e| eprintln!("[hal-vulkan] import: {e}")).ok()?;
        let mut desc = image.desc.clone(); desc.imported = true; desc.bytes = 0;
        if let Some(old) = self.tex.insert((s.id, id.index), Texture { gen: id.gen, image }) {
            if let Some(registry) = self.sharing.registry() { registry.retire(s.id, &old.image); }
        }
        Some(desc)
    }
    fn buffer_create(&mut self, s: &mut Session, id: Id, b: &mut BufferObj) -> Result<(), String> {
        let buffer = Buffer::new(self.dev.clone(), b.size, buffer_usage())?; b.bytes = buffer.bytes;
        self.buf.insert((s.id, id.index), Retained { gen: id.gen, buffer, evictable: b.evictable }); Ok(())
    }
    fn buffer_update(&mut self, s: &mut Session, id: Id, b: &mut BufferObj, off: u64, data: &[u8]) {
        let result = (|| {
            let retained = self.buf.get_mut(&(s.id, id.index)).filter(|r| r.gen == id.gen).ok_or("missing retained buffer")?;
            if Arc::strong_count(&retained.buffer) > 1 {
                let fresh = Buffer::new(self.dev.clone(), b.size, buffer_usage())?;
                if off != 0 || data.len() as u64 != b.size { fresh.write(0, &retained.buffer.read(b.size as usize)?)?; }
                retained.buffer = fresh;
            }
            retained.buffer.write(off, data)?; b.bytes = retained.buffer.bytes; Ok::<_, String>(())
        })();
        if let Err(e) = result { eprintln!("[hal-vulkan] buffer update: {e}"); s.bad(); }
    }
    fn buffer_destroy(&mut self, sid: u32, id: Id) { if self.buf.get(&(sid, id.index)).is_some_and(|b| b.gen <= id.gen) { self.buf.remove(&(sid, id.index)); } }
    fn buffer_evictable(&mut self, sid: u32, id: Id) { if let Some(b) = self.buf.get_mut(&(sid, id.index)).filter(|b| b.gen == id.gen) { b.evictable = true; } }
    fn pipeline_create(&mut self, s: &mut Session, id: Id, dialect: u32, blob: &PipelineBlob, _: Attr) -> PipeStatus {
        if dialect != self::dialect::SPIRV { return PipeStatus::Failed("Vulkan takes binary SPIR-V".into()); }
        let code = match crate::spirv::SpirvPipeline::decode(blob.code) { Ok(c) => c, Err(e) => return PipeStatus::Failed(e) };
        let mut key = blob.code.to_vec();
        key.extend_from_slice(&(blob.bindings.len() as u64).to_le_bytes());
        key.extend_from_slice(&(blob.vertex_layout.len() as u64).to_le_bytes());
        for v in blob.bindings.iter().chain(blob.vertex_layout.iter()).copied().chain([blob.desc.blend, blob.desc.instance_stride, blob.desc.outputs]) { key.extend_from_slice(&v.to_le_bytes()); }
        let state = match self.programs.get(&key).and_then(Weak::upgrade) {
            Some(state) => state,
            None => {
                let stride = blob.vertex_layout.first().copied().unwrap_or(0);
                let state = State::new(Arc::new(pipeline::Source { desc: blob.desc, code, bindings: blob.bindings.clone(), stride, key: key.clone() }));
                // registered before the compile is queued, so its answer cannot be missed
                state.wait(s.id);
                self.compile(&state);
                self.programs.insert(key, Arc::downgrade(&state)); state
            }
        };
        state.wait(s.id);
        self.pipe.insert((s.id, id.index), Pipe { gen: id.gen, state, epoch: 0, announced: false, desc: blob.desc }); PipeStatus::Pending
    }
    fn pipeline_destroy(&mut self, sid: u32, id: Id) -> bool {
        if self.pipe.get(&(sid, id.index)).is_some_and(|p| p.gen <= id.gen) { return self.pipe.remove(&(sid, id.index)).is_some_and(|p| !p.announced); } false
    }
    fn frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<exec::SubmittedFrame> {
        // pipelines were made and none for two seconds: the cache goes to its file (compile worker)
        if self.dev.pipelines_made.load(std::sync::atomic::Ordering::Relaxed) > 0 && now.saturating_sub(self.dev.pipeline_made_ns.load(std::sync::atomic::Ordering::Relaxed)) > 2_000_000_000 {
            let dev = self.dev.clone();
            self.compiler.push(move || dev.save_pipeline_cache());
        }
        #[cfg(target_os = "android")]
        if kind & !target::FLAGS == target::WINDOW && kind & (target::PICK | target::BADGES | target::REPLAY) == 0 {
            self.media.native_frame(s, target, packet, view);
            match self.native_layers.prepare(&self.dev, s, target, packet, view) {
                Ok(native_layer::Plan::Ready(frame)) => self.native_layers.frame = Some(frame),
                Ok(native_layer::Plan::Plain) => {},
                Ok(native_layer::Plan::Busy) => {
                    if let Some(frame) = KeptFrame::keep(view, packet) {
                        if let Some((_, previous)) = self.native_layers.pending.insert((sid, target), (kind, frame)) {
                            let frame = previous.view().frames[0].frame;
                            s.events.frame_done(target, frame, 0, 0, 0.0, 0.0);
                        }
                    }
                    return None;
                }
                Err(e) => { eprintln!("[hal-android-layer] {e}"); s.bad(); return None; }
            }
            if let Some((_, previous)) = self.native_layers.pending.remove(&(sid, target)) {
                s.events.frame_done(target, previous.view().frames[0].frame, 0, 0, 0.0, 0.0);
            }
        }
        #[cfg(target_os = "linux")]
        {
            let mut native = std::mem::take(&mut self.native_apps);
            native.begin(self, s);
            self.native_apps = native;
        }
        let result = self.encode_frame(s, sid, target, kind, packet, view, now);
        #[cfg(target_os = "linux")]
        if let Some(packet) = view.frames.get(packet as usize) {
            self.native_apps.end(sid, target, packet.frame, result.as_ref().is_some_and(|r| r.window ||
                crate::hosted_native::bridge_enabled() && kind & !crate::target::FLAGS == crate::target::WINDOW));
        }
        result
    }
    fn pick(&mut self, s: &mut Session, window: Id, req: u32, pos: [f32; 2]) { self.read_pick(s, window, req, pos); }
    fn snapshot(&mut self, s: &mut Session, window: Id, req: u32) {
        let image = self.window_image(s, window); let rect = self.window_rect(s.id, window, image.as_deref()); self.read(s, image, req, rect, None);
    }
    fn window_copy(&mut self, s: &mut Session, window: Id, texture: Id, req: u32) { self.copy_window(s, window, texture, req); }
    fn window_closed(&mut self, sid: u32, id: Id) {
        #[cfg(target_os = "android")]
        self.native_layers.close_window(sid, id);
        #[cfg(target_os = "linux")]
        self.capture_pool.remove(&(sid, id));
        self.win.remove(&(sid, id.index)); self.hosted_sizes.remove(&(sid, id.index)); self.pick_ids.remove(&(sid, id));
        self.scanout_images.remove(&(sid, id));
        if sid == HOST { self.badged.remove(&id.index); self.retained_stale.remove(&id.index); }
        self.windows.remove((sid, id), self.queue.as_ref().unwrap());
    }
    fn window_lent(&mut self, sid: u32, id: Id, host: Id, attach: bool) {
        #[cfg(target_os = "linux")]
        if let Some(shell) = self.shell { shell.lend(host, if attach { sid } else { HOST }, if attach { id } else { host }); }
        #[cfg(target_os = "android")]
        if let Some(shell) = &mut self.shell { shell.lent(sid, id, host, attach); }
        if attach { self.aliases.insert((sid, id), host); } else { self.aliases.remove(&(sid, id)); self.pick_ids.remove(&(sid, id)); }
    }
}

fn window_row(id: Id, image: &Image, now: u64, frame: u64) -> LedgerEntry {
    LedgerEntry { kind: Kind::Window, detail: "window surface", index: id.index, gen: id.gen, bytes: image.bytes,
        format: image.desc.format, width: image.desc.w, height: image.desc.h, mips: 1, attr: Attr::default(),
        created_ns: now, created_frame: frame, last_used_frame: frame, last_used_ns: now }
}

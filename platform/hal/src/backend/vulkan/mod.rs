//! Vulkan HAL core. Commands, generations, font jobs and lending use the shared executor;
//! native surfaces are supplied by a shell through VkSurfaceSource. Linux desktop's CxVulkan
//! backend remains independent until its HAL switch.

#[cfg(target_os = "linux")]
#[path = "../linux/media/linux_video_gpu.rs"]
mod media_gpu;
#[cfg(target_os = "linux")]
#[path = "../linux/media/capture_gpu.rs"]
mod capture_gpu;
mod device;
mod frame;
mod pipeline;
mod readback;
mod resources;
pub(crate) mod share;
mod submit;
pub mod surface;

pub use device::Device;
pub use resources::Image;
pub use share::{NativeHandle, NativeImages, VkExternalImages};
pub use surface::{Offscreen, VkSurfaceSource};

use super::exec::{self, Gpu, PipeStatus};
use super::{Backend, Pixels};
use crate::buffers::{CmdView, PipelineBlob};
use crate::ledger::{Kind, LedgerEntry};
use crate::session::*;
use crate::types::*;
use device::Worker;
use pipeline::{PassKey, RenderPass, State};
use resources::{buffer_usage, Buffer};
use submit::{Batch, Event, Mailbox, Token};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

struct Texture { gen: u32, image: Arc<Image> }
struct Retained { gen: u32, buffer: Arc<Buffer>, evictable: bool }
struct Pipe { gen: u32, state: Arc<State>, epoch: u64, announced: bool }

/// Native window-system work stays outside the GPU core.
pub trait Shell: Send {
    fn command(&mut self, session: u32, cmd: exec::OsCmd) -> bool;
    fn lent(&mut self, session: u32, alias: Id, host: Id, attach: bool);
    fn auto_adopt(&self) -> bool { false }
}

pub struct Vulkan {
    #[cfg(target_os = "linux")]
    media: super::linux::media::Media,
    #[cfg(target_os = "android")]
    media: super::android::media::AndroidMedia,
    #[cfg(target_os = "linux")]
    capture_pool: HashMap<(u32, Id), Vec<Arc<Buffer>>>,
    #[cfg(target_os = "linux")]
    shell: Option<&'static super::linux::Shared>,
    /// the Android shell (Activity surface, lifecycle, input)
    #[cfg(target_os = "android")]
    shell: Option<Box<dyn Shell>>,
    dev: Arc<Device>,
    info: HalInfo,
    tex: HashMap<(u32, u32), Texture>,
    buf: HashMap<(u32, u32), Retained>,
    pipe: HashMap<(u32, u32), Pipe>,
    programs: HashMap<Vec<u8>, Weak<State>>,
    /// Scratch of `prepare`: one draw's buffer and image descriptors, cleared per draw.
    draw_bins: Vec<(u32, makepad_vulkan::vk::DescriptorType, makepad_vulkan::vk::DescriptorBufferInfo)>,
    draw_images: Vec<(u32, makepad_vulkan::vk::DescriptorType, makepad_vulkan::vk::DescriptorImageInfo)>,
    passes: HashMap<PassKey, Arc<RenderPass>>,
    win: HashMap<(u32, u32), Texture>,
    scanout_images: HashMap<(u32, Id), Vec<Arc<Image>>>,
    aliases: HashMap<(u32, Id), Id>,
    pick_ids: HashMap<(u32, Id), Id>,
    pick_pending: HashSet<(u32, Id)>,
    fallback: HashMap<u32, Arc<Image>>,
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
}

impl Vulkan {
    pub fn new(flags: u32, wake: Arc<dyn Fn() + Send + Sync>, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        #[cfg(not(target_os = "linux"))]
        drop(notify);
        #[cfg(target_os = "linux")]
        if flags & open_flags::OFFSCREEN_WINDOWS == 0 {
            let shell = super::linux::Shared::global();
            shell.start(flags, notify)?;
            return Self::on_shell(flags, wake, shell).inspect_err(|_| super::linux::stop_main_loop());
        }
        Self::with_sources(flags, wake, Box::new(Offscreen), Box::new(NativeImages::default()))
    }

    /// On a started Linux shell (X11, Wayland or the DRM/KMS direct shell).
    #[cfg(target_os = "linux")]
    pub fn on_shell(flags: u32, wake: Arc<dyn Fn() + Send + Sync>, shell: &'static super::linux::Shared) -> Result<Self, String> {
        let mut backend = Self::with_sources(flags, wake, super::linux::surface_source(shell), Box::new(NativeImages::default()))?;
        backend.info.flags |= info_flags::DISPLAY_CLOCK;
        if shell.client_chrome() { backend.info.flags |= info_flags::CLIENT_CHROME; }
        if shell.direct_display() { backend.info.flags |= info_flags::DIRECT_DISPLAY; }
        backend.info.dpi = shell.dpi();
        backend.shell = Some(shell);
        Ok(backend)
    }

    /// Shell construction seam. Native surface loss only retires Windows' swapchain objects.
    pub fn with_sources(flags: u32, wake: Arc<dyn Fn() + Send + Sync>, mut source: Box<dyn VkSurfaceSource>, sharing: Box<dyn VkExternalImages>) -> Result<Self, String> {
        if flags & open_flags::OFFSCREEN_WINDOWS != 0 { source = Box::new(Offscreen); }
        let mut extensions = source.instance_extensions();
        if !extensions.is_empty() && !extensions.contains(&makepad_vulkan::vk::KHR_SURFACE_NAME) { extensions.push(makepad_vulkan::vk::KHR_SURFACE_NAME); }
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
        let info = HalInfo { backend: backend_id::VULKAN, dialect: dialect::SPIRV, formats, msaa: 1, dpi: 1.0,
            flags: if dev.timestamp_bits > 0 { info_flags::GPU_TIMESTAMPS } else { 0 },
            max_texture: dev.properties.limits.max_image_dimension2_d,
            max_window: [dev.properties.limits.max_framebuffer_width, dev.properties.limits.max_framebuffer_height],
            max_buffer: dev.properties.limits.max_storage_buffer_range as u64, working_set, ..Default::default() };
        crate::gpu_limits::set_max_gpu_buffer_bytes(info.max_buffer);
        crate::gpu_limits::set_gpu_working_set_bytes(info.working_set);
        Ok(Self { #[cfg(any(target_os = "linux", target_os = "android"))] media: Default::default(), #[cfg(target_os = "linux")] capture_pool: HashMap::new(), #[cfg(any(target_os = "linux", target_os = "android"))] shell: None, dev, info, tex: HashMap::new(), buf: HashMap::new(), pipe: HashMap::new(), programs: HashMap::new(), draw_bins: Vec::new(), draw_images: Vec::new(),
            passes: HashMap::new(), win: HashMap::new(), scanout_images: HashMap::new(), aliases: HashMap::new(), pick_ids: HashMap::new(), pick_pending: HashSet::new(),
            fallback: HashMap::new(), windows: surface::Windows::new(source), sharing,
            #[cfg(target_os = "linux")] foreign_support,
            batch: None, spares: Default::default(), mailbox: Mailbox::new(wake),
            queue: Some(Worker::new("makepad-vulkan-submit")?), complete: Arc::new(Worker::new("makepad-vulkan-complete")?),
            compiler: Worker::new("makepad-vulkan-compile")?, pressure: Default::default() })
    }

    #[cfg(target_os = "android")]
    pub fn set_shell(&mut self, shell: Box<dyn Shell>) { self.shell = Some(shell); }

    fn token(s: &Session) -> Token { Token { sid: s.id, incarnation: s.font_frames.clone() } }
    fn window_key(&self, sid: u32, id: Id) -> (u32, Id) { self.aliases.get(&(sid, id)).map_or((sid, id), |host| (HOST, *host)) }

    fn ensure_batch(&mut self) -> Result<&mut Batch, String> {
        if self.dev.lost.load(Ordering::Acquire) { return Err("Vulkan device lost".into()); }
        if self.batch.is_none() { self.batch = Some(Batch::new(self.dev.clone(), &self.spares, false, 0)?); }
        Ok(self.batch.as_mut().unwrap())
    }

    fn flush(&mut self) {
        if let Some(batch) = self.batch.take() { batch.finish(self.queue.as_ref().unwrap(), self.complete.clone(), self.mailbox.clone()); }
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
                    #[cfg(target_os = "linux")]
                    if let Some(shell) = self.shell { shell.presented(self.window_key(sid, target)); }
                }
                Event::Frame { target, kind, frame, window, gpu_ns } => {
                    if let Some(s) = world.session_mut(sid) {
                        s.stats.frames += 1;
                        if gpu_ns > 0 { s.stats.gpu_frames += 1; s.stats.gpu_ns_total += gpu_ns; s.stats.gpu_ns_max = s.stats.gpu_ns_max.max(gpu_ns); }
                    }
                    if window {
                        world.window_presented(sid, target, frame, gpu_ns, done.time);
                        #[cfg(target_os = "linux")]
                        if let Some(shell) = self.shell { shell.presented(self.window_key(sid, target)); }
                    }
                    world.frame_done(sid, target, kind, frame, gpu_ns, done.time, [0.0; 2]);
                }
                Event::Readback { req, status, rect, pixels } => world.to_session(sid, |q| q.readback(req, status, rect, &pixels)),
                Event::Pick { req, ids } => world.to_session(sid, |q| q.pick_result(req, &ids.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>())),
                Event::Shared { id, handle } => world.to_session(sid, |q| q.texture_shared(id, handle)),
                Event::Lost => { let g = world.device_generation; world.to_session(sid, |q| q.device_lost(g)) }
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
        let freed = if short > 0 {
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
    fn info(&self) -> HalInfo { self.info }
    fn submit(&mut self, world: &mut World, sid: u32, view: &CmdView) { self.drain(world); exec::submit(self, world, sid, view); self.flush(); }
    fn host_gpu(&mut self) -> Option<&mut dyn Gpu> { Some(self) }
    fn step(&mut self, world: &mut World, sid: u32) {
        self.flush(); self.drain(world);
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
        let timer = exec::next_timer(world, sid).into_iter().chain(watch).min();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        { return timer.into_iter().chain(self.media.next_due(sid, world.now())).min(); }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        timer
    }
    fn session_closed(&mut self, world: &mut World, sid: u32) {
        self.flush();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        self.media.session_closed(sid);
        #[cfg(target_os = "linux")]
        self.capture_pool.retain(|key, _| key.0 != sid);
        self.windows.close_session(sid, self.queue.as_ref().unwrap());
        self.tex.retain(|k, _| k.0 != sid); self.buf.retain(|k, _| k.0 != sid); self.pipe.retain(|k, _| k.0 != sid);
        self.win.retain(|k, _| k.0 != sid); self.aliases.retain(|k, _| k.0 != sid);
        self.scanout_images.retain(|k, _| k.0 != sid);
        self.pick_ids.retain(|k, _| k.0 != sid); self.pick_pending.retain(|k| k.0 != sid);
        self.programs.retain(|_, state| state.strong_count() > 0);
        self.sharing.session_closed(sid);
        self.pressure.forget_session(sid);
        exec::free_session(world, sid);
        // The last app session closed: its process is about to exit. The frames still in flight
        // free their buffers and descriptor pools on the queue workers when their fences signal;
        // exit() unloads the driver under them (a SIGSEGV in the ICD at every quit). Those
        // workers run out first.
        if world.session_ids().iter().all(|id| *id == sid || *id == HOST) {
            let limit = std::time::Duration::from_secs(2);
            let settled = self.compiler.settle(limit) && self.queue.as_ref().is_some_and(|q| q.settle(limit)) && self.complete.settle(limit);
            if !settled { eprintln!("[hal-vulkan] the last session closed with GPU work still running after {limit:?}; exiting may race the driver"); }
        }
        #[cfg(target_os = "linux")]
        if let Some(shell) = self.shell { shell.session_closed(sid); }
    }
    fn window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> { self.host_window_pixels(world, sid, window) }
}

impl Drop for Vulkan {
    fn drop(&mut self) {
        self.flush();
        self.windows.clear(self.queue.as_ref().unwrap());
        // Queue jobs retain the completion worker; both drain after the HAL lets them go.
        self.queue.take();
    }
}

impl Gpu for Vulkan {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn media(&mut self, s: &mut Session, c: &crate::cmds::HalCmd) -> bool {
        let mut media = std::mem::take(&mut self.media);
        let handled = media.command(self, s, c);
        self.media = media;
        handled
    }
    /// The host's run-state overlay is drawn by the macOS backend only so far (the bootloader
    /// host runs on macOS); here it is counted as unsupported, never silently dropped.
    fn window_overlay(&mut self, s: &mut Session, _id: Id, rect: Option<[f32; 4]>, _now: u64) {
        if rect.is_some() {
            s.stats.overlay_unsupported += 1;
        }
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
        self.pressure.starve(sid, id, bytes, crate::clock_ns());
        true
    }
    fn texture_create(&mut self, s: &mut Session, id: Id, t: &mut TextureObj) -> Result<(), String> {
        let image = if t.usage & usage::SHARED != 0 { self.sharing.create(self.dev.clone(), t.clone()) } else { Image::new(self.dev.clone(), t.clone()) }
            .map_err(|e| { eprintln!("[hal-vulkan] texture create: {e}"); e })?;
        t.bytes = image.bytes;
        if let Some(old) = self.tex.insert((s.id, id.index), Texture { gen: id.gen, image }) {
            if let Some(registry) = self.sharing.registry() { registry.retire(s.id, &old.image); }
        }
        Ok(())
    }
    fn texture_update(&mut self, s: &mut Session, id: Id, t: &TextureObj, mip: u16, rect: [u32; 4], layer: u32, data: &[u8]) {
        if let Err(e) = self.upload(s, id, t, mip, rect, layer, data) { eprintln!("[hal-vulkan] texture upload: {e}"); s.bad(); }
    }
    fn texture_destroy(&mut self, sid: u32, id: Id) {
        if self.tex.get(&(sid, id.index)).is_some_and(|t| t.gen <= id.gen) {
            if let Some(texture) = self.tex.remove(&(sid, id.index)) {
                if let Some(registry) = self.sharing.registry() { registry.retire(sid, &texture.image); }
            }
        }
        self.pick_pending.remove(&(sid, id));
        if self.pressure.is_starved(sid, id) { self.pressure.forget(sid, id); }
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
        let state = self.programs.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
            let state = State::new(); let job = state.clone(); let dev = self.dev.clone(); let desc = blob.desc;
            let bindings = blob.bindings.clone(); let stride = blob.vertex_layout.first().copied().unwrap_or(0); let wake = self.mailbox.wake.clone();
            self.compiler.push(move || {
                let program = pipeline::Program::new(dev, desc, code, bindings, stride);
                if let Ok(program) = &program { job.predict(program); }
                let _ = job.program.set(program); job.epoch.fetch_add(1, Ordering::Release); wake(); });
            self.programs.insert(key, Arc::downgrade(&state)); state
        });
        self.pipe.insert((s.id, id.index), Pipe { gen: id.gen, state, epoch: 0, announced: false }); PipeStatus::Pending
    }
    fn pipeline_destroy(&mut self, sid: u32, id: Id) -> bool {
        if self.pipe.get(&(sid, id.index)).is_some_and(|p| p.gen <= id.gen) { return self.pipe.remove(&(sid, id.index)).is_some_and(|p| !p.announced); } false
    }
    fn frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<exec::SubmittedFrame> { self.encode_frame(s, sid, target, kind, packet, view, now) }
    fn pick(&mut self, s: &mut Session, window: Id, req: u32, pos: [f32; 2]) { self.read_pick(s, window, req, pos); }
    fn snapshot(&mut self, s: &mut Session, window: Id, req: u32) {
        let image = self.window_image(s, window); let rect = image.as_ref().map_or([0; 4], |i| [0, 0, i.desc.w, i.desc.h]); self.read(s, image, req, rect, None);
    }
    fn window_copy(&mut self, s: &mut Session, window: Id, texture: Id, req: u32) { self.copy_window(s, window, texture, req); }
    fn window_closed(&mut self, sid: u32, id: Id) {
        #[cfg(target_os = "linux")]
        self.capture_pool.remove(&(sid, id));
        self.win.remove(&(sid, id.index)); self.pick_ids.remove(&(sid, id));
        self.scanout_images.remove(&(sid, id));
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

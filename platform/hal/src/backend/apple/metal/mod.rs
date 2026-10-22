//! The Metal backend of the HAL: `Backend` for macOS over the HAL tables. Resources come from
//! commands (`TextureCreate`, `BufferCreate`, `PipelineCreate` ... keyed by the ids Cx chose),
//! frames come as packets (`Frame`) and are encoded here and nowhere else, results go out as
//! events (`FrameDone` with GPU time, `Readback`, `PickResult`, `PipelineReady`).
//!
//! A window is a `CAMetalLayer` of the AppKit shell (`super::appkit`): a window frame renders into
//! the layer's next drawable and presents it. With `open_flags::OFFSCREEN_WINDOWS` (a permanent
//! mode for GPU-real headless runs) a window is an offscreen render target of the window's size
//! and a finished frame is a presented frame. With `open_flags::RETAIN_WINDOWS` a real window
//! also keeps a copy of its newest frame, for snapshots.
//!
//! Moved from os/apple/metal.rs (see each module); the tables, ledger and command validation
//! are `backend::exec`'s.

mod device;
mod frame;
mod media;
mod pipeline;
mod readback;
mod resources;
mod timing;

use super::super::exec::{self, Gpu, OsCmd, PipeStatus};
use super::shell::Shared;
use super::media::Media;
use super::super::{Backend, Pixels};
use crate::buffers::{CmdView, PipelineBlob};
use crate::ledger::{Kind, LedgerEntry};
use crate::session::*;
use crate::types::*;
use device::{Device, Done, Mailbox};
pub use pipeline::compile_msl;
use makepad_apple_sys::*;
use resources::*;
use crate::FastMap;
use std::sync::Arc;

/// How long a transient render target may sit unbound before its GPU storage goes.
const TRANSIENT_IDLE_NS: u64 = 3_000_000_000;
/// The capture buffers are given back this long after the last frame that asked for one.
const CAPTURE_IDLE_NS: u64 = 2_000_000_000;
/// The device is asked how much it holds, and transient targets are looked at, this often.
const HOUSEKEEPING_NS: u64 = 250_000_000;
/// Retained instance data the HAL may drop is dropped, least recently used first, once the
/// device holds more than this fraction of its recommended working set, down to the low mark.
const RESIDENCY_HIGH: (u64, u64) = (3, 4);
const RESIDENCY_LOW: (u64, u64) = (5, 8);

/// The autorelease pool of a call into the backend: the thread that drives it has no run loop.
struct Pool(ObjcId);

impl Pool {
    fn new() -> Pool {
        Pool(unsafe { msg_send![class!(NSAutoreleasePool), new] })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.0, release];
        }
    }
}

/// A window realised as an offscreen target (the window itself, or the retained copy of a real
/// window's newest frame).
struct Win {
    gen: u32,
    tex: Tex,
    w: u32,
    h: u32,
}

pub struct Metal {
    dev: Device,
    info: HalInfo,
    tex: FastMap<(u32, u32), Tex>,
    buf: FastMap<(u32, u32), Buf>,
    pipe: FastMap<(u32, u32), pipeline::Pipe>,
    win: FastMap<(u32, u32), Win>,
    /// App aliases resolve to host storage; lending never moves or frees that storage.
    window_aliases: FastMap<(u32, Id), Id>,
    overlays: FastMap<Id, frame::Overlay>,
    /// Dismissal retries when AppKit temporarily has no drawable.
    restores: FastMap<Id, Win>,
    /// per window: the id texture the newest pick frame drew into (`Pick` reads it back)
    pick_ids: FastMap<(u32, u32), Id>,
    /// Pick targets whose latest replay skipped a not-yet-ready draw.
    pick_pending: std::collections::HashSet<(u32, Id)>,
    batch: Option<frame::Batch>,
    uploads: Vec<Upload>,
    housekept_ns: u64,
    /// the window system, `None` in offscreen mode
    shell: Option<&'static Shared>,
    /// real windows keep a copy of their newest frame
    retain: bool,
    /// the drawable the window frame being encoded renders into
    frame_drawable: Option<frame::FrameDrawable>,
    /// the size the ledger last saw of each real window's drawables
    layer_sizes: FastMap<(u32, u32), (u32, u32, u32)>,
    /// video playback, audio, camera ...: everything of os/apple that is not Metal or AppKit
    media: Media,
    /// the pool of capture buffers a recorder's frames are copied into, and when it was last used
    capture: Option<super::media::capture::Pool>,
    capture_ns: u64,
    /// `frame::depth_only`'s scratch: the (pipeline, blend) pairs of a pass already asked
    depth_only_seen: Vec<(u32, bool)>,
    /// the GPU profiler's encoders of the frame being encoded (moved into its ticket)
    prof: Vec<frame::ProfPass>,
    /// the profiled frame being encoded
    prof_key: Option<crate::gpu_prof::FrameKey>,
    /// Draws left out so far for a pipeline variant still compiling (a frame that adds to it is
    /// partial, `FramePartial`).
    pending_draws: u64,
}

impl Metal {
    /// `None` when there is no Metal device, or when real windows are wanted and the host never
    /// ran the main loop. `sessions` makes a GPU result's session look, `notify` every pump (the
    /// shell's events).
    pub fn new(flags: u32, sessions: crate::backend::SessionNotify, notify: Arc<dyn Fn() + Send + Sync>) -> Option<Metal> {
        let dev = Device::new(Mailbox::new(sessions))?;
        let shell = if flags & open_flags::OFFSCREEN_WINDOWS == 0 {
            let shell = Shared::global();
            if !shell.start(dev.device, flags, notify) {
                eprintln!("[metal] the window system did not start: the host must run makepad_hal::run_main_loop on the main thread");
                return None;
            }
            Some(shell)
        } else {
            None
        };
        let texture_compression = unsafe {
            let bc: bool = if msg_send![dev.device, respondsToSelector: sel!(supportsBCTextureCompression)] { msg_send![dev.device, supportsBCTextureCompression] } else { false };
            // MTLGPUFamilyApple2 (1002): every Apple GPU samples ASTC LDR
            let astc: bool = if msg_send![dev.device, respondsToSelector: sel!(supportsFamily:)] { msg_send![dev.device, supportsFamily: 1002i64] } else { false };
            (bc, astc)
        };
        let mut formats = (1u64 << format::COUNT) - 1;
        if !texture_compression.0 {
            formats &= !(1 << format::BC7 | 1 << format::BC5);
        }
        if !texture_compression.1 {
            formats &= !(1 << format::ASTC4X4);
        }
        let info = HalInfo {
            session: 0,
            backend: backend_id::METAL,
            dialect: dialect::MSL,
            flags: info_flags::WINDOW_LAYERS
                | if shell.is_some() { info_flags::DISPLAY_CLOCK } else { 0 }
                | if shell.is_some() && cfg!(target_os = "macos") { info_flags::WINDOW_BADGES } else { 0 },
            formats,
            msaa: 1,
            dpi: shell.map_or(1.0, |s| s.dpi()),
            max_texture: 16384,
            max_window: [16384, 16384],
            max_buffer: crate::gpu_limits::max_gpu_buffer_bytes(),
            working_set: crate::gpu_limits::gpu_working_set_bytes(),
        };
        Some(Metal {
            dev,
            info,
            tex: FastMap::default(),
            buf: FastMap::default(),
            pipe: FastMap::default(),
            win: FastMap::default(),
            window_aliases: FastMap::default(),
            overlays: FastMap::default(),
            restores: FastMap::default(),
            pick_ids: FastMap::default(),
            pick_pending: Default::default(),
            batch: None,
            uploads: Vec::new(),
            housekept_ns: 0,
            shell,
            retain: flags & open_flags::RETAIN_WINDOWS != 0,
            frame_drawable: None,
            layer_sizes: FastMap::default(),
            media: Media::new(),
            capture: None,
            capture_ns: 0,
            depth_only_seen: Vec::new(),
            prof: Vec::new(),
            prof_key: None,
            pending_draws: 0,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.dev.name
    }

    fn window_key(&self, sid: u32, window: Id) -> (u32, Id) {
        self.window_aliases.get(&(sid, window)).map_or((sid, window), |host| (HOST, *host))
    }

    fn retained_window(&self, owner: u32, id: Id) -> Option<&Win> {
        self.win.get(&(owner, id.index)).filter(|w| w.gen == id.gen).or_else(|| {
            if owner != HOST { return None; }
            self.overlays.get(&id).and_then(|o| o.base.as_ref()).or_else(|| self.restores.get(&id))
        })
    }

    fn label(s: &Session, attr: Attr) -> String {
        s.string(attr.label).or_else(|| s.string(attr.owner)).or_else(|| s.string(attr.site)).unwrap_or("hal").to_string()
    }

    fn attr_of(s: &Session, kind: Kind, id: Id) -> Attr {
        s.ledger.get(kind, id.index).filter(|e| e.gen == id.gen).map(|e| e.attr).unwrap_or_default()
    }

    /// The first outcome of a pipeline's compile ends its pending count.
    fn announce(pipes: &mut FastMap<(u32, u32), pipeline::Pipe>, s: &mut Session, id: Id) {
        if let Some(p) = pipes.get_mut(&(s.id, id.index)).filter(|p| p.gen == id.gen) {
            if !std::mem::replace(&mut p.announced, true) {
                s.pipelines_pending = s.pipelines_pending.saturating_sub(1);
            }
        }
    }

    /// Applies what finished on the GPU to the sessions: events, counters.
    fn drain(&mut self, world: &mut World) {
        for done in self.dev.mailbox.take() {
            let (sid, incarnation) = match &done {
                Done::Frame { sid, incarnation, .. } | Done::Readback { sid, incarnation, .. }
                | Done::Pick { sid, incarnation, .. } | Done::PipelineReady { sid, incarnation, .. }
                | Done::PipelineFailed { sid, incarnation, .. } | Done::Surface { sid, incarnation, .. } => (*sid, incarnation),
            };
            // Every completion belongs to a session incarnation, including driver callbacks
            // which can finish long after the session id has been reused by a relaunch.
            if !world.session(sid).is_some_and(|s| Arc::ptr_eq(&s.font_frames, incarnation)) {
                if let Done::Surface { buffer, .. } = done { unsafe { CFRelease(buffer as *const std::ffi::c_void) }; }
                continue;
            }
            match done {
                Done::Frame { sid, target, frame, gpu_ns, present_ns, gpu_span, failed, partial, window, kind, .. } => {
                    if window && !failed && kind & target::OVERLAY == 0 {
                        world.window_presented(sid, target, frame, gpu_ns, present_ns);
                        if let Some(shell) = self.shell {
                            shell.presented();
                        }
                    }
                    if let Some(s) = world.session_mut(sid) {
                        if failed {
                            s.stats.bad_args += 1;
                        }
                        s.stats.frames += 1;
                        if gpu_ns > 0 {
                            s.stats.gpu_frames += 1;
                            s.stats.gpu_ns_total += gpu_ns;
                            s.stats.gpu_ns_max = s.stats.gpu_ns_max.max(gpu_ns);
                        }
                    }
                    // the one place FrameDone is posted: the presenter, and the parent of a lent window
                    if !failed {
                        if partial { world.frame_partial(sid, target, frame); }
                        world.frame_done(sid, target, kind, frame, gpu_ns, present_ns, gpu_span);
                    }
                }
                Done::Surface { sid, window, w, h, buffer, .. } => match world.session_mut(sid) {
                    Some(s) => s.events.screen_surface(window, [w as u16, h as u16], buffer as u64),
                    // the session is gone: nobody takes the buffer
                    None => unsafe { CFRelease(buffer as *const std::ffi::c_void) },
                },
                Done::Readback { sid, req, status, rect, pixels, .. } => {
                    if let Some(s) = world.session_mut(sid) {
                        s.events.readback(req, status, rect, &pixels);
                    }
                }
                Done::Pick { sid, req, ids, .. } => {
                    if let Some(s) = world.session_mut(sid) {
                        let bytes: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
                        s.events.pick_result(req, &bytes);
                    }
                }
                Done::PipelineReady { sid, id, .. } => {
                    if let Some(s) = world.session_mut(sid) {
                        if s.pipelines.get(sid, id).is_ok() {
                            Self::announce(&mut self.pipe, s, id);
                            s.events.pipeline_ready(id);
                        }
                    }
                }
                Done::PipelineFailed { sid, id, log, .. } => {
                    if let Some(s) = world.session_mut(sid) {
                        if s.pipelines.get(sid, id).is_ok() {
                            Self::announce(&mut self.pipe, s, id);
                            s.events.pipeline_failed(id, log.as_bytes());
                        }
                    }
                }
            }
        }
    }

    /// The device-wide upkeep: what the device holds against the ledger, transient targets given
    /// back, retained instance data dropped under pressure.
    fn housekeeping(&mut self, world: &mut World) {
        let now = world.now();
        if now.saturating_sub(self.housekept_ns) < HOUSEKEEPING_NS {
            return;
        }
        self.housekept_ns = now;
        // recording is over (no frame asked for a capture for a while): the buffers go back
        if self.capture.is_some() && now.saturating_sub(self.capture_ns) > CAPTURE_IDLE_NS {
            self.capture = None;
        }
        let allocated = self.dev.device_allocated();
        let completed = self.dev.completed.load(std::sync::atomic::Ordering::Acquire);
        for sid in world.session_ids() {
            let Some(s) = world.session_mut(sid) else { continue };
            s.ledger.device_allocated = Some(allocated);
            // transient render targets idle for a while give their storage back; the app is told
            // and produces them again when they are next needed
            let idle: Vec<Id> = s
                .textures
                .keys_of(sid)
                .into_iter()
                .filter(|id| {
                    let transient = s.textures.get(sid, *id).is_ok_and(|e| e.val.usage & usage::TRANSIENT != 0);
                    let last = s.ledger.get(Kind::Texture, id.index).map_or(now, |l| l.last_used_ns.max(l.created_ns));
                    transient && now.saturating_sub(last) >= TRANSIENT_IDLE_NS && self.tex.get(&(sid, id.index)).is_some_and(|t| t.gen == id.gen && t.last_bound <= completed)
                })
                .collect();
            for id in idle {
                if let Some(t) = self.tex.remove(&(sid, id.index)) {
                    self.dev.retire(Box::new(t));
                    if let Some(l) = s.ledger.get_mut(Kind::Texture, id.index) {
                        l.bytes = 0;
                    }
                    s.events.texture_evicted(id);
                }
            }
        }
        // pressure: least recently used evictable buffers no pending command buffer reads
        let high = self.dev.recommended_working_set / RESIDENCY_HIGH.1 * RESIDENCY_HIGH.0;
        if allocated > high {
            let low = self.dev.recommended_working_set / RESIDENCY_LOW.1 * RESIDENCY_LOW.0;
            let mut victims: Vec<(u64, u32, Id, u64)> = Vec::new();
            for sid in world.session_ids() {
                let Some(s) = world.session(sid) else { continue };
                for id in s.buffers.keys_of(sid) {
                    let Some(b) = self.buf.get(&(sid, id.index)).filter(|b| b.gen == id.gen && b.evictable && b.last_bound <= completed) else { continue };
                    let last = s.ledger.get(Kind::Buffer, id.index).map_or(0, |l| l.last_used_ns.max(l.created_ns));
                    victims.push((last, sid, id, b.allocated()));
                }
            }
            victims.sort_by_key(|v| v.0);
            let mut held = allocated;
            for (_, sid, id, bytes) in victims {
                if held <= low {
                    break;
                }
                if let Some(b) = self.buf.remove(&(sid, id.index)) {
                    self.dev.retire(Box::new(b));
                }
                if let Some(s) = world.session_mut(sid) {
                    let _ = s.buffers.remove(sid, id);
                    s.ledger.remove(Kind::Buffer, id.index);
                    s.events.buffer_evicted(id);
                }
                held = held.saturating_sub(bytes);
            }
        }
    }
}

// SAFETY: the Hal drives the backend under its state lock, one thread at a time; the Metal
// objects it holds are usable from any thread
unsafe impl Send for Metal {}

impl Backend for Metal {
    fn info(&self) -> HalInfo {
        self.info
    }

    fn submit(&mut self, world: &mut World, sid: u32, view: &CmdView) {
        let _pool = Pool::new();
        exec::submit(self, world, sid, view);
        self.account_host_layers(world);
        self.end_batch();
    }

    fn host_gpu(&mut self) -> Option<&mut dyn Gpu> {
        Some(self)
    }

    fn step(&mut self, world: &mut World, sid: u32) {
        let _pool = Pool::new();
        if let Some(shell) = self.shell {
            shell.drain_events(sid, world);
        }
        self.dev.flush_submissions();
        self.drain(world);
        self.housekeeping(world);
        let now = world.now();
        if sid == HOST && !self.restores.is_empty() {
            if let Some(s) = world.session_mut(HOST) { self.restore_overlays(s, now); }
            self.account_host_layers(world);
            self.end_batch();
        }
        if let Some(s) = world.session_mut(sid) {
            self.media.step(&mut media::MetalSink { dev: &self.dev, tex: &mut self.tex, now }, s);
        }
        exec::step_timers(world, sid);
    }

    fn next_due(&self, world: &World, sid: u32) -> Option<u64> {
        // timers, and the next look at transient targets while some hold storage
        let timer = exec::next_timer(world, sid).into_iter().chain(self.media.next_due(sid, world.now())).min();
        let trim = world.session(sid).and_then(|s| {
            s.textures
                .keys_of(sid)
                .into_iter()
                .filter(|id| s.textures.get(sid, *id).is_ok_and(|e| e.val.usage & usage::TRANSIENT != 0) && self.tex.contains_key(&(sid, id.index)))
                .filter_map(|id| s.ledger.get(Kind::Texture, id.index).map(|l| l.last_used_ns.max(l.created_ns) + TRANSIENT_IDLE_NS))
                .min()
        });
        let restore = (sid == HOST && !self.restores.is_empty()).then(|| world.now() + 16_666_667);
        timer.into_iter().chain(trim).chain(restore).min()
    }

    fn session_closed(&mut self, world: &mut World, sid: u32) {
        // commands this session left pending belong to its objects: they go out first
        self.end_batch();
        let lent: Vec<_> = self.window_aliases.iter().filter(|((s, _), _)| *s == sid).map(|((_, id), host)| (*id, *host)).collect();
        let now = world.now();
        for (id, host) in lent {
            if let Some(s) = world.session_mut(HOST) { self.set_overlay(s, host, None, Id::NONE, now); }
            self.window_lent(sid, id, host, false);
        }
        if sid == HOST {
            let ids: Vec<_> = self.overlays.keys().chain(self.restores.keys()).copied().collect();
            for id in ids { self.drop_overlay(id); }
        }
        self.end_batch();
        self.media.session_closed(sid);
        let own = |k: &(u32, u32)| k.0 == sid;
        let tex: Vec<_> = self.tex.keys().filter(|k| own(k)).copied().collect();
        for k in tex {
            if let Some(t) = self.tex.remove(&k) {
                self.dev.retire(Box::new(t));
            }
        }
        let buf: Vec<_> = self.buf.keys().filter(|k| own(k)).copied().collect();
        for k in buf {
            if let Some(b) = self.buf.remove(&k) {
                self.dev.retire(Box::new(b));
            }
        }
        self.pipe.retain(|k, _| !own(k));
        let win: Vec<_> = self.win.keys().filter(|k| own(k)).copied().collect();
        for k in win {
            if let Some(w) = self.win.remove(&k) {
                self.dev.retire(Box::new(w.tex));
            }
        }
        self.pick_ids.retain(|k, _| !own(k));
        self.pick_pending.retain(|(session, _)| *session != sid);
        self.layer_sizes.retain(|k, _| !own(k));
        if let Some(shell) = self.shell {
            shell.session_closed(sid);
        }
        exec::free_session(world, sid);
    }

    fn window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> {
        let id = window.or_else(|| world.session(sid)?.windows.keys_of(sid).into_iter().next())?;
        let (sid, id) = self.window_key(sid, id);
        let win = self.retained_window(sid, id)?;
        self.read_window(win)
    }
}

impl Gpu for Metal {
    fn gpu_budget(&self) -> Option<(u64, u64)> {
        Some((self.info.working_set, self.dev.device_allocated()))
    }
    fn texture_create(&mut self, s: &mut Session, id: Id, t: &mut TextureObj) -> Result<(), String> {
        let label = Self::label(s, Self::attr_of(s, Kind::Texture, id));
        let tex = create_texture(self.dev.device, t, id.gen, &label)?;
        t.bytes = allocated_size(tex.obj.as_id());
        if let Some(old) = self.tex.insert((s.id, id.index), tex) {
            self.dev.retire(Box::new(old));
        }
        Ok(())
    }

    fn texture_update(&mut self, s: &mut Session, id: Id, t: &TextureObj, mip: u16, rect: [u32; 4], layer: u32, data: &[u8]) {
        let Some(tex) = self.tex.get(&(s.id, id.index)).filter(|x| x.gen == id.gen) else { return };
        let texture = tex.obj.clone();
        let [x, y, w, h] = rect;
        let (row, rows) = row_layout(t.format, w, h);
        let (_, seq) = self.ensure_batch();
        // uploads come from host buffers freed right after the call (the font service's atlas
        // rectangles): same address and length is not same content
        let Some(loc) = self.dev.arena_put_transient(data, seq) else {
            s.stats.skipped_draws += 1;
            return;
        };
        self.uploads.push(Upload { texture, staging: loc.buffer, offset: loc.offset, bytes_per_row: row, rows, size: [w as u64, h as u64], mip: mip as u64, layer: layer as u64, origin: [x as u64, y as u64] });
    }

    fn texture_destroy(&mut self, sid: u32, id: Id) {
        self.pick_pending.remove(&(sid, id));
        if self.tex.get(&(sid, id.index)).is_some_and(|t| t.gen <= id.gen) {
            if let Some(t) = self.tex.remove(&(sid, id.index)) {
                self.dev.retire(Box::new(t));
            }
        }
    }

    fn texture_readback(&mut self, s: &mut Session, id: Id, req: u32, rect: [u32; 4]) {
        self.readback_texture(s, id, req, rect);
    }

    fn capture_texture(&mut self, s: &mut Session, id: Id, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        self.capture_texture_storage(s, id, done);
    }

    fn capture_buffer(&mut self, sid: u32, id: Id) -> Option<Vec<u8>> {
        let b = self.buf.get(&(sid, id.index)).filter(|b| b.gen == id.gen)?;
        // SAFETY: a shared buffer of at least `size` bytes; writes happen on this thread only
        Some(unsafe { std::slice::from_raw_parts(b.contents, b.size as usize) }.to_vec())
    }

    fn texture_share(&mut self, s: &mut Session, id: Id) {
        let handle = self.tex.get(&(s.id, id.index)).filter(|t| t.gen == id.gen).map_or(0, |t| t.surface_id as u64);
        s.events.texture_shared(id, handle);
    }

    fn texture_import(&mut self, s: &mut Session, id: Id, handle: u64, kind: u32) -> Option<TextureObj> {
        if kind != texture_handle::NATIVE {
            return None;
        }
        let (tex, w, h, fmt) = import_surface(self.dev.device, IOSurfaceID::try_from(handle).ok()?, id.gen)?;
        if let Some(old) = self.tex.insert((s.id, id.index), tex) {
            self.dev.retire(Box::new(old));
        }
        Some(TextureObj { format: fmt, w, h, depth: 1, mips: 1, usage: usage::SAMPLED | usage::RENDER_TARGET, bytes: 0, imported: true, uploaded: 0, lent: None })
    }

    fn buffer_create(&mut self, s: &mut Session, id: Id, b: &mut BufferObj) -> Result<(), String> {
        let buf = Buf::new(self.dev.device, id.gen, b.size).ok_or_else(|| format!("a {} byte buffer could not be allocated", b.size))?;
        b.bytes = buf.allocated();
        if let Some(old) = self.buf.insert((s.id, id.index), buf) {
            self.dev.retire(Box::new(old));
        }
        Ok(())
    }

    fn buffer_update(&mut self, s: &mut Session, id: Id, b: &mut BufferObj, off: u64, data: &[u8]) {
        let completed = self.dev.completed.load(std::sync::atomic::Ordering::Acquire);
        let Some(buf) = self.buf.get_mut(&(s.id, id.index)).filter(|x| x.gen == id.gen) else { return };
        if !writable_in_place(buf, completed) {
            let covers_all = off == 0 && data.len() as u64 >= buf.size;
            match copy_on_write(self.dev.device, buf, covers_all) {
                Some(new) => {
                    let old = std::mem::replace(buf, new);
                    self.dev.retire(Box::new(old));
                    b.bytes = buf.allocated();
                }
                None => {
                    s.stats.bad_args += 1;
                    return;
                }
            }
        }
        fill(buf, off, data);
    }

    fn buffer_destroy(&mut self, sid: u32, id: Id) {
        if self.buf.get(&(sid, id.index)).is_some_and(|b| b.gen <= id.gen) {
            if let Some(b) = self.buf.remove(&(sid, id.index)) {
                self.dev.retire(Box::new(b));
            }
        }
    }

    fn buffer_evictable(&mut self, sid: u32, id: Id) {
        if let Some(b) = self.buf.get_mut(&(sid, id.index)).filter(|b| b.gen == id.gen) {
            b.evictable = true;
        }
    }

    fn pipeline_create(&mut self, s: &mut Session, id: Id, dialect: u32, blob: &PipelineBlob, _attr: Attr) -> PipeStatus {
        if dialect != self::dialect::MSL {
            return PipeStatus::Failed(format!("the Metal backend takes MSL, not dialect {dialect}"));
        }
        let Ok(source) = std::str::from_utf8(blob.code) else { return PipeStatus::Failed("the pipeline source is not utf8".into()) };
        let state = pipeline::create(&self.dev, s.id, id, s.font_frames.clone(), source, &blob.desc);
        let stats = crate::gpu_prof::source_stats(source).into();
        let kept = crate::gpu_prof::keep_sources().then(|| source.into());
        self.pipe.insert((s.id, id.index), pipeline::Pipe { gen: id.gen, state, desc: blob.desc, bindings: blob.bindings.as_slice().into(), announced: false, stats, source: kept });
        PipeStatus::Pending
    }

    fn pipeline_destroy(&mut self, sid: u32, id: Id) -> bool {
        if self.pipe.get(&(sid, id.index)).is_some_and(|p| p.gen <= id.gen) {
            return self.pipe.remove(&(sid, id.index)).is_some_and(|p| !p.announced);
        }
        false
    }

    fn frame(&mut self, s: &mut Session, sid: u32, target: Id, kind: u32, packet: u32, view: &CmdView, now: u64) -> Option<exec::SubmittedFrame> {
        self.encode_frame(s, sid, target, kind, packet, view, now)
    }

    fn pick(&mut self, s: &mut Session, window: Id, req: u32, pos: [f32; 2]) {
        self.read_pick(s, window, req, pos);
    }

    fn snapshot(&mut self, s: &mut Session, window: Id, req: u32) {
        self.readback_window(s, window, req);
    }

    fn window_copy(&mut self, s: &mut Session, window: Id, texture: Id, req: u32) {
        self.copy_window(s, window, texture, req);
    }

    fn window_closed(&mut self, sid: u32, id: Id) {
        if sid == HOST { self.drop_overlay(id); }
        if let Some(w) = self.win.remove(&(sid, id.index)) {
            self.dev.retire(Box::new(w.tex));
        }
        self.pick_ids.remove(&(sid, id.index));
        self.layer_sizes.remove(&(sid, id.index));
    }

    fn window_overlay(&mut self, s: &mut Session, id: Id, rect: Option<[f32; 4]>, layer: Id, now: u64) {
        self.set_overlay(s, id, rect, layer, now);
    }

    fn badges(&self) -> bool {
        self.shell.is_some() && cfg!(target_os = "macos")
    }

    fn window_lent(&mut self, sid: u32, id: Id, host: Id, attach: bool) {
        if attach {
            self.window_aliases.insert((sid, id), host);
        } else {
            self.window_aliases.remove(&(sid, id));
            self.pick_ids.remove(&(sid, id.index));
            self.pick_pending.retain(|(s, w)| *s != sid || *w != id);
        }
        if let Some(shell) = self.shell {
            shell.lend(host, if attach { sid } else { HOST }, if attach { id } else { host });
        }
    }

    fn media(&mut self, s: &mut Session, c: &crate::cmds::HalCmd) -> bool {
        let now = crate::clock_ns();
        self.media.command(&mut media::MetalSink { dev: &self.dev, tex: &mut self.tex, now }, s, c)
    }

    fn os(&mut self, sid: u32, cmd: OsCmd) -> bool {
        match self.shell {
            Some(shell) => {
                shell.command(sid, cmd);
                true
            }
            None => false,
        }
    }
}

/// A window's ledger row: what the offscreen target holds.
pub(crate) fn window_row(index: u32, gen: u32, bytes: u64, w: u32, h: u32, now: u64, frame: u64) -> LedgerEntry {
    LedgerEntry {
        kind: Kind::Window,
        detail: "window surface",
        index,
        gen,
        bytes,
        format: format::BGRA8,
        width: w,
        height: h,
        mips: 1,
        attr: Attr::default(),
        created_ns: now,
        created_frame: frame,
        last_used_frame: 0,
        last_used_ns: 0,
    }
}

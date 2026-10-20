//! The D3D11 HAL backend (Windows). Commands, generations, font jobs and lending use the shared
//! executor; the device work is here: resources keyed by (session, index) with generations,
//! HLSL pipelines compiled off the HAL's thread, frame packets encoded on the immediate context,
//! completion and readbacks on the completion thread, swapchains for the Win32 shell's windows.
//! Offscreen windows (`OPEN_FLAGS::OFFSCREEN_WINDOWS`, hidden runs) render into retained images
//! with no window system at all.

mod chain;
mod complete;
pub(crate) mod device;
mod frame;
mod pipeline;
mod readback;
pub(crate) mod resources;
pub mod share_copy;

use super::super::exec::{self, Gpu, PipeStatus};
use super::super::{Backend, Pixels};
use super::win32;
use crate::buffers::{CmdView, PipelineBlob};
use crate::hlsl::HlslPipeline;
use crate::ledger::{Kind, LedgerEntry};
use crate::session::*;
use crate::types::*;
use complete::{Completion, Event, Mailbox, Token};
use device::{Device, Worker};
use pipeline::{PipeSrc, PipeState};
use resources::{Arena, Buffer, Texture};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

pub(crate) struct Tex {
    pub(crate) gen: u32,
    pub(crate) tex: Arc<Texture>,
}
struct Buf {
    gen: u32,
    buf: Arc<Buffer>,
    evictable: bool,
}
struct Pipe {
    gen: u32,
    state: Arc<PipeState>,
    epoch: u64,
    announced: bool,
}

pub struct D3d11 {
    shell: Option<&'static win32::Shared>,
    dev: Arc<Device>,
    info: HalInfo,
    tex: HashMap<(u32, u32), Tex>,
    buf: HashMap<(u32, u32), Buf>,
    pipe: HashMap<(u32, u32), Pipe>,
    /// compiled pipelines shared by identical sources and layouts (by `PipeSrc::hash_key`)
    programs: HashMap<u64, Vec<Weak<PipeState>>>,
    /// windows' retained images, by (owner, index)
    win: HashMap<(u32, u32), Tex>,
    aliases: HashMap<(u32, Id), Id>,
    /// the id target of each window's newest pick frame, and the targets missing a draw
    pick_ids: HashMap<(u32, Id), Id>,
    pick_pending: std::collections::HashSet<(u32, Id)>,
    /// window capture textures (recording), reused once no sink holds them
    captures: Vec<([u32; 2], makepad_windows_sys::Win32::Graphics::Direct3D11::ID3D11Texture2D)>,
    /// cross-process images this pass holds (keyed mutex), given back when it is encoded
    acquired: Vec<Arc<Texture>>,
    fallback: HashMap<u32, Arc<Texture>>,
    chains: chain::Chains,
    batch: complete::Batch,
    cb_arena: Arena,
    word_arena: Arena,
    /// one pass's draw lists, reused pass after pass
    scratch: frame::PassScratch,
    mailbox: Mailbox,
    complete: Completion,
    /// HLSL compiles in parallel (FXC is single-threaded per call; a cold start compiles dozens)
    compilers: Vec<Worker>,
    next_compiler: usize,
    /// video, audio, MIDI, camera, game input (`backend/windows/media`)
    media: super::media::Media,
    /// the sessions heard `DeviceLost` and the replacement device is not made yet
    lost_posted: bool,
    /// when the next attempt to make a replacement device may run (HAL clock ns)
    retry_ns: u64,
    /// `simulate_device_loss` asked for a loss: the next step treats the device as removed
    simulate: bool,
    /// `MAKEPAD_HAL_SIMULATE_DEVICE_LOSS_MS`: a loss is simulated this long after the device was
    /// made (app-level checks of the reset path, e.g. a grab before and after)
    simulate_at_ns: Option<u64>,
}

impl D3d11 {
    pub fn new(flags: u32, wake: Arc<dyn Fn() + Send + Sync>, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let dev = Device::new()?;
        let shell = if flags & open_flags::OFFSCREEN_WINDOWS == 0 {
            let shell = win32::Shared::global();
            shell.start(flags, notify)?;
            Some(shell)
        } else {
            None
        };
        let budget = dev.budget().map_or(0, |b| b.0);
        let mut info = HalInfo {
            backend: backend_id::D3D11,
            dialect: dialect::HLSL,
            formats: dev.formats,
            msaa: 1,
            dpi: shell.map_or_else(win32::system_dpi, |s| s.dpi()),
            flags: info_flags::GPU_TIMESTAMPS,
            max_texture: 16384,
            max_window: [16384, 16384],
            max_buffer: (u32::MAX as u64) & !15,
            working_set: budget,
            ..Default::default()
        };
        if shell.is_some() {
            // apps draw their own captions on Windows
            info.flags |= info_flags::DISPLAY_CLOCK | info_flags::CLIENT_CHROME;
        }
        crate::gpu_limits::set_max_gpu_buffer_bytes(info.max_buffer);
        crate::gpu_limits::set_gpu_working_set_bytes(info.working_set);
        let mailbox = Mailbox::new(wake);
        let complete = Completion::new(dev.clone(), mailbox.clone())?;
        if win32::log_enabled() {
            eprintln!("[hal-d3d11] device on \"{}\": formats {:#x} renderable {:#x} offscreen={}", dev.adapter_name, dev.formats, dev.renderable, shell.is_none());
        }
        Ok(Self {
            shell,
            cb_arena: Arena::new(&dev, 256 * 1024, true)?,
            word_arena: Arena::new(&dev, 256 * 1024, false)?,
            scratch: Default::default(),
            dev,
            info,
            tex: HashMap::new(),
            buf: HashMap::new(),
            pipe: HashMap::new(),
            programs: HashMap::new(),
            win: HashMap::new(),
            aliases: HashMap::new(),
            pick_ids: HashMap::new(),
            pick_pending: Default::default(),
            acquired: Vec::new(),
            captures: Vec::new(),
            fallback: HashMap::new(),
            chains: chain::Chains::default(),
            batch: complete::Batch::default(),
            mailbox,
            complete,
            compilers: {
                let n = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 8));
                (0..n).map(|i| Worker::new(&format!("makepad-d3d11-compile-{i}"))).collect::<Result<Vec<_>, _>>()?
            },
            next_compiler: 0,
            media: super::media::Media::new(),
            lost_posted: false,
            retry_ns: 0,
            simulate: false,
            simulate_at_ns: super::super::simulated_loss_at(),
        })
    }

    /// Queues the compile of `state`'s program on a worker (round robin); it wakes the HAL when done.
    fn compile(&mut self, state: &Arc<PipeState>) {
        let job = state.clone();
        let dev = self.dev.clone();
        let wake = self.mailbox.wake.clone();
        self.next_compiler = (self.next_compiler + 1) % self.compilers.len();
        self.compilers[self.next_compiler].push(move || {
            let t0 = std::time::Instant::now();
            let src = &job.src;
            if win32::log_enabled() {
                eprintln!("[hal-d3d11] compiling `{}` ({} bytes of HLSL)", src.name, src.code.source.len());
            }
            let program = pipeline::build(&dev, &src.code, src.desc, src.bindings.clone(), src.layout.clone(), &src.name);
            if win32::log_enabled() || t0.elapsed() > std::time::Duration::from_secs(2) {
                eprintln!("[hal-d3d11] compiled `{}` in {:.0} ms", src.name, t0.elapsed().as_secs_f64() * 1e3);
            }
            let _ = job.program.set(program);
            job.epoch.fetch_add(1, Ordering::Release);
            wake();
        });
    }

    fn token(s: &Session) -> Token {
        Token { sid: s.id, incarnation: s.font_frames.clone() }
    }

    fn window_key(&self, sid: u32, id: Id) -> (u32, Id) {
        self.aliases.get(&(sid, id)).map_or((sid, id), |host| (HOST, *host))
    }

    /// Presents a window's image on its swapchain; an offscreen window's frame is done as rendered.
    fn present(&mut self, key: (u32, Id), image: &Arc<Texture>) -> bool {
        match self.shell {
            Some(shell) => self.chains.present(&self.dev, shell, key, image),
            None => true,
        }
    }

    /// A lost device (removed, reset, or a simulated loss) is replaced here: the sessions hear
    /// `DeviceLost` once, then `exec::device_reset` runs until a device can be made.
    fn recover(&mut self, world: &mut World) {
        if self.simulate_at_ns.is_some_and(|t| crate::clock_ns() >= t) {
            self.simulate_at_ns = None;
            self.simulate = true;
        }
        if self.simulate {
            self.simulate = false;
            self.dev.lost.store(true, Ordering::Release);
        }
        if !self.dev.check_lost() {
            return;
        }
        if !self.lost_posted {
            let generation = exec::device_lost(world);
            eprintln!("[hal-d3d11] device generation {generation} lost; making a new one");
            self.lost_posted = true;
            self.retry_ns = 0;
        }
        let now = crate::clock_ns();
        if now < self.retry_ns {
            return;
        }
        if exec::device_reset(self, world) {
            self.lost_posted = false;
            eprintln!("[hal-d3d11] device generation {} on \"{}\"", world.device_generation, self.dev.adapter_name);
        } else {
            self.retry_ns = now + 500_000_000;
        }
    }

    fn drain(&mut self, world: &mut World) {
        for done in self.mailbox.take() {
            let sid = done.token.sid;
            if !world.session(sid).is_some_and(|s| Arc::ptr_eq(&s.font_frames, &done.token.incarnation)) {
                continue;
            }
            match done.event {
                Event::Frame { target, kind, frame, window, gpu_ns } => {
                    if let Some(s) = world.session_mut(sid) {
                        s.stats.frames += 1;
                        if gpu_ns > 0 {
                            s.stats.gpu_frames += 1;
                            s.stats.gpu_ns_total += gpu_ns;
                            s.stats.gpu_ns_max = s.stats.gpu_ns_max.max(gpu_ns);
                        }
                    }
                    if window {
                        world.window_presented(sid, target, frame, gpu_ns, done.time);
                    }
                    world.frame_done(sid, target, kind, frame, gpu_ns, done.time, [0.0; 2]);
                }
                Event::Readback { req, status, rect, pixels } => world.to_session(sid, |q| q.readback(req, status, rect, &pixels)),
                Event::Pick { req, ids } => world.to_session(sid, |q| q.pick_result(req, &ids.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>())),
            }
        }
        for ((sid, index), pipe) in &mut self.pipe {
            let epoch = pipe.state.epoch.load(Ordering::Acquire);
            if epoch == pipe.epoch {
                continue;
            }
            pipe.epoch = epoch;
            let id = Id::new(*index, pipe.gen);
            let Some(s) = world.session_mut(*sid) else { continue };
            if !pipe.announced {
                pipe.announced = true;
                s.pipelines_pending = s.pipelines_pending.saturating_sub(1);
            }
            if let Some(e) = pipe.state.failure() {
                eprintln!("[hal-d3d11] pipeline {id:?} failed: {}", e.lines().next().unwrap_or(""));
                let log = e.to_owned();
                world.to_session(*sid, |q| q.pipeline_failed(id, log.as_bytes()));
            } else if pipe.state.ready().is_some() {
                world.to_session(*sid, |q| q.pipeline_ready(id));
            }
        }
        let allocated = self.dev.allocated.load(Ordering::Relaxed);
        if let Some(host) = world.session_mut(HOST) {
            for ((sid, index), t) in &self.win {
                if *sid == HOST && host.ledger.get(Kind::Window, *index).is_none_or(|r| r.gen != t.gen || r.bytes != t.tex.bytes
                    || r.width != t.tex.desc.w || r.height != t.tex.desc.h) {
                    host.ledger.add(window_row(Id::new(*index, t.gen), &t.tex, crate::clock_ns(), host.frame));
                }
            }
        }
        for sid in world.session_ids() {
            if let Some(s) = world.session_mut(sid) {
                s.ledger.device_allocated = Some(allocated);
            }
        }
    }
}

impl Backend for D3d11 {
    fn info(&self) -> HalInfo {
        self.info
    }
    fn submit(&mut self, world: &mut World, sid: u32, view: &CmdView) {
        self.recover(world);
        self.drain(world);
        exec::submit(self, world, sid, view);
    }
    fn host_gpu(&mut self) -> Option<&mut dyn Gpu> {
        Some(self)
    }
    fn step(&mut self, world: &mut World, sid: u32) {
        self.recover(world);
        self.drain(world);
        if let Some(shell) = self.shell {
            for (owner, id) in shell.drain_events(sid, world) {
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
        if let Some(s) = world.session_mut(sid) {
            self.media.step(&mut super::media::Sink { dev: &self.dev, tex: &mut self.tex, now: crate::clock_ns() }, s);
        }
        exec::step_timers(world, sid);
    }
    fn next_due(&self, world: &World, sid: u32) -> Option<u64> {
        exec::next_timer(world, sid)
    }
    fn session_closed(&mut self, world: &mut World, sid: u32) {
        if let Some(shell) = self.shell {
            self.chains.close_session(shell, sid);
        }
        self.media.session_closed(sid);
        self.tex.retain(|k, _| k.0 != sid);
        self.buf.retain(|k, _| k.0 != sid);
        self.pipe.retain(|k, _| k.0 != sid);
        self.win.retain(|k, _| k.0 != sid);
        self.aliases.retain(|k, _| k.0 != sid);
        self.pick_ids.retain(|k, _| k.0 != sid);
        self.pick_pending.retain(|k| k.0 != sid);
        self.programs.retain(|_, list| {
            list.retain(|w| w.strong_count() > 0);
            !list.is_empty()
        });
        exec::free_session(world, sid);
        if let Some(shell) = self.shell {
            shell.session_closed(sid);
        }
    }
    fn window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> {
        self.host_window_pixels(world, sid, window)
    }
    fn simulate_device_loss(&mut self) {
        self.simulate = true;
    }
}

impl exec::DeviceReset for D3d11 {
    fn replace_device(&mut self, world: &mut World) -> Result<(), String> {
        let dev = Device::new()?;
        let cb_arena = Arena::new(&dev, 256 * 1024, true)?;
        let word_arena = Arena::new(&dev, 256 * 1024, false)?;
        let complete = Completion::new(dev.clone(), self.mailbox.clone())?;
        // the old device's work: its completion thread answers what was submitted (readbacks and
        // picks as failed, frames as finished) and ends; then those answers are delivered
        self.dev.lost.store(true, Ordering::Release);
        let batch = std::mem::take(&mut self.batch);
        self.complete.submit(batch);
        drop(std::mem::replace(&mut self.complete, complete));
        self.drain(world);
        if let Some(shell) = self.shell {
            self.chains.clear(shell);
        }
        self.tex.clear();
        self.buf.clear();
        self.win.clear();
        self.pick_ids.clear();
        self.pick_pending.clear();
        self.captures.clear();
        self.acquired.clear();
        self.fallback.clear();
        self.media.device_reset(world);
        self.cb_arena = cb_arena;
        self.word_arena = word_arena;
        self.dev = dev;
        self.info.working_set = self.dev.budget().map_or(0, |b| b.0);
        crate::gpu_limits::set_gpu_working_set_bytes(self.info.working_set);
        Ok(())
    }

    fn rebuild_pipelines(&mut self, world: &mut World) {
        self.programs.clear();
        let mut rebuilt: HashMap<*const PipeState, Arc<PipeState>> = HashMap::new();
        let keys: Vec<(u32, u32)> = self.pipe.keys().copied().collect();
        for key in keys {
            let old = self.pipe[&key].state.clone();
            let state = match rebuilt.get(&Arc::as_ptr(&old)) {
                Some(st) => st.clone(),
                None => {
                    let st = PipeState::new(old.src.clone());
                    self.compile(&st);
                    self.programs.entry(st.src.hash_key()).or_default().push(Arc::downgrade(&st));
                    rebuilt.insert(Arc::as_ptr(&old), st.clone());
                    st
                }
            };
            let pipe = self.pipe.get_mut(&key).expect("listed above");
            pipe.state = state;
            pipe.epoch = 0;
            // pending again until the new program answers
            if std::mem::replace(&mut pipe.announced, false) {
                if let Some(s) = world.session_mut(key.0) {
                    s.pipelines_pending += 1;
                }
            }
        }
    }
}

impl Gpu for D3d11 {
    fn media(&mut self, s: &mut Session, c: &crate::cmds::HalCmd) -> bool {
        self.media.command(&mut super::media::Sink { dev: &self.dev, tex: &mut self.tex, now: crate::clock_ns() }, s, c)
    }
    fn gpu_budget(&self) -> Option<(u64, u64)> {
        self.dev.budget()
    }

    fn window_overlay(&mut self, s: &mut Session, _id: Id, rect: Option<[f32; 4]>, _layer: Id, _now: u64) {
        if rect.is_some() {
            s.stats.overlay_unsupported += 1;
        }
    }

    fn os(&mut self, sid: u32, command: exec::OsCmd) -> bool {
        match self.shell {
            Some(shell) => shell.command(sid, command),
            None => false,
        }
    }

    fn texture_create(&mut self, s: &mut Session, id: Id, t: &mut TextureObj) -> Result<(), String> {
        let tex = Texture::new(self.dev.clone(), t.clone()).map_err(|e| {
            eprintln!("[hal-d3d11] texture create: {e}");
            e
        })?;
        t.bytes = tex.bytes;
        self.tex.insert((s.id, id.index), Tex { gen: id.gen, tex });
        Ok(())
    }
    fn texture_update(&mut self, s: &mut Session, id: Id, _t: &TextureObj, mip: u16, rect: [u32; 4], layer: u32, data: &[u8]) {
        if let Err(e) = self.upload(s, id, mip, rect, layer, data) {
            eprintln!("[hal-d3d11] texture upload: {e}");
            s.bad();
        }
    }
    fn texture_destroy(&mut self, sid: u32, id: Id) {
        if self.tex.get(&(sid, id.index)).is_some_and(|t| t.gen <= id.gen) {
            self.tex.remove(&(sid, id.index));
        }
        self.pick_pending.remove(&(sid, id));
    }
    fn texture_readback(&mut self, s: &mut Session, id: Id, req: u32, rect: [u32; 4]) {
        if self.pick_pending.contains(&(s.id, id)) {
            s.events.readback(req, crate::types::readback::INCOMPLETE, rect, &[]);
            return;
        }
        let t = self.resolve(s, id);
        self.read(s, t, req, rect, None);
    }
    fn texture_share(&mut self, s: &mut Session, id: Id) {
        let key = self.tex.get(&(s.id, id.index)).filter(|t| t.gen == id.gen).and_then(|t| t.tex.shared.as_ref()).map_or(0, |sh| sh.key);
        s.events.texture_shared(id, key);
    }
    fn texture_import(&mut self, s: &mut Session, id: Id, handle: u64, kind: u32) -> Option<TextureObj> {
        if kind != texture_handle::NATIVE || handle == 0 {
            return None;
        }
        let tex = Texture::import(self.dev.clone(), handle).map_err(|e| eprintln!("[hal-d3d11] import: {e}")).ok()?;
        let desc = tex.desc.clone();
        self.tex.insert((s.id, id.index), Tex { gen: id.gen, tex });
        Some(desc)
    }
    fn buffer_create(&mut self, s: &mut Session, id: Id, b: &mut BufferObj) -> Result<(), String> {
        let buf = Buffer::new(self.dev.clone(), b.kind, b.size)?;
        b.bytes = buf.bytes;
        self.buf.insert((s.id, id.index), Buf { gen: id.gen, buf, evictable: b.evictable });
        Ok(())
    }
    fn buffer_update(&mut self, s: &mut Session, id: Id, b: &mut BufferObj, off: u64, data: &[u8]) {
        match self.buf.get(&(s.id, id.index)).filter(|r| r.gen == id.gen) {
            Some(r) => {
                r.buf.write(off, data);
                b.bytes = r.buf.bytes;
            }
            None => {
                eprintln!("[hal-d3d11] buffer update: missing buffer {id:?}");
                s.bad();
            }
        }
    }
    fn buffer_destroy(&mut self, sid: u32, id: Id) {
        if self.buf.get(&(sid, id.index)).is_some_and(|b| b.gen <= id.gen) {
            self.buf.remove(&(sid, id.index));
        }
    }
    fn buffer_evictable(&mut self, sid: u32, id: Id) {
        if let Some(b) = self.buf.get_mut(&(sid, id.index)).filter(|b| b.gen == id.gen) {
            b.evictable = true;
        }
    }
    fn pipeline_create(&mut self, s: &mut Session, id: Id, dialect: u32, blob: &PipelineBlob, _: Attr) -> PipeStatus {
        if dialect != self::dialect::HLSL {
            return PipeStatus::Failed("D3D11 takes the HLSL payload".into());
        }
        let code = match HlslPipeline::decode(blob.code) {
            Ok(c) => c,
            Err(e) => return PipeStatus::Failed(e),
        };
        let src = PipeSrc {
            code,
            desc: blob.desc,
            bindings: blob.bindings.clone(),
            layout: blob.vertex_layout.clone(),
            name: s.string(blob.desc.name).unwrap_or("").to_owned(),
        };
        let key = src.hash_key();
        let shared = self.programs.get(&key).and_then(|v| v.iter().filter_map(Weak::upgrade).find(|p| p.src.same_program(&src)));
        let state = shared.unwrap_or_else(|| {
            let state = PipeState::new(Arc::new(src));
            self.compile(&state);
            let list = self.programs.entry(key).or_default();
            list.retain(|w| w.strong_count() > 0);
            list.push(Arc::downgrade(&state));
            state
        });
        self.pipe.insert((s.id, id.index), Pipe { gen: id.gen, state, epoch: 0, announced: false });
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
        let t = self.window_image(s, window);
        let rect = t.as_ref().map_or([0; 4], |t| [0, 0, t.desc.w, t.desc.h]);
        self.read(s, t, req, rect, None);
    }
    fn window_copy(&mut self, s: &mut Session, window: Id, texture: Id, req: u32) {
        self.copy_window(s, window, texture, req);
    }
    fn window_closed(&mut self, sid: u32, id: Id) {
        self.win.remove(&(sid, id.index));
        self.pick_ids.remove(&(sid, id));
        if let Some(shell) = self.shell {
            self.chains.remove(shell, (sid, id));
        }
    }
    fn window_lent(&mut self, sid: u32, id: Id, host: Id, attach: bool) {
        if let Some(shell) = self.shell {
            shell.lend(host, if attach { sid } else { HOST }, if attach { id } else { host });
        }
        if attach {
            self.aliases.insert((sid, id), host);
        } else {
            self.aliases.remove(&(sid, id));
            self.pick_ids.remove(&(sid, id));
        }
    }
}

fn window_row(id: Id, t: &Texture, now: u64, frame: u64) -> LedgerEntry {
    LedgerEntry {
        kind: Kind::Window,
        detail: "window surface",
        index: id.index,
        gen: id.gen,
        bytes: t.bytes,
        format: t.desc.format,
        width: t.desc.w,
        height: t.desc.h,
        mips: 1,
        attr: Attr::default(),
        created_ns: now,
        created_frame: frame,
        last_used_frame: frame,
        last_used_ns: now,
    }
}

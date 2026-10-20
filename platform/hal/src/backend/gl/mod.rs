//! The GLES 3 HAL backend (Linux machines without a usable Vulkan device, or
//! MAKEPAD_HAL_BACKEND=gl). Same executor contract as Vulkan/Metal/D3D11: commands, generations,
//! font jobs and lending use the shared executor; resources are keyed by (session, index) with
//! generations; pipelines come as the GLSL dialect payload Cx's shader compiler emitted.
//!
//! One GL context, owned by one render thread (render.rs), does all GL work; the hooks here run
//! inside `hal_submit` and only validate, lay out and queue owned jobs. Windows of the X11 and
//! Wayland shells present through EGL window surfaces; offscreen windows (hidden runs, tests) use
//! the surfaceless platform. Images keep their top row first as on the other backends: every
//! vertex shader of the dialect flips clip-space Y and the present blit flips it back.

pub(crate) mod egl;
mod frame;
mod program;
mod render;
pub mod surface;
mod sys;

use super::exec::{self, Gpu, PipeStatus};
use super::{Backend, Pixels};
use crate::buffers::{CmdView, PipelineBlob};
use crate::glsl::GlslPipeline;
use crate::ledger::{Kind, LedgerEntry};
use crate::session::*;
use crate::types::*;
use render::{Caps, Event, Mailbox, Queue, ReadFormat, Token};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

/// A texture's storage on the render thread; dropping the last lease deletes it there (after
/// every job queued before).
pub(crate) struct TexObj {
    key: u64,
    desc: TextureObj,
    /// a BGRA8 texture that is only uploaded keeps its bytes as RGBA storage, sampled swizzled
    swizzled: bool,
    queue: Queue,
    allocated: Arc<AtomicU64>,
}

impl Drop for TexObj {
    fn drop(&mut self) {
        self.allocated.fetch_sub(self.desc.bytes, Ordering::Relaxed);
        let key = self.key;
        self.queue.push(move |r| r.delete_texture(key));
    }
}

pub(crate) struct BufObj {
    key: u64,
    size: u64,
    queue: Queue,
    allocated: Arc<AtomicU64>,
}

impl Drop for BufObj {
    fn drop(&mut self) {
        self.allocated.fetch_sub(self.size, Ordering::Relaxed);
        let key = self.key;
        self.queue.push(move |r| r.delete_buffer(key));
    }
}

/// What a link tells the HAL side: the data size of each uniform block in `GlslPipeline::blocks`
/// order (None: the program does not use it).
pub(crate) struct Linked {
    block_sizes: Vec<Option<u32>>,
}

/// A program shared by every pipeline with the same payload and layout.
pub(crate) struct PipeState {
    key: u64,
    code: GlslPipeline,
    desc: PipelineDesc,
    linked: OnceLock<Result<Linked, String>>,
    epoch: AtomicU64,
    queue: Queue,
}

impl Drop for PipeState {
    fn drop(&mut self) {
        let key = self.key;
        self.queue.push(move |r| r.delete_program(key));
    }
}

pub(crate) struct Tex {
    gen: u32,
    obj: Arc<TexObj>,
}
struct Buf {
    gen: u32,
    obj: Arc<BufObj>,
}
struct Pipe {
    gen: u32,
    state: Arc<PipeState>,
    epoch: u64,
    announced: bool,
}

pub struct GlBackend {
    shell: Option<&'static super::linux::Shared>,
    info: HalInfo,
    caps: Arc<Caps>,
    queue: Queue,
    thread: Option<std::thread::JoinHandle<()>>,
    next_key: u64,
    allocated: Arc<AtomicU64>,
    tex: HashMap<(u32, u32), Tex>,
    buf: HashMap<(u32, u32), Buf>,
    pipe: HashMap<(u32, u32), Pipe>,
    programs: HashMap<Vec<u8>, Weak<PipeState>>,
    /// windows' images, by (owner, index)
    win: HashMap<(u32, u32), Tex>,
    aliases: HashMap<(u32, Id), Id>,
    /// the id target of each window's newest pick frame, and the targets missing a draw
    pick_ids: HashMap<(u32, Id), Id>,
    pick_pending: HashSet<(u32, Id)>,
    mailbox: Mailbox,
    /// video, audio, MIDI, camera, dialogs (`backend/linux/media`)
    media: super::linux::media::Media,
}

impl GlBackend {
    pub fn new(flags: u32, wake: Arc<dyn Fn() + Send + Sync>, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        if flags & open_flags::OFFSCREEN_WINDOWS != 0 {
            return Self::with_source(wake, Box::new(surface::Offscreen), None);
        }
        let shell = super::linux::Shared::global();
        shell.start(flags, notify)?;
        Self::on_shell(wake, shell)
    }

    /// On a started X11/Wayland shell (the DRM/KMS direct shell presents through Vulkan only).
    pub fn on_shell(wake: Arc<dyn Fn() + Send + Sync>, shell: &'static super::linux::Shared) -> Result<Self, String> {
        let mut backend = Self::with_source(wake, Box::new(super::linux::EglSurfaceSource(shell)), Some(shell))?;
        backend.info.flags |= info_flags::DISPLAY_CLOCK;
        if shell.client_chrome() { backend.info.flags |= info_flags::CLIENT_CHROME; }
        backend.info.dpi = shell.dpi();
        Ok(backend)
    }

    fn with_source(wake: Arc<dyn Fn() + Send + Sync>, source: Box<dyn surface::EglSurfaceSource>, shell: Option<&'static super::linux::Shared>) -> Result<Self, String> {
        let mailbox = Mailbox::new(wake);
        let (queue, caps, thread) = render::start(source, mailbox.clone())?;
        let info = HalInfo { backend: backend_id::GL, dialect: dialect::GLSL, formats: caps.formats, msaa: 1, dpi: 1.0, flags: 0,
            max_texture: caps.max_texture, max_window: [caps.max_texture; 2], ..Default::default() };
        if super::log_enabled() { eprintln!("[hal-gl] {} offscreen={} software={}", caps.renderer, shell.is_none(), caps.software); }
        Ok(Self { shell, info, caps, queue, thread: Some(thread), next_key: 1, allocated: Arc::new(AtomicU64::new(0)),
            tex: HashMap::new(), buf: HashMap::new(), pipe: HashMap::new(), programs: HashMap::new(), win: HashMap::new(),
            aliases: HashMap::new(), pick_ids: HashMap::new(), pick_pending: HashSet::new(), mailbox, media: Default::default() })
    }

    /// A software rasterizer (llvmpipe): the default selection prefers hardware Vulkan and GL.
    pub fn software(&self) -> bool {
        self.caps.software
    }

    fn token(s: &Session) -> Token {
        Token { sid: s.id, incarnation: s.font_frames.clone() }
    }

    fn window_key(&self, sid: u32, id: Id) -> (u32, Id) {
        self.aliases.get(&(sid, id)).map_or((sid, id), |host| (HOST, *host))
    }

    fn key(&mut self) -> u64 {
        self.next_key += 1;
        self.next_key
    }

    fn new_texture(&mut self, mut desc: TextureObj) -> Result<Arc<TexObj>, String> {
        let c = &self.caps;
        if desc.w == 0 || desc.h == 0 || desc.depth == 0 || desc.mips == 0 || desc.mips > 32 - desc.w.max(desc.h).leading_zeros()
            || desc.w > c.max_texture || desc.h > c.max_texture || desc.depth > c.max_layers {
            return Err("invalid texture dimensions or mip count".into());
        }
        if c.formats & (1 << desc.format) == 0 {
            return Err(format!("this GLES context cannot sample {}", format::name(desc.format)));
        }
        if desc.usage & usage::RENDER_TARGET != 0 && format::is_block(desc.format) {
            return Err("compressed textures cannot be render targets".into());
        }
        desc.bytes = exec::texture_bytes(desc.format, desc.w, desc.h, desc.depth, desc.mips);
        desc.imported = false;
        let swizzled = desc.format == format::BGRA8 && desc.usage & usage::RENDER_TARGET == 0;
        let key = self.key();
        let made = desc.clone();
        self.queue.push(move |r| r.create_texture(key, made, swizzled));
        self.allocated.fetch_add(desc.bytes, Ordering::Relaxed);
        Ok(Arc::new(TexObj { key, desc, swizzled, queue: self.queue.clone(), allocated: self.allocated.clone() }))
    }

    fn new_buffer(&mut self, size: u64) -> Arc<BufObj> {
        let key = self.key();
        self.queue.push(move |r| r.create_buffer(key, size));
        self.allocated.fetch_add(size, Ordering::Relaxed);
        Arc::new(BufObj { key, size, queue: self.queue.clone(), allocated: self.allocated.clone() })
    }

    fn drain(&mut self, world: &mut World) {
        for done in self.mailbox.take() {
            let sid = done.token.sid;
            if !world.session(sid).is_some_and(|s| Arc::ptr_eq(&s.font_frames, &done.token.incarnation)) { continue; }
            match done.event {
                Event::Frame { target, kind, frame, window } => {
                    if let Some(s) = world.session_mut(sid) { s.stats.frames += 1; }
                    if window {
                        world.window_presented(sid, target, frame, 0, done.time);
                        if let Some(shell) = self.shell { shell.presented(self.window_key(sid, target)); }
                    }
                    world.frame_done(sid, target, kind, frame, 0, done.time, [0.0; 2]);
                }
                Event::Readback { req, status, rect, pixels } => world.to_session(sid, |q| q.readback(req, status, rect, &pixels)),
                Event::Pick { req, ids } => world.to_session(sid, |q| q.pick_result(req, &ids.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>())),
            }
        }
        for ((sid, index), pipe) in &mut self.pipe {
            let epoch = pipe.state.epoch.load(Ordering::Acquire);
            if epoch == pipe.epoch { continue; }
            pipe.epoch = epoch;
            let id = Id::new(*index, pipe.gen);
            let Some(s) = world.session_mut(*sid) else { continue };
            if !pipe.announced {
                pipe.announced = true;
                s.pipelines_pending = s.pipelines_pending.saturating_sub(1);
            }
            match pipe.state.linked.get() {
                Some(Err(e)) => {
                    eprintln!("[hal-gl] pipeline {id:?} failed: {e}");
                    let log = e.clone();
                    world.to_session(*sid, |q| q.pipeline_failed(id, log.as_bytes()));
                }
                Some(Ok(_)) => world.to_session(*sid, |q| q.pipeline_ready(id)),
                None => {}
            }
        }
        let allocated = self.allocated.load(Ordering::Relaxed);
        if let Some(host) = world.session_mut(HOST) {
            for ((sid, index), t) in &self.win {
                if *sid == HOST && host.ledger.get(Kind::Window, *index).is_none_or(|r| r.gen != t.gen || r.bytes != t.obj.desc.bytes
                    || r.width != t.obj.desc.w || r.height != t.obj.desc.h) {
                    host.ledger.add(window_row(Id::new(*index, t.gen), &t.obj.desc, crate::clock_ns(), host.frame));
                }
            }
        }
        for sid in world.session_ids() {
            if let Some(s) = world.session_mut(sid) { s.ledger.device_allocated = Some(allocated); }
        }
    }

    /// Validates a readback of level 0 and queues it; answers the failure at once.
    fn read(&mut self, s: &mut Session, tex: Option<Arc<TexObj>>, req: u32, rect: [u32; 4], pick: Option<[f32; 2]>) {
        let result = (|| {
            let t = tex.ok_or("readback texture has no storage")?;
            if rect[0] >= t.desc.w || rect[1] >= t.desc.h { return Err("readback origin exceeds texture".to_string()); }
            let rect = [rect[0], rect[1], rect[2].min(t.desc.w - rect[0]), rect[3].min(t.desc.h - rect[1])];
            if rect[2] == 0 || rect[3] == 0 { return Err("empty readback region".into()); }
            let read = ReadFormat::of(&t)?;
            let (token, key) = (Self::token(s), t.key);
            self.queue.push(move |r| r.read(token, key, read, req, rect, pick));
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("[hal-gl] readback: {e}");
            if pick.is_some() { s.events.pick_result(req, &[]); } else { s.events.readback(req, readback::FAILED, rect, &[]); }
        }
    }

    fn read_pick(&mut self, s: &mut Session, window: Id, req: u32, pos: [f32; 2]) {
        let Some(id) = self.pick_ids.get(&(s.id, window)).copied() else { s.events.pick_result(req, &[]); return };
        if self.pick_pending.contains(&(s.id, id)) || pos.iter().any(|p| !p.is_finite()) {
            s.events.pick_result(req, &[]);
            return;
        }
        let tex = self.resolve(s, id);
        let Some(t) = &tex else { s.events.pick_result(req, &[]); return };
        // the 8-texel neighbourhood, as Metal, Vulkan and D3D11 read it
        let pos = pos.map(f32::floor);
        let x = (pos[0] as i64).saturating_sub(8).clamp(0, t.desc.w as i64) as u32;
        let y = (pos[1] as i64).saturating_sub(8).clamp(0, t.desc.h as i64) as u32;
        let x1 = (pos[0] as i64).saturating_add(9).clamp(x as i64, t.desc.w as i64) as u32;
        let y1 = (pos[1] as i64).saturating_add(9).clamp(y as i64, t.desc.h as i64) as u32;
        if x1 == x || y1 == y { s.events.pick_result(req, &[]); return; }
        self.read(s, tex, req, [x, y, x1 - x, y1 - y], Some(pos));
    }

    /// The host's synchronous read of a window's newest frame (automation); never on a shell thread.
    fn host_window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> {
        let s = world.session_mut(sid)?;
        let id = window.or_else(|| s.windows.keys_of(sid).into_iter().next())?;
        let t = self.window_image(s, id)?;
        if !matches!(t.desc.format, format::BGRA8 | format::RGBA8) { return None; }
        let read = ReadFormat::of(&t).ok()?;
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.queue.push(move |r| { let _ = reply.send(r.read_now(t.key, read)); });
        rx.recv().ok().flatten()
    }
}

impl Backend for GlBackend {
    fn info(&self) -> HalInfo {
        self.info
    }
    fn submit(&mut self, world: &mut World, sid: u32, view: &CmdView) {
        self.drain(world);
        exec::submit(self, world, sid, view);
    }
    fn host_gpu(&mut self) -> Option<&mut dyn Gpu> {
        Some(self)
    }
    fn step(&mut self, world: &mut World, sid: u32) {
        self.drain(world);
        if let Some(shell) = self.shell {
            for (owner, id) in shell.drain_events(sid, world) {
                // a parent close also closes native popups, and an X server can destroy a window
                // on its own: retire their surfaces without a WindowClose command
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
            let mut media = std::mem::take(&mut self.media);
            media.step(self, s);
            self.media = media;
        }
        exec::step_timers(world, sid);
    }
    fn next_due(&self, world: &World, sid: u32) -> Option<u64> {
        exec::next_timer(world, sid).into_iter().chain(self.media.next_due(sid, world.now())).min()
    }
    /// The app may make GL contexts of the HAL's share group (GL interop): it hears how.
    fn session_opened(&mut self, world: &mut World, sid: u32) {
        let [display, config, context] = self.caps.share;
        world.to_session(sid, |q| q.gl_share(display, config, context));
    }
    fn session_closed(&mut self, world: &mut World, sid: u32) {
        self.media.session_closed(sid);
        self.queue.push(move |r| r.ctx.close_session(sid));
        self.tex.retain(|k, _| k.0 != sid);
        self.buf.retain(|k, _| k.0 != sid);
        self.pipe.retain(|k, _| k.0 != sid);
        self.win.retain(|k, _| k.0 != sid);
        self.aliases.retain(|k, _| k.0 != sid);
        self.pick_ids.retain(|k, _| k.0 != sid);
        self.pick_pending.retain(|k| k.0 != sid);
        self.programs.retain(|_, state| state.strong_count() > 0);
        exec::free_session(world, sid);
        if let Some(shell) = self.shell { shell.session_closed(sid); }
    }
    fn window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> {
        self.host_window_pixels(world, sid, window)
    }
}

impl Drop for GlBackend {
    fn drop(&mut self) {
        self.queue.push(|r| r.exit());
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

impl Gpu for GlBackend {
    fn media(&mut self, s: &mut Session, c: &crate::cmds::HalCmd) -> bool {
        let mut media = std::mem::take(&mut self.media);
        let handled = media.command(self, s, c);
        self.media = media;
        handled
    }
    /// The host's run-state overlay is drawn by the macOS backend only so far; counted here.
    fn window_overlay(&mut self, s: &mut Session, _id: Id, rect: Option<[f32; 4]>, _layer: Id, _now: u64) {
        if rect.is_some() { s.stats.overlay_unsupported += 1; }
    }
    fn os(&mut self, sid: u32, command: exec::OsCmd) -> bool {
        if matches!(&command, exec::OsCmd::FileDialog { .. }) { return self.media.dialog(sid, command); }
        if let Some(shell) = self.shell { shell.command(sid, command); true } else { false }
    }
    fn texture_create(&mut self, s: &mut Session, id: Id, t: &mut TextureObj) -> Result<(), String> {
        let obj = self.new_texture(t.clone()).map_err(|e| { eprintln!("[hal-gl] texture create: {e}"); e })?;
        t.bytes = obj.desc.bytes;
        self.tex.insert((s.id, id.index), Tex { gen: id.gen, obj });
        Ok(())
    }
    fn texture_update(&mut self, s: &mut Session, id: Id, _t: &TextureObj, mip: u16, rect: [u32; 4], layer: u32, data: &[u8]) {
        let Some(t) = self.resolve(s, id) else { eprintln!("[hal-gl] texture upload: missing texture {id:?}"); s.bad(); return };
        let mip = mip as u32;
        let [x, y, w, h] = rect;
        let (mw, mh) = ((t.desc.w >> mip).max(1), (t.desc.h >> mip).max(1));
        let fits = mip < t.desc.mips && layer < t.desc.depth && w > 0 && h > 0 && x + w <= mw && y + h <= mh
            && (!format::is_block(t.desc.format) || (x % 4 == 0 && y % 4 == 0 && (w % 4 == 0 || x + w == mw) && (h % 4 == 0 || y + h == mh)));
        if !fits || data.len() as u64 != format::level_bytes(t.desc.format, w, h) {
            eprintln!("[hal-gl] texture upload: invalid region");
            s.bad();
            return;
        }
        let (key, data, swap) = (t.key, data.to_vec(), !t.swizzled);
        self.queue.push(move |r| r.upload(key, mip, rect, layer, data, swap));
    }
    fn texture_destroy(&mut self, sid: u32, id: Id) {
        if self.tex.get(&(sid, id.index)).is_some_and(|t| t.gen <= id.gen) { self.tex.remove(&(sid, id.index)); }
        self.pick_pending.remove(&(sid, id));
    }
    fn texture_readback(&mut self, s: &mut Session, id: Id, req: u32, rect: [u32; 4]) {
        if self.pick_pending.contains(&(s.id, id)) {
            s.events.readback(req, readback::INCOMPLETE, rect, &[]);
            return;
        }
        let t = self.resolve(s, id);
        self.read(s, t, req, rect, None);
    }
    /// GLES textures have no cross-process handle here: handle 0, as headless answers.
    fn texture_share(&mut self, s: &mut Session, id: Id) {
        s.events.texture_shared(id, 0);
    }
    fn texture_import(&mut self, _s: &mut Session, _id: Id, _handle: u64, _kind: u32) -> Option<TextureObj> {
        None
    }
    fn texture_adopt_gl(&mut self, s: &mut Session, id: Id, name: u32, format: u32, w: u32, h: u32, owned: bool) -> Option<TextureObj> {
        let bytes = if owned { exec::texture_bytes(format, w, h, 1, 1) } else { 0 };
        let desc = TextureObj { format, w, h, depth: 1, mips: 1, usage: usage::SAMPLED, bytes, imported: true, uploaded: 0, lent: None };
        let key = self.key();
        let made = desc.clone();
        self.queue.push(move |r| r.adopt_texture(key, name, made, owned));
        self.allocated.fetch_add(bytes, Ordering::Relaxed);
        let obj = Arc::new(TexObj { key, desc: desc.clone(), swizzled: false, queue: self.queue.clone(), allocated: self.allocated.clone() });
        self.tex.insert((s.id, id.index), Tex { gen: id.gen, obj });
        Some(desc)
    }
    fn gl_fence(&mut self, _s: &mut Session, sync: u64) {
        self.queue.push(move |r| r.wait_fence(sync));
    }
    fn buffer_create(&mut self, s: &mut Session, id: Id, b: &mut BufferObj) -> Result<(), String> {
        let obj = self.new_buffer(b.size);
        b.bytes = b.size;
        self.buf.insert((s.id, id.index), Buf { gen: id.gen, obj });
        Ok(())
    }
    fn buffer_update(&mut self, s: &mut Session, id: Id, _b: &mut BufferObj, off: u64, data: &[u8]) {
        match self.buf.get(&(s.id, id.index)).filter(|r| r.gen == id.gen) {
            Some(r) => {
                let (key, data) = (r.obj.key, data.to_vec());
                self.queue.push(move |r| r.write_buffer(key, off, data));
            }
            None => {
                eprintln!("[hal-gl] buffer update: missing buffer {id:?}");
                s.bad();
            }
        }
    }
    fn buffer_destroy(&mut self, sid: u32, id: Id) {
        if self.buf.get(&(sid, id.index)).is_some_and(|b| b.gen <= id.gen) { self.buf.remove(&(sid, id.index)); }
    }
    fn buffer_evictable(&mut self, _sid: u32, _id: Id) {}
    fn pipeline_create(&mut self, s: &mut Session, id: Id, dialect: u32, blob: &PipelineBlob, _: Attr) -> PipeStatus {
        if dialect != self::dialect::GLSL { return PipeStatus::Failed("GLES takes the GLSL dialect payload".into()); }
        let code = match GlslPipeline::decode(blob.code) { Ok(c) => c, Err(e) => return PipeStatus::Failed(e) };
        let custom = blob.bindings.len().saturating_sub(pipeline_binding::BUFFERS);
        if blob.bindings.len() < pipeline_binding::BUFFERS || blob.bindings[pipeline_binding::TEXTURES] as usize != code.textures.len()
            || code.blocks.len() != 5 + custom {
            return PipeStatus::Failed("GLSL payload and pipeline bindings disagree".into());
        }
        let mut key = blob.code.to_vec();
        for v in [blob.desc.blend, blob.desc.instance_stride, blob.desc.outputs] { key.extend_from_slice(&v.to_le_bytes()); }
        let state = self.programs.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
            let state = Arc::new(PipeState { key: self.key(), code, desc: blob.desc, linked: OnceLock::new(), epoch: AtomicU64::new(0), queue: self.queue.clone() });
            let job = state.clone();
            self.queue.push(move |r| r.compile(&job));
            self.programs.insert(key, Arc::downgrade(&state));
            state
        });
        // a program linked for an earlier pipeline is announced at the next drain like a new one
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
        let result = (|| {
            let source = self.window_image(s, window).ok_or("window has no retained frame")?;
            let dest = self.resolve(s, texture).ok_or("copy destination has no storage")?;
            if source.desc.w != dest.desc.w || source.desc.h != dest.desc.h || source.desc.format != dest.desc.format
                || dest.desc.usage & usage::RENDER_TARGET == 0 || Arc::ptr_eq(&source, &dest) {
                return Err("window copy dimensions/format differ".to_string());
            }
            let (token, from, to, rect) = (Self::token(s), source.key, dest.key, [0, 0, source.desc.w, source.desc.h]);
            self.queue.push(move |r| r.copy(token, from, to, req, rect));
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("[hal-gl] window copy: {e}");
            s.events.readback(req, readback::FAILED, [0; 4], &[]);
        }
    }
    fn window_closed(&mut self, sid: u32, id: Id) {
        self.win.remove(&(sid, id.index));
        self.pick_ids.remove(&(sid, id));
        self.queue.push(move |r| r.ctx.close((sid, id)));
    }
    fn window_lent(&mut self, sid: u32, id: Id, host: Id, attach: bool) {
        if let Some(shell) = self.shell { shell.lend(host, if attach { sid } else { HOST }, if attach { id } else { host }); }
        if attach {
            self.aliases.insert((sid, id), host);
        } else {
            self.aliases.remove(&(sid, id));
            self.pick_ids.remove(&(sid, id));
        }
    }
}

/// Decoded video planes reach GL as uploads into ordinary 2D textures; DMA-BUF planes are
/// imported by the Vulkan backend only, so the media service falls back to system memory.
impl super::linux::media::Sink for GlBackend {
    fn declare(&mut self, s: &mut Session, id: Id, fmt: u32) {
        if s.textures.get(s.id, id).is_ok() { return; }
        let desc = TextureObj { format: fmt, w: 1, h: 1, depth: 1, mips: 1, usage: usage::SAMPLED, bytes: 0, imported: true, uploaded: 0, lent: None };
        if s.textures.insert(s.id, id, s.id, desc.clone()).is_ok() { video_row(s, id, &desc); } else { s.stale(); }
    }
    fn present(&mut self, s: &mut Session, ids: [Id; 4], frame: &super::linux::media::video::Frame) -> Result<bool, String> {
        use super::linux::media::video::Storage;
        let targets: Vec<_> = if frame.yuv & crate::media::yuv::ENABLED != 0 { ids[1..].to_vec() } else { vec![ids[0]] };
        if targets.len() != frame.planes.len() { return Err("video frame plane count differs from texture targets".into()); }
        for id in &targets {
            if s.textures.get(s.id, *id).is_err() { return Ok(false); }
        }
        let mut uploads = Vec::new();
        for (&id, plane) in targets.iter().zip(&frame.planes) {
            let Storage::Bytes(bytes) = &plane.storage else { return Err("GLES does not import DMA-BUF video planes".into()) };
            if bytes.len() as u64 != format::level_bytes(plane.format, plane.width, plane.height) {
                return Err("video plane byte count differs from its extent".into());
            }
            let reuse = self.tex.get(&(s.id, id.index)).filter(|t| t.gen == id.gen && t.obj.desc.format == plane.format
                && t.obj.desc.w == plane.width && t.obj.desc.h == plane.height).map(|t| t.obj.clone());
            let obj = match reuse {
                Some(obj) => obj,
                None => self.new_texture(TextureObj { format: plane.format, w: plane.width, h: plane.height, depth: 1, mips: 1,
                    usage: usage::SAMPLED, bytes: 0, imported: true, uploaded: 0, lent: None })?,
            };
            uploads.push((id, obj, bytes.clone()));
        }
        for (id, obj, bytes) in uploads {
            let (key, w, h, swap) = (obj.key, obj.desc.w, obj.desc.h, !obj.swizzled);
            self.queue.push(move |r| r.upload(key, 0, [0, 0, w, h], 0, bytes.to_vec(), swap));
            let mut desc = obj.desc.clone();
            desc.imported = true;
            s.textures.get_mut(s.id, id).map_err(|_| "stale video texture")?.val = desc.clone();
            self.tex.insert((s.id, id.index), Tex { gen: id.gen, obj });
            video_row(s, id, &desc);
        }
        Ok(true)
    }
    fn release(&mut self, s: &mut Session, ids: [Id; 4]) {
        for id in ids {
            if self.tex.get(&(s.id, id.index)).is_some_and(|t| t.gen == id.gen) { self.tex.remove(&(s.id, id.index)); }
            if let Ok(entry) = s.textures.get_mut(s.id, id) {
                entry.val.bytes = 0;
                entry.val.imported = true;
            }
            if let Some(row) = s.ledger.get_mut(Kind::Texture, id.index).filter(|r| r.gen == id.gen) { row.bytes = 0; }
        }
    }
}

fn video_row(s: &mut Session, id: Id, desc: &TextureObj) {
    if let Some(row) = s.ledger.get_mut(Kind::Texture, id.index).filter(|r| r.gen == id.gen) {
        row.bytes = desc.bytes;
        row.width = desc.w;
        row.height = desc.h;
        row.format = desc.format;
    } else {
        s.ledger.add(LedgerEntry { kind: Kind::Texture, detail: "video frame", index: id.index, gen: id.gen, bytes: desc.bytes,
            format: desc.format, width: desc.w, height: desc.h, mips: 1, attr: Attr::default(),
            created_ns: crate::clock_ns(), created_frame: s.frame, last_used_frame: 0, last_used_ns: 0 });
    }
}

fn window_row(id: Id, desc: &TextureObj, now: u64, frame: u64) -> LedgerEntry {
    LedgerEntry { kind: Kind::Window, detail: "window surface", index: id.index, gen: id.gen, bytes: desc.bytes,
        format: desc.format, width: desc.w, height: desc.h, mips: 1, attr: Attr::default(),
        created_ns: now, created_frame: frame, last_used_frame: frame, last_used_ns: now }
}

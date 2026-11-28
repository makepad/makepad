//! The render thread. It owns the GLES context and every GL object, keyed by the HAL side's
//! object keys, and runs the jobs the HAL's Gpu hooks queue in submission order (GL orders a
//! write after the reads queued before it, so no lease outlives a job). Completion is a fence
//! per frame or readback, polled between jobs; results go to the mailbox the HAL drains.

use super::egl::{lock_context_lifecycle, Context};
use super::surface::EglSurfaceSource;
use super::sys::{self, GLenum, GLsync, GLuint};
use super::{GlKey, PipeState, TexObj};
use crate::font::FrameProgress;
use crate::ledger::{Charge, Pool, Pools};
use crate::session::TextureObj;
use crate::types::*;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

pub(super) type Job = Box<dyn FnOnce(&mut Renderer) + Send>;

/// The HAL side's handle to the render thread.
#[derive(Clone)]
pub(super) struct Queue {
    jobs: mpsc::Sender<Job>,
    /// jobs that run before the ones queued ahead of them, as soon as the running one ends
    #[cfg(target_os = "android")]
    urgent: Arc<Mutex<Vec<Job>>>,
    /// how many window surfaces the render thread holds
    #[cfg(target_os = "android")]
    pub surfaces: Arc<std::sync::atomic::AtomicUsize>,
}

impl Queue {
    pub fn push(&self, job: impl FnOnce(&mut Renderer) + Send + 'static) {
        // after the backend is gone the context and all its objects are gone with the thread
        let _ = self.jobs.send(Box::new(job));
    }

    /// For a job that can't wait behind queued work (Android's surfaceDestroyed, the compile
    /// worker's answers): it must not depend on anything queued before it.
    #[cfg(target_os = "android")]
    pub fn push_urgent(&self, job: impl FnOnce(&mut Renderer) + Send + 'static) {
        self.urgent.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(job));
        self.push(|_| {});
    }
}

/// What the context can do, read once on the render thread.
pub(super) struct Caps {
    pub formats: u64,
    pub max_texture: u32,
    pub max_vertex_texture_units: u32,
    pub max_fragment_texture_units: u32,
    pub max_combined_texture_units: u32,
    pub max_layers: u32,
    pub ubo_align: usize,
    pub max_ubo: usize,
    pub border_clamp: bool,
    pub float_blend: bool,
    /// OES_texture_float_linear: RGBA32F and R32F textures can be filtered
    pub float_linear: bool,
    /// The anisotropy limit, or 1 when the context has no anisotropic filtering.
    pub max_anisotropy: f32,
    /// KHR_parallel_shader_compile: links finish on the driver's threads. EGL may hand out its
    /// entry point on a driver without the extension, so the extension list decides.
    pub parallel_compile: bool,
    pub renderer: String,
    /// a software rasterizer: the default selection prefers hardware Vulkan and GL to it
    pub software: bool,
    /// frames carry their GPU time (`timer.rs`)
    pub gpu_timer: bool,
    /// the program cache keeps binaries but has none yet (`info_flags::PIPELINE_CACHE_COLD`)
    pub program_cache_cold: bool,
    /// the EGL display, config and context of the share group (the `GlShare` event)
    #[cfg(target_os = "linux")]
    pub share: [u64; 3],
}

#[derive(Clone)]
pub(super) struct Token {
    pub sid: u32,
    pub incarnation: Arc<FrameProgress>,
}

#[derive(Default)]
pub(super) struct UploadStats {
    pub ns: u64,
    pub driver_uploads: u64,
}

pub(super) enum Event {
    /// `gpu_ns`: 0 when the GPU time isn't known. `ran`: the GPU did the frame's work.
    Frame { target: Id, kind: u32, frame: u64, window: bool, gpu_ns: u64, ran: bool, binds: u64, cpu_ns: u64, upload_ns: UploadStats },
    Counters { binds: u64, cpu_ns: u64, upload_ns: UploadStats },
    BufferLeases { req: u32, status: u32, leases: Vec<crate::lease::Lease> },
    Readback { req: u32, status: u32, rect: [u32; 4], lease: Option<crate::lease::Lease> },
}

pub(super) struct Done {
    pub token: Token,
    pub time: u64,
    pub event: Event,
}

#[derive(Clone)]
pub(super) struct Mailbox {
    events: Arc<Mutex<Vec<Done>>>,
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl Mailbox {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { events: Arc::new(Mutex::new(Vec::new())), wake }
    }
    pub fn push(&self, token: Token, event: Event) {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).push(Done { token, event, time: crate::clock_ns() });
        (self.wake)();
    }
    pub fn take(&self) -> Vec<Done> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// How a readback's texels become the event's pixels.
#[derive(Clone, Copy)]
pub(super) struct ReadFormat {
    pub format: GLenum,
    pub ty: GLenum,
    /// bytes of one texel as read
    pub texel: usize,
    /// what the HAL hands out: RGBA8 (`OK`) or the texture's own layout (`NATIVE`)
    pub hal: u32,
    pub swap_rb: bool,
}

impl ReadFormat {
    pub fn of(t: &TexObj) -> Result<Self, String> {
        let (format, ty, texel) = match t.desc.format {
            format::RGBA8 | format::BGRA8 | format::R8 | format::RG8 => (sys::RGBA, sys::UNSIGNED_BYTE, 4),
            format::RGBA16F | format::RGBA32F | format::R32F | format::RG16F | format::R16F => (sys::RGBA, sys::FLOAT, 16),
            format::R32UINT => (sys::RGBA_INTEGER, sys::UNSIGNED_INT, 16),
            f if format::is_depth(f) => return Err("GLES cannot read depth texels back".into()),
            _ => return Err("GLES cannot read compressed texels back".into()),
        };
        Ok(Self { format, ty, texel, hal: t.desc.format, swap_rb: t.swizzled })
    }

    /// The HAL's bytes of tightly packed read texels.
    pub fn convert(&self, read: &[u8]) -> (u32, Vec<u8>) {
        let texels = read.chunks_exact(self.texel);
        match self.hal {
            format::RGBA8 | format::BGRA8 => {
                let mut out = read.to_vec();
                if self.swap_rb { for p in out.chunks_exact_mut(4) { p.swap(0, 2); } }
                (readback::OK, out)
            }
            format::R8 => (readback::NATIVE, texels.map(|p| p[0]).collect()),
            format::RG8 => (readback::NATIVE, texels.flat_map(|p| [p[0], p[1]]).collect()),
            format::RGBA32F => (readback::NATIVE, read.to_vec()),
            format::R32F | format::R32UINT => (readback::NATIVE, texels.flat_map(|p| [p[0], p[1], p[2], p[3]]).collect()),
            format::RGBA16F => (readback::NATIVE, texels.flat_map(|p| (0..4).flat_map(move |c| half(&p[c * 4..c * 4 + 4]))).collect()),
            format::R16F => (readback::NATIVE, texels.flat_map(|p| half(&p[0..4])).collect()),
            _ => (readback::NATIVE, texels.flat_map(|p| (0..2).flat_map(move |c| half(&p[c * 4..c * 4 + 4]))).collect()),
        }
    }
}

/// An f32 (little-endian bytes) as IEEE half bytes, rounded to nearest even.
fn half(b: &[u8]) -> [u8; 2] {
    let x = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let man = x & 0x7f_ffff;
    let h = if exp == 0xff {
        sign | 0x7c00 | if man != 0 { 0x200 } else { 0 }
    } else if exp - 127 + 15 >= 0x1f {
        sign | 0x7c00
    } else if exp - 127 + 15 <= 0 {
        let shift = 14 - (exp - 127 + 15);
        if shift > 24 { sign } else {
            let m = man | 0x80_0000;
            let r = (m >> shift) as u16;
            let rest = m & ((1 << shift) - 1);
            let mid = 1 << (shift - 1);
            sign | (r + (rest > mid || (rest == mid && r & 1 == 1)) as u16)
        }
    } else {
        let e = ((exp - 127 + 15) as u32) << 10;
        let r = e | (man >> 13);
        let rest = man & 0x1fff;
        sign | (r + (rest > 0x1000 || (rest == 0x1000 && r & 1 == 1)) as u32) as u16
    };
    h.to_le_bytes()
}

pub(super) enum Then {
    Frame { token: Token, target: Id, kind: u32, frame: u64, window: bool, timing: Option<(GLuint, u64)>, binds: u64, cpu_ns: u64, upload_ns: UploadStats },
    Read { token: Token, req: u32, rect: [u32; 4], read: ReadFormat, pbo: GLuint, _charge: Charge },
    Copy { token: Token, req: u32, rect: [u32; 4] },
}

pub(super) struct GlTex {
    pub name: GLuint,
    pub target: GLenum,
    pub desc: TextureObj,
    /// false for a texture the app adopted from its own GL context (the app deletes it)
    pub owned: bool,
}

pub(super) struct GlBuf {
    pub name: GLuint,
    pub size: u64,
    append: bool,
    /// the words as an R32UI texture, for programs that fetch instances by id (made on use)
    pub words: Words,
    /// Byte range `[start, end)` to refresh before the next draw that fetches words.
    pub words_dirty: Option<(u64, u64)>,
}

/// Instance words as an R32UI array texture of `WORD_ROW`-wide rows in layers of `WORD_ROW` rows
/// (`Renderer::words_texture`), and its ledger charge.
#[derive(Default)]
pub(super) struct Words {
    pub tex: GLuint,
    rows: u32,
    _charge: Option<Charge>,
}

/// The size of the instance word textures' rows and of their layers' height (GLES 3 guarantees
/// 2048 texels): a layer is one window of 2048² words. The shader emitter's
/// `INSTANCE_WORD_ROW_SHIFT` addresses the same row and layer.
pub(super) const WORD_ROW: u32 = 2048;

pub(super) struct Renderer {
    pub ctx: Context,
    /// The context's GL_CULL_FACE_MODE, retained across passes even while culling is disabled.
    pub cull_face: GLenum,
    pub binds: u64,
    upload_times: HashMap<u32, (Token, UploadStats)>,
    pub caps: Arc<Caps>,
    pub textures: Vec<Option<GlTex>>,
    tex_recycled: Arc<Mutex<Vec<GlKey>>>,
    pub buffers: Vec<Option<GlBuf>>,
    buf_recycled: Arc<Mutex<Vec<GlKey>>>,
    pub programs: Vec<Option<super::program::GlProgram>>,
    pub(super) prog_recycled: Arc<Mutex<Vec<GlKey>>>,
    pub linking: Vec<(Weak<PipeState>, super::program::Linking)>,
    pub fbos: HashMap<[(GlKey, u32); 5], GLuint>,
    pub read_fbo: GLuint,
    pub copy_fbo: GLuint,
    /// 1x1 textures per sampled shape for a draw that names no texture
    pub fallback: [GLuint; 5],
    /// nearest samplers per address mode and compare mode, for formats the driver can't filter
    pub nearest: [GLuint; 8],
    /// the frame's uniform blocks
    pub uniform_arena: GLuint,
    pub zero_uniform: (GLuint, usize),
    zero_uniform_charge: Option<Charge>,
    /// what this thread allocates for itself, on the HAL's ledger as backend rows
    pub pools: Arc<Pools>,
    /// the uniform arena's size as last specified, and its charge
    pub arena_charge: Option<(u64, Charge)>,
    _fallback_charges: [Charge; 5],
    pending: VecDeque<(GLsync, Then)>,
    pub mailbox: Mailbox,
    /// its own queue's urgent lane, which the compile worker answers through
    #[cfg(target_os = "android")]
    pub queue: Queue,
    /// a window frame has been shown: from then on the compile worker can take compiles
    #[cfg(target_os = "android")]
    pub presented: bool,
    #[cfg(target_os = "android")]
    pub worker: super::worker::WorkerState,
    /// made with the context, since the Activity's params (the app's cache dir) come before the app starts
    pub cache: super::cache::ProgramCache,
    pub timer: Option<super::timer::GpuTimer>,
    exit: bool,
}

pub(super) fn slot_for_insert<T>(slots: &mut Vec<Option<T>>, key: GlKey) -> &mut Option<T> {
    let slot = key as usize;
    if slot >= slots.len() { slots.resize_with(slot + 1, || None); }
    &mut slots[slot]
}

pub(super) fn start(source: Box<dyn EglSurfaceSource>, mailbox: Mailbox, pools: Arc<Pools>,
    tex_recycled: Arc<Mutex<Vec<GlKey>>>, buf_recycled: Arc<Mutex<Vec<GlKey>>>, prog_recycled: Arc<Mutex<Vec<GlKey>>>,
) -> Result<(Queue, Arc<Caps>, std::thread::JoinHandle<()>), String> {
    let (tx, rx) = mpsc::channel::<Job>();
    let queue = Queue { jobs: tx, #[cfg(target_os = "android")] urgent: Arc::new(Mutex::new(Vec::new())), #[cfg(target_os = "android")] surfaces: Default::default() };
    #[cfg(target_os = "android")]
    let own = queue.clone();
    let (ready, made) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new().name("makepad-gl-render".into()).spawn(move || {
        match Renderer::new(source, mailbox, pools, tex_recycled, buf_recycled, prog_recycled, #[cfg(target_os = "android")] own) {
            Ok(r) => {
                let _ = ready.send(Ok(r.caps.clone()));
                r.run(rx);
            }
            Err(e) => { let _ = ready.send(Err(e)); }
        }
    }).map_err(|e| format!("render thread: {e}"))?;
    match made.recv() {
        Ok(Ok(caps)) => Ok((queue, caps, thread)),
        Ok(Err(e)) => { let _ = thread.join(); Err(e) }
        Err(_) => { let _ = thread.join(); Err("the GL render thread ended during startup".into()) }
    }
}

impl Renderer {
    pub(super) fn ensure_zero_uniform(&mut self, need: usize) {
        let (mut name, capacity) = self.zero_uniform;
        if name != 0 && capacity >= need { return; }
        let gl = &self.ctx.gl;
        let size = need.max(256).next_power_of_two().min(self.caps.max_ubo.max(256));
        let zeros = vec![0u8; size];
        unsafe {
            if name == 0 { (gl.glGenBuffers)(1, &mut name); }
            (gl.glBindBuffer)(sys::UNIFORM_BUFFER, name);
            (gl.glBufferData)(sys::UNIFORM_BUFFER, size as isize, zeros.as_ptr() as _, sys::STATIC_DRAW);
            (gl.glBindBuffer)(sys::UNIFORM_BUFFER, 0);
        }
        self.zero_uniform = (name, size);
        self.zero_uniform_charge = Some(self.pools.charge(Pool::Fallback, size as u64));
    }

    fn new(source: Box<dyn EglSurfaceSource>, mailbox: Mailbox, pools: Arc<Pools>,
        tex_recycled: Arc<Mutex<Vec<GlKey>>>, buf_recycled: Arc<Mutex<Vec<GlKey>>>, prog_recycled: Arc<Mutex<Vec<GlKey>>>,
        #[cfg(target_os = "android")] queue: Queue,
    ) -> Result<Self, String> {
        // Drop the guard before ctx on errors; Context::drop takes the same lock.
        let ctx;
        let _context_lifecycle = lock_context_lifecycle();
        ctx = Context::new(source, #[cfg(target_os = "android")] queue.surfaces.clone())?;
        let gl = &ctx.gl;
        let version = gl.string(sys::VERSION);
        if !version.starts_with("OpenGL ES 3") {
            return Err(format!("a GLES 3 context is required, the driver made \"{version}\""));
        }
        let ext = gl.extensions();
        let has = |name: &str| ext.iter().any(|e| e == name);
        let mut max_anisotropy = 1.0;
        let core_anisotropy = !version.starts_with("OpenGL ES")
            && (gl.integer(sys::MAJOR_VERSION), gl.integer(sys::MINOR_VERSION)) >= (4, 6);
        if core_anisotropy || has("GL_EXT_texture_filter_anisotropic") || has("GL_ARB_texture_filter_anisotropic") {
            unsafe { (gl.glGetFloatv)(sys::MAX_TEXTURE_MAX_ANISOTROPY, &mut max_anisotropy); }
        }
        let mut formats = [format::RGBA8, format::BGRA8, format::R8, format::RG8, format::RGBA16F, format::RGBA32F,
            format::DEPTH32F, format::DEPTH24_STENCIL8, format::R32F, format::RG16F, format::R16F, format::R32UINT].iter().fold(0u64, |b, f| b | 1 << f);
        if has("GL_EXT_texture_compression_bptc") { formats |= 1 << format::BC7; }
        if has("GL_EXT_texture_compression_rgtc") { formats |= 1 << format::BC5; }
        if has("GL_KHR_texture_compression_astc_ldr") { formats |= 1 << format::ASTC4X4; }
        // A quarter off a cold compile, and the driver keeps it for every later context. Only the
        // process's first context may ask: NVIDIA's driver can sleep in this call for good when
        // another context is up, here under the lock every other context waits on.
        static COMPILER_THREADS: std::sync::Once = std::sync::Once::new();
        let parallel_compile = has("GL_KHR_parallel_shader_compile") && gl.glMaxShaderCompilerThreadsKHR.is_some();
        if let Some(threads) = gl.glMaxShaderCompilerThreadsKHR.filter(|_| parallel_compile) {
            COMPILER_THREADS.call_once(|| unsafe { threads(u32::MAX) })
        }
        let timer = super::timer::GpuTimer::new(gl, &ext);
        let cache = super::cache::ProgramCache::new(gl);
        let caps = Arc::new(Caps {
            formats,
            max_vertex_texture_units: gl.integer(sys::MAX_VERTEX_TEXTURE_IMAGE_UNITS) as u32,
            max_fragment_texture_units: gl.integer(sys::MAX_TEXTURE_IMAGE_UNITS) as u32,
            max_combined_texture_units: gl.integer(sys::MAX_COMBINED_TEXTURE_IMAGE_UNITS) as u32,
            max_texture: gl.integer(sys::MAX_TEXTURE_SIZE).min(gl.integer(sys::MAX_RENDERBUFFER_SIZE)).max(2048) as u32,
            max_layers: gl.integer(sys::MAX_ARRAY_TEXTURE_LAYERS).max(256) as u32,
            ubo_align: gl.integer(sys::UNIFORM_BUFFER_OFFSET_ALIGNMENT).max(16) as usize,
            max_ubo: gl.integer(sys::MAX_UNIFORM_BLOCK_SIZE).max(16384) as usize,
            border_clamp: has("GL_EXT_texture_border_clamp") || has("GL_OES_texture_border_clamp"),
            float_blend: has("GL_EXT_float_blend"),
            float_linear: has("GL_OES_texture_float_linear"),
            max_anisotropy,
            parallel_compile,
            renderer: format!("{} ({version})", gl.string(sys::RENDERER)),
            software: ctx.software(),
            gpu_timer: timer.is_some(),
            program_cache_cold: cache.cold,
            #[cfg(target_os = "linux")]
            share: [ctx.share.display as u64, ctx.share.config as u64, ctx.share.context as u64],
        });
        let mut names = [0; 3];
        unsafe {
            (gl.glGenFramebuffers)(2, names.as_mut_ptr());
            (gl.glGenBuffers)(1, names[2..].as_mut_ptr());
            (gl.glPixelStorei)(sys::UNPACK_ALIGNMENT, 1);
            (gl.glPixelStorei)(sys::PACK_ALIGNMENT, 1);
            (gl.glDepthFunc)(sys::LEQUAL);
            // every vertex shader of this dialect flips clip-space Y (images keep the top row
            // first, as on the other backends), which reverses the winding
            (gl.glFrontFace)(sys::CW);
            (gl.glCullFace)(sys::BACK);
            (gl.glBlendFuncSeparate)(sys::ONE, sys::ONE_MINUS_SRC_ALPHA, sys::ONE, sys::ONE_MINUS_SRC_ALPHA);
        }
        let made = [0, 1, 2, 3, 4].map(|shape| fallback(gl, shape));
        let fallback = made.map(|(name, _)| name);
        let fallback_charges = made.map(|(_, bytes)| pools.charge(Pool::Fallback, bytes));
        let nearest = std::array::from_fn(|i| {
            let mut sampler = 0;
            unsafe {
                (gl.glGenSamplers)(1, &mut sampler);
                (gl.glSamplerParameteri)(sampler, sys::TEXTURE_MAG_FILTER, sys::NEAREST as i32);
                (gl.glSamplerParameteri)(sampler, sys::TEXTURE_MIN_FILTER, sys::NEAREST_MIPMAP_NEAREST as i32);
                let wrap = wrap(i as u32 / 2, caps.border_clamp) as i32;
                for p in [sys::TEXTURE_WRAP_S, sys::TEXTURE_WRAP_T, sys::TEXTURE_WRAP_R] { (gl.glSamplerParameteri)(sampler, p, wrap) }
                if i % 2 == 1 {
                    (gl.glSamplerParameteri)(sampler, sys::TEXTURE_COMPARE_MODE, sys::COMPARE_REF_TO_TEXTURE as i32);
                    (gl.glSamplerParameteri)(sampler, sys::TEXTURE_COMPARE_FUNC, sys::LEQUAL as i32);
                }
            }
            sampler
        });
        if super::super::log_enabled() {
            eprintln!("[hal-gl] context: {} ubo_align={} extensions={} float_linear={}", caps.renderer, caps.ubo_align, ext.len(), caps.float_linear);
        }
        Ok(Self { ctx, caps, cull_face: sys::BACK, binds: 0, upload_times: HashMap::new(), textures: Vec::new(), tex_recycled,
            buffers: Vec::new(), buf_recycled, programs: Vec::new(), prog_recycled, linking: Vec::new(),
            fbos: HashMap::new(), read_fbo: names[0], copy_fbo: names[1], fallback, nearest,
            uniform_arena: names[2], zero_uniform: (0, 0), zero_uniform_charge: None, pools, arena_charge: None,
            _fallback_charges: fallback_charges, pending: VecDeque::new(), mailbox,
            #[cfg(target_os = "android")] queue, #[cfg(target_os = "android")] presented: false,
            #[cfg(target_os = "android")] worker: super::worker::WorkerState::Unstarted, cache, timer, exit: false })
    }

    pub fn exit(&mut self) {
        self.exit = true;
    }

    pub fn time_upload(&mut self, token: Token, upload: impl FnOnce(&mut Self) -> u64) {
        let started = crate::clock_ns();
        let driver_uploads = upload(self);
        let elapsed = crate::clock_ns().saturating_sub(started);
        let stats = &mut self.upload_times.entry(token.sid).or_insert_with(|| (token, UploadStats::default())).1;
        stats.ns += elapsed;
        stats.driver_uploads += driver_uploads;
    }

    pub fn take_upload_ns(&mut self, sid: u32) -> UploadStats {
        let stats = self.upload_times.remove(&sid).map(|(_, stats)| stats).unwrap_or_default();
        self.flush_upload_times();
        stats
    }

    // An upload-only session may never submit a frame of its own.
    fn flush_upload_times(&mut self) {
        if !self.upload_times.is_empty() {
            for (_, (token, upload_ns)) in self.upload_times.drain() {
                self.mailbox.push(token, Event::Counters { binds: 0, cpu_ns: 0, upload_ns });
            }
        }
    }

    fn run(mut self, rx: Receiver<Job>) {
        loop {
            #[cfg(target_os = "android")]
            {
                let urgent = std::mem::take(&mut *self.queue.urgent.lock().unwrap_or_else(|e| e.into_inner()));
                for job in urgent { job(&mut self); }
            }
            let next = rx.try_recv();
            if matches!(next, Err(TryRecvError::Empty)) {
                self.flush_upload_times();
            }
            let next = match next {
                Ok(job) => Some(job),
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) if self.pending.is_empty() && self.linking.is_empty() => match rx.recv() {
                    Ok(job) => Some(job),
                    Err(_) => break,
                },
                Err(TryRecvError::Empty) => match self.pending.front() {
                    Some((fence, _)) => {
                        unsafe { (self.ctx.gl.glClientWaitSync)(*fence, sys::SYNC_FLUSH_COMMANDS_BIT, 1_000_000) };
                        None
                    }
                    None => match rx.recv_timeout(Duration::from_millis(1)) {
                        Ok(job) => Some(job),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    },
                },
            };
            if let Some(job) = next {
                job(&mut self);
                if self.exit { break; }
            }
            self.poll();
        }
    }

    /// Completions whose fence signalled, in submission order, and finished links.
    fn poll(&mut self) {
        while let Some((fence, _)) = self.pending.front() {
            let status = unsafe { (self.ctx.gl.glClientWaitSync)(*fence, 0, 0) };
            if status != sys::ALREADY_SIGNALED && status != sys::CONDITION_SATISFIED && status != sys::WAIT_FAILED { break; }
            let (fence, then) = self.pending.pop_front().unwrap();
            unsafe { (self.ctx.gl.glDeleteSync)(fence) };
            self.complete(then, status == sys::WAIT_FAILED);
        }
        if !self.linking.is_empty() { self.poll_links(); }
    }

    /// Runs `then` once the GPU has executed everything queued so far.
    pub fn after_gpu(&mut self, then: Then) {
        let fence = unsafe { (self.ctx.gl.glFenceSync)(sys::SYNC_GPU_COMMANDS_COMPLETE, 0) };
        unsafe { (self.ctx.gl.glFlush)() };
        self.pending.push_back((fence, then));
    }

    fn complete(&mut self, then: Then, failed: bool) {
        let gl = &self.ctx.gl;
        match then {
            Then::Frame { token, target, kind, frame, window, timing, binds, cpu_ns, upload_ns } => {
                let gpu_ns = match (timing, &mut self.timer) {
                    (Some((query, began)), Some(timer)) if !failed => timer.read(gl, query, began),
                    _ => 0,
                };
                token.incarnation.done(frame);
                self.mailbox.push(token, Event::Frame { target, kind, frame, window, gpu_ns, ran: !failed, binds, cpu_ns, upload_ns });
            }
            Then::Copy { token, req, rect } => {
                self.mailbox.push(token, Event::Readback { req, rect, status: if failed { readback::FAILED } else { readback::OK }, lease: None });
            }
            Then::Read { token, req, rect, read, pbo, .. } => {
                let len = rect[2] as usize * rect[3] as usize * read.texel;
                let mut bytes = Vec::new();
                unsafe {
                    (gl.glBindBuffer)(sys::PIXEL_PACK_BUFFER, pbo);
                    let p = if failed { std::ptr::null_mut() } else { (gl.glMapBufferRange)(sys::PIXEL_PACK_BUFFER, 0, len as isize, sys::MAP_READ_BIT) };
                    if !p.is_null() {
                        bytes.extend_from_slice(std::slice::from_raw_parts(p as *const u8, len));
                        (gl.glUnmapBuffer)(sys::PIXEL_PACK_BUFFER);
                    }
                    (gl.glBindBuffer)(sys::PIXEL_PACK_BUFFER, 0);
                    (gl.glDeleteBuffers)(1, &pbo);
                }
                let event = if bytes.len() != len {
                    eprintln!("[hal-gl] readback: the pixels could not be mapped");
                    Event::Readback { req, rect, status: readback::FAILED, lease: None }
                } else {
                    let (status, pixels) = read.convert(&bytes);
                    Event::Readback { req, rect, status, lease: Some(crate::lease::Lease::from_vec(pixels, 0)) }
                };
                self.mailbox.push(token, event);
            }
        }
    }

    // ---- objects

    pub fn create_texture(&mut self, key: GlKey, desc: TextureObj, swizzled: bool) {
        let gl = &self.ctx.gl;
        let (ifmt, _, _) = gl_format(desc.format);
        let target = texture_target(&desc);
        let mut name = 0;
        unsafe {
            (gl.glGenTextures)(1, &mut name);
            (gl.glBindTexture)(target, name);
            if target == sys::TEXTURE_2D_ARRAY {
                (gl.glTexStorage3D)(target, desc.mips as i32, ifmt, desc.w as i32, desc.h as i32, desc.depth as i32);
            } else {
                (gl.glTexStorage2D)(target, desc.mips as i32, ifmt, desc.w as i32, desc.h as i32);
            }
            if swizzled {
                (gl.glTexParameteri)(target, sys::TEXTURE_SWIZZLE_R, sys::BLUE as i32);
                (gl.glTexParameteri)(target, sys::TEXTURE_SWIZZLE_B, sys::RED as i32);
            }
            (gl.glBindTexture)(target, 0);
            let error = (gl.glGetError)();
            if error != sys::NO_ERROR {
                eprintln!("[hal-gl] texture {}x{}x{} format {} mips {}: GL error {error:#x}", desc.w, desc.h, desc.depth, format::name(desc.format), desc.mips);
            }
        }
        if let Some(old) = slot_for_insert(&mut self.textures, key).replace(GlTex { name, target, desc, owned: true }) { self.free_texture(old, key); }
    }

    fn free_texture(&mut self, t: GlTex, key: GlKey) {
        let gl = &self.ctx.gl;
        self.fbos.retain(|k, fbo| {
            let used = k.iter().any(|(tex, _)| *tex == key);
            if used { unsafe { (gl.glDeleteFramebuffers)(1, fbo) } }
            !used
        });
        if t.owned { unsafe { (gl.glDeleteTextures)(1, &t.name) } }
    }

    /// A 2D texture of the app's GL context (same share group) under `key`. It is sampled as the
    /// one level it was adopted with: its last level is set to 0, so the mip-filtered samplers
    /// find it complete.
    pub fn adopt_texture(&mut self, key: GlKey, name: GLuint, desc: TextureObj, owned: bool) {
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindTexture)(sys::TEXTURE_2D, name);
            (gl.glTexParameteri)(sys::TEXTURE_2D, sys::TEXTURE_MAX_LEVEL, 0);
            (gl.glBindTexture)(sys::TEXTURE_2D, 0);
        }
        if let Some(old) = slot_for_insert(&mut self.textures, key).replace(GlTex { name, target: sys::TEXTURE_2D, desc, owned }) { self.free_texture(old, key); }
    }

    /// The following GL commands wait on the GPU for the app context's fence, then it is deleted.
    pub fn wait_fence(&mut self, sync: u64) {
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glWaitSync)(sync as GLsync, 0, sys::TIMEOUT_IGNORED);
            (gl.glDeleteSync)(sync as GLsync);
        }
    }

    pub fn delete_texture(&mut self, key: GlKey) {
        if let Some(t) = self.textures.get_mut(key as usize).and_then(Option::take) { self.free_texture(t, key); }
        self.tex_recycled.lock().unwrap_or_else(|e| e.into_inner()).push(key);
    }

    /// `data`: tightly packed rows of `rect` (HAL layout); already validated by the HAL side.
    pub fn upload(&mut self, key: GlKey, mip: u32, rect: [u32; 4], layer: u32, data: Vec<u8>, swap_rb: bool) {
        let Some(t) = self.textures.get(key as usize).and_then(Option::as_ref) else { return };
        let gl = &self.ctx.gl;
        let (_, fmt, ty) = gl_format(t.desc.format);
        let data = match t.desc.format {
            format::BGRA8 if swap_rb => { let mut d = data; for p in d.chunks_exact_mut(4) { p.swap(0, 2); } d }
            // HAL: depth in the low 24 bits, stencil in the high byte; GL: depth << 8 | stencil
            format::DEPTH24_STENCIL8 => data.chunks_exact(4).flat_map(|p| {
                let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                ((v << 8) | (v >> 24)).to_le_bytes()
            }).collect(),
            _ => data,
        };
        let [x, y, w, h] = rect.map(|v| v as i32);
        unsafe {
            (gl.glBindTexture)(t.target, t.name);
            if format::is_block(t.desc.format) {
                if t.target == sys::TEXTURE_2D_ARRAY {
                    (gl.glCompressedTexSubImage3D)(t.target, mip as i32, x, y, layer as i32, w, h, 1, fmt, data.len() as i32, data.as_ptr() as _);
                } else {
                    (gl.glCompressedTexSubImage2D)(face(t.target, layer), mip as i32, x, y, w, h, fmt, data.len() as i32, data.as_ptr() as _);
                }
            } else if t.target == sys::TEXTURE_2D_ARRAY {
                (gl.glTexSubImage3D)(t.target, mip as i32, x, y, layer as i32, w, h, 1, fmt, ty, data.as_ptr() as _);
            } else {
                (gl.glTexSubImage2D)(face(t.target, layer), mip as i32, x, y, w, h, fmt, ty, data.as_ptr() as _);
            }
            (gl.glBindTexture)(t.target, 0);
        }
    }

    pub fn read_buffers(&mut self, token: Token, req: u32, ranges: &[(GlKey, u64, u64)]) {
        let result = (|| {
            let gl = &self.ctx.gl;
            let mut leases = Vec::with_capacity(ranges.len());
            for (order, (key, offset, len)) in ranges.iter().enumerate() {
                let source = self.buffers.get(*key as usize)?.as_ref()?;
                let mut bytes = Vec::new();
                bytes.try_reserve_exact(*len as usize).ok()?;
                bytes.resize(*len as usize, 0);
                if *len != 0 { unsafe {
                    (gl.glBindBuffer)(sys::ARRAY_BUFFER, source.name);
                    let ptr = (gl.glMapBufferRange)(sys::ARRAY_BUFFER, *offset as isize, *len as isize, sys::MAP_READ_BIT);
                    if ptr.is_null() { (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0); return None; }
                    // GLES has no GetBufferSubData: this is the driver's one read into the heap lease.
                    std::ptr::copy_nonoverlapping(ptr as *const u8, bytes.as_mut_ptr(), bytes.len());
                    let valid = (gl.glUnmapBuffer)(sys::ARRAY_BUFFER);
                    (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0);
                    if valid == 0 { return None; }
                } }
                leases.push(crate::lease::Lease::from_vec(bytes, order as u32));
            }
            Some(leases)
        })();
        let status = if result.is_some() { readback::OK } else { readback::FAILED };
        self.flush_upload_times();
        self.mailbox.push(token, Event::BufferLeases { req, status, leases: result.unwrap_or_default() });
    }

    pub fn create_buffer(&mut self, key: GlKey, kind: u16, size: u64) {
        let gl = &self.ctx.gl;
        let mut name = 0;
        unsafe {
            (gl.glGenBuffers)(1, &mut name);
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, name);
            (gl.glBufferData)(sys::ARRAY_BUFFER, size.max(4) as isize, std::ptr::null(), sys::DYNAMIC_DRAW);
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0);
        }
        let buf = GlBuf { name, size, append: kind & buffer_kind::APPEND != 0, words: Words::default(), words_dirty: Some((0, size)) };
        if let Some(old) = slot_for_insert(&mut self.buffers, key).replace(buf) { self.free_buffer(old); }
    }

    fn free_buffer(&mut self, b: GlBuf) {
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glDeleteBuffers)(1, &b.name);
            if b.words.tex != 0 { (gl.glDeleteTextures)(1, &b.words.tex) }
        }
    }

    pub fn delete_buffer(&mut self, key: GlKey) {
        if let Some(b) = self.buffers.get_mut(key as usize).and_then(Option::take) { self.free_buffer(b); }
        self.buf_recycled.lock().unwrap_or_else(|e| e.into_inner()).push(key);
    }

    pub fn write_buffer_parts(&mut self, key: GlKey, mut off: u64, bytes: &[u8], gaps: &[(usize, u64)]) -> u64 {
        let Some(b) = self.buffers.get_mut(key as usize).and_then(Option::as_mut) else { return 0 };
        let gl = &self.ctx.gl;
        let mut start = 0;
        for &(end, next_off) in gaps {
            Self::write_buffer(gl, b, off, &bytes[start..end]);
            start = end;
            off = next_off;
        }
        Self::write_buffer(gl, b, off, &bytes[start..]);
        gaps.len() as u64 + 1 // one glBufferSubData per run
    }

    fn write_buffer(gl: &sys::Gl, b: &mut GlBuf, off: u64, data: &[u8]) {
        unsafe {
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, b.name);
            if b.append && off == 0 {
                (gl.glBufferData)(sys::ARRAY_BUFFER, b.size.max(4) as isize, std::ptr::null(), sys::DYNAMIC_DRAW);
                b.words_dirty = None;
            }
            (gl.glBufferSubData)(sys::ARRAY_BUFFER, off as isize, data.len() as isize, data.as_ptr() as _);
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0);
        }
        let end = off + data.len() as u64;
        b.words_dirty = Some(match b.words_dirty {
            Some((start, stop)) => (start.min(off), stop.max(end)),
            None => (off, end),
        });
    }

    /// Refreshes the word texture's dirty rows from the unpack buffer. A new texture needs
    /// every row, including the unchanged prefix when the texture grows. Rows past one layer go
    /// to the next: a buffer of any size reads whole, up to the device's layer count.
    /// Pixel-unpack state is assumed at its defaults (`UNPACK_ROW_LENGTH`, `UNPACK_IMAGE_HEIGHT`
    /// and `UNPACK_SKIP_*` 0): any other uploader that changes it restores it.
    pub fn words_texture(gl: &sys::Gl, pools: &Arc<Pools>, max_layers: u32, buffer: GLuint, bytes: u64, mut dirty: (u64, u64), tex: &mut Words) {
        let row = u64::from(WORD_ROW);
        let row_bytes = row * 4;
        let words = bytes / 4;
        let rows = words.div_ceil(row).max(1);
        unsafe {
            if tex.tex == 0 || u64::from(tex.rows) < rows {
                if tex.tex != 0 { (gl.glDeleteTextures)(1, &tex.tex) }
                let rows = rows.next_power_of_two();
                let (height, mut layers) = if rows <= row { (rows, 1) } else { (row, rows / row) };
                if layers > 1 {
                    eprintln!("[hal-gl] instance words of {bytes} bytes span {layers} texture windows of {} words", row * row);
                }
                if layers > u64::from(max_layers) {
                    eprintln!("[hal-gl] instance words need {layers} texture windows and the device takes {max_layers}: words past {} read as zero", u64::from(max_layers) * row * row);
                    layers = u64::from(max_layers);
                }
                (gl.glGenTextures)(1, &mut tex.tex);
                (gl.glBindTexture)(sys::TEXTURE_2D_ARRAY, tex.tex);
                (gl.glTexStorage3D)(sys::TEXTURE_2D_ARRAY, 1, sys::R32UI, WORD_ROW as i32, height as i32, layers as i32);
                // it's sampled without a sampler object, and an integer texture is only complete unfiltered
                (gl.glTexParameteri)(sys::TEXTURE_2D_ARRAY, sys::TEXTURE_MIN_FILTER, sys::NEAREST as i32);
                (gl.glTexParameteri)(sys::TEXTURE_2D_ARRAY, sys::TEXTURE_MAG_FILTER, sys::NEAREST as i32);
                tex.rows = (height * layers) as u32;
                tex._charge = Some(pools.charge(Pool::WordTextures, row * height * layers * 4));
                dirty = (0, bytes);
            } else {
                (gl.glBindTexture)(sys::TEXTURE_2D_ARRAY, tex.tex);
            }
            (gl.glBindBuffer)(sys::PIXEL_UNPACK_BUFFER, buffer);
            let held = u64::from(tex.rows);
            let first = dirty.0 / row_bytes;
            let end = dirty.1.min(words * 4).div_ceil(row_bytes).min(held);
            let full_end = end.min(bytes / row_bytes);
            // whole rows, one transfer per layer they cross
            let mut at = first;
            while at < full_end {
                let stop = full_end.min((at / row + 1) * row);
                (gl.glTexSubImage3D)(sys::TEXTURE_2D_ARRAY, 0, 0, (at % row) as i32, (at / row) as i32, WORD_ROW as i32, (stop - at) as i32, 1,
                    sys::RED_INTEGER, sys::UNSIGNED_INT, (at * row_bytes) as usize as *const _);
                at = stop;
            }
            if first < end && full_end < end {
                // The unpack buffer can end partway through the texture's last row.
                let rest = (words % row) as i32;
                (gl.glTexSubImage3D)(sys::TEXTURE_2D_ARRAY, 0, 0, (full_end % row) as i32, (full_end / row) as i32, rest, 1, 1,
                    sys::RED_INTEGER, sys::UNSIGNED_INT, (full_end * row_bytes) as usize as *const _);
            }
            (gl.glBindBuffer)(sys::PIXEL_UNPACK_BUFFER, 0);
            (gl.glBindTexture)(sys::TEXTURE_2D_ARRAY, 0);
        }
    }

    /// The framebuffer of a pass's attachments ((texture key, layer) per colour slot, then depth).
    pub fn framebuffer(&mut self, key: [(GlKey, u32); 5]) -> Result<GLuint, String> {
        if let Some(fbo) = self.fbos.get(&key) { return Ok(*fbo); }
        let gl = &self.ctx.gl;
        let mut fbo = 0;
        unsafe {
            (gl.glGenFramebuffers)(1, &mut fbo);
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, fbo);
        }
        for (slot, (tex, layer)) in key.iter().enumerate() {
            if *tex == 0 { continue; }
            let t = self.textures.get(*tex as usize).and_then(Option::as_ref).ok_or("a pass attachment has no storage")?;
            let attachment = if slot < 4 { sys::COLOR_ATTACHMENT0 + slot as u32 }
                else if t.desc.format == format::DEPTH24_STENCIL8 { sys::DEPTH_STENCIL_ATTACHMENT } else { sys::DEPTH_ATTACHMENT };
            attach(gl, sys::FRAMEBUFFER, attachment, t, *layer);
        }
        let status = unsafe { (gl.glCheckFramebufferStatus)(sys::FRAMEBUFFER) };
        if status != sys::FRAMEBUFFER_COMPLETE {
            unsafe { (gl.glDeleteFramebuffers)(1, &fbo) };
            return Err(format!("incomplete framebuffer ({status:#x}); a float target needs GL_EXT_color_buffer_float"));
        }
        self.fbos.insert(key, fbo);
        Ok(fbo)
    }

    /// Binds `tex` (level 0, layer 0) for reading on the read framebuffer.
    fn bind_read(&self, tex: &GlTex) {
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindFramebuffer)(sys::READ_FRAMEBUFFER, self.read_fbo);
            attach(gl, sys::READ_FRAMEBUFFER, sys::COLOR_ATTACHMENT0, tex, 0);
            (gl.glReadBuffer)(sys::COLOR_ATTACHMENT0);
        }
    }

    /// Reads `rect` of level 0 into a pixel buffer; completes after the GPU did.
    pub fn read(&mut self, token: Token, key: GlKey, read: ReadFormat, req: u32, rect: [u32; 4]) {
        let Some(t) = self.textures.get(key as usize).and_then(Option::as_ref) else {
            self.mailbox.push(token, Event::Readback { req, rect, status: readback::FAILED, lease: None });
            return;
        };
        self.bind_read(t);
        let gl = &self.ctx.gl;
        let mut pbo = 0;
        unsafe {
            (gl.glGenBuffers)(1, &mut pbo);
            (gl.glBindBuffer)(sys::PIXEL_PACK_BUFFER, pbo);
            (gl.glBufferData)(sys::PIXEL_PACK_BUFFER, (rect[2] as usize * rect[3] as usize * read.texel) as isize, std::ptr::null(), sys::STREAM_READ);
            (gl.glReadPixels)(rect[0] as i32, rect[1] as i32, rect[2] as i32, rect[3] as i32, read.format, read.ty, std::ptr::null_mut());
            (gl.glBindBuffer)(sys::PIXEL_PACK_BUFFER, 0);
            (gl.glBindFramebuffer)(sys::READ_FRAMEBUFFER, 0);
        }
        let charge = self.pools.charge(Pool::Staging, rect[2] as u64 * rect[3] as u64 * read.texel as u64);
        self.after_gpu(Then::Read { token, req, rect, read, pbo, _charge: charge });
    }

    /// The host's synchronous read of a whole RGBA8/BGRA8 image as RGBA rows, top first.
    pub fn read_now(&mut self, key: GlKey, read: ReadFormat) -> Option<crate::backend::Pixels> {
        let t = self.textures.get(key as usize)?.as_ref()?;
        let (w, h) = (t.desc.w, t.desc.h);
        self.bind_read(t);
        let gl = &self.ctx.gl;
        let mut bytes = vec![0u8; w as usize * h as usize * 4];
        unsafe {
            (gl.glReadPixels)(0, 0, w as i32, h as i32, sys::RGBA, sys::UNSIGNED_BYTE, bytes.as_mut_ptr() as _);
            (gl.glBindFramebuffer)(sys::READ_FRAMEBUFFER, 0);
        }
        let (_, rgba) = read.convert(&bytes);
        Some(crate::backend::Pixels { w, h, rgba })
    }

    /// Copies a whole image into another of its size and format.
    pub fn copy(&mut self, token: Token, from: GlKey, to: GlKey, req: u32, rect: [u32; 4]) {
        if !self.blit(from, to, [rect[2], rect[3]]) {
            self.mailbox.push(token, Event::Readback { req, rect, status: readback::FAILED, lease: None });
            return;
        }
        self.after_gpu(Then::Copy { token, req, rect });
    }

    /// Copies the top-left `size` texels of image `from` into image `to` (level 0, layer 0 of
    /// both, which hold the same colour layout). False: one of them is gone.
    pub fn blit(&mut self, from: GlKey, to: GlKey, size: [u32; 2]) -> bool {
        let (Some(src), Some(dst)) = (self.textures.get(from as usize).and_then(Option::as_ref),
            self.textures.get(to as usize).and_then(Option::as_ref)) else { return false };
        self.bind_read(src);
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindFramebuffer)(sys::DRAW_FRAMEBUFFER, self.copy_fbo);
            attach(gl, sys::DRAW_FRAMEBUFFER, sys::COLOR_ATTACHMENT0, dst, 0);
            (gl.glDrawBuffers)(1, &sys::COLOR_ATTACHMENT0);
            (gl.glDisable)(sys::SCISSOR_TEST);
            let (w, h) = (size[0] as i32, size[1] as i32);
            (gl.glBlitFramebuffer)(0, 0, w, h, 0, 0, w, h, sys::COLOR_BUFFER_BIT, sys::NEAREST);
            // the copy framebuffer keeps no image: an attached one outlives its deletion (a
            // badge layer's composed image after the layer went)
            (gl.glFramebufferTexture2D)(sys::DRAW_FRAMEBUFFER, sys::COLOR_ATTACHMENT0, sys::TEXTURE_2D, 0, 0);
            (gl.glBindFramebuffer)(sys::READ_FRAMEBUFFER, 0);
            (gl.glBindFramebuffer)(sys::DRAW_FRAMEBUFFER, 0);
        }
        true
    }

    /// The window's image goes to its surface; `damage` is what changed since its last frame (`None`:
    /// all of it). Ok(false): nothing is on screen for it.
    pub fn present(&mut self, window: (u32, Id), key: GlKey, damage: Option<&[[i32; 4]]>) -> Result<bool, String> {
        let t = self.textures.get(key as usize).and_then(Option::as_ref).ok_or("the window image has no storage")?;
        let size = [t.desc.w, t.desc.h];
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, self.read_fbo);
            attach(gl, sys::FRAMEBUFFER, sys::COLOR_ATTACHMENT0, t, 0);
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, 0);
        }
        self.ctx.present(window, self.read_fbo, size, damage)
    }
}

/// The 1x1 texture of a sampled shape: transparent black colour, depth at the far plane; and
/// its bytes.
fn fallback(gl: &sys::Gl, shape: u32) -> (GLuint, u64) {
    let (target, layers) = match shape {
        1 => (sys::TEXTURE_CUBE_MAP, 6),
        2 | 4 => (sys::TEXTURE_2D_ARRAY, 2),
        _ => (sys::TEXTURE_2D, 1),
    };
    let depth = matches!(shape, 3 | 4);
    let mut name = 0;
    unsafe {
        (gl.glGenTextures)(1, &mut name);
        (gl.glBindTexture)(target, name);
        let (ifmt, fmt, ty) = if depth { (sys::DEPTH_COMPONENT32F, sys::DEPTH_COMPONENT, sys::FLOAT) } else { (sys::RGBA8, sys::RGBA, sys::UNSIGNED_BYTE) };
        let texel: [u8; 4] = if depth { 1f32.to_le_bytes() } else { [0; 4] };
        if target == sys::TEXTURE_2D_ARRAY {
            (gl.glTexStorage3D)(target, 1, ifmt, 1, 1, layers);
            for layer in 0..layers { (gl.glTexSubImage3D)(target, 0, 0, 0, layer, 1, 1, 1, fmt, ty, texel.as_ptr() as _) }
        } else {
            (gl.glTexStorage2D)(target, 1, ifmt, 1, 1);
            for layer in 0..layers as u32 { (gl.glTexSubImage2D)(face(target, layer), 0, 0, 0, 1, 1, fmt, ty, texel.as_ptr() as _) }
        }
        (gl.glBindTexture)(target, 0);
    }
    (name, 4 * layers as u64)
}

fn attach(gl: &sys::Gl, fb: GLenum, attachment: GLenum, t: &GlTex, layer: u32) {
    unsafe {
        if t.target == sys::TEXTURE_2D_ARRAY {
            (gl.glFramebufferTextureLayer)(fb, attachment, t.name, 0, layer as i32);
        } else {
            (gl.glFramebufferTexture2D)(fb, attachment, face(t.target, layer), t.name, 0);
        }
    }
}

/// The 2D target of a cube face, or the texture's own target.
pub(super) fn face(target: GLenum, layer: u32) -> GLenum {
    if target == sys::TEXTURE_CUBE_MAP { sys::TEXTURE_CUBE_MAP_POSITIVE_X + layer } else { target }
}

/// A cube ([`TextureObj::is_cube`]); other layered textures, and every texture created with
/// [`usage::ARRAY`] (a one-layer array included), are arrays.
pub(super) fn texture_target(t: &TextureObj) -> GLenum {
    if t.is_cube() { sys::TEXTURE_CUBE_MAP }
    else if t.depth > 1 || t.usage & usage::ARRAY != 0 { sys::TEXTURE_2D_ARRAY }
    else { sys::TEXTURE_2D }
}

/// The GL target a sampled shape needs (pipeline texture shapes: 0 2D, 1 cube, 2 array,
/// 3 depth, 4 depth array).
pub(super) fn shape_target(shape: u32) -> GLenum {
    match shape { 1 => sys::TEXTURE_CUBE_MAP, 2 | 4 => sys::TEXTURE_2D_ARRAY, _ => sys::TEXTURE_2D }
}

/// The wrap mode of a sampler address (0 repeat, 1 edge, 2 transparent border, 3 mirrored repeat).
pub(super) fn wrap(address: u32, border_clamp: bool) -> GLenum {
    match address {
        0 => sys::REPEAT,
        2 if border_clamp => sys::CLAMP_TO_BORDER,
        3 => sys::MIRRORED_REPEAT,
        _ => sys::CLAMP_TO_EDGE,
    }
}

/// Whether a filtering sampler keeps a texture of `format` complete. GLES 3 never filters integers,
/// filters 32-bit floats only with OES_texture_float_linear, and depth only when it compares.
pub(super) fn filterable(caps: &Caps, format: u32, compare: bool) -> bool {
    match format {
        format::RGBA32F | format::R32F => caps.float_linear,
        format::R32UINT => false,
        f => compare || !format::is_depth(f),
    }
}

/// (internal format, upload format, upload type). BGRA8 is stored as RGBA8: a texture that is
/// only uploaded keeps the uploaded byte order and swizzles R/B when sampled, a render target
/// holds the colours the shaders write.
pub(super) fn gl_format(f: u32) -> (GLenum, GLenum, GLenum) {
    match f {
        format::RGBA8 | format::BGRA8 => (sys::RGBA8, sys::RGBA, sys::UNSIGNED_BYTE),
        format::R8 => (sys::R8, sys::RED, sys::UNSIGNED_BYTE),
        format::RG8 => (sys::RG8, sys::RG, sys::UNSIGNED_BYTE),
        format::RGBA16F => (sys::RGBA16F, sys::RGBA, sys::HALF_FLOAT),
        format::RGBA32F => (sys::RGBA32F, sys::RGBA, sys::FLOAT),
        format::DEPTH32F => (sys::DEPTH_COMPONENT32F, sys::DEPTH_COMPONENT, sys::FLOAT),
        format::DEPTH24_STENCIL8 => (sys::DEPTH24_STENCIL8, sys::DEPTH_STENCIL, sys::UNSIGNED_INT_24_8),
        format::R32F => (sys::R32F, sys::RED, sys::FLOAT),
        format::RG16F => (sys::RG16F, sys::RG, sys::HALF_FLOAT),
        format::R16F => (sys::R16F, sys::RED, sys::HALF_FLOAT),
        format::R32UINT => (sys::R32UI, sys::RED_INTEGER, sys::UNSIGNED_INT),
        format::BC7 => (sys::COMPRESSED_RGBA_BPTC_UNORM, sys::COMPRESSED_RGBA_BPTC_UNORM, 0),
        format::BC5 => (sys::COMPRESSED_RED_GREEN_RGTC2, sys::COMPRESSED_RED_GREEN_RGTC2, 0),
        _ => (sys::COMPRESSED_RGBA_ASTC_4X4, sys::COMPRESSED_RGBA_ASTC_4X4, 0),
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let gl = &self.ctx.gl;
        unsafe {
            for (fence, _) in self.pending.drain(..) { (gl.glDeleteSync)(fence) }
            (gl.glFinish)();
            if self.zero_uniform.0 != 0 { (gl.glDeleteBuffers)(1, &self.zero_uniform.0); }
        }
        if let Some(timer) = &mut self.timer { timer.delete(gl); }
    }
}

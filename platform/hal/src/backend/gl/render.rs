//! The render thread. It owns the GLES context and every GL object, keyed by the HAL side's
//! object keys, and runs the jobs the HAL's Gpu hooks queue in submission order (GL orders a
//! write after the reads queued before it, so no lease outlives a job). Completion is a fence
//! per frame or readback, polled between jobs; results go to the mailbox the HAL drains.

use super::egl::Context;
use super::surface::EglSurfaceSource;
use super::sys::{self, GLenum, GLsync, GLuint};
use super::{PipeState, TexObj};
use crate::font::FrameProgress;
use crate::session::TextureObj;
use crate::types::*;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

pub(super) type Job = Box<dyn FnOnce(&mut Renderer) + Send>;

/// The HAL side's handle to the render thread.
#[derive(Clone)]
pub(super) struct Queue(mpsc::Sender<Job>);

impl Queue {
    pub fn push(&self, job: impl FnOnce(&mut Renderer) + Send + 'static) {
        // after the backend is gone the context and all its objects are gone with the thread
        let _ = self.0.send(Box::new(job));
    }
}

/// What the context can do, read once on the render thread.
pub(super) struct Caps {
    pub formats: u64,
    pub max_texture: u32,
    pub max_layers: u32,
    pub ubo_align: usize,
    pub max_ubo: usize,
    pub border_clamp: bool,
    pub float_blend: bool,
    pub renderer: String,
    /// a software rasterizer: the default selection prefers hardware Vulkan and GL to it
    pub software: bool,
    /// the EGL display, config and context of the share group (the `GlShare` event)
    pub share: [u64; 3],
}

#[derive(Clone)]
pub(super) struct Token {
    pub sid: u32,
    pub incarnation: Arc<FrameProgress>,
}

pub(super) enum Event {
    Frame { target: Id, kind: u32, frame: u64, window: bool },
    Readback { req: u32, status: u32, rect: [u32; 4], pixels: Vec<u8> },
    Pick { req: u32, ids: Vec<u32> },
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
            format::RGBA16F | format::RGBA32F | format::R32F | format::RG16F => (sys::RGBA, sys::FLOAT, 16),
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
    Frame { token: Token, target: Id, kind: u32, frame: u64, window: bool },
    Read { token: Token, req: u32, rect: [u32; 4], read: ReadFormat, pbo: GLuint, pick: Option<[f32; 2]> },
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
    /// the words as an R32UI texture, for programs that fetch instances by id (made on use)
    pub words: Option<(GLuint, u32)>,
    pub words_stale: bool,
}

/// The size of the instance word textures' rows (GLES 3 guarantees 2048 texels).
pub(super) const WORD_ROW: u32 = 2048;

pub(super) struct Renderer {
    pub ctx: Context,
    pub caps: Arc<Caps>,
    pub textures: HashMap<u64, GlTex>,
    pub buffers: HashMap<u64, GlBuf>,
    pub programs: HashMap<u64, super::program::GlProgram>,
    pub linking: Vec<(Weak<PipeState>, super::program::Linking)>,
    pub fbos: HashMap<[(u64, u32); 5], GLuint>,
    pub read_fbo: GLuint,
    pub copy_fbo: GLuint,
    /// 1x1 textures per sampled shape for a draw that names no texture
    pub fallback: [GLuint; 5],
    /// the frame's uniform blocks and inline instances, and the instance words as a texture
    pub uniform_arena: GLuint,
    pub instance_arena: GLuint,
    pub arena_words: (GLuint, u32),
    pending: VecDeque<(GLsync, Then)>,
    pub mailbox: Mailbox,
    exit: bool,
}

pub(super) fn start(source: Box<dyn EglSurfaceSource>, mailbox: Mailbox) -> Result<(Queue, Arc<Caps>, std::thread::JoinHandle<()>), String> {
    let (tx, rx) = mpsc::channel::<Job>();
    let (ready, made) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new().name("makepad-gl-render".into()).spawn(move || {
        match Renderer::new(source, mailbox) {
            Ok(r) => {
                let _ = ready.send(Ok(r.caps.clone()));
                r.run(rx);
            }
            Err(e) => { let _ = ready.send(Err(e)); }
        }
    }).map_err(|e| format!("render thread: {e}"))?;
    match made.recv() {
        Ok(Ok(caps)) => Ok((Queue(tx), caps, thread)),
        Ok(Err(e)) => { let _ = thread.join(); Err(e) }
        Err(_) => { let _ = thread.join(); Err("the GL render thread ended during startup".into()) }
    }
}

impl Renderer {
    fn new(source: Box<dyn EglSurfaceSource>, mailbox: Mailbox) -> Result<Self, String> {
        let ctx = Context::new(source)?;
        let gl = &ctx.gl;
        let version = gl.string(sys::VERSION);
        if !version.starts_with("OpenGL ES 3") {
            return Err(format!("a GLES 3 context is required, the driver made \"{version}\""));
        }
        let ext = gl.extensions();
        let has = |name: &str| ext.iter().any(|e| e == name);
        let mut formats = [format::RGBA8, format::BGRA8, format::R8, format::RG8, format::RGBA16F, format::RGBA32F,
            format::DEPTH32F, format::DEPTH24_STENCIL8, format::R32F, format::RG16F, format::R32UINT].iter().fold(0u64, |b, f| b | 1 << f);
        if has("GL_EXT_texture_compression_bptc") { formats |= 1 << format::BC7; }
        if has("GL_EXT_texture_compression_rgtc") { formats |= 1 << format::BC5; }
        if has("GL_KHR_texture_compression_astc_ldr") { formats |= 1 << format::ASTC4X4; }
        if let Some(threads) = gl.glMaxShaderCompilerThreadsKHR {
            if has("GL_KHR_parallel_shader_compile") { unsafe { threads(u32::MAX) } }
        }
        let caps = Arc::new(Caps {
            formats,
            max_texture: gl.integer(sys::MAX_TEXTURE_SIZE).min(gl.integer(sys::MAX_RENDERBUFFER_SIZE)).max(2048) as u32,
            max_layers: gl.integer(sys::MAX_ARRAY_TEXTURE_LAYERS).max(256) as u32,
            ubo_align: gl.integer(sys::UNIFORM_BUFFER_OFFSET_ALIGNMENT).max(16) as usize,
            max_ubo: gl.integer(sys::MAX_UNIFORM_BLOCK_SIZE).max(16384) as usize,
            border_clamp: has("GL_EXT_texture_border_clamp") || has("GL_OES_texture_border_clamp"),
            float_blend: has("GL_EXT_float_blend"),
            renderer: format!("{} ({version})", gl.string(sys::RENDERER)),
            software: ctx.software(),
            share: [ctx.share.display as u64, ctx.share.config as u64, ctx.share.context as u64],
        });
        let mut names = [0; 4];
        unsafe {
            (gl.glGenFramebuffers)(2, names.as_mut_ptr());
            (gl.glGenBuffers)(2, names[2..].as_mut_ptr());
            (gl.glPixelStorei)(sys::UNPACK_ALIGNMENT, 1);
            (gl.glPixelStorei)(sys::PACK_ALIGNMENT, 1);
            (gl.glDepthFunc)(sys::LEQUAL);
            // every vertex shader of this dialect flips clip-space Y (images keep the top row
            // first, as on the other backends), which reverses the winding
            (gl.glFrontFace)(sys::CW);
            (gl.glCullFace)(sys::BACK);
            (gl.glBlendFuncSeparate)(sys::ONE, sys::ONE_MINUS_SRC_ALPHA, sys::ONE, sys::ONE_MINUS_SRC_ALPHA);
        }
        let fallback = [0, 1, 2, 3, 4].map(|shape| fallback(gl, shape));
        if super::super::log_enabled() {
            eprintln!("[hal-gl] context: {} ubo_align={} extensions={}", caps.renderer, caps.ubo_align, ext.len());
        }
        Ok(Self { ctx, caps, textures: HashMap::new(), buffers: HashMap::new(), programs: HashMap::new(), linking: Vec::new(),
            fbos: HashMap::new(), read_fbo: names[0], copy_fbo: names[1], fallback,
            uniform_arena: names[2], instance_arena: names[3], arena_words: (0, 0), pending: VecDeque::new(), mailbox, exit: false })
    }

    pub fn exit(&mut self) {
        self.exit = true;
    }

    fn run(mut self, rx: Receiver<Job>) {
        loop {
            let next = match rx.try_recv() {
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
            Then::Frame { token, target, kind, frame, window } => {
                token.incarnation.done(frame);
                self.mailbox.push(token, Event::Frame { target, kind, frame, window });
            }
            Then::Copy { token, req, rect } => {
                self.mailbox.push(token, Event::Readback { req, rect, status: if failed { readback::FAILED } else { readback::OK }, pixels: Vec::new() });
            }
            Then::Read { token, req, rect, read, pbo, pick } => {
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
                    if pick.is_some() { Event::Pick { req, ids: Vec::new() } } else { Event::Readback { req, rect, status: readback::FAILED, pixels: Vec::new() } }
                } else {
                    let (status, pixels) = read.convert(&bytes);
                    match pick {
                        Some(pos) => Event::Pick { req, ids: pick_ids(&pixels, rect, pos) },
                        None => Event::Readback { req, rect, status, pixels },
                    }
                };
                self.mailbox.push(token, event);
            }
        }
    }

    // ---- objects

    pub fn create_texture(&mut self, key: u64, desc: TextureObj, swizzled: bool) {
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
        if let Some(old) = self.textures.insert(key, GlTex { name, target, desc, owned: true }) { self.free_texture(old, key); }
    }

    fn free_texture(&mut self, t: GlTex, key: u64) {
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
    pub fn adopt_texture(&mut self, key: u64, name: GLuint, desc: TextureObj, owned: bool) {
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindTexture)(sys::TEXTURE_2D, name);
            (gl.glTexParameteri)(sys::TEXTURE_2D, sys::TEXTURE_MAX_LEVEL, 0);
            (gl.glBindTexture)(sys::TEXTURE_2D, 0);
        }
        if let Some(old) = self.textures.insert(key, GlTex { name, target: sys::TEXTURE_2D, desc, owned }) { self.free_texture(old, key); }
    }

    /// The following GL commands wait on the GPU for the app context's fence, then it is deleted.
    pub fn wait_fence(&mut self, sync: u64) {
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glWaitSync)(sync as GLsync, 0, sys::TIMEOUT_IGNORED);
            (gl.glDeleteSync)(sync as GLsync);
        }
    }

    pub fn delete_texture(&mut self, key: u64) {
        if let Some(t) = self.textures.remove(&key) { self.free_texture(t, key); }
    }

    /// `data`: tightly packed rows of `rect` (HAL layout); already validated by the HAL side.
    pub fn upload(&mut self, key: u64, mip: u32, rect: [u32; 4], layer: u32, data: Vec<u8>, swap_rb: bool) {
        let Some(t) = self.textures.get(&key) else { return };
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

    pub fn create_buffer(&mut self, key: u64, size: u64) {
        let gl = &self.ctx.gl;
        let mut name = 0;
        unsafe {
            (gl.glGenBuffers)(1, &mut name);
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, name);
            (gl.glBufferData)(sys::ARRAY_BUFFER, size.max(4) as isize, std::ptr::null(), sys::DYNAMIC_DRAW);
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0);
        }
        if let Some(old) = self.buffers.insert(key, GlBuf { name, size, words: None, words_stale: true }) { self.free_buffer(old); }
    }

    fn free_buffer(&mut self, b: GlBuf) {
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glDeleteBuffers)(1, &b.name);
            if let Some((tex, _)) = b.words { (gl.glDeleteTextures)(1, &tex) }
        }
    }

    pub fn delete_buffer(&mut self, key: u64) {
        if let Some(b) = self.buffers.remove(&key) { self.free_buffer(b); }
    }

    pub fn write_buffer(&mut self, key: u64, off: u64, data: Vec<u8>) {
        let Some(b) = self.buffers.get_mut(&key) else { return };
        b.words_stale = true;
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, b.name);
            (gl.glBufferSubData)(sys::ARRAY_BUFFER, off as isize, data.len() as isize, data.as_ptr() as _);
            (gl.glBindBuffer)(sys::ARRAY_BUFFER, 0);
        }
    }

    /// Copies `words` 32-bit words of a GL buffer into an R32UI texture of WORD_ROW-wide rows
    /// (a GPU copy through the unpack binding); grows the texture when it is too small.
    pub fn words_texture(gl: &sys::Gl, buffer: GLuint, words: u64, tex: &mut (GLuint, u32)) {
        let rows = (words.div_ceil(WORD_ROW as u64) as u32).max(1);
        unsafe {
            if tex.0 == 0 || tex.1 < rows {
                if tex.0 != 0 { (gl.glDeleteTextures)(1, &tex.0) }
                let rows = rows.next_power_of_two();
                (gl.glGenTextures)(1, &mut tex.0);
                (gl.glBindTexture)(sys::TEXTURE_2D, tex.0);
                (gl.glTexStorage2D)(sys::TEXTURE_2D, 1, sys::R32UI, WORD_ROW as i32, rows as i32);
                *tex = (tex.0, rows);
            } else {
                (gl.glBindTexture)(sys::TEXTURE_2D, tex.0);
            }
            (gl.glBindBuffer)(sys::PIXEL_UNPACK_BUFFER, buffer);
            let full = (words / WORD_ROW as u64) as i32;
            if full > 0 {
                (gl.glTexSubImage2D)(sys::TEXTURE_2D, 0, 0, 0, WORD_ROW as i32, full, sys::RED_INTEGER, sys::UNSIGNED_INT, std::ptr::null());
            }
            let rest = (words % WORD_ROW as u64) as i32;
            if rest > 0 {
                let at = full as usize * WORD_ROW as usize * 4;
                (gl.glTexSubImage2D)(sys::TEXTURE_2D, 0, 0, full, rest, 1, sys::RED_INTEGER, sys::UNSIGNED_INT, at as *const _);
            }
            (gl.glBindBuffer)(sys::PIXEL_UNPACK_BUFFER, 0);
            (gl.glBindTexture)(sys::TEXTURE_2D, 0);
        }
    }

    /// The framebuffer of a pass's attachments ((texture key, layer) per colour slot, then depth).
    pub fn framebuffer(&mut self, key: [(u64, u32); 5]) -> Result<GLuint, String> {
        if let Some(fbo) = self.fbos.get(&key) { return Ok(*fbo); }
        let gl = &self.ctx.gl;
        let mut fbo = 0;
        unsafe {
            (gl.glGenFramebuffers)(1, &mut fbo);
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, fbo);
        }
        for (slot, (tex, layer)) in key.iter().enumerate() {
            if *tex == 0 { continue; }
            let t = self.textures.get(tex).ok_or("a pass attachment has no storage")?;
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
    pub fn read(&mut self, token: Token, key: u64, read: ReadFormat, req: u32, rect: [u32; 4], pick: Option<[f32; 2]>) {
        let Some(t) = self.textures.get(&key) else {
            self.mailbox.push(token, if pick.is_some() { Event::Pick { req, ids: Vec::new() } } else { Event::Readback { req, rect, status: readback::FAILED, pixels: Vec::new() } });
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
        self.after_gpu(Then::Read { token, req, rect, read, pbo, pick });
    }

    /// The host's synchronous read of a whole RGBA8/BGRA8 image as RGBA rows, top first.
    pub fn read_now(&mut self, key: u64, read: ReadFormat) -> Option<crate::backend::Pixels> {
        let t = self.textures.get(&key)?;
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
    pub fn copy(&mut self, token: Token, from: u64, to: u64, req: u32, rect: [u32; 4]) {
        let (Some(src), Some(dst)) = (self.textures.get(&from), self.textures.get(&to)) else {
            self.mailbox.push(token, Event::Readback { req, rect, status: readback::FAILED, pixels: Vec::new() });
            return;
        };
        self.bind_read(src);
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindFramebuffer)(sys::DRAW_FRAMEBUFFER, self.copy_fbo);
            attach(gl, sys::DRAW_FRAMEBUFFER, sys::COLOR_ATTACHMENT0, dst, 0);
            (gl.glDrawBuffers)(1, &sys::COLOR_ATTACHMENT0);
            (gl.glDisable)(sys::SCISSOR_TEST);
            let (w, h) = (rect[2] as i32, rect[3] as i32);
            (gl.glBlitFramebuffer)(0, 0, w, h, 0, 0, w, h, sys::COLOR_BUFFER_BIT, sys::NEAREST);
            (gl.glBindFramebuffer)(sys::READ_FRAMEBUFFER, 0);
            (gl.glBindFramebuffer)(sys::DRAW_FRAMEBUFFER, 0);
        }
        self.after_gpu(Then::Copy { token, req, rect });
    }

    /// The window's image goes to its surface. Ok(false): nothing is on screen for it.
    pub fn present(&mut self, window: (u32, Id), key: u64) -> Result<bool, String> {
        let t = self.textures.get(&key).ok_or("the window image has no storage")?;
        let size = [t.desc.w, t.desc.h];
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, self.read_fbo);
            attach(gl, sys::FRAMEBUFFER, sys::COLOR_ATTACHMENT0, t, 0);
            (gl.glBindFramebuffer)(sys::FRAMEBUFFER, 0);
        }
        self.ctx.present(window, self.read_fbo, size)
    }
}

/// The ids a pick readback found, nearest to `pos` first (as Metal, Vulkan and D3D11 order them).
fn pick_ids(rgba: &[u8], rect: [u32; 4], pos: [f32; 2]) -> Vec<u32> {
    let mut ordered: Vec<_> = rgba.chunks_exact(4).enumerate().map(|(i, px)| {
        let x = rect[0] as f32 + (i as u32 % rect[2]) as f32;
        let y = rect[1] as f32 + (i as u32 / rect[2]) as f32;
        ((x - pos[0]).powi(2) + (y - pos[1]).powi(2), u32::from_le_bytes(px.try_into().unwrap()) & 0x00ff_ffff)
    }).collect();
    ordered.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut ids = Vec::new();
    for (_, id) in ordered {
        if id != 0 && !ids.contains(&id) { ids.push(id); }
    }
    ids
}

/// The 1x1 texture of a sampled shape: transparent black colour, depth at the far plane.
fn fallback(gl: &sys::Gl, shape: u32) -> GLuint {
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
    name
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

/// Six square colour layers are a cube; other layered textures are arrays.
pub(super) fn texture_target(t: &TextureObj) -> GLenum {
    if t.depth == 1 { sys::TEXTURE_2D }
    else if t.depth == 6 && t.w == t.h && !format::is_depth(t.format) { sys::TEXTURE_CUBE_MAP }
    else { sys::TEXTURE_2D_ARRAY }
}

/// The GL target a sampled shape needs (pipeline texture shapes: 0 2D, 1 cube, 2 array,
/// 3 depth, 4 depth array).
pub(super) fn shape_target(shape: u32) -> GLenum {
    match shape { 1 => sys::TEXTURE_CUBE_MAP, 2 | 4 => sys::TEXTURE_2D_ARRAY, _ => sys::TEXTURE_2D }
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
        }
    }
}

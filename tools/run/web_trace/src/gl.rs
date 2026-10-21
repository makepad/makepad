//! The page's WebGL side (web_gl.js), recorded instead of drawn: every resource and draw call the
//! program asks for is kept as what it would have uploaded and drawn, and each frame gets a digest
//! of its GL work (the values and the bytes behind every pointer, never the pointers): the same
//! program in a browser, run through the same frames, makes the same digests (the export's
//! correctness check compares them).
//!
//! What the program reads back is answered as a WebGL 2 context with every program linking at
//! once (no `parallel_shader_compile`): `ToWasmWebGLShadersDone` per compile, or
//! `ToWasmWebGLShadersLinked` and the Done at its first draw while first draws are spread;
//! a completion poll succeeds at once; a render-texture capture answers zeroed pixels.

use crate::imports::read_bytes;
use makepad_platform::web_protocol::from_wasm::*;
use makepad_wasm_bridge::{WasmPtrF32, WasmPtrU32, WasmPtrU8};

/// FNV-1a, 64-bit: stable across runs and machines.
#[derive(Clone, Copy)]
pub struct Digest(pub u64);
impl Digest {
    pub fn new() -> Self {
        Digest(0xcbf2_9ce4_8422_2325)
    }
    pub fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= *x as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }
}

fn f32s(p: &WasmPtrF32) -> Vec<u8> {
    read_bytes(p.ptr, (p.len * 4) as u32)
}
fn u32s(p: &WasmPtrU32) -> Vec<u8> {
    read_bytes(p.ptr, (p.len * 4) as u32)
}
fn u8s(p: &WasmPtrU8) -> Vec<u8> {
    read_bytes(p.ptr, p.len as u32)
}

#[derive(Default)]
pub struct Shader {
    pub first_draw_waiting: bool,
}

/// One frame's GL work.
#[derive(Clone)]
pub struct Frame {
    pub draw_calls: u32,
    pub passes: u32,
    pub upload_bytes: u64,
    pub digest: u64,
    /// GPU bytes of the live textures and render targets at the frame's end (a leak grows it).
    pub texture_bytes: u64,
}

pub struct Gl {
    pub shaders: Vec<Option<Shader>>,
    pub first_draw_spread: bool,
    pub first_draw_this_frame: bool,
    pub first_draws_deferred: bool,
    /// the frame being recorded
    frame: Frame,
    hash: Digest,
    pub frames: Vec<Frame>,
    /// totals over the run
    pub shaders_compiled: u32,
    /// Every program's GLSL the page compiled (vertex, fragment): what a shader pack must hold.
    pub glsl: std::collections::HashSet<(String, String)>,
    pub texture_bytes: u64,
    pub buffer_bytes: u64,
    /// Live textures and render targets: id -> GPU bytes (an upload's, or a target's size times
    /// its format's), until freed.
    pub live_textures: std::collections::HashMap<usize, u64>,
    pub peak_texture_bytes: u64,
}

/// What the page sends back for a GL message.
pub enum Reply {
    ShadersDone { count: usize, waited: usize },
    ShadersLinked { count: usize },
    GpuCompletion { serial_lo: u32, serial_hi: u32, success: bool },
    Capture { texture_id: usize, ticket_lo: u32, ticket_hi: u32, width: usize, height: usize },
}

impl Gl {
    pub fn new() -> Self {
        Gl {
            glsl: Default::default(),
            shaders: Vec::new(),
            first_draw_spread: false,
            first_draw_this_frame: false,
            first_draws_deferred: false,
            frame: Frame { draw_calls: 0, passes: 0, upload_bytes: 0, digest: 0, texture_bytes: 0 },
            hash: Digest::new(),
            frames: Vec::new(),
            shaders_compiled: 0,
            texture_bytes: 0,
            live_textures: Default::default(),
            peak_texture_bytes: 0,
            buffer_bytes: 0,
        }
    }

    fn upload(&mut self, kind: u64, id: usize, bytes: &[u8]) {
        self.hash.u64(kind);
        self.hash.u64(id as u64);
        self.hash.u64(bytes.len() as u64);
        self.hash.bytes(bytes);
        self.frame.upload_bytes += bytes.len() as u64;
    }

    /// The frame's work so far ends (the page's animation frame returned).
    pub fn end_frame(&mut self) {
        self.frame.digest = self.hash.0;
        self.frame.texture_bytes = self.live_textures.values().sum();
        self.peak_texture_bytes = self.peak_texture_bytes.max(self.frame.texture_bytes);
        if self.frame.draw_calls != 0 || self.frame.upload_bytes != 0 || self.frame.passes != 0 {
            self.frames.push(self.frame.clone());
        }
        self.frame = Frame { draw_calls: 0, passes: 0, upload_bytes: 0, digest: 0, texture_bytes: 0 };
        self.hash = Digest::new();
        self.first_draw_this_frame = false;
    }

    pub fn spread_first_draws(&mut self, m: FromWasmSpreadFirstDraws, out: &mut Vec<Reply>) -> bool {
        self.first_draw_spread = m.on;
        if m.on {
            return false;
        }
        let mut released = 0;
        for s in self.shaders.iter_mut().flatten() {
            if s.first_draw_waiting {
                s.first_draw_waiting = false;
                released += 1;
            }
        }
        if released != 0 {
            out.push(Reply::ShadersDone { count: released, waited: released });
            return true;
        }
        false
    }

    pub fn compile(&mut self, m: FromWasmCompileWebGLShader, out: &mut Vec<Reply>) {
        self.hash.u64(1);
        self.hash.u64(m.shader_id as u64);
        self.hash.str(&m.vertex);
        self.hash.str(&m.pixel);
        if self.shaders.len() <= m.shader_id {
            self.shaders.resize_with(m.shader_id + 1, || None);
        }
        let waiting = self.first_draw_spread;
        self.shaders[m.shader_id] = Some(Shader { first_draw_waiting: waiting });
        self.shaders_compiled += 1;
        self.glsl.insert((m.vertex.clone(), m.pixel.clone()));
        out.push(if waiting { Reply::ShadersLinked { count: 1 } } else { Reply::ShadersDone { count: 1, waited: 0 } });
    }

    pub fn array_buffer(&mut self, m: FromWasmAllocArrayBuffer) {
        let mut b = f32s(&m.data);
        b.extend(u8s(&m.byte_data));
        self.buffer_bytes += b.len() as u64;
        self.upload(2, m.buffer_id, &b);
    }
    pub fn retained_buffer(&mut self, m: FromWasmRetainedArrayBuffer) {
        let b = f32s(&m.data);
        self.buffer_bytes += b.len() as u64;
        self.hash.u64(m.first_slot as u64);
        self.upload(3, m.buffer_id, &b);
    }
    pub fn retained_update(&mut self, m: FromWasmRetainedArrayUpdate) {
        self.hash.u64(4);
        self.hash.u64(m.buffer_id as u64);
        self.hash.u64(m.slot_count as u64);
        self.hash.u64(m.capacity_bytes as u64);
        self.hash.u64(m.replace as u64);
        for c in &m.copies {
            self.hash.u64(c.source_slot as u64);
            self.hash.u64(c.destination_slot as u64);
            self.hash.u64(c.slot_count as u64);
        }
        for w in &m.writes {
            let b = f32s(&w.data);
            self.buffer_bytes += b.len() as u64;
            self.upload(5, w.destination_slot, &b);
        }
    }
    pub fn index_buffer(&mut self, m: FromWasmAllocIndexBuffer) {
        let mut b = u32s(&m.data);
        b.extend(u8s(&m.byte_data));
        self.buffer_bytes += b.len() as u64;
        self.hash.u64(m.index_width as u64);
        self.upload(6, m.buffer_id, &b);
    }
    pub fn vao(&mut self, m: FromWasmAllocVao) {
        self.hash.u64(7);
        for v in [m.vao_id, m.shader_id, m.geom_ib_id, m.geom_vb_id, m.inst_vb_id] {
            self.hash.u64(v as u64);
        }
    }
    pub fn free(&mut self, m: FromWasmFreeWebGLResources) {
        self.hash.u64(8);
        for id in &m.texture_ids {
            self.live_textures.remove(id);
        }
        for l in [&m.array_buffer_ids, &m.index_buffer_ids, &m.vao_ids, &m.texture_ids, &m.framebuffer_ids] {
            self.hash.u64(l.len() as u64);
            for v in l {
                self.hash.u64(*v as u64);
            }
        }
    }
    pub fn texture(&mut self, kind: u64, id: usize, w: usize, h: usize, bytes: Vec<u8>) {
        self.hash.u64(w as u64);
        self.hash.u64(h as u64);
        self.texture_bytes += bytes.len() as u64;
        self.live_textures.insert(id, bytes.len() as u64);
        self.upload(kind, id, &bytes);
    }
    pub fn texture_bgra(&mut self, m: FromWasmAllocTextureImage2D_BGRAu8_32) {
        let b = u32s(&m.data);
        self.texture(9, m.texture_id, m.width, m.height, b)
    }
    pub fn texture_r8(&mut self, m: FromWasmAllocTextureImage2D_Ru8) {
        let b = u8s(&m.data);
        self.texture(10, m.texture_id, m.width, m.height, b)
    }
    pub fn texture_rgbaf32(&mut self, m: FromWasmAllocTextureImage2D_RGBAf32) {
        let b = f32s(&m.data);
        self.texture(11, m.texture_id, m.width, m.height, b)
    }
    pub fn texture_cube(&mut self, m: FromWasmAllocTextureCube_BGRAu8_32) {
        let b = u32s(&m.data);
        self.texture(12, m.texture_id, m.width, m.height, b)
    }
    pub fn begin_texture(&mut self, m: FromWasmBeginRenderTexture) {
        self.frame.passes += 1;
        self.hash.u64(13);
        self.hash.u64(m.pass_id as u64);
        self.hash.u64(m.width as u64);
        self.hash.u64(m.height as u64);
        for c in &m.color_targets {
            // 0 RGBA8, 1 R32F, 2 RGBA32F, 3 RGBA16F
            let bpp = match c.format { 2 => 16, 3 => 8, _ => 4 };
            self.live_textures.insert(c.texture_id, m.width as u64 * m.height as u64 * bpp);
            self.hash.u64(c.texture_id as u64);
            self.hash.u64(c.init_only as u64);
            self.hash.u64(c.format as u64);
            for f in [c.clear_color.r, c.clear_color.g, c.clear_color.b, c.clear_color.a] {
                self.hash.u64(f.to_bits() as u64);
            }
        }
        let d = &m.depth_target;
        if d.attached {
            self.live_textures.insert(d.texture_id, m.width as u64 * m.height as u64 * 4);
        }
        self.hash.u64(d.attached as u64);
        self.hash.u64(d.texture_id as u64);
        self.hash.u64(d.clear_depth.to_bits() as u64);
    }
    pub fn begin_canvas(&mut self, m: FromWasmBeginRenderCanvas) {
        self.frame.passes += 1;
        self.hash.u64(14);
        for f in [m.clear_color.r, m.clear_color.g, m.clear_color.b, m.clear_color.a, m.clear_depth] {
            self.hash.u64(f.to_bits() as u64);
        }
    }
    pub fn draw(&mut self, m: FromWasmDrawCall, out: &mut Vec<Reply>) {
        // first draws spread (web_gl.js FromWasmDrawCall): a waiting program's first draw goes in a
        // frame of its own; a second one waits for the next frame
        if let Some(Some(s)) = self.shaders.get_mut(m.shader_id) {
            if s.first_draw_waiting {
                if self.first_draw_this_frame {
                    self.first_draws_deferred = true;
                    return;
                }
                self.first_draw_this_frame = true;
                s.first_draw_waiting = false;
                out.push(Reply::ShadersDone { count: 1, waited: 1 });
            }
        }
        self.frame.draw_calls += 1;
        self.hash.u64(15);
        for v in [m.vao_id, m.shader_id, m.index_width as usize, m.depth_write as usize, m.alpha_blend as usize, m.blend_max as usize, m.backface_culling as usize, m.reset_draw_uniforms as usize] {
            self.hash.u64(v as u64);
        }
        for u in [&m.pass_uniforms, &m.draw_list_uniforms, &m.draw_call_uniforms, &m.user_uniforms, &m.live_uniforms, &m.const_table] {
            let b = f32s(u);
            self.hash.u64(b.len() as u64);
            self.hash.bytes(&b);
        }
        for c in &m.custom_uniforms {
            self.hash.str(&c.block_name);
            self.hash.bytes(&u8s(&c.data));
        }
        for t in &m.textures {
            self.hash.u64(t.map_or(u64::MAX, |t| t as u64));
        }
    }
}

//! Programs from the GLSL dialect payload: loaded from the program cache, or compiled and
//! linked on the render thread (finished by the driver's threads where KHR_parallel_shader_compile
//! exists, by the compile worker after the first frame on Android where it doesn't, `worker.rs`),
//! then the fixed bindings: uniform block i at binding i, texture i on unit i, the pick
//! twin of texture i on unit textures + i, the instance words after them; attribute pointers per
//! draw on one VAO.

use super::cache::Key;
use super::render::Renderer;
use super::sys::{self, GLenum, GLint, GLuint};
use super::{Linked, PipeState};
use crate::glsl::GlslPipeline;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub(super) struct Linking {
    program: GLuint,
    shaders: [GLuint; 2],
    /// where its binary goes once it links; `None` for a program loaded from the cache
    key: Option<Key>,
}

#[derive(Clone, Copy)]
pub(super) struct Attr {
    pub loc: GLuint,
    pub size: GLint,
    pub ty: GLenum,
    pub normalized: bool,
    pub integer: bool,
    pub offset: u32,
}

pub(super) struct GlProgram {
    pub name: GLuint,
    pub vao: GLuint,
    pub geometry: Vec<Attr>,
    pub instance: Vec<Attr>,
    pub vertex_stride: i32,
    /// per texture: its sampler object and the GL target its shape samples
    pub samplers: Vec<GLuint>,
    pub targets: Vec<GLenum>,
    pub shapes: Vec<u32>,
    /// per texture: whether its sampler compares depth, and the renderer's nearest sampler of
    /// the same wrap and compare mode, for a format the driver can't filter
    pub compare: Vec<bool>,
    pub nearest: Vec<GLuint>,
    pub pick: GLint,
    pub inst_base: GLint,
    /// the unit of the instance words (programs that fetch instances by id)
    pub words_unit: Option<u32>,
    /// whether the geometry arrays are enabled (a draw without a vertex buffer disables them)
    pub geometry_on: bool,
}

/// (components, type, normalized) of a payload attribute format (DrawShaderAttrFormat order).
fn attr_format(format: u32, lanes: u32) -> (GLint, GLenum, bool) {
    match format {
        0..=3 => (lanes as GLint, sys::FLOAT, false),
        4 => (2, sys::HALF_FLOAT, false),
        5 => (4, sys::HALF_FLOAT, false),
        6 => (2, sys::UNSIGNED_SHORT, false),
        7 => (2, sys::SHORT, false),
        8 => (2, sys::UNSIGNED_SHORT, true),
        9 => (2, sys::SHORT, true),
        10 => (4, sys::UNSIGNED_BYTE, true),
        11 => (4, sys::BYTE, true),
        12 => (1, sys::UNSIGNED_INT, false),
        _ => (1, sys::INT, false),
    }
}

fn shader_log(gl: &sys::Gl, shader: GLuint) -> String {
    let mut len = 0;
    unsafe { (gl.glGetShaderiv)(shader, sys::INFO_LOG_LENGTH, &mut len) };
    let mut log = vec![0u8; len.max(1) as usize];
    unsafe { (gl.glGetShaderInfoLog)(shader, len.max(1), std::ptr::null_mut(), log.as_mut_ptr()) };
    String::from_utf8_lossy(&log).trim_end_matches('\0').trim().to_owned()
}

fn cstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// Compiles and attaches both stages and starts the link (`hint`: the driver keeps a binary to hand back).
pub(super) fn start_link(gl: &sys::Gl, vertex: &str, fragment: &str, hint: bool) -> (GLuint, [GLuint; 2]) {
    let mut shaders = [0; 2];
    unsafe {
        let program = (gl.glCreateProgram)();
        for (i, (stage, source)) in [(sys::VERTEX_SHADER, vertex), (sys::FRAGMENT_SHADER, fragment)].into_iter().enumerate() {
            let shader = (gl.glCreateShader)(stage);
            let ptr = source.as_ptr();
            let len = source.len() as GLint;
            (gl.glShaderSource)(shader, 1, &ptr, &len);
            (gl.glCompileShader)(shader);
            (gl.glAttachShader)(program, shader);
            shaders[i] = shader;
        }
        // a driver may only hand back a binary for the cache when asked before the link
        if hint { (gl.glProgramParameteri)(program, sys::PROGRAM_BINARY_RETRIEVABLE_HINT, sys::TRUE as GLint) }
        (gl.glLinkProgram)(program);
        (program, shaders)
    }
}

/// Why a program didn't link: its log, and those of the stages that didn't compile.
pub(super) fn link_error(gl: &sys::Gl, program: GLuint, shaders: [GLuint; 2]) -> String {
    let mut len = 0;
    unsafe { (gl.glGetProgramiv)(program, sys::INFO_LOG_LENGTH, &mut len) };
    let mut log = vec![0u8; len.max(1) as usize];
    unsafe { (gl.glGetProgramInfoLog)(program, len.max(1), std::ptr::null_mut(), log.as_mut_ptr()) };
    let mut log = String::from_utf8_lossy(&log).trim_end_matches('\0').trim().to_owned();
    for (stage, s) in ["vertex", "fragment"].iter().zip(shaders) {
        let mut compiled = 0;
        unsafe { (gl.glGetShaderiv)(s, sys::COMPILE_STATUS, &mut compiled) };
        if compiled == 0 { log.push_str(&format!("\n{stage}: {}", shader_log(gl, s))); }
    }
    format!("GLES link failed: {log}")
}

impl Renderer {
    pub fn compile(&mut self, state: &Arc<PipeState>) {
        let key = Key::of(&state.code.vertex, &state.code.fragment);
        #[cfg(target_os = "android")]
        if self.compile_on_worker(state, key) { return; }
        self.compile_here(state, key);
    }

    /// Loads the program from the cache, or compiles and links it on this thread.
    pub(super) fn compile_here(&mut self, state: &Arc<PipeState>, key: Key) {
        let gl = &self.ctx.gl;
        if let Some(program) = self.cache.load(gl, key) {
            self.loaded(state, program);
            return;
        }
        let keep = self.cache.keeps();
        let (program, shaders) = start_link(gl, &state.code.vertex, &state.code.fragment, keep);
        self.linking.push((Arc::downgrade(state), Linking { program, shaders, key: keep.then_some(key) }));
        if !self.caps.parallel_compile { self.poll_links(); }
    }

    /// A program made from a driver binary gets its bindings, and the HAL side hears it linked.
    pub(super) fn loaded(&mut self, state: &PipeState, program: GLuint) {
        let result = self.finish(state, Linking { program, shaders: [0; 2], key: None });
        self.linked(state, result);
    }

    pub(super) fn linked(&self, state: &PipeState, result: Result<Linked, String>) {
        let _ = state.linked.set(result);
        state.epoch.fetch_add(1, Ordering::Release);
        (self.mailbox.wake)();
    }

    pub fn poll_links(&mut self) {
        let parallel = self.caps.parallel_compile;
        let mut i = 0;
        while i < self.linking.len() {
            let program = self.linking[i].1.program;
            let mut done = 1;
            if parallel { unsafe { (self.ctx.gl.glGetProgramiv)(program, sys::COMPLETION_STATUS_KHR, &mut done) } }
            if done == 0 { i += 1; continue; }
            let (state, linking) = self.linking.swap_remove(i);
            let Some(state) = state.upgrade() else {
                let gl = &self.ctx.gl;
                unsafe {
                    for s in linking.shaders { (gl.glDeleteShader)(s) }
                    (gl.glDeleteProgram)(program);
                }
                continue;
            };
            let result = self.finish(&state, linking);
            self.linked(&state, result);
        }
    }

    fn finish(&mut self, state: &PipeState, linking: Linking) -> Result<Linked, String> {
        let gl = &self.ctx.gl;
        let code: &GlslPipeline = &state.code;
        let program = linking.program;
        let mut ok = 0;
        unsafe { (gl.glGetProgramiv)(program, sys::LINK_STATUS, &mut ok) };
        if ok == 0 {
            let log = link_error(gl, program, linking.shaders);
            unsafe {
                for s in linking.shaders { (gl.glDeleteShader)(s) }
                (gl.glDeleteProgram)(program);
            }
            return Err(log);
        }
        unsafe { for s in linking.shaders { (gl.glDeleteShader)(s) } }
        if let Some(key) = linking.key { self.cache.store(gl, key, program); }
        let uniform = |name: &str| unsafe { (gl.glGetUniformLocation)(program, cstr(name).as_ptr()) };
        unsafe { (gl.glUseProgram)(program) };
        let flip = uniform("mp_clip_y_flip");
        if flip >= 0 { unsafe { (gl.glUniform1f)(flip, 1.0) } }
        // every pass stores its rows top-down (the flip above): `fragment_position()`'s y is
        // gl_FragCoord.y as it is
        let frag_y = uniform("mp_frag_y");
        if frag_y >= 0 { unsafe { (gl.glUniform2f)(frag_y, 0.0, 1.0) } }
        let mut block_sizes = Vec::with_capacity(code.blocks.len());
        for (binding, name) in code.blocks.iter().enumerate() {
            let index = unsafe { (gl.glGetUniformBlockIndex)(program, cstr(name).as_ptr()) };
            if index == sys::INVALID_INDEX { block_sizes.push(None); continue; }
            let mut size = 0;
            unsafe {
                (gl.glUniformBlockBinding)(program, index, binding as GLuint);
                (gl.glGetActiveUniformBlockiv)(program, index, sys::UNIFORM_BLOCK_DATA_SIZE, &mut size);
            }
            block_sizes.push(Some(size.max(0) as u32));
        }
        let ntex = code.textures.len() as u32;
        let mut samplers = Vec::new();
        let mut targets = Vec::new();
        let mut compares = Vec::new();
        let mut nearest = Vec::new();
        for (unit, t) in code.textures.iter().enumerate() {
            let loc = uniform(&t.name);
            if loc >= 0 { unsafe { (gl.glUniform1i)(loc, unit as GLint) } }
            if code.pick {
                let twin = uniform(&format!("{}__pick", t.name));
                if twin >= 0 { unsafe { (gl.glUniform1i)(twin, (ntex + unit as u32) as GLint) } }
            }
            let mut sampler = 0;
            let s = t.sampler;
            let linear = s.filter == 1;
            let wrap = super::render::wrap(s.address, self.caps.border_clamp);
            // a shadow sampler (shape 3) compares; every other shape reads plain values
            let compare = t.shape == 3;
            unsafe {
                (gl.glGenSamplers)(1, &mut sampler);
                (gl.glSamplerParameteri)(sampler, sys::TEXTURE_MAG_FILTER, if linear { sys::LINEAR } else { sys::NEAREST } as GLint);
                (gl.glSamplerParameteri)(sampler, sys::TEXTURE_MIN_FILTER, if linear { sys::LINEAR_MIPMAP_LINEAR } else { sys::NEAREST_MIPMAP_LINEAR } as GLint);
                for p in [sys::TEXTURE_WRAP_S, sys::TEXTURE_WRAP_T, sys::TEXTURE_WRAP_R] { (gl.glSamplerParameteri)(sampler, p, wrap as GLint) }
                if compare {
                    (gl.glSamplerParameteri)(sampler, sys::TEXTURE_COMPARE_MODE, sys::COMPARE_REF_TO_TEXTURE as GLint);
                    (gl.glSamplerParameteri)(sampler, sys::TEXTURE_COMPARE_FUNC, sys::LEQUAL as GLint);
                }
            }
            samplers.push(sampler);
            targets.push(super::render::shape_target(t.shape));
            compares.push(compare);
            nearest.push(self.nearest[(s.address.min(3) * 2 + compare as u32) as usize]);
        }
        let words_unit = code.instance_texture.then(|| {
            let unit = ntex * if code.pick { 2 } else { 1 };
            let loc = uniform("mp_inst_data");
            if loc >= 0 { unsafe { (gl.glUniform1i)(loc, unit as GLint) } }
            unit
        });
        let pick = uniform("mp_pick");
        let inst_base = uniform("mp_inst_base");
        // the active inputs' GLSL types say which take integer pointers
        let mut geometry = Vec::new();
        let mut instance = Vec::new();
        let mut column = 0u32;
        for (i, a) in code.attributes.iter().enumerate() {
            column = if i > 0 && code.attributes[i - 1].name == a.name { column + 1 } else { 0 };
            let base = unsafe { (gl.glGetAttribLocation)(program, cstr(&a.name).as_ptr()) };
            if base < 0 { continue; }
            let (size, ty, normalized) = attr_format(a.format, a.lanes);
            let integer = is_integer_input(gl, program, &a.name);
            let attr = Attr { loc: base as GLuint + column, size, ty, normalized, integer, offset: a.offset };
            if a.instance { instance.push(attr) } else { geometry.push(attr) }
        }
        let mut vao = 0;
        unsafe {
            (gl.glGenVertexArrays)(1, &mut vao);
            (gl.glBindVertexArray)(vao);
            for a in geometry.iter().chain(&instance) { (gl.glEnableVertexAttribArray)(a.loc) }
            for a in &instance { (gl.glVertexAttribDivisor)(a.loc, 1) }
            (gl.glBindVertexArray)(0);
            (gl.glUseProgram)(0);
        }
        self.programs.insert(state.key, GlProgram { name: program, vao, geometry, instance, vertex_stride: code.vertex_stride as i32,
            samplers, targets, shapes: code.textures.iter().map(|t| t.shape).collect(), compare: compares, nearest, pick, inst_base, words_unit, geometry_on: true });
        Ok(Linked { block_sizes })
    }

    pub fn delete_program(&mut self, key: u64) {
        let Some(p) = self.programs.remove(&key) else { return };
        let gl = &self.ctx.gl;
        unsafe {
            (gl.glDeleteVertexArrays)(1, &p.vao);
            (gl.glDeleteSamplers)(p.samplers.len() as i32, p.samplers.as_ptr());
            (gl.glDeleteProgram)(p.name);
        }
    }
}

/// Whether a linked program declares `name` as an integer input: those take integer pointers,
/// float inputs converting ones (normalized for the *Norm formats).
fn is_integer_input(gl: &sys::Gl, program: GLuint, name: &str) -> bool {
    let mut count = 0;
    unsafe { (gl.glGetProgramiv)(program, sys::ACTIVE_ATTRIBUTES, &mut count) };
    for i in 0..count.max(0) as GLuint {
        let mut buf = [0u8; 256];
        let (mut len, mut size, mut ty) = (0, 0, 0);
        unsafe { (gl.glGetActiveAttrib)(program, i, buf.len() as i32, &mut len, &mut size, &mut ty, buf.as_mut_ptr()) };
        if &buf[..len.max(0) as usize] == name.as_bytes() {
            return matches!(ty, sys::INT | sys::INT_VEC2 | sys::INT_VEC3 | sys::INT_VEC4 | sys::UNSIGNED_INT | sys::UNSIGNED_INT_VEC2 | sys::UNSIGNED_INT_VEC3 | sys::UNSIGNED_INT_VEC4);
        }
    }
    false
}

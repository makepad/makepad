//! Programs from the GLSL dialect payload: loaded from the program cache, or compiled and
//! linked on the render thread (finished by the driver's threads where KHR_parallel_shader_compile
//! exists, by the compile worker after the first frame on Android where it doesn't, `worker.rs`),
//! then uniform blocks in payload order and dense texture units for active uniforms.

use super::cache::Key;
use super::render::{slot_for_insert, Caps, Renderer};
use super::sys::{self, GLenum, GLint, GLuint};
use super::{GlKey, Linked, PipeState};
use crate::glsl::{GlslPipeline, Texture};
use std::collections::HashSet;
use crate::types::{format, pipeline_blend};
use std::sync::atomic::Ordering;
use std::sync::Arc;

relax_api::option_layouts! {
    Key,
}

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

pub(super) struct TexBinding {
    pub unit: u32,
    pub slot: usize,
    pub target: GLenum,
    pub shape: u32,
    pub sampler: GLuint,
    pub nearest: GLuint,
    pub compare: bool,
}

pub(super) struct PickBinding {
    pub unit: u32,
    pub twin_index: usize,
}

pub(super) struct GlProgram {
    pub name: GLuint,
    pub vao: GLuint,
    pub geometry: Vec<Attr>,
    pub instance: Vec<Attr>,
    pub vertex_stride: i32,
    pub bindings: Vec<TexBinding>,
    pub pick_bindings: Vec<PickBinding>,
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

fn unit_limit_error<'a>(
    name: &str, caps: &Caps, textures: impl Iterator<Item = &'a Texture>,
    twins: usize, instance: bool, static_counts: bool,
) -> Option<String> {
    let mut slots = HashSet::new();
    let mut pairs = 0;
    let mut vertex = usize::from(instance);
    let mut fragment = twins;
    for t in textures {
        slots.insert(t.slot);
        pairs += 1;
        vertex += usize::from(t.stages & 1 != 0);
        fragment += usize::from(t.stages & 2 != 0);
    }
    let combined = pairs + twins + usize::from(instance);
    if vertex <= caps.max_vertex_texture_units as usize && fragment <= caps.max_fragment_texture_units as usize
        && combined <= caps.max_combined_texture_units as usize { return None; }
    let qualifier = if static_counts { "up to " } else { "" };
    Some(format!("shader `{name}` texture unit limit exceeded: need vertex={vertex} (max {}), fragment={fragment} (max {}), combined={combined} (max {}) [logical_textures={}, extra_sampler_pairs={}, pick_twins={qualifier}{twins}, inst_data={}]",
        caps.max_vertex_texture_units, caps.max_fragment_texture_units, caps.max_combined_texture_units,
        slots.len(), pairs - slots.len(), usize::from(instance)))
}

impl Renderer {
    pub fn compile(&mut self, state: &Arc<PipeState>) {
        if state.desc.blend & (pipeline_blend::ADD | pipeline_blend::RAW) == pipeline_blend::ADD {
            for (i, f) in state.desc.color_formats.iter().enumerate() {
                if state.desc.outputs & (1 << i) == 0 { continue; }
                if let Some(reason) = add_blend_error(*f, self.caps.float_blend) {
                    self.linked(state, Err(format!("shader `{}`: {reason}", state.name)));
                    return;
                }
            }
        }
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
        // Worker links and render-thread links both report here.
        let result = result.map_err(|mut log| {
            if log.starts_with("GLES link failed:") {
                let code = &state.code;
                if let Some(reason) = unit_limit_error(&state.name, &self.caps, code.textures.iter(),
                    code.pick_twins.len(), code.instance_texture, true) {
                    log.push('\n');
                    log.push_str(&reason);
                }
            }
            log
        });
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
                    for s in linking.shaders.into_iter().filter(|s| *s != 0) { (gl.glDeleteShader)(s) }
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
                for s in linking.shaders.into_iter().filter(|s| *s != 0) { (gl.glDeleteShader)(s) }
                (gl.glDeleteProgram)(program);
            }
            return Err(log);
        }
        unsafe { for s in linking.shaders.into_iter().filter(|s| *s != 0) { (gl.glDeleteShader)(s) } }
        let uniform = |name: &str| unsafe { (gl.glGetUniformLocation)(program, cstr(name).as_ptr()) };
        let active: Vec<_> = code.textures.iter().filter_map(|t| {
            let loc = uniform(&t.name);
            (loc >= 0).then_some((t, loc))
        }).collect();
        let twins: Vec<_> = code.pick_twins.iter().enumerate().filter_map(|(i, t)| {
            let loc = uniform(&t.name);
            (loc >= 0).then_some((i, loc))
        }).collect();
        let words = code.instance_texture.then(|| uniform("mp_inst_data")).filter(|&loc| loc >= 0);
        if let Some(reason) = unit_limit_error(&state.name, &self.caps, active.iter().map(|&(t, _)| t),
            twins.len(), words.is_some(), false) {
            unsafe { (gl.glDeleteProgram)(program) };
            return Err(reason);
        }
        if let Some(key) = linking.key { self.cache.store(gl, key, program); }
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
        let mut bindings = Vec::with_capacity(active.len());
        for (unit, (t, loc)) in active.into_iter().enumerate() {
            unsafe { (gl.glUniform1i)(loc, unit as GLint) }
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
                if s.anisotropy > 1 && self.caps.max_anisotropy > 1.0 {
                    (gl.glSamplerParameterf)(sampler, sys::TEXTURE_MAX_ANISOTROPY, (s.anisotropy as f32).min(self.caps.max_anisotropy));
                }
                for p in [sys::TEXTURE_WRAP_S, sys::TEXTURE_WRAP_T, sys::TEXTURE_WRAP_R] { (gl.glSamplerParameteri)(sampler, p, wrap as GLint) }
                if compare {
                    (gl.glSamplerParameteri)(sampler, sys::TEXTURE_COMPARE_MODE, sys::COMPARE_REF_TO_TEXTURE as GLint);
                    (gl.glSamplerParameteri)(sampler, sys::TEXTURE_COMPARE_FUNC, sys::LEQUAL as GLint);
                }
            }
            bindings.push(TexBinding {
                unit: unit as u32, slot: t.slot as usize, target: super::render::shape_target(t.shape), shape: t.shape,
                sampler, compare, nearest: self.nearest[(s.address.min(3) * 2 + compare as u32) as usize],
            });
        }
        let mut pick_bindings = Vec::with_capacity(twins.len());
        for (twin_index, loc) in twins {
            let unit = (bindings.len() + pick_bindings.len()) as u32;
            unsafe { (gl.glUniform1i)(loc, unit as GLint) }
            pick_bindings.push(PickBinding { unit, twin_index });
        }
        let active_twins = pick_bindings.iter().map(|b| b.twin_index).collect();
        let words_unit = words.map(|loc| {
            let unit = (bindings.len() + pick_bindings.len()) as u32;
            unsafe { (gl.glUniform1i)(loc, unit as GLint) }
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
        *slot_for_insert(&mut self.programs, state.key) = Some(GlProgram { name: program, vao, geometry, instance, vertex_stride: code.vertex_stride as i32,
            bindings, pick_bindings, pick, inst_base, words_unit, geometry_on: true });
        Ok(Linked { block_sizes, active_twins })
    }

    pub fn delete_program(&mut self, key: GlKey) {
        if let Some(p) = self.programs.get_mut(key as usize).and_then(Option::take) {
            let gl = &self.ctx.gl;
            unsafe {
                (gl.glDeleteVertexArrays)(1, &p.vao);
                for b in &p.bindings { (gl.glDeleteSamplers)(1, &b.sampler); }
                (gl.glDeleteProgram)(p.name);
            }
        }
        self.prog_recycled.lock().unwrap_or_else(|e| e.into_inner()).push(key);
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

/// Add never falls back to a raw write on an attachment the context cannot blend.
pub(super) fn add_blend_error(format: u32, float_blend: bool) -> Option<String> {
    match format {
        format::BGRA8 | format::RGBA8 | format::RGBA16F | format::RG16F | format::R16F | format::R8 | format::RG8 => None,
        format::RGBA32F | format::R32F if float_blend => None,
        format::RGBA32F | format::R32F => Some("blend_op: @Add on a 32-bit float target requires GL_EXT_float_blend".into()),
        _ => Some(format!("blend_op: @Add requires color attachment blending for target format {format}")),
    }
}

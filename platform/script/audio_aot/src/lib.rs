//! Splash audio shaders: instruments and effects written as small Splash
//! functions, compiled ahead of time (at edit time, on a worker thread)
//! into realtime-safe code for the audio thread. The math_aot concept
//! (platform/script/math_aot/MATH_AOT.md) grown with logic and state; the
//! plan of record is local/agent_state/edits/design/AUDIO-SHADERS.md.
//!
//! ```text
//! shader source --[parse]--> AST --[lower: types, inlining, math]--> AIR
//!     AIR --> ir::run (the reference interpreter, and the no-JIT path)
//!     AIR --> arm64 (native code: MAP_JIT, W^X)
//! ```
//!
//! Every backend is bit-identical to [`ir::run`]: AIR's ops are total and
//! exactly specified, and math functions are polynomials made of those ops
//! (no libm on the audio path), so a score renders the same samples on
//! every machine, offline or live.

pub mod fuse;
pub mod ir;
pub mod lower;
pub mod parse;

#[cfg(target_arch = "aarch64")]
pub mod arm64;

pub use fuse::{fuse, FuseError, FuseNode, Port};
pub use lower::{header, Kind, ParamInfo, StateVar, CONTROL_BLOCK, CTX_FRAME, CTX_PARAMS, CTX_RATE, MAX_FRAMES};

use std::sync::Arc;

/// A compile error with a byte span into the shader's code string.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderError {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

impl ShaderError {
    pub fn new(start: usize, end: usize, message: String) -> Self {
        ShaderError { start, end, message }
    }

    /// 1-based line and column of the error start in `src`.
    pub fn line_col(&self, src: &str) -> (usize, usize) {
        let at = self.start.min(src.len());
        let before = &src[..at];
        let line = before.matches('\n').count() + 1;
        let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        (line, col)
    }
}

impl std::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at {}..{})", self.message, self.start, self.end)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The AIR interpreter (the reference; the path where executable
    /// memory is unavailable).
    Interp,
    /// Native machine code for the host (ARM64 today).
    Native,
}

/// A compiled audio shader. Immutable and shareable; all mutable data
/// (ctx, state, scratch) belongs to the caller, allocated off the audio
/// thread.
pub struct AudioShader {
    pub kind: Kind,
    params: Vec<ParamInfo>,
    state_init: Box<[u32]>,
    state_vars: Vec<StateVar>,
    shared: Box<[u32]>,
    render: ir::Program,
    #[cfg(target_arch = "aarch64")]
    native: Option<arm64::Code>,
}

/// Compiles with the fastest backend available on this host.
pub fn compile(code: &str) -> Result<Arc<AudioShader>, Vec<ShaderError>> {
    compile_with(code, Backend::Native)
}

/// Compiles for a given backend (`Native` falls back to the interpreter
/// when the host has no native backend).
pub fn compile_with(code: &str, backend: Backend) -> Result<Arc<AudioShader>, Vec<ShaderError>> {
    let toks = parse::lex(code).map_err(|e| vec![e])?;
    let items = parse::Parser::new(&toks).items().map_err(|e| vec![e])?;
    compile_items(&items, backend)
}

/// Compiles a parsed shader (also the back half of [`fuse::fuse`]).
pub(crate) fn compile_items(items: &[parse::Item], backend: Backend) -> Result<Arc<AudioShader>, Vec<ShaderError>> {
    let lowered = lower::lower(items).map_err(|e| vec![e])?;
    let ctx_words = CTX_PARAMS as usize + lowered.params.len();
    let state_words = lowered.state_init.len();
    let shared_words = lowered.shared_init.len().max(1);
    let sizes = move |r: ir::Region| -> u32 {
        match r {
            ir::Region::Ctx => ctx_words as u32,
            ir::Region::State => state_words as u32,
            ir::Region::Shared => shared_words as u32,
            ir::Region::Frame => u32::MAX,
        }
    };
    if let Err(e) = ir::validate(&lowered.render, &sizes) {
        return Err(vec![ShaderError::new(0, 1, format!("internal compiler error: {}", e))]);
    }
    let mut shared = lowered.shared_init.clone();
    if shared.is_empty() {
        shared.push(0);
    }
    #[cfg(target_arch = "aarch64")]
    let native_ok = backend == Backend::Native;
    // Tables: run init() once (natively when possible: it can be big).
    if let Some(init) = &lowered.init {
        if let Err(e) = ir::validate(init, &sizes) {
            return Err(vec![ShaderError::new(0, 1, format!("internal compiler error in init: {}", e))]);
        }
        let mut ctx = vec![0u32; ctx_words];
        let mut state = vec![0u32; state_words.max(1)];
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        #[cfg(target_arch = "aarch64")]
        {
            if native_ok {
                if let Some(code) = arm64::compile(init) {
                    code.run(&mut ctx, &mut state, &mut shared, [&zeros, &zeros], [&mut o0, &mut o1], 1);
                } else {
                    run_init_interp(init, &mut ctx, &mut state, &mut shared);
                }
            } else {
                run_init_interp(init, &mut ctx, &mut state, &mut shared);
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = (&zeros, &mut o0, &mut o1, backend);
            run_init_interp(init, &mut ctx, &mut state, &mut shared);
        }
    }
    #[cfg(target_arch = "aarch64")]
    let native = if native_ok { arm64::compile(&lowered.render) } else { None };
    Ok(Arc::new(AudioShader {
        kind: lowered.kind,
        params: lowered.params,
        state_init: lowered.state_init.into_boxed_slice(),
        state_vars: lowered.state_vars,
        shared: shared.into_boxed_slice(),
        render: lowered.render,
        #[cfg(target_arch = "aarch64")]
        native,
    }))
}

fn run_init_interp(init: &ir::Program, ctx: &mut [u32], state: &mut [u32], shared: &mut [u32]) {
    let mut scratch = vec![0u32; init.scratch_words()];
    let zeros = [0f32; 1];
    let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
    let mut mem = ir::Mem { ctx, state, shared };
    let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
    ir::run(init, &mut scratch, &mut mem, &mut io, 1);
}

impl std::fmt::Debug for AudioShader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AudioShader({:?}, {} params, {} state words, {:?})", self.kind, self.params.len(), self.state_init.len(), self.backend())
    }
}

impl AudioShader {
    pub fn params(&self) -> &[ParamInfo] {
        &self.params
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }

    pub fn state_vars(&self) -> &[StateVar] {
        &self.state_vars
    }

    pub fn state_words(&self) -> usize {
        self.state_init.len()
    }

    pub fn ctx_words(&self) -> usize {
        CTX_PARAMS as usize + self.params.len()
    }

    /// Scratch words `run` needs (the interpreter's registers and frame).
    pub fn scratch_words(&self) -> usize {
        self.render.scratch_words()
    }

    pub fn backend(&self) -> Backend {
        #[cfg(target_arch = "aarch64")]
        if self.native.is_some() {
            return Backend::Native;
        }
        Backend::Interp
    }

    /// The AIR render program (for tests and tools).
    pub fn program(&self) -> &ir::Program {
        &self.render
    }

    /// Bytes of native code, if any.
    pub fn native_code_bytes(&self) -> usize {
        #[cfg(target_arch = "aarch64")]
        if let Some(code) = &self.native {
            return code.len();
        }
        0
    }

    /// A fresh ctx block: the sample rate and every param at its default.
    pub fn new_ctx(&self, sample_rate: f32) -> Box<[u32]> {
        let mut ctx = vec![0u32; self.ctx_words()].into_boxed_slice();
        ctx[CTX_RATE as usize] = sample_rate.to_bits();
        for (k, p) in self.params.iter().enumerate() {
            ctx[CTX_PARAMS as usize + k] = p.default.to_bits();
        }
        ctx
    }

    pub fn new_state(&self) -> Box<[u32]> {
        self.state_init.clone()
    }

    pub fn new_scratch(&self) -> Box<[u32]> {
        vec![0u32; self.scratch_words()].into_boxed_slice()
    }

    /// Sets a param's target (it ramps there over the next control block).
    pub fn set_param(&self, ctx: &mut [u32], index: usize, value: f32) {
        if let Some(p) = self.params.get(index) {
            let v = if value.is_finite() { value.clamp(p.min, p.max) } else { p.default };
            ctx[CTX_PARAMS as usize + index] = v.to_bits();
        }
    }

    /// Resets a voice or effect state to its initial image (no allocation).
    pub fn reset_state(&self, state: &mut [u32]) {
        state.copy_from_slice(&self.state_init);
    }

    /// Starts a note on a voice state: reset, then note inputs; `frame` is
    /// the absolute frame the note starts at, `seed` seeds `rand()`.
    pub fn note_on(&self, state: &mut [u32], note: f32, velocity: f32, frame: u32, seed: u32) {
        self.reset_state(state);
        let freq = 440.0 * ((note - 69.0) / 12.0).exp2();
        state[header::NOTE as usize] = note.to_bits();
        state[header::FREQ as usize] = freq.to_bits();
        state[header::GATE as usize] = 1f32.to_bits();
        state[header::VELOCITY as usize] = velocity.to_bits();
        state[header::TRIGGER as usize] = frame;
        state[header::RNG as usize] = seed.wrapping_mul(0x9e37_79b9) | 1;
    }

    /// Hot swap: fills `state` (this shader's layout) from `old_state` (the
    /// `old` shader's): the note inputs, every param's smoothing, and every
    /// state variable whose name and type match; the rest starts from its
    /// initial value. Returns true when everything carried (a seamless
    /// swap); otherwise the host should crossfade. No allocation.
    pub fn carry_state(&self, state: &mut [u32], old: &AudioShader, old_state: &[u32]) -> bool {
        self.reset_state(state);
        let h = lower::HEADER_WORDS as usize;
        state[..h].copy_from_slice(&old_state[..h]);
        let mut all = self.kind == old.kind;
        for (k, p) in self.params.iter().enumerate() {
            if let Some(j) = old.param_index(&p.name) {
                let (a, b) = (h + 2 * k, h + 2 * j);
                state[a] = old_state[b];
                state[a + 1] = old_state[b + 1];
            }
        }
        for v in &self.state_vars {
            match old.state_vars.iter().find(|o| o.name == v.name && o.sig == v.sig) {
                Some(o) => {
                    let (a, b) = (v.offset as usize, o.offset as usize);
                    state[a..a + v.words as usize].copy_from_slice(&old_state[b..b + o.words as usize]);
                }
                None => all = false,
            }
        }
        if !all {
            // block() and the param ramps restart on the next frame.
            state[header::FRESH as usize] = 1;
        }
        all
    }

    pub fn note_off(&self, state: &mut [u32]) {
        state[header::GATE as usize] = 0f32.to_bits();
    }

    pub fn stopped(&self, state: &[u32]) -> bool {
        state[header::STOP as usize] != 0
    }

    /// Renders `n` frames (1..=MAX_FRAMES), ADDING into `outs`. `ctx`
    /// holds the absolute frame of the first one (`CTX_FRAME`). Realtime
    /// safe: no allocation, no locks, bounded time.
    pub fn run(&self, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32], ins: [&[f32]; 2], outs: [&mut [f32]; 2], n: usize) {
        let n = n.min(MAX_FRAMES as usize);
        if n == 0 {
            return;
        }
        assert!(ctx.len() == self.ctx_words() && state.len() == self.state_init.len());
        assert!(ins[0].len() >= n && ins[1].len() >= n && outs[0].len() >= n && outs[1].len() >= n);
        #[cfg(target_arch = "aarch64")]
        if let Some(code) = &self.native {
            // SAFETY-relevant contract: `run` only touches the slices
            // passed here, with every access clamped into them.
            let shared = self.shared.as_ptr() as *mut u32;
            let shared = unsafe { std::slice::from_raw_parts_mut(shared, self.shared.len()) };
            code.run(ctx, state, shared, ins, outs, n as u32);
            return;
        }
        self.run_interp(ctx, state, scratch, ins, outs, n);
    }

    /// The reference interpreter, whatever the backend.
    pub fn run_interp(&self, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32], ins: [&[f32]; 2], outs: [&mut [f32]; 2], n: usize) {
        let n = n.min(MAX_FRAMES as usize);
        // The render program never writes shared tables (checked by the
        // front end), so a shared view is sound.
        let shared = self.shared.as_ptr() as *mut u32;
        let shared = unsafe { std::slice::from_raw_parts_mut(shared, self.shared.len()) };
        let mut mem = ir::Mem { ctx, state, shared };
        let [o0, o1] = outs;
        let mut io = ir::Io { ins, outs: [o0, o1] };
        ir::run(&self.render, scratch, &mut mem, &mut io, n as u32);
    }
}

/// One playable instance (a voice or an effect) with its own frame
/// counter: the simple host for tests, tools and offline renders.
pub struct Instance {
    pub shader: Arc<AudioShader>,
    pub ctx: Box<[u32]>,
    pub state: Box<[u32]>,
    scratch: Box<[u32]>,
    frame: u32,
    zeros: Box<[f32]>,
}

impl Instance {
    pub fn new(shader: Arc<AudioShader>, sample_rate: f32) -> Self {
        Instance {
            ctx: shader.new_ctx(sample_rate),
            state: shader.new_state(),
            scratch: shader.new_scratch(),
            frame: 0,
            zeros: vec![0.0; MAX_FRAMES as usize].into_boxed_slice(),
            shader,
        }
    }

    pub fn frame(&self) -> u32 {
        self.frame
    }

    pub fn set_param(&mut self, name: &str, value: f32) -> bool {
        match self.shader.param_index(name) {
            Some(k) => {
                self.shader.set_param(&mut self.ctx, k, value);
                true
            }
            None => false,
        }
    }

    pub fn note_on(&mut self, note: f32, velocity: f32, seed: u32) {
        self.shader.note_on(&mut self.state, note, velocity, self.frame, seed);
    }

    pub fn note_off(&mut self) {
        self.shader.note_off(&mut self.state);
    }

    /// Instrument render: adds `l.len()` frames into l/r.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        let mut done = 0;
        while done < n {
            let k = (n - done).min(MAX_FRAMES as usize);
            self.ctx[CTX_FRAME as usize] = self.frame;
            let zeros = &self.zeros[..k];
            self.shader.run(&mut self.ctx, &mut self.state, &mut self.scratch, [zeros, zeros], [&mut l[done..done + k], &mut r[done..done + k]], k);
            self.frame = self.frame.wrapping_add(k as u32);
            done += k;
        }
    }

    /// Effect render: reads in_l/in_r, adds into out_l/out_r.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        let mut done = 0;
        while done < n {
            let k = (n - done).min(MAX_FRAMES as usize);
            self.ctx[CTX_FRAME as usize] = self.frame;
            self.shader.run(
                &mut self.ctx,
                &mut self.state,
                &mut self.scratch,
                [&in_l[done..done + k], &in_r[done..done + k]],
                [&mut out_l[done..done + k], &mut out_r[done..done + k]],
                k,
            );
            self.frame = self.frame.wrapping_add(k as u32);
            done += k;
        }
    }
}

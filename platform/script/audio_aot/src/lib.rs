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
mod guide;

// The language, IR, interpreter and native backends are the Splash compute
// core's; audio is one of its front ends.
#[cfg(target_arch = "aarch64")]
pub use makepad_script_compute::arm64;
pub use makepad_script_compute::{ir, lower, parse, Backend, ShaderError};

pub use guide::GUIDE;
pub use fuse::{fuse, FuseError, FuseNode, Port};
pub use lower::{header, Kind, ParamInfo, StateVar, CONTROL_BLOCK, CTX_FRAME, CTX_PARAMS, CTX_RATE, MAX_FRAMES};

use std::sync::Arc;

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
    compile_items(&items, code.len(), backend)
}

/// The prelude: DSP building blocks every shader sees (see prelude.splash).
pub const PRELUDE: &str = include_str!("prelude.splash");

fn prelude_items(user_len: usize) -> Vec<parse::Item> {
    makepad_script_compute::parse_prelude(PRELUDE, user_len)
}

/// Compiles a parsed shader (also the back half of [`fuse::fuse`]).
pub(crate) fn compile_items(items: &[parse::Item], user_len: usize, backend: Backend) -> Result<Arc<AudioShader>, Vec<ShaderError>> {
    // Prelude items the shader does not redefine come first (a shader's
    // state may be a prelude struct), then the shader's own.
    let names: std::collections::HashSet<String> = items.iter().map(|i| i.name().to_string()).collect();
    let mut all: Vec<parse::Item> = prelude_items(user_len).into_iter().filter(|i| !names.contains(i.name())).collect();
    all.extend(items.iter().cloned());
    let lowered = lower::lower(&all, user_len + 1).map_err(|e| vec![e])?;
    let ctx_words = CTX_PARAMS as usize + lowered.params.len();
    let state_words = lowered.state_init.len();
    let shared_words = lowered.shared_init.len().max(1);
    let regions = |p: &ir::Program, init: bool| ir::Regions {
        ctx: ctx_words as u32,
        state: state_words as u32,
        shared: shared_words as u32,
        frame: p.frame_words,
        shared_writable: init,
        bufs: Vec::new(),
        io: true,
    };
    if let Err(e) = ir::validate(&lowered.render, &regions(&lowered.render, false)) {
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
        if let Err(e) = ir::validate(init, &regions(init, true)) {
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
                    // SAFETY: `shared` is this init's own table vector.
                    unsafe { code.run(&mut ctx, &mut state, shared.as_mut_ptr(), [&zeros, &zeros], [&mut o0, &mut o1], 1) };
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
    let mut mem = ir::Mem { ctx, state, shared: ir::Shared::Write(shared), bufs: &[] };
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

    /// A legato note change: new pitch and velocity without a retrigger
    /// (state, envelopes and `trigger` untouched; slides are the shader's).
    pub fn legato(&self, state: &mut [u32], note: f32, velocity: f32) {
        state[header::NOTE as usize] = note.to_bits();
        state[header::VELOCITY as usize] = velocity.to_bits();
        state[header::GATE as usize] = 1f32.to_bits();
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
            // SAFETY: the render program never stores to shared tables
            // (validated), so handing native code a pointer derived from
            // the immutable table only ever reads it.
            unsafe { code.run(ctx, state, self.shared.as_ptr() as *mut u32, ins, outs, n as u32) };
            return;
        }
        self.run_interp(ctx, state, scratch, ins, outs, n);
    }

    /// The reference interpreter, whatever the backend.
    pub fn run_interp(&self, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32], ins: [&[f32]; 2], outs: [&mut [f32]; 2], n: usize) {
        let n = n.min(MAX_FRAMES as usize);
        let mut mem = ir::Mem { ctx, state, shared: ir::Shared::Read(&self.shared), bufs: &[] };
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

/// What [`check`] found: a compile plus an offline audition of a test
/// phrase, for the editor and the AI composer's shader tools.
#[derive(Clone, Debug)]
pub struct CheckReport {
    pub kind: Kind,
    pub params: Vec<ParamInfo>,
    pub state_words: usize,
    pub backend: Backend,
    /// Largest absolute sample.
    pub peak: f32,
    pub rms: f32,
    /// Mean of the output (a DC offset shows here).
    pub dc: f32,
    /// NaN or infinite samples.
    pub nonfinite: usize,
    /// Share of samples at or above full scale.
    pub clipped: f32,
    /// Native cost of one instance, ns per frame (48 kHz: 20833 = one core).
    pub ns_per_frame: f64,
    /// Instruments: seconds from the last release until the voice stopped
    /// or went quiet (-80 dB); None if it never did within 8 s.
    pub tail_secs: Option<f32>,
    /// Plain-language problems found (empty when it looks healthy).
    pub warnings: Vec<String>,
}

/// Compiles `code` and auditions it at 48 kHz: an instrument plays a short
/// phrase (C3 E3 G3, held, then released), an effect processes a test
/// signal (a saw chord with clicks). Off the audio thread.
pub fn check(code: &str) -> Result<CheckReport, Vec<ShaderError>> {
    const RATE: f32 = 48000.0;
    let shader = compile(code)?;
    let secs = 3.0;
    let n = (secs * RATE) as usize;
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    let mut tail_secs = None;
    let t0 = std::time::Instant::now();
    let mut instance_frames = 0usize;
    match shader.kind {
        Kind::Instrument => {
            let notes = [48.0f32, 52.0, 55.0];
            let release_at = (1.0 * RATE) as usize;
            let mut voices: Vec<Instance> = notes.iter().map(|_| Instance::new(shader.clone(), RATE)).collect();
            let mut last_loud = 0usize;
            let mut stopped_at = None;
            let (mut vl, mut vr) = (vec![0.0f32; 128], vec![0.0f32; 128]);
            let mut at = 0;
            while at < n {
                let k = 128.min(n - at);
                let mut block_peak = 0.0f32;
                let mut all_stopped = at >= release_at;
                for (i, v) in voices.iter_mut().enumerate() {
                    let start = (i as f32 * 0.25 * RATE) as usize;
                    if at == start {
                        v.note_on(notes[i], 0.8, i as u32 + 1);
                    }
                    if at == release_at {
                        v.note_off();
                    }
                    if at < start || shader.stopped(&v.state) {
                        continue;
                    }
                    all_stopped = false;
                    vl[..k].fill(0.0);
                    vr[..k].fill(0.0);
                    v.render(&mut vl[..k], &mut vr[..k]);
                    instance_frames += k;
                    for j in 0..k {
                        l[at + j] += vl[j];
                        r[at + j] += vr[j];
                        block_peak = block_peak.max(vl[j].abs()).max(vr[j].abs());
                    }
                }
                if block_peak > 1e-4 {
                    last_loud = at + k;
                }
                if all_stopped && stopped_at.is_none() {
                    stopped_at = Some(at);
                }
                at += k;
            }
            let end = stopped_at.unwrap_or(last_loud).max(release_at);
            if end < n - 128 {
                tail_secs = Some((end - release_at) as f32 / RATE);
            }
        }
        Kind::Effect => {
            let mut inst = Instance::new(shader.clone(), RATE);
            let mut phases = [0.0f32; 3];
            let (mut il, mut ir) = (vec![0.0f32; n], vec![0.0f32; n]);
            for i in 0..n {
                let mut x = 0.0;
                for (k, f) in [130.8f32, 164.8, 196.0].iter().enumerate() {
                    phases[k] = (phases[k] + f / RATE).fract();
                    x += (2.0 * phases[k] - 1.0) * 0.15;
                }
                if i % 12000 == 0 {
                    x += 0.5;
                }
                il[i] = x;
                ir[i] = x * 0.9;
            }
            inst.process(&il, &ir, &mut l, &mut r);
            instance_frames = n;
        }
    }
    let ns_per_frame = t0.elapsed().as_secs_f64() * 1e9 / instance_frames.max(1) as f64;
    let all = l.iter().chain(r.iter());
    let nonfinite = all.clone().filter(|x| !x.is_finite()).count();
    let finite: Vec<f32> = all.filter(|x| x.is_finite()).copied().collect();
    let peak = finite.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let rms = (finite.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / finite.len().max(1) as f64).sqrt() as f32;
    let dc = (finite.iter().map(|x| *x as f64).sum::<f64>() / finite.len().max(1) as f64) as f32;
    let clipped = finite.iter().filter(|x| x.abs() >= 1.0).count() as f32 / finite.len().max(1) as f32;
    let mut warnings = Vec::new();
    if nonfinite > 0 {
        warnings.push(format!("{} NaN/infinite samples: look for a division by zero, log of 0 or an unstable filter", nonfinite));
    }
    if peak < 1e-4 {
        warnings.push("silent: nothing reaches the output (check the return value and the envelope)".into());
    } else if clipped > 0.001 {
        warnings.push(format!("{:.1}% of samples at or above full scale: lower the gain", clipped * 100.0));
    } else if peak > 1.0 {
        warnings.push(format!("peaks at {:.2}: lower the gain", peak));
    }
    if dc.abs() > 0.05 * rms.max(1e-6) && dc.abs() > 0.01 {
        warnings.push(format!("DC offset {:.3}: add a dc_block() (asymmetric shapers make DC)", dc));
    }
    if shader.kind == Kind::Instrument && tail_secs.is_none() {
        warnings.push("voices never end after release: fade out on `gate == 0.0` (an adsr() does), or call stop()".into());
    }
    if ns_per_frame > 2000.0 {
        warnings.push(format!("expensive: {:.0} ns per frame per voice ({:.1}% of a core at 48 kHz)", ns_per_frame, ns_per_frame * 48000.0 / 1e7));
    }
    Ok(CheckReport {
        kind: shader.kind,
        params: shader.params.clone(),
        state_words: shader.state_words(),
        backend: shader.backend(),
        peak,
        rms,
        dc,
        nonfinite,
        clipped,
        ns_per_frame,
        tail_secs,
        warnings,
    })
}

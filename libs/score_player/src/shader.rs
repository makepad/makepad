//! Audio shader nodes: instruments and effects written as Splash audio
//! shaders (makepad-script-audio-aot), compiled off the audio thread and
//! run here realtime-safe: fixed voice pools and buffers allocated at
//! construction, no allocation, locking or freeing while rendering.
//!
//! Every instance keeps its own absolute frame count; the shader keys its
//! control blocks (params, `block()`) to it, so any slicing of `render`
//! calls gives the same samples (offline render = live).
//!
//! Hot swap (`swap_from`, at a block boundary): voices and effect state
//! carry over variable by variable (same name and type); when a variable
//! could not carry, the old program keeps rendering for [`FADE`] frames,
//! fading out while the new one fades in.

use makepad_script_audio_aot::{Backend, CTX_FRAME};

pub use makepad_script_audio_aot::{check, AudioShader, AudioShaderRef, CheckReport, Kind, GUIDE};

/// A compile error with a byte span into the shader's source.
#[derive(Clone, Debug)]
pub struct CompileError {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

/// Compiles an audio shader with the fastest backend on this host; the
/// errors are copied out of the compiler's result views.
pub fn compile(code: &str) -> Result<AudioShaderRef, Vec<CompileError>> {
    let mut compiled = makepad_script_audio_aot::boundary::compile_audio(code, Backend::Native, false);
    if let Some(shader) = compiled.take_shader() {
        return Ok(shader);
    }
    Err((0..compiled.error_count())
        .map(|index| {
            let error = compiled.error(index);
            CompileError { start: error.start, end: error.end, message: error.message.to_string() }
        })
        .collect())
}

/// The index of the param called `name`.
pub fn param_index(shader: &AudioShader, name: &str) -> Option<usize> {
    (0..shader.param_count()).find(|&index| shader.param_view(index).name == name)
}

/// A ctx block for `shader` at `rate`, every param at its default.
fn new_ctx(shader: &AudioShader, rate: f32) -> Box<[u32]> {
    let mut ctx = vec![0u32; shader.ctx_words()].into_boxed_slice();
    shader.init_ctx(&mut ctx, rate);
    ctx
}

/// A voice or effect state at its initial image.
fn new_state(shader: &AudioShader) -> Box<[u32]> {
    let mut state = vec![0u32; shader.state_words()].into_boxed_slice();
    shader.reset_state(&mut state);
    state
}

fn new_scratch(shader: &AudioShader) -> Box<[u32]> {
    vec![0u32; shader.scratch_words()].into_boxed_slice()
}

/// The shipped shader library: instruments and character effects as
/// audio-shader documents (editable like any shader; a Score names them).
pub const LIBRARY: &[(&str, &str)] = &[
    ("supersaw", include_str!("../shaders/supersaw.splash")),
    ("analog_poly", include_str!("../shaders/analog_poly.splash")),
    ("acid", include_str!("../shaders/acid.splash")),
    ("pluck", include_str!("../shaders/pluck.splash")),
    ("epiano", include_str!("../shaders/epiano.splash")),
    ("modal_bell", include_str!("../shaders/modal_bell.splash")),
    ("techno_kick", include_str!("../shaders/techno_kick.splash")),
    ("drive", include_str!("../shaders/drive.splash")),
    ("phaser", include_str!("../shaders/phaser.splash")),
    ("flanger", include_str!("../shaders/flanger.splash")),
    ("echo", include_str!("../shaders/echo.splash")),
    ("gated_verb", include_str!("../shaders/gated_verb.splash")),
];

/// A library shader's source by name.
pub fn library(name: &str) -> Option<&'static str> {
    LIBRARY.iter().find(|(n, _)| *n == name).map(|(_, code)| *code)
}

/// Voices per shader instrument (as a loop's own pool).
pub const SHADER_VOICES: usize = 12;
/// Crossfade length of a hot swap that could not carry all state.
pub const FADE: u32 = 256;
/// Most frames one render call takes.
pub const MAX_RENDER: usize = 256;
/// A released voice ends after this many frames below [`SILENT`].
const QUIET_FRAMES: u32 = 128;
const SILENT: f32 = 1e-5;
/// ... or at the latest this long after its release.
const RELEASE_LIMIT_SECS: f32 = 30.0;

struct Voice {
    state: Box<[u32]>,
    scratch: Box<[u32]>,
    active: bool,
    released: bool,
    quiet: u32,
    released_frames: u32,
    /// Fades in during a crossfade swap.
    fade_in: bool,
}

impl Voice {
    fn new(shader: &AudioShader) -> Voice {
        Voice {
            state: new_state(shader),
            scratch: new_scratch(shader),
            active: false,
            released: false,
            quiet: 0,
            released_frames: 0,
            fade_in: false,
        }
    }
}

struct Fade {
    shader: AudioShaderRef,
    ctx: Box<[u32]>,
    voices: Box<[Voice]>,
    /// Frames left; 0 = done (the data stays until the owner is dropped
    /// off the audio thread).
    left: u32,
}

/// Mono/stereo work buffers.
struct Bufs {
    l: Box<[f32]>,
    r: Box<[f32]>,
    zeros: Box<[f32]>,
}

impl Bufs {
    fn new() -> Bufs {
        Bufs {
            l: vec![0.0; MAX_RENDER].into_boxed_slice(),
            r: vec![0.0; MAX_RENDER].into_boxed_slice(),
            zeros: vec![0.0; MAX_RENDER].into_boxed_slice(),
        }
    }
}

/// Renders one voice into the work buffers and adds it to l/r with a
/// per-frame gain ramp (`g0 + dg * i`); returns the voice's peak.
fn render_voice(shader: &AudioShader, ctx: &mut [u32], v: &mut Voice, bufs: &mut Bufs, l: &mut [f32], r: &mut [f32], g0: f32, dg: f32) -> f32 {
    let n = l.len();
    let (bl, br) = (&mut bufs.l[..n], &mut bufs.r[..n]);
    bl.fill(0.0);
    br.fill(0.0);
    let z = &bufs.zeros[..n];
    shader.run(ctx, &mut v.state, &mut v.scratch, z, z, bl, br, n);
    let mut peak = 0.0f32;
    for i in 0..n {
        let g = (g0 + dg * i as f32).clamp(0.0, 1.0);
        peak = peak.max(bufs.l[i].abs()).max(bufs.r[i].abs());
        l[i] += bufs.l[i] * g;
        r[i] += bufs.r[i] * g;
    }
    peak
}

fn end_voice(shader: &AudioShader, v: &mut Voice, peak: f32, n: u32, rate: f32) {
    if shader.stopped(&v.state) {
        v.active = false;
        return;
    }
    if v.released {
        v.released_frames = v.released_frames.saturating_add(n);
        v.quiet = if peak < SILENT { v.quiet + n } else { 0 };
        if v.quiet >= QUIET_FRAMES || v.released_frames as f32 > RELEASE_LIMIT_SECS * rate {
            v.active = false;
        }
    }
}

/// A shader instrument: a fixed pool of voices.
pub struct ShaderInst {
    shader: AudioShaderRef,
    ctx: Box<[u32]>,
    voices: Box<[Voice]>,
    rate: f32,
    frame: u32,
    bufs: Bufs,
    fade: Option<Fade>,
    /// Frames into the current crossfade (0..FADE) for the fade-in side.
    fade_pos: u32,
}

impl ShaderInst {
    /// Allocates the voice pool: call off the audio thread.
    pub fn new(shader: AudioShaderRef, rate: f32) -> ShaderInst {
        assert!(shader.kind() == Kind::Instrument, "an effect shader cannot be an instrument");
        ShaderInst {
            ctx: new_ctx(&shader, rate),
            voices: (0..SHADER_VOICES).map(|_| Voice::new(&shader)).collect(),
            shader,
            rate,
            frame: 0,
            bufs: Bufs::new(),
            fade: None,
            fade_pos: FADE,
        }
    }

    pub fn shader(&self) -> &AudioShaderRef {
        &self.shader
    }

    pub fn set_param(&mut self, index: usize, value: f32) {
        self.shader.set_param(&mut self.ctx, index, value);
    }

    pub fn set_param_by_name(&mut self, name: &str, value: f32) -> bool {
        match param_index(&self.shader, name) {
            Some(k) => {
                self.set_param(k, value);
                true
            }
            None => false,
        }
    }

    /// Starts `note` (MIDI, fractions allowed) on voice slot `voice`.
    pub fn note_on(&mut self, note: f32, velocity: f32, voice: u8) {
        let Some(v) = self.voices.get_mut(voice as usize) else { return };
        let seed = (voice as u32 + 1).wrapping_mul(0x85eb_ca6b) ^ self.frame ^ note.to_bits();
        self.shader.note_on(&mut v.state, note, velocity.clamp(0.0, 1.0), self.frame, seed);
        v.active = true;
        v.released = false;
        v.quiet = 0;
        v.released_frames = 0;
        v.fade_in = false;
    }

    /// Changes a sounding voice's pitch without retriggering it (mono
    /// lines, slides); a silent voice starts normally.
    pub fn legato(&mut self, note: f32, velocity: f32, voice: u8) {
        let Some(v) = self.voices.get_mut(voice as usize) else { return };
        if !v.active || v.released {
            self.note_on(note, velocity, voice);
            return;
        }
        self.shader.legato(&mut v.state, note, velocity.clamp(0.0, 1.0));
    }

    /// Silences one voice at once (a choke).
    pub fn stop_voice(&mut self, voice: u8) {
        if let Some(v) = self.voices.get_mut(voice as usize) {
            v.active = false;
        }
    }

    pub fn note_off(&mut self, voice: u8) {
        if let Some(v) = self.voices.get_mut(voice as usize) {
            if v.active {
                self.shader.note_off(&mut v.state);
                v.released = true;
            }
        }
    }

    /// Silences everything at once.
    pub fn kill(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
        }
        if let Some(f) = &mut self.fade {
            f.left = 0;
        }
    }

    pub fn active(&self, voice: u8) -> bool {
        self.voices.get(voice as usize).is_some_and(|v| v.active)
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// ADDS every sounding voice into l/r (any length; rendered in pieces
    /// of at most [`MAX_RENDER`] frames).
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        let mut at = 0;
        while at < n {
            let k = (n - at).min(MAX_RENDER);
            self.render_chunk(&mut l[at..at + k], &mut r[at..at + k]);
            at += k;
        }
    }

    fn render_chunk(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len()).min(MAX_RENDER);
        if n == 0 {
            return;
        }
        let (l, r) = (&mut l[..n], &mut r[..n]);
        self.ctx[CTX_FRAME as usize] = self.frame;
        let fading = self.fade.as_ref().is_some_and(|f| f.left > 0);
        // Fade-in gains for carried voices: position in the crossfade.
        let (g0, dg) = if fading {
            (self.fade_pos as f32 / FADE as f32, 1.0 / FADE as f32)
        } else {
            (1.0, 0.0)
        };
        for v in self.voices.iter_mut() {
            if !v.active {
                continue;
            }
            let peak = if v.fade_in && fading {
                render_voice(&self.shader, &mut self.ctx, v, &mut self.bufs, l, r, g0, dg)
            } else {
                render_voice(&self.shader, &mut self.ctx, v, &mut self.bufs, l, r, 1.0, 0.0)
            };
            end_voice(&self.shader, v, peak, n as u32, self.rate);
        }
        if let Some(f) = &mut self.fade {
            if f.left > 0 {
                f.ctx[CTX_FRAME as usize] = self.frame;
                let (g0, dg) = (f.left as f32 / FADE as f32, -1.0 / FADE as f32);
                for v in f.voices.iter_mut().filter(|v| v.active) {
                    render_voice(&f.shader, &mut f.ctx, v, &mut self.bufs, l, r, g0, dg);
                }
                // Ramps past the end clamp at silence / full.
                let step = (n as u32).min(f.left);
                f.left -= step;
                self.fade_pos = (self.fade_pos + step).min(FADE);
                if f.left == 0 {
                    for v in self.voices.iter_mut() {
                        v.fade_in = false;
                    }
                }
            }
        }
        self.frame = self.frame.wrapping_add(n as u32);
    }

    /// Hot swap: takes over `old`'s sounding voices and frame count. Voices
    /// whose state carries completely continue seamlessly; otherwise the
    /// old program fades out over [`FADE`] frames as this one fades in.
    /// No allocation: `old` keeps empty pools and is dropped by its owner.
    pub fn swap_from(&mut self, old: &mut ShaderInst) {
        self.frame = old.frame;
        let mut all = true;
        for (v, o) in self.voices.iter_mut().zip(old.voices.iter()) {
            if !o.active {
                continue;
            }
            let carried = self.shader.carry_state(&mut v.state, &old.shader, &o.state);
            all &= carried;
            v.active = true;
            v.released = o.released;
            v.quiet = o.quiet;
            v.released_frames = o.released_frames;
            v.fade_in = !carried;
        }
        if !all {
            // The old voices keep playing, fading out.
            let voices = std::mem::replace(&mut old.voices, Vec::new().into_boxed_slice());
            let ctx = std::mem::replace(&mut old.ctx, Vec::new().into_boxed_slice());
            let fade = Fade { shader: old.shader.clone(), ctx, voices, left: FADE };
            // Whatever this instance was fading goes to `old` to be dropped.
            old.fade = self.fade.replace(fade);
            self.fade_pos = 0;
            // Carried voices do not fade: they play on, the old copies of
            // them are silenced.
            if let Some(f) = &mut self.fade {
                for (fv, v) in f.voices.iter_mut().zip(self.voices.iter()) {
                    if v.active && !v.fade_in {
                        fv.active = false;
                    }
                }
            }
        }
    }
}

/// The fading-out side of an effect swap.
struct FxFade {
    shader: AudioShaderRef,
    ctx: Box<[u32]>,
    state: Box<[u32]>,
    scratch: Box<[u32]>,
    left: u32,
}

/// A shader effect: one state, stereo in, stereo out.
pub struct ShaderFx {
    shader: AudioShaderRef,
    ctx: Box<[u32]>,
    state: Box<[u32]>,
    scratch: Box<[u32]>,
    frame: u32,
    bufs: Bufs,
    fade: Option<FxFade>,
    fade_pos: u32,
}

impl ShaderFx {
    /// Allocates the effect state (delay lines included): call off the
    /// audio thread.
    pub fn new(shader: AudioShaderRef, rate: f32) -> ShaderFx {
        assert!(shader.kind() == Kind::Effect, "an instrument shader cannot be an effect");
        ShaderFx {
            ctx: new_ctx(&shader, rate),
            state: new_state(&shader),
            scratch: new_scratch(&shader),
            shader,
            frame: 0,
            bufs: Bufs::new(),
            fade: None,
            fade_pos: FADE,
        }
    }

    pub fn shader(&self) -> &AudioShaderRef {
        &self.shader
    }

    pub fn set_param(&mut self, index: usize, value: f32) {
        self.shader.set_param(&mut self.ctx, index, value);
    }

    pub fn set_param_by_name(&mut self, name: &str, value: f32) -> bool {
        match param_index(&self.shader, name) {
            Some(k) => {
                self.set_param(k, value);
                true
            }
            None => false,
        }
    }

    /// Clears delay lines and filters (a stop / locate).
    pub fn reset(&mut self) {
        self.shader.reset_state(&mut self.state);
    }

    /// Reads in_l/in_r and ADDS the result into out_l/out_r (any length;
    /// processed in pieces of at most [`MAX_RENDER`] frames).
    pub fn render(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        let mut at = 0;
        while at < n {
            let k = (n - at).min(MAX_RENDER);
            self.render_chunk(&in_l[at..at + k], &in_r[at..at + k], &mut out_l[at..at + k], &mut out_r[at..at + k]);
            at += k;
        }
    }

    fn render_chunk(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len()).min(MAX_RENDER);
        if n == 0 {
            return;
        }
        self.ctx[CTX_FRAME as usize] = self.frame;
        let fading = self.fade.as_ref().is_some_and(|f| f.left > 0);
        if !fading {
            self.shader.run(&mut self.ctx, &mut self.state, &mut self.scratch, &in_l[..n], &in_r[..n], &mut out_l[..n], &mut out_r[..n], n);
            self.frame = self.frame.wrapping_add(n as u32);
            return;
        }
        // New side, fading in.
        let (bl, br) = (&mut self.bufs.l[..n], &mut self.bufs.r[..n]);
        bl.fill(0.0);
        br.fill(0.0);
        self.shader.run(&mut self.ctx, &mut self.state, &mut self.scratch, &in_l[..n], &in_r[..n], bl, br, n);
        let (g0, dg) = (self.fade_pos as f32 / FADE as f32, 1.0 / FADE as f32);
        for i in 0..n {
            let g = (g0 + dg * i as f32).min(1.0);
            out_l[i] += self.bufs.l[i] * g;
            out_r[i] += self.bufs.r[i] * g;
        }
        // Old side, fading out.
        let f = self.fade.as_mut().unwrap();
        f.ctx[CTX_FRAME as usize] = self.frame;
        let (bl, br) = (&mut self.bufs.l[..n], &mut self.bufs.r[..n]);
        bl.fill(0.0);
        br.fill(0.0);
        f.shader.run(&mut f.ctx, &mut f.state, &mut f.scratch, &in_l[..n], &in_r[..n], bl, br, n);
        let (g0, dg) = (f.left as f32 / FADE as f32, -1.0 / FADE as f32);
        for i in 0..n {
            let g = (g0 + dg * i as f32).max(0.0);
            out_l[i] += self.bufs.l[i] * g;
            out_r[i] += self.bufs.r[i] * g;
        }
        let step = (n as u32).min(f.left);
        f.left -= step;
        self.fade_pos = (self.fade_pos + step).min(FADE);
        self.frame = self.frame.wrapping_add(n as u32);
    }

    /// Hot swap: carries the delay lines and filters that match by name and
    /// type; crossfades from the old program when anything could not carry.
    /// No allocation: the old state moves into this instance's fade slot.
    pub fn swap_from(&mut self, old: &mut ShaderFx) {
        self.frame = old.frame;
        if self.shader.carry_state(&mut self.state, &old.shader, &old.state) {
            return;
        }
        let fade = FxFade {
            shader: old.shader.clone(),
            ctx: std::mem::replace(&mut old.ctx, Vec::new().into_boxed_slice()),
            state: std::mem::replace(&mut old.state, Vec::new().into_boxed_slice()),
            scratch: std::mem::replace(&mut old.scratch, Vec::new().into_boxed_slice()),
            left: FADE,
        };
        old.fade = self.fade.replace(fade);
        self.fade_pos = 0;
    }
}

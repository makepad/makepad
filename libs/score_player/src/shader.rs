//! Audio shader nodes: instruments and effects written as Splash audio
//! shaders (makepad-script-audio-aot), compiled off the audio thread and
//! run here realtime-safe: fixed voice pools and buffers allocated at
//! construction, no allocation, locking or freeing while rendering.
//!
//! Control blocks (params, `block()`) follow the absolute frame. A schedule
//! supplies the first frame of each pass. Standalone instances count from zero.
//!
//! Hot swap (`swap_from`, at a block boundary): voices and effect state
//! carry over variable by variable (same name and type); when a variable
//! could not carry, the old program keeps rendering for [`FADE`] frames,
//! fading out while the new one fades in.

use makepad_script_audio_aot::{Backend, CC_WORDS, CONTROL_BLOCK, CTX_CC, CTX_FRAME, CTX_PARAMS, CTX_TEMPO};
use makepad_lock_free::Producer;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}};

pub use makepad_script_audio_aot::{align, AlignEdge, AlignError, AlignNode};
pub use makepad_script_audio_aot::{check, check_with, declares_audio_kind, mix_seed, AudioBindings, AudioShader, AudioShaderRef, CheckReport, Expression, Kind, Module, ParamCurve, ParamMetaView, ParamUnit, SampleView, GUIDE};

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
    compile_with(code, &AudioBindings::default())
}

/// Compiles with the host's bindings: sample tables and modules.
pub fn compile_with(code: &str, bindings: &AudioBindings<'_>) -> Result<AudioShaderRef, Vec<CompileError>> {
    use makepad_script_audio_aot::boundary::compile_audio_with;
    let mut compiled = compile_audio_with(code, Backend::Native, false, bindings);
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

/// Non-header words are at their initial values until a render or a carry.
#[derive(Default)]
struct ShaderState {
    words: Box<[u32]>,
    used: bool,
}

impl ShaderState {
    fn new(shader: &AudioShader) -> Result<Self, String> {
        let mut words = Vec::new();
        words.try_reserve_exact(shader.state_words()).map_err(|_| {
            format!("could not allocate {} bytes for shader state", shader.state_words() as u64 * 4)
        })?;
        words.extend_from_slice(shader.state_init());
        Ok(Self { words: words.into_boxed_slice(), used: false })
    }

    fn take_from(&mut self, shader: &AudioShader, old: &mut Self, old_shader: &AudioShader) -> bool {
        if shader.same_state_layout(old_shader) {
            std::mem::swap(&mut self.words, &mut old.words);
            // The two programs may have different initial values even at identical offsets.
            self.used = true;
            old.used = true;
            return true;
        }
        if self.used { shader.reset_state(&mut self.words); }
        self.used = true;
        shader.carry_fresh_state(&mut self.words, old_shader, &old.words)
    }
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
    ("lfo", include_str!("../shaders/lfo.splash")),
    ("filter", include_str!("../shaders/filter.splash")),
    ("drive", include_str!("../shaders/drive.splash")),
    ("phaser", include_str!("../shaders/phaser.splash")),
    ("flanger", include_str!("../shaders/flanger.splash")),
    ("echo", include_str!("../shaders/echo.splash")),
    ("keyed_comp", include_str!("../shaders/keyed_comp.splash")),
    ("spectral_gate", include_str!("../shaders/spectral_gate.splash")),
    ("convolver", include_str!("../shaders/convolver.splash")),
    ("gated_verb", include_str!("../shaders/gated_verb.splash")),
];

/// A library shader's source by name.
pub fn library(name: &str) -> Option<&'static str> {
    LIBRARY.iter().find(|(n, _)| *n == name).map(|(_, code)| *code)
}

/// The song's time over one pass, as a schedule hands it to a shader
/// host. Frames are relative to the pass's first frame, so any host
/// places it on its own timebase by adding its pass frame (a nested
/// player's schedule included).
#[derive(Clone, Copy, Debug, Default)]
pub struct SongTime {
    /// (beat at a whole frame, that frame relative to the pass start,
    /// beats per frame from there, bpm from there). Entry 0 is in effect
    /// at the pass start (its frame is 0 or earlier); later ones are
    /// tempo changes inside the pass, in frame order.
    pub anchors: [(f64, i64, f64, f32); crate::loop_engine::MAX_TEMPO_SPLITS + 1],
    pub count: u8,
    pub beats_per_bar: f32,
    pub playing: bool,
}

impl SongTime {
    /// Writes anchor `k` into a ctx, placed on the host's timebase.
    fn apply(&self, k: usize, shader: &AudioShader, ctx: &mut [u32], pass_frame: u32) {
        let (beat, frame, per_frame, bpm) = self.anchors[k];
        shader.set_time(ctx, bpm, beat, pass_frame.wrapping_add(frame as u32), per_frame, self.beats_per_bar, self.playing);
    }

    /// The pass start: the last anchor in effect there, and the index of
    /// the first one after it. `None` when there is no clock.
    fn at_start(&self) -> Option<(usize, usize)> {
        let count = (self.count as usize).min(self.anchors.len());
        let next = self.anchors[..count].iter().take_while(|a| a.1 <= 0).count();
        (next > 0).then(|| (next - 1, next))
    }
}

/// Voices per shader instrument (as a loop's own pool).
pub const SHADER_VOICES: usize = 12;
/// Primary and fading slots, each with its own probe records.
pub const SHADER_PROBE_VOICES: usize = SHADER_VOICES + SHADER_VOICES / 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Steal {
    #[default]
    Oldest,
    Quietest,
}

impl Steal {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "oldest" => Some(Self::Oldest),
            "quietest" => Some(Self::Quietest),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Oldest => "oldest",
            Self::Quietest => "quietest",
        }
    }
}

/// Crossfade length of a hot swap that could not carry all state.
pub const FADE: u32 = 256;
/// Most frames one render call takes.
pub const MAX_RENDER: usize = 256;

/// Values reached by one probe during a call. Effects use voice `u16::MAX`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeRecord {
    pub frame: u32,
    pub frames: u32,
    pub voice: u16,
    pub probe: u16,
    pub last: f32,
    pub min: f32,
    pub max: f32,
}

pub struct ProbeTap {
    ring: Producer<ProbeRecord>,
    spill: Box<[Option<ProbeRecord>]>,
    // Occupied slots in delivery order. Each index enters at most once
    // until delivered, so reserved storage suffices for every push.
    pending: VecDeque<usize>,
    folded: Arc<AtomicU64>,
}

impl ProbeTap {
    pub fn new(ring: Producer<ProbeRecord>, folded: Arc<AtomicU64>) -> Self {
        Self { ring, spill: Box::default(), pending: VecDeque::new(), folded }
    }

    fn prepare(mut self, probes: usize, voices: usize) -> Result<Self, &'static str> {
        if probes > u16::MAX as usize + 1 {
            return Err("probe indices must fit the record's 16-bit field");
        }
        let slots = voices * probes;
        self.spill = vec![None; slots].into_boxed_slice();
        self.pending = VecDeque::with_capacity(slots);
        Ok(self)
    }

    fn flush(&mut self) -> bool {
        while let Some(&slot) = self.pending.front() {
            if self.ring.push(self.spill[slot].unwrap()).is_err() {
                return false;
            }
            self.spill[slot] = None;
            self.pending.pop_front();
        }
        true
    }

    fn record(&mut self, shader: &AudioShader, ctx: &[u32], voice: u16, frames: usize) {
        let count = shader.probe_count();
        let base = if voice == u16::MAX { 0 } else { voice as usize * count };
        for k in 0..count {
            let Some((last, min, max)) = shader.probe_read(ctx, k) else { continue };
            let frame = ctx[CTX_FRAME as usize];
            let record = ProbeRecord { frame, frames: frames as u32, voice, probe: k as u16, last, min, max };
            if self.flush() && self.ring.push(record).is_ok() {
                continue;
            }
            let slot = base + k;
            if let Some(old) = &mut self.spill[slot] {
                let previous_end = old.frame.wrapping_add(old.frames);
                old.frames = old.frames.saturating_add(frame.wrapping_sub(previous_end)).saturating_add(record.frames);
                old.last = last;
                old.min = old.min.min(min);
                old.max = old.max.max(max);
            } else {
                self.spill[slot] = Some(record);
                self.pending.push_back(slot);
            }
            self.folded.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// After an offline render owns both ends, drain its final folded values.
    pub fn drain_spill(&mut self, mut read: impl FnMut(ProbeRecord)) {
        while let Some(slot) = self.pending.pop_front() {
            read(self.spill[slot].take().unwrap());
        }
    }
}
/// A released voice ends after this many frames below [`SILENT`].
const QUIET_FRAMES: u32 = 128;
const SILENT: f32 = 1e-5;
/// ... or at the latest this long after its release.
const RELEASE_LIMIT_SECS: f32 = 30.0;

struct Voice {
    handle: u32,
    state: ShaderState,
    scratch: Box<[u32]>,
    active: bool,
    released: bool,
    deferred_lift: Option<f32>,
    quiet: u32,
    serial: u32,
    recent_peak: f32,
    fade_left: u32,
    released_frames: u32,
    /// Fades in during a crossfade swap.
    fade_in: bool,
}

impl Voice {
    fn new(shader: &AudioShader) -> Result<Voice, String> {
        Ok(Voice {
            handle: 0,
            state: ShaderState::new(shader)?,
            scratch: new_scratch(shader),
            active: false,
            released: false,
            deferred_lift: None,
            quiet: 0,
            serial: 0,
            recent_peak: 0.0,
            fade_left: 0,
            released_frames: 0,
            fade_in: false,
        })
    }
}

#[derive(Default)]
struct ChannelTables<'a> {
    ins: Vec<&'a [f32]>,
    outs: Vec<&'a mut [f32]>,
}

impl ChannelTables<'_> {
    fn empty<'call>(mut self) -> ChannelTables<'call> {
        self.ins.clear();
        self.outs.clear();
        let mut tables = std::mem::ManuallyDrop::new(self);
        // SAFETY: both allocations are empty. Only the reference lifetimes change.
        // Each Vec takes back its own allocation with the original capacity.
        unsafe {
            ChannelTables {
                ins: Vec::from_raw_parts(tables.ins.as_mut_ptr().cast(), 0, tables.ins.capacity()),
                outs: Vec::from_raw_parts(tables.outs.as_mut_ptr().cast(), 0, tables.outs.capacity()),
            }
        }
    }
}

#[derive(Clone, Copy)]
struct PortChannels {
    first: usize,
    stereo: bool,
}

impl PortChannels {
    // Input indices address the counted table; output indices address extra_out.
    fn new(shader: &AudioShader, index: usize) -> Self {
        let port = shader.port_view(index);
        let main = if port.output && shader.kind() != Kind::Control { 2 } else { 0 };
        Self { first: port.first as usize - main, stereo: port.channels == 2 }
    }
}

struct WiredPort {
    index: usize,
    current: PortChannels,
    previous: Option<PortChannels>,
    data: Box<[f32]>,
}

impl WiredPort {
    fn new(shader: &AudioShader, index: usize, planes: usize) -> Self {
        Self {
            index, current: PortChannels::new(shader, index),
            previous: None, data: vec![0.0; planes * MAX_RENDER].into_boxed_slice(),
        }
    }

    fn bind_previous(&mut self, shader: &AudioShader, old: Option<&PortBuffers>) {
        let port = shader.port_view(self.index);
        self.previous = old.and_then(|old| old.audio_ports.as_ref()).and_then(|ports| ports.get(port.name))
            .and_then(|&(output, channels)| (output == port.output).then_some(channels));
    }

    fn read_output(&mut self, discard: &[f32], at: usize, n: usize, old: bool, gain: Option<(f32, f32)>) {
        let channels = if old { self.previous } else { Some(self.current) };
        let Some(ch) = channels else { return };
        let first = ch.first * MAX_RENDER;
        let right = first + usize::from(ch.stereo) * MAX_RENDER;
        let (l, r) = self.data.split_at_mut(MAX_RENDER);
        if let Some((start, step)) = gain {
            for i in 0..n {
                let g = (start + step * i as f32).clamp(0.0, 1.0);
                if old {
                    l[at + i] += discard[first + i] * g;
                    r[at + i] += discard[right + i] * g;
                } else {
                    l[at + i] = discard[first + i] * g;
                    r[at + i] = discard[right + i] * g;
                }
            }
        } else {
            l[at..at + n].copy_from_slice(&discard[first..first + n]);
            r[at..at + n].copy_from_slice(&discard[right..right + n]);
        }
    }
}

#[derive(Default)]
struct ShaderWires {
    inputs: Vec<WiredPort>,
    outputs: Vec<WiredPort>,
    // Parameter index in the current and fading programs, in source-span order.
    params: Vec<(usize, Option<usize>)>,
}

impl ShaderWires {
    /// The schedule calls this at prepare with validated port indices in op order.
    fn prepare_wires(&mut self, shader: &AudioShader, sides: impl Iterator<Item = usize>, outs: impl Iterator<Item = usize>) {
        self.inputs = sides.map(|i| WiredPort::new(shader, i, 3)).collect();
        self.outputs = outs.map(|i| WiredPort::new(shader, i, 2)).collect();
    }

    fn prepare_param_wires(&mut self, indices: impl Iterator<Item = usize>) {
        self.params = indices.map(|index| (index, None)).collect();
    }

    fn bind_fading_wires(&mut self, shader: &AudioShader, fade: Option<(&AudioShader, &PortBuffers)>) {
        for (index, previous) in &mut self.params {
            *previous = fade.and_then(|(old, _)| param_index(old, shader.param_view(*index).name));
        }
        let old = fade.map(|(_, ports)| ports);
        for wire in self.inputs.iter_mut().chain(&mut self.outputs) {
            wire.bind_previous(shader, old);
        }
    }

    pub fn side_buffer(&mut self, slot: usize) -> (&mut [f32], &mut [f32]) {
        let (l, rest) = self.inputs[slot].data.split_at_mut(MAX_RENDER);
        (l, &mut rest[..MAX_RENDER])
    }

    pub fn output_buffer(&self, slot: usize) -> (&[f32], &[f32]) {
        self.outputs[slot].data.split_at(MAX_RENDER)
    }

    fn prepare_mono(&mut self, n: usize, direct: &DirectSide<'_>) {
        for (slot, side) in self.inputs.iter_mut().enumerate() {
            if !side.current.stereo || side.previous.is_some_and(|p| !p.stereo) {
                let (stereo, mono) = side.data.split_at_mut(2 * MAX_RENDER);
                let (l, r) = direct(slot).unwrap_or_else(|| stereo.split_at(MAX_RENDER));
                for i in 0..n { mono[i] = ((f64::from(l[i]) + f64::from(r[i])) * 0.5) as f32; }
            }
        }
    }
}

#[derive(Default)]
struct PortBuffers {
    tables: ChannelTables<'static>,
    extra_out: Vec<f32>,
    // Prepared once. A fade can bind the new wiring by name in linear time
    // even when its programs declare the same ports in different orders.
    // Empty fade slots are constructed on the audio thread. Leave their hash
    // builder uninitialized until a prepared PortBuffers moves into the slot.
    audio_ports: Option<HashMap<String, (bool, PortChannels)>>,
}

impl PortBuffers {
    fn new(shader: &AudioShader) -> Self {
        if shader.port_count() == 0 {
            return Self::default();
        }
        Self {
            tables: ChannelTables {
                ins: Vec::with_capacity(shader.input_channels()),
                outs: Vec::with_capacity(shader.output_channels()),
            },
            extra_out: vec![0.0; (shader.output_channels() - if shader.kind() == Kind::Control { 0 } else { 2 }) * MAX_RENDER],
            audio_ports: Some({
                (0..shader.port_count()).map(|i| {
                    let p = shader.port_view(i);
                    (p.name.to_owned(), (p.output, PortChannels::new(shader, i)))
                })
                    .collect()
            }),
        }
    }

    fn clear(&mut self, n: usize) {
        for channel in self.extra_out.chunks_exact_mut(MAX_RENDER) {
            channel[..n].fill(0.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run(&mut self, shader: &AudioShader, ctx: &mut [u32], state: &mut [u32], scratch: &mut [u32],
        ins: [&[f32]; 2], outs: [&mut [f32]; 2], silence: &[f32], n: usize, sides: &[WiredPort], at: usize, old: bool, direct: &DirectSide<'_>) {
        if shader.port_count() == 0 {
            let [l, r] = outs;
            shader.run(ctx, state, scratch, ins[0], ins[1], l, r, n);
            return;
        }
        let mut tables = std::mem::take(&mut self.tables).empty();
        if shader.kind() != Kind::Control { tables.ins.extend(ins); }
        tables.ins.resize(shader.input_channels(), &silence[..n]);
        for (slot, side) in sides.iter().enumerate() {
            let Some(ch) = (if old { side.previous } else { Some(side.current) }) else { continue };
            if ch.stereo {
                let (l, r) = direct(slot).unwrap_or_else(|| side.data.split_at(MAX_RENDER));
                tables.ins[ch.first] = &l[at..at + n];
                tables.ins[ch.first + 1] = &r[at..at + n];
            } else {
                tables.ins[ch.first] = &side.data[2 * MAX_RENDER + at..2 * MAX_RENDER + at + n];
            }
        }
        if shader.kind() != Kind::Control { tables.outs.extend(outs); }
        tables.outs.extend(self.extra_out.chunks_exact_mut(MAX_RENDER).map(|ch| &mut ch[..n]));
        shader.run_ports(ctx, state, scratch, &tables.ins, &mut tables.outs, n);
        self.tables = tables.empty();
    }
}

/// A single stereo source can be borrowed directly from the pass's arena.
pub type DirectSide<'a> = dyn Fn(usize) -> Option<(&'a [f32], &'a [f32])> + 'a;

struct Fade {
    ports: PortBuffers,
    shader: AudioShaderRef,
    probes: Option<ProbeTap>,
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

struct InstBufs {
    voice: Bufs,
    wires: ShaderWires,
}

/// Renders one voice into the work buffers and adds it to l/r with a
/// per-frame gain ramp. Returns the unmixed peak and the number of frames played.
fn render_voice(shader: &AudioShader, ports: &mut PortBuffers, ctx: &mut [u32], v: &mut Voice, bufs: &mut InstBufs, l: &mut [f32], r: &mut [f32], g0: f32, dg: f32, decay: f32, pass_at: usize, old: bool, direct: &DirectSide<'_>) -> (f32, usize) {
    let n = if v.fade_left > 0 { l.len().min(v.fade_left as usize) } else { l.len() };
    let InstBufs { voice: bufs, wires } = bufs;
    let (bl, br) = (&mut bufs.l[..n], &mut bufs.r[..n]);
    bl.fill(0.0);
    br.fill(0.0);
    let z = &bufs.zeros[..n];
    // Extra outputs are additive just like main. Each run starts with fresh
    // per-voice samples before its gains are applied to the instance sum.
    if !wires.outputs.is_empty() { ports.clear(n); }
    v.state.used = true;
    ports.run(shader, ctx, &mut v.state.words, &mut v.scratch, [z,z], [bl,br], z, n, &wires.inputs, pass_at, old, direct);
    let fade_left = v.fade_left;
    let gain = |i: usize| {
        let g = (g0 + dg * i as f32).clamp(0.0, 1.0);
        if fade_left > 0 { g * ((fade_left - i as u32) as f32 / FADE as f32) } else { g }
    };
    let mut peak = 0.0f32;
    for i in 0..n {
        let g = gain(i);
        let magnitude = bufs.l[i].abs().max(bufs.r[i].abs());
        peak = peak.max(magnitude);
        v.recent_peak = (v.recent_peak - decay).max(magnitude);
        l[i] += bufs.l[i] * g;
        r[i] += bufs.r[i] * g;
    }
    for port in &mut wires.outputs {
        let Some(ch) = (if old { port.previous } else { Some(port.current) }) else { continue };
        let first = ch.first * MAX_RENDER;
        let right = first + usize::from(ch.stereo) * MAX_RENDER;
        let (l, r) = port.data.split_at_mut(MAX_RENDER);
        let mut recent = 0.0f32;
        for i in 0..n {
            let g = gain(i);
            let (left, right) = (ports.extra_out[first + i], ports.extra_out[right + i]);
            l[pass_at + i] += left * g;
            r[pass_at + i] += right * g;
            let magnitude = left.abs().max(right.abs());
            peak = peak.max(magnitude);
            recent = (recent - decay).max(magnitude);
        }
        v.recent_peak = v.recent_peak.max(recent);
    }
    if v.fade_left > 0 {
        v.fade_left -= n as u32;
        v.active = v.fade_left > 0;
    }
    (peak, n)
}

fn end_voice(shader: &AudioShader, v: &mut Voice, peak: f32, n: u32, rate: f32) {
    if shader.stopped(&v.state.words) {
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
    pedal: bool,
    steal: Steal,
    serial: u32,
    cuts: u64,
    peak_decay: f32,
    seed: u32,
    shader: AudioShaderRef,
    probes: Option<ProbeTap>,
    ctx: Box<[u32]>,
    voices: Box<[Voice]>,
    rate: f32,
    frame: u32,
    bufs: InstBufs,
    ports: PortBuffers,
    fade: Option<Fade>,
    /// Frames into the current crossfade (0..FADE) for the fade-in side.
    fade_pos: u32,
    time: SongTime,
    pass_frame: u32,
    next_anchor: u8,
}

impl ShaderInst {
    /// Allocates the voice pool: call off the audio thread.
    pub fn new(shader: AudioShaderRef, rate: f32) -> ShaderInst {
        Self::try_new(shader, rate).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Prepares state off the audio thread and reports the byte count on allocation failure.
    pub fn try_new(shader: AudioShaderRef, rate: f32) -> Result<Self, String> {
        assert!(shader.kind() == Kind::Instrument, "an effect shader cannot be an instrument");
        Ok(ShaderInst {
            pedal: false,
            steal: Steal::Oldest,
            serial: 0,
            cuts: 0,
            // A full-scale peak falls to silence in one tenth of a second.
            peak_decay: 10.0 / rate,
            seed: 0,
            ports: PortBuffers::new(&shader),
            ctx: new_ctx(&shader, rate),
            voices: (0..SHADER_PROBE_VOICES).map(|_| Voice::new(&shader)).collect::<Result<_, _>>()?,
            shader,
            rate,
            probes: None,
            frame: 0,
            bufs: InstBufs { voice: Bufs::new(), wires: ShaderWires::default() },
            fade: None,
            fade_pos: FADE,
            time: SongTime::default(),
            pass_frame: 0,
            next_anchor: 0,
        })
    }

    pub fn set_seed(&mut self, seed: u32) {
        self.seed = seed;
        self.shader.set_instance(&mut self.ctx, seed);
    }

    pub(crate) fn same_program_and_seed(&self, other: &Self) -> bool {
        self.shader.ptr_eq(&other.shader) && self.seed == other.seed
    }

    pub fn shader(&self) -> &AudioShaderRef {
        &self.shader
    }

    /// Set the absolute frame of the next sample.
    pub fn sync_frame(&mut self, frame: u32) {
        self.frame = frame;
    }

    /// The song's time over the pass that starts at the synced frame:
    /// a schedule calls it right after `sync_frame`.
    pub fn set_song_time(&mut self, time: &SongTime) {
        self.time = *time;
        self.pass_frame = self.frame;
        self.next_anchor = 0;
        if let Some((last, next)) = time.at_start() {
            self.apply_anchor(last);
            self.next_anchor = next as u8;
        }
    }

    /// Writes anchor `k` into the instance's ctx, and the fading one's.
    fn apply_anchor(&mut self, k: usize) {
        self.time.apply(k, &self.shader, &mut self.ctx, self.pass_frame);
        if let Some(f) = self.fade.as_mut().filter(|f| f.left > 0) {
            self.time.apply(k, &f.shader, &mut f.ctx, self.pass_frame);
        }
    }

    /// Attach off the audio thread, before this instance plays.
    pub fn attach_probes(&mut self, tap: ProbeTap) -> Result<(), &'static str> {
        self.probes = Some(tap.prepare(self.shader.probe_count(), self.voices.len())?);
        Ok(())
    }

    pub fn probe_tap_mut(&mut self) -> Option<&mut ProbeTap> {
        self.probes.as_mut()
    }

    pub(crate) fn swap_probes(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.probes, &mut other.probes);
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

    pub fn set_steal(&mut self, steal: Steal) {
        self.steal = steal;
    }

    pub fn steal_cuts(&self) -> u64 {
        self.cuts
    }

    pub fn pick_voice(&self) -> u8 {
        let voices = &self.voices[..SHADER_VOICES];
        if let Some(free) = voices.iter().position(|v| !v.active) {
            return free as u8;
        }
        voices.iter().enumerate().min_by(|(_, a), (_, b)| {
            let released = |v: &Voice| v.released || v.deferred_lift.is_some();
            released(b).cmp(&released(a))
                .then_with(|| match self.steal {
                    Steal::Quietest => a.recent_peak.total_cmp(&b.recent_peak),
                    Steal::Oldest => std::cmp::Ordering::Equal,
                })
                .then_with(|| self.serial.wrapping_sub(b.serial).cmp(&self.serial.wrapping_sub(a.serial)))
        }).unwrap().0 as u8
    }

    pub(crate) fn clear_spares(&mut self) {
        for v in &mut self.voices[SHADER_VOICES..] {
            v.active = false;
        }
        if let Some(f) = self.fade.as_mut().filter(|f| f.left > 0) {
            for v in &mut f.voices[SHADER_VOICES..] {
                v.active = false;
            }
        }
    }

    /// Starts `note` (MIDI, fractions allowed) on voice slot `voice`.
    pub fn note_on(&mut self, note: f32, velocity: f32, voice: u8, event_seed: u32) {
        let (voices, spares) = self.voices.split_at_mut(SHADER_VOICES);
        let Some(v) = voices.get_mut(voice as usize) else { return };
        if v.active {
            if let Some(spare) = spares.iter_mut().find(|s| !s.active) {
                std::mem::swap(v, spare);
                spare.fade_left = FADE;
            } else {
                self.cuts = self.cuts.saturating_add(1);
            }
        }
        v.handle = 0;
        v.serial = self.serial;
        self.serial = self.serial.wrapping_add(1);
        v.recent_peak = 0.0;
        v.fade_left = 0;
        let seed = mix_seed(self.seed, event_seed);
        v.state.used = false;
        self.shader.note_on(&mut v.state.words, note, velocity.clamp(0.0, 1.0), self.frame, seed, u32::from(voice));
        v.active = true;
        v.released = false;
        v.deferred_lift = None;
        v.quiet = 0;
        v.released_frames = 0;
        v.fade_in = false;
    }

    pub(crate) fn handle(&self, voice: usize) -> u32 { self.voices[voice].handle }

    pub(crate) fn set_handle(&mut self, voice: usize, handle: u32) { self.voices[voice].handle = handle; }

    pub(crate) fn glide_held(&mut self, voice: usize, note: f32, by: f32, velocity: f32, seconds: f32) {
        use makepad_script_audio_aot::header;
        let state = &mut self.voices[voice].state.words;
        if velocity.is_finite() {
            state[header::VELOCITY as usize] = velocity.clamp(0.0, 1.0).to_bits();
        }
        let target = if note.is_finite() {
            note
        } else if by.is_finite() {
            f32::from_bits(state[header::GLIDE_NOTE as usize]) + by
        } else {
            return;
        };
        let velocity = f32::from_bits(state[header::VELOCITY as usize]);
        self.shader.glide(state, target, velocity, seconds, self.rate);
    }

    /// Changes a sounding voice's pitch without retriggering it (mono
    /// lines, slides); a silent voice starts normally.
    pub fn legato(&mut self, note: f32, velocity: f32, voice: u8, event_seed: u32) {
        self.glide(note, velocity, voice, event_seed, 0.0);
    }

    pub fn glide(&mut self, note: f32, velocity: f32, voice: u8, event_seed: u32, seconds: f32) {
        let Some(v) = self.voices[..SHADER_VOICES].get_mut(voice as usize) else { return };
        if !v.active || v.released || v.deferred_lift.is_some() {
            self.note_on(note, velocity, voice, event_seed);
            return;
        }
        self.shader.glide(&mut v.state.words, note, velocity.clamp(0.0, 1.0), seconds, self.rate);
    }

    /// Silences one voice at once (a choke).
    pub fn stop_voice(&mut self, voice: u8) {
        if let Some(v) = self.voices[..SHADER_VOICES].get_mut(voice as usize) {
            v.active = false;
        }
    }

    pub fn set_cc(&mut self, cc: u8, value: f32) {
        self.shader.set_cc(&mut self.ctx, cc, value);
        if let Some(f) = self.fade.as_mut().filter(|f| f.left > 0) {
            f.shader.set_cc(&mut f.ctx, cc, value);
        }
    }

    pub fn set_expression(&mut self, voice: u8, which: Expression, value: f32) {
        if let Some(v) = self.voices[..SHADER_VOICES].get_mut(voice as usize).filter(|v| v.active) {
            self.shader.set_expression(&mut v.state.words, which, value);
        }
    }

    /// A channel or key message also reaches the stolen notes still fading.
    pub(crate) fn update_expression(&mut self, which: Expression, value: impl Fn(u32) -> Option<f32>) {
        let update = |shader: &AudioShader, voices: &mut [Voice]| {
            for v in voices.iter_mut().filter(|v| v.active) {
                if let Some(value) = value(v.handle) {
                    shader.set_expression(&mut v.state.words, which, value);
                }
            }
        };
        update(&self.shader, &mut self.voices);
        if let Some(f) = self.fade.as_mut().filter(|f| f.left > 0) {
            update(&f.shader, &mut f.voices);
        }
    }

    pub fn note_off(&mut self, voice: u8, release_velocity: f32) {
        if let Some(v) = self.voices[..SHADER_VOICES].get_mut(voice as usize).filter(|v| v.active) {
            if self.pedal {
                v.deferred_lift = Some(release_velocity);
            } else {
                self.shader.note_off(&mut v.state.words, release_velocity);
                v.released = true;
            }
        }
    }

    pub fn set_pedal(&mut self, down: bool) {
        if self.pedal && !down {
            let release = |shader: &AudioShader, voices: &mut [Voice]| {
                for v in voices.iter_mut().filter(|v| v.active) {
                    if let Some(lift) = v.deferred_lift.take() {
                        shader.note_off(&mut v.state.words, lift);
                        v.released = true;
                    }
                }
            };
            release(&self.shader, &mut self.voices);
            if let Some(f) = self.fade.as_mut().filter(|f| f.left > 0) {
                release(&f.shader, &mut f.voices);
            }
        }
        self.pedal = down;
    }

    /// Silences everything at once.
    pub fn kill(&mut self) {
        self.pedal = false;
        for v in self.voices.iter_mut() {
            v.active = false;
        }
        if let Some(f) = &mut self.fade {
            f.left = 0;
        }
    }

    pub fn active(&self, voice: u8) -> bool {
        self.voices[..SHADER_VOICES].get(voice as usize).is_some_and(|v| v.active)
    }

    /// The schedule calls this at prepare with validated port indices in op order.
    pub fn prepare_wires(&mut self, sides: impl Iterator<Item = usize>, outs: impl Iterator<Item = usize>) {
        self.bufs.wires.prepare_wires(&self.shader, sides, outs);
    }

    pub fn prepare_param_wires(&mut self, indices: impl Iterator<Item = usize>) {
        self.bufs.wires.prepare_param_wires(indices);
    }

    fn bind_fading_wires(&mut self) {
        let fade = self.fade.as_ref().filter(|f| f.left > 0).map(|f| (&*f.shader, &f.ports));
        self.bufs.wires.bind_fading_wires(&self.shader, fade);
    }

    /// A wire edit can keep a running program, including a fade already underway.
    pub fn take_prepared_wires(&mut self, fresh: &mut Self) {
        // Disconnected targets return to the newly prepared literal or default.
        // A continuing wire supplies its next target on the next block boundary.
        for &(index, previous) in &self.bufs.wires.params {
            if fresh.bufs.wires.params.iter().any(|&(i, _)| i == index) { continue; }
            let value = f32::from_bits(fresh.ctx[CTX_PARAMS as usize + index]);
            self.shader.set_param(&mut self.ctx, index, value);
            if let (Some(i), Some(f)) = (previous, self.fade.as_mut().filter(|f| f.left > 0)) {
                f.shader.set_param(&mut f.ctx, i, value);
            }
        }
        std::mem::swap(&mut self.bufs.wires, &mut fresh.bufs.wires);
        self.bind_fading_wires();
    }

    pub fn side_buffer(&mut self, slot: usize) -> (&mut [f32], &mut [f32]) {
        self.bufs.wires.side_buffer(slot)
    }

    pub fn output_buffer(&self, slot: usize) -> (&[f32], &[f32]) {
        self.bufs.wires.output_buffer(slot)
    }

    pub fn active_voices(&self) -> usize {
        self.voices[..SHADER_VOICES].iter().filter(|v| v.active).count()
    }

    /// ADDS every sounding voice into l/r (any length; rendered in pieces
    /// of at most [`MAX_RENDER`] frames, cut where the song's tempo changes).
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        // Standalone callers may render more than one host pass.
        for (l, r) in l.chunks_mut(MAX_RENDER).zip(r.chunks_mut(MAX_RENDER)) {
            let n = l.len().min(r.len());
            self.begin_pass(n, &|_| None);
            self.render_piece(&mut l[..n], &mut r[..n], 0, &|_| None, &|_, _| 0.0);
        }
    }

    pub fn begin_pass(&mut self, n: usize, direct: &DirectSide<'_>) {
        self.bufs.wires.prepare_mono(n, direct);
        for port in &mut self.bufs.wires.outputs {
            let (l, r) = port.data.split_at_mut(MAX_RENDER);
            l[..n].fill(0.0);
            r[..n].fill(0.0);
        }
    }

    /// A subrange of the prepared pass, split by loop events or tempo changes.
    pub fn render_piece(&mut self, l: &mut [f32], r: &mut [f32], pass_at: usize, direct: &DirectSide<'_>, value: &dyn Fn(usize, usize) -> f32) {
        let n = l.len().min(r.len());
        let mut at = 0;
        while at < n {
            let mut k = (n - at).min(MAX_RENDER);
            if !self.bufs.wires.params.is_empty() {
                let phase = self.frame % CONTROL_BLOCK;
                k = k.min((CONTROL_BLOCK - phase) as usize);
                if phase == 0 {
                    for (slot, &(index, previous)) in self.bufs.wires.params.iter().enumerate() {
                        let target = value(slot, pass_at + at);
                        self.shader.set_param(&mut self.ctx, index, target);
                        if let (Some(i), Some(f)) = (previous, self.fade.as_mut().filter(|f| f.left > 0)) {
                            f.shader.set_param(&mut f.ctx, i, target);
                        }
                    }
                }
            }
            let rel = self.frame.wrapping_sub(self.pass_frame) as i64;
            while (self.next_anchor as usize) < self.time.count as usize {
                let a = self.time.anchors[self.next_anchor as usize].1;
                if a <= rel {
                    self.apply_anchor(self.next_anchor as usize);
                    self.next_anchor += 1;
                } else {
                    k = k.min((a - rel) as usize);
                    break;
                }
            }
            self.render_chunk(&mut l[at..at + k], &mut r[at..at + k], pass_at + at, direct);
            at += k;
        }
    }

    fn render_chunk(&mut self, l: &mut [f32], r: &mut [f32], pass_at: usize, direct: &DirectSide<'_>) {
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
        if let Some(tap) = &mut self.probes {
            tap.flush();
        }
        for (voice, v) in self.voices.iter_mut().enumerate() {
            if !v.active {
                continue;
            }
            let (peak, frames) = if v.fade_in && fading {
                render_voice(&self.shader, &mut self.ports, &mut self.ctx, v, &mut self.bufs, l, r, g0, dg, self.peak_decay, pass_at, false, direct)
            } else {
                render_voice(&self.shader, &mut self.ports, &mut self.ctx, v, &mut self.bufs, l, r, 1.0, 0.0, self.peak_decay, pass_at, false, direct)
            };
            if let Some(tap) = &mut self.probes {
                tap.record(&self.shader, &self.ctx, voice as u16, frames);
            }
            end_voice(&self.shader, v, peak, frames as u32, self.rate);
        }
        if let Some(f) = &mut self.fade {
            if f.left > 0 {
                f.ctx[CTX_FRAME as usize] = self.frame;
                let (g0, dg) = (f.left as f32 / FADE as f32, -1.0 / FADE as f32);
                for (voice, v) in f.voices.iter_mut().enumerate().filter(|(_, v)| v.active) {
                    let (_, frames) = render_voice(&f.shader, &mut f.ports, &mut f.ctx, v, &mut self.bufs, l, r, g0, dg, self.peak_decay, pass_at, true, direct);
                    if let Some(tap) = &mut f.probes {
                        tap.record(&f.shader, &f.ctx, voice as u16, frames);
                    }
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
    /// The owner retires every replaced pool off the audio thread.
    pub fn swap_from(&mut self, old: &mut ShaderInst) {
        self.pedal = old.pedal;
        self.serial = old.serial;
        self.cuts = old.cuts;
        self.frame = old.frame;
        self.time = old.time;
        self.pass_frame = old.pass_frame;
        self.next_anchor = old.next_anchor;
        let cc = CTX_TEMPO as usize..(CTX_CC + CC_WORDS) as usize;
        self.ctx[cc.clone()].copy_from_slice(&old.ctx[cc]);
        self.shader.set_instance(&mut self.ctx, self.seed);
        // A note may start after the swap but before the next absolute block.
        for &(index, _) in &self.bufs.wires.params {
            let name = self.shader.param_view(index).name;
            if let Some(&(i, _)) = old.bufs.wires.params.iter().find(|&&(i, _)| old.shader.param_view(i).name == name) {
                self.shader.set_param(&mut self.ctx, index, f32::from_bits(old.ctx[CTX_PARAMS as usize + i]));
            }
        }
        let mut all = true;
        for (v, o) in self.voices.iter_mut().zip(old.voices.iter_mut()) {
            if !o.active {
                continue;
            }
            let carried = v.state.take_from(&self.shader, &mut o.state, &old.shader);
            all &= carried;
            v.active = true;
            v.handle = o.handle;
            v.released = o.released;
            v.deferred_lift = o.deferred_lift;
            v.quiet = o.quiet;
            v.serial = o.serial;
            v.fade_left = o.fade_left;
            v.recent_peak = o.recent_peak;
            v.released_frames = o.released_frames;
            v.fade_in = !carried;
        }
        if !all {
            // Old may itself own an earlier fade. Leave it there for retirement.
            let fade = self.fade.get_or_insert_with(|| Fade {
                shader: old.shader.clone(), ports: PortBuffers::default(), probes: None, ctx: Box::default(), voices: Box::default(), left: 0,
            });
            std::mem::swap(&mut fade.shader, &mut old.shader);
            std::mem::swap(&mut fade.ports, &mut old.ports);
            std::mem::swap(&mut fade.probes, &mut old.probes);
            std::mem::swap(&mut fade.ctx, &mut old.ctx);
            std::mem::swap(&mut fade.voices, &mut old.voices);
            fade.left = FADE;
            self.fade_pos = 0;
            // Carried voices do not fade: they play on, the old copies of
            // them are silenced.
            for (fv, v) in fade.voices.iter_mut().zip(self.voices.iter()) {
                if v.active && !v.fade_in {
                    fv.active = false;
                }
            }
        }
        self.bind_fading_wires();
    }
}

/// The fading-out side of an effect swap.
struct FxFade {
    ports: PortBuffers,
    shader: AudioShaderRef,
    probes: Option<ProbeTap>,
    ctx: Box<[u32]>,
    state: ShaderState,
    scratch: Box<[u32]>,
    left: u32,
}

/// Storage reserved by the host for a shader's input latency. While an old
/// offset can still be heard, a subsequent prepare must cover it too.
#[derive(Clone, Debug, Default)]
pub struct SongTimeDelay {
    offset: u64,
    width: u64,
    wider_pending: Option<Arc<AtomicBool>>,
    dropped: Option<Arc<AtomicU64>>,
}

impl SongTimeDelay {
    pub fn new(offset: u64, base: Option<&Self>) -> Self {
        let previous = base.map_or(0, |old| {
            if old.wider_pending.as_ref().is_some_and(|p| p.load(Ordering::Acquire)) {
                old.width
            } else {
                old.offset
            }
        });
        let width = offset.max(previous);
        Self {
            offset,
            width,
            wider_pending: (previous > offset).then(|| Arc::new(AtomicBool::new(true))),
            dropped: (width != 0).then(|| base.and_then(|old| old.dropped.clone()).unwrap_or_default()),
        }
    }

    fn capacity(&self) -> Option<usize> {
        if self.width == 0 { return Some(0); }
        usize::try_from(self.width).ok()?.checked_add(crate::loop_engine::MAX_TEMPO_SPLITS + 2)
    }

    /// Include the whole ring in the schedule's budget before allocating it.
    pub fn bytes(&self) -> Option<usize> {
        self.capacity()?.checked_mul(std::mem::size_of::<TimeRecord>())
    }

    /// Records lost to an undersized ring in this delay's history.
    pub fn dropped_records(&self) -> u64 {
        self.dropped.as_ref().map_or(0, |count| count.load(Ordering::Relaxed))
    }

    fn count_dropped(&self, records: usize) {
        if let Some(count) = &self.dropped { count.fetch_add(records as u64, Ordering::Relaxed); }
    }
}

// Compare the complete anchored line by its bits. Only a changed line gets
// an effective frame: seeing the same anchor at a later pass is no event.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SongAnchor {
    beat: u64,
    step: u64,
    anchor: u32,
    bpm: u32,
    beats_per_bar: u32,
    playing: bool,
}

impl SongAnchor {
    fn apply(self, shader: &AudioShader, ctx: &mut [u32], delay: u64) {
        shader.set_time(
            ctx, f32::from_bits(self.bpm), f64::from_bits(self.beat), self.anchor.wrapping_add(delay as u32),
            f64::from_bits(self.step), f32::from_bits(self.beats_per_bar), self.playing,
        );
    }
}

#[derive(Clone, Copy)]
struct TimeRecord {
    effective: u64,
    line: SongAnchor,
}

#[derive(Default)]
struct DelayedSongTime {
    plan: SongTimeDelay,
    records: VecDeque<TimeRecord>,
    current: usize,
    // Extend the shader's wrapping frame for ordering events and long holds.
    now: u64,
    applied: u64,
    switch_at: Option<u64>,
}

impl DelayedSongTime {
    fn sync(&mut self, frame: u32) {
        self.now += u64::from(frame.wrapping_sub(self.now as u32));
    }

    fn finish_hold(&mut self) {
        if self.switch_at.is_some_and(|end| self.now >= end) {
            self.applied = self.plan.offset;
            self.switch_at = None;
            self.current = 0;
            if let Some(pending) = &self.plan.wider_pending { pending.store(false, Ordering::Release); }
        }
    }

    fn trim(&mut self) {
        let oldest = self.now.saturating_sub(self.applied.max(self.plan.offset));
        while self.records.get(1).is_some_and(|second| second.effective <= oldest) {
            self.records.pop_front();
            self.current = self.current.saturating_sub(1);
        }
    }

    fn record_pass(&mut self, time: &SongTime, frame: u32) -> bool {
        self.sync(frame);
        self.finish_hold();
        if self.applied == 0 && self.switch_at.is_none() {
            self.records.clear();
            return false;
        }
        self.trim();
        if let Some((last, _)) = time.at_start() {
            let end = usize::from(time.count).min(time.anchors.len());
            for &(beat, relative, step, bpm) in &time.anchors[last..end] {
                let line = SongAnchor {
                    anchor: frame.wrapping_add(relative as u32),
                    beat: beat.to_bits(), step: step.to_bits(), bpm: bpm.to_bits(),
                    beats_per_bar: time.beats_per_bar.to_bits(), playing: time.playing,
                };
                let effective = self.now + relative.max(0) as u64;
                if let Some(newest) = self.records.back_mut() {
                    if newest.line == line { continue; }
                    if newest.effective == effective {
                        newest.line = line;
                        continue;
                    }
                }
                // At most one record per frame in the window, its predecessor,
                // and this pass's anchors. Never grow a ring on the audio thread.
                let record = TimeRecord { effective, line };
                if self.records.len() == self.records.capacity() {
                    debug_assert!(false, "song-time ring exhausted its prepared capacity");
                    if let Some(newest) = self.records.back_mut() {
                        *newest = record;
                        // The replacement may not be due yet.
                        self.current = self.current.min(self.records.len().saturating_sub(2));
                    }
                    self.plan.count_dropped(1);
                    continue;
                }
                self.records.push_back(record);
            }
        }
        true
    }

    fn piece(&mut self, max: usize) -> (usize, Option<SongAnchor>) {
        self.finish_hold();
        let mut end = (self.now + max as u64).min(self.switch_at.unwrap_or(u64::MAX));
        while let Some(next) = self.records.get(self.current + 1) {
            let due = next.effective + self.applied;
            if due > self.now {
                end = end.min(due);
                break;
            }
            self.current += 1;
        }
        let count = (end - self.now) as usize;
        self.now = end;
        (count, self.records.get(self.current).map(|record| record.line))
    }

    fn take_from(&mut self, old: &mut Self, frame: u32) {
        old.sync(frame);
        old.finish_hold();
        old.trim();
        self.now = old.now;
        self.applied = old.applied;
        self.switch_at = if self.applied == self.plan.offset {
            None
        } else if self.plan.offset == old.plan.offset {
            old.switch_at
        } else {
            Some(self.now + self.plan.offset + u64::from(FADE))
        };
        self.current = 0;
        self.records.clear();
        if self.records.capacity() != 0 {
            // trim() leaves exactly the old window, even if prepare raced a
            // completing hold. The new allocation covers every surviving tap.
            let dropped = old.records.len().saturating_sub(self.records.capacity());
            debug_assert_eq!(dropped, 0, "song-time history exceeds its prepared capacity");
            if dropped != 0 { self.plan.count_dropped(dropped); }
            self.records.extend(old.records.iter().skip(dropped).copied());
        }
        if self.applied <= self.plan.offset {
            if let Some(pending) = &self.plan.wider_pending { pending.store(false, Ordering::Release); }
        }
    }
}

/// A shader effect: one state, stereo in, stereo out.
pub struct ShaderFx {
    seed: Option<u32>,
    shader: AudioShaderRef,
    probes: Option<ProbeTap>,
    ctx: Box<[u32]>,
    state: ShaderState,
    scratch: Box<[u32]>,
    frame: u32,
    bufs: Bufs,
    ports: PortBuffers,
    wires: ShaderWires,
    fade: Option<FxFade>,
    fade_pos: u32,
    delayed: DelayedSongTime,
    time: SongTime,
    pass_frame: u32,
    next_anchor: u8,
}

impl ShaderFx {
    /// Allocates the effect state (delay lines included): call off the
    /// audio thread.
    pub fn new(shader: AudioShaderRef, rate: f32) -> ShaderFx {
        Self::try_new(shader, rate).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Prepares state off the audio thread and reports the byte count on allocation failure.
    pub fn try_new(shader: AudioShaderRef, rate: f32) -> Result<Self, String> {
        assert!(shader.kind() != Kind::Instrument, "an instrument shader cannot be an effect");
        Ok(ShaderFx {
            seed: None,
            ports: PortBuffers::new(&shader),
            wires: ShaderWires::default(),
            ctx: new_ctx(&shader, rate),
            state: ShaderState::new(&shader)?,
            scratch: new_scratch(&shader),
            shader,
            probes: None,
            frame: 0,
            bufs: Bufs::new(),
            fade: None,
            fade_pos: FADE,
            delayed: DelayedSongTime::default(),
            time: SongTime::default(),
            pass_frame: 0,
            next_anchor: 0,
        })
    }

    /// The schedule calls this at prepare with validated port indices in op order.
    pub fn prepare_wires(&mut self, sides: impl Iterator<Item = usize>, outs: impl Iterator<Item = usize>) {
        self.wires.prepare_wires(&self.shader, sides, outs);
    }

    pub fn prepare_param_wires(&mut self, indices: impl Iterator<Item = usize>) {
        self.wires.prepare_param_wires(indices);
    }

    fn bind_fading_wires(&mut self) {
        let fade = self.fade.as_ref().filter(|f| f.left > 0).map(|f| (&*f.shader, &f.ports));
        self.wires.bind_fading_wires(&self.shader, fade);
    }

    /// A wire edit can keep a running program, including a fade already underway.
    pub fn take_prepared_wires(&mut self, fresh: &mut Self) {
        // Disconnected targets return to the newly prepared literal or default.
        // A continuing wire supplies its next target on the next block boundary.
        for &(index, previous) in &self.wires.params {
            let value = f32::from_bits(fresh.ctx[CTX_PARAMS as usize + index]);
            self.shader.set_param(&mut self.ctx, index, value);
            if let (Some(i), Some(f)) = (previous, self.fade.as_mut().filter(|f| f.left > 0)) {
                f.shader.set_param(&mut f.ctx, i, value);
            }
        }
        std::mem::swap(&mut self.wires, &mut fresh.wires);
        self.bind_fading_wires();
    }

    pub fn side_buffer(&mut self, slot: usize) -> (&mut [f32], &mut [f32]) {
        self.wires.side_buffer(slot)
    }

    pub fn output_buffer(&self, slot: usize) -> (&[f32], &[f32]) {
        self.wires.output_buffer(slot)
    }

    /// Allocate the input's song-time history on the prepare thread.
    pub fn prepare_song_time(&mut self, plan: &SongTimeDelay) {
        self.delayed = DelayedSongTime {
            records: VecDeque::with_capacity(plan.capacity().expect("song-time ring budgeted")),
            applied: plan.offset, plan: plan.clone(), ..Default::default()
        };
    }

    /// Carry the playing history into the new allocation when keeping this
    /// program. The other instance returns its old storage to the host.
    pub fn take_prepared_song_time(&mut self, fresh: &mut Self) {
        fresh.delayed.take_from(&mut self.delayed, self.frame);
        std::mem::swap(&mut self.delayed, &mut fresh.delayed);
    }

    pub fn set_seed(&mut self, seed: u32) {
        self.seed = Some(seed);
        self.shader.set_instance(&mut self.ctx, seed);
        self.shader.seed_state(&mut self.state.words, seed);
    }

    pub fn same_program_and_seed(&self, other: &Self) -> bool {
        self.shader.ptr_eq(&other.shader) && self.seed == other.seed
    }

    /// Current effect state, in the compiled shader's word layout.
    pub fn state(&self) -> &[u32] { &self.state.words }

    pub fn shader(&self) -> &AudioShaderRef {
        &self.shader
    }

    /// Set the absolute frame of the next sample.
    pub fn sync_frame(&mut self, frame: u32) {
        self.frame = frame;
    }

    /// The song's time over the pass that starts at the synced frame:
    /// a schedule calls it right after `sync_frame`.
    pub fn set_song_time(&mut self, time: &SongTime) {
        if self.delayed.records.capacity() != 0 && self.delayed.record_pass(time, self.frame) {
            return;
        }
        self.time = *time;
        self.pass_frame = self.frame;
        self.next_anchor = 0;
        if let Some((last, next)) = time.at_start() {
            self.apply_anchor(last);
            self.next_anchor = next as u8;
        }
    }

    /// Writes anchor `k` into the effect's ctx, and the fading one's.
    fn apply_anchor(&mut self, k: usize) {
        self.time.apply(k, &self.shader, &mut self.ctx, self.pass_frame);
        if let Some(f) = self.fade.as_mut().filter(|f| f.left > 0) {
            self.time.apply(k, &f.shader, &mut f.ctx, self.pass_frame);
        }
    }

    /// Attach off the audio thread, before this instance plays.
    pub fn attach_probes(&mut self, tap: ProbeTap) -> Result<(), &'static str> {
        self.probes = Some(tap.prepare(self.shader.probe_count(), 1)?);
        Ok(())
    }

    pub fn probe_tap_mut(&mut self) -> Option<&mut ProbeTap> {
        self.probes.as_mut()
    }

    pub fn swap_probes(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.probes, &mut other.probes);
    }

    /// Also sets the parameter of that name on a program still fading out, as
    /// a wired parameter does, so a control moved during an edit's fade moves
    /// both sides of it.
    pub fn set_param(&mut self, index: usize, value: f32) {
        self.shader.set_param(&mut self.ctx, index, value);
        if let Some(f) = self.fade.as_mut().filter(|f| f.left > 0) {
            if let Some(i) = param_index(&f.shader, self.shader.param_view(index).name) {
                f.shader.set_param(&mut f.ctx, i, value);
            }
        }
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
        self.shader.reset_state(&mut self.state.words);
        self.state.used = false;
        if let Some(seed) = self.seed {
            self.shader.seed_state(&mut self.state.words, seed);
        }
    }

    /// Reads in_l/in_r and ADDS the result into out_l/out_r (any length;
    /// processed in pieces of at most [`MAX_RENDER`] frames, cut where the
    /// song's tempo changes).
    pub fn render(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.render_inner(in_l, in_r, out_l, out_r, false, |_, _| 0.0);
    }

    /// One prepared pass. Side buffers contain this pass's stereo sums.
    pub fn render_wired(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.render_with_params(in_l, in_r, out_l, out_r, |_, _| 0.0);
    }

    /// Read one prepared source sum at the pass-relative frame of each block start.
    pub fn render_with_params(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32],
        value: impl Fn(usize, usize) -> f32) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len()).min(MAX_RENDER);
        let fading = self.fade.as_ref().is_some_and(|f| f.left > 0);
        for side in &mut self.wires.inputs {
            if !side.current.stereo || (fading && side.previous.is_some_and(|p| !p.stereo)) {
                let (stereo, mono) = side.data.split_at_mut(2 * MAX_RENDER);
                for i in 0..n { mono[i] = (stereo[i] + stereo[MAX_RENDER + i]) * 0.5; }
            }
        }
        self.render_inner(&in_l[..n], &in_r[..n], &mut out_l[..n], &mut out_r[..n], true, value);
    }

    fn render_inner(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], wired: bool, value: impl Fn(usize, usize) -> f32) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        let mut at = 0;
        while at < n {
            let mut k = (n - at).min(MAX_RENDER);
            if wired && !self.wires.params.is_empty() {
                let offset = self.frame % CONTROL_BLOCK;
                k = k.min((CONTROL_BLOCK - offset) as usize);
                if offset == 0 {
                    for (slot, &(index, previous)) in self.wires.params.iter().enumerate() {
                        let target = value(slot, at);
                        self.shader.set_param(&mut self.ctx, index, target);
                        if let (Some(i), Some(f)) = (previous, self.fade.as_mut().filter(|f| f.left > 0)) {
                            f.shader.set_param(&mut f.ctx, i, target);
                        }
                    }
                }
            }
            if self.delayed.plan.offset != 0 || !self.delayed.records.is_empty() || self.delayed.switch_at.is_some() {
                let (count, line) = self.delayed.piece(k);
                k = count;
                if let Some(line) = line {
                    line.apply(&self.shader, &mut self.ctx, self.delayed.applied);
                    if let Some(f) = self.fade.as_mut().filter(|f| f.left > 0) {
                        line.apply(&f.shader, &mut f.ctx, self.delayed.applied);
                    }
                }
            } else {
                let rel = self.frame.wrapping_sub(self.pass_frame) as i64;
                while (self.next_anchor as usize) < self.time.count as usize {
                    let a = self.time.anchors[self.next_anchor as usize].1;
                    if a <= rel {
                        self.apply_anchor(self.next_anchor as usize);
                        self.next_anchor += 1;
                    } else {
                        k = k.min((a - rel) as usize);
                        break;
                    }
                }
            }
            self.render_chunk(&in_l[at..at + k], &in_r[at..at + k], &mut out_l[at..at + k], &mut out_r[at..at + k], at, wired);
            at += k;
        }
    }

    fn render_chunk(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], at: usize, wired: bool) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len()).min(MAX_RENDER);
        if n == 0 {
            return;
        }
        self.ctx[CTX_FRAME as usize] = self.frame;
        self.state.used = true;
        let fading = self.fade.as_ref().is_some_and(|f| f.left > 0);
        let sides = if wired { self.wires.inputs.as_slice() } else { &[] };
        let outs = if wired { self.wires.outputs.as_mut_slice() } else { &mut [] };
        self.ports.clear(n);
        if !fading {
            self.ports.run(&self.shader, &mut self.ctx, &mut self.state.words, &mut self.scratch,
                [in_l, in_r], [out_l, out_r], &self.bufs.zeros, n, sides, at, false, &|_| None);
            for port in outs { port.read_output(&self.ports.extra_out, at, n, false, None); }
            if let Some(tap) = &mut self.probes {
                tap.flush();
                tap.record(&self.shader, &self.ctx, u16::MAX, n);
            }
            self.frame = self.frame.wrapping_add(n as u32);
            return;
        }
        // New side, fading in.
        let (bl, br) = (&mut self.bufs.l[..n], &mut self.bufs.r[..n]);
        bl.fill(0.0);
        br.fill(0.0);
        self.ports.run(&self.shader, &mut self.ctx, &mut self.state.words, &mut self.scratch,
            [in_l, in_r], [bl, br], &self.bufs.zeros, n, sides, at, false, &|_| None);
        if let Some(tap) = &mut self.probes {
            tap.flush();
            tap.record(&self.shader, &self.ctx, u16::MAX, n);
        }
        let (g0, dg) = (self.fade_pos as f32 / FADE as f32, 1.0 / FADE as f32);
        for port in &mut *outs { port.read_output(&self.ports.extra_out, at, n, false, Some((g0, dg))); }
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
        f.ports.clear(n);
        f.state.used = true;
        f.ports.run(&f.shader, &mut f.ctx, &mut f.state.words, &mut f.scratch,
            [in_l, in_r], [bl, br], &self.bufs.zeros, n, sides, at, true, &|_| None);
        if let Some(tap) = &mut f.probes {
            tap.record(&f.shader, &f.ctx, u16::MAX, n);
        }
        let (g0, dg) = (f.left as f32 / FADE as f32, -1.0 / FADE as f32);
        for port in &mut *outs { port.read_output(&f.ports.extra_out, at, n, true, Some((g0, dg))); }
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
    /// type, and crossfades from the old program over [`FADE`] frames. When the
    /// whole state carries, the old program fades out on a copy of it, so a
    /// changed program (a new response, a new formula) never switches unfaded.
    /// No allocation: the old state moves into this instance's fade slot, and
    /// `old` is spent (retire it; it is never the target of another swap).
    pub fn swap_from(&mut self, old: &mut ShaderFx) {
        self.delayed.take_from(&mut old.delayed, old.frame);
        self.frame = old.frame;
        self.time = old.time;
        self.pass_frame = old.pass_frame;
        self.next_anchor = old.next_anchor;
        let cc = CTX_TEMPO as usize..(CTX_CC + CC_WORDS) as usize;
        self.ctx[cc.clone()].copy_from_slice(&old.ctx[cc]);
        let same_layout = self.shader.same_state_layout(&old.shader);
        self.state.take_from(&self.shader, &mut old.state, &old.shader);
        if same_layout {
            // The carry swapped boxes, handing `old` this instance's fresh one (same
            // layout, same size): the old program fades out on a copy of the state.
            // Any other carry copied words over and left `old` its own state.
            old.state.words.copy_from_slice(&self.state.words);
        }
        if let Some(seed) = self.seed {
            self.shader.set_instance(&mut self.ctx, seed);
            if self.seed != old.seed {
                self.shader.seed_state(&mut self.state.words, seed);
            }
        }
        let fade = self.fade.get_or_insert_with(|| FxFade {
            shader: old.shader.clone(), ports: PortBuffers::default(), probes: None, ctx: Box::default(), state: ShaderState::default(), scratch: Box::default(), left: 0,
        });
        std::mem::swap(&mut fade.shader, &mut old.shader);
        std::mem::swap(&mut fade.ports, &mut old.ports);
        std::mem::swap(&mut fade.probes, &mut old.probes);
        std::mem::swap(&mut fade.ctx, &mut old.ctx);
        std::mem::swap(&mut fade.state, &mut old.state);
        std::mem::swap(&mut fade.scratch, &mut old.scratch);
        fade.left = FADE;
        self.fade_pos = 0;
        self.bind_fading_wires();
    }
}

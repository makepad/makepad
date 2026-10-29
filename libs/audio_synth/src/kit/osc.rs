//! The kit's oscillators, per voice:
//!
//! - [`Osc`]: one main oscillator in four modes, each with a unison stack:
//!   wavetable (frames morphed by position, two warps), granular (grains of
//!   the table or of a sample), spectral (the frame's partials resynthesised
//!   and reshaped: shift, stretch, tilt, odd/even, formant, limits) and
//!   sample (start, loops).
//! - [`Sub`]: a plain sub oscillator; [`NoiseOsc`]: coloured, crackle and
//!   pitched digital noise, stereo.
//!
//! A block's settings (modulated once a block) go in through `prepare`;
//! `tick` makes one stereo sample and takes the warp source's last sample,
//! so oscillators can FM, PM, AM and ring-modulate each other.

use super::fast_tanh;
use super::schema::{Builder, Unit::{self, Cents, Db, Hz, Ms, Percent, Semis}, Values};
use super::table::Table;
use crate::dsp::{poly_blep, sin_turns, Noise, NoiseColor, Rng};
use std::f32::consts::TAU;
use std::sync::Arc;

pub const MAX_UNISON: usize = 16;
/// Unison voices the spectral mode resynthesises (each costs its partials).
pub const SPECTRAL_UNISON: usize = 2;
pub const MAX_PARTIALS: usize = 128;
pub const MAX_GRAINS: usize = 24;

pub const MODES: &[&str] = &["wavetable", "granular", "spectral", "sample"];
pub const WARPS: &[&str] = &[
    "off", "sync", "bend_up", "bend_down", "bend", "pwm", "asym_up", "asym_down", "flip", "mirror", "remap", "quantize", "resonant",
    "odd_even", "lowpass", "highpass", "fm", "fm_exp", "pm", "am", "rm", "fold", "sine_fold", "drive", "tube", "rectify", "crush",
    "decimate",
];
pub const WARP_SOURCES: &[&str] = &["a", "b", "c", "sub", "noise"];
pub const STACKS: &[&str] = &["off", "octave", "two_octaves", "fifth", "center_down", "center_down2", "major", "minor"];
pub const SPREADS: &[&str] = &["linear", "super", "exp", "inverse", "random"];
pub const ROUTES: &[&str] = &["f1", "f2", "both", "direct"];
pub const WINDOWS: &[&str] = &["hann", "welch", "gauss", "blackman", "tukey", "triangle", "trapezoid", "decay", "rise"];
pub const DIRECTIONS: &[&str] = &["forward", "reverse", "random"];
pub const LOOPS: &[&str] = &["off", "forward", "pingpong"];
pub const IMPORTS: &[&str] = &["fixed", "pitch"];
pub const SUB_SHAPES: &[&str] = &["sine", "triangle", "saw", "square", "pulse", "round"];
pub const NOISES: &[&str] = &["white", "pink", "brown", "blue", "velvet", "crackle", "digital"];

// Warp kinds by index in WARPS.
const W_SYNC: u8 = 1;
const W_BEND_UP: u8 = 2;
const W_BEND_DOWN: u8 = 3;
const W_BEND: u8 = 4;
const W_PWM: u8 = 5;
const W_ASYM_UP: u8 = 6;
const W_ASYM_DOWN: u8 = 7;
const W_FLIP: u8 = 8;
const W_MIRROR: u8 = 9;
const W_REMAP: u8 = 10;
const W_QUANTIZE: u8 = 11;
const W_RESONANT: u8 = 12;
const W_ODD_EVEN: u8 = 13;
const W_LOWPASS: u8 = 14;
const W_HIGHPASS: u8 = 15;
const W_FM: u8 = 16;
const W_FM_EXP: u8 = 17;
const W_PM: u8 = 18;
const W_AM: u8 = 19;
const W_RM: u8 = 20;
const W_FOLD: u8 = 21;
const W_SINE_FOLD: u8 = 22;
const W_DRIVE: u8 = 23;
const W_TUBE: u8 = 24;
const W_RECTIFY: u8 = 25;
const W_CRUSH: u8 = 26;
const W_DECIMATE: u8 = 27;

/// A sample an oscillator plays or cuts frames from (mono, its rate).
#[derive(Clone)]
pub struct SampleBuf {
    pub data: Arc<[f32]>,
    pub rate: f32,
}

/// What an oscillator reads: its table and, for sample/granular/import, a sample.
#[derive(Clone)]
pub struct OscSource {
    pub table: Arc<Table>,
    pub sample: Option<SampleBuf>,
}

/// Where a main oscillator's parameters sit in the layout.
#[derive(Clone, Debug, Default)]
pub struct OscIx {
    pub on: usize,
    pub mode: usize,
    pub table: usize,
    pub pos: usize,
    pub smooth: usize,
    pub warp1: usize,
    pub warp1_amt: usize,
    pub warp2: usize,
    pub warp2_amt: usize,
    pub warp_src: usize,
    pub oct: usize,
    pub semi: usize,
    pub fine: usize,
    pub level: usize,
    pub pan: usize,
    pub phase: usize,
    pub rand_phase: usize,
    pub unison: usize,
    pub detune: usize,
    pub blend: usize,
    pub width: usize,
    pub stack: usize,
    pub spread: usize,
    pub pos_spread: usize,
    pub warp_spread: usize,
    pub out: usize,
    pub keytrack: usize,
    pub grain: usize,
    pub density: usize,
    pub spray: usize,
    pub grain_pitch: usize,
    pub grain_pan: usize,
    pub grain_level: usize,
    pub window: usize,
    pub direction: usize,
    pub scan: usize,
    pub partials: usize,
    pub tilt: usize,
    pub shift: usize,
    pub stretch: usize,
    pub odd: usize,
    pub formant: usize,
    pub cut_lo: usize,
    pub cut_hi: usize,
    pub root: usize,
    pub start: usize,
    pub loop_mode: usize,
    pub loop_start: usize,
    pub loop_end: usize,
    pub import: usize,
}

/// Declare a main oscillator `name` (`a`, `b`, `c`): keys `a_pos`, `a_warp1` …
pub fn declare(b: &mut Builder, name: &str, tables: &'static [&'static str], on: bool, warp_src: usize) -> OscIx {
    let k = |s: &str| format!("{name}_{s}");
    b.group(&format!("osc_{name}"), "a main oscillator: wavetable (frames morphed by pos, two warps), granular (grains of the table or the sample), spectral (the frame's partials resynthesised and reshaped) or sample; a unison stack; out picks the filter");
    OscIx {
        on: b.flag(k("on"), "on", on, ""),
        mode: b.choice(k("mode"), "mode", MODES, 0, ""),
        table: b.choice(k("table"), "table", tables, 0, "basic sine>tri>saw>square, harmonic 1..63 partials, formant a>e>i>o>u, vocal ah>oh>oo>ee; user: a_frames; import: cut from a_src"),
        pos: b.num(k("pos"), "pos", 0.0, 1.0, 0.0, Percent, "where in the table (morphs its frames)"),
        smooth: b.flag(k("smooth"), "smooth", true, "blend between frames (off: step)"),
        warp1: b.choice(k("warp1"), "warp", WARPS, 0, "sync..resonant bend the phase; odd_even lowpass highpass thin the partials; fm..rm read warp_src; fold..decimate shape the output"),
        warp1_amt: b.num(k("warp1_amt"), "amount", 0.0, 1.0, 0.0, Percent, ""),
        warp2: b.choice(k("warp2"), "warp 2", WARPS, 0, "a second warp after the first"),
        warp2_amt: b.num(k("warp2_amt"), "amount 2", 0.0, 1.0, 0.0, Percent, ""),
        warp_src: b.choice(k("warp_src"), "warp src", WARP_SOURCES, warp_src, "the oscillator fm/pm/am/rm read"),
        oct: b.int(k("oct"), "oct", -4.0, 4.0, 0.0, ""),
        semi: b.num(k("semi"), "semi", -24.0, 24.0, 0.0, Semis, ""),
        fine: b.num(k("fine"), "fine", -100.0, 100.0, 0.0, Cents, ""),
        level: b.num(k("level"), "level", 0.0, 1.0, 0.75, Percent, ""),
        pan: b.num(k("pan"), "pan", -1.0, 1.0, 0.0, Unit::None, ""),
        phase: b.num(k("phase"), "phase", 0.0, 1.0, 0.0, Percent, "start phase"),
        rand_phase: b.num(k("rand_phase"), "rand", 0.0, 1.0, 1.0, Percent, "random start phase per voice"),
        unison: b.int(k("unison"), "unison", 1.0, MAX_UNISON as f32, 1.0, "voices stacked per note"),
        detune: b.num(k("detune"), "detune", 0.0, 100.0, 15.0, Cents, "outermost voices +-this"),
        blend: b.num(k("blend"), "blend", 0.0, 1.0, 0.75, Percent, "side voices against the centre (0.75 even)"),
        width: b.num(k("width"), "width", 0.0, 1.0, 1.0, Percent, "stereo spread of the voices"),
        stack: b.choice(k("stack"), "stack", STACKS, 0, "pitch offsets on alternate voices"),
        spread: b.choice(k("spread"), "spread", SPREADS, 0, "how detune spreads over the voices"),
        pos_spread: b.num(k("pos_spread"), "pos spread", 0.0, 1.0, 0.0, Percent, "unison voices spread over the table"),
        warp_spread: b.num(k("warp_spread"), "warp spread", 0.0, 1.0, 0.0, Percent, "unison voices spread the warp amounts"),
        out: b.choice(k("out"), "out", ROUTES, 0, "which filter it feeds (direct: none)"),
        keytrack: b.num(k("keytrack"), "keytrack", 0.0, 1.0, 1.0, Percent, "0: every note at c4's pitch (drones, hits)"),
        grain: b.log(k("grain"), "grain", 5.0, 500.0, 80.0, Ms, "granular: grain length"),
        density: b.log(k("density"), "density", 1.0, 200.0, 30.0, Hz, "granular: grains a second"),
        spray: b.num(k("spray"), "spray", 0.0, 1.0, 0.1, Percent, "granular: random position"),
        grain_pitch: b.num(k("grain_pitch"), "rand pitch", 0.0, 12.0, 0.0, Semis, "granular: random pitch per grain"),
        grain_pan: b.num(k("grain_pan"), "rand pan", 0.0, 1.0, 0.3, Percent, "granular: random pan per grain"),
        grain_level: b.num(k("grain_level"), "rand level", 0.0, 1.0, 0.0, Percent, "granular: random level per grain"),
        window: b.choice(k("window"), "window", WINDOWS, 0, "granular: grain shape"),
        direction: b.choice(k("direction"), "dir", DIRECTIONS, 0, "granular: grain playback"),
        scan: b.num(k("scan"), "scan", -1.0, 1.0, 0.0, Unit::None, "granular: pos moves this much a second"),
        partials: b.int(k("partials"), "partials", 4.0, MAX_PARTIALS as f32, 64.0, "spectral: partials resynthesised"),
        tilt: b.num(k("tilt"), "tilt", -12.0, 12.0, 0.0, Db, "spectral: dB per octave"),
        shift: b.num(k("shift"), "shift", -16.0, 16.0, 0.0, Unit::None, "spectral: move the spectrum by partials"),
        stretch: b.num(k("stretch"), "stretch", -0.5, 0.5, 0.0, Unit::None, "spectral: inharmonic stretch (bells, pianos)"),
        odd: b.num(k("odd"), "odd/even", -1.0, 1.0, 0.0, Unit::None, "spectral: -1 odd partials only, 1 even only"),
        formant: b.num(k("formant"), "formant", -24.0, 24.0, 0.0, Semis, "spectral: move the spectral envelope, pitch kept"),
        cut_lo: b.num(k("cut_lo"), "lo", 1.0, 64.0, 1.0, Unit::None, "spectral: lowest partial"),
        cut_hi: b.num(k("cut_hi"), "hi", 1.0, MAX_PARTIALS as f32, MAX_PARTIALS as f32, Unit::None, "spectral: highest partial"),
        root: b.int(k("root"), "root", 0.0, 127.0, 60.0, "sample: the note it plays as recorded"),
        start: b.num(k("start"), "start", 0.0, 1.0, 0.0, Percent, "sample: where it starts"),
        loop_mode: b.choice(k("loop"), "loop", LOOPS, 0, "sample: looping"),
        loop_start: b.num(k("loop_start"), "loop start", 0.0, 1.0, 0.0, Percent, ""),
        loop_end: b.num(k("loop_end"), "loop end", 0.0, 1.0, 1.0, Percent, ""),
        import: b.choice(k("import"), "import", IMPORTS, 0, "table @import: fixed 2048-sample frames, or one pitch period a frame"),
    }
}

#[derive(Clone, Debug, Default)]
pub struct SubIx {
    pub on: usize,
    pub shape: usize,
    pub oct: usize,
    pub level: usize,
    pub pan: usize,
    pub out: usize,
}

pub fn declare_sub(b: &mut Builder) -> SubIx {
    b.group("sub", "a plain sub oscillator under the note");
    SubIx {
        on: b.flag("sub_on".into(), "on", false, ""),
        shape: b.choice("sub_shape".into(), "shape", SUB_SHAPES, 0, ""),
        oct: b.int("sub_oct".into(), "oct", -3.0, 1.0, -1.0, ""),
        level: b.num("sub_level".into(), "level", 0.0, 1.0, 0.5, Percent, ""),
        pan: b.num("sub_pan".into(), "pan", -1.0, 1.0, 0.0, Unit::None, ""),
        out: b.choice("sub_out".into(), "out", ROUTES, 3, ""),
    }
}

#[derive(Clone, Debug, Default)]
pub struct NoiseIx {
    pub on: usize,
    pub color: usize,
    pub level: usize,
    pub pan: usize,
    pub pitch: usize,
    pub stereo: usize,
    pub out: usize,
}

pub fn declare_noise(b: &mut Builder) -> NoiseIx {
    b.group("noise", "a noise oscillator (crackle: random clicks; digital: sample-and-hold noise at the note's pitch)");
    NoiseIx {
        on: b.flag("noise_on".into(), "on", false, ""),
        color: b.choice("noise_color".into(), "color", NOISES, 0, ""),
        level: b.num("noise_level".into(), "level", 0.0, 1.0, 0.3, Percent, ""),
        pan: b.num("noise_pan".into(), "pan", -1.0, 1.0, 0.0, Unit::None, ""),
        pitch: b.num("noise_pitch".into(), "pitch", 0.0, 1.0, 1.0, Percent, "digital: follows the note this much"),
        stereo: b.num("noise_stereo".into(), "stereo", 0.0, 1.0, 0.0, Percent, "0 mono, 1 independent left and right"),
        out: b.choice("noise_out".into(), "out", ROUTES, 0, ""),
    }
}

/// Equal-power gains for pan -1..1.
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * std::f32::consts::PI;
    (a.cos(), a.sin())
}

fn hash01(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / 16_777_216.0
}

/// One unison voice's place: pitch ratio, stereo gains, table offset,
/// warp scale.
#[derive(Clone, Copy, Debug, Default)]
struct Uni {
    ratio: f32,
    gl: f32,
    gr: f32,
    pos: f32,
    warp: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Grain {
    on: bool,
    /// Sample source: position and step in source frames. Table source:
    /// the cycle's phase and step, at table position `frame`.
    pos: f64,
    step: f64,
    frame: f32,
    age: u32,
    len: u32,
    gl: f32,
    gr: f32,
}

/// A block's settings for one main oscillator (read through modulation).
#[derive(Clone, Copy, Debug, Default)]
pub struct OscBlock {
    pub on: bool,
    pub mode: u8,
    pub pos: f32,
    pub smooth: bool,
    pub warps: [(u8, f32); 2],
    pub warp_src: u8,
    pub level: f32,
    pub pan: f32,
    /// Pitch in Hz of this oscillator (note, octave, semitones, fine,
    /// keytrack, bend and global pitch already applied).
    pub hz: f32,
    pub unison: u8,
    pub detune: f32,
    pub blend: f32,
    pub width: f32,
    pub stack: u8,
    pub spread: u8,
    pub pos_spread: f32,
    pub warp_spread: f32,
    pub out: u8,
    pub phase: f32,
    pub rand_phase: f32,
    pub grain: f32,
    pub density: f32,
    pub spray: f32,
    pub grain_pitch: f32,
    pub grain_pan: f32,
    pub grain_level: f32,
    pub window: u8,
    pub direction: u8,
    pub scan: f32,
    pub partials: u8,
    pub tilt: f32,
    pub shift: f32,
    pub stretch: f32,
    pub odd: f32,
    pub formant: f32,
    pub cut_lo: f32,
    pub cut_hi: f32,
    pub root: f32,
    pub start: f32,
    pub loop_mode: u8,
    pub loop_start: f32,
    pub loop_end: f32,
}

impl OscBlock {
    /// Read an oscillator's block from (modulated) values; `note_hz` is the
    /// voice's pitch, `c4_hz` the pitch a non-tracking oscillator plays at,
    /// `pitch_semis` the bend and global pitch.
    pub fn read(v: &(impl Values + ?Sized), x: &OscIx, note_hz: f32, pitch_semis: f32) -> OscBlock {
        let key = v.val(x.keytrack).clamp(0.0, 1.0);
        let c4 = 261.625_58f32;
        let tracked = c4 * (note_hz / c4).powf(key);
        let semis = v.val(x.oct) * 12.0 + v.val(x.semi) + v.val(x.fine) / 100.0 + pitch_semis;
        OscBlock {
            on: v.on(x.on),
            mode: v.index(x.mode) as u8,
            pos: v.val(x.pos),
            smooth: v.on(x.smooth),
            warps: [(v.index(x.warp1) as u8, v.val(x.warp1_amt)), (v.index(x.warp2) as u8, v.val(x.warp2_amt))],
            warp_src: v.index(x.warp_src) as u8,
            level: v.val(x.level),
            pan: v.val(x.pan),
            hz: tracked * (semis / 12.0).exp2(),
            unison: v.val(x.unison).clamp(1.0, MAX_UNISON as f32) as u8,
            detune: v.val(x.detune),
            blend: v.val(x.blend),
            width: v.val(x.width),
            stack: v.index(x.stack) as u8,
            spread: v.index(x.spread) as u8,
            pos_spread: v.val(x.pos_spread),
            warp_spread: v.val(x.warp_spread),
            out: v.index(x.out) as u8,
            phase: v.val(x.phase),
            rand_phase: v.val(x.rand_phase),
            grain: v.val(x.grain),
            density: v.val(x.density),
            spray: v.val(x.spray),
            grain_pitch: v.val(x.grain_pitch),
            grain_pan: v.val(x.grain_pan),
            grain_level: v.val(x.grain_level),
            window: v.index(x.window) as u8,
            direction: v.index(x.direction) as u8,
            scan: v.val(x.scan),
            partials: v.val(x.partials).clamp(1.0, MAX_PARTIALS as f32) as u8,
            tilt: v.val(x.tilt),
            shift: v.val(x.shift),
            stretch: v.val(x.stretch),
            odd: v.val(x.odd),
            formant: v.val(x.formant),
            cut_lo: v.val(x.cut_lo),
            cut_hi: v.val(x.cut_hi),
            root: v.val(x.root),
            start: v.val(x.start),
            loop_mode: v.index(x.loop_mode) as u8,
            loop_start: v.val(x.loop_start),
            loop_end: v.val(x.loop_end),
        }
    }
}

/// One main oscillator of one voice.
pub struct Osc {
    phases: [f32; MAX_UNISON],
    incs: [f32; MAX_UNISON],
    levels: [u8; MAX_UNISON],
    uni: [Uni; MAX_UNISON],
    /// The layout's inputs (count, detune, spread, stack, width, blend) it was made for.
    uni_key: [f32; 6],
    n: usize,
    /// Output-warp state per unison voice: one-pole, decimator hold.
    lp: [f32; MAX_UNISON],
    hold: [f32; MAX_UNISON],
    hold_count: [f32; MAX_UNISON],
    /// Granular.
    grains: [Grain; MAX_GRAINS],
    spawn: f32,
    scan: f32,
    next_uni: usize,
    /// Spectral: per unison voice and partial, the rotor (re, im) and its step.
    rot: Box<[[f32; 4]]>,
    mag: Box<[f32]>,
    dmag: Box<[f32]>,
    spec_on: bool,
    /// Sample playback per unison voice: position (source frames) and direction.
    spos: [f64; MAX_UNISON],
    sdir: [f32; MAX_UNISON],
    sdone: bool,
    rng: Rng,
    /// The last mono sample (what another oscillator's warp reads).
    pub last: f32,
    seed: u32,
    rate: f32,
}

impl Osc {
    pub fn new(seed: u32) -> Osc {
        Osc {
            phases: [0.0; MAX_UNISON],
            incs: [0.0; MAX_UNISON],
            levels: [0; MAX_UNISON],
            uni: [Uni::default(); MAX_UNISON],
            uni_key: [f32::NAN; 6],
            n: 1,
            lp: [0.0; MAX_UNISON],
            hold: [0.0; MAX_UNISON],
            hold_count: [0.0; MAX_UNISON],
            grains: [Grain::default(); MAX_GRAINS],
            spawn: 0.0,
            scan: 0.0,
            next_uni: 0,
            rot: vec![[0.0; 4]; SPECTRAL_UNISON * MAX_PARTIALS].into_boxed_slice(),
            mag: vec![0.0; SPECTRAL_UNISON * MAX_PARTIALS].into_boxed_slice(),
            dmag: vec![0.0; SPECTRAL_UNISON * MAX_PARTIALS].into_boxed_slice(),
            spec_on: false,
            spos: [0.0; MAX_UNISON],
            sdir: [1.0; MAX_UNISON],
            sdone: false,
            rng: Rng::new(seed.wrapping_mul(2_654_435_761) | 1),
            last: 0.0,
            seed,
            rate: 48_000.0,
        }
    }

    /// A new note: phases, grains, sample heads and partial phases restart.
    pub fn start(&mut self, blk: &OscBlock, src: &OscSource, note_seed: u32) {
        self.seed = note_seed;
        self.rng = Rng::new(note_seed.wrapping_mul(2_246_822_519) | 1);
        for u in 0..MAX_UNISON {
            let r = self.rng.unit();
            self.phases[u] = (blk.phase + r * blk.rand_phase).fract();
            self.lp[u] = 0.0;
            self.hold[u] = 0.0;
            self.hold_count[u] = 0.0;
            self.sdir[u] = 1.0;
        }
        let len = src.sample.as_ref().map_or(0, |s| s.data.len()) as f64;
        for p in self.spos.iter_mut() {
            *p = blk.start.clamp(0.0, 1.0) as f64 * len;
        }
        self.sdone = false;
        self.grains = [Grain::default(); MAX_GRAINS];
        self.spawn = 1.0;
        self.scan = 0.0;
        self.spec_on = false;
        self.uni_key = [f32::NAN; 6];
        self.last = 0.0;
    }

    fn layout(&mut self, blk: &OscBlock) {
        let n = match blk.mode {
            2 => (blk.unison as usize).min(SPECTRAL_UNISON),
            _ => blk.unison as usize,
        }
        .clamp(1, MAX_UNISON);
        let key = [n as f32, blk.detune, blk.spread as f32, blk.stack as f32, blk.width, blk.blend];
        if key == self.uni_key {
            return;
        }
        self.uni_key = key;
        self.n = n;
        let mut sum = 0.0;
        for u in 0..n {
            let x = if n == 1 { 0.0 } else { -1.0 + 2.0 * u as f32 / (n - 1) as f32 };
            let d = match blk.spread {
                1 => x.signum() * x.abs().powf(1.6),
                2 => x.signum() * ((2.0 * x.abs()).exp() - 1.0) / (2f32.exp() - 1.0),
                3 => if x < 0.0 { x } else { x * 0.35 },
                4 => if n == 1 { 0.0 } else { hash01(self.seed ^ (u as u32 * 7919)) * 2.0 - 1.0 },
                _ => x,
            };
            let centre = n == 1 || (n % 2 == 1 && u == n / 2) || (n % 2 == 0 && (u == n / 2 || u + 1 == n / 2));
            let stack = match blk.stack {
                1 => if u % 2 == 1 { 12.0 } else { 0.0 },
                2 => [0.0, 12.0, 24.0][u % 3],
                3 => [0.0, 7.0, 12.0][u % 3],
                4 => if centre && n > 1 { -12.0 } else { 0.0 },
                5 => if centre && n > 1 { -24.0 } else { 0.0 },
                6 => [0.0, 4.0, 7.0][u % 3],
                7 => [0.0, 3.0, 7.0][u % 3],
                _ => 0.0,
            };
            let semis = d * blk.detune / 100.0 + stack;
            let g = if n <= 2 { 1.0 } else if centre { ((1.0 - blk.blend) / 0.25).min(1.0) } else { (blk.blend / 0.75).min(1.0) };
            let (gl, gr) = pan_gains(x * blk.width);
            self.uni[u] = Uni { ratio: (semis / 12.0).exp2(), gl: gl * g, gr: gr * g, pos: x * 0.5, warp: 1.0 + x * 0.5 };
            sum += g * g;
        }
        let norm = 1.0 / sum.max(1e-6).sqrt();
        for u in &mut self.uni[..n] {
            u.gl *= norm;
            u.gr *= norm;
        }
    }

    /// Ready a block: unison layout, increments, band-limit levels; for
    /// spectral the partials' targets over `frames` samples.
    pub fn prepare(&mut self, blk: &OscBlock, src: &OscSource, rate: f32, frames: usize) {
        self.layout(blk);
        self.rate = rate;
        let base = blk.hz / rate;
        // How far phase warps push the partials up (choose a lower band).
        let mut expand = 1.0f32;
        for (kind, amt) in blk.warps {
            expand *= match kind {
                W_SYNC => 1.0 + 15.0 * amt,
                W_RESONANT => 1.0 + 7.0 * amt,
                W_MIRROR => 1.0 + amt,
                W_BEND_UP | W_BEND_DOWN | W_BEND | W_ASYM_UP | W_ASYM_DOWN => 1.0 + 2.0 * amt,
                W_PWM => 1.0 / (1.0 - 0.97 * amt),
                _ => 1.0,
            };
        }
        for u in 0..self.n {
            self.incs[u] = base * self.uni[u].ratio;
            self.levels[u] = Table::level_for(self.incs[u] * expand) as u8;
        }
        if blk.mode == 2 {
            self.prepare_spectral(blk, src, rate, frames);
        }
    }

    fn prepare_spectral(&mut self, blk: &OscBlock, src: &OscSource, rate: f32, frames: usize) {
        let p = blk.partials as usize;
        let fr = (blk.formant / 12.0).exp2();
        let lo = blk.cut_lo.max(1.0);
        let hi = blk.cut_hi.max(lo);
        let table = &src.table;
        let pos = blk.pos;
        let mag_at = |x: f32| -> f32 {
            // Magnitude at a fractional partial number (1 = fundamental).
            if x < 1.0 {
                return if x > 0.0 { table.partial(pos, 0).0 * x } else { 0.0 };
            }
            let i = x as usize;
            let t = x - i as f32;
            let a = table.partial(pos, i - 1).0;
            let b = table.partial(pos, i).0;
            a + (b - a) * t
        };
        let inv = 1.0 / frames.max(1) as f32;
        for u in 0..self.n {
            let ratio = self.uni[u].ratio;
            for j in 0..MAX_PARTIALS {
                let idx = u * MAX_PARTIALS + j;
                let k = (j + 1) as f32;
                let mut m = 0.0;
                let f = blk.hz * ratio * k.powf(1.0 + blk.stretch);
                if j < p && k >= lo && k <= hi && f < rate * 0.45 {
                    m = mag_at((k - blk.shift) / fr);
                    if blk.tilt != 0.0 {
                        m *= 10f32.powf(blk.tilt * k.log2() / 20.0);
                    }
                    if (j + 1) % 2 == 0 {
                        m *= 1.0 - (-blk.odd).max(0.0);
                    } else {
                        m *= 1.0 - blk.odd.max(0.0);
                    }
                }
                let w = TAU * f / rate;
                let r = &mut self.rot[idx];
                if !self.spec_on {
                    // Start each partial at the table's phase, randomised by rand_phase.
                    let ph = table.partial(pos, j).1 + self.rng.unit() * blk.rand_phase;
                    r[0] = (TAU * ph).cos();
                    r[1] = (TAU * ph).sin();
                    self.mag[idx] = m;
                }
                r[2] = w.cos();
                r[3] = w.sin();
                // Renormalise the rotor (it drifts).
                let len = (r[0] * r[0] + r[1] * r[1]).sqrt().max(1e-9);
                r[0] /= len;
                r[1] /= len;
                self.dmag[idx] = (m - self.mag[idx]) * inv;
            }
        }
        self.spec_on = true;
    }

    /// One stereo sample; `m` is the warp source's last sample.
    #[inline]
    pub fn tick(&mut self, blk: &OscBlock, src: &OscSource, m: f32, rate: f32) -> (f32, f32) {
        let (l, r) = match blk.mode {
            1 => self.tick_granular(blk, src, rate),
            2 => self.tick_spectral(),
            3 => self.tick_sample(blk, src, m, rate),
            _ => self.tick_table(blk, &src.table, m),
        };
        self.last = (l + r) * 0.5;
        (l, r)
    }

    #[inline]
    fn tick_table(&mut self, blk: &OscBlock, table: &Table, m: f32) -> (f32, f32) {
        let (mut l, mut r) = (0.0, 0.0);
        for u in 0..self.n {
            let uni = self.uni[u];
            let ws = 1.0 + (uni.warp - 1.0) * blk.warp_spread * 2.0;
            let mut inc = self.incs[u];
            for (kind, amt) in blk.warps {
                let a = (amt * ws).clamp(0.0, 1.0);
                match kind {
                    W_FM => inc *= 1.0 + 4.0 * a * m,
                    W_FM_EXP => inc *= (4.0 * a * m).exp2(),
                    _ => {}
                }
            }
            let mut p = self.phases[u];
            let mut gain = 1.0;
            let mut odd_even = None;
            for (kind, amt) in blk.warps {
                let t = (amt * ws).clamp(0.0, 1.0);
                if t <= 0.0 {
                    continue;
                }
                match kind {
                    W_SYNC => p = (p * (1.0 + 15.0 * t)).fract(),
                    W_BEND_UP => p = p.powf(1.0 / (1.0 + 3.0 * t)),
                    W_BEND_DOWN => p = p.powf(1.0 + 3.0 * t),
                    W_BEND => {
                        let e = 1.0 + 3.0 * t;
                        p = if p < 0.5 { 0.5 * (2.0 * p).powf(e) } else { 1.0 - 0.5 * (2.0 - 2.0 * p).powf(e) };
                    }
                    W_PWM => p = (p / (1.0 - 0.97 * t)).min(0.999_99),
                    W_ASYM_UP | W_ASYM_DOWN => {
                        let k = if kind == W_ASYM_UP { 0.5 + 0.49 * t } else { 0.5 - 0.49 * t };
                        p = if p < k { 0.5 * p / k } else { 0.5 + 0.5 * (p - k) / (1.0 - k) };
                    }
                    W_FLIP => {
                        if p > 1.0 - t {
                            gain = -gain;
                        }
                    }
                    W_MIRROR => {
                        let mp = if p < 0.5 { 2.0 * p } else { 2.0 - 2.0 * p };
                        p = (p + (mp - p) * t).clamp(0.0, 0.999_99);
                    }
                    W_REMAP => p = (p - t * (TAU * p).sin() / TAU).rem_euclid(1.0),
                    W_QUANTIZE => {
                        let steps = 2.0 + (1.0 - t).powi(3) * 1022.0;
                        p = (p * steps).floor() / steps;
                    }
                    W_RESONANT => {
                        gain *= 1.0 - p;
                        p = (p * (1.0 + 7.0 * t)).fract();
                    }
                    W_PM => p = (p + t * m * 0.5).rem_euclid(1.0),
                    W_ODD_EVEN => odd_even = Some(t),
                    _ => {}
                }
            }
            let level = self.levels[u] as usize;
            let pos = (blk.pos + uni.pos * blk.pos_spread).clamp(0.0, 1.0);
            let mut y = table.read(level, pos, blk.smooth, p);
            if let Some(t) = odd_even {
                let y2 = table.read(level, pos, blk.smooth, p + 0.5);
                let even = (y + y2) * 0.5;
                let odd = (y - y2) * 0.5;
                y = odd * (1.0 - (2.0 * t - 1.0).max(0.0)) + even * (1.0 - 2.0 * t).abs();
            }
            y *= gain;
            y = self.output_warps(blk, u, y, m, ws);
            l += y * uni.gl;
            r += y * uni.gr;
            p = self.phases[u] + inc;
            self.phases[u] = p - p.floor();
        }
        (l, r)
    }

    #[inline]
    fn output_warps(&mut self, blk: &OscBlock, u: usize, mut y: f32, m: f32, ws: f32) -> f32 {
        for (kind, amt) in blk.warps {
            let t = (amt * ws).clamp(0.0, 1.0);
            if t <= 0.0 && kind != W_DECIMATE {
                continue;
            }
            y = match kind {
                W_AM => y * ((1.0 - t) + t * (0.5 + 0.5 * m)),
                W_RM => y * (1.0 - t + t * m),
                W_FOLD => {
                    let x = y * (1.0 + 7.0 * t);
                    // A triangle fold into -1..1.
                    let z = (x + 1.0).rem_euclid(4.0);
                    if z < 2.0 { z - 1.0 } else { 3.0 - z }
                }
                W_SINE_FOLD => {
                    let s = (y * (1.0 + 7.0 * t) * std::f32::consts::FRAC_PI_2).sin();
                    y + (s - y) * (t * 4.0).min(1.0)
                }
                W_DRIVE => {
                    let g = 1.0 + 15.0 * t;
                    fast_tanh(y * g) / fast_tanh(g)
                }
                W_TUBE => {
                    let g = 1.0 + 9.0 * t;
                    let bias = 0.3 * t;
                    (fast_tanh(g * (y + bias)) - fast_tanh(g * bias)) / fast_tanh(g).max(0.1)
                }
                W_RECTIFY => y + (y.abs() * 2.0 - 0.6366 - y) * t,
                W_CRUSH => {
                    let levels = (15.0 * (1.0 - t)).exp2();
                    (y * levels).round() / levels
                }
                W_DECIMATE => {
                    if t <= 0.0 {
                        y
                    } else {
                        self.hold_count[u] += 1.0;
                        if self.hold_count[u] >= 1.0 + 63.0 * t * t {
                            self.hold_count[u] = 0.0;
                            self.hold[u] = y;
                        }
                        self.hold[u]
                    }
                }
                W_LOWPASS => {
                    let hz = blk.hz * (8.0 * (1.0 - t)).exp2() * 1.5;
                    let a = (1.0 - (-TAU * hz / self.rate).exp()).clamp(0.001, 1.0);
                    self.lp[u] += (y - self.lp[u]) * a;
                    self.lp[u]
                }
                W_HIGHPASS => {
                    let hz = blk.hz * (6.0 * t).exp2() * 0.5;
                    let a = (1.0 - (-TAU * hz / self.rate).exp()).clamp(0.001, 1.0);
                    self.lp[u] += (y - self.lp[u]) * a;
                    y - self.lp[u]
                }
                _ => y,
            };
        }
        y
    }

    fn window(shape: u8, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match shape {
            1 => 1.0 - (2.0 * x - 1.0) * (2.0 * x - 1.0),
            2 => (-0.5 * ((x - 0.5) / 0.18) * ((x - 0.5) / 0.18)).exp(),
            3 => 0.42 - 0.5 * (TAU * x).cos() + 0.08 * (2.0 * TAU * x).cos(),
            4 => {
                let a = 0.25;
                if x < a { 0.5 - 0.5 * (std::f32::consts::PI * x / a).cos() } else if x > 1.0 - a { 0.5 - 0.5 * (std::f32::consts::PI * (1.0 - x) / a).cos() } else { 1.0 }
            }
            5 => 1.0 - (2.0 * x - 1.0).abs(),
            6 => (4.0 * x.min(1.0 - x)).min(1.0),
            7 => (x * 50.0).min(1.0) * (1.0 - x) * (1.0 - x),
            8 => ((1.0 - x) * 50.0).min(1.0) * x * x,
            _ => 0.5 - 0.5 * (TAU * x).cos(),
        }
    }

    fn tick_granular(&mut self, blk: &OscBlock, src: &OscSource, rate: f32) -> (f32, f32) {
        // Spawn on the density clock.
        self.spawn += blk.density / rate;
        self.scan = (self.scan + blk.scan / rate).rem_euclid(1.0);
        let len = ((blk.grain * 0.001 * rate) as u32).max(16);
        let overlap = (blk.density * blk.grain * 0.001).max(1.0);
        let norm = 1.0 / overlap.sqrt();
        if self.spawn >= 1.0 {
            self.spawn -= self.spawn.floor();
            if let Some(g) = self.grains.iter_mut().find(|g| !g.on) {
                let u = self.next_uni % self.n;
                self.next_uni = self.next_uni.wrapping_add(1);
                let uni = self.uni[u];
                let pos = (blk.pos + self.scan + (self.rng.bipolar() * blk.spray)).rem_euclid(1.0);
                let pitch = ((self.rng.bipolar() * blk.grain_pitch) / 12.0).exp2() * uni.ratio;
                let reverse = match blk.direction {
                    1 => true,
                    2 => self.rng.unit() < 0.5,
                    _ => false,
                };
                let (pl, pr) = pan_gains((blk.pan + self.rng.bipolar() * blk.grain_pan).clamp(-1.0, 1.0));
                let gain = norm * (1.0 - self.rng.unit() * blk.grain_level);
                *g = match &src.sample {
                    Some(s) => {
                        let step = (blk.hz / midi_hz(blk.root)) as f64 * (s.rate / rate) as f64 * pitch as f64;
                        Grain { on: true, pos: pos as f64 * s.data.len() as f64, step: if reverse { -step } else { step }, frame: 0.0, age: 0, len, gl: pl * gain, gr: pr * gain }
                    }
                    None => {
                        let step = (blk.hz / rate * pitch) as f64;
                        Grain { on: true, pos: self.rng.unit() as f64, step: if reverse { -step } else { step }, frame: pos, age: 0, len, gl: pl * gain, gr: pr * gain }
                    }
                };
            }
        }
        let (mut l, mut r) = (0.0, 0.0);
        let level = Table::level_for(blk.hz / rate);
        for g in self.grains.iter_mut().filter(|g| g.on) {
            let w = Self::window(blk.window, g.age as f32 / g.len as f32);
            let y = match &src.sample {
                Some(s) => {
                    let n = s.data.len();
                    let p = g.pos.rem_euclid(n as f64);
                    let i = p as usize;
                    let t = (p - i as f64) as f32;
                    let a = s.data[i % n];
                    let b = s.data[(i + 1) % n];
                    a + (b - a) * t
                }
                None => src.table.read(level, g.frame, blk.smooth, g.pos.rem_euclid(1.0) as f32),
            };
            g.pos += g.step;
            l += y * w * g.gl;
            r += y * w * g.gr;
            g.age += 1;
            if g.age >= g.len {
                g.on = false;
            }
        }
        (l, r)
    }

    fn tick_spectral(&mut self) -> (f32, f32) {
        let (mut l, mut r) = (0.0, 0.0);
        for u in 0..self.n {
            let mut y = 0.0;
            let base = u * MAX_PARTIALS;
            for j in 0..MAX_PARTIALS {
                let idx = base + j;
                let m = self.mag[idx];
                if m == 0.0 && self.dmag[idx] == 0.0 {
                    continue;
                }
                let rr = &mut self.rot[idx];
                let (re, im) = (rr[0] * rr[2] - rr[1] * rr[3], rr[0] * rr[3] + rr[1] * rr[2]);
                rr[0] = re;
                rr[1] = im;
                y += m * im;
                self.mag[idx] = m + self.dmag[idx];
            }
            let uni = self.uni[u];
            l += y * uni.gl;
            r += y * uni.gr;
        }
        (l, r)
    }

    fn tick_sample(&mut self, blk: &OscBlock, src: &OscSource, m: f32, rate: f32) -> (f32, f32) {
        let Some(s) = &src.sample else { return (0.0, 0.0) };
        let n = s.data.len();
        if n < 2 || self.sdone {
            return (0.0, 0.0);
        }
        let ls = (blk.loop_start.clamp(0.0, 1.0) * n as f32) as f64;
        let le = ((blk.loop_end.clamp(0.0, 1.0) * n as f32) as f64).max(ls + 2.0).min(n as f64 - 1.0);
        let base = (blk.hz / midi_hz(blk.root)) as f64 * (s.rate / rate) as f64;
        let (mut l, mut r) = (0.0, 0.0);
        let mut done = true;
        for u in 0..self.n {
            let uni = self.uni[u];
            let mut step = base * uni.ratio as f64;
            for (kind, amt) in blk.warps {
                if kind == W_FM {
                    step *= 1.0 + 4.0 * (amt * m) as f64;
                }
            }
            let p = self.spos[u];
            if p < 0.0 || p >= n as f64 - 1.0 {
                continue;
            }
            done = false;
            let i = p as usize;
            let t = (p - i as f64) as f32;
            let mut y = s.data[i] + (s.data[i + 1] - s.data[i]) * t;
            y = self.output_warps(blk, u, y, m, 1.0);
            l += y * uni.gl;
            r += y * uni.gr;
            let mut next = p + step * self.sdir[u] as f64;
            match blk.loop_mode {
                1 if next >= le => next = ls + (next - le),
                2 if next >= le => {
                    self.sdir[u] = -1.0;
                    next = le - (next - le);
                }
                2 if next < ls && self.sdir[u] < 0.0 => {
                    self.sdir[u] = 1.0;
                    next = ls + (ls - next);
                }
                _ => {}
            }
            self.spos[u] = next;
        }
        self.sdone = done;
        (l, r)
    }
}

#[inline]
pub fn midi_hz(note: f32) -> f32 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

/// The sub oscillator.
pub struct Sub {
    phase: f32,
}

impl Default for Sub {
    fn default() -> Self {
        Sub { phase: 0.0 }
    }
}

impl Sub {
    pub fn start(&mut self) {
        self.phase = 0.0;
    }

    #[inline]
    pub fn tick(&mut self, shape: u8, inc: f32) -> f32 {
        let t = self.phase;
        let dt = inc.clamp(1e-6, 0.5);
        let square = |w: f32| {
            let mut v = if t < w { 1.0 } else { -1.0 };
            v += poly_blep(t, dt);
            let t2 = (t + 1.0 - w).fract();
            v - poly_blep(t2, dt)
        };
        let y = match shape {
            1 => 1.0 - 4.0 * ((t + 0.25).fract() - 0.5).abs(),
            2 => 1.0 - 2.0 * t + poly_blep(t, dt),
            3 => square(0.5),
            4 => square(0.25),
            5 => fast_tanh(3.0 * sin_turns(t)) / fast_tanh(3.0),
            _ => sin_turns(t),
        };
        self.phase = (t + dt).fract();
        y
    }
}

/// The noise oscillator: two generators (left, right) blended by `stereo`.
pub struct NoiseOsc {
    l: Noise,
    r: Noise,
    rng: Rng,
    hold: (f32, f32),
    phase: f32,
    lp: (f32, f32),
}

impl NoiseOsc {
    pub fn new(seed: u32) -> NoiseOsc {
        NoiseOsc { l: Noise::new(NoiseColor::White, seed | 1), r: Noise::new(NoiseColor::White, seed.wrapping_mul(747_796_405) | 1), rng: Rng::new(seed ^ 0x9e37_79b9), hold: (0.0, 0.0), phase: 0.0, lp: (0.0, 0.0) }
    }

    /// One stereo sample of colour `color`; `inc` is the note's phase step
    /// (digital noise steps at it).
    #[inline]
    pub fn tick(&mut self, color: u8, stereo: f32, inc: f32) -> (f32, f32) {
        let (a, b) = match color {
            5 => {
                // Crackle: sparse clicks of random size, softened.
                let click = |lp: &mut f32, rng: &mut Rng| {
                    let x = if rng.unit() < 0.0015 { rng.bipolar() * 2.5 } else { 0.0 };
                    *lp += (x - *lp) * 0.6;
                    *lp
                };
                let a = click(&mut self.lp.0, &mut self.rng);
                let b = click(&mut self.lp.1, &mut self.rng);
                (a, b)
            }
            6 => {
                self.phase += inc;
                if self.phase >= 1.0 {
                    self.phase -= self.phase.floor();
                    self.hold = (self.rng.bipolar(), self.rng.bipolar());
                }
                self.hold
            }
            c => {
                let color = match c {
                    1 => NoiseColor::Pink,
                    2 => NoiseColor::Brown,
                    3 => NoiseColor::Blue,
                    4 => NoiseColor::Velvet,
                    _ => NoiseColor::White,
                };
                self.l.color = color;
                self.r.color = color;
                (self.l.next(), self.r.next())
            }
        };
        let s = stereo.clamp(0.0, 1.0);
        (a, a + (b - a) * s)
    }
}

/// One cycle of an oscillator as it plays (frame, warps, spectral
/// reshaping), unison off, into `out` (-1..1): for pictures. Allocates.
pub fn cycle_picture(blk: &OscBlock, src: &OscSource, out: &mut [f32]) {
    let n = out.len().max(1);
    let mut b = *blk;
    b.unison = 1;
    b.phase = 0.0;
    b.rand_phase = 0.0;
    b.pan = 0.0;
    if b.mode == 1 || b.mode == 3 {
        b.mode = 0;
    }
    let rate = 48_000.0;
    // A pitch whose cycle is `n` samples; settle one cycle, keep the next.
    b.hz = rate / n as f32;
    let mut osc = Osc::new(1);
    osc.start(&b, src, 1);
    osc.prepare(&b, src, rate, n);
    for pass in 0..2 {
        for o in out.iter_mut() {
            // Cross-modulation reads a silent source here.
            let (l, r) = osc.tick(&b, src, 0.0, rate);
            if pass == 1 {
                *o = (l + r) * std::f32::consts::FRAC_1_SQRT_2;
            }
        }
        if b.mode == 2 {
            // Spectral partials ramp in over the first block: hold them.
            osc.prepare(&b, src, rate, n);
        }
    }
}

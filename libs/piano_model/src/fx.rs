// Output stage: early reflections + feedback-delay-network reverb + tone
// shelves + DC blocker + soft saturation. Everything algorithmic (no impulse
// responses shipped); the whole chain is per-sample state machines, so it is
// allocation-free, lock-free and bit-deterministic across block sizes.
//
// Reverb architecture (Jot-style FDN):
// - mono sum -> predelay -> 4 series Schroeder allpasses (input diffusion)
// - 8 delay lines with mutually rough-coprime lengths, scaled by `size`
// - per-line absorptive shelf: g_hi + (g_lo - g_hi) * onepole, with
//   g = 10^(-3 L_i / (fs * RT60)) so the decay time actually is the setting,
//   and RT60_high = hf_ratio * RT60_low so highs die faster like air/walls
// - lossless 8x8 fast-Hadamard feedback (orthogonal), scaled 1/sqrt(8):
//   the loop skeleton is energy-preserving, all loss lives in the shelves,
//   which makes long tails clean and boundedness provable: per-line loop gain
//   <= max(g_lo, g_hi) < 1, modulated fractional reads are convex
//   (interpolation gain <= 1), so the loop is a strict contraction
// - four of the lines have slowly LFO-modulated fractional read taps, which
//   breaks up degenerate ring patterns (the "metallic" FDN sound)
// - L/R taps use two orthogonal sign rows over the line outputs, giving
//   decorrelated tails from one network
//
// A user-supplied IR convolver can later slot in as another `StereoEffect`.

pub trait StereoEffect {
    fn process_block(&mut self, l: &mut [f32], r: &mut [f32]);
    fn reset(&mut self);
}

pub const NUM_LINES: usize = 8;
const BASE_MS: [f32; NUM_LINES] = [29.7, 37.1, 41.1, 43.7, 53.3, 61.9, 73.7, 79.9];
const MAX_SIZE: f32 = 1.6;
const MIN_SIZE: f32 = 0.4;
const MOD_DEPTH: f32 = 5.0; // samples, at 48k; scaled with fs
const AP_MS: [f32; 4] = [4.7, 3.6, 12.7, 9.3];
const AP_G: f32 = 0.62;
const MAX_PREDELAY_S: f32 = 0.12;

/// Ready-made rooms, ordered small to vast. `ReverbPreset::ALL` lists them
/// for a UI dropdown; each maps to a distinct `ReverbParams` via `params()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReverbPreset {
    PracticeRoom,
    Studio,
    SmallHall,
    ConcertHall,
    Cathedral,
}

impl ReverbPreset {
    /// All presets, in musical small-to-large order (for UI enumeration).
    pub const ALL: [ReverbPreset; 5] = [
        ReverbPreset::PracticeRoom,
        ReverbPreset::Studio,
        ReverbPreset::SmallHall,
        ReverbPreset::ConcertHall,
        ReverbPreset::Cathedral,
    ];
}

/// The four continuous reverb controls. Out-of-range values are clamped on
/// apply; read the effective values back with `Piano::reverb_params`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    /// Broadband RT60 at low frequencies, seconds. Clamped to 0.2..=12.0.
    pub decay_s: f32,
    /// Room scale (delay-line lengths). Clamped to 0.4..=1.6.
    pub size: f32,
    /// High-frequency absorption: 0 = bright marble, 1 = heavy drapes.
    /// Clamped to 0.0..=1.0.
    pub damping: f32,
    /// Gap between the direct sound and the tail, seconds. Larger reads as
    /// a bigger, more distant room. Clamped to 0.0..=0.119.
    pub predelay_s: f32,
}

impl ReverbPreset {
    pub fn params(self) -> ReverbParams {
        match self {
            ReverbPreset::PracticeRoom => ReverbParams { decay_s: 0.45, size: 0.5, damping: 0.7, predelay_s: 0.004 },
            ReverbPreset::Studio => ReverbParams { decay_s: 0.7, size: 0.7, damping: 0.55, predelay_s: 0.008 },
            ReverbPreset::SmallHall => ReverbParams { decay_s: 1.3, size: 0.95, damping: 0.45, predelay_s: 0.014 },
            ReverbPreset::ConcertHall => ReverbParams { decay_s: 2.1, size: 1.25, damping: 0.35, predelay_s: 0.02 },
            ReverbPreset::Cathedral => ReverbParams { decay_s: 4.5, size: 1.6, damping: 0.3, predelay_s: 0.032 },
        }
    }
}

struct DelayLine {
    buf: Box<[f32]>,
    write: usize,
}

impl DelayLine {
    fn new(capacity: usize) -> Self {
        Self { buf: vec![0.0; capacity.next_power_of_two()].into_boxed_slice(), write: 0 }
    }

    #[inline(always)]
    fn mask(&self) -> usize {
        self.buf.len() - 1
    }

    #[inline(always)]
    fn push(&mut self, x: f32) {
        self.write = (self.write + 1) & self.mask();
        self.buf[self.write] = x;
    }

    /// Integer tap `d` samples back (d >= 1 relative to last pushed sample).
    #[inline(always)]
    fn read(&self, d: usize) -> f32 {
        self.buf[(self.write + self.buf.len() - d) & self.mask()]
    }

    /// Fractional tap, linear interpolation (convex: gain <= 1).
    #[inline(always)]
    fn read_frac(&self, d: f32) -> f32 {
        let di = d as usize;
        let fr = d - di as f32;
        let a = self.read(di);
        let b = self.read(di + 1);
        a + fr * (b - a)
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

struct Allpass {
    buf: Box<[f32]>,
    len: usize,
    pos: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)].into_boxed_slice(), len: len.max(1), pos: 0 }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let d = self.buf[self.pos];
        let v = x - AP_G * d;
        let y = d + AP_G * v;
        self.buf[self.pos] = v;
        self.pos += 1;
        if self.pos >= self.len {
            self.pos = 0;
        }
        y
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

pub struct Reverb {
    sample_rate: f32,
    lines: [DelayLine; NUM_LINES],
    len: [usize; NUM_LINES],
    g_lo: [f32; NUM_LINES],
    g_hi: [f32; NUM_LINES],
    lp: [f32; NUM_LINES],
    damp_c: f32,
    ap: [Allpass; 4],
    predelay: DelayLine,
    predelay_len: usize,
    lfo_phase: [f32; 4],
    lfo_inc: [f32; 4],
    mod_depth: f32,
    params: ReverbParams,
}

// L/R output tap sign rows (orthogonal Hadamard rows -> decorrelated tails).
const TAP_L: [f32; NUM_LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
const TAP_R: [f32; NUM_LINES] = [1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0];
// Injection signs spread the mono diffused input over the lines.
const INJECT: [f32; NUM_LINES] = [1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0];

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let cap = |ms: f32| (ms * 0.001 * MAX_SIZE * sample_rate) as usize + (MOD_DEPTH * sample_rate / 48000.0) as usize + 8;
        let lines = [
            DelayLine::new(cap(BASE_MS[0])),
            DelayLine::new(cap(BASE_MS[1])),
            DelayLine::new(cap(BASE_MS[2])),
            DelayLine::new(cap(BASE_MS[3])),
            DelayLine::new(cap(BASE_MS[4])),
            DelayLine::new(cap(BASE_MS[5])),
            DelayLine::new(cap(BASE_MS[6])),
            DelayLine::new(cap(BASE_MS[7])),
        ];
        let ap = [
            Allpass::new((AP_MS[0] * 0.001 * sample_rate) as usize),
            Allpass::new((AP_MS[1] * 0.001 * sample_rate) as usize),
            Allpass::new((AP_MS[2] * 0.001 * sample_rate) as usize),
            Allpass::new((AP_MS[3] * 0.001 * sample_rate) as usize),
        ];
        let rates = [0.31f32, 0.41, 0.53, 0.67];
        let mut rv = Self {
            sample_rate,
            lines,
            len: [1; NUM_LINES],
            g_lo: [0.5; NUM_LINES],
            g_hi: [0.5; NUM_LINES],
            lp: [0.0; NUM_LINES],
            damp_c: 1.0 - (-core::f32::consts::TAU * 2200.0 / sample_rate).exp(),
            ap,
            predelay: DelayLine::new((MAX_PREDELAY_S * sample_rate) as usize + 4),
            predelay_len: 1,
            lfo_phase: [0.0, 1.7, 3.1, 4.9],
            lfo_inc: [0.0; 4],
            mod_depth: MOD_DEPTH * sample_rate / 48000.0,
            params: ReverbPreset::SmallHall.params(),
        };
        for i in 0..4 {
            rv.lfo_inc[i] = core::f32::consts::TAU * rates[i] / sample_rate;
        }
        rv.apply_params();
        rv
    }

    pub fn set_preset(&mut self, preset: ReverbPreset) {
        self.params = preset.params();
        self.apply_params();
    }

    pub fn set_params(&mut self, params: ReverbParams) {
        self.params = params;
        self.apply_params();
    }

    pub fn params(&self) -> ReverbParams {
        self.params
    }

    /// Control path only (called from setters, never per sample). No alloc.
    fn apply_params(&mut self) {
        let p = &mut self.params;
        p.decay_s = p.decay_s.clamp(0.2, 12.0);
        p.size = p.size.clamp(MIN_SIZE, MAX_SIZE);
        p.damping = p.damping.clamp(0.0, 1.0);
        p.predelay_s = p.predelay_s.clamp(0.0, MAX_PREDELAY_S - 0.001);
        let hf_ratio = 1.0 - 0.78 * p.damping; // RT60_high / RT60_low
        for i in 0..NUM_LINES {
            let l = ((BASE_MS[i] * 0.001 * p.size * self.sample_rate) as usize).max(32);
            self.len[i] = l;
            let g_lo = 10f32.powf(-3.0 * l as f32 / (self.sample_rate * p.decay_s));
            let g_hi = 10f32.powf(-3.0 * l as f32 / (self.sample_rate * p.decay_s * hf_ratio));
            self.g_lo[i] = g_lo.min(0.99995);
            self.g_hi[i] = g_hi.min(0.99995);
        }
        self.predelay_len = ((p.predelay_s * self.sample_rate) as usize).max(1);
    }

    /// One sample in (stereo), one wet sample out. Dry/wet is the caller's.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        // input: mono sum -> predelay -> diffusion
        let x = 0.5 * (in_l + in_r);
        self.predelay.push(x);
        let mut d = self.predelay.read(self.predelay_len);
        d = self.ap[0].process(d);
        d = self.ap[1].process(d);
        d = self.ap[2].process(d);
        d = self.ap[3].process(d);

        // read + absorb
        let mut y = [0.0f32; NUM_LINES];
        for i in 0..NUM_LINES {
            let v = if i < 4 {
                // modulated fractional tap
                let m = self.mod_depth * (0.5 + 0.5 * self.lfo_phase_sin(i));
                self.lines[i].read_frac(self.len[i] as f32 - 1.0 - m)
            } else {
                self.lines[i].read(self.len[i])
            };
            self.lp[i] += self.damp_c * (v - self.lp[i]);
            y[i] = self.g_hi[i] * v + (self.g_lo[i] - self.g_hi[i]) * self.lp[i];
        }

        // lossless fast-Hadamard feedback, scaled 1/sqrt(8)
        let mut h = y;
        for stride in [1usize, 2, 4] {
            let mut i = 0;
            while i < NUM_LINES {
                let (a, b) = (h[i], h[i + stride]);
                h[i] = a + b;
                h[i + stride] = a - b;
                i += 1;
                if i % stride == 0 {
                    i += stride;
                }
            }
        }
        const NORM: f32 = 0.35355338; // 1/sqrt(8)

        for i in 0..NUM_LINES {
            self.lines[i].push(h[i] * NORM + INJECT[i] * d * 0.4);
        }
        for i in 0..4 {
            self.lfo_phase[i] += self.lfo_inc[i];
            if self.lfo_phase[i] > core::f32::consts::TAU {
                self.lfo_phase[i] -= core::f32::consts::TAU;
            }
        }

        let mut wl = 0.0;
        let mut wr = 0.0;
        for i in 0..NUM_LINES {
            wl += TAP_L[i] * y[i];
            wr += TAP_R[i] * y[i];
        }
        (wl * NORM, wr * NORM)
    }

    #[inline(always)]
    fn lfo_phase_sin(&self, i: usize) -> f32 {
        self.lfo_phase[i].sin()
    }

    pub fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        for a in &mut self.ap {
            a.clear();
        }
        self.predelay.clear();
        self.lp = [0.0; NUM_LINES];
    }
}

// ---------------------------------------------------------------------------
// Early reflections + listener perspective
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Perspective {
    /// Sitting at the keys: bass left, tight lid reflections, close image.
    Player,
    /// In the hall: image mirrored and narrowed, later/stronger reflections.
    Audience,
}

const ER_TAPS: usize = 6;

pub struct EarlyReflections {
    buf: DelayLine,
    delay: [usize; ER_TAPS],
    gain_l: [f32; ER_TAPS],
    gain_r: [f32; ER_TAPS],
    // Reflections come off wood and cloth darker than the direct sound.
    lp_l: f32,
    lp_r: f32,
    lp_c: f32,
}

impl EarlyReflections {
    pub fn new(sample_rate: f32) -> Self {
        let mut er = Self {
            buf: DelayLine::new((0.1 * sample_rate) as usize + 4),
            delay: [1; ER_TAPS],
            gain_l: [0.0; ER_TAPS],
            gain_r: [0.0; ER_TAPS],
            lp_l: 0.0,
            lp_r: 0.0,
            lp_c: 1.0 - (-core::f32::consts::TAU * 4200.0 / sample_rate).exp(),
        };
        er.set_perspective(Perspective::Player, sample_rate);
        er
    }

    pub fn set_perspective(&mut self, p: Perspective, sample_rate: f32) {
        let (ms, gl, gr): ([f32; ER_TAPS], [f32; ER_TAPS], [f32; ER_TAPS]) = match p {
            Perspective::Player => (
                [8.9, 12.7, 16.3, 21.1, 27.7, 34.9],
                [0.16, -0.05, 0.11, -0.04, 0.07, 0.03],
                [-0.05, 0.15, 0.04, -0.11, 0.03, 0.06],
            ),
            Perspective::Audience => (
                [18.1, 23.9, 31.7, 40.3, 52.9, 66.1],
                [0.15, -0.07, 0.12, -0.06, 0.08, 0.04],
                [-0.07, 0.16, 0.05, -0.13, 0.04, 0.08],
            ),
        };
        for i in 0..ER_TAPS {
            self.delay[i] = ((ms[i] * 0.001 * sample_rate) as usize).max(1);
            self.gain_l[i] = gl[i];
            self.gain_r[i] = gr[i];
        }
    }

    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        self.buf.push(0.5 * (in_l + in_r));
        let mut l = 0.0;
        let mut r = 0.0;
        for i in 0..ER_TAPS {
            let v = self.buf.read(self.delay[i]);
            l += self.gain_l[i] * v;
            r += self.gain_r[i] * v;
        }
        self.lp_l += self.lp_c * (l - self.lp_l);
        self.lp_r += self.lp_c * (r - self.lp_r);
        (self.lp_l, self.lp_r)
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.lp_l = 0.0;
        self.lp_r = 0.0;
    }
}

// ---------------------------------------------------------------------------
// Tone control, DC blocker, soft saturation
// ---------------------------------------------------------------------------

/// Gentle two-band shelf EQ (one-pole shelves at 120 Hz and 6 kHz).
pub struct Tone {
    lp1_l: f32,
    lp1_r: f32,
    lp2_l: f32,
    lp2_r: f32,
    c_low: f32,
    c_high: f32,
    g_low: f32,
    g_high: f32,
}

impl Tone {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            lp1_l: 0.0,
            lp1_r: 0.0,
            lp2_l: 0.0,
            lp2_r: 0.0,
            c_low: 1.0 - (-core::f32::consts::TAU * 120.0 / sample_rate).exp(),
            c_high: 1.0 - (-core::f32::consts::TAU * 6000.0 / sample_rate).exp(),
            g_low: 1.0,
            g_high: 1.0,
        }
    }

    pub fn set(&mut self, bass_db: f32, treble_db: f32) {
        self.g_low = 10f32.powf(bass_db.clamp(-12.0, 12.0) / 20.0);
        self.g_high = 10f32.powf(treble_db.clamp(-12.0, 12.0) / 20.0);
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.lp1_l += self.c_low * (l - self.lp1_l);
        self.lp1_r += self.c_low * (r - self.lp1_r);
        let l = l + (self.g_low - 1.0) * self.lp1_l;
        let r = r + (self.g_low - 1.0) * self.lp1_r;
        self.lp2_l += self.c_high * (l - self.lp2_l);
        self.lp2_r += self.c_high * (r - self.lp2_r);
        let l = l + (self.g_high - 1.0) * (l - self.lp2_l);
        let r = r + (self.g_high - 1.0) * (r - self.lp2_r);
        (l, r)
    }

    pub fn reset(&mut self) {
        self.lp1_l = 0.0;
        self.lp1_r = 0.0;
        self.lp2_l = 0.0;
        self.lp2_r = 0.0;
    }
}

/// 20 Hz DC blocker.
pub struct DcBlock {
    x1: f32,
    y1: f32,
    r: f32,
}

impl DcBlock {
    pub fn new(sample_rate: f32) -> Self {
        Self { x1: 0.0, y1: 0.0, r: 1.0 - core::f32::consts::TAU * 20.0 / sample_rate }
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}


/// The output limiter: a gain that RIDES the music, ahead of the safety knee.
///
/// [`soft_clip`] is a waveshaper. It is exact below its threshold and it can
/// never let a sample past full scale, which makes it a good last line, but it
/// has no time constant at all: it decides sample by sample, so a peak 6 dB
/// over the ceiling is 6 dB of gain reduction applied to that sample and none
/// to its neighbour. That is distortion by construction, and on a dense
/// fortissimo — measured at a pre-limiter peak of 2.05 on a Liszt climax, with
/// 0.15% of samples over the knee — it is audible as overdrive.
///
/// A limiter instead moves ONE gain slowly relative to the audio, so a loud
/// passage is quieter rather than distorted. The detector takes peaks
/// instantly and lets them go over [`RELEASE_MS`]; the gain follows downward
/// over [`ATTACK_MS`] and back up at the release rate. Nothing here looks
/// ahead, so a transient's first millisecond can still cross the ceiling —
/// that is precisely what the knee behind it is for, and it now catches
/// microseconds of transient instead of shaping whole chords.
///
/// Per-sample state and no branching on block length: N blocks of any sizes
/// give bit-identical output to one block of their sum.
pub struct Limiter {
    /// Peak envelope of the input.
    env: f32,
    /// The gain being applied.
    gain: f32,
    attack: f32,
    release: f32,
    ceiling: f32,
}

/// How fast the gain comes down onto a peak. Long enough not to modulate the
/// audio it is riding (which would be distortion again), short enough that the
/// knee behind it only ever sees a transient's leading edge.
const ATTACK_MS: f32 = 1.5;
/// How fast it lets go. Slow enough that a run of loud chords is held at one
/// level rather than pumped between them.
const RELEASE_MS: f32 = 180.0;
/// Where the gain stops it. Under the knee, so in normal playing the two
/// stages never both work on the same sample.
const CEILING: f32 = 0.72;

impl Limiter {
    pub fn new(sample_rate: f32) -> Self {
        let coefficient = |ms: f32| {
            let samples = ms * 0.001 * sample_rate.max(1.0);
            1.0 - (-1.0 / samples.max(1.0)).exp()
        };
        Self {
            env: 0.0,
            gain: 1.0,
            attack: coefficient(ATTACK_MS),
            release: coefficient(RELEASE_MS),
            ceiling: CEILING,
        }
    }

    /// One stereo frame. The same gain goes on both channels, so the image
    /// does not move when one hand is louder than the other.
    #[inline(always)]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let peak = left.abs().max(right.abs());
        // Instant attack on the DETECTOR: the envelope must already know about
        // a peak before the gain starts moving towards it.
        self.env = if peak > self.env {
            peak
        } else {
            self.env + self.release * (peak - self.env)
        };
        let target = if self.env > self.ceiling {
            self.ceiling / self.env
        } else {
            1.0
        };
        let rate = if target < self.gain { self.attack } else { self.release };
        self.gain += rate * (target - self.gain);
        if !self.gain.is_finite() {
            self.gain = 1.0;
        }
        (left * self.gain, right * self.gain)
    }

    /// How much the limiter is holding back right now, in dB. Zero when it is
    /// out of the way.
    pub fn reduction_db(&self) -> f32 {
        -20.0 * self.gain.max(1.0e-6).log10()
    }
}

/// Output SAFETY knee, behind [`Limiter`]: exactly unity below the 0.78
/// threshold, then a
/// smooth (C1) rational knee that approaches +/-1.0 asymptotically, so a
/// fortissimo chord can never digital-clip.
///
/// This replaces an always-on odd-cubic waveshaper. That curve was already
/// -2.6% at 0.3 and -22% at 1.0 — i.e. a distortion/compression stage
/// working through every loud passage (measured: 14% of samples above 0.3
/// on an alla-turca render) — and, being an un-oversampled nonlinearity,
/// it folded its harmonics back across the band on exactly the transients
/// a piano lives on. The knee form is transparent for >99.4% of samples on
/// the loudest test pieces and touches only extreme transient tips, where
/// its brief fold products sit under the broadband transient itself. True
/// lookahead limiting or an oversampled shaper would need latency, which
/// the sample-accurate event contract does not allow.
#[inline(always)]
pub fn soft_clip(x: f32) -> f32 {
    const T: f32 = 0.78;
    const R: f32 = 1.0 - T; // knee range
    let a = x.abs();
    if a <= T {
        return x;
    }
    let u = (a - T) / R;
    let y = T + R * u / (1.0 + u);
    if x >= 0.0 {
        y
    } else {
        -y
    }
}


// ---------------------------------------------------------------------------
// Output EQ: a treble shelf with a settable corner plus one parametric
// presence bell. Sits between the dry instrument and the room sends (the
// room hears the EQ'd source, as on a mixing desk). Flat by default and
// bypassed when flat, so the physical voicing stays the primary character.
// RBJ biquads, f64 design, f32 state; coefficients are safe to change
// between process() calls (control path, no allocation).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn identity() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    fn set_coeffs(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        let inv = 1.0 / a0;
        self.b0 = (b0 * inv) as f32;
        self.b1 = (b1 * inv) as f32;
        self.b2 = (b2 * inv) as f32;
        self.a1 = (a1 * inv) as f32;
        self.a2 = (a2 * inv) as f32;
    }
}

pub struct Eq {
    fs: f64,
    shelf_l: Biquad,
    shelf_r: Biquad,
    bell_l: Biquad,
    bell_r: Biquad,
    shelf_on: bool,
    bell_on: bool,
    shelf_db: f32,
    shelf_hz: f32,
    bell_hz: f32,
    bell_db: f32,
    bell_q: f32,
}

impl Eq {
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            shelf_l: Biquad::identity(),
            shelf_r: Biquad::identity(),
            bell_l: Biquad::identity(),
            bell_r: Biquad::identity(),
            shelf_on: false,
            bell_on: false,
            shelf_db: 0.0,
            shelf_hz: 6000.0,
            bell_hz: 3000.0,
            bell_db: 0.0,
            bell_q: 1.4,
        }
    }

    /// High shelf: gain_db in -24..=12, corner 1 kHz..16 kHz.
    pub fn set_shelf(&mut self, gain_db: f32, corner_hz: f32) {
        let g = if gain_db.is_finite() { gain_db.clamp(-24.0, 12.0) } else { 0.0 };
        let fc = if corner_hz.is_finite() { corner_hz.clamp(1000.0, 16000.0) } else { 6000.0 };
        self.shelf_db = g;
        self.shelf_hz = fc;
        if g.abs() < 0.01 {
            self.shelf_on = false;
            return;
        }
        self.shelf_on = true;
        let a = 10f64.powf(g as f64 / 40.0);
        let w0 = core::f64::consts::TAU * (fc as f64).min(0.45 * self.fs) / self.fs;
        let (sw, cw) = (w0.sin(), w0.cos());
        let s = 1.0f64; // shelf slope
        let alpha = sw / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) + (a - 1.0) * cw + two_sqrt_a_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cw);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cw - two_sqrt_a_alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cw + two_sqrt_a_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cw);
        let a2 = (a + 1.0) - (a - 1.0) * cw - two_sqrt_a_alpha;
        self.shelf_l.set_coeffs(b0, b1, b2, a0, a1, a2);
        self.shelf_r.set_coeffs(b0, b1, b2, a0, a1, a2);
    }

    /// Parametric bell: freq 200 Hz..12 kHz, gain -24..=12 dB, Q 0.3..=8.
    pub fn set_bell(&mut self, freq_hz: f32, gain_db: f32, q: f32) {
        let g = if gain_db.is_finite() { gain_db.clamp(-24.0, 12.0) } else { 0.0 };
        let fc = if freq_hz.is_finite() { freq_hz.clamp(200.0, 12000.0) } else { 3000.0 };
        let q = if q.is_finite() { q.clamp(0.3, 8.0) } else { 1.4 };
        self.bell_hz = fc;
        self.bell_db = g;
        self.bell_q = q;
        if g.abs() < 0.01 {
            self.bell_on = false;
            return;
        }
        self.bell_on = true;
        let a = 10f64.powf(g as f64 / 40.0);
        let w0 = core::f64::consts::TAU * (fc as f64).min(0.45 * self.fs) / self.fs;
        let alpha = w0.sin() / (2.0 * q as f64);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * w0.cos();
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = b1;
        let a2 = 1.0 - alpha / a;
        self.bell_l.set_coeffs(b0, b1, b2, a0, a1, a2);
        self.bell_r.set_coeffs(b0, b1, b2, a0, a1, a2);
    }

    pub fn shelf(&self) -> (f32, f32) {
        (self.shelf_db, self.shelf_hz)
    }

    pub fn bell(&self) -> (f32, f32, f32) {
        (self.bell_hz, self.bell_db, self.bell_q)
    }

    #[inline(always)]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (mut l, mut r) = (l, r);
        if self.shelf_on {
            l = self.shelf_l.process(l);
            r = self.shelf_r.process(r);
        }
        if self.bell_on {
            l = self.bell_l.process(l);
            r = self.bell_r.process(r);
        }
        (l, r)
    }

    pub fn reset(&mut self) {
        self.shelf_l.reset();
        self.shelf_r.reset();
        self.bell_l.reset();
        self.bell_r.reset();
    }
}

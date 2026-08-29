//! Deck audio DSP for the two-deck music mode: pitch-preserving time
//! stretching, a phase-coherent three-band split EQ with true kills, one
//! sweepable filter, and the stem-mix seam.
//!
//! Everything here is pure, deterministic and allocation-free once
//! constructed — the mixer's device callback runs it per frame, so every
//! buffer is sized at construction and never grows. `alloc_free_hot_path`
//! asserts that with a counting allocator.
//!
//! Design notes:
//!
//! - **Time stretch** is WSOLA (waveform-similarity overlap-add), not a
//!   phase vocoder. A deck only ever asks for small ratios (±8–16% for
//!   tempo matching); over that range WSOLA is transient-exact on
//!   percussive material — no phasiness, no smeared kicks — while costing
//!   a fraction of an FFT-based stretcher and needing no spectral state.
//!   The window search keeps successive grains waveform-aligned, so the
//!   pitch is untouched and the tempo follows the ratio exactly.
//! - **Scratching bypasses the stretcher entirely** and reads the source
//!   varispeed, because that IS the vinyl semantic: a hand on the record
//!   changes pitch with speed.
//! - **Ratio 1.0 bypasses the stretcher** and reads the source directly, so
//!   an untouched deck is sample-exact.
//! - **The EQ is a real crossover**, not a set of peaking bells: fourth-order
//!   Linkwitz-Riley splits at 250 Hz and 2.5 kHz, so a band gain of zero is
//!   a genuine kill (the band's signal is simply not summed back in) and
//!   unity gains sum flat. A wet/dry ramp bypasses the whole chain when
//!   every band sits at unity and the filter is centred, keeping an
//!   untouched deck bit-transparent.

use std::f32::consts::PI;

// ---------------------------------------------------------------------------
// frame sources (the stem-mix seam)
// ---------------------------------------------------------------------------

/// Random-access stereo source in SOURCE frames. Implemented by the mixer
/// over a deck's PCM (optionally over its separated stems).
pub trait FrameSource {
    fn frame_count(&self) -> usize;
    /// Sample at an integer source frame. Out-of-range reads return silence.
    fn frame(&self, index: usize) -> [f32; 2];
}

/// The four stem lanes a separated track carries. `Full` is the ordinary
/// case: one mixed file, no separation available or wanted.
///
/// This is the seam a stem-separation backend slots into: it publishes four
/// PCM buffers on the SAME timeline as the mixed file, the deck holds them
/// beside the full mix, and every lane gets its own gain. Nothing else in
/// the chain changes — the EQ, the stretcher and the transport all see one
/// stereo stream either way.
pub const STEM_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum StemKind {
    Vocals = 0,
    Drums = 1,
    Bass = 2,
    Other = 3,
}

impl StemKind {
    pub const ALL: [StemKind; STEM_COUNT] =
        [StemKind::Vocals, StemKind::Drums, StemKind::Bass, StemKind::Other];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            StemKind::Vocals => "VOCALS",
            StemKind::Drums => "DRUMS",
            StemKind::Bass => "BASS",
            StemKind::Other => "OTHER",
        }
    }
}

/// Linear interpolation between two stereo frames.
#[inline]
pub fn lerp_frame(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Catmull-Rom between `b` and `c`, with `a` and `d` as the shoulders.
///
/// A straight line between two samples is a poor guess at what the waveform
/// did in between, and the error is broadband hiss that rises with the
/// resampling ratio — which is exactly what a key shift asks for. A cubic
/// through four points costs a handful of multiplies and puts that hiss far
/// enough down to stop mattering. At `t == 0` it returns `b` unchanged, so
/// an unresampled deck is still the sample the decoder produced.
#[inline]
pub fn cubic_frame(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2], t: f32) -> [f32; 2] {
    let mut out = [0.0f32; 2];
    for channel in 0..2 {
        let (a, b, c, d) = (a[channel], b[channel], c[channel], d[channel]);
        let c0 = b;
        let c1 = 0.5 * (c - a);
        let c2 = a - 2.5 * b + 2.0 * c - 0.5 * d;
        let c3 = 0.5 * (d - a) + 1.5 * (b - c);
        out[channel] = ((c3 * t + c2) * t + c1) * t + c0;
    }
    out
}

// ---------------------------------------------------------------------------
// parameter ramps
// ---------------------------------------------------------------------------

/// Linear per-frame parameter ramp. A copy of the mixer's private ramp so
/// the DSP stays independently testable.
#[derive(Clone, Copy, Debug)]
pub struct ParamRamp {
    current: f32,
    target: f32,
    /// Units per second; 0 = settled.
    step: f32,
}

impl ParamRamp {
    pub fn at(value: f32) -> ParamRamp {
        ParamRamp { current: value, target: value, step: 0.0 }
    }

    pub fn slew(&mut self, target: f32, secs: f32) {
        self.target = target;
        let distance = (target - self.current).abs();
        self.step = if secs <= 0.0 { f32::MAX } else { (distance / secs).max(1e-6) };
    }

    pub fn jump(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.step = 0.0;
    }

    #[inline]
    pub fn tick(&mut self, rate: f32) -> f32 {
        if self.current != self.target {
            let per_frame = self.step / rate.max(1.0);
            let delta = self.target - self.current;
            if delta.abs() <= per_frame {
                self.current = self.target;
            } else {
                self.current += per_frame * delta.signum();
            }
        }
        self.current
    }

    pub fn current(&self) -> f32 {
        self.current
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn settled(&self) -> bool {
        self.current == self.target
    }
}

// ---------------------------------------------------------------------------
// scratch: vinyl rate ramps
// ---------------------------------------------------------------------------

/// Hand-on-the-record brake time: the platter stops fast but not instantly.
pub const SCRATCH_GRAB_SECS: f32 = 0.045;
/// Hand-off spin-up back to the deck's tempo.
pub const SCRATCH_RELEASE_SECS: f32 = 0.22;
/// How fast the rate follows the pointer while dragging.
pub const SCRATCH_TRACK_SECS: f32 = 0.010;

/// Vinyl-style rate override. While a pointer holds the waveform the deck's
/// rate follows the drag; on release it ramps back to the deck's tempo.
#[derive(Clone, Copy, Debug)]
pub struct ScratchRamp {
    rate: ParamRamp,
    /// A hand is on the record.
    held: bool,
    /// The ramp still owns the rate (releasing but not yet back at tempo).
    releasing: bool,
}

impl Default for ScratchRamp {
    fn default() -> Self {
        ScratchRamp {
            rate: ParamRamp::at(1.0),
            held: false,
            releasing: false,
        }
    }
}

impl ScratchRamp {
    /// Pointer down: brake to a stop from wherever the deck was.
    pub fn grab(&mut self, deck_rate: f32) {
        if !self.held && !self.releasing {
            self.rate.jump(deck_rate);
        }
        self.held = true;
        self.releasing = false;
        self.rate.slew(0.0, SCRATCH_GRAB_SECS);
    }

    /// Pointer motion: scrub at the drag rate (negative = backwards).
    pub fn drag(&mut self, rate: f32) {
        if !self.held {
            return;
        }
        self.rate.slew(rate.clamp(-MAX_SCRATCH_RATE, MAX_SCRATCH_RATE), SCRATCH_TRACK_SECS);
    }

    /// Pointer up: spin back up to the deck's tempo.
    pub fn release(&mut self, deck_rate: f32) {
        if !self.held {
            return;
        }
        self.held = false;
        self.releasing = true;
        self.rate.slew(deck_rate, SCRATCH_RELEASE_SECS);
    }

    /// True while the scratch ramp — not the deck tempo — owns playback.
    pub fn active(&self) -> bool {
        self.held || self.releasing
    }

    pub fn held(&self) -> bool {
        self.held
    }

    pub fn rate(&self) -> f32 {
        self.rate.current()
    }

    /// Advance one output frame; returns the current rate. The release ramp
    /// hands control back to the deck once it lands on the deck rate.
    #[inline]
    pub fn tick(&mut self, device_rate: f32, deck_rate: f32) -> f32 {
        if self.releasing {
            // Follow a tempo change made mid-release.
            if (self.rate.target() - deck_rate).abs() > 1e-6 {
                self.rate.slew(deck_rate, SCRATCH_RELEASE_SECS);
            }
        }
        let value = self.rate.tick(device_rate);
        if self.releasing && self.rate.settled() {
            self.releasing = false;
        }
        value
    }
}

/// Fastest scrub the deck will follow, source frames per output frame.
pub const MAX_SCRATCH_RATE: f32 = 8.0;

// ---------------------------------------------------------------------------
// WSOLA time stretch
// ---------------------------------------------------------------------------

/// Grain length. 2048 frames is ~43 ms at 48 kHz: long enough to hold a bass
/// period, short enough that a transient lands inside one grain.
pub const WSOLA_WINDOW: usize = 2048;
/// Synthesis hop. Half the window, so the Hann overlap-add sums to unity.
pub const WSOLA_HOP: usize = WSOLA_WINDOW / 2;
/// Alignment search radius around the ideal grain start.
pub const WSOLA_SEARCH: usize = 256;
/// Correlation length used to score an alignment.
pub const WSOLA_CORR: usize = 512;
/// Correlation subsampling: the similarity surface is smooth at audio rates,
/// so every other sample scores the same peak for half the work.
const WSOLA_CORR_STRIDE: usize = 2;
/// Ratios inside this band are treated as "no stretch" and bypass entirely.
pub const STRETCH_BYPASS_EPSILON: f64 = 1e-4;
/// Widest stretch the grain search can still track. A caller that splits a
/// tempo between the stretcher and a resampler must clamp to the SAME pair
/// and recover the resampler from the result, or the two disagree about the
/// ratio and the tempo quietly drifts.
pub const STRETCH_RATIO_MIN: f64 = 0.05;
pub const STRETCH_RATIO_MAX: f64 = 4.0;

/// Streaming WSOLA over a random-access source.
///
/// The stretcher owns the source position: `position()` is where the deck's
/// playhead actually is, and output frames come out at the source sample
/// rate with the original pitch.
pub struct Stretcher {
    /// Hann window, precomputed.
    window: Box<[f32; WSOLA_WINDOW]>,
    /// Overlap-add accumulator, per channel.
    ola: Box<[[f32; WSOLA_WINDOW]; 2]>,
    /// Frames of `ola` already handed out from the front of the current hop.
    emitted: usize,
    /// Whether `ola` currently holds a grain at all.
    primed: bool,
    /// Ideal source start of the next grain.
    anchor: f64,
    /// Source start actually chosen for the last grain.
    last_start: usize,
    ratio: f64,
    ended: bool,
}

impl Default for Stretcher {
    fn default() -> Self {
        Stretcher::new()
    }
}

impl Stretcher {
    pub fn new() -> Stretcher {
        let mut window = Box::new([0.0f32; WSOLA_WINDOW]);
        for (index, value) in window.iter_mut().enumerate() {
            // Periodic Hann: two of these at 50% overlap sum to exactly 1.
            *value = 0.5 - 0.5 * (2.0 * PI * index as f32 / WSOLA_WINDOW as f32).cos();
        }
        Stretcher {
            window,
            ola: Box::new([[0.0; WSOLA_WINDOW]; 2]),
            emitted: 0,
            primed: false,
            anchor: 0.0,
            last_start: 0,
            ratio: 1.0,
            ended: false,
        }
    }

    /// Jump the playhead. The overlap-add state is discarded, so the next
    /// grain starts clean.
    pub fn reset_to(&mut self, position: f64) {
        self.anchor = position.max(0.0);
        self.last_start = self.anchor as usize;
        self.primed = false;
        self.emitted = 0;
        self.ended = false;
        for channel in self.ola.iter_mut() {
            channel.fill(0.0);
        }
    }

    pub fn set_ratio(&mut self, ratio: f64) {
        self.ratio = ratio.clamp(STRETCH_RATIO_MIN, STRETCH_RATIO_MAX);
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Source position of the next frame this stretcher will emit.
    pub fn position(&self) -> f64 {
        // `anchor` already points at the grain AFTER the one being emitted,
        // so back out the part of the current grain still queued.
        let pending = if self.primed {
            (WSOLA_HOP - self.emitted) as f64
        } else {
            0.0
        };
        (self.anchor - pending * self.ratio).max(0.0)
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    /// One output frame at the source sample rate, pitch unchanged.
    /// `None` once the source has run out and looping is off.
    pub fn next<S: FrameSource>(&mut self, source: &S, loop_on: bool) -> Option<[f32; 2]> {
        if self.emitted >= WSOLA_HOP || !self.primed {
            if !self.advance(source, loop_on) {
                return None;
            }
        }
        let index = self.emitted;
        self.emitted += 1;
        Some([self.ola[0][index], self.ola[1][index]])
    }

    /// Slide the overlap-add buffer by one hop and mix in the next grain.
    fn advance<S: FrameSource>(&mut self, source: &S, loop_on: bool) -> bool {
        let len = source.frame_count();
        if len < WSOLA_WINDOW + 1 {
            self.ended = true;
            return false;
        }
        let last = len - WSOLA_WINDOW;
        if self.primed {
            // Shift the tail (the half that has not been emitted) to the
            // front and clear the rest, ready for the incoming grain.
            for channel in self.ola.iter_mut() {
                channel.copy_within(WSOLA_HOP.., 0);
                channel[WSOLA_HOP..].fill(0.0);
            }
        } else {
            for channel in self.ola.iter_mut() {
                channel.fill(0.0);
            }
        }

        if self.anchor > last as f64 {
            if !loop_on {
                self.ended = true;
                return false;
            }
            // Wrap to the head; the grain search re-aligns from there.
            self.anchor -= last as f64;
            self.primed = false;
            for channel in self.ola.iter_mut() {
                channel.fill(0.0);
            }
        }

        let ideal = (self.anchor as usize).min(last);
        let start = if self.primed {
            self.best_start(source, ideal, last)
        } else {
            ideal
        };
        for offset in 0..WSOLA_WINDOW {
            let frame = source.frame(start + offset);
            let weight = self.window[offset];
            self.ola[0][offset] += frame[0] * weight;
            self.ola[1][offset] += frame[1] * weight;
        }
        self.last_start = start;
        self.anchor += WSOLA_HOP as f64 * self.ratio;
        self.emitted = 0;
        if !self.primed {
            // The very first grain has no partner underneath it: its rising
            // Hann half would fade the track in. Prime by mixing the grain's
            // mirror so the head is at unity, then continue normally.
            for offset in 0..WSOLA_HOP {
                let frame = source.frame(start + offset);
                let weight = 1.0 - self.window[offset];
                self.ola[0][offset] += frame[0] * weight;
                self.ola[1][offset] += frame[1] * weight;
            }
            self.primed = true;
        }
        true
    }

    /// The grain start near `ideal` whose head best continues the waveform
    /// the previous grain was heading into.
    fn best_start<S: FrameSource>(&self, source: &S, ideal: usize, last: usize) -> usize {
        let template_at = self.last_start + WSOLA_HOP;
        if template_at + WSOLA_CORR >= source.frame_count() {
            return ideal;
        }
        let low = ideal.saturating_sub(WSOLA_SEARCH);
        let high = (ideal + WSOLA_SEARCH).min(last);
        if high <= low {
            return ideal.min(last);
        }
        let mut best = ideal.min(last);
        let mut best_score = f32::NEG_INFINITY;
        let mut candidate = low;
        while candidate <= high {
            if candidate + WSOLA_CORR >= source.frame_count() {
                break;
            }
            let mut dot = 0.0f32;
            let mut energy = 1e-9f32;
            let mut offset = 0;
            while offset < WSOLA_CORR {
                let a = source.frame(template_at + offset);
                let b = source.frame(candidate + offset);
                let am = a[0] + a[1];
                let bm = b[0] + b[1];
                dot += am * bm;
                energy += bm * bm;
                offset += WSOLA_CORR_STRIDE;
            }
            // Normalizing by the candidate's own energy keeps the search
            // from always jumping onto the loudest nearby transient.
            let score = dot / energy.sqrt();
            if score > best_score {
                best_score = score;
                best = candidate;
            }
            candidate += 1;
        }
        best
    }
}

// ---------------------------------------------------------------------------
// rate reader: source frames -> device frames
// ---------------------------------------------------------------------------

/// Pulls whole source frames and resamples them with a 4-point cubic. At
/// `step == 1.0` it is a pass-through: the output is the input frame for
/// frame.
///
/// It carries the device-rate conversion AND, when the stretcher has already
/// spent the tempo, the key shift — so this is the interpolator a transposed
/// deck is heard through.
#[derive(Clone, Copy, Debug, Default)]
pub struct RateReader {
    frac: f64,
    /// The read head sits between `cur` and `next`; `prev` and `next2` are
    /// the shoulders the cubic needs.
    prev: [f32; 2],
    cur: [f32; 2],
    next: [f32; 2],
    next2: [f32; 2],
    primed: bool,
    drained: bool,
}

impl RateReader {
    pub fn reset(&mut self) {
        *self = RateReader::default();
    }

    /// One output frame. `pull` yields consecutive source frames.
    pub fn read(
        &mut self,
        step: f64,
        pull: &mut impl FnMut() -> Option<[f32; 2]>,
    ) -> Option<[f32; 2]> {
        if self.drained {
            return None;
        }
        if !self.primed {
            let Some(first) = pull() else {
                self.drained = true;
                return None;
            };
            self.cur = first;
            self.next = pull().unwrap_or(first);
            self.next2 = pull().unwrap_or(self.next);
            // Nothing precedes the first frame, so carry the line backwards
            // rather than repeating it: a repeat is a corner, and a corner at
            // the head of every grain is a click.
            self.prev = [
                2.0 * self.cur[0] - self.next[0],
                2.0 * self.cur[1] - self.next[1],
            ];
            self.primed = true;
            self.frac = 0.0;
        }
        let out = cubic_frame(self.prev, self.cur, self.next, self.next2, self.frac as f32);
        self.frac += step.max(0.0);
        while self.frac >= 1.0 {
            self.frac -= 1.0;
            self.prev = self.cur;
            self.cur = self.next;
            self.next = self.next2;
            match pull() {
                Some(frame) => self.next2 = frame,
                None => {
                    self.drained = true;
                    break;
                }
            }
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// biquads
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BiquadState {
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn from_raw(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Biquad {
        let inv = 1.0 / a0;
        Biquad {
            b0: b0 * inv,
            b1: b1 * inv,
            b2: b2 * inv,
            a1: a1 * inv,
            a2: a2 * inv,
        }
    }

    fn shared(cutoff: f32, sample_rate: f32, q: f32) -> (f32, f32, f32) {
        let nyquist = sample_rate * 0.5;
        let cutoff = cutoff.clamp(10.0, nyquist * 0.98);
        let w0 = 2.0 * PI * cutoff / sample_rate;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.05));
        (cos_w0, alpha, w0)
    }

    pub fn lowpass(cutoff: f32, sample_rate: f32, q: f32) -> Biquad {
        let (cos_w0, alpha, _) = Biquad::shared(cutoff, sample_rate, q);
        let b1 = 1.0 - cos_w0;
        Biquad::from_raw(b1 * 0.5, b1, b1 * 0.5, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    pub fn highpass(cutoff: f32, sample_rate: f32, q: f32) -> Biquad {
        let (cos_w0, alpha, _) = Biquad::shared(cutoff, sample_rate, q);
        let b0 = (1.0 + cos_w0) * 0.5;
        Biquad::from_raw(b0, -(1.0 + cos_w0), b0, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    pub fn allpass(cutoff: f32, sample_rate: f32, q: f32) -> Biquad {
        let (cos_w0, alpha, _) = Biquad::shared(cutoff, sample_rate, q);
        Biquad::from_raw(
            1.0 - alpha,
            -2.0 * cos_w0,
            1.0 + alpha,
            1.0 + alpha,
            -2.0 * cos_w0,
            1.0 - alpha,
        )
    }

    #[inline]
    pub fn process(&self, state: &mut BiquadState, x: f32) -> f32 {
        // Transposed direct form II: one multiply-add chain, good f32
        // behaviour at low cutoffs.
        let y = self.b0 * x + state.z1;
        state.z1 = self.b1 * x - self.a1 * y + state.z2;
        state.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Butterworth Q values for a cascade of two biquads (4th order).
const BUTTERWORTH_Q4: [f32; 2] = [0.541_196_1, 1.306_562_9];
/// Linkwitz-Riley 4th order = two identical Butterworth (Q = 1/√2) sections.
const LR4_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

// ---------------------------------------------------------------------------
// three-band split EQ with true kills + one sweepable filter
// ---------------------------------------------------------------------------

/// Crossover between the low and mid bands.
pub const EQ_LOW_HZ: f32 = 250.0;
/// Crossover between the mid and high bands.
pub const EQ_HIGH_HZ: f32 = 2_500.0;
/// Highest boost a band knob can apply.
pub const EQ_MAX_GAIN: f32 = 2.0;
/// Below this a band gain counts as a kill.
pub const EQ_KILL_EPSILON: f32 = 1e-4;
/// Filter knob positions inside this band of centre count as "off".
pub const FILTER_DEADZONE: f32 = 0.02;
/// Low end of the low-pass sweep.
const FILTER_LP_MIN_HZ: f32 = 40.0;
/// Top of the low-pass sweep (effectively open).
const FILTER_LP_MAX_HZ: f32 = 20_000.0;
/// Bottom of the high-pass sweep (effectively open).
const FILTER_HP_MIN_HZ: f32 = 20.0;
/// Top of the high-pass sweep.
const FILTER_HP_MAX_HZ: f32 = 9_000.0;
/// Wet/dry crossfade when the chain engages or returns to unity.
const EQ_ENGAGE_SECS: f32 = 0.012;
/// Autopilot blend moves on an ENGAGED strip: fast enough to read as a cut
/// on the bar, slow enough never to click. Matches the mixer's stem-lane
/// blend so the EQ and stems media perform the same choreography at the
/// same speed.
const BLEND_ENGAGE_SECS: f32 = 0.08;

#[derive(Clone, Copy, Debug, Default)]
struct EqChannelState {
    /// Split at EQ_HIGH_HZ: low-pass pair then high-pass pair.
    split_lp: [BiquadState; 2],
    split_hp: [BiquadState; 2],
    /// Split at EQ_LOW_HZ inside the low-passed branch.
    band_lp: [BiquadState; 2],
    band_hp: [BiquadState; 2],
    /// Phase compensation for the high branch.
    band_ap: BiquadState,
    /// Sweepable filter, 4th order.
    sweep: [BiquadState; 2],
}

#[derive(Clone, Copy, Debug)]
struct EqCoeffs {
    split_lp: Biquad,
    split_hp: Biquad,
    band_lp: Biquad,
    band_hp: Biquad,
    band_ap: Biquad,
    sweep: [Biquad; 2],
    sweep_on: bool,
}

impl EqCoeffs {
    fn new(sample_rate: f32) -> EqCoeffs {
        EqCoeffs {
            split_lp: Biquad::lowpass(EQ_HIGH_HZ, sample_rate, LR4_Q),
            split_hp: Biquad::highpass(EQ_HIGH_HZ, sample_rate, LR4_Q),
            band_lp: Biquad::lowpass(EQ_LOW_HZ, sample_rate, LR4_Q),
            band_hp: Biquad::highpass(EQ_LOW_HZ, sample_rate, LR4_Q),
            band_ap: Biquad::allpass(EQ_LOW_HZ, sample_rate, LR4_Q),
            sweep: [Biquad::default(); 2],
            sweep_on: false,
        }
    }
}

/// One deck's tone chain: three-band split EQ (with kills) into one
/// sweepable low-pass / high-pass filter.
pub struct DeckEq {
    sample_rate: f32,
    coeffs: EqCoeffs,
    channels: [EqChannelState; 2],
    gain: [ParamRamp; 3],
    /// The autopilot's second pair of hands: multiplies the operator's
    /// band gains without ever moving them. 1.0 = hands off.
    blend: [ParamRamp; 3],
    /// Bipolar filter knob, 0.5 = off.
    filter: ParamRamp,
    /// Cutoff the coefficients were last built for.
    filter_built: f32,
    /// Crossfade between the dry input and the processed chain.
    wet: ParamRamp,
}

impl DeckEq {
    pub fn new(sample_rate: f32) -> DeckEq {
        DeckEq {
            sample_rate,
            coeffs: EqCoeffs::new(sample_rate),
            channels: [EqChannelState::default(); 2],
            gain: [ParamRamp::at(1.0); 3],
            blend: [ParamRamp::at(1.0); 3],
            filter: ParamRamp::at(0.5),
            filter_built: f32::NAN,
            wet: ParamRamp::at(0.0),
        }
    }

    /// Rebuild the fixed crossover coefficients for a new device rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if (self.sample_rate - sample_rate).abs() < 0.5 {
            return;
        }
        self.sample_rate = sample_rate;
        let sweep = self.coeffs.sweep;
        let sweep_on = self.coeffs.sweep_on;
        self.coeffs = EqCoeffs::new(sample_rate);
        self.coeffs.sweep = sweep;
        self.coeffs.sweep_on = sweep_on;
        self.filter_built = f32::NAN;
        self.reset();
    }

    pub fn reset(&mut self) {
        self.channels = [EqChannelState::default(); 2];
    }

    /// Band gain, 0 = kill, 1 = unity, up to [`EQ_MAX_GAIN`].
    pub fn set_band(&mut self, band: usize, gain: f32) {
        if band >= 3 {
            return;
        }
        self.gain[band].slew(gain.clamp(0.0, EQ_MAX_GAIN), EQ_ENGAGE_SECS);
    }

    pub fn band(&self, band: usize) -> f32 {
        self.gain.get(band).map(|g| g.target()).unwrap_or(1.0)
    }

    /// Autopilot blend factor for one band; composes with the operator's
    /// gain multiplicatively and never moves the knob. While the chain is
    /// disengaged (wet at zero — a cued deck, or an untouched strip) the
    /// factor JUMPS instead of slewing: nothing of it is audible yet, and a
    /// pre-mute set a bar before play must be fully seated when the deck
    /// starts, not still crossing its ramp.
    pub fn set_blend_band(&mut self, band: usize, gain: f32) {
        if band >= 3 {
            return;
        }
        let gain = gain.clamp(0.0, 1.0);
        if self.wet.current() <= 0.0 {
            self.blend[band].jump(gain);
        } else {
            self.blend[band].slew(gain, BLEND_ENGAGE_SECS);
        }
    }

    /// Ramp every blend factor home.
    pub fn clear_blend(&mut self) {
        for ramp in &mut self.blend {
            if self.wet.current() <= 0.0 {
                ramp.jump(1.0);
            } else {
                ramp.slew(1.0, BLEND_ENGAGE_SECS);
            }
        }
    }

    /// Snap the blend home instantly — a fresh track never inherits a
    /// transition's ducking.
    pub fn reset_blend(&mut self) {
        self.blend = [ParamRamp::at(1.0); 3];
    }

    #[cfg(test)]
    fn blend_current(&self, band: usize) -> f32 {
        self.blend[band].current()
    }

    /// Bipolar filter knob: 0 = full low-pass, 0.5 = off, 1 = full high-pass.
    pub fn set_filter(&mut self, position: f32) {
        self.filter.slew(position.clamp(0.0, 1.0), EQ_ENGAGE_SECS * 4.0);
    }

    pub fn filter(&self) -> f32 {
        self.filter.target()
    }

    /// True when every knob sits at unity/centre, so the chain can be
    /// bypassed and the deck stays bit-transparent.
    pub fn at_unity(&self) -> bool {
        self.gain.iter().all(|g| (g.target() - 1.0).abs() < EQ_KILL_EPSILON)
            && self.blend.iter().all(|g| (g.target() - 1.0).abs() < EQ_KILL_EPSILON)
            && (self.filter.target() - 0.5).abs() <= FILTER_DEADZONE
    }

    /// Rebuild rate-dependent coefficients. Called once per device buffer,
    /// never per frame — the trig is the expensive part and the ear cannot
    /// hear a cutoff quantized to one buffer.
    pub fn prepare_block(&mut self) {
        let position = self.filter.target();
        let engaged = !self.at_unity();
        self.wet.slew(if engaged { 1.0 } else { 0.0 }, EQ_ENGAGE_SECS);
        if (position - self.filter_built).abs() < 1e-4 {
            return;
        }
        self.filter_built = position;
        let centre = 0.5;
        if (position - centre).abs() <= FILTER_DEADZONE {
            self.coeffs.sweep_on = false;
            return;
        }
        self.coeffs.sweep_on = true;
        if position < centre {
            // Low-pass sweeping down as the knob turns left.
            let t = ((centre - position) / (centre - FILTER_DEADZONE)).clamp(0.0, 1.0);
            let cutoff = log_sweep(FILTER_LP_MAX_HZ, FILTER_LP_MIN_HZ, t);
            for (index, q) in BUTTERWORTH_Q4.iter().enumerate() {
                self.coeffs.sweep[index] = Biquad::lowpass(cutoff, self.sample_rate, *q);
            }
        } else {
            let t = ((position - centre) / (centre - FILTER_DEADZONE)).clamp(0.0, 1.0);
            let cutoff = log_sweep(FILTER_HP_MIN_HZ, FILTER_HP_MAX_HZ, t);
            for (index, q) in BUTTERWORTH_Q4.iter().enumerate() {
                self.coeffs.sweep[index] = Biquad::highpass(cutoff, self.sample_rate, *q);
            }
        }
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let gains = [
            self.gain[0].tick(device_rate) * self.blend[0].tick(device_rate),
            self.gain[1].tick(device_rate) * self.blend[1].tick(device_rate),
            self.gain[2].tick(device_rate) * self.blend[2].tick(device_rate),
        ];
        self.filter.tick(device_rate);
        let wet = self.wet.tick(device_rate);
        if wet <= 0.0 {
            // Untouched deck: the sample the decoder produced, unchanged.
            return frame;
        }
        let mut out = [0.0f32; 2];
        for channel in 0..2 {
            let x = frame[channel];
            let state = &mut self.channels[channel];
            // Split at the upper crossover.
            let mut low_branch = x;
            for index in 0..2 {
                low_branch = self.coeffs.split_lp.process(&mut state.split_lp[index], low_branch);
            }
            let mut high_branch = x;
            for index in 0..2 {
                high_branch =
                    self.coeffs.split_hp.process(&mut state.split_hp[index], high_branch);
            }
            // Split the lower branch again.
            let mut low = low_branch;
            for index in 0..2 {
                low = self.coeffs.band_lp.process(&mut state.band_lp[index], low);
            }
            let mut mid = low_branch;
            for index in 0..2 {
                mid = self.coeffs.band_hp.process(&mut state.band_hp[index], mid);
            }
            // The high branch takes the lower crossover's all-pass so all
            // three bands stay phase-coherent and sum flat at unity.
            let high = self.coeffs.band_ap.process(&mut state.band_ap, high_branch);

            let mut wet_sample = low * gains[0] + mid * gains[1] + high * gains[2];
            if self.coeffs.sweep_on {
                for index in 0..2 {
                    wet_sample =
                        self.coeffs.sweep[index].process(&mut state.sweep[index], wet_sample);
                }
            }
            out[channel] = x + (wet_sample - x) * wet;
        }
        out
    }
}

fn log_sweep(from: f32, to: f32, t: f32) -> f32 {
    let from = from.max(1.0);
    let to = to.max(1.0);
    from * (to / from).powf(t.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub mod alloc_probe {
    //! Thread-local allocation counter. Tests run in parallel, so a global
    //! counter would see every other test's allocations; a thread-local one
    //! measures exactly the code under test.
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    pub struct CountingAllocator;

    fn bump() {
        // `try_with` because the allocator may be called while thread-local
        // storage is being torn down.
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
    }

    pub fn count() -> usize {
        COUNT.with(|c| c.get())
    }

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            bump();
            System.alloc(layout)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            System.dealloc(ptr, layout)
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            bump();
            System.alloc_zeroed(layout)
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            bump();
            System.realloc(ptr, layout, new_size)
        }
    }
}

#[cfg(test)]
#[global_allocator]
static COUNTING_ALLOCATOR: alloc_probe::CountingAllocator = alloc_probe::CountingAllocator;

#[cfg(test)]
mod tests {
    use super::*;

    pub struct Buffer {
        frames: Vec<[f32; 2]>,
    }

    impl FrameSource for Buffer {
        fn frame_count(&self) -> usize {
            self.frames.len()
        }
        fn frame(&self, index: usize) -> [f32; 2] {
            self.frames.get(index).copied().unwrap_or([0.0, 0.0])
        }
    }

    fn sine(frequency: f64, rate: f64, seconds: f64) -> Buffer {
        let len = (rate * seconds) as usize;
        let mut frames = Vec::with_capacity(len);
        for index in 0..len {
            let value =
                (2.0 * std::f64::consts::PI * frequency * index as f64 / rate).sin() as f32;
            frames.push([value, value]);
        }
        Buffer { frames }
    }

    /// Frequency of the dominant partial, by counting positive-going zero
    /// crossings over a windowed stretch of a pure tone.
    fn measured_frequency(samples: &[f32], rate: f64) -> f64 {
        let mut crossings = 0usize;
        let mut first = None;
        let mut last = 0usize;
        for index in 1..samples.len() {
            if samples[index - 1] <= 0.0 && samples[index] > 0.0 {
                if first.is_none() {
                    first = Some(index);
                }
                last = index;
                crossings += 1;
            }
        }
        let Some(first) = first else { return 0.0 };
        if crossings < 2 {
            return 0.0;
        }
        (crossings - 1) as f64 * rate / (last - first) as f64
    }

    fn cents(a: f64, b: f64) -> f64 {
        1200.0 * (a / b).log2()
    }

    // ---- time stretch ----------------------------------------------------

    #[test]
    fn stretcher_keeps_pitch_and_changes_duration() {
        const RATE: f64 = 48_000.0;
        for ratio in [1.05, 0.92, 1.16] {
            let source = sine(440.0, RATE, 6.0);
            let mut stretcher = Stretcher::new();
            stretcher.set_ratio(ratio);
            stretcher.reset_to(0.0);
            let mut out = Vec::new();
            while let Some(frame) = stretcher.next(&source, false) {
                out.push(frame[0]);
                if out.len() > 400_000 {
                    break;
                }
            }
            // Duration: consuming N source frames at `ratio` must emit
            // N/ratio output frames.
            let expected = source.frame_count() as f64 / ratio;
            let error = (out.len() as f64 - expected).abs() / expected;
            assert!(
                error < 0.01,
                "ratio {ratio}: emitted {} frames, expected ~{expected:.0} ({:.3}% off)",
                out.len(),
                error * 100.0
            );
            // Pitch: unchanged, measured well inside the stream so the
            // priming grain is not part of the window.
            let window = &out[24_000..out.len().min(24_000 + 96_000)];
            let measured = measured_frequency(window, RATE);
            assert!(
                cents(measured, 440.0).abs() < 12.0,
                "ratio {ratio}: measured {measured:.2} Hz, {:.1} cents off",
                cents(measured, 440.0)
            );
        }
    }

    #[test]
    fn stretcher_position_tracks_the_source_playhead() {
        let source = sine(220.0, 48_000.0, 4.0);
        let mut stretcher = Stretcher::new();
        stretcher.set_ratio(1.10);
        stretcher.reset_to(0.0);
        let mut emitted = 0usize;
        while emitted < 60_000 {
            if stretcher.next(&source, false).is_none() {
                break;
            }
            emitted += 1;
        }
        let expected = emitted as f64 * 1.10;
        let position = stretcher.position();
        assert!(
            (position - expected).abs() < WSOLA_WINDOW as f64,
            "position {position:.0} vs expected {expected:.0}"
        );
    }

    #[test]
    fn stretcher_seek_and_short_sources_are_safe() {
        let source = sine(220.0, 48_000.0, 3.0);
        let mut stretcher = Stretcher::new();
        stretcher.set_ratio(1.0);
        stretcher.reset_to(96_000.0);
        assert!(stretcher.next(&source, false).is_some());
        assert!(stretcher.position() >= 96_000.0 - WSOLA_WINDOW as f64);
        // A source shorter than one grain never panics; it reports the end.
        let tiny = Buffer { frames: vec![[0.1, 0.1]; 100] };
        let mut stretcher = Stretcher::new();
        stretcher.reset_to(0.0);
        assert!(stretcher.next(&tiny, false).is_none());
        assert!(stretcher.ended());
    }

    #[test]
    fn stretcher_loops_instead_of_ending_when_asked() {
        let source = sine(220.0, 48_000.0, 1.0);
        let mut stretcher = Stretcher::new();
        stretcher.set_ratio(1.0);
        stretcher.reset_to(0.0);
        let mut emitted = 0usize;
        while emitted < 200_000 {
            if stretcher.next(&source, true).is_none() {
                break;
            }
            emitted += 1;
        }
        assert_eq!(emitted, 200_000, "a looping deck never runs out");
    }

    // ---- rate reader -----------------------------------------------------

    #[test]
    fn rate_reader_is_transparent_at_unity_step() {
        let frames: Vec<[f32; 2]> = (0..64)
            .map(|i| [i as f32 / 64.0, -(i as f32) / 64.0])
            .collect();
        let mut reader = RateReader::default();
        let mut index = 0usize;
        let mut pull = || {
            let out = frames.get(index).copied();
            index += 1;
            out
        };
        for expect in 0..32 {
            let got = reader.read(1.0, &mut pull).unwrap();
            let want = [expect as f32 / 64.0, -(expect as f32) / 64.0];
            assert_eq!(got, want, "frame {expect} must survive unchanged");
        }
    }

    #[test]
    fn rate_reader_resamples_and_reports_exhaustion() {
        let frames: Vec<[f32; 2]> = (0..8).map(|i| [i as f32, i as f32]).collect();
        let mut reader = RateReader::default();
        let mut index = 0usize;
        let mut pull = || {
            let out = frames.get(index).copied();
            index += 1;
            out
        };
        // Half rate: every other output frame lands halfway between inputs.
        let a = reader.read(0.5, &mut pull).unwrap();
        let b = reader.read(0.5, &mut pull).unwrap();
        assert_eq!(a[0], 0.0);
        assert!((b[0] - 0.5).abs() < 1e-6);
        let mut count = 2;
        while reader.read(0.5, &mut pull).is_some() {
            count += 1;
            assert!(count < 100, "reader must terminate");
        }
        assert!(count >= 8, "the whole buffer must be played back");
    }

    #[test]
    fn rate_reader_beats_a_straight_line_on_a_curve() {
        // The interpolator IS the pitch shifter once the stretcher has spent
        // the tempo, so measure it against the signal it is meant to
        // reconstruct: a sine read at an awkward ratio, scored against the
        // sine the read head was actually sitting on.
        let rate = 48_000.0;
        let hz = 1_000.0;
        let step = 1.5_f64;
        let frames: Vec<[f32; 2]> = (0..4096)
            .map(|i| {
                let phase = 2.0 * PI as f64 * hz * i as f64 / rate;
                [phase.sin() as f32, phase.sin() as f32]
            })
            .collect();

        let mut reader = RateReader::default();
        let mut index = 0usize;
        let mut pull = || {
            let out = frames.get(index).copied();
            index += 1;
            out
        };
        let mut cubic_err = 0.0f64;
        let mut linear_err = 0.0f64;
        let mut count = 0usize;
        // Skip the first few frames: the head has no real history, and the
        // extrapolated shoulder is a guess by construction.
        for out_index in 0..2_000 {
            let Some(got) = reader.read(step, &mut pull) else { break };
            if out_index < 4 {
                continue;
            }
            let source_pos = out_index as f64 * step;
            let want = (2.0 * PI as f64 * hz * source_pos / rate).sin();
            // What a straight line between the same two neighbours gives.
            let floor = source_pos.floor();
            let frac = source_pos - floor;
            let a = frames[floor as usize][0] as f64;
            let b = frames[floor as usize + 1][0] as f64;
            let linear = a + (b - a) * frac;
            cubic_err += (got[0] as f64 - want).powi(2);
            linear_err += (linear - want).powi(2);
            count += 1;
        }
        assert!(count > 1_000, "the test must actually measure something");
        let cubic_rms = (cubic_err / count as f64).sqrt();
        let linear_rms = (linear_err / count as f64).sqrt();
        assert!(
            cubic_rms < linear_rms * 0.25,
            "the cubic should beat a straight line by a wide margin: \
             cubic {cubic_rms:.6} vs linear {linear_rms:.6}"
        );
    }

    // ---- EQ --------------------------------------------------------------

    fn eq_response(eq: &mut DeckEq, frequency: f64, rate: f64) -> f64 {
        // Settle the ramps and filter state, then measure RMS gain.
        let settle = (rate * 0.6) as usize;
        let measure = (rate * 0.4) as usize;
        eq.prepare_block();
        let mut in_energy = 0.0f64;
        let mut out_energy = 0.0f64;
        for index in 0..settle + measure {
            let value =
                (2.0 * std::f64::consts::PI * frequency * index as f64 / rate).sin() as f32;
            let out = eq.process([value, value], rate as f32);
            if index >= settle {
                in_energy += (value as f64) * (value as f64);
                out_energy += (out[0] as f64) * (out[0] as f64);
            }
        }
        (out_energy / in_energy.max(1e-30)).sqrt()
    }

    fn db(value: f64) -> f64 {
        20.0 * value.max(1e-12).log10()
    }

    #[test]
    fn unity_eq_is_bit_transparent() {
        let mut eq = DeckEq::new(48_000.0);
        eq.prepare_block();
        assert!(eq.at_unity());
        for index in 0..4_000 {
            let value = ((index as f32) * 0.017).sin() * 0.6;
            let out = eq.process([value, -value], 48_000.0);
            assert_eq!(out, [value, -value], "untouched EQ must not alter a sample");
        }
    }

    #[test]
    fn engaged_eq_at_unity_gains_sums_flat() {
        // Bands at unity but the chain engaged (a kill on the way back to
        // unity, say): the three-band split must still sum flat.
        let rate = 48_000.0;
        for frequency in [60.0, 250.0, 800.0, 2_500.0, 6_000.0, 12_000.0] {
            let mut eq = DeckEq::new(rate as f32);
            eq.set_band(0, 1.0);
            // Force the wet path on without changing any gain.
            eq.wet.jump(1.0);
            let gain = eq_response(&mut eq, frequency, rate);
            assert!(
                db(gain).abs() < 0.5,
                "{frequency} Hz: {:.2} dB through a unity split",
                db(gain)
            );
        }
    }

    #[test]
    fn killing_the_low_band_removes_bass_and_leaves_treble() {
        let rate = 48_000.0;
        let mut eq = DeckEq::new(rate as f32);
        eq.set_band(0, 0.0);
        let bass = eq_response(&mut eq, 60.0, rate);
        assert!(
            db(bass) < -40.0,
            "a killed low band must remove 60 Hz, got {:.1} dB",
            db(bass)
        );

        let mut eq = DeckEq::new(rate as f32);
        eq.set_band(0, 0.0);
        let treble = eq_response(&mut eq, 5_000.0, rate);
        assert!(
            db(treble).abs() < 0.5,
            "killing bass must leave 5 kHz alone, got {:.2} dB",
            db(treble)
        );
    }

    #[test]
    fn killing_the_high_band_removes_treble_and_leaves_bass() {
        let rate = 48_000.0;
        let mut eq = DeckEq::new(rate as f32);
        eq.set_band(2, 0.0);
        let treble = eq_response(&mut eq, 10_000.0, rate);
        assert!(
            db(treble) < -40.0,
            "a killed high band must remove 10 kHz, got {:.1} dB",
            db(treble)
        );

        let mut eq = DeckEq::new(rate as f32);
        eq.set_band(2, 0.0);
        let bass = eq_response(&mut eq, 60.0, rate);
        assert!(
            db(bass).abs() < 0.5,
            "killing treble must leave 60 Hz alone, got {:.2} dB",
            db(bass)
        );
    }

    #[test]
    fn killing_the_mid_band_scoops_the_middle() {
        let rate = 48_000.0;
        let mut eq = DeckEq::new(rate as f32);
        eq.set_band(1, 0.0);
        let mid = eq_response(&mut eq, 800.0, rate);
        assert!(db(mid) < -30.0, "killed mid at 800 Hz: {:.1} dB", db(mid));
    }

    fn filtered(position: f32, frequency: f64, rate: f64) -> f64 {
        let mut eq = DeckEq::new(rate as f32);
        eq.set_filter(position);
        eq.filter.jump(position);
        eq.wet.jump(1.0);
        db(eq_response(&mut eq, frequency, rate))
    }

    #[test]
    fn the_sweep_filter_opens_and_closes() {
        let rate = 48_000.0;
        // Left of centre = low-pass. A quarter turn sits around 800 Hz:
        // the bass is untouched, the top is gone.
        assert!(filtered(0.25, 60.0, rate).abs() < 1.0, "{}", filtered(0.25, 60.0, rate));
        assert!(filtered(0.25, 8_000.0, rate) < -40.0, "{}", filtered(0.25, 8_000.0, rate));
        // Hard left closes the low-pass right down: even a mid tone goes.
        assert!(filtered(0.0, 1_000.0, rate) < -60.0, "{}", filtered(0.0, 1_000.0, rate));

        // Right of centre = high-pass, mirrored.
        assert!(filtered(0.75, 5_000.0, rate).abs() < 1.0, "{}", filtered(0.75, 5_000.0, rate));
        assert!(filtered(0.75, 60.0, rate) < -40.0, "{}", filtered(0.75, 60.0, rate));
        assert!(filtered(1.0, 1_000.0, rate) < -40.0, "{}", filtered(1.0, 1_000.0, rate));
    }

    #[test]
    fn a_centred_filter_is_off() {
        let mut eq = DeckEq::new(48_000.0);
        eq.set_filter(0.5);
        eq.prepare_block();
        assert!(eq.at_unity());
        assert!(!eq.coeffs.sweep_on);
    }

    // ---- scratch ---------------------------------------------------------

    #[test]
    fn scratch_brakes_on_grab_and_spins_back_up_on_release() {
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        assert!(!scratch.active());
        scratch.grab(1.0);
        assert!(scratch.active() && scratch.held());
        // Inside the brake time the platter reaches a stop.
        for _ in 0..(rate * SCRATCH_GRAB_SECS * 1.2) as usize {
            scratch.tick(rate, 1.0);
        }
        assert!(scratch.rate().abs() < 1e-3, "grab must stop the platter");

        // A drag scrubs at the pointer's rate, backwards included.
        scratch.drag(-2.0);
        for _ in 0..(rate * SCRATCH_TRACK_SECS * 2.0) as usize {
            scratch.tick(rate, 1.0);
        }
        assert!((scratch.rate() + 2.0).abs() < 0.05, "drag rate {}", scratch.rate());

        scratch.release(1.0);
        assert!(scratch.active() && !scratch.held());
        for _ in 0..(rate * SCRATCH_RELEASE_SECS * 1.2) as usize {
            scratch.tick(rate, 1.0);
        }
        assert!((scratch.rate() - 1.0).abs() < 1e-3, "release must reach tempo");
        assert!(!scratch.active(), "the deck owns the rate again");
    }

    #[test]
    fn a_release_follows_a_tempo_change_made_mid_ramp() {
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        scratch.grab(1.0);
        scratch.tick(rate, 1.0);
        scratch.release(1.0);
        // The pitch slider moves while the platter is spinning back up.
        for _ in 0..(rate * SCRATCH_RELEASE_SECS * 1.5) as usize {
            scratch.tick(rate, 1.08);
        }
        assert!((scratch.rate() - 1.08).abs() < 1e-3, "rate {}", scratch.rate());
        assert!(!scratch.active());
    }

    // ---- allocation ------------------------------------------------------

    #[test]
    fn alloc_free_hot_path() {
        let source = sine(440.0, 48_000.0, 4.0);
        let mut stretcher = Stretcher::new();
        stretcher.set_ratio(1.06);
        stretcher.reset_to(0.0);
        let mut eq = DeckEq::new(48_000.0);
        eq.set_band(0, 0.4);
        eq.set_filter(0.3);
        let mut reader = RateReader::default();
        let mut scratch = ScratchRamp::default();
        // Warm the chain up so nothing lazily initializes inside the probe.
        eq.prepare_block();
        for _ in 0..8_000 {
            let mut pull = || stretcher.next(&source, true);
            if let Some(frame) = reader.read(0.9188, &mut pull) {
                eq.process(frame, 48_000.0);
            }
            scratch.tick(48_000.0, 1.0);
        }

        let before = alloc_probe::count();
        for _ in 0..48_000 {
            let mut pull = || stretcher.next(&source, true);
            if let Some(frame) = reader.read(0.9188, &mut pull) {
                eq.process(frame, 48_000.0);
            }
            scratch.tick(48_000.0, 1.0);
        }
        eq.prepare_block();
        let after = alloc_probe::count();
        assert_eq!(
            after, before,
            "the deck DSP allocated {} times in one second of audio",
            after - before
        );
    }
    #[test]
    fn an_engaged_blend_defeats_the_unity_bypass() {
        let mut eq = DeckEq::new(48_000.0);
        assert!(eq.at_unity(), "fresh strip is bit-transparent");
        // The autopilot's hand alone must engage the chain, or a blend on
        // an untouched strip would be silently bypassed.
        eq.set_blend_band(0, 0.0);
        assert!(!eq.at_unity(), "a blended band engages the chain");
        eq.clear_blend();
        assert!(eq.at_unity(), "released, the strip is transparent again");
        // reset_blend is the instant form installs use.
        eq.set_blend_band(1, 0.3);
        eq.reset_blend();
        assert!(eq.at_unity());
    }

    #[test]
    fn a_silent_strip_seats_the_blend_instantly_and_an_engaged_one_glides() {
        // Cued deck (never processed → wet still 0): the pre-mute must be
        // fully seated the moment it is asked for, so the deck's first
        // audible frame is already bass-less.
        let mut eq = DeckEq::new(48000.0);
        eq.set_blend_band(0, 0.0);
        assert!((eq.blend_current(0) - 0.0).abs() < 1e-9, "snapped while silent");
        // Engage the strip (a real band cut, then audio flowing) and the
        // next blend move glides at the ~80 ms transition slew instead of
        // the 12 ms operator-engage ramp.
        eq.set_band(0, 0.5);
        // The mixer engages the chain per buffer; without prepare_block the
        // wet target never moves and the strip stays officially silent.
        eq.prepare_block();
        for _ in 0..4800 {
            eq.process([0.0, 0.0], 48000.0);
        }
        eq.set_blend_band(0, 1.0);
        for _ in 0..960 {
            // 20 ms: a 12 ms ramp would already have landed.
            eq.process([0.0, 0.0], 48000.0);
        }
        let mid = eq.blend_current(0);
        assert!(
            mid > 0.05 && mid < 0.5,
            "20 ms into an 80 ms glide the blend reads {mid}"
        );
        for _ in 0..9600 {
            eq.process([0.0, 0.0], 48000.0);
        }
        assert!((eq.blend_current(0) - 1.0).abs() < 1e-6, "landed");
    }

}

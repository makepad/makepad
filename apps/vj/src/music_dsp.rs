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

// ---------------------------------------------------------------------------
// the floating-point environment
// ---------------------------------------------------------------------------

/// A control value the engine will act on: `value` held inside its range,
/// or `None` when it is not a number the operator could have meant.
///
/// A clamp alone is not containment here. `f32::clamp` hands NaN straight
/// back, and it turns an infinity into a range END — so a division by zero
/// upstream arrives as master gain 0 (the room goes quiet) or as the
/// loudest the console can be. Neither is what anybody asked for, so a
/// value that is not finite moves nothing at all.
#[inline]
pub fn knob(value: f32, low: f32, high: f32) -> Option<f32> {
    value.is_finite().then(|| value.clamp(low, high))
}

/// The same rule for the controls the engine keeps in double precision:
/// tempo, key shift, seconds along a track.
#[inline]
pub fn knob64(value: f64, low: f64, high: f64) -> Option<f64> {
    value.is_finite().then(|| value.clamp(low, high))
}

/// A sample the bus can carry, or silence.
///
/// Nothing upstream should ever produce a number that is not one, but a
/// resonant filter driven past stability, a rate that came from a division
/// by zero, or one bad byte from a learned controller all can — and a
/// non-finite sample is not a click, it is a dead output until the device is
/// reopened, because it poisons every accumulator it touches. One test at
/// the mix point costs nothing measurable and cannot be reasoned around.
#[inline]
pub fn audible(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

/// Arm flush-to-zero on the calling thread: a result too small to be a
/// normal number becomes exactly zero, and a denormal input is read as zero.
///
/// Every recursive filter here — the crossover, the sweep, the stretcher's
/// overlap state — decays into the denormal range when its deck goes quiet,
/// and a denormal multiply costs the FPU a hundred times a normal one: the
/// classic crackle that appears exactly when a kill is engaged or a deck is
/// paused. The flag is per thread and cheap to set, and some hosts reset it
/// behind the app's back, so the device callback arms it every buffer.
pub fn flush_denormals_to_zero() {
    set_flush_denormals(true);
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(crate) fn set_flush_denormals(on: bool) {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64 as arch;
    #[cfg(target_arch = "x86")]
    use std::arch::x86 as arch;
    // MXCSR bit 15 flushes results, bit 6 reads denormal inputs as zero.
    const FLUSH_TO_ZERO: u32 = 1 << 15;
    const DENORMALS_ARE_ZERO: u32 = 1 << 6;
    // The control register is per thread; reading and writing it is two
    // instructions with no side effect beyond the two flags below.
    #[allow(deprecated)]
    unsafe {
        let current = arch::_mm_getcsr();
        let wanted = if on {
            current | FLUSH_TO_ZERO | DENORMALS_ARE_ZERO
        } else {
            current & !(FLUSH_TO_ZERO | DENORMALS_ARE_ZERO)
        };
        if wanted != current {
            arch::_mm_setcsr(wanted);
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn set_flush_denormals(on: bool) {
    // FPCR bit 24 is flush-to-zero for both results and inputs.
    const FLUSH_TO_ZERO: u64 = 1 << 24;
    unsafe {
        let current: u64;
        std::arch::asm!("mrs {0}, fpcr", out(reg) current);
        let wanted = if on { current | FLUSH_TO_ZERO } else { current & !FLUSH_TO_ZERO };
        if wanted != current {
            std::arch::asm!("msr fpcr, {0}", in(reg) wanted);
        }
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
pub(crate) fn set_flush_denormals(_on: bool) {}

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

    /// Move to `target` over `secs`. A target that is not a number is
    /// REFUSED rather than clamped: `f32::clamp` passes NaN straight
    /// through, and one NaN here settles nothing ever again — `current !=
    /// target` stays true for the rest of the session, so the ramp adds NaN
    /// to NaN forever and whatever it drives goes quiet until the track is
    /// reloaded. Keeping the last good value degrades a bad number into a
    /// knob that did not move.
    pub fn slew(&mut self, target: f32, secs: f32) {
        if !target.is_finite() {
            return;
        }
        self.target = target;
        let distance = (target - self.current).abs();
        self.step = if secs <= 0.0 { f32::MAX } else { (distance / secs).max(1e-6) };
    }

    pub fn jump(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
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
/// How fast the rate follows the pointer while dragging. This is the
/// output smoother's time constant, well inside the position loop's own,
/// so it damps the steps between pointer events without ringing.
pub const SCRATCH_TRACK_SECS: f32 = 0.010;
/// How hard the record is pulled back onto the finger, per second of
/// position error. One over this is the time the loop takes to close a
/// standing error, so it has to be quick enough to feel rigid and slow
/// enough that a display frame's worth of disagreement is not a lurch.
pub const SCRATCH_PULL: f32 = 8.0;
/// How far a hand may drag the record from where it says it is, in source
/// seconds. Past this the finger and the record have lost each other --
/// a stall, a seek underneath, an anchor from another track -- and
/// chasing it would be a jump, not a follow.
pub const SCRATCH_ERROR_MAX: f32 = 0.5;
/// A flick keeps spinning. The hand-off time grows with how far the
/// platter is from tempo, so a hard throw coasts and a gentle let-go does
/// not, and it is capped so nothing spins for a whole phrase.
pub const SCRATCH_THROW_MAX: f32 = 5.0;

// The MOTOR times. Everything above is a hand on the record; these are the
// platter driving itself, and they are here beside the others so every
// platter time is in one block.

/// Into reverse when the hold starts. Fast enough to read as a flip.
pub const CENSOR_FLIP_SECS: f32 = 0.006;
/// Out of reverse when it ends. This MUST stay under the seek blend the
/// landing hides beneath (`SEEK_XFADE_SECS` in the mixer), or a tail of
/// near-reversed audio pokes out past it. A test asserts the relation.
pub const CENSOR_RETURN_SECS: f32 = 0.004;
/// The reverse hold plays the record backwards at its own speed.
pub const CENSOR_RATE: f32 = -1.0;
/// A motor brake: the platter coasts to a stop the way a heavy one does.
pub const BRAKE_SECS: f32 = 0.9;
/// A spin-back is two legs: a hard throw backwards, then a fall to rest.
pub const SPINBACK_THROW_SECS: f32 = 0.05;
pub const SPINBACK_PEAK: f32 = -4.0;
pub const SPINBACK_FALL_SECS: f32 = 0.6;
/// And a start that winds up rather than cutting in.
pub const SOFT_START_SECS: f32 = 1.2;

/// What a motor gesture does the moment it reaches its target rate.
///
/// The distinction is load-bearing. A reverse HOLD must stay in charge for
/// as long as the operator holds it, however long that is; a brake is
/// finished when the platter stops and must hand the rate back, because a
/// motor that never retires holds the deck's sync servos and its frame
/// pump off for the rest of the set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotorEnd {
    /// Stay in charge until something else takes over.
    Hold,
    /// Hand the rate back to the deck.
    Retire,
    /// Run a second leg first: a throw backwards, and then the fall to rest.
    Then(f32, f32),
}

/// Vinyl-style rate override. While a pointer holds the waveform the deck's
/// rate follows the drag; on release it ramps back to the deck's tempo.
#[derive(Clone, Copy, Debug)]
pub struct ScratchRamp {
    rate: ParamRamp,
    /// A hand is on the record.
    held: bool,
    /// The ramp still owns the rate (releasing but not yet back at tempo).
    releasing: bool,
    /// A MOTOR gesture owns the rate: no hand is on the record, and the
    /// ramp does not chase the deck's tempo while it runs.
    motor: bool,
    /// What the motor gesture does when it lands, armed here so the
    /// callback never needs a timer of its own.
    end: MotorEnd,
    /// Where the finger says the record should be, in source seconds, and
    /// how fast the finger itself is travelling. Both come from the
    /// surface, which is the side with the pointer timestamps: a velocity
    /// worked out in the callback from one pointer hop and a sample clock
    /// is a hyperbola, not a speed.
    target_secs: f64,
    target_rate: f32,
    /// What the loop wraps have moved the record by since the hand landed.
    /// The SURFACE knows nothing about them — its anchor is where the
    /// finger touched down — so the correction is kept here and applied to
    /// every place it sends, rather than to the target once.
    wrap_offset: f64,
    /// A hand is dragging by POSITION. `held` alone is the brake that
    /// follows the hand landing; this says the loop has something to
    /// close on.
    tracking: bool,
    /// How long the release currently in flight takes. Carried rather than
    /// hard-coded because a censor hands back in four milliseconds and a
    /// soft start winds up over more than a second, and the tempo-chase
    /// below re-slews with whatever this says.
    release_secs: f32,
}

impl Default for ScratchRamp {
    fn default() -> Self {
        ScratchRamp {
            rate: ParamRamp::at(1.0),
            held: false,
            releasing: false,
            motor: false,
            end: MotorEnd::Retire,
            target_secs: 0.0,
            target_rate: 0.0,
            wrap_offset: 0.0,
            tracking: false,
            release_secs: SCRATCH_RELEASE_SECS,
        }
    }
}

impl ScratchRamp {
    /// Pointer down: brake to a stop from wherever the deck was.
    pub fn grab(&mut self, deck_rate: f32) {
        // `!active()` rather than the two flags: a hand landing on a
        // platter that a motor is still winding down brakes it from where
        // the motor got to, not from the deck's tempo.
        if !self.active() {
            self.rate.jump(deck_rate);
        }
        self.held = true;
        self.releasing = false;
        self.motor = false;
        self.tracking = false;
        self.wrap_offset = 0.0;
        self.end = MotorEnd::Retire;
        self.rate.slew(0.0, SCRATCH_GRAB_SECS);
    }

    /// Pointer motion, as a PLACE rather than a speed.
    ///
    /// `secs` is where on the record the finger is; `rate` is how fast the
    /// finger is travelling, measured on the surface where the pointer
    /// timestamps are. The rate is the feed-forward — it is what sustains
    /// a steady drag with no error at all — and the place is what closes
    /// the loop, so a clamp, a dropped frame or a coalesced event is
    /// recovered rather than lost forever.
    pub fn drag(&mut self, secs: f64, rate: f32) {
        if !self.held {
            return;
        }
        self.target_secs = secs + self.wrap_offset;
        self.target_rate = rate.clamp(-MAX_SCRATCH_RATE, MAX_SCRATCH_RATE);
        self.tracking = true;
    }

    /// The render wrapped the playhead through a loop. The finger did not,
    /// so its target comes round too — otherwise the error is a whole loop
    /// wide and the record runs at its clamp until the hand comes off.
    pub fn note_wrap(&mut self, delta_secs: f64) {
        if self.tracking {
            self.wrap_offset += delta_secs;
            self.target_secs += delta_secs;
        }
    }

    /// Pointer up: let the platter carry on and settle back to tempo.
    ///
    /// The hand-off time grows with how far the record is from the deck's
    /// own speed, so a flick coasts and a gentle let-go simply lands. It
    /// stays a LINEAR ramp: it reaches the tempo exactly and hands the
    /// rate back, where an asymptote would report a hand on the record for
    /// seconds after there was one.
    pub fn release(&mut self, deck_rate: f32) {
        let momentum = (self.rate.current() - deck_rate).abs();
        let stretch = (1.0 + momentum).min(SCRATCH_THROW_MAX);
        self.release_over(deck_rate, SCRATCH_RELEASE_SECS * stretch);
    }

    /// The same hand-back over a chosen time. A hand off the record takes
    /// the platter's own spin-up; a reverse hold has to be back inside the
    /// seek blend that hides its landing.
    pub fn release_over(&mut self, deck_rate: f32, secs: f32) {
        if !self.held && !self.motor {
            return;
        }
        self.held = false;
        self.motor = false;
        self.tracking = false;
        self.end = MotorEnd::Retire;
        self.releasing = true;
        self.release_secs = secs;
        self.rate.slew(deck_rate, secs);
    }

    /// Drive the platter with no hand on it. `end` says what happens when
    /// it lands: hold there, hand back, or run a second leg first.
    pub fn motor(&mut self, from: f32, target: f32, secs: f32, end: MotorEnd) {
        if !self.active() {
            self.rate.jump(from);
        }
        self.held = false;
        self.releasing = false;
        self.motor = true;
        self.end = end;
        self.rate.slew(target, secs);
    }

    /// Wind up to the deck's tempo from a standstill. This is a RELEASE, so
    /// it inherits the tempo chase below — over its own time, not the
    /// hand-off's.
    pub fn spin_up_from(&mut self, from: f32, deck_rate: f32, secs: f32) {
        self.rate.jump(from);
        self.held = false;
        self.motor = false;
        self.end = MotorEnd::Retire;
        self.releasing = true;
        self.release_secs = secs;
        self.rate.slew(deck_rate, secs);
    }

    /// True while the scratch ramp — not the deck tempo — owns playback.
    pub fn active(&self) -> bool {
        self.held || self.releasing || self.motor
    }

    /// A HAND is on the record. Deliberately not true for a motor gesture:
    /// the deck engine keys its phase-lock rules on the hand.
    pub fn held(&self) -> bool {
        self.held
    }

    /// The platter is driving itself.
    pub fn motoring(&self) -> bool {
        self.motor
    }

    pub fn rate(&self) -> f32 {
        self.rate.current()
    }

    /// Advance one output frame; returns the current rate. The release ramp
    /// hands control back to the deck once it lands on the deck rate.
    #[inline]
    pub fn tick(&mut self, device_rate: f32, deck_rate: f32, pos_secs: f64) -> f32 {
        // A hand dragging by POSITION runs its own loop: the finger's own
        // speed sustains the drag, and the error pulls the record back
        // onto the finger, so nothing lost along the way stays lost.
        if self.tracking {
            let error =
                ((self.target_secs - pos_secs) as f32).clamp(-SCRATCH_ERROR_MAX, SCRATCH_ERROR_MAX);
            let want =
                (self.target_rate + SCRATCH_PULL * error).clamp(-MAX_SCRATCH_RATE, MAX_SCRATCH_RATE);
            // One pole rather than a linear ramp: the steps between
            // pointer events are smoothed without a target to overshoot.
            // The reciprocal form is the tab's own, and needs no
            // transcendental in the callback.
            let pole = (1.0 / (SCRATCH_TRACK_SECS * device_rate.max(1.0))).min(1.0);
            let value = self.rate.current() + (want - self.rate.current()) * pole;
            self.rate.jump(value);
            return value;
        }
        if self.releasing {
            // Follow a tempo change made mid-release, over the time THIS
            // release was given. A censor hands back in four milliseconds
            // and re-slewing it over the hand-off's fifth of a second
            // would poke a tail of reversed audio past the seek blend.
            if (self.rate.target() - deck_rate).abs() > 1e-6 {
                self.rate.slew(deck_rate, self.release_secs);
            }
        }
        let value = self.rate.tick(device_rate);
        if self.releasing && self.rate.settled() {
            self.releasing = false;
            self.release_secs = SCRATCH_RELEASE_SECS;
        }
        // A motor that never retires leaves `active()` true for the rest of
        // the set, and with it the deck's sync servos, its follower and its
        // frame pump all held off. So it retires the moment it lands —
        // unless a second leg was armed, which starts here.
        if self.motor && self.rate.settled() {
            match self.end {
                MotorEnd::Then(target, secs) => {
                    self.end = MotorEnd::Retire;
                    self.rate.slew(target, secs);
                }
                MotorEnd::Retire => self.motor = false,
                MotorEnd::Hold => {}
            }
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
/// How many scores the correlation actually takes, after subsampling.
const WSOLA_CORR_TAPS: usize = WSOLA_CORR / WSOLA_CORR_STRIDE;
/// The grain search walks the radius in two gears. The coarse pass steps
/// this far, on the same bet the subsampling above already makes: the
/// similarity surface is smooth, so a peak is not missed between strides.
/// The fine pass then walks one frame at a time either side of what the
/// coarse pass found, far enough to reach any candidate it stepped over.
const WSOLA_COARSE_STRIDE: usize = 8;
const WSOLA_FINE_RADIUS: usize = WSOLA_COARSE_STRIDE - 1;
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
    /// The waveform the last grain was heading into, read once per search
    /// instead of once per candidate. The template is the same for every
    /// candidate, and re-reading it through the source was half the work
    /// -- and on a separated deck the source is a four-lane sum, so it was
    /// half of something expensive.
    template: Box<[f32; WSOLA_CORR_TAPS]>,
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
            *value = crate::dsp_math::hann(index, WSOLA_WINDOW);
        }
        Stretcher {
            window,
            ola: Box::new([[0.0; WSOLA_WINDOW]; 2]),
            emitted: 0,
            primed: false,
            anchor: 0.0,
            last_start: 0,
            template: Box::new([0.0; WSOLA_CORR_TAPS]),
            ratio: 1.0,
            ended: false,
        }
    }

    /// Jump the playhead. The overlap-add state is discarded, so the next
    /// grain starts clean.
    /// Take on another stretcher's whole overlap-add state.
    ///
    /// An instant double taken while the stretcher is live has to copy this
    /// as well as the position: the two decks are reading the same record
    /// at the same tempo, and grain streams that start at different points
    /// in their overlap beat against each other. `window` is the same
    /// constant table in both, so it is the one field not copied.
    ///
    /// The accumulator is copied INTO the box this stretcher already owns.
    /// The caller holds the audio state lock and must not allocate.
    pub fn copy_state_from(&mut self, other: &Stretcher) {
        *self.ola = *other.ola;
        self.emitted = other.emitted;
        self.primed = other.primed;
        self.anchor = other.anchor;
        self.last_start = other.last_start;
        self.ratio = other.ratio;
        self.ended = other.ended;
    }

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
    /// How well the grain starting at `candidate` continues the waveform
    /// the last grain was heading into. Higher is better.
    #[inline]
    fn score_at<S: FrameSource>(&self, source: &S, candidate: usize) -> f32 {
        let mut dot = 0.0f32;
        let mut energy = 1e-9f32;
        for (tap, want) in self.template.iter().enumerate() {
            let b = source.frame(candidate + tap * WSOLA_CORR_STRIDE);
            let bm = b[0] + b[1];
            dot += want * bm;
            energy += bm * bm;
        }
        // Normalizing by the candidate's own energy keeps the search from
        // always jumping onto the loudest nearby transient.
        dot / energy.sqrt()
    }

    /// The grain start near `ideal` whose head best continues the waveform
    /// the previous grain was heading into.
    ///
    /// Two gears rather than one. A full stride-one walk of the radius is
    /// five hundred and thirteen candidates, and on a deck reading four
    /// separated lanes that is most of a small buffer's whole budget --
    /// two such decks advancing a grain in the same buffer were over the
    /// deadline on their own. A coarse pass over the same radius followed
    /// by a fine walk around what it found costs about a ninth of that,
    /// on the same bet the correlation subsampling above already makes.
    fn best_start<S: FrameSource>(&mut self, source: &S, ideal: usize, last: usize) -> usize {
        let template_at = self.last_start + WSOLA_HOP;
        if template_at + WSOLA_CORR >= source.frame_count() {
            return ideal;
        }
        let low = ideal.saturating_sub(WSOLA_SEARCH);
        let high = (ideal + WSOLA_SEARCH).min(last);
        if high <= low {
            return ideal.min(last);
        }
        let reach = source.frame_count().saturating_sub(WSOLA_CORR + 1);
        let high = high.min(reach);
        if high <= low {
            return ideal.min(last);
        }
        // Once per search, not once per candidate.
        for (tap, slot) in self.template.iter_mut().enumerate() {
            let a = source.frame(template_at + tap * WSOLA_CORR_STRIDE);
            *slot = a[0] + a[1];
        }

        let mut best = ideal.min(high).max(low);
        let mut best_score = f32::NEG_INFINITY;
        let mut candidate = low;
        while candidate <= high {
            let score = self.score_at(source, candidate);
            if score > best_score {
                best_score = score;
                best = candidate;
            }
            candidate += WSOLA_COARSE_STRIDE;
        }
        // And again, one frame at a time, over what the coarse pass
        // stepped across.
        let fine_low = best.saturating_sub(WSOLA_FINE_RADIUS).max(low);
        let fine_high = (best + WSOLA_FINE_RADIUS).min(high);
        let mut candidate = fine_low;
        while candidate <= fine_high {
            let score = self.score_at(source, candidate);
            if score > best_score {
                best_score = score;
                best = candidate;
            }
            candidate += 1;
        }
        best
    }

    /// Where the last grain was taken from. For the tests: the search's
    /// only observable output.
    pub fn last_start(&self) -> usize {
        self.last_start
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
    ///
    /// FORWARD ONLY, by construction rather than by choice: `pull` is the
    /// time stretcher, which produces the next grain and cannot be asked
    /// for a previous one. A negative step therefore HOLDS the current
    /// frame rather than rewinding — and that is right, because reverse
    /// never comes through here. The render drops out of the stretcher the
    /// moment a hand or a motor owns the rate, and reads the source
    /// directly, where the playhead is free to travel either way.
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
        // See the note on `read`: the window only ever advances.
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
    /// The same filter at its PREVIOUS setting, kept running while a
    /// change crossfades. Its own copy of the state, so it can ring down
    /// on its own while the new filter is faded in over it. The new
    /// filter inherits the state when the change is a small one -- a
    /// knob dragged a little -- and starts from silence when the KIND of
    /// filter changes, because a low-pass's memory handed to a high-pass
    /// is not a small mismatch, it is a ring that dwarfs the signal.
    sweep_out: [BiquadState; 2],
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
    /// What the sweep was, for the frames it takes to hand over.
    sweep_prev: [Biquad; 2],
    sweep_prev_on: bool,
    /// Which side of the dead zone the sweep is on: -1 low-pass, 0 off,
    /// 1 high-pass. A change of side is a change of KIND, and that is
    /// what decides whether the new filter may keep the old one's memory.
    sweep_side: i8,
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
            sweep_prev: [Biquad::default(); 2],
            sweep_prev_on: false,
            sweep_side: 0,
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
    /// The autopilot's own hand on the sweep, as an OFFSET from wherever
    /// the operator left the knob. Rests at zero.
    blend_filter: ParamRamp,
    /// Cutoff the coefficients were last built for.
    filter_built: f32,
    /// Frames left in a filter handover; zero when nothing is changing.
    handover: u32,
    /// How hard the sweep rings; 1.0 is Butterworth, which is flat.
    resonance: f32,
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
            blend_filter: ParamRamp::at(0.0),
            filter_built: f32::NAN,
            handover: 0,
            resonance: 1.0,
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

    /// The autopilot's hand on the sweep filter, as an offset from the
    /// operator's knob rather than a value in its place.
    ///
    /// An offset because the knob is bipolar and belongs to the hand: a
    /// factor has no meaning on it, and writing a position would need the
    /// operator's own to be saved and put back. Zero is the identity, so a
    /// deck nobody has swept is bit-transparent, and clearing IS the
    /// restore. Seats instantly on a silent strip for the same reason the
    /// band factors do.
    pub fn set_blend_filter(&mut self, offset: f32) {
        let offset = offset.clamp(-1.0, 1.0);
        if self.wet.current() <= 0.0 {
            self.blend_filter.jump(offset);
        } else {
            self.blend_filter.slew(offset, BLEND_ENGAGE_SECS);
        }
    }

    /// Where the sweep actually sits: the hand's knob plus the autopilot's
    /// offset, clamped into the knob's own range.
    pub fn effective_filter(&self) -> f32 {
        (self.filter.target() + self.blend_filter.target()).clamp(0.0, 1.0)
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
        if self.wet.current() <= 0.0 {
            self.blend_filter.jump(0.0);
        } else {
            self.blend_filter.slew(0.0, BLEND_ENGAGE_SECS);
        }
    }

    /// Snap the blend home instantly — a fresh track never inherits a
    /// transition's ducking.
    pub fn reset_blend(&mut self) {
        self.blend = [ParamRamp::at(1.0); 3];
        self.blend_filter = ParamRamp::at(0.0);
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
            && (self.effective_filter() - 0.5).abs() <= FILTER_DEADZONE
    }

    /// How much the sweep may ring at its corner.
    ///
    /// Off is Butterworth -- the flattest four-pole there is, and what
    /// the sweep has always been. The two rungs above it are the sound a
    /// DJ filter is expected to make: a lift at the corner that turns a
    /// sweep from a curtain into a note. The top rung lifts the corner by
    /// about seven decibels where the ceiling is not in force, and hot
    /// material at the corner will meet the master clamp; that is the
    /// sound, and it is the operator's to reach for.
    ///
    /// Three rungs and not a knob, because the only free room is the
    /// kill-and-solo slot under the FILTER knob, and because a resonance
    /// that has to be dialled in is one nobody uses mid-mix.
    pub const RESONANCE_RUNGS: [f32; 3] = [1.0, 1.9, 3.2];

    /// How long a filter change takes to hand over, in frames.
    ///
    /// Five milliseconds at 48 kHz: long enough that the two filters'
    /// outputs are blended rather than switched even with the corner
    /// ringing at the top rung, short enough that a hand sweeping the
    /// knob hears a sweep and not a smear. A fixed length rather than a
    /// share of the buffer, so the answer does not change with the
    /// device's block size -- and a running handover is allowed to finish
    /// before the next one starts, for the same reason.
    const FILTER_HANDOVER_FRAMES: u32 = 256;

    /// Rebuild rate-dependent coefficients. Called once per device buffer,
    /// never per frame — the trig is the expensive part and the ear cannot
    /// hear a cutoff quantized to one buffer.
    pub fn prepare_block(&mut self) {
        let position = self.effective_filter();
        let engaged = !self.at_unity();
        self.wet.slew(if engaged { 1.0 } else { 0.0 }, EQ_ENGAGE_SECS);
        if (position - self.filter_built).abs() < 1e-4 {
            return;
        }
        // A running handover finishes first. Restarting it would drop the
        // outgoing filter at whatever weight it had reached, in one
        // sample -- a step whose size depends on how far the crossfade
        // had got, which is to say on the device's block size. The new
        // position is picked up on the first block after the crossfade
        // lands, at most five milliseconds late.
        if self.handover > 0 {
            return;
        }
        self.filter_built = position;
        // What the filter WAS keeps running for the handover: new
        // coefficients meeting the old filter's state is a discontinuity
        // in the output, which is a click, and no ramp on the outside can
        // take it back out.
        self.coeffs.sweep_prev = self.coeffs.sweep;
        self.coeffs.sweep_prev_on = self.coeffs.sweep_on;
        self.handover = Self::FILTER_HANDOVER_FRAMES;
        for channel in &mut self.channels {
            channel.sweep_out = channel.sweep;
        }
        let centre = 0.5;
        let side: i8 = if (position - centre).abs() <= FILTER_DEADZONE {
            0
        } else if position < centre {
            -1
        } else {
            1
        };
        // A change of KIND -- off to on, or across the dead zone -- starts
        // the new filter from silence. Its memory is the other filter's,
        // and a low-pass's memory handed to a high-pass rings at many
        // times the signal; the crossfade then hides a bounded start-up
        // instead of an unbounded ring. A small drag on the same side
        // keeps the memory, which is nearly right and settles at once.
        if side != self.coeffs.sweep_side {
            for channel in &mut self.channels {
                channel.sweep = [BiquadState::default(); 2];
            }
        }
        self.coeffs.sweep_side = side;
        if side == 0 {
            self.coeffs.sweep_on = false;
            return;
        }
        self.coeffs.sweep_on = true;
        let (cutoff, lowpass) = if side < 0 {
            // Low-pass sweeping down as the knob turns left.
            let t = ((centre - position) / (centre - FILTER_DEADZONE)).clamp(0.0, 1.0);
            (log_sweep(FILTER_LP_MAX_HZ, FILTER_LP_MIN_HZ, t), true)
        } else {
            let t = ((position - centre) / (centre - FILTER_DEADZONE)).clamp(0.0, 1.0);
            (log_sweep(FILTER_HP_MIN_HZ, FILTER_HP_MAX_HZ, t), false)
        };
        // The lift rides the low-Q section only. The corner's height is
        // the product of the two sections' Qs whichever takes it, and the
        // slope past the corner is the order's; what the choice sets is
        // the WIDTH of the peak, and a broad hump reads as a sweep where a
        // narrow one reads as a whistle.
        let lift = self.resonance.min(resonance_ceiling(cutoff));
        for (index, q) in BUTTERWORTH_Q4.iter().enumerate() {
            let q = if index == 0 { *q * lift } else { *q };
            self.coeffs.sweep[index] = match lowpass {
                true => Biquad::lowpass(cutoff, self.sample_rate, q),
                false => Biquad::highpass(cutoff, self.sample_rate, q),
            };
        }
    }

    /// How hard the sweep rings at its corner. Rebuilds on the next block
    /// and hands over like any other change, so it can be turned mid-mix.
    pub fn set_resonance(&mut self, lift: f32) {
        // Never above the top rung: the ceiling plateaus there, and a lift
        // past it would step at the plateau's edge.
        let lift = match lift.is_finite() {
            true => lift.clamp(1.0, Self::RESONANCE_RUNGS[2]),
            false => 1.0,
        };
        if (lift - self.resonance).abs() < 1e-6 {
            return;
        }
        self.resonance = lift;
        // Force the rebuild: the POSITION has not moved, and that is what
        // the built-coefficient guard watches.
        self.filter_built = f32::NAN;
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
        self.blend_filter.tick(device_rate);
        let wet = self.wet.tick(device_rate);
        // Counted once per FRAME, not per channel: the two channels are
        // the same moment in time and must land on the same blend.
        let handover_after = self.handover.saturating_sub(1);
        let blend = 1.0 - self.handover as f32 / Self::FILTER_HANDOVER_FRAMES as f32;
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

            let banded = low * gains[0] + mid * gains[1] + high * gains[2];
            let mut wet_sample = banded;
            if self.coeffs.sweep_on {
                for index in 0..2 {
                    wet_sample =
                        self.coeffs.sweep[index].process(&mut state.sweep[index], wet_sample);
                }
            }
            // Both filters run while the handover lasts, on the same
            // input, and the output walks from one to the other. The old
            // one is fed even when it is being faded out: a biquad that
            // stops seeing input does not hold its last output, it rings
            // down, and the ring is what would be heard.
            if self.handover > 0 {
                let mut going = banded;
                if self.coeffs.sweep_prev_on {
                    for index in 0..2 {
                        going = self.coeffs.sweep_prev[index]
                            .process(&mut state.sweep_out[index], going);
                    }
                }
                wet_sample = going + (wet_sample - going) * blend;
            }
            out[channel] = x + (wet_sample - x) * wet;
        }
        self.handover = handover_after;
        out
    }
}

/// What resonance a sweep may actually have with its corner HERE.
///
/// A four-pole filter asked to ring hard with its corner at the edge of
/// hearing is a filter asked to make a whistle or a rumble: a resonant
/// corner on the bass is a sub-bass boost, and one on the air is a sine
/// wave. That is true at the bottom of the low-pass sweep and at the
/// START of the high-pass one -- both put the corner on the bass -- so
/// the ceiling is a function of the corner's frequency and not of how
/// far the knob has travelled, and it guards both ends of both sides.
///
/// Full lift from about 90 Hz to 6 kHz, coming down to flat by 40 Hz
/// and by 14 kHz, with a soft knee. Never below flat.
fn resonance_ceiling(corner_hz: f32) -> f32 {
    const FLAT_LOW_HZ: f32 = 40.0;
    const FULL_LOW_HZ: f32 = 90.0;
    const FULL_HIGH_HZ: f32 = 6_000.0;
    const FLAT_HIGH_HZ: f32 = 14_000.0;
    let corner = if corner_hz.is_finite() { corner_hz.max(1.0) } else { 1.0 };
    let reach = if corner < FULL_LOW_HZ {
        (corner / FLAT_LOW_HZ).ln() / (FULL_LOW_HZ / FLAT_LOW_HZ).ln()
    } else if corner > FULL_HIGH_HZ {
        (FLAT_HIGH_HZ / corner).ln() / (FLAT_HIGH_HZ / FULL_HIGH_HZ).ln()
    } else {
        return f32::INFINITY;
    };
    let reach = reach.clamp(0.0, 1.0);
    let ceiling = 1.0 + (DeckEq::RESONANCE_RUNGS[2] - 1.0) * reach * reach;
    ceiling.max(1.0)
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
    /// The resonance a sweep is allowed, and where it is taken away:
    /// at both ends of hearing, whichever side of the sweep put the
    /// corner there.
    #[test]
    fn resonance_is_reined_in_at_the_ends_of_hearing() {
        // Through the middle of the music the operator gets what they
        // asked for.
        for hz in [90.0, 200.0, 800.0, 2_000.0, 6_000.0] {
            assert!(resonance_ceiling(hz).is_infinite(), "{hz}");
        }
        // Down towards the bass it comes down, and it is flat by 40 Hz:
        // that is the bottom of the low-pass sweep AND the first engaged
        // position of the high-pass one, and a resonant corner on the
        // bass is a sub-bass boost either way.
        let near = resonance_ceiling(70.0);
        let further = resonance_ceiling(50.0);
        assert!(near > further, "{near} then {further}");
        assert!((resonance_ceiling(40.0) - 1.0).abs() < 1e-6);
        assert!((resonance_ceiling(FILTER_HP_MIN_HZ) - 1.0).abs() < 1e-6, "the high-pass's near end");
        assert!((resonance_ceiling(FILTER_LP_MIN_HZ) - 1.0).abs() < 1e-6, "the low-pass's far end");
        // Up towards the air likewise: flat by 14 kHz, which covers the
        // top of the high-pass sweep and the start of the low-pass one.
        assert!(resonance_ceiling(8_000.0) > resonance_ceiling(11_000.0));
        assert!((resonance_ceiling(14_000.0) - 1.0).abs() < 1e-6);
        assert!((resonance_ceiling(FILTER_LP_MAX_HZ) - 1.0).abs() < 1e-6, "the low-pass's near end");
        // Never below flat, and never above the top rung, whatever it is
        // asked -- including nonsense.
        for hz in [0.0, 1.0, 39.0, 41.0, 9_000.0, 20_000.0, 1e9, f32::NAN] {
            let ceiling = resonance_ceiling(hz);
            assert!(ceiling >= 1.0, "{hz}");
            assert!(ceiling.is_infinite() || ceiling <= DeckEq::RESONANCE_RUNGS[2], "{hz}");
        }
    }

    /// Turning RES mid-play hands over like a knob move does. The tab's
    /// absolute click rule cannot gate this one -- a corner ringing at
    /// the top rung steps by more than that every sample on its own --
    /// so the toggle is held to the steady state it toggles between.
    #[test]
    fn turning_resonance_mid_play_adds_no_step_of_its_own() {
        let rate = 48_000.0f32;
        let worst = |toggle_at: Option<usize>| {
            let mut eq = DeckEq::new(rate);
            eq.set_resonance(DeckEq::RESONANCE_RUNGS[2]);
            // The corner on the tone, which is the loudest the ring gets.
            eq.set_filter(0.1517);
            let mut phase = 0.0f32;
            let mut out = Vec::with_capacity(12_000);
            for block in 0..48 {
                if Some(block) == toggle_at {
                    eq.set_resonance(1.0);
                }
                eq.prepare_block();
                for _ in 0..256 {
                    phase += 2.0 * std::f32::consts::PI * 220.0 / rate;
                    let x = phase.sin() * 0.5;
                    out.push(eq.process([x, x], rate)[0]);
                }
            }
            out[6_000..]
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .fold(0.0f32, f32::max)
        };
        let steady = worst(None);
        let toggled = worst(Some(30));
        assert!(
            toggled <= steady * 1.05 + 1e-4,
            "the toggle stepped by {toggled} against a steady {steady}"
        );
    }

    /// The device's block size must not change what a sweep sounds like.
    /// A second move landing while a handover is still running waits for
    /// it, rather than dropping the outgoing filter at whatever weight it
    /// had reached.
    #[test]
    fn a_second_filter_move_waits_for_the_running_handover() {
        let rate = 48_000.0f32;
        for block_frames in [16usize, 32, 64, 256, 2048] {
            let mut eq = DeckEq::new(rate);
            let mut phase = 0.0f32;
            let mut out = Vec::with_capacity(8192);
            let mut render = |eq: &mut DeckEq, frames: usize, out: &mut Vec<f32>, phase: &mut f32| {
                eq.prepare_block();
                for _ in 0..frames {
                    *phase += 2.0 * std::f32::consts::PI * 220.0 / rate;
                    let x = phase.sin() * 0.5;
                    out.push(eq.process([x, x], rate)[0]);
                }
            };
            eq.set_filter(0.2);
            let mut rendered = 0;
            while rendered < 2048 {
                render(&mut eq, block_frames, &mut out, &mut phase);
                rendered += block_frames;
            }
            let settled = out.len();
            eq.set_filter(0.85);
            render(&mut eq, block_frames, &mut out, &mut phase);
            eq.set_filter(0.86);
            let mut rendered = 0;
            while rendered < 2048 {
                render(&mut eq, block_frames, &mut out, &mut phase);
                rendered += block_frames;
            }
            let worst = out[settled - 1..]
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 0.02, "{block_frames}-frame blocks stepped by {worst}");
        }
    }

    /// Resonance is a lift at the corner: with the sweep parked as a
    /// low-pass, a tone sitting AT the corner comes out louder with it
    /// than without, and one well below is untouched.
    #[test]
    fn resonance_lifts_the_corner_and_leaves_the_passband_alone() {
        let rate = 48_000.0f32;
        let level = |lift: f32, hz: f32| {
            let mut eq = DeckEq::new(rate);
            eq.set_resonance(lift);
            // Mid-travel on the low-pass side, well clear of the clamp.
            eq.set_filter(0.25);
            eq.prepare_block();
            let mut phase = 0.0f32;
            let mut peak = 0.0f32;
            for n in 0..12_000 {
                phase += 2.0 * std::f32::consts::PI * hz / rate;
                let x = phase.sin() * 0.5;
                let y = eq.process([x, x], rate)[0];
                // Ignore the settling, measure the steady state.
                if n > 6_000 {
                    peak = peak.max(y.abs());
                }
            }
            peak
        };
        // The corner at a quarter travel, from the sweep's own law.
        let corner = {
            let t = 0.25f32 / (0.5 - FILTER_DEADZONE);
            log_sweep(FILTER_LP_MAX_HZ, FILTER_LP_MIN_HZ, t)
        };
        let flat = level(1.0, corner);
        let rung = level(DeckEq::RESONANCE_RUNGS[2], corner);
        assert!(rung > flat * 1.3, "flat {flat}, resonant {rung}");
        // Deep in the passband nothing moves. Not two octaves down but
        // four: a resonant section's shape reaches further than its peak,
        // and a couple of percent at two octaves is the filter's own
        // arithmetic rather than a fault.
        let flat_low = level(1.0, corner / 16.0);
        let rung_low = level(DeckEq::RESONANCE_RUNGS[2], corner / 16.0);
        assert!((rung_low - flat_low).abs() < 0.01, "{flat_low} vs {rung_low}");
    }

    /// A filter sweep is a gesture, and a gesture must not click.
    ///
    /// Moving the knob replaces four biquads' coefficients while their
    /// state is mid-ring. Handed straight over, the output steps; the two
    /// filters are run side by side and blended instead.
    #[test]
    fn a_filter_change_hands_over_without_a_step() {
        let rate = 48_000.0f32;
        let mut eq = DeckEq::new(rate);
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(4096);
        // A tone the filter really acts on, so a discontinuity in the
        // filter shows up in the output.
        let mut render = |eq: &mut DeckEq, frames: usize, out: &mut Vec<f32>, phase: &mut f32| {
            eq.prepare_block();
            for _ in 0..frames {
                *phase += 2.0 * std::f32::consts::PI * 220.0 / rate;
                let x = phase.sin() * 0.5;
                out.push(eq.process([x, x], rate)[0]);
            }
        };
        // Settle with the filter well into the low-pass side.
        eq.set_filter(0.2);
        render(&mut eq, 2048, &mut out, &mut phase);
        let settled = out.len();
        // Then a big jump the other way, which is the worst a hand can do.
        eq.set_filter(0.85);
        render(&mut eq, 2048, &mut out, &mut phase);
        let worst = out[settled - 1..]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        // The tone itself steps by about 0.014 per sample at 220 Hz and
        // half scale, so anything under the click rule is the filter
        // adding nothing of its own.
        assert!(worst < 0.02, "a filter change stepped by {worst}");
    }

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
        crate::dsp_math::ratio_to_db_f64(value)
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
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!(scratch.rate().abs() < 1e-3, "grab must stop the platter");

        // A drag runs the record backwards under a finger going backwards.
        // The record is HELD at zero here, so the error grows with every
        // frame and the pull adds to the feed-forward; what is pinned is
        // the direction and that the clamp is respected.
        scratch.drag(-2.0, -2.0);
        for _ in 0..(rate * SCRATCH_TRACK_SECS * 2.0) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!(scratch.rate() <= -2.0, "a backward drag runs back: {}", scratch.rate());
        assert!(scratch.rate() >= -MAX_SCRATCH_RATE, "and inside the clamp");

        scratch.release(1.0);
        assert!(scratch.active() && !scratch.held());
        // The hand-off is longer the further the platter is from tempo, so
        // this let-go from full reverse takes its longest.
        for _ in 0..(rate * SCRATCH_RELEASE_SECS * SCRATCH_THROW_MAX * 1.2) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!((scratch.rate() - 1.0).abs() < 1e-3, "release must reach tempo");
        assert!(!scratch.active(), "the deck owns the rate again");
    }

    #[test]
    fn a_release_follows_a_tempo_change_made_mid_ramp() {
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        scratch.grab(1.0);
        scratch.tick(rate, 1.0, 0.0);
        scratch.release(1.0);
        // The pitch slider moves while the platter is spinning back up.
        for _ in 0..(rate * SCRATCH_RELEASE_SECS * 3.0) as usize {
            scratch.tick(rate, 1.08, 0.0);
        }
        assert!((scratch.rate() - 1.08).abs() < 1e-3, "rate {}", scratch.rate());
        assert!(!scratch.active());
    }



    /// What the grain search costs, per second of stretched audio, on the
    /// machine that runs it. Opt-in, because a wall-clock number is a
    /// property of the machine and not of the code -- but it is the only
    /// way to see the search's cost against the buffer deadline it has to
    /// fit inside, which is what the two-gear walk exists for.
    ///
    /// `cargo test -p makepad-vj --release grain_search_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "opt-in: reports a wall-clock cost, not a pass or fail"]
    fn grain_search_cost() {
        let rate = 48_000.0;
        let source = sine(220.0, rate, 30.0);
        let mut stretch = Stretcher::new();
        stretch.set_ratio(1.25);
        stretch.reset_to(0.0);
        // One second of output, so the figure reads per second per deck.
        let began = std::time::Instant::now();
        let mut frames = 0usize;
        let mut sink = 0.0f32;
        while frames < rate as usize {
            let Some(out) = stretch.next(&source, false) else { break };
            sink += out[0];
            frames += 1;
        }
        assert!(sink.is_finite());
        let took = began.elapsed().as_secs_f64() * 1000.0;
        let grains = frames as f64 / WSOLA_HOP as f64;
        println!(
            "grain search: {took:.2} ms per second of stretched audio,              {:.4} ms per grain ({grains:.0} grains); a 64-frame buffer at              48 kHz is 1.33 ms",
            took / grains,
        );
    }

    // ---- the hand and the record, closed ---------------------------------

    /// Drive the ramp the way the callback does: one tick per frame, with
    /// the position the record has actually reached fed back in.
    struct Platter {
        scratch: ScratchRamp,
        pos: f64,
        rate: f32,
        device: f32,
    }

    impl Platter {
        fn new(pos: f64) -> Platter {
            Platter { scratch: ScratchRamp::default(), pos, rate: 48_000.0, device: 48_000.0 }
        }
        /// One frame. Returns the rate the record is running at.
        fn frame(&mut self, deck_rate: f32) -> f32 {
            let rate = self.scratch.tick(self.device, deck_rate, self.pos);
            if self.scratch.active() {
                self.pos += rate as f64 / self.rate as f64;
            } else {
                self.pos += deck_rate as f64 / self.rate as f64;
            }
            rate
        }
        fn run(&mut self, secs: f32, deck_rate: f32) {
            for _ in 0..(self.rate * secs) as usize {
                self.frame(deck_rate);
            }
        }
    }

    #[test]
    fn a_dragged_record_ends_up_where_the_finger_put_it() {
        // The whole point of closing the loop. Every loss along the way --
        // a clamp, a slew, a dropped frame, a coalesced event -- used to be
        // drift the record never recovered.
        let mut p = Platter::new(10.0);
        p.scratch.grab(1.0);
        p.run(0.05, 1.0);
        // The finger walks the record forward a second, in twelve hops,
        // and one hop is dropped entirely.
        for hop in 1..=12 {
            if hop != 7 {
                p.scratch.drag(10.0 + hop as f64 / 12.0, 1.0);
            }
            p.run(1.0 / 12.0, 1.0);
        }
        // And stops. The surface says so — a finger that has held still
        // for a moment publishes the same place at no speed.
        p.scratch.drag(11.0, 0.0);
        p.run(0.6, 1.0);
        assert!(
            (p.pos - 11.0).abs() < 0.01,
            "the record lands where the finger left it: want 11.0, got {}",
            p.pos,
        );
    }

    #[test]
    fn a_record_dragged_at_a_steady_speed_is_followed_without_a_standing_lag() {
        // Feed-forward: the finger's own speed comes in from the surface
        // that has the timestamps, so a steady drag needs no error to
        // sustain it and the record does not trail behind.
        let mut p = Platter::new(10.0);
        p.scratch.grab(1.0);
        p.run(0.05, 1.0);
        let speed = 2.0f64;
        let hop = 1.0 / 120.0;
        for step in 1..=120 {
            p.scratch.drag(10.0 + speed * hop * step as f64, speed as f32);
            p.run(hop as f32, 1.0);
        }
        let rate = p.frame(1.0);
        assert!(
            (rate - speed as f32).abs() < 0.05,
            "the record runs at the finger's speed, got {rate}",
        );
        let want = 10.0 + speed * hop * 120.0;
        assert!((p.pos - want).abs() < 0.02, "and is where it should be: {} of {want}", p.pos);
    }

    #[test]
    fn a_held_still_finger_stops_the_record() {
        let mut p = Platter::new(10.0);
        p.scratch.grab(1.0);
        p.run(0.05, 1.0);
        p.scratch.drag(10.5, 1.0);
        p.run(0.4, 1.0);
        // The target stops moving and the speed reported goes to zero.
        p.scratch.drag(10.5, 0.0);
        p.run(0.8, 1.0);
        let before = p.pos;
        p.run(0.2, 1.0);
        assert!((p.pos - before).abs() < 1e-3, "the record stands still: {before} -> {}", p.pos);
        assert!((p.pos - 10.5).abs() < 0.01, "under the finger, at {}", p.pos);
    }

    #[test]
    fn a_flick_coasts_further_than_a_gentle_let_go() {
        let coast = |speed: f32| {
            let mut p = Platter::new(10.0);
            p.scratch.grab(1.0);
            p.run(0.02, 1.0);
            let hop = 1.0 / 120.0;
            for step in 1..=30 {
                p.scratch.drag(10.0 + speed as f64 * hop * step as f64, speed);
                p.run(hop as f32, 1.0);
            }
            p.scratch.release(1.0);
            let mut frames = 0usize;
            while p.scratch.active() && frames < 48_000 * 4 {
                p.frame(1.0);
                frames += 1;
            }
            frames as f32 / 48_000.0
        };
        let gentle = coast(1.1);
        let hard = coast(6.0);
        assert!(
            hard > gentle * 1.5,
            "a hard flick keeps spinning: gentle {gentle:.3} s, hard {hard:.3} s",
        );
        assert!(hard < 4.0, "but it does come to rest, at {hard:.3} s");
    }

    #[test]
    fn a_loop_wrap_carries_the_finger_target_with_the_record() {
        // Without this the record wraps and the finger does not, so the
        // error becomes a whole loop and the controller drives the record
        // at its clamp until the hand comes off.
        let mut p = Platter::new(9.9);
        p.scratch.grab(1.0);
        p.run(0.05, 1.0);
        p.scratch.drag(10.4, 1.0);
        // The render wrapped the head back into a one-second span.
        p.pos -= 1.0;
        p.scratch.note_wrap(-1.0);
        // The surface knows nothing of the wrap: its anchor is where the
        // finger landed, so it goes on saying 10.4.
        p.scratch.drag(10.4, 0.0);
        p.run(0.8, 1.0);
        assert!(
            (p.pos - 9.4).abs() < 0.02,
            "the target came round with the record, to {}",
            p.pos,
        );
    }
    // ---- the platter driving itself --------------------------------------

    #[test]
    fn the_streaming_reader_is_forward_only_and_a_negative_step_holds() {
        // The contract, written down so nobody "fixes" the clamp into a
        // rewind the stretcher cannot honour: `pull` produces the NEXT
        // grain and has no previous one to give. Reverse lives on the
        // direct read in the mixer, not here.
        let mut source = (0..64).map(|i| [i as f32, i as f32]);
        let mut pull = || source.next();
        let mut reader = RateReader::default();
        let first = reader.read(1.0, &mut pull).expect("a frame");
        let mut held = reader.read(0.0, &mut pull).expect("a frame");
        assert!(held[0] >= first[0], "a zero step never goes backwards");
        for _ in 0..8 {
            let next = reader.read(-1.0, &mut pull).expect("a frame");
            assert_eq!(next, held, "a negative step holds rather than rewinds");
            held = next;
        }
    }

    #[test]
    fn a_motor_gesture_drives_the_platter_with_no_hand_on_it_and_retires() {
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        scratch.motor(1.0, 0.0, BRAKE_SECS, MotorEnd::Retire);
        assert!(scratch.active(), "the ramp owns the rate");
        assert!(!scratch.held(), "but no hand is on the record");
        assert!(scratch.motoring());
        for _ in 0..(rate * BRAKE_SECS * 1.2) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!(scratch.rate().abs() < 1e-3, "the platter stopped");
        // The load-bearing half: a motor that never retires holds the
        // deck's sync servos and its frame pump off for the rest of the set.
        assert!(!scratch.active(), "and handed the rate back");
        assert!(!scratch.motoring());
    }

    #[test]
    fn a_throw_backwards_falls_to_a_stop_on_its_own() {
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        scratch.motor(
            1.0,
            SPINBACK_PEAK,
            SPINBACK_THROW_SECS,
            MotorEnd::Then(0.0, SPINBACK_FALL_SECS),
        );
        for _ in 0..(rate * SPINBACK_THROW_SECS * 1.1) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!(
            (scratch.rate() - SPINBACK_PEAK).abs() < SPINBACK_PEAK.abs() * 0.05,
            "the throw reaches its peak, got {}",
            scratch.rate(),
        );
        assert!(scratch.active(), "and the second leg is still to come");
        for _ in 0..(rate * SPINBACK_FALL_SECS * 1.2) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!(scratch.rate().abs() < 1e-3, "the fall reaches rest");
        assert!(!scratch.active(), "with no timer on the caller thread");
    }

    #[test]
    fn a_motor_holds_its_rate_where_a_release_chases_the_tempo() {
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        // The pitch slider moves throughout. A brake does not follow it.
        scratch.motor(1.0, 0.0, BRAKE_SECS, MotorEnd::Retire);
        for _ in 0..(rate * BRAKE_SECS * 1.2) as usize {
            scratch.tick(rate, 1.08, 0.0);
        }
        assert!(scratch.rate().abs() < 1e-3, "a brake stops at zero, not at tempo");

        // A soft start IS a release, so it chases — but over its OWN time,
        // not the hand-off's. Ticked with a tempo that differs from the one
        // it was handed, so the chase actually fires.
        scratch.spin_up_from(0.0, 1.0, SOFT_START_SECS);
        for _ in 0..(rate * SCRATCH_RELEASE_SECS * 1.5) as usize {
            scratch.tick(rate, 1.08, 0.0);
        }
        assert!(
            scratch.active(),
            "the wind-up must still be running well past the hand-off time, at {}",
            scratch.rate(),
        );
        for _ in 0..(rate * SOFT_START_SECS) as usize {
            scratch.tick(rate, 1.08, 0.0);
        }
        assert!((scratch.rate() - 1.08).abs() < 1e-3, "rate {}", scratch.rate());
        assert!(!scratch.active());
    }

    #[test]
    fn a_hand_landing_on_a_winding_down_platter_takes_it_over_from_there() {
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        scratch.motor(1.0, 0.0, BRAKE_SECS, MotorEnd::Retire);
        for _ in 0..(rate * BRAKE_SECS / 3.0) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        let coasting = scratch.rate();
        assert!(coasting < 1.0 && coasting > 0.0, "mid-brake, got {coasting}");
        scratch.grab(1.0);
        let after = scratch.tick(rate, 1.0, 0.0);
        assert!(
            after <= coasting + 1e-4,
            "a hand must not snap the platter back up to tempo first: {coasting} -> {after}",
        );
        assert!(scratch.held() && !scratch.motoring());
    }

    #[test]
    fn a_reverse_hold_stays_in_charge_for_as_long_as_it_is_held() {
        // A brake is finished when the platter stops. A hold is not
        // finished until somebody lets go, however long it runs.
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        scratch.motor(1.0, CENSOR_RATE, CENSOR_FLIP_SECS, MotorEnd::Hold);
        for _ in 0..(rate * CENSOR_FLIP_SECS * 20.0) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!((scratch.rate() - CENSOR_RATE).abs() < 1e-3);
        assert!(scratch.active() && scratch.motoring(), "still holding");
        scratch.release_over(1.0, CENSOR_RETURN_SECS);
        for _ in 0..(rate * CENSOR_RETURN_SECS * 1.2) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!(!scratch.active(), "and lets go when told");
    }

    #[test]
    fn a_short_hand_back_returns_from_reverse_inside_the_seek_blend() {
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        scratch.motor(1.0, CENSOR_RATE, CENSOR_FLIP_SECS, MotorEnd::Hold);
        for _ in 0..(rate * CENSOR_FLIP_SECS * 1.2) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!((scratch.rate() - CENSOR_RATE).abs() < 1e-3, "in reverse");
        scratch.release_over(1.0, CENSOR_RETURN_SECS);
        for _ in 0..(rate * CENSOR_RETURN_SECS * 1.2) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        assert!((scratch.rate() - 1.0).abs() < 1e-3, "back at tempo");
        assert!(!scratch.active());
        // The landing hides under the seek blend. Longer than the blend and
        // a tail of near-reversed audio pokes out past it.
        assert!(
            CENSOR_RETURN_SECS <= 0.005,
            "the hand-back must fit inside SEEK_XFADE_SECS",
        );
    }

    #[test]
    fn a_short_hand_back_keeps_its_own_time_when_the_tempo_moves_under_it() {
        // The chase used to re-slew every release over the hand-off's fifth
        // of a second, whatever time the release was given.
        let rate = 48_000.0f32;
        let mut scratch = ScratchRamp::default();
        scratch.motor(1.0, CENSOR_RATE, CENSOR_FLIP_SECS, MotorEnd::Hold);
        for _ in 0..(rate * CENSOR_FLIP_SECS * 1.2) as usize {
            scratch.tick(rate, 1.0, 0.0);
        }
        scratch.release_over(1.0, CENSOR_RETURN_SECS);
        for _ in 0..(rate * CENSOR_RETURN_SECS * 1.5) as usize {
            scratch.tick(rate, 1.05, 0.0);
        }
        assert!(
            !scratch.active(),
            "the hand-back must land in its own time, not the pointer's: {}",
            scratch.rate(),
        );
    }
    // ---- numbers that are not numbers -------------------------------------

    #[test]
    fn a_ramp_keeps_its_last_good_value_when_handed_one_that_is_not_a_number() {
        // Left to itself a ramp is poisoned for the session: `current !=
        // target` is true forever once either is NaN, so every tick adds NaN
        // to NaN and the band never sounds again until the track is reloaded.
        let mut ramp = ParamRamp::at(0.75);
        ramp.slew(f32::NAN, 0.01);
        assert_eq!(ramp.target(), 0.75, "the target must not move");
        for _ in 0..64 {
            assert!(ramp.tick(48_000.0).is_finite());
        }
        assert_eq!(ramp.current(), 0.75);
        ramp.jump(f32::INFINITY);
        assert_eq!(ramp.current(), 0.75, "a jump refuses it too");
        ramp.slew(0.25, 0.01);
        for _ in 0..4_800 {
            ramp.tick(48_000.0);
        }
        assert!((ramp.current() - 0.25).abs() < 1e-6, "and the ramp still works after");
    }

    #[test]
    fn a_tone_control_handed_one_that_is_not_a_number_stays_where_it_was() {
        let mut eq = DeckEq::new(48_000.0);
        eq.set_band(0, 0.4);
        eq.set_filter(0.3);
        eq.set_band(0, f32::NAN);
        eq.set_filter(f32::NAN);
        assert_eq!(eq.band(0), 0.4);
        assert_eq!(eq.filter(), 0.3);
        eq.prepare_block();
        for _ in 0..512 {
            let out = eq.process([0.5, -0.5], 48_000.0);
            assert!(out[0].is_finite() && out[1].is_finite(), "the chain stays finite");
        }
    }

    #[test]
    fn a_control_handed_one_that_is_not_a_number_moves_nothing() {
        assert_eq!(knob(0.5, 0.0, 1.2), Some(0.5));
        assert_eq!(knob(2.0, 0.0, 1.2), Some(1.2), "an ordinary value still clamps");
        assert_eq!(knob(-1.0, 0.0, 1.2), Some(0.0));
        assert_eq!(knob(f32::NAN, 0.0, 1.2), None);
        assert_eq!(knob(f32::INFINITY, 0.0, 1.2), None, "never the loudest the console can be");
        assert_eq!(knob(f32::NEG_INFINITY, 0.0, 1.2), None, "and never silence");
        assert_eq!(knob64(1.5, 0.5, 2.0), Some(1.5));
        assert_eq!(knob64(f64::NAN, 0.5, 2.0), None);
        assert_eq!(knob64(f64::INFINITY, 0.5, 2.0), None);
    }

    #[test]
    fn a_sample_that_is_not_a_number_leaves_the_bus_as_silence() {
        assert_eq!(audible(f32::NAN), 0.0);
        assert_eq!(audible(f32::INFINITY), 0.0);
        assert_eq!(audible(f32::NEG_INFINITY), 0.0);
        assert_eq!(audible(0.25), 0.25, "an ordinary sample passes untouched");
        assert_eq!(audible(-0.25), -0.25);
        assert_eq!(audible(0.0), 0.0);
    }

    // ---- floating-point environment ---------------------------------------

    #[test]
    fn flush_denormals_to_zero_makes_a_denormal_product_vanish() {
        // A filter that has gone quiet decays into the denormal range, where
        // every multiply costs a hundred times more; the audio thread wants
        // those flushed to exactly zero. The product below is denormal on
        // every target this runs on.
        let tiny = std::hint::black_box(f32::MIN_POSITIVE);
        let half = std::hint::black_box(0.5f32);
        flush_denormals_to_zero();
        assert_eq!(tiny * half, 0.0, "armed: a denormal product flushes to zero");
        // The flag is per thread and this thread runs other tests: put it
        // back, and prove the arithmetic is honest again.
        set_flush_denormals(false);
        assert!(
            std::hint::black_box(tiny) * std::hint::black_box(half) > 0.0,
            "disarmed: the denormal is a number again"
        );
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
            scratch.tick(48_000.0, 1.0, 0.0);
        }

        let before = alloc_probe::count();
        for _ in 0..48_000 {
            let mut pull = || stretcher.next(&source, true);
            if let Some(frame) = reader.read(0.9188, &mut pull) {
                eq.process(frame, 48_000.0);
            }
            scratch.tick(48_000.0, 1.0, 0.0);
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
    fn the_blend_filter_offsets_the_knob_and_lets_go_of_it() {
        let mut eq = DeckEq::new(48_000.0);
        assert!(eq.at_unity(), "fresh strip is bit-transparent");

        // An offset, not a value: it composes with wherever the hand left
        // the knob, and rests at zero so an untouched deck is untouched.
        eq.set_blend_filter(-0.5);
        assert!(!eq.at_unity(), "a swept filter engages the chain");
        assert!((eq.effective_filter() - 0.0).abs() < 1e-6, "centre and a full sweep down");

        eq.set_filter(0.7);
        assert!((eq.effective_filter() - 0.2).abs() < 1e-6, "the hand and the sweep add");

        // Past either end it clamps rather than wrapping into the far side.
        eq.set_blend_filter(-1.0);
        assert!((eq.effective_filter() - 0.0).abs() < 1e-6);

        eq.clear_blend();
        assert!((eq.effective_filter() - 0.7).abs() < 1e-6, "the knob is where the hand left it");
        eq.set_filter(0.5);
        assert!(eq.at_unity(), "released, the strip is transparent again");
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

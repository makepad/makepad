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
/// The ratio at which the stretcher ENGAGES; it disengages back at
/// [`STRETCH_BYPASS_EPSILON`]. The gap is hysteresis: a sync servo trimming
/// the rate around unity would otherwise switch the stretcher in and out
/// many times a second, and every switch re-seats the playhead.
pub const STRETCH_ENGAGE_EPSILON: f64 = 1e-3;
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

#[derive(Clone, Copy, Debug, PartialEq)]
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

    /// A peaking bell: `gain` above one lifts a hump at `cutoff`, below
    /// one digs a notch, and exactly one is a straight wire.
    pub fn peaking(cutoff: f32, sample_rate: f32, q: f32, gain: f32) -> Biquad {
        let (cos_w0, alpha, _) = Biquad::shared(cutoff, sample_rate, q);
        let a = gain.max(1e-4).sqrt();
        Biquad::from_raw(
            1.0 + alpha * a,
            -2.0 * cos_w0,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos_w0,
            1.0 - alpha / a,
        )
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

/// Where the low and mid bands part company by default.
pub const EQ_LOW_HZ: f32 = 250.0;
/// Where the mid and high bands part company by default.
pub const EQ_HIGH_HZ: f32 = 2_500.0;
/// How far either crossover may be moved. The lower one stays out of the
/// sub-bass, where moving it turns the LOW knob into a rumble control;
/// the upper one stops short of the air band, where a HIGH knob that
/// reaches down into the presence range stops being a treble control and
/// starts being a vocal one.
pub const EQ_LOW_HZ_MIN: f32 = 80.0;
pub const EQ_LOW_HZ_MAX: f32 = 800.0;
pub const EQ_HIGH_HZ_MIN: f32 = 1_000.0;
pub const EQ_HIGH_HZ_MAX: f32 = 8_000.0;
/// The two are kept this far apart, in octaves, so the mid band always
/// has something in it. Crossed or touching corners do not make a
/// three-band EQ with an empty middle, they make an unstable one.
const EQ_CROSSOVER_MIN_OCTAVES: f32 = 1.0;

/// The pair of corners a request actually lands on: each inside its own
/// range, and the two at least [`EQ_CROSSOVER_MIN_OCTAVES`] apart.
///
/// Public because the control side has to store exactly what the engine
/// will run. Two clamps that agree today and drift tomorrow is how a
/// readout starts lying about what is being heard.
pub fn eq_crossovers_for(low_hz: f32, high_hz: f32) -> Option<(f32, f32)> {
    let low = knob(low_hz, EQ_LOW_HZ_MIN, EQ_LOW_HZ_MAX)?;
    let high = knob(high_hz, EQ_HIGH_HZ_MIN, EQ_HIGH_HZ_MAX)?;
    let gap = 2f32.powf(EQ_CROSSOVER_MIN_OCTAVES);
    let high = (high.max(low * gap)).min(EQ_HIGH_HZ_MAX);
    // If the ceiling stopped the push, the lower one gives way instead:
    // the gap is the invariant, not either corner.
    let low = low.min(high / gap);
    Some((low, high))
}
/// How much of the remaining distance a crossover closes each block
/// while it travels.
///
/// It travels rather than jumps because a jumped corner clicks either
/// way: rebuild the band filters and clear their memory and the step
/// measures 0.45, keep the memory and feed it through corners it was not
/// built for and it still measures 0.16, against this file's budget of
/// 0.02. Walking it in small steps is what makes each rebuild small
/// enough to be inaudible -- the same answer the sweep's own corner
/// takes, for the same reason.
const EQ_CROSSOVER_GLIDE: f32 = 0.12;
/// How broad a boost bell is. Under one octave wide at the half-way
/// point: wide enough to read as "more bass" rather than as a resonance,
/// narrow enough that lifting the low band does not drag the mids up
/// with it.
const EQ_BELL_Q: f32 = 0.9;
/// Close enough to be there. An exponential walk never quite arrives, so
/// the last sliver is taken in one step -- a hundredth of an octave,
/// which is under a percent of the corner and far below what a whole
/// jump costs.
const EQ_CROSSOVER_SNAP_OCTAVES: f32 = 0.01;
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

/// Where the sweep's corner sits for a knob at `position`: `None` inside
/// the dead zone about centre, else the corner in Hz and whether it is
/// the high-pass (`true`) or the low-pass. The one place a knob's travel
/// becomes a frequency -- `prepare_block` builds its coefficients from
/// it, so a readout that asks it cannot disagree with what is playing.
/// Pure and cheap: a compare and a power, per block, never per frame.
pub fn filter_corner_hz(position: f32) -> Option<(bool, f32)> {
    let centre = 0.5;
    if (position - centre).abs() <= FILTER_DEADZONE {
        return None;
    }
    if position < centre {
        // Low-pass sweeping down as the knob turns left.
        let t = ((centre - position) / (centre - FILTER_DEADZONE)).clamp(0.0, 1.0);
        Some((false, log_sweep(FILTER_LP_MAX_HZ, FILTER_LP_MIN_HZ, t)))
    } else {
        let t = ((position - centre) / (centre - FILTER_DEADZONE)).clamp(0.0, 1.0);
        Some((true, log_sweep(FILTER_HP_MIN_HZ, FILTER_HP_MAX_HZ, t)))
    }
}
/// Autopilot blend moves on an ENGAGED strip: fast enough to read as a cut
/// on the bar, slow enough never to click. Matches the mixer's stem-lane
/// blend so the EQ and stems media perform the same choreography at the
/// same speed.
const BLEND_ENGAGE_SECS: f32 = 0.08;

/// Where each band's bell sits, given the corners in force. The outer
/// two have no centre of their own -- a band that runs to DC or to
/// Nyquist has no middle -- so they take their corner shifted an octave
/// into the band, which is where a shelf-like lift wants to sit.
fn bell_centres(low_hz: f32, high_hz: f32) -> [f32; 3] {
    [low_hz * 0.5, (low_hz * high_hz).sqrt(), high_hz * 2.0]
}

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
    /// One bell per band, for the boost half of the knob's travel.
    bells: [BiquadState; 3],
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
    /// One bell per band, for the BOOST half of the knob's travel. Below
    /// unity the isolator does the work; above it these do, so a lift is
    /// a hump inside the band rather than the whole crossover slice
    /// turned up. `bells_on` says whether any is doing anything at all,
    /// so the ordinary case costs one compare and no filtering.
    bells: [Biquad; 3],
    bells_on: bool,
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
        EqCoeffs::at(sample_rate, EQ_LOW_HZ, EQ_HIGH_HZ)
    }

    fn at(sample_rate: f32, low_hz: f32, high_hz: f32) -> EqCoeffs {
        EqCoeffs {
            split_lp: Biquad::lowpass(high_hz, sample_rate, LR4_Q),
            split_hp: Biquad::highpass(high_hz, sample_rate, LR4_Q),
            band_lp: Biquad::lowpass(low_hz, sample_rate, LR4_Q),
            band_hp: Biquad::highpass(low_hz, sample_rate, LR4_Q),
            band_ap: Biquad::allpass(low_hz, sample_rate, LR4_Q),
            bells: [Biquad::default(); 3],
            bells_on: false,
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
    /// Where the three bands are split: what has been ASKED for.
    low_hz: f32,
    high_hz: f32,
    /// And what the coefficients were actually built for, which walks
    /// toward the ask a little each block rather than arriving at once.
    low_built: f32,
    high_built: f32,
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
            low_hz: EQ_LOW_HZ,
            high_hz: EQ_HIGH_HZ,
            low_built: EQ_LOW_HZ,
            high_built: EQ_HIGH_HZ,
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

    /// Move a crossover. The two are held at least
    /// [`EQ_CROSSOVER_MIN_OCTAVES`] apart, so pushing one into the other
    /// pushes the other along rather than leaving the mid band with
    /// nothing in it.
    pub fn set_crossovers(&mut self, low_hz: f32, high_hz: f32) {
        let Some((low, high)) = eq_crossovers_for(low_hz, high_hz) else {
            return;
        };
        self.low_hz = low;
        self.high_hz = high;
    }

    pub fn crossovers(&self) -> (f32, f32) {
        (self.low_hz, self.high_hz)
    }

    /// Rebuild the boost bells from the gains in force.
    ///
    /// THE LAW. Below unity a band knob is an isolator: it scales its own
    /// crossover slice, and zero is a true kill because the slice simply
    /// stops being summed. That is what a DJ EQ is for and it is not
    /// changing.
    ///
    /// Above unity, scaling the slice is the wrong instrument. A
    /// crossover band is a brick with corners; turning the whole thing up
    /// lifts everything in it equally and stacks phase at both seams,
    /// which reads as honk rather than as more. So the boost half comes
    /// off the isolator entirely -- the slice stays at unity -- and a
    /// gentle bell in the middle of the band does the lifting instead.
    ///
    /// Nothing is crossfaded between the two because nothing needs to
    /// be: at exactly unity the isolator is a wire and the bell is a
    /// wire, so the two halves already meet.
    fn build_bells(&mut self) {
        let centres = bell_centres(self.low_built, self.high_built);
        let mut any = false;
        for band in 0..3 {
            // The autopilot's blend multiplies the operator's knob, the
            // same way it does for the cut half.
            let boost = (self.gain[band].target() * self.blend[band].target()).max(1.0);
            if boost > 1.0 + EQ_KILL_EPSILON {
                any = true;
            }
            self.coeffs.bells[band] =
                Biquad::peaking(centres[band], self.sample_rate, EQ_BELL_Q, boost);
        }
        self.coeffs.bells_on = any;
    }

    /// Walk the built corners toward the asked-for ones, in octaves --
    /// a corner moving from 100 Hz to 200 Hz and one moving from 1 kHz
    /// to 2 kHz are the same journey to the ear, and should take the
    /// same time. Called once a block; rebuilds only when something
    /// actually moved.
    fn glide_crossovers(&mut self) {
        let step = |built: f32, want: f32| -> f32 {
            let octaves = (want / built).log2();
            match octaves.abs() <= EQ_CROSSOVER_SNAP_OCTAVES {
                true => want,
                false => built * 2f32.powf(octaves * EQ_CROSSOVER_GLIDE),
            }
        };
        let low = step(self.low_built, self.low_hz);
        let high = step(self.high_built, self.high_hz);
        if (low - self.low_built).abs() < 1e-4 && (high - self.high_built).abs() < 1e-4 {
            return;
        }
        self.low_built = low;
        self.high_built = high;
        self.rebuild_crossovers();
    }

    /// Rebuild the band splits, keeping whatever the sweep was doing.
    fn rebuild_crossovers(&mut self) {
        let sweep = self.coeffs.sweep;
        let sweep_on = self.coeffs.sweep_on;
        let sweep_prev = self.coeffs.sweep_prev;
        let sweep_prev_on = self.coeffs.sweep_prev_on;
        let sweep_side = self.coeffs.sweep_side;
        self.coeffs = EqCoeffs::at(self.sample_rate, self.low_built, self.high_built);
        self.coeffs.sweep = sweep;
        self.coeffs.sweep_on = sweep_on;
        self.coeffs.sweep_prev = sweep_prev;
        self.coeffs.sweep_prev_on = sweep_prev_on;
        self.coeffs.sweep_side = sweep_side;
        // The memory is KEPT. At a step this small it is very nearly
        // right for the new corner and settles within a few samples;
        // clearing it would be the larger transient of the two, measured
        // at three times the step keeping it costs.
    }

    /// Rebuild the fixed crossover coefficients for a new device rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if (self.sample_rate - sample_rate).abs() < 0.5 {
            return;
        }
        self.sample_rate = sample_rate;
        let sweep = self.coeffs.sweep;
        let sweep_on = self.coeffs.sweep_on;
        self.coeffs = EqCoeffs::at(sample_rate, self.low_built, self.high_built);
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
        self.glide_crossovers();
        self.build_bells();
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
        let corner = filter_corner_hz(position);
        let side: i8 = match corner {
            None => 0,
            Some((false, _)) => -1,
            Some((true, _)) => 1,
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
        // `side` is nonzero here, so the corner is there; the fallback is
        // for the type, not for a path.
        let (highpass, cutoff) = corner.unwrap_or((false, FILTER_LP_MAX_HZ));
        let lowpass = !highpass;
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

            // The isolator half: at or below unity the band is scaled,
            // above it the slice is left alone and the bell does the
            // lifting.
            let banded = low * gains[0].min(1.0)
                + mid * gains[1].min(1.0)
                + high * gains[2].min(1.0);
            let banded = match self.coeffs.bells_on {
                false => banded,
                true => {
                    let mut lifted = banded;
                    for band in 0..3 {
                        lifted = self.coeffs.bells[band]
                            .process(&mut state.bells[band], lifted);
                    }
                    lifted
                }
            };
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

/// A soft clip: the (3,2) Pade form of tanh, exact enough for a feedback
/// path and cheaper than the real thing. Reaches exactly 1.0 at x = 3 and
/// would climb past it beyond that, so the input is clamped there first.
/// NaN clamps to nothing, so it is turned into silence explicitly; an
/// infinity clamps to 3 like any other large input and saturates the
/// same way.
#[inline]
fn pade_tanh(x: f32) -> f32 {
    if x.is_nan() {
        return 0.0;
    }
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// The fractions the echo chip cycles through: whole, half, quarter beat.
pub const ECHO_RUNGS: [(u32, u32); 3] = [(1, 1), (1, 2), (1, 4)];

/// The rows the echo's own dropdown serves: off, then each rung above,
/// by the index `DeckState::echo_rung` keeps. Off is zero, matching the
/// LFO ladder beside it on the same page.
pub const ECHO_RUNG_ROWS: [(u32, &str); 4] =
    [(0, "off"), (1, "1"), (2, "1/2"), (3, "1/4")];

/// Frames the delay line holds — a power of two so the write index is a
/// mask. At 48 kHz this is 5.46 seconds, a whole beat down to about
/// 11 BPM; at 192 kHz, 1.37 seconds, down to about 44 BPM. Below that a
/// requested delay is clamped short rather than refused.
const ECHO_MAX_FRAMES: usize = 1 << 18;
const ECHO_MASK: usize = ECHO_MAX_FRAMES - 1;

/// How long a retune takes to hand over, in frames — fixed like the
/// filter's handover, so the answer does not change with the device's
/// block size.
const ECHO_HANDOVER_FRAMES: u32 = 256;

pub(crate) const ECHO_FEEDBACK_MAX: f32 = 0.95;
/// Below a 16-bit step: past here the tail is inaudible.
const ECHO_QUIET: f32 = 1e-5;
const ECHO_SEND: f32 = 0.5;
pub(crate) const ECHO_FEEDBACK: f32 = 0.55;

/// One deck's beat-quantised echo: a stereo delay line whose tap follows
/// the record's own tempo.
///
/// The line is never bulk-cleared on the audio thread. Instead every
/// frame actually written is counted (`written`), and a tap older than
/// `silence_mark` reads as silence regardless of what is still sitting in
/// memory from before -- the same effect as zeroing the span, at the cost
/// of one comparison instead of a memset. `reset`, called only from the
/// caller thread, does zero the line, as a cheap belt beside that braces.
pub struct DeckEcho {
    line: Box<[[f32; 2]]>,
    write: usize,
    written: u64,
    silence_mark: u64,
    /// The tap in use, in frames.
    delay: u32,
    delay_prev: u32,
    /// A retune that arrived while a handover was already running; taken
    /// up the instant that one finishes.
    pending: Option<u32>,
    handover: u32,
    /// The rung the operator asked for, or none: off.
    fraction: Option<(u32, u32)>,
    send: ParamRamp,
    feedback: ParamRamp,
    /// 0 = repeats land on their own channel, 1 = the other one.
    pingpong: ParamRamp,
    tail_peak: f32,
    period_left: u32,
    /// Nothing to do here: the send is at zero and the tail has decayed
    /// away. `process` uses this to skip the line entirely.
    quiet: bool,
}

impl DeckEcho {
    pub fn new() -> DeckEcho {
        DeckEcho {
            line: vec![[0.0f32; 2]; ECHO_MAX_FRAMES].into_boxed_slice(),
            write: 0,
            written: 0,
            silence_mark: 0,
            delay: 1,
            delay_prev: 1,
            pending: None,
            handover: 0,
            fraction: None,
            send: ParamRamp::at(0.0),
            feedback: ParamRamp::at(ECHO_FEEDBACK),
            pingpong: ParamRamp::at(0.0),
            tail_peak: 0.0,
            period_left: 0,
            quiet: true,
        }
    }

    /// Forget whatever the line was carrying, and any handover or parked
    /// retune in progress. The operator's own settings -- the rung,
    /// the feedback, ping-pong -- are the STRIP's, not the record's
    /// (the doc on `resonance` says the same: a load leaves them), so
    /// they are left exactly as they stand and simply carry on, echoing
    /// whatever plays next.
    ///
    /// Safe on ANY thread, the audio callback's own included: nothing
    /// here writes to `line`. A tap older than `silence_mark` already
    /// reads as silence (`tap_at`), so moving the mark past everything
    /// written so far does the memset's job without touching the 2 MiB
    /// the line actually occupies -- which a bulk clear from inside the
    /// callback would have been a real-time violation to do.
    pub fn silence(&mut self) {
        self.pending = None;
        self.handover = 0;
        self.silence_mark = self.written;
        // Still engaged, still not quiet: content the record about to
        // play writes is real content, and the drain bookkeeping is what
        // gets to notice it decayed away, not this call pretending it
        // already has. Off, there is nothing left to protect.
        self.quiet = self.fraction.is_none();
    }

    /// The rung to echo at, or none for off. Off rings the tail out over
    /// the same crossfade the filter uses rather than cutting it.
    pub fn set_fraction(&mut self, fraction: Option<(u32, u32)>) {
        self.fraction = fraction;
        match fraction {
            Some(_) => {
                self.send.slew(ECHO_SEND, EQ_ENGAGE_SECS);
                // A fresh engagement is never quiet, even if the last one
                // ended that way: `quiet` otherwise stays stale-true from
                // construction (or from the last time the tail actually
                // rang out) and, the moment THIS engagement is switched
                // off and its send ramps back down to zero, the bypass
                // would trigger on that stale flag before this tail has
                // rung out at all.
                self.quiet = false;
            }
            None => self.send.slew(0.0, EQ_ENGAGE_SECS),
        }
    }

    pub fn set_feedback(&mut self, feedback: f32) {
        if let Some(feedback) = knob(feedback, 0.0, ECHO_FEEDBACK_MAX) {
            self.feedback.slew(feedback, EQ_ENGAGE_SECS);
        }
    }

    /// A routing switch written as a blend, like the filter's handover:
    /// mid-crossfade both channels carry a share of each repeat rather
    /// than the line's content jumping side at a sample boundary.
    pub fn set_pingpong(&mut self, on: bool) {
        self.pingpong.slew(if on { 1.0 } else { 0.0 }, EQ_ENGAGE_SECS);
    }

    /// Whether the operator has asked for the echo, regardless of whether
    /// its tail has actually rung out yet.
    pub fn engaged(&self) -> bool {
        self.fraction.is_some()
    }

    /// The rung the operator has asked for, or none for off.
    pub fn fraction(&self) -> Option<(u32, u32)> {
        self.fraction
    }

    #[cfg(test)]
    fn delay(&self) -> u32 {
        self.delay
    }

    #[cfg(test)]
    fn pending(&self) -> Option<u32> {
        self.pending
    }

    #[cfg(test)]
    fn handover(&self) -> u32 {
        self.handover
    }

    /// The write cursor and the raw content sitting `frames_ago` behind
    /// it -- unlike `tap_at`, this does not consult `silence_mark`, so a
    /// test can tell a genuine memset apart from a mark that only makes
    /// old content unreadable.
    #[cfg(test)]
    fn raw_write(&self) -> usize {
        self.write
    }

    #[cfg(test)]
    fn raw_at(&self, frames_ago: usize) -> [f32; 2] {
        self.line[(self.write + ECHO_MAX_FRAMES - frames_ago.min(ECHO_MAX_FRAMES)) & ECHO_MASK]
    }

    /// Retune the tap to `beat_frames` (device frames per beat, at the
    /// tempo the room hears) times the rung in force. Called once per
    /// device buffer, never per frame. A retune that lands while a
    /// handover is already running waits for it rather than restarting
    /// it at whatever weight it had reached.
    pub fn prepare_block(&mut self, beat_frames: f64) {
        let Some((num, den)) = self.fraction else { return };
        if !beat_frames.is_finite() || beat_frames <= 0.0 || den == 0 {
            return;
        }
        let target = (beat_frames * num as f64 / den as f64).round();
        if !target.is_finite() {
            return;
        }
        let target = target.clamp(1.0, (ECHO_MAX_FRAMES - 1) as f64) as u32;
        if target == self.delay || self.pending == Some(target) {
            return;
        }
        if self.handover == 0 {
            self.delay_prev = self.delay;
            self.delay = target;
            self.handover = ECHO_HANDOVER_FRAMES;
            self.period_left = self.delay;
        } else {
            self.pending = Some(target);
        }
    }

    #[inline]
    fn tap_at(&self, delay: u32) -> [f32; 2] {
        let delay = delay.max(1) as u64;
        if self.written < delay || self.written - delay < self.silence_mark {
            return [0.0, 0.0];
        }
        let idx = (self.write + ECHO_MAX_FRAMES - (delay as usize).min(ECHO_MAX_FRAMES))
            & ECHO_MASK;
        self.line[idx]
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let send = self.send.tick(device_rate);
        let feedback = self.feedback.tick(device_rate);
        let pingpong = self.pingpong.tick(device_rate);
        if self.quiet && send == 0.0 && self.send.target() == 0.0 {
            // Nothing engaged and nothing ringing: the line is not
            // touched at all, and the frame is the input exactly.
            return frame;
        }
        let mut tap = self.tap_at(self.delay);
        if self.handover > 0 {
            let tap_prev = self.tap_at(self.delay_prev);
            let blend = 1.0 - self.handover as f32 / ECHO_HANDOVER_FRAMES as f32;
            tap = crate::dsp_math::lerp_frame(tap_prev, tap, blend);
            self.handover -= 1;
            if self.handover == 0 {
                if let Some(pending) = self.pending.take() {
                    self.delay_prev = self.delay;
                    self.delay = pending;
                    self.handover = ECHO_HANDOVER_FRAMES;
                    self.period_left = self.delay;
                }
            }
        }
        let out = [frame[0] + tap[0], frame[1] + tap[1]];
        let fb = [pade_tanh(tap[0] * feedback), pade_tanh(tap[1] * feedback)];
        let written = [
            frame[0] * send + crate::dsp_math::lerp(fb[0], fb[1], pingpong),
            frame[1] * send + crate::dsp_math::lerp(fb[1], fb[0], pingpong),
        ];
        self.line[self.write] = written;
        self.write = (self.write + 1) & ECHO_MASK;
        self.written += 1;
        // Drain bookkeeping, once a beat: settle the line to silence
        // (by moving the mark, never by clearing memory here) once the
        // send has been at zero and the tail has actually decayed away.
        //
        // Watches what is WRITTEN, not what is read back: a tap lags its
        // write by a whole delay, so a window that judged by the tap
        // would still be reporting last beat's silence while THIS
        // beat's write carries a fresh feedback tail that has not been
        // read back yet -- and would be declared quiet just before the
        // read that needed it.
        self.tail_peak = self.tail_peak.max(written[0].abs()).max(written[1].abs());
        if self.period_left == 0 {
            self.period_left = self.delay.max(1);
        }
        self.period_left -= 1;
        if self.period_left == 0 {
            if send == 0.0 && self.send.target() == 0.0 && self.tail_peak < ECHO_QUIET {
                self.quiet = true;
                self.silence_mark = self.written;
            }
            self.tail_peak = 0.0;
        }
        out
    }
}

/// A beat's worth of device audio the ring can hold. A generous few
/// seconds rather than sized to any one tempo's worst case: `hold`
/// clamps whatever it is asked for into this, and the lap is "up to a
/// beat" rather than guaranteed to be a whole one at the far end of the
/// tempo range.
const FREEZE_CAP_FRAMES: usize = 240_000;
/// The seam crossfade at the head of every lap after the first, and the
/// floor a lap's length is never asked to go under -- the loop wrap's
/// own width (`LOOP_XFADE_SECS`, mixer.rs), given its own name here
/// because a freeze lap is not a loop span and has no track to measure
/// a fraction of.
const FREEZE_XFADE_SECS: f32 = 0.010;
/// How long the press and the release each take to cross, the deck's
/// own gesture ramp (`SLEW_SECS`, mixer.rs).
const FREEZE_BLEND_SECS: f32 = 0.008;

/// A momentary FREEZE: while held, repeats a beat-sized capture of what
/// the deck just played, post-filter, while the record underneath keeps
/// running at its own pace. Release blends back to the live signal
/// where it has got to.
///
/// The ring is never bulk-cleared, on the audio thread or off it --
/// `reset` is a field reset only, exactly the shape `DeckEcho::silence`
/// settled on and for the same reason. Nothing here needs a write-count
/// invalidation scheme the way the echo's arbitrary-length delay did:
/// a lap can never reach back further than `2 * xf` frames beyond what
/// has genuinely been written since writing last resumed (`fresh`,
/// below), so the very worst a fresh load or a rapid re-press can leak
/// is a seam-blended sliver on the order of the crossfade itself, not a
/// standalone repeat of unrelated audio.
pub struct Freeze {
    ring: Box<[[f32; 2]]>,
    write: usize,
    /// Frames written since writing last resumed (after construction,
    /// or after a release finished and a reset). Saturates at the
    /// ring's capacity. `hold` clamps the requested length to this, so
    /// a press can never reach back across a gap the ring never
    /// actually recorded -- the ordinary "press, release, press again"
    /// stutter would otherwise splice pre-hold audio straight onto
    /// post-release audio with no crossfade to hide the seam.
    fresh: usize,
    held: Option<Held>,
    wet: ParamRamp,
}

struct Held {
    start: usize,
    len: usize,
    xf: usize,
    pos: usize,
}

impl Freeze {
    pub fn new() -> Freeze {
        Freeze {
            ring: vec![[0.0f32; 2]; FREEZE_CAP_FRAMES].into_boxed_slice(),
            write: 0,
            fresh: 0,
            held: None,
            wet: ParamRamp::at(0.0),
        }
    }

    /// Forget where writing had got to and let go of any hold in
    /// progress -- a fresh record, an emptied deck, or a deck another
    /// record was just cloned onto. Never touches the ring itself, so
    /// it is safe from any thread, the audio callback's own included.
    pub fn reset(&mut self) {
        self.held = None;
        self.wet = ParamRamp::at(0.0);
        self.fresh = 0;
    }

    /// Latch a lap `requested_len` frames long, ending at the write
    /// cursor, and start crossfading it in. A press that arrives while
    /// one is already sounding (wet at or heading to 1) changes
    /// nothing, like a key already held; a press during the release
    /// tail (wet heading to 0) re-latches onto the very same lap and
    /// heads back up, rather than being dropped along with it.
    pub fn hold(&mut self, requested_len: usize, device_rate: f32) {
        let xf = ((FREEZE_XFADE_SECS * device_rate).round() as usize).max(1);
        if self.held.is_some() {
            if self.wet.target() <= 0.0 {
                self.wet.slew(1.0, FREEZE_BLEND_SECS);
            }
            return;
        }
        let cap = FREEZE_CAP_FRAMES;
        // The seam blend needs `xf` frames of genuine content BEFORE the
        // lap's own start to pre-roll from (see `process`), so what is
        // actually available to draw a lap from is `fresh` less that.
        let available = self.fresh.saturating_sub(xf);
        // Under `2 * xf` and there is not enough freshly-written content
        // to fill even the shortest lap the seam blend can work with --
        // a hold this soon after a reset (a fresh load, a clone, or a
        // release that only just let go) would have to reach past what
        // `fresh` actually certifies and read whatever the ring was
        // last carrying, load or lifetime before this one. The floor
        // used to be applied AFTER this clamp, which defeated it outright
        // in exactly this case; refusing outright is what the doc above
        // on `fresh` has always promised, so nothing here forces a lap
        // across that boundary instead.
        if available < 2 * xf {
            return;
        }
        let ceiling = cap.saturating_sub(2 * xf).max(2 * xf);
        let len = requested_len.clamp(2 * xf, available.min(ceiling));
        let start = (self.write + cap - len) % cap;
        self.held = Some(Held { start, len, xf, pos: 0 });
        self.wet.slew(1.0, FREEZE_BLEND_SECS);
    }

    /// Let go: the tail keeps sounding, on the same crossfade, until the
    /// blend has settled back on the live signal.
    pub fn release(&mut self) {
        if self.held.is_some() {
            self.wet.slew(0.0, FREEZE_BLEND_SECS);
        }
    }

    /// Whether a lap is sounding or crossfading toward one -- true from
    /// `hold` until the release blend has fully settled.
    pub fn held(&self) -> bool {
        self.held.is_some()
    }

    /// The lap's length in frames, for the tests that check what a press
    /// asked for actually became.
    #[cfg(test)]
    pub fn lap_frames(&self) -> Option<usize> {
        self.held.as_ref().map(|held| held.len)
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, live: [f32; 2], device_rate: f32) -> [f32; 2] {
        // Not held: `wet` is always 0 here (it is cleared to 0 in the
        // very call that clears `held`, below, and `release` moves it
        // only while `held` is Some), so the untouched path is the
        // sample just read, bit-for-bit, exactly `DeckEcho`'s own rule.
        let Some(h) = self.held.as_mut() else {
            let cap = self.ring.len();
            self.ring[self.write] = live;
            self.write = (self.write + 1) % cap;
            self.fresh = (self.fresh + 1).min(cap);
            return live;
        };
        let cap = self.ring.len();
        let frozen = h.pos;
        let mut sample = self.ring[(h.start + frozen) % cap];
        // The last `xf` frames of every lap blend the tail into the
        // lap's own HEAD, exactly the loop wrap's crossfade (mixer.rs),
        // so the seam lands in phase with itself rather than splicing.
        if frozen + h.xf >= h.len {
            let u = frozen + h.xf - h.len;
            // A PRE-ROLL, not the lap's own head: read from just BEFORE
            // `start`, walking up to (but not quite reaching) it, so
            // the last blended frame sits one sample short of exactly
            // where the wrap's own direct read picks up -- the same
            // one-sample residual the loop wrap's own crossfade leaves
            // (mixer.rs), not the several-hundred-sample phase jump a
            // head read INSIDE the lap would have produced.
            let head = self.ring[(h.start + cap - h.xf + u) % cap];
            let t = u as f32 / h.xf as f32;
            sample = crate::dsp_math::lerp_frame(sample, head, t);
        }
        h.pos = (h.pos + 1) % h.len;
        let wet = self.wet.tick(device_rate);
        if wet <= 0.0 && self.wet.target() <= 0.0 {
            self.held = None;
            self.fresh = 0;
        }
        crate::dsp_math::lerp_frame(live, sample, wet)
    }

    #[cfg(test)]
    fn write_index(&self) -> usize {
        self.write
    }
}

/// Frames the flanger's short delay line holds -- generous for the LFO's
/// full sweep at up to 192 kHz, with margin for a fed-back tail to decay
/// below [`FLANGER_QUIET`] before the drain check gives up watching it.
const FLANGER_MAX_FRAMES: usize = 2048;
const FLANGER_MASK: usize = FLANGER_MAX_FRAMES - 1;
/// Centre and swing of the modulated delay, in milliseconds -- the classic
/// flange range (roughly half a millisecond to five and a half), deep
/// enough for the swept-comb whoosh without wandering into slapback echo.
const FLANGER_CENTER_MS: f32 = 3.0;
const FLANGER_DEPTH_MS: f32 = 2.5;
pub(crate) const FLANGER_RATE_MIN: f32 = 0.02;
pub(crate) const FLANGER_RATE_MAX: f32 = 8.0;
pub(crate) const FLANGER_RATE_DEFAULT: f32 = 0.25;
pub(crate) const FLANGER_DEPTH_DEFAULT: f32 = 0.7;
pub(crate) const FLANGER_FEEDBACK_MAX: f32 = 0.9;
pub(crate) const FLANGER_FEEDBACK_DEFAULT: f32 = 0.25;
/// Below a 16-bit step: past here a fed-back tail is inaudible.
const FLANGER_QUIET: f32 = 1e-5;

/// One deck's flanger: a short stereo delay line whose read tap is swept
/// by a sine LFO, fed back through [`pade_tanh`] so cranked regeneration
/// saturates instead of runs away.
///
/// No discrete retune/handover machinery like the echo's rung: the delay
/// here is CONTINUOUSLY modulated, so every frame's tap position is
/// already a smooth function of the LFO phase, and the fractional-sample
/// interpolated read (`crate::dsp_math::lerp_frame`) is what keeps that
/// click-free -- there is never a jump to hand over.
pub struct Flanger {
    line: Box<[[f32; 2]]>,
    write: usize,
    written: u64,
    silence_mark: u64,
    phase: f32,
    wet: ParamRamp,
    rate: ParamRamp,
    depth: ParamRamp,
    feedback: ParamRamp,
    tail_peak: f32,
    period_left: u32,
    /// Nothing to do here: wet is at zero and the fed-back tail has
    /// decayed away. `process` uses this to skip the line entirely.
    quiet: bool,
    /// Which rung of [`LFO_SYNC_ROWS`] the sweep runs on:
    /// [`LFO_SYNC_FREE`] to follow `rate`'s Hz, or eighths of a cycle
    /// per beat to follow the deck's grid instead, through `active_hz`
    /// -- recomputed once a buffer in `prepare_block`. A division is
    /// deliberately NOT stored in `rate`: that setter clamps to this
    /// effect's own Hz range, which a division has no reason to fit
    /// inside.
    sync_units: u32,
    /// Phase offset within the cycle, 0..1, applied once at the moment
    /// of engage (see `set_wet`), never live.
    beat_offset: f32,
    /// This buffer's locked rate in Hz; unused (stale) on the
    /// free-running rung.
    active_hz: f32,
}

impl Flanger {
    pub fn new() -> Flanger {
        Flanger {
            line: vec![[0.0f32; 2]; FLANGER_MAX_FRAMES].into_boxed_slice(),
            write: 0,
            written: 0,
            silence_mark: 0,
            phase: 0.0,
            wet: ParamRamp::at(0.0),
            rate: ParamRamp::at(FLANGER_RATE_DEFAULT),
            depth: ParamRamp::at(FLANGER_DEPTH_DEFAULT),
            feedback: ParamRamp::at(FLANGER_FEEDBACK_DEFAULT),
            tail_peak: 0.0,
            period_left: 0,
            quiet: true,
            sync_units: LFO_SYNC_FREE,
            beat_offset: 0.0,
            active_hz: FLANGER_RATE_DEFAULT,
        }
    }

    /// Forget whatever the line was carrying. Safe on any thread, the
    /// audio callback's own included: nothing here writes to `line` --
    /// moving the mark past everything written so far does the memset's
    /// job without touching it, the same reason [`DeckEcho::silence`]
    /// works this way. The operator's own rate/depth/feedback are the
    /// STRIP's, not the record's, so they are left exactly as they stand.
    pub fn silence(&mut self) {
        self.silence_mark = self.written;
        self.quiet = self.wet.target() <= 0.0;
    }

    /// The on/off switch, ramped like every other engage here rather than
    /// snapped, so switching it off lets the swept comb ring out instead
    /// of cutting it.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            let engaging = wet > 0.0;
            if engaging && self.wet.target() == 0.0 && self.sync_units > 0 {
                // Start the sweep at its chosen place in the cycle,
                // gated on the engage transition so it can never move
                // the sweep under a sounding deck.
                self.phase = self.beat_offset * std::f32::consts::TAU;
            }
            self.wet.slew(wet, EQ_ENGAGE_SECS);
            if engaging {
                self.quiet = false;
            }
        }
    }

    /// The LFO's sweep speed, in Hz -- what plays on the free-running
    /// rung. A locked rung takes its rate from the grid instead and
    /// leaves this alone.
    pub fn set_rate(&mut self, hz: f32) {
        if let Some(hz) = knob(hz, FLANGER_RATE_MIN, FLANGER_RATE_MAX) {
            self.rate.slew(hz, EQ_ENGAGE_SECS);
        }
    }

    /// Pick the sweep's rung: [`LFO_SYNC_FREE`] for the Hz slider, or
    /// eighths of a cycle per beat to follow the grid.
    pub fn set_sync_units(&mut self, units: u32) {
        self.sync_units = units.min(LFO_SYNC_MAX_UNITS);
    }

    pub fn sync_units(&self) -> u32 {
        self.sync_units
    }

    /// Where in the cycle the sweep starts on engage, 0..1.
    pub fn set_beat_offset(&mut self, offset: f32) {
        if let Some(offset) = knob(offset, 0.0, 1.0) {
            self.beat_offset = offset;
        }
    }

    /// Recompute this buffer's beat-synced rate from the deck's current
    /// tempo; see [`Tremolo::prepare_block`] for the full reasoning.
    pub fn prepare_block(
        &mut self,
        clock: &crate::wave_analysis::DeckClock,
        buffer_secs: f32,
    ) {
        if self.sync_units == 0 {
            return;
        }
        // Ungridded or stopped, the counted beat -- the same default a
        // loop or a jump takes when nothing has measured the record.
        let beat_secs = clock.beat_len().unwrap_or(60.0 / crate::decks::COUNTED_BPM);
        let cycles = sync_cycles_per_beat(self.sync_units);
        let base = (cycles / beat_secs.max(1e-6)) as f32;
        self.active_hz =
            lfo_locked_hz(self.phase, base, cycles, self.beat_offset, buffer_secs, clock);
    }

    /// How far the sweep reaches from the centre delay, 0..1 of the full
    /// swing.
    pub fn set_depth(&mut self, depth: f32) {
        if let Some(depth) = knob(depth, 0.0, 1.0) {
            self.depth.slew(depth, EQ_ENGAGE_SECS);
        }
    }

    /// How much of the delayed tap feeds back into the line.
    pub fn set_feedback(&mut self, feedback: f32) {
        if let Some(feedback) = knob(feedback, 0.0, FLANGER_FEEDBACK_MAX) {
            self.feedback.slew(feedback, EQ_ENGAGE_SECS);
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    #[cfg(test)]
    fn raw_write(&self) -> usize {
        self.write
    }

    #[cfg(test)]
    fn raw_at(&self, frames_ago: usize) -> [f32; 2] {
        self.line[(self.write + FLANGER_MAX_FRAMES - frames_ago.min(FLANGER_MAX_FRAMES))
            & FLANGER_MASK]
    }

    /// The delayed tap at a fractional frame count, linearly interpolated
    /// between the two neighboring integer taps -- unlike the echo's
    /// integer-frame `tap_at`, this one moves every sample, so a
    /// non-interpolated read would zipper audibly as the LFO sweeps.
    #[inline]
    fn tap_at(&self, delay_frac: f32) -> [f32; 2] {
        let d0 = delay_frac.floor().max(0.0) as u64;
        let d1 = d0 + 1;
        if self.written < d1 || self.written - d1 < self.silence_mark {
            return [0.0, 0.0];
        }
        let frac = delay_frac - d0 as f32;
        let idx0 =
            (self.write + FLANGER_MAX_FRAMES - (d0 as usize).min(FLANGER_MAX_FRAMES)) & FLANGER_MASK;
        let idx1 =
            (self.write + FLANGER_MAX_FRAMES - (d1 as usize).min(FLANGER_MAX_FRAMES)) & FLANGER_MASK;
        crate::dsp_math::lerp_frame(self.line[idx0], self.line[idx1], frac)
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if self.quiet && wet == 0.0 && self.wet.target() == 0.0 {
            // Nothing engaged and nothing ringing: the line is not
            // touched at all, and the frame is the input exactly.
            return frame;
        }
        let rate_hz = if self.sync_units > 0 {
            self.active_hz
        } else {
            self.rate.tick(device_rate)
        };
        let depth = self.depth.tick(device_rate);
        let feedback = self.feedback.tick(device_rate);

        self.phase += std::f32::consts::TAU * rate_hz / device_rate.max(1.0);
        if self.phase >= std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }

        let center = FLANGER_CENTER_MS * 0.001 * device_rate;
        let swing = FLANGER_DEPTH_MS * 0.001 * device_rate * depth;
        let delay = (center + swing * self.phase.sin()).max(1.0);

        let tap = self.tap_at(delay);
        let out = [frame[0] + tap[0] * wet, frame[1] + tap[1] * wet];
        // The dry share entering the line is scaled by `wet` too, not
        // just the output blend -- otherwise the line fills with
        // full-amplitude content from the first frame of an engage,
        // and once that content ages past the (short) delay it surfaces
        // all at once at whatever `wet` has ramped to by then, rather
        // than a content level that grew in step with the ramp. Echo's
        // `send` gates its own write the same way and for the same
        // reason; a beat-length delay simply outlasts the ramp so the
        // effect is invisible there, but a flange delay is short enough
        // to land the mismatch inside the very ramp meant to hide it.
        let written = [
            frame[0] * wet + pade_tanh(tap[0] * feedback),
            frame[1] * wet + pade_tanh(tap[1] * feedback),
        ];
        self.line[self.write] = written;
        self.write = (self.write + 1) & FLANGER_MASK;
        self.written += 1;

        // Drain bookkeeping, once a sweep period, mirroring the echo's:
        // watches what is WRITTEN, not what is read back, for the same
        // reason -- a tap lags its write by a whole delay.
        self.tail_peak = self.tail_peak.max(written[0].abs()).max(written[1].abs());
        if self.period_left == 0 {
            self.period_left = delay.round().max(1.0) as u32;
        }
        self.period_left -= 1;
        if self.period_left == 0 {
            if wet == 0.0 && self.wet.target() == 0.0 && self.tail_peak < FLANGER_QUIET {
                self.quiet = true;
                self.silence_mark = self.written;
            }
            self.tail_peak = 0.0;
        }
        out
    }
}

pub(crate) const BITCRUSHER_RATE_MIN: f32 = 500.0;
pub(crate) const BITCRUSHER_RATE_MAX: f32 = 24_000.0;
pub(crate) const BITCRUSHER_RATE_DEFAULT: f32 = 4_000.0;
pub(crate) const BITCRUSHER_BITS_MIN: f32 = 1.0;
pub(crate) const BITCRUSHER_BITS_MAX: f32 = 16.0;
pub(crate) const BITCRUSHER_BITS_DEFAULT: f32 = 8.0;
/// How long a bit-depth change takes to hand over, matching the filter's
/// and the echo's own handover windows.
const BITCRUSHER_HANDOVER_FRAMES: u32 = 256;

/// One deck's bitcrusher: a sample-and-hold decimator feeding a
/// bit-depth quantizer.
///
/// Two knobs, two different click hazards, two different fixes. `rate`
/// only moves WHEN the next sample is captured -- a hard step there just
/// shifts the phase of a staircase the effect already has, so a plain
/// ramp is enough. `bits` moves WHAT a captured sample rounds to, and
/// `round(x / step)` is not a continuous function of `step`: a smoothly
/// ramped `step` can still make the rounded OUTPUT jump by a whole step
/// the instant `x / step` crosses a half-integer boundary. Ramping the
/// scalar into a rounding function does not bound what comes out of it,
/// so `bits` gets the same two-stream crossfade `DeckEq`'s filter
/// handover and `DeckEcho`'s delay retune already use: both the old and
/// the new quantization of the SAME held sample are computed every
/// frame during a handover, and only the blend between those two
/// already-rounded streams is smoothed.
pub struct Bitcrusher {
    /// The last raw sample the hold captured. Two floats, not a line --
    /// there is nothing here a bulk clear could be a real-time violation
    /// to touch.
    held: [f32; 2],
    /// Fractional hold-period phase, 0..1, advanced like [`Flanger`]'s
    /// own LFO phase but linearly rather than around a circle.
    hold_phase: f32,
    wet: ParamRamp,
    rate: ParamRamp,
    bits: ParamRamp,
    /// The bit depth the OUTGOING quantization stream still uses while a
    /// handover crossfades to a freshly changed `bits`; equal to
    /// `bits.target()` whenever `handover` is zero.
    bits_active: f32,
    /// A bits change that arrived while a handover was already running;
    /// taken up the instant that one finishes, the same reason
    /// [`DeckEcho`]'s own retune queues rather than restarts one.
    /// Without this, a fast slider drag calling `set_bits` on every
    /// tick would reset `handover` to full on each call while
    /// `bits_active` stayed at whatever it was before the FIRST call --
    /// snapping the crossfade's blend back to zero and discarding
    /// whatever fraction of the interrupted transition had already
    /// played, a click the handover exists specifically to prevent.
    pending: Option<f32>,
    handover: u32,
}

impl Bitcrusher {
    pub fn new() -> Bitcrusher {
        Bitcrusher {
            held: [0.0, 0.0],
            hold_phase: 0.0,
            wet: ParamRamp::at(0.0),
            rate: ParamRamp::at(BITCRUSHER_RATE_DEFAULT),
            bits: ParamRamp::at(BITCRUSHER_BITS_DEFAULT),
            bits_active: BITCRUSHER_BITS_DEFAULT,
            pending: None,
            handover: 0,
        }
    }

    /// Forces the very next frame to recapture rather than keep
    /// reflecting whatever the hold last caught -- there is no buffer
    /// here for a record change to leave stale, only these two floats,
    /// so unlike [`DeckEcho::silence`] this is a plain, unconditional
    /// reset rather than a bookkeeping-only move.
    pub fn silence(&mut self) {
        self.held = [0.0, 0.0];
        self.hold_phase = 1.0;
        // Whatever crossfade or queued retune was in flight belonged to
        // the record that just left; a fresh one starts clean rather
        // than finishing a blend toward a target picked for different
        // audio, the same reason `DeckEcho::silence` clears its own
        // `pending`/`handover`.
        self.pending = None;
        self.handover = 0;
    }

    /// The on/off switch.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            self.wet.slew(wet, EQ_ENGAGE_SECS);
        }
    }

    /// How often the hold captures a fresh sample, in Hz. A hard change
    /// here only shifts the phase of the effect's own staircase, so a
    /// plain ramp -- not a handover -- is enough.
    pub fn set_rate(&mut self, hz: f32) {
        if let Some(hz) = knob(hz, BITCRUSHER_RATE_MIN, BITCRUSHER_RATE_MAX) {
            self.rate.slew(hz, EQ_ENGAGE_SECS);
        }
    }

    /// The quantizer's bit depth. Arms a handover rather than ramping
    /// the scalar directly into `quantize`; see the type's own doc. A
    /// retune that arrives mid-handover is parked rather than raced --
    /// see `pending`'s own doc -- so a fast drag through several
    /// values lands cleanly on wherever it ends rather than clicking at
    /// every intermediate tick.
    pub fn set_bits(&mut self, bits: f32) {
        if let Some(bits) = knob(bits, BITCRUSHER_BITS_MIN, BITCRUSHER_BITS_MAX) {
            if bits == self.bits.target() || self.pending == Some(bits) {
                return;
            }
            if self.handover == 0 {
                self.bits.jump(bits);
                self.handover = BITCRUSHER_HANDOVER_FRAMES;
            } else {
                self.pending = Some(bits);
            }
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    #[cfg(test)]
    fn held(&self) -> [f32; 2] {
        self.held
    }

    #[inline]
    fn quantize(x: f32, bits: f32) -> f32 {
        let levels = bits.exp2();
        let step = 2.0 / levels;
        (x / step).round() * step
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if wet <= 0.0 {
            // No feedback, nothing rings: the crossfade below already
            // equals the input exactly once wet is at rest, so there is
            // no tail left to protect the way the echo's or the
            // flanger's bypass has to.
            return frame;
        }
        let rate_hz = self.rate.tick(device_rate);

        self.hold_phase += rate_hz / device_rate.max(1.0);
        if self.hold_phase >= 1.0 {
            self.hold_phase -= 1.0;
            self.held = frame;
        }

        let bits_target = self.bits.target();
        let mut crushed = [
            Self::quantize(self.held[0], bits_target),
            Self::quantize(self.held[1], bits_target),
        ];
        if self.handover > 0 {
            let outgoing = [
                Self::quantize(self.held[0], self.bits_active),
                Self::quantize(self.held[1], self.bits_active),
            ];
            let blend = 1.0 - self.handover as f32 / BITCRUSHER_HANDOVER_FRAMES as f32;
            crushed = crate::dsp_math::lerp_frame(outgoing, crushed, blend);
            self.handover -= 1;
            if self.handover == 0 {
                self.bits_active = bits_target;
                if let Some(pending) = self.pending.take() {
                    self.bits.jump(pending);
                    self.handover = BITCRUSHER_HANDOVER_FRAMES;
                }
            }
        }

        [
            frame[0] + (crushed[0] - frame[0]) * wet,
            frame[1] + (crushed[1] - frame[1]) * wet,
        ]
    }
}

/// An LFO's locked rate is carried as EIGHTHS of a cycle per beat, so
/// the whole ladder is integers -- the same reason the loop ladder
/// counts 1/32-beat ticks (`decks.rs`) rather than storing fractions.
pub(crate) const LFO_SYNC_EIGHTHS: u32 = 8;

/// The free-running rung: not a division at all, but whatever Hz the
/// rate slider carries. Zero, so `sync_units > 0` reads as "locked".
pub const LFO_SYNC_FREE: u32 = 0;

/// The rows an LFO effect's sync dropdown serves, in order: free-running
/// first, then every division from an eighth of a cycle per beat (one
/// sweep every eight beats -- two bars, the slow flanger/phaser end) up
/// to sixty-four cycles inside a single beat (the fast tremolo/autopan
/// end). Powers of two throughout, like the loop ladder's own rungs.
///
/// This list is the WHOLE set of rates a locked effect can run at: a
/// dropdown can only emit a value on it, so nothing has to snap a raw
/// drag to a rung and nothing can land between two of them.
pub const LFO_SYNC_ROWS: [(u32, &str); 11] = [
    (LFO_SYNC_FREE, "Hz"),
    (1, "1/8"),
    (2, "1/4"),
    (4, "1/2"),
    (8, "1"),
    (16, "2"),
    (32, "4"),
    (64, "8"),
    (128, "16"),
    (256, "32"),
    (512, "64"),
];

/// The top rung, so a setter can clamp to the ladder without walking it.
pub const LFO_SYNC_MAX_UNITS: u32 = 512;

/// How far a locked LFO may run fast or slow to close a phase error, as
/// a share of the rate it is meant to be running at. A quarter, so the
/// worst case -- half a cycle out -- is back in phase within about two
/// cycles, which is quick enough to feel locked and slow enough that the
/// catching-up is not itself the effect.
const LFO_LOCK_TRIM: f32 = 0.25;
/// How long the servo would take to close an error if nothing clamped
/// it. The clamp above is what usually governs; this sets the gentleness
/// of the last, small part of the correction.
const LFO_LOCK_SECS: f32 = 0.5;

/// The rate a locked LFO should run at this buffer: its own, plus
/// whatever trim closes the gap between where its phase is and where the
/// grid says it should be.
///
/// The correction leans on the RATE and never on the phase. Moving the
/// phase directly would be a step in the output -- for a tremolo the
/// gain is a function of phase, so a nudge of a tenth of a radian is a
/// gain step of about 0.04, twice this file's click budget. Leaning on
/// the rate cannot step anything: it only changes where the phase will
/// be next sample, which is what a rate does anyway.
#[inline]
fn lfo_locked_hz(
    phase: f32,
    base_hz: f32,
    cycles_per_beat: f64,
    offset: f32,
    buffer_secs: f32,
    clock: &crate::wave_analysis::DeckClock,
) -> f32 {
    if !clock.has_grid {
        return base_hz;
    }
    let want =
        (clock.beat_at_end * cycles_per_beat + offset as f64).rem_euclid(1.0) as f32;
    // The grid is read at the END of this buffer, so the phase has to be
    // too: comparing where the LFO is NOW against where the beat will be
    // THEN leaves the servo one buffer of phase short for ever, which is
    // a standing error of about two percent of a cycle at an ordinary
    // buffer size. Project it forward at the rate it is about to run.
    let projected = phase / std::f32::consts::TAU + base_hz * buffer_secs;
    let have = projected.rem_euclid(1.0);
    // The short way round: half a cycle late is half a cycle early.
    let mut error = want - have;
    if error > 0.5 {
        error -= 1.0;
    } else if error < -0.5 {
        error += 1.0;
    }
    let trim = (error / LFO_LOCK_SECS).clamp(-base_hz.abs() * LFO_LOCK_TRIM, base_hz.abs() * LFO_LOCK_TRIM);
    base_hz + trim
}

/// A rung's cycles per beat. Zero units is free-running and has none.
#[inline]
fn sync_cycles_per_beat(units: u32) -> f64 {
    units as f64 / LFO_SYNC_EIGHTHS as f64
}

pub(crate) const TREMOLO_RATE_MIN: f32 = 0.1;
pub(crate) const TREMOLO_RATE_MAX: f32 = 20.0;
pub(crate) const TREMOLO_RATE_DEFAULT: f32 = 4.0;
pub(crate) const TREMOLO_DEPTH_DEFAULT: f32 = 0.85;

/// One deck's tremolo: a unipolar sine LFO driving a subtractive gain
/// multiplier -- it can only attenuate, never boost past unity, the same
/// as a real tremolo circuit.
///
/// No delay line, no filter history, nothing that persists audio
/// content across frames -- `phase` is an oscillator position, not
/// sound, so none of the fed-back-line hazards `DeckEcho`/`Flanger`
/// guard against apply here: there is no "later" for stale content to
/// resurface from, and disengage needs no ring-out tail. That also
/// means no lifecycle hook is wired anywhere for this unit -- see the
/// module-level note beside its wiring in `mixer.rs`.
pub struct Tremolo {
    phase: f32,
    wet: ParamRamp,
    rate: ParamRamp,
    depth: ParamRamp,
    /// Which rung of [`LFO_SYNC_ROWS`] the LFO runs on:
    /// [`LFO_SYNC_FREE`] to follow `rate`'s Hz, or eighths of a cycle
    /// per beat to follow the deck's grid instead, through `active_hz`
    /// -- recomputed once a buffer in `prepare_block`. A division is
    /// deliberately NOT stored in `rate`: that setter clamps to this
    /// effect's own Hz range, which a division has no reason to fit
    /// inside.
    sync_units: u32,
    /// Phase offset within the cycle, 0..1, applied once at the moment
    /// of engage (see `set_wet`) -- not live while already engaged, so
    /// dragging it has no click surface to cover.
    beat_offset: f32,
    /// This buffer's locked rate in Hz; unused (stale) on the
    /// free-running rung, where `process` ticks `rate` directly instead.
    active_hz: f32,
}

impl Tremolo {
    pub fn new() -> Tremolo {
        Tremolo {
            phase: 0.0,
            wet: ParamRamp::at(0.0),
            rate: ParamRamp::at(TREMOLO_RATE_DEFAULT),
            depth: ParamRamp::at(TREMOLO_DEPTH_DEFAULT),
            sync_units: LFO_SYNC_FREE,
            beat_offset: 0.0,
            active_hz: TREMOLO_RATE_DEFAULT,
        }
    }

    /// The on/off switch. Resets the LFO to its offset position on the
    /// moment of engage when beat-synced, so the wobble starts at a
    /// known, chosen place in the cycle every time it is turned on --
    /// gated on the wet transition, not applied live, so this can never
    /// click.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            if wet > 0.0 && self.wet.target() == 0.0 && self.sync_units > 0 {
                self.phase = self.beat_offset * std::f32::consts::TAU;
            }
            self.wet.slew(wet, EQ_ENGAGE_SECS);
        }
    }

    /// The LFO's speed, in Hz -- what plays on the free-running rung.
    /// A locked rung takes its rate from the grid instead and leaves
    /// this alone, ready for the moment Hz is picked again. A hard
    /// change
    /// here only shifts the modulation's slope, not the gain's value at
    /// any instant, so a plain ramp is enough -- ramped anyway, for
    /// idiom consistency with every other numeric setter in this file,
    /// not because skipping it would click.
    pub fn set_rate(&mut self, hz: f32) {
        if let Some(hz) = knob(hz, TREMOLO_RATE_MIN, TREMOLO_RATE_MAX) {
            self.rate.slew(hz, EQ_ENGAGE_SECS);
        }
    }

    /// The LFO's swing, 0..1 of the full attenuation range.
    pub fn set_depth(&mut self, depth: f32) {
        if let Some(depth) = knob(depth, 0.0, 1.0) {
            self.depth.slew(depth, EQ_ENGAGE_SECS);
        }
    }

    /// Pick the LFO's rung: [`LFO_SYNC_FREE`] for the Hz slider, or
    /// eighths of a cycle per beat to follow the grid.
    pub fn set_sync_units(&mut self, units: u32) {
        self.sync_units = units.min(LFO_SYNC_MAX_UNITS);
    }

    pub fn sync_units(&self) -> u32 {
        self.sync_units
    }

    /// Where in the cycle the wobble starts on engage, 0..1.
    pub fn set_beat_offset(&mut self, offset: f32) {
        if let Some(offset) = knob(offset, 0.0, 1.0) {
            self.beat_offset = offset;
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    /// Recompute this buffer's beat-synced rate from the deck's current
    /// tempo. Called once per device buffer, mirroring `DeckEcho`'s own
    /// `prepare_block` (`mixer.rs`) -- not a hard grid-lock (no phase
    /// snap toward an absolute target), just a tempo-tracking Hz that
    /// self-corrects whenever the tempo or grid changes, since
    /// `beat_secs` is read fresh every buffer. Ordinary long-run phase
    /// drift is accepted, the same as free-Hz mode already has.
    pub fn prepare_block(
        &mut self,
        clock: &crate::wave_analysis::DeckClock,
        buffer_secs: f32,
    ) {
        if self.sync_units == 0 {
            return;
        }
        // Ungridded or stopped, the counted beat -- the same default a
        // loop or a jump takes when nothing has measured the record.
        let beat_secs = clock.beat_len().unwrap_or(60.0 / crate::decks::COUNTED_BPM);
        let cycles = sync_cycles_per_beat(self.sync_units);
        let base = (cycles / beat_secs.max(1e-6)) as f32;
        self.active_hz =
            lfo_locked_hz(self.phase, base, cycles, self.beat_offset, buffer_secs, clock);
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if wet == 0.0 && self.wet.target() == 0.0 {
            // No line, no tail: off is exactly the input, and stays
            // exactly the input the instant the disengage ramp
            // finishes -- unlike Flanger/Echo there is nothing here
            // that can still be ringing.
            return frame;
        }
        let rate_hz = if self.sync_units > 0 {
            self.active_hz
        } else {
            self.rate.tick(device_rate)
        };
        let depth = self.depth.tick(device_rate);

        self.phase += std::f32::consts::TAU * rate_hz / device_rate.max(1.0);
        if self.phase >= std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }

        let lfo01 = 0.5 * (1.0 + self.phase.sin());
        let gain = 1.0 - wet * depth * (1.0 - lfo01);

        [frame[0] * gain, frame[1] * gain]
    }
}

/// How fast the makeup's two level meters follow the music. Long enough
/// that the correction is a level match and not a compressor riding the
/// waveform, short enough that it has caught up well inside the engage
/// ramp.
const DISTORTION_ENV_SECS: f32 = 0.05;
/// Mean square below which there is nothing worth measuring -- about
/// -70 dB. Under it the last good makeup is held.
const DISTORTION_ENV_GATE: f32 = 1e-7;
/// How far the makeup may reach. The shaper's own gain runs to about
/// `drive`, so the correction has to reach a twentieth; the ceiling is
/// there so a pathological ratio cannot turn the effect into a boost.
const DISTORTION_MAKEUP_MIN: f32 = 0.02;
const DISTORTION_MAKEUP_MAX: f32 = 4.0;

pub(crate) const DISTORTION_DRIVE_MIN: f32 = 1.0;
pub(crate) const DISTORTION_DRIVE_MAX: f32 = 20.0;
pub(crate) const DISTORTION_DRIVE_DEFAULT: f32 = 4.0;

/// One deck's distortion: [`pade_tanh`] soft-clipping driven by a pre-gain
/// ("drive"), with a makeup gain that keeps the effect's OWN average
/// level roughly steady as drive moves, so turning the knob changes the
/// character rather than jumping the volume.
///
/// The makeup here is a static function of `drive` alone --
/// `1 / pade_tanh(drive)` -- not a live envelope follower over the
/// actual signal. A true RMS follower would need its own persisted
/// envelope state and its own lifecycle policy (another `silence`-style
/// question this file has already answered three times over for the
/// echo, the flanger and the bitcrusher); `pade_tanh(drive)` already
/// says, for a unity-amplitude peak, how much THIS drive setting
/// compresses it, which is the number that actually needs compensating.
/// Nothing here holds audio content across frames, so -- like
/// [`Tremolo`] -- there is no lifecycle hook wired for it anywhere.
pub struct Distortion {
    wet: ParamRamp,
    drive: ParamRamp,
    /// Mean square of what goes into the shaper and of what comes out,
    /// one-poled. Their ratio is the makeup: what the shaper did to the
    /// level, measured on the material actually playing rather than
    /// guessed from the drive.
    in_env: f32,
    out_env: f32,
    /// The correction in force. Held through silence rather than reset,
    /// so a gap in the music does not hand the next note full drive.
    makeup: f32,
}

impl Distortion {
    pub fn new() -> Distortion {
        Distortion {
            wet: ParamRamp::at(0.0),
            drive: ParamRamp::at(DISTORTION_DRIVE_DEFAULT),
            in_env: 0.0,
            out_env: 0.0,
            makeup: 1.0,
        }
    }

    /// The on/off switch.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            self.wet.slew(wet, EQ_ENGAGE_SECS);
        }
    }

    /// The pre-gain into the soft clip. `pade_tanh` is continuous
    /// everywhere it is defined, so unlike the bitcrusher's bit depth
    /// this needs no crossfaded handover -- a plain ramp cannot step
    /// the output, because the function it feeds cannot step either.
    pub fn set_drive(&mut self, drive: f32) {
        if let Some(drive) = knob(drive, DISTORTION_DRIVE_MIN, DISTORTION_DRIVE_MAX) {
            self.drive.slew(drive, EQ_ENGAGE_SECS);
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if wet <= 0.0 {
            // No line, no history: off is exactly the input.
            return frame;
        }
        let drive = self.drive.tick(device_rate);
        let shaped = [pade_tanh(frame[0] * drive), pade_tanh(frame[1] * drive)];

        // Makeup by measurement, not by formula. The old one divided by
        // `pade_tanh(drive)`, which holds a FULL-SCALE input at full
        // scale -- but real material sits well below that, where the
        // shaper is still nearly straight with slope `drive`, so the
        // whole pre-gain came through as volume: +1.9 dB at drive 1.0,
        // where the effect should be transparent, and +9 dB at the
        // default. Comparing the two levels instead makes drive a
        // control over character, which is what it is meant to be.
        let coeff = (1.0 / (DISTORTION_ENV_SECS * device_rate.max(1.0))).min(1.0);
        let inp = (frame[0] * frame[0] + frame[1] * frame[1]) * 0.5;
        let outp = (shaped[0] * shaped[0] + shaped[1] * shaped[1]) * 0.5;
        self.in_env += (inp - self.in_env) * coeff;
        self.out_env += (outp - self.out_env) * coeff;
        // Both envelopes carry the same smoothing, so their ratio is
        // usable long before either has settled. Below the gate there is
        // nothing to measure and the last good correction stands.
        if self.in_env > DISTORTION_ENV_GATE {
            self.makeup = (self.in_env / self.out_env.max(1e-20))
                .sqrt()
                .clamp(DISTORTION_MAKEUP_MIN, DISTORTION_MAKEUP_MAX);
        }
        let shaped = [shaped[0] * self.makeup, shaped[1] * self.makeup];

        [
            frame[0] + (shaped[0] - frame[0]) * wet,
            frame[1] + (shaped[1] - frame[1]) * wet,
        ]
    }
}

/// Four allpass stages, the classic count (two sweeping notches) small
/// pedals of this shape have used for decades -- a deliberate, tasteful
/// middle ground, not a tunable roster size.
const PHASER_STAGES: usize = 4;
/// The corner frequency's sweep range, in Hz -- the classic phaser
/// range: low enough to sweep through the low-mids, high enough to
/// reach into the presence range without leaving the notches feeling
/// disconnected from the material.
const PHASER_CORNER_LOW_HZ: f32 = 200.0;
const PHASER_CORNER_HIGH_HZ: f32 = 2_000.0;
pub(crate) const PHASER_RATE_MIN: f32 = 0.05;
pub(crate) const PHASER_RATE_MAX: f32 = 5.0;
pub(crate) const PHASER_RATE_DEFAULT: f32 = 0.5;
pub(crate) const PHASER_FEEDBACK_MAX: f32 = 0.9;
pub(crate) const PHASER_FEEDBACK_DEFAULT: f32 = 0.3;

/// One first-order allpass stage's state -- two floats per channel, the
/// tiny, fixed-size kind of filter memory [`DeckEq`]'s own biquads carry,
/// not a buffer.
#[derive(Clone, Copy, Default)]
struct AllpassStage {
    x_prev: [f32; 2],
    y_prev: [f32; 2],
}

impl AllpassStage {
    /// One sample through one first-order allpass, corner coefficient
    /// `a` from the standard bilinear-transform form:
    /// `y = -a*x + x_prev + a*y_prev`. Unity gain at every frequency;
    /// only the PHASE the signal comes out with moves, which is what
    /// summing several of these against the dry signal turns into
    /// sweeping notches.
    #[inline]
    fn process(&mut self, x: f32, a: f32, channel: usize) -> f32 {
        let y = -a * x + self.x_prev[channel] + a * self.y_prev[channel];
        self.x_prev[channel] = x;
        self.y_prev[channel] = y;
        y
    }
}

/// One deck's phaser: [`PHASER_STAGES`] cascaded allpass filters sharing
/// one LFO-swept corner frequency, summed with the dry signal to fold
/// unity-gain phase shifts into sweeping notches, with an optional
/// feedback tap back into the first stage for a more resonant character.
///
/// Filter memory only -- the same small, fixed-size kind [`DeckEq`]'s
/// biquads already carry, not a delay line -- so `reset` clears it
/// directly rather than moving a bookkeeping mark, the same reason
/// `DeckEq::reset` does.
pub struct Phaser {
    stages: [AllpassStage; PHASER_STAGES],
    /// The chain's own last output, fed back into the first stage's
    /// input next frame when `feedback` is above zero.
    last_output: [f32; 2],
    phase: f32,
    wet: ParamRamp,
    rate: ParamRamp,
    feedback: ParamRamp,
    /// Which rung of [`LFO_SYNC_ROWS`] the sweep runs on:
    /// [`LFO_SYNC_FREE`] to follow `rate`'s Hz, or eighths of a cycle
    /// per beat to follow the deck's grid instead, through `active_hz`
    /// -- recomputed once a buffer in `prepare_block`. A division is
    /// deliberately NOT stored in `rate`: that setter clamps to this
    /// effect's own Hz range, which a division has no reason to fit
    /// inside.
    sync_units: u32,
    /// Phase offset within the cycle, 0..1, applied once at the moment
    /// of engage (see `set_wet`), never live.
    beat_offset: f32,
    /// This buffer's locked rate in Hz; unused (stale) on the
    /// free-running rung.
    active_hz: f32,
}

impl Phaser {
    pub fn new() -> Phaser {
        Phaser {
            stages: [AllpassStage::default(); PHASER_STAGES],
            last_output: [0.0, 0.0],
            phase: 0.0,
            wet: ParamRamp::at(0.0),
            rate: ParamRamp::at(PHASER_RATE_DEFAULT),
            feedback: ParamRamp::at(PHASER_FEEDBACK_DEFAULT),
            sync_units: LFO_SYNC_FREE,
            beat_offset: 0.0,
            active_hz: PHASER_RATE_DEFAULT,
        }
    }

    /// Clears the allpass stages' and the feedback tap's memory
    /// directly -- a handful of floats, not a buffer, so there is
    /// nothing here a real-time bulk clear would be unsafe to touch,
    /// the same reason `DeckEq::reset` is shaped this way.
    pub fn reset(&mut self) {
        self.stages = [AllpassStage::default(); PHASER_STAGES];
        self.last_output = [0.0, 0.0];
    }

    /// The on/off switch. Starts the sweep at its chosen place in the
    /// cycle when beat-synced, gated on the engage transition so it can
    /// never move the sweep under a sounding deck.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            if wet > 0.0 && self.wet.target() == 0.0 && self.sync_units > 0 {
                self.phase = self.beat_offset * std::f32::consts::TAU;
            }
            self.wet.slew(wet, EQ_ENGAGE_SECS);
        }
    }

    /// The LFO's sweep speed, in Hz -- what plays on the free-running
    /// rung. A locked rung takes its rate from the grid instead and
    /// leaves this alone.
    pub fn set_rate(&mut self, hz: f32) {
        if let Some(hz) = knob(hz, PHASER_RATE_MIN, PHASER_RATE_MAX) {
            self.rate.slew(hz, EQ_ENGAGE_SECS);
        }
    }
    /// Pick the sweep's rung: [`LFO_SYNC_FREE`] for the Hz slider, or
    /// eighths of a cycle per beat to follow the grid.
    pub fn set_sync_units(&mut self, units: u32) {
        self.sync_units = units.min(LFO_SYNC_MAX_UNITS);
    }

    pub fn sync_units(&self) -> u32 {
        self.sync_units
    }

    /// Where in the cycle the sweep starts on engage, 0..1.
    pub fn set_beat_offset(&mut self, offset: f32) {
        if let Some(offset) = knob(offset, 0.0, 1.0) {
            self.beat_offset = offset;
        }
    }

    /// Recompute this buffer's beat-synced rate from the deck's current
    /// tempo; see [`Tremolo::prepare_block`] for the full reasoning.
    pub fn prepare_block(
        &mut self,
        clock: &crate::wave_analysis::DeckClock,
        buffer_secs: f32,
    ) {
        if self.sync_units == 0 {
            return;
        }
        // Ungridded or stopped, the counted beat -- the same default a
        // loop or a jump takes when nothing has measured the record.
        let beat_secs = clock.beat_len().unwrap_or(60.0 / crate::decks::COUNTED_BPM);
        let cycles = sync_cycles_per_beat(self.sync_units);
        let base = (cycles / beat_secs.max(1e-6)) as f32;
        self.active_hz =
            lfo_locked_hz(self.phase, base, cycles, self.beat_offset, buffer_secs, clock);
    }


    /// How much of the chain's own output feeds back into the first
    /// stage, saturated through [`pade_tanh`] like every other
    /// regeneration path in this file, so cranking it saturates instead
    /// of runs away.
    pub fn set_feedback(&mut self, feedback: f32) {
        if let Some(feedback) = knob(feedback, 0.0, PHASER_FEEDBACK_MAX) {
            self.feedback.slew(feedback, EQ_ENGAGE_SECS);
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if wet <= 0.0 {
            // No line, no ring: the frozen filter memory below is not
            // touched again until re-engaged, exactly how DeckEq's own
            // biquad state behaves while bypassed.
            return frame;
        }
        let rate_hz = if self.sync_units > 0 {
            self.active_hz
        } else {
            self.rate.tick(device_rate)
        };
        let feedback = self.feedback.tick(device_rate);

        self.phase += std::f32::consts::TAU * rate_hz / device_rate.max(1.0);
        if self.phase >= std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }
        let lfo01 = 0.5 * (1.0 + self.phase.sin());
        let corner = PHASER_CORNER_LOW_HZ + (PHASER_CORNER_HIGH_HZ - PHASER_CORNER_LOW_HZ) * lfo01;
        let t = (PI * corner / device_rate.max(1.0)).tan();
        let a = ((t - 1.0) / (t + 1.0)).clamp(-0.999, 0.999);

        let mut allpassed = [0.0f32; 2];
        for channel in 0..2 {
            let fb = pade_tanh(self.last_output[channel] * feedback);
            let mut sample = frame[channel] + fb;
            for stage in &mut self.stages {
                sample = stage.process(sample, a, channel);
            }
            allpassed[channel] = sample;
        }
        self.last_output = allpassed;
        // The classic phaser mix: equal parts dry and allpassed, which
        // is what turns a filter that is unity gain at every frequency
        // into sweeping notches -- the frequencies where the two land
        // out of phase cancel. `wet` blends THIS whole notched signal
        // in and out, on top of that fixed internal mix.
        let notched =
            [(frame[0] + allpassed[0]) * 0.5, (frame[1] + allpassed[1]) * 0.5];

        [
            frame[0] + (notched[0] - frame[0]) * wet,
            frame[1] + (notched[1] - frame[1]) * wet,
        ]
    }
}

pub(crate) const AUTOPAN_RATE_MIN: f32 = 0.1;
pub(crate) const AUTOPAN_RATE_MAX: f32 = 20.0;
pub(crate) const AUTOPAN_RATE_DEFAULT: f32 = 1.0;
pub(crate) const AUTOPAN_DEPTH_DEFAULT: f32 = 1.0;

/// One deck's autopan: a sine LFO sweeping the stereo position through
/// an equal-power pan law, normalized so the LFO's own centre (pan = 0)
/// is unity gain on both channels -- unlike the tremolo's reference
/// gain, which sits at the TOP of its range (`gain = 1.0` at rest, only
/// ever attenuating), a pan law's natural centre is -3 dB per channel,
/// which would make `depth = 0` a real, audible cut rather than a true
/// no-op. The `sqrt(2)` normalization below is what fixes that: at
/// depth 0 the swing never leaves centre, and centre times the
/// normalization is exactly 1.0.
///
/// No line, no filter history -- like [`Tremolo`], `phase` is an
/// oscillator position, not audio content, so there is no lifecycle
/// hook wired for this one anywhere either.
pub struct Autopan {
    phase: f32,
    wet: ParamRamp,
    rate: ParamRamp,
    depth: ParamRamp,
    /// Which rung of [`LFO_SYNC_ROWS`] the sweep runs on:
    /// [`LFO_SYNC_FREE`] to follow `rate`'s Hz, or eighths of a cycle
    /// per beat to follow the deck's grid instead, through `active_hz`
    /// -- recomputed once a buffer in `prepare_block`. A division is
    /// deliberately NOT stored in `rate`: that setter clamps to this
    /// effect's own Hz range, which a division has no reason to fit
    /// inside.
    sync_units: u32,
    /// Phase offset within the cycle, 0..1, applied once at the moment
    /// of engage (see `set_wet`), never live.
    beat_offset: f32,
    /// This buffer's locked rate in Hz; unused (stale) on the
    /// free-running rung.
    active_hz: f32,
}

impl Autopan {
    pub fn new() -> Autopan {
        Autopan {
            phase: 0.0,
            wet: ParamRamp::at(0.0),
            rate: ParamRamp::at(AUTOPAN_RATE_DEFAULT),
            depth: ParamRamp::at(AUTOPAN_DEPTH_DEFAULT),
            sync_units: LFO_SYNC_FREE,
            beat_offset: 0.0,
            active_hz: AUTOPAN_RATE_DEFAULT,
        }
    }

    /// The on/off switch. Starts the swing at its chosen place in the
    /// cycle when beat-synced, gated on the engage transition so it can
    /// never move the pan under a sounding deck.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            if wet > 0.0 && self.wet.target() == 0.0 && self.sync_units > 0 {
                self.phase = self.beat_offset * std::f32::consts::TAU;
            }
            self.wet.slew(wet, EQ_ENGAGE_SECS);
        }
    }

    /// The LFO's sweep speed, in Hz -- what plays on the free-running
    /// rung. A locked rung takes its rate from the grid instead and
    /// leaves this alone.
    pub fn set_rate(&mut self, hz: f32) {
        if let Some(hz) = knob(hz, AUTOPAN_RATE_MIN, AUTOPAN_RATE_MAX) {
            self.rate.slew(hz, EQ_ENGAGE_SECS);
        }
    }
    /// Pick the swing's rung: [`LFO_SYNC_FREE`] for the Hz slider, or
    /// eighths of a cycle per beat to follow the grid.
    pub fn set_sync_units(&mut self, units: u32) {
        self.sync_units = units.min(LFO_SYNC_MAX_UNITS);
    }

    pub fn sync_units(&self) -> u32 {
        self.sync_units
    }

    /// Where in the cycle the swing starts on engage, 0..1.
    pub fn set_beat_offset(&mut self, offset: f32) {
        if let Some(offset) = knob(offset, 0.0, 1.0) {
            self.beat_offset = offset;
        }
    }

    /// Recompute this buffer's beat-synced rate from the deck's current
    /// tempo; see [`Tremolo::prepare_block`] for the full reasoning.
    pub fn prepare_block(
        &mut self,
        clock: &crate::wave_analysis::DeckClock,
        buffer_secs: f32,
    ) {
        if self.sync_units == 0 {
            return;
        }
        // Ungridded or stopped, the counted beat -- the same default a
        // loop or a jump takes when nothing has measured the record.
        let beat_secs = clock.beat_len().unwrap_or(60.0 / crate::decks::COUNTED_BPM);
        let cycles = sync_cycles_per_beat(self.sync_units);
        let base = (cycles / beat_secs.max(1e-6)) as f32;
        self.active_hz =
            lfo_locked_hz(self.phase, base, cycles, self.beat_offset, buffer_secs, clock);
    }


    /// How far the pan swings from centre, 0..1. `cos`/`sin` are
    /// continuous everywhere, so -- like the tremolo's depth -- this
    /// needs no crossfaded handover the way the bitcrusher's bit depth
    /// does; a plain ramp cannot step an output the pan law cannot step
    /// either.
    pub fn set_depth(&mut self, depth: f32) {
        if let Some(depth) = knob(depth, 0.0, 1.0) {
            self.depth.slew(depth, EQ_ENGAGE_SECS);
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if wet == 0.0 && self.wet.target() == 0.0 {
            // No line, no tail: off is exactly the input.
            return frame;
        }
        let rate_hz = if self.sync_units > 0 {
            self.active_hz
        } else {
            self.rate.tick(device_rate)
        };
        let depth = self.depth.tick(device_rate);

        self.phase += std::f32::consts::TAU * rate_hz / device_rate.max(1.0);
        if self.phase >= std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }
        let pan = self.phase.sin() * depth;
        let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let norm = std::f32::consts::SQRT_2;
        let panned = [frame[0] * angle.cos() * norm, frame[1] * angle.sin() * norm];

        [
            frame[0] + (panned[0] - frame[0]) * wet,
            frame[1] + (panned[1] - frame[1]) * wet,
        ]
    }
}

pub(crate) const STEREO_WIDTH_MIN: f32 = 0.0;
pub(crate) const STEREO_WIDTH_MAX: f32 = 2.0;
pub(crate) const STEREO_WIDTH_DEFAULT: f32 = 1.5;

/// One deck's stereo width: mid/side decompose, scale the side signal
/// by `width`, recombine. `width = 1.0` is the identity -- mid + side
/// and mid - side reconstruct the original left and right exactly --
/// `width = 0.0` collapses to mono (both channels become the mid), and
/// `width > 1.0` exaggerates the difference between the channels.
///
/// No line, no filter history -- like [`Tremolo`]/[`Autopan`], the
/// whole state is two `ParamRamp`s, so no lifecycle hook is wired for
/// this one anywhere either. Deliberately no internal saturation: a
/// worst-case, fully out-of-phase source at the top of the width range
/// can genuinely exceed unity, and the master bus's own clamp downstream
/// is what catches that, the same safety net every deck already relies
/// on rather than each effect duplicating it.
pub struct StereoWidth {
    wet: ParamRamp,
    width: ParamRamp,
}

impl StereoWidth {
    pub fn new() -> StereoWidth {
        StereoWidth { wet: ParamRamp::at(0.0), width: ParamRamp::at(STEREO_WIDTH_DEFAULT) }
    }

    /// The on/off switch.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            self.wet.slew(wet, EQ_ENGAGE_SECS);
        }
    }

    /// How much the side signal is scaled: 0 collapses to mono, 1 is
    /// the identity, above 1 widens further. A plain multiply is
    /// continuous everywhere, so -- like the tremolo's depth -- this
    /// needs no crossfaded handover.
    pub fn set_width(&mut self, width: f32) {
        if let Some(width) = knob(width, STEREO_WIDTH_MIN, STEREO_WIDTH_MAX) {
            self.width.slew(width, EQ_ENGAGE_SECS);
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if wet == 0.0 && self.wet.target() == 0.0 {
            // No line, no tail: off is exactly the input.
            return frame;
        }
        let width = self.width.tick(device_rate);

        let mid = (frame[0] + frame[1]) * 0.5;
        let side = (frame[0] - frame[1]) * 0.5;
        let widened = [mid + side * width, mid - side * width];

        [
            frame[0] + (widened[0] - frame[0]) * wet,
            frame[1] + (widened[1] - frame[1]) * wet,
        ]
    }
}

pub(crate) const PLATE_REVERB_SIZE_MIN: f32 = 0.0;
pub(crate) const PLATE_REVERB_SIZE_MAX: f32 = 1.0;
pub(crate) const PLATE_REVERB_SIZE_DEFAULT: f32 = 0.5;

/// Comb delay lengths for the left and right tanks, in milliseconds.
/// Mutually non-commensurate (no small shared factor) so the parallel
/// combs' resonances interleave into a dense tail rather than
/// reinforcing a single audible pitch, and the left set is offset from
/// the right so the two channels decorrelate instead of reading as one
/// mono tail panned to both speakers.
const PLATE_REVERB_COMB_MS_L: [f32; 4] = [29.7, 37.1, 41.1, 43.7];
const PLATE_REVERB_COMB_MS_R: [f32; 4] = [30.9, 38.3, 42.5, 44.9];
/// Series allpass stages after the comb bank, textbook Schroeder
/// diffusion: short, again non-commensurate with each other and with
/// the combs above.
const PLATE_REVERB_ALLPASS_MS: [f32; 2] = [5.0, 1.7];
const PLATE_REVERB_ALLPASS_G: f32 = 0.5;
/// One-pole lowpass coefficient inside each comb's feedback path: higher
/// damps the tail's top end faster than its body, the way a real room's
/// air and surfaces do.
const PLATE_REVERB_DAMP: f32 = 0.2;
const PLATE_REVERB_FEEDBACK_MIN: f32 = 0.6;
const PLATE_REVERB_FEEDBACK_MAX: f32 = 0.97;
/// Below this peak the tail is treated as fully decayed -- the same
/// threshold `DeckEcho`'s own `quiet` tracking uses: once wet is at
/// zero AND the tank has actually rung out, the lines stop being
/// touched at all rather than being asked to keep circulating silence
/// forever.
const PLATE_REVERB_QUIET: f32 = 1e-5;
/// How often the accumulated peak is checked against [`PLATE_REVERB_QUIET`],
/// rather than testing one raw sample every call: a comb/allpass tank's
/// release-phase output is a decaying but OSCILLATING signal -- sparse
/// and gappy, not a smooth envelope -- so a single sample can land on a
/// zero-crossing long before the tail has genuinely decayed, freezing
/// the tank there and chopping off real, if declining, tail content.
/// Longer than the longest comb's delay (44.9ms) so every tap gets at
/// least one full cycle inside each window, mirroring `DeckEcho`'s own
/// `tail_peak`/`period_left` windowing over its tap period exactly.
const PLATE_REVERB_QUIET_PERIOD_MS: f32 = 50.0;

#[inline]
fn ms_to_frames(ms: f32, sample_rate: f32) -> usize {
    ((ms / 1000.0) * sample_rate).round().max(1.0) as usize
}

/// A comb filter with a one-pole damping filter in its feedback path:
/// the resonant building block of the tank below. `process` returns the
/// OLD content of the line before writing this call's input plus
/// feedback into the same slot, so a length-1 line is a trivial one-
/// sample delay rather than a divide-by-zero.
struct ReverbComb {
    line: Vec<f32>,
    write: usize,
    damp_state: f32,
}

impl ReverbComb {
    fn new(frames: usize) -> ReverbComb {
        ReverbComb { line: vec![0.0; frames.max(1)], write: 0, damp_state: 0.0 }
    }

    #[inline]
    fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.line[self.write];
        self.damp_state = out * (1.0 - damp) + self.damp_state * damp;
        self.line[self.write] = x + self.damp_state * feedback;
        self.write += 1;
        if self.write >= self.line.len() {
            self.write = 0;
        }
        out
    }

    fn silence(&mut self) {
        self.line.iter_mut().for_each(|s| *s = 0.0);
        self.damp_state = 0.0;
    }
}

/// The one-multiply Schroeder allpass: unity gain at every frequency,
/// so it diffuses the comb bank's output into a denser tail without
/// coloring it. `w[n] = x[n] + g*w[n-D]`, `y[n] = w[n-D] - g*w[n]` --
/// one delay line carries both the numerator and denominator halves of
/// the transfer function, so only `w` needs storing.
struct ReverbAllpass {
    line: Vec<f32>,
    write: usize,
}

impl ReverbAllpass {
    fn new(frames: usize) -> ReverbAllpass {
        ReverbAllpass { line: vec![0.0; frames.max(1)], write: 0 }
    }

    #[inline]
    fn process(&mut self, x: f32, g: f32) -> f32 {
        let delayed = self.line[self.write];
        let w = x + g * delayed;
        let y = delayed - g * w;
        self.line[self.write] = w;
        self.write += 1;
        if self.write >= self.line.len() {
            self.write = 0;
        }
        y
    }

    fn silence(&mut self) {
        self.line.iter_mut().for_each(|s| *s = 0.0);
    }
}

/// One channel's worth of tank: four parallel combs summed and averaged,
/// then diffused through two series allpasses. `PlateReverb` holds two
/// of these, built from different comb lengths, for the left and right
/// output.
struct ReverbTank {
    combs: [ReverbComb; 4],
    allpasses: [ReverbAllpass; 2],
}

impl ReverbTank {
    fn new(comb_ms: [f32; 4], sample_rate: f32) -> ReverbTank {
        ReverbTank {
            combs: comb_ms.map(|ms| ReverbComb::new(ms_to_frames(ms, sample_rate))),
            allpasses: PLATE_REVERB_ALLPASS_MS.map(|ms| ReverbAllpass::new(ms_to_frames(ms, sample_rate))),
        }
    }

    #[inline]
    fn process(&mut self, x: f32, feedback: f32) -> f32 {
        let mut sum = 0.0f32;
        for comb in &mut self.combs {
            sum += comb.process(x, feedback, PLATE_REVERB_DAMP);
        }
        sum *= 0.25;
        for allpass in &mut self.allpasses {
            sum = allpass.process(sum, PLATE_REVERB_ALLPASS_G);
        }
        sum
    }

    fn silence(&mut self) {
        for comb in &mut self.combs {
            comb.silence();
        }
        for allpass in &mut self.allpasses {
            allpass.silence();
        }
    }
}

/// One deck's plate reverb: a mono send into two decorrelated tanks,
/// added back onto the dry frame -- an additive send like
/// [`DeckEcho`], never a dry/wet crossfade like the tone-shaping
/// effects above it, because muting the dry signal the instant the
/// reverb engages would remove exactly the transient a reverb is
/// supposed to be heard alongside.
///
/// `size` is the tank's only exposed control today, the same
/// ship-one-knob-first choice this file already made for the autopan's
/// depth: it maps onto the comb feedback, the single parameter that
/// most changes how the reverb reads (how long the tail rings), while
/// damping stays a fixed constant.
pub struct PlateReverb {
    tank_l: ReverbTank,
    tank_r: ReverbTank,
    sample_rate: f32,
    wet: ParamRamp,
    size: ParamRamp,
    /// The tank's output peak accumulated over the current
    /// [`PLATE_REVERB_QUIET_PERIOD_MS`] window -- checked against
    /// [`PLATE_REVERB_QUIET`] only once the window elapses, never from
    /// a single sample.
    tail_peak: f32,
    /// Frames left in the current peak-accumulation window.
    period_left: u32,
    /// Set once a full window's accumulated peak has genuinely decayed
    /// below the quiet threshold after `wet` reached zero, so a
    /// long-released tail keeps decaying through the lines instead of
    /// being cut the moment the engage ramp finishes -- or, worse, the
    /// moment any one sample happens to cross zero.
    quiet: bool,
}

impl PlateReverb {
    pub fn new(sample_rate: f32) -> PlateReverb {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        PlateReverb {
            tank_l: ReverbTank::new(PLATE_REVERB_COMB_MS_L, sample_rate),
            tank_r: ReverbTank::new(PLATE_REVERB_COMB_MS_R, sample_rate),
            sample_rate,
            wet: ParamRamp::at(0.0),
            size: ParamRamp::at(PLATE_REVERB_SIZE_DEFAULT),
            tail_peak: 0.0,
            period_left: 0,
            quiet: true,
        }
    }

    /// Rebuild the tank's fixed delay lines for a new device rate. A
    /// rare, hard-reset-worthy event, like the EQ's own crossover
    /// coefficients rebuilt by [`DeckEq::set_sample_rate`]: these
    /// lengths are physical constants converted to frames, not
    /// something an operator retunes live, so there is no in-flight
    /// handover to preserve across the change.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }
        if (self.sample_rate - sample_rate).abs() < 0.5 {
            return;
        }
        self.sample_rate = sample_rate;
        self.tank_l = ReverbTank::new(PLATE_REVERB_COMB_MS_L, sample_rate);
        self.tank_r = ReverbTank::new(PLATE_REVERB_COMB_MS_R, sample_rate);
        self.tail_peak = 0.0;
        self.period_left = 0;
        self.quiet = true;
    }

    /// The on/off switch.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            let engaging = wet > 0.0;
            self.wet.slew(wet, EQ_ENGAGE_SECS);
            if engaging {
                // A fresh engagement is never quiet, even if the last
                // one ended that way: `quiet` otherwise stays
                // stale-true from the last time the tail actually rang
                // out, and a re-engage-then-disengage cycle faster than
                // one `PLATE_REVERB_QUIET_PERIOD_MS` window (so the
                // periodic check in `process` never runs to correct it)
                // would bypass on that stale flag with fresh, un-decayed
                // energy still sitting in the tank -- the same fix
                // `Flanger::set_wet` and `DeckEcho::set_fraction` apply
                // for the identical reason.
                self.quiet = false;
            }
        }
    }

    /// How long the tail rings: maps onto the comb bank's feedback.
    pub fn set_size(&mut self, size: f32) {
        if let Some(size) = knob(size, PLATE_REVERB_SIZE_MIN, PLATE_REVERB_SIZE_MAX) {
            self.size.slew(size, EQ_ENGAGE_SECS);
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    /// Drop the tank's stored energy directly rather than freeing it.
    /// A different technique from [`DeckEcho::silence`] and
    /// [`Flanger::silence`]'s watermark trick (advancing a mark so
    /// stale content merely reads as silence, since memsetting THEIR
    /// multi-thousand-frame lines from the callback would be a
    /// real-time violation) -- but a reasonable one here, since the
    /// longest comb line is only ~1-2 thousand samples (44.9ms at
    /// 48kHz), cheap enough to clear directly.
    pub fn silence(&mut self) {
        self.tank_l.silence();
        self.tank_r.silence();
        self.tail_peak = 0.0;
        self.period_left = 0;
        self.quiet = true;
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if self.quiet && wet == 0.0 && self.wet.target() == 0.0 {
            // Off, and the tail already rang out: the lines are not
            // touched at all, and the frame is the input exactly.
            return frame;
        }
        let size = self.size.tick(device_rate);
        let feedback =
            PLATE_REVERB_FEEDBACK_MIN + (PLATE_REVERB_FEEDBACK_MAX - PLATE_REVERB_FEEDBACK_MIN) * size;
        // The write into the tank is scaled by the SAME `wet` that
        // gates the output -- the wet-gates-the-write rule every
        // delay-holding effect in this file follows, or full-amplitude
        // content would enter the lines during an engage ramp and
        // surface later as a click once it ages past the delay.
        let mono_in = (frame[0] + frame[1]) * 0.5 * wet;
        let wet_l = self.tank_l.process(mono_in, feedback);
        let wet_r = self.tank_r.process(mono_in, feedback);
        // Accumulate the peak across a full window rather than testing
        // one raw sample: the tank's release-phase output oscillates as
        // it decays, so any single sample can land on a zero-crossing
        // long before the true envelope has actually decayed away.
        self.tail_peak = self.tail_peak.max(wet_l.abs()).max(wet_r.abs());
        if self.period_left == 0 {
            self.period_left =
                ms_to_frames(PLATE_REVERB_QUIET_PERIOD_MS, device_rate.max(1.0)) as u32;
        }
        self.period_left -= 1;
        if self.period_left == 0 {
            if wet == 0.0 && self.wet.target() == 0.0 && self.tail_peak < PLATE_REVERB_QUIET {
                self.quiet = true;
            }
            self.tail_peak = 0.0;
        }
        [frame[0] + wet_l, frame[1] + wet_r]
    }
}

pub(crate) const MOOG_LADDER_CUTOFF_MIN: f32 = 60.0;
pub(crate) const MOOG_LADDER_CUTOFF_MAX: f32 = 12_000.0;
pub(crate) const MOOG_LADDER_CUTOFF_DEFAULT: f32 = 1_200.0;
pub(crate) const MOOG_LADDER_RESONANCE_MIN: f32 = 0.0;
pub(crate) const MOOG_LADDER_RESONANCE_MAX: f32 = 1.0;
pub(crate) const MOOG_LADDER_RESONANCE_DEFAULT: f32 = 0.3;
/// The classic ladder's feedback coefficient at full resonance --
/// public-domain textbook territory (Stilson and Smith's 1996 analysis
/// of the Moog transistor ladder), where self-oscillation begins near
/// 4.0. Fed through [`pade_tanh`] the way the real transistor ladder's
/// own saturation bounds it, so a value at or even past this point
/// stays finite rather than diverging.
const MOOG_LADDER_RESONANCE_K: f32 = 4.0;

/// One channel's four cascaded one-pole lowpass stages plus the
/// resonance feedback from the last stage back to the first. The
/// textbook simplified digital ladder model: only the feedback path is
/// saturated (not every stage), which is enough to keep the loop
/// bounded at any resonance without the extra per-stage nonlinearity's
/// aliasing cost.
#[derive(Default)]
struct MoogLadderChannel {
    y: [f32; 4],
}

impl MoogLadderChannel {
    #[inline]
    fn process(&mut self, x: f32, g: f32, k: f32) -> f32 {
        let mut v = x - pade_tanh(self.y[3]) * k;
        for stage in &mut self.y {
            *stage += g * (v - *stage);
            v = *stage;
        }
        self.y[3]
    }

    fn reset(&mut self) {
        self.y = [0.0; 4];
    }
}

/// One deck's Moog-style resonant lowpass: a swept cutoff with a
/// resonance knob that can push the ladder into self-oscillation at its
/// top end, the character the real analog filter is known for.
///
/// Filter memory only, a few floats per channel -- no line, no tail
/// that would ring on after a record change -- so this gets `reset()`
/// at the four lifecycle sites, the same shape [`DeckEq::reset`] and
/// [`Phaser::reset`] already settled on, not the "no hook" treatment
/// the LFO-only effects get.
pub struct MoogLadder {
    channels: [MoogLadderChannel; 2],
    wet: ParamRamp,
    cutoff: ParamRamp,
    resonance: ParamRamp,
}

impl MoogLadder {
    pub fn new() -> MoogLadder {
        MoogLadder {
            channels: Default::default(),
            wet: ParamRamp::at(0.0),
            cutoff: ParamRamp::at(MOOG_LADDER_CUTOFF_DEFAULT),
            resonance: ParamRamp::at(MOOG_LADDER_RESONANCE_DEFAULT),
        }
    }

    /// The on/off switch.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            self.wet.slew(wet, EQ_ENGAGE_SECS);
        }
    }

    /// Where the ladder starts rolling off, in Hz. Ramped over 144ms
    /// (12x [`EQ_ENGAGE_SECS`]), much wider than the 48ms
    /// [`DeckEq::set_filter`] uses for its own swept corner: unlike the
    /// phaser's allpass coefficient (unity gain everywhere, only phase
    /// moves), this filter's coefficient controls actual GAIN, so a
    /// wide, ordinary-speed excursion -- ordinary because the slider
    /// spans 60 to 12000Hz and an ordinary drag covers that range in a
    /// few dozen milliseconds -- can leave the cascade's state lagging
    /// far behind a coefficient that has already reached a much less
    /// attenuating value, producing a real, audible step on release
    /// even though the ramp itself is perfectly continuous. This is a
    /// threshold effect, not a proportional one: an adversarial review
    /// found a worst-case step of 0.052 against this file's own 0.02
    /// click budget at 12ms, widening to 48ms (DeckEq's own multiplier)
    /// only brought it to 0.046, and it took 120ms to clear the budget
    /// -- 144ms is that with margin. No dual-cascade crossfade handover
    /// (DeckEq's other mechanism, needed there because its coefficients
    /// are quantized to one recompute per device buffer rather than
    /// ramped continuously) turned out to be necessary once the ramp
    /// itself was slow enough.
    pub fn set_cutoff(&mut self, hz: f32) {
        if let Some(hz) = knob(hz, MOOG_LADDER_CUTOFF_MIN, MOOG_LADDER_CUTOFF_MAX) {
            self.cutoff.slew(hz, EQ_ENGAGE_SECS * 12.0);
        }
    }

    /// How much of the last stage feeds back into the first; 1.0 can
    /// self-oscillate. Same widened ramp as [`Self::set_cutoff`] and
    /// for the same reason: `k` scales the feedback tap directly, so a
    /// fast excursion while the tap is already large can also step the
    /// output.
    pub fn set_resonance(&mut self, resonance: f32) {
        if let Some(resonance) = knob(resonance, MOOG_LADDER_RESONANCE_MIN, MOOG_LADDER_RESONANCE_MAX) {
            self.resonance.slew(resonance, EQ_ENGAGE_SECS * 12.0);
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    pub fn reset(&mut self) {
        self.channels = Default::default();
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        let wet = self.wet.tick(device_rate);
        if wet <= 0.0 {
            // No line, no tail: off is exactly the input.
            return frame;
        }
        let cutoff = self.cutoff.tick(device_rate);
        let resonance = self.resonance.tick(device_rate);
        // The standard one-pole coefficient from a cutoff in Hz; a
        // continuous function of a continuously-ramped `cutoff`, so
        // this needs no crossfaded handover the way a discontinuous
        // parameter (the bitcrusher's bit depth) does.
        let g = 1.0 - (-2.0 * PI * cutoff / device_rate.max(1.0)).exp();
        let k = resonance * MOOG_LADDER_RESONANCE_K;
        let filtered = [
            self.channels[0].process(frame[0], g, k),
            self.channels[1].process(frame[1], g, k),
        ];
        [
            frame[0] + (filtered[0] - frame[0]) * wet,
            frame[1] + (filtered[1] - frame[1]) * wet,
        ]
    }
}

// ---------------------------------------------------------------------------
// what a chain slot does about its effect's level
// ---------------------------------------------------------------------------

/// How fast a slot's two level meters follow the music when it is
/// matching. Long: this is a loudness match, not a compressor, and it
/// must not ride the waveform.
const SLOT_ENV_SECS: f32 = 0.3;
/// How fast a ceiling lets go once the peak that triggered it has passed.
const SLOT_RELEASE_SECS: f32 = 0.15;
/// Mean square below which there is nothing to measure, about -70 dB.
/// Under it the last good correction stands.
const SLOT_ENV_GATE: f32 = 1e-7;
/// How far a correction may reach, plus and minus twelve decibels. An
/// effect that needs more than this is not being level-matched, it is
/// being rebuilt, and the operator should hear that rather than have it
/// hidden.
const SLOT_GAIN_MIN: f32 = 0.25;
const SLOT_GAIN_MAX: f32 = 4.0;

/// What a slot does about the level its effect hands back.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LevelMode {
    /// Whatever the deck's own default says. The default for every slot,
    /// so one setting moves the whole chain and any slot may still be
    /// pinned on its own.
    #[default]
    Follow,
    /// Leave it alone. What the deck defaults to, so nothing changes for
    /// anyone until they ask for it.
    Off,
    /// Hold the effect's output at the loudness of what went in, so
    /// engaging it changes the sound and not the volume.
    MatchInput,
    /// Hold the output under `ceiling`, in linear amplitude.
    Ceiling,
}

/// The rows a slot's level dropdown serves. Short labels: the picker is
/// the same narrow chip the deck's own dropdowns use.
pub const LEVEL_MODE_ROWS: [(u32, &str); 4] =
    [(0, "FOL"), (1, "OFF"), (2, "MTCH"), (3, "CAP")];

impl LevelMode {
    /// The dropdown row this mode sits on.
    pub fn as_row(self) -> u32 {
        match self {
            LevelMode::Follow => 0,
            LevelMode::Off => 1,
            LevelMode::MatchInput => 2,
            LevelMode::Ceiling => 3,
        }
    }

    /// The mode a picked row means. Anything unknown reads as Follow,
    /// which is the harmless answer.
    pub fn from_row(row: u32) -> LevelMode {
        match row {
            1 => LevelMode::Off,
            2 => LevelMode::MatchInput,
            3 => LevelMode::Ceiling,
            _ => LevelMode::Follow,
        }
    }
}

/// One chain slot's level policy: the wet/dry mix the operator sets, and
/// what, if anything, is done about the level the effect returns.
///
/// This lives at the SLOT rather than inside the effects because it then
/// serves all twelve of them, and any future one, with no per-effect
/// code. It composes correctly with the additive effects too: a slot
/// blend of the reverb's `dry + send` works out as `dry + send * mix`,
/// which is exactly "scale the send".
///
/// The effects keep their own `wet` ramp as the ENGAGE ramp -- zero or
/// one, the click-free on/off each of them already tests. `mix` is a
/// separate, operator-facing control that rides on top.
pub struct SlotLevel {
    mode: LevelMode,
    mix: ParamRamp,
    ceiling: f32,
    in_env: f32,
    out_env: f32,
    /// The correction actually in force.
    gain: f32,
}

impl SlotLevel {
    pub fn new() -> SlotLevel {
        SlotLevel {
            mode: LevelMode::Follow,
            mix: ParamRamp::at(1.0),
            ceiling: 1.0,
            in_env: 0.0,
            out_env: 0.0,
            gain: 1.0,
        }
    }

    pub fn set_mode(&mut self, mode: LevelMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> LevelMode {
        self.mode
    }

    /// The wet/dry blend, 0 = the effect is inaudible, 1 = all of it.
    pub fn set_mix(&mut self, mix: f32) {
        if let Some(mix) = knob(mix, 0.0, 1.0) {
            self.mix.slew(mix, EQ_ENGAGE_SECS);
        }
    }

    pub fn mix(&self) -> f32 {
        self.mix.target()
    }

    /// The amplitude a `Ceiling` slot holds its output under.
    pub fn set_ceiling(&mut self, ceiling: f32) {
        if let Some(ceiling) = knob(ceiling, 0.01, 1.0) {
            self.ceiling = ceiling;
        }
    }

    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }

    /// Blend and correct one frame. `dry` is what went into the effect,
    /// `wet` what it returned, and `default` the deck's own policy for
    /// the slots that follow it.
    #[inline]
    pub fn apply(
        &mut self,
        dry: [f32; 2],
        wet: [f32; 2],
        device_rate: f32,
        default: LevelMode,
    ) -> [f32; 2] {
        // `Follow` resolving to `Follow` would be a loop; the deck's own
        // default is never that, but resolve it defensively rather than
        // trust a caller.
        let mode = match self.mode {
            LevelMode::Follow => match default {
                LevelMode::Follow => LevelMode::Off,
                resolved => resolved,
            },
            pinned => pinned,
        };

        match mode {
            LevelMode::Off | LevelMode::Follow => self.gain = 1.0,
            LevelMode::MatchInput => {
                let coeff = (1.0 / (SLOT_ENV_SECS * device_rate.max(1.0))).min(1.0);
                let dry_ms = (dry[0] * dry[0] + dry[1] * dry[1]) * 0.5;
                let wet_ms = (wet[0] * wet[0] + wet[1] * wet[1]) * 0.5;
                self.in_env += (dry_ms - self.in_env) * coeff;
                self.out_env += (wet_ms - self.out_env) * coeff;
                // Both envelopes carry the same smoothing, so the ratio
                // is usable well before either has settled.
                if self.in_env > SLOT_ENV_GATE && self.out_env > SLOT_ENV_GATE {
                    self.gain = (self.in_env / self.out_env)
                        .sqrt()
                        .clamp(SLOT_GAIN_MIN, SLOT_GAIN_MAX);
                }
            }
            LevelMode::Ceiling => {
                let peak = wet[0].abs().max(wet[1].abs());
                let target = match peak > self.ceiling {
                    true => (self.ceiling / peak).max(SLOT_GAIN_MIN),
                    false => 1.0,
                };
                // Down at once, back up slowly: a limiter has to catch
                // the sample that overshot, and may take its time
                // letting go.
                if target < self.gain {
                    self.gain = target;
                } else {
                    let coeff = (1.0 / (SLOT_RELEASE_SECS * device_rate.max(1.0))).min(1.0);
                    self.gain += (target - self.gain) * coeff;
                }
            }
        }

        let mix = self.mix.tick(device_rate);
        // The settled default -- no correction, all wet -- hands the
        // effect's own output straight back. `dry + (wet - dry) * 1.0` is
        // not bit-identical to `wet` in floating point, and every
        // bit-transparency test in this file depends on it being so.
        if self.gain == 1.0 && mix >= 1.0 {
            return wet;
        }
        let corrected = [wet[0] * self.gain, wet[1] * self.gain];
        [
            dry[0] + (corrected[0] - dry[0]) * mix,
            dry[1] + (corrected[1] - dry[1]) * mix,
        ]
    }

    /// Drop the measurement, so a slot does not carry one record's
    /// levels into the next.
    pub fn reset(&mut self) {
        self.in_env = 0.0;
        self.out_env = 0.0;
        self.gain = 1.0;
    }
}

pub(crate) const COMPRESSOR_THRESHOLD_MIN_DB: f32 = -40.0;
pub(crate) const COMPRESSOR_THRESHOLD_MAX_DB: f32 = 0.0;
pub(crate) const COMPRESSOR_THRESHOLD_DEFAULT_DB: f32 = -18.0;
pub(crate) const COMPRESSOR_RATIO_MIN: f32 = 1.0;
pub(crate) const COMPRESSOR_RATIO_MAX: f32 = 20.0;
pub(crate) const COMPRESSOR_RATIO_DEFAULT: f32 = 4.0;
/// How wide the bend around the threshold is, in decibels. A knee this
/// size is the difference between a compressor that grabs and one that
/// leans: material sitting near the threshold is eased into gain
/// reduction rather than switched into it.
const COMPRESSOR_KNEE_DB: f32 = 6.0;
/// How fast it takes hold, and how slowly it lets go. Fast enough to
/// catch a kick's front, slow enough that the release does not chew
/// through the bar behind it.
const COMPRESSOR_ATTACK_SECS: f32 = 0.005;
const COMPRESSOR_RELEASE_SECS: f32 = 0.15;

/// One deck's compressor: a soft-knee peak compressor with automatic
/// makeup.
///
/// The makeup is derived rather than knobbed. What a compressor gives
/// back is fixed by what it takes away -- the gain reduction at full
/// scale is exactly what the threshold and ratio say it is -- so a
/// makeup knob is a second control for the one number the first two
/// already decided, and getting it wrong is how a compressor becomes a
/// volume control by accident. This one lifts by the reduction the
/// loudest possible input would see, so pushing the ratio up makes the
/// quiet parts louder rather than making everything quieter.
///
/// Filter memory only -- two envelope followers, a few floats -- so it
/// takes the `reset` treatment the EQ and the phaser do rather than a
/// silence mark.
pub struct Compressor {
    wet: ParamRamp,
    threshold_db: ParamRamp,
    ratio: ParamRamp,
    /// The envelope it is riding, in decibels, and the gain reduction in
    /// force.
    env_db: f32,
    gain_db: f32,
    /// A cheap linear peak follower kept running even while bypassed, so
    /// that engaging starts the detector where the music actually IS.
    ///
    /// Without it the detector starts at silence, the reduction is zero
    /// for the length of the attack, and the makeup -- which is a fixed
    /// number the moment the threshold and ratio are known -- arrives
    /// alone. At a ratio of eight that is twenty-one decibels of boost
    /// landing before anything holds it back, which is not a click, it
    /// is a bang.
    idle_peak: f32,
    /// The attack and release coefficients, and the rate they were
    /// worked out for. Cached because they cost an `exp` each and
    /// nothing about them changes from frame to frame.
    attack_coeff: f32,
    release_coeff: f32,
    coeff_rate: f32,
}

/// Linear amplitude as decibels, floored so silence is a number.
#[inline]
fn amp_to_db(amp: f32) -> f32 {
    20.0 * amp.max(1e-6).log10()
}

/// The soft-knee curve: how many decibels of OUTPUT a given input level
/// earns, for a threshold, a ratio and the knee width above.
#[inline]
fn knee_curve(input_db: f32, threshold_db: f32, ratio: f32) -> f32 {
    let over = input_db - threshold_db;
    let half = COMPRESSOR_KNEE_DB * 0.5;
    if over <= -half {
        // Below the knee: untouched.
        input_db
    } else if over >= half {
        // Above it: the full ratio.
        threshold_db + over / ratio
    } else {
        // Inside it: a quadratic that meets both sides with the same
        // slope, so the curve has no corner to hear.
        let t = over + half;
        input_db + (1.0 / ratio - 1.0) * t * t / (2.0 * COMPRESSOR_KNEE_DB)
    }
}

impl Compressor {
    pub fn new() -> Compressor {
        Compressor {
            wet: ParamRamp::at(0.0),
            threshold_db: ParamRamp::at(COMPRESSOR_THRESHOLD_DEFAULT_DB),
            ratio: ParamRamp::at(COMPRESSOR_RATIO_DEFAULT),
            env_db: -120.0,
            gain_db: 0.0,
            idle_peak: 0.0,
            attack_coeff: 1.0,
            release_coeff: 1.0,
            coeff_rate: 0.0,
        }
    }

    /// The on/off switch.
    pub fn set_wet(&mut self, wet: f32) {
        if let Some(wet) = knob(wet, 0.0, 1.0) {
            if wet > 0.0 && self.wet.target() == 0.0 {
                // Start the detector where the music is, not at silence.
                self.env_db = amp_to_db(self.idle_peak);
            }
            self.wet.slew(wet, EQ_ENGAGE_SECS);
        }
    }

    /// Where it starts working, in decibels below full scale.
    pub fn set_threshold_db(&mut self, db: f32) {
        if let Some(db) = knob(db, COMPRESSOR_THRESHOLD_MIN_DB, COMPRESSOR_THRESHOLD_MAX_DB) {
            self.threshold_db.slew(db, EQ_ENGAGE_SECS);
        }
    }

    /// How hard it works above that.
    pub fn set_ratio(&mut self, ratio: f32) {
        if let Some(ratio) = knob(ratio, COMPRESSOR_RATIO_MIN, COMPRESSOR_RATIO_MAX) {
            self.ratio.slew(ratio, EQ_ENGAGE_SECS);
        }
    }

    pub fn engaged(&self) -> bool {
        self.wet.target() > 0.0
    }

    /// Drop the envelope, so one record's peaks do not ride the start of
    /// the next.
    pub fn reset(&mut self) {
        self.env_db = -120.0;
        self.gain_db = 0.0;
        self.idle_peak = 0.0;
    }

    /// The gain reduction in force, in decibels, for a meter.
    pub fn reduction_db(&self) -> f32 {
        self.gain_db
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        if (self.coeff_rate - device_rate).abs() > 0.5 {
            self.coeff_rate = device_rate;
            let rate = device_rate.max(1.0);
            self.attack_coeff = 1.0 - (-1.0 / (COMPRESSOR_ATTACK_SECS * rate)).exp();
            self.release_coeff = 1.0 - (-1.0 / (COMPRESSOR_RELEASE_SECS * rate)).exp();
        }
        // Both channels are detected together, or a loud left would duck
        // only itself and walk the image about.
        let peak = frame[0].abs().max(frame[1].abs());
        // Kept warm even while bypassed, and cheaply -- no logarithm on a
        // path that is not doing anything.
        let idle_coeff = match peak > self.idle_peak {
            true => self.attack_coeff,
            false => self.release_coeff,
        };
        self.idle_peak += (peak - self.idle_peak) * idle_coeff;

        let wet = self.wet.tick(device_rate);
        if wet <= 0.0 {
            // Two envelope followers and no line: off is exactly the
            // input, with no tail that could still be ringing.
            return frame;
        }
        let threshold_db = self.threshold_db.tick(device_rate);
        let ratio = self.ratio.tick(device_rate).max(1.0);

        let peak_db = amp_to_db(peak);
        // Attack and release on the DETECTOR, both one-poles. Rising
        // takes the attack, falling the release.
        let coeff = match peak_db > self.env_db {
            true => self.attack_coeff,
            false => self.release_coeff,
        };
        self.env_db += (peak_db - self.env_db) * coeff;

        self.gain_db = knee_curve(self.env_db, threshold_db, ratio) - self.env_db;
        // The makeup: what full scale itself would lose. Derived, not
        // knobbed -- see the note on the struct.
        let makeup_db = -(knee_curve(0.0, threshold_db, ratio));
        let gain = 10f32.powf((self.gain_db + makeup_db) / 20.0);

        let squeezed = [frame[0] * gain, frame[1] * gain];
        [
            frame[0] + (squeezed[0] - frame[0]) * wet,
            frame[1] + (squeezed[1] - frame[1]) * wet,
        ]
    }
}

// ---------------------------------------------------------------------------
// the master limiter
// ---------------------------------------------------------------------------

/// How far ahead the limiter looks. Long enough to be fully ducked by
/// the time a transient arrives rather than ducking on top of it, short
/// enough that the delay it costs the whole output is well under
/// anything an operator would feel as latency.
pub const LIMITER_LOOKAHEAD_SECS: f32 = 0.003;

/// The look-ahead in frames at a given rate: the delay a bus running a
/// [`Limiter`] pays. One function, used by the limiter to size its window
/// and by anything lining the output up against what went into it, so
/// the two cannot disagree.
pub fn limiter_latency_frames(sample_rate: f32) -> usize {
    if !(sample_rate > 0.0) {
        return 1;
    }
    ((LIMITER_LOOKAHEAD_SECS * sample_rate).round() as usize).clamp(1, LIMITER_MAX_FRAMES - 1)
}
/// How long it takes to give the gain back once the loud passage has
/// gone. Slow enough not to pump on a kick, quick enough that one stab
/// does not duck the next bar.
const LIMITER_RELEASE_SECS: f32 = 0.12;
/// The most the output may reach.
///
/// Full scale, deliberately, and not the fraction under it a mastering
/// limiter would take. This replaces a hard clamp at exactly this value,
/// and the contract that clamp kept -- everything below it comes through
/// untouched -- is worth keeping: a lower ceiling would quietly start
/// shaving material that has been passing cleanly for the life of the
/// app, and the tests that pin an untouched deck as transparent would
/// all have to be loosened to allow it. What changes here is only what
/// used to CLIP.
///
/// The usual argument for a lower ceiling is the inter-sample peak: a
/// converter reconstructing a curve through samples that each sit at
/// full scale can overshoot between them. That is real, and a reason to
/// revisit this, but it is a different job from replacing a clipper and
/// doing both at once would make neither possible to judge.
pub const LIMITER_CEILING: f32 = 1.0;
/// The line is sized once, for the highest rate this could run at, and
/// the live look-ahead is a window inside it.
const LIMITER_MAX_FRAMES: usize = 2048;
/// How much of the look-ahead the attack ramp actually spends. Arriving
/// exactly as the peak does leaves the samples just BEFORE it under a
/// gain that has not quite finished falling -- a hair over the ceiling,
/// measured at about a thousandth. Arriving halfway through means every
/// sample in the second half of the window is already fully covered,
/// and each sample in the first half has set its own target on the way
/// in, so nothing is left leaning on a ramp that is still moving.
const LIMITER_ATTACK_SHARE: f32 = 0.5;

/// The master bus's limiter: a look-ahead peak limiter in place of the
/// hard clamp the sum used to end on.
///
/// A clamp is a clipper. Pushed past full scale it flat-tops every
/// sample that got there, which is a square wave's worth of harmonics
/// and audible as grit rather than as loudness -- and it did happen: the
/// distortion effect's own golden reference sat with its peaks pinned at
/// full scale for months.
///
/// The attack is what makes this a limiter rather than a faster clamp.
/// It is a LINEAR ramp that arrives in exactly the look-ahead window, so
/// the gain that a peak needs is already in force by the time that peak
/// reaches the output -- the output is the delayed signal, and the peak
/// was seen when it went in. Nothing is ever clamped on the way out
/// because nothing ever gets there too loud.
pub struct Limiter {
    line: Box<[[f32; 2]]>,
    write: usize,
    /// The live look-ahead, in frames, for the rate in force.
    len: usize,
    /// The rate `len` was worked out for.
    rate: f32,
    /// Where the output is held, as a multiplier of full scale. Full scale
    /// itself by default, because the first thing this replaced was a
    /// clamp at exactly that; a bus that wants margin for inter-sample
    /// peaks sets it lower.
    ceiling: f32,
    /// The gain in force, one being none at all.
    gain: f32,
    /// Where the attack ramp is heading, where it set off from, and how
    /// many frames of it are left.
    ///
    /// Counted rather than accumulated, and that is not a style choice.
    /// A sustained tone nudges the target down by a hair every cycle, so
    /// the ramp is forever restarting with a microscopic step -- and a
    /// step of 1.7e-9 subtracted from a gain of 0.16 is a no-op in f32,
    /// whose ulp there is ten times larger. The gain then never reaches
    /// its target, never leaves the attack, never reaches the release,
    /// and the limiter stays ducked for the rest of the set. Interpolating
    /// from a remembered start cannot stall: when the count runs out the
    /// gain IS the target, whatever the arithmetic did on the way.
    target: f32,
    from: f32,
    attack_left: usize,
    attack_total: usize,
    /// Frames left before the gain may start climbing again. Without
    /// this the gain reaches its target and immediately begins
    /// releasing -- while the peak that asked for it is still in the
    /// line, a whole look-ahead away from the output -- and arrives a
    /// fraction of a decibel too high. Small, but the ceiling is a
    /// guarantee or it is nothing.
    hold: usize,
    /// The deepest reduction since the meter last read it.
    worst: f32,
}

impl Limiter {
    pub fn new(sample_rate: f32) -> Limiter {
        let mut limiter = Limiter {
            line: vec![[0.0f32; 2]; LIMITER_MAX_FRAMES].into_boxed_slice(),
            write: 0,
            len: 1,
            rate: 0.0,
            ceiling: LIMITER_CEILING,
            gain: 1.0,
            target: 1.0,
            from: 1.0,
            attack_left: 0,
            attack_total: 1,
            hold: 0,
            worst: 1.0,
        };
        limiter.set_sample_rate(sample_rate);
        limiter
    }

    /// Re-window the look-ahead for a new device rate. Cheap and
    /// allocation-free: the line is already as long as it will ever need
    /// to be, and only the window inside it moves.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if (self.rate - sample_rate).abs() < 0.5 || !(sample_rate > 0.0) {
            return;
        }
        self.rate = sample_rate;
        self.len = limiter_latency_frames(sample_rate);
    }

    /// Where to hold the output, as a multiplier of full scale. Refused
    /// above full scale -- a limiter that lets more than that through is
    /// not one -- and below a hundredth, where it would be a mute.
    pub fn set_ceiling(&mut self, ceiling: f32) {
        if ceiling.is_finite() {
            self.ceiling = ceiling.clamp(0.01, LIMITER_CEILING);
        }
    }

    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }

    /// The gain in force right now, for a meter that wants the block's
    /// worst without clearing it.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Forget what the line was carrying, so one set's peaks cannot duck
    /// the start of the next.
    pub fn reset(&mut self) {
        self.line.iter_mut().for_each(|frame| *frame = [0.0; 2]);
        self.write = 0;
        self.gain = 1.0;
        self.target = 1.0;
        self.from = 1.0;
        self.attack_left = 0;
        self.hold = 0;
        self.worst = 1.0;
    }

    /// The deepest gain reduction since this was last called, as a
    /// multiplier -- one meaning the limiter never had to do anything.
    /// Reading it clears it, so the meter shows the period it covers.
    pub fn worst_reduction(&mut self) -> f32 {
        std::mem::replace(&mut self.worst, 1.0)
    }

    /// The look-ahead in frames, which is the delay the bus is paying.
    pub fn latency_frames(&self) -> usize {
        self.len
    }

    /// Process one stereo frame, returning the frame from `len` ago with
    /// whatever gain that frame turned out to need.
    #[inline]
    pub fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        self.line[self.write] = frame;
        let read = (self.write + LIMITER_MAX_FRAMES - self.len) % LIMITER_MAX_FRAMES;
        let delayed = self.line[read];
        self.write = (self.write + 1) % LIMITER_MAX_FRAMES;

        // Both channels take one gain: ducking them apart would walk the
        // stereo image around under a loud passage.
        let peak = frame[0].abs().max(frame[1].abs());
        if peak > self.ceiling {
            let needed = self.ceiling / peak;
            if needed < self.target {
                // Arrive before the sample that asked for it does.
                self.from = self.gain;
                self.target = needed;
                self.attack_total =
                    ((self.len as f32 * LIMITER_ATTACK_SHARE) as usize).max(1);
                self.attack_left = self.attack_total;
            }
            // Anything over the ceiling re-arms the hold, so the gain
            // cannot start climbing while that sample is still in the
            // line waiting to come out.
            self.hold = self.len;
        }

        if self.attack_left > 0 {
            self.attack_left -= 1;
            let left = self.attack_left as f32 / self.attack_total as f32;
            self.gain = self.target + (self.from - self.target) * left;
        } else if self.hold > 0 {
            // Landed, and holding until the loudest thing that asked for
            // this gain has been through the output.
            self.hold -= 1;
        } else {
            // Released rather than attacking: let the target go first, so
            // the next peak is measured against an honest gain.
            self.target = 1.0;
            let coeff = (1.0 / (LIMITER_RELEASE_SECS * self.rate.max(1.0))).min(1.0);
            self.gain += (1.0 - self.gain) * coeff;
        }
        self.worst = self.worst.min(self.gain);

        [delayed[0] * self.gain, delayed[1] * self.gain]
    }
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
    use std::f32::consts::PI;

    /// Settle every ramp `DeckEcho` carries -- send, feedback, ping-pong
    /// -- and any handover a `prepare_block` just armed, before a test
    /// measures anything against silence.
    const SETTLE_FRAMES: usize = 2_000;

    fn settled_echo(rate: f32, fraction: (u32, u32), feedback: f32, beat_frames: f64) -> DeckEcho {
        let mut echo = DeckEcho::new();
        echo.set_fraction(Some(fraction));
        echo.set_feedback(feedback);
        echo.prepare_block(beat_frames);
        for _ in 0..SETTLE_FRAMES {
            echo.process([0.0, 0.0], rate);
        }
        echo
    }

    /// Odd, bounded by 1, flat through the origin, and close to the real
    /// thing near it -- everything the feedback clamp actually needs.
    #[test]
    fn the_soft_clip_is_odd_bounded_and_flat_at_the_origin() {
        assert_eq!(pade_tanh(0.0), 0.0);
        for x in [0.3f32, 1.0, 2.5, 7.0] {
            assert!((pade_tanh(-x) + pade_tanh(x)).abs() < 1e-6, "odd at {x}");
        }
        let mut i = -400;
        while i <= 400 {
            let x = i as f32 * 0.25;
            assert!(pade_tanh(x).abs() <= 1.0, "over 1 at {x}");
            i += 1;
        }
        assert!((pade_tanh(0.1) - 0.1f32.tanh()).abs() < 1e-4);
        assert!((pade_tanh(1.0) - 1.0f32.tanh()).abs() < 2e-2);
        assert_eq!(pade_tanh(3.0), 1.0);
        assert_eq!(pade_tanh(50.0), 1.0);
        let mut last = pade_tanh(-4.0);
        let mut i = -399;
        while i <= 400 {
            let x = i as f32 * 0.01;
            let v = pade_tanh(x);
            assert!(v >= last - 1e-7, "not monotone at {x}: {v} < {last}");
            last = v;
            i += 1;
        }
        assert_eq!(pade_tanh(f32::NAN), 0.0);
        assert_eq!(pade_tanh(f32::INFINITY), 1.0, "saturates like any large input");
    }

    /// Off, a fresh unit is exactly its input -- the whole line untouched.
    #[test]
    fn an_echo_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut echo = DeckEcho::new();
        echo.prepare_block(24_000.0);
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(echo.process([x, -x], rate), [x, -x]);
        }
    }

    /// A half beat later, the sent input comes back once; with feedback
    /// on, it comes back a second time, softened by the clamp.
    #[test]
    fn a_half_beat_echo_repeats_the_input_that_much_later() {
        let rate = 48_000.0f32;
        let mut dry = settled_echo(rate, (1, 2), 0.0, 24_000.0);
        assert_eq!(dry.delay(), 12_000);
        let mut out = vec![dry.process([1.0, 1.0], rate)[0]];
        for _ in 1..40_000 {
            out.push(dry.process([0.0, 0.0], rate)[0]);
        }
        assert_eq!(out[0], 1.0, "the dry input passes through untouched");
        assert!((out[12_000] - ECHO_SEND).abs() < 1e-6, "{}", out[12_000]);
        for (index, &value) in out.iter().enumerate() {
            if index == 0 || index == 12_000 {
                continue;
            }
            assert!(value.abs() < 1e-6, "unexpected sound at {index}: {value}");
        }

        let mut wet = settled_echo(rate, (1, 2), 0.5, 24_000.0);
        let mut out = vec![wet.process([1.0, 1.0], rate)[0]];
        for _ in 1..30_000 {
            out.push(wet.process([0.0, 0.0], rate)[0]);
        }
        let expect = pade_tanh(ECHO_SEND * 0.5);
        assert!((out[24_000] - expect).abs() < 1e-6, "{} vs {expect}", out[24_000]);
    }

    /// A retune crossfades over ECHO_HANDOVER_FRAMES rather than jumping,
    /// and the tap really does move once it lands.
    #[test]
    fn a_delay_change_hands_over_instead_of_jumping() {
        let rate = 48_000.0f32;
        let mut echo = settled_echo(rate, (1, 1), 0.5, 24_000.0);
        assert_eq!(echo.delay(), 24_000);
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(12_000);
        let mut render = |echo: &mut DeckEcho, frames: usize, out: &mut Vec<f32>, phase: &mut f32| {
            for _ in 0..frames {
                *phase += 2.0 * PI * 37.0 / rate;
                out.push(echo.process([phase.sin() * 0.5, phase.sin() * 0.5], rate)[0]);
            }
        };
        render(&mut echo, 8_000, &mut out, &mut phase);
        let settled = out.len();
        // A jump that shares no whole number of the tone's cycles with
        // the old delay, so a hard switch would show up as a step.
        echo.prepare_block(18_500.0);
        render(&mut echo, 4_096, &mut out, &mut phase);
        let worst = out[settled - 1..]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.02, "a retune stepped by {worst}");
        render(&mut echo, ECHO_HANDOVER_FRAMES as usize, &mut out, &mut phase);
        assert_eq!(echo.delay(), 18_500, "and the handover actually lands on the new tap");
    }

    /// A second retune landing while the first is still crossfading is
    /// parked, not raced: the running handover finishes on its own target
    /// before the parked one gets its turn.
    #[test]
    fn a_retune_that_arrives_mid_handover_waits_its_turn() {
        let rate = 48_000.0f32;
        let mut echo = DeckEcho::new();
        echo.set_fraction(Some((1, 1)));
        echo.prepare_block(24_000.0);
        assert_eq!(echo.delay(), 24_000, "the first target lands at once, nothing was running");
        for _ in 0..50 {
            echo.process([0.0, 0.0], rate);
        }
        echo.prepare_block(19_200.0);
        assert_eq!(echo.delay(), 24_000, "a running handover finishes first");
        assert_eq!(echo.pending(), Some(19_200));
        for _ in 0..(ECHO_HANDOVER_FRAMES - 50) {
            echo.process([0.0, 0.0], rate);
        }
        assert_eq!(echo.pending(), None, "the parked target has been taken up");
        assert_eq!(echo.delay(), 19_200);
        assert!(echo.handover() > 0, "and hands over in its own turn rather than jumping");
        for _ in 0..ECHO_HANDOVER_FRAMES {
            echo.process([0.0, 0.0], rate);
        }
        assert_eq!(echo.handover(), 0);
        assert_eq!(echo.delay(), 19_200);
    }

    /// Cranked past any sane setting, the feedback clamp keeps the line
    /// bounded rather than climbing without end.
    #[test]
    fn cranked_feedback_saturates_instead_of_running_away() {
        let rate = 48_000.0f32;
        let mut echo = DeckEcho::new();
        echo.set_fraction(Some((1, 1)));
        echo.set_feedback(4.0);
        echo.prepare_block(3_000.0);
        for _ in 0..SETTLE_FRAMES {
            echo.process([0.0, 0.0], rate);
        }
        let mut out = Vec::with_capacity(480_000);
        for i in 0..480_000usize {
            let x = if i % 3_000 == 0 { 1.0 } else { 0.0 };
            out.push(echo.process([x, x], rate)[0]);
        }
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 3.0), "unbounded output");
        let rms = |span: &[f32]| (span.iter().map(|v| v * v).sum::<f32>() / span.len() as f32).sqrt();
        let last = rms(&out[out.len() - 48_000..]);
        let before = rms(&out[out.len() - 96_000..out.len() - 48_000]);
        assert!(last <= before + 1e-6, "{last} grew past {before}");
    }

    /// Each repeat lands on the OTHER channel from the one before it, a
    /// hand-traced three-repeat chain from a left-only impulse.
    #[test]
    fn ping_pong_lands_each_repeat_on_the_other_side() {
        let rate = 48_000.0f32;
        let mut echo = DeckEcho::new();
        echo.set_fraction(Some((1, 1)));
        echo.set_feedback(0.5);
        echo.set_pingpong(true);
        echo.prepare_block(6_000.0);
        for _ in 0..SETTLE_FRAMES {
            echo.process([0.0, 0.0], rate);
        }
        let mut out_l = vec![echo.process([1.0, 0.0], rate)[0]];
        let mut out_r = vec![0.0f32];
        for _ in 1..19_000 {
            let frame = echo.process([0.0, 0.0], rate);
            out_l.push(frame[0]);
            out_r.push(frame[1]);
        }
        assert!(out_r[6_000].abs() < 1e-6, "{}", out_r[6_000]);
        assert!(out_l[6_000] > 0.4, "the first repeat is the sent input, on its own side");
        assert!(out_r[12_000] > 0.1, "the second crossed");
        assert!(out_l[12_000].abs() < 1e-6, "{}", out_l[12_000]);
        assert!(out_l[18_000] > 0.02, "the third crossed back");
        assert!(out_r[18_000].abs() < 1e-6, "{}", out_r[18_000]);
    }

    /// Switching off does not cut the tail; it rings out, and only once
    /// it has genuinely decayed does the unit go back to a bit-exact
    /// bypass.
    #[test]
    fn switching_the_echo_off_lets_the_tail_ring_out_then_goes_transparent() {
        let rate = 48_000.0f32;
        let mut echo = settled_echo(rate, (1, 1), 0.5, 6_000.0);
        let mut out = vec![echo.process([1.0, 1.0], rate)[0]];
        for i in 1..400_000usize {
            if i == 100 {
                echo.set_fraction(None);
            }
            out.push(echo.process([0.0, 0.0], rate)[0]);
        }
        assert!(!echo.engaged(), "the operator's ask flips at once");
        assert!(out[6_000] > 0.4, "the tail still arrives after the switch: {}", out[6_000]);
        assert!(out[12_000] > 0.1, "{}", out[12_000]);
        let tail = &out[out.len() - 1_000..];
        assert!(tail.iter().all(|&v| v == 0.0), "silent, once the tail has actually decayed");
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(echo.process([x, -x], rate), [x, -x], "a bit-exact bypass, not merely quiet");
        }
    }

    /// `silence`, unlike `reset`, is safe from the audio thread precisely
    /// because it never writes to the line: a memset over 2 MiB inside
    /// the callback is the one thing this type exists to avoid. Prove it
    /// two ways -- the raw memory is untouched, and yet nothing before
    /// the mark can be read back.
    #[test]
    fn silence_never_touches_the_line_only_the_reach_of_it() {
        let rate = 48_000.0f32;
        let mut echo = settled_echo(rate, (1, 1), 0.0, 6_000.0);
        echo.process([0.8, -0.8], rate);
        let write_before = echo.raw_write();
        let raw_before = echo.raw_at(1);
        assert!(raw_before[0].abs() > 0.1, "a real value is there to protect: {raw_before:?}");
        echo.silence();
        assert_eq!(echo.raw_write(), write_before, "silence must not move the write cursor");
        assert_eq!(echo.raw_at(1), raw_before, "and must not have memset the line either");
        // The setting the operator asked for is the strip's and stands.
        assert!(echo.engaged());
        assert_eq!(echo.fraction(), Some((1, 1)));
        // But the record changed underneath it: for as long as the delay
        // would still be looking at the old content, nothing comes back.
        for _ in 0..5_000 {
            assert_eq!(echo.process([0.0, 0.0], rate), [0.0, 0.0], "stale content must not resurface");
        }
    }

    /// Off, a fresh unit is exactly the sample it was handed, over and
    /// over -- the whole point, since every golden reference depends on
    /// this stage being invisible when nothing is held.
    #[test]
    fn a_freeze_at_rest_is_the_sample_it_was_given() {
        let rate = 48_000.0f32;
        let mut freeze = Freeze::new();
        assert!(!freeze.held());
        for i in 0..4_000 {
            let x = (i as f32 * 0.037).sin() * 0.6;
            let frame = [x, -x];
            assert_eq!(freeze.process(frame, rate), frame);
        }
        assert!(!freeze.held());
    }

    /// A hold pressed before enough has genuinely been written since the
    /// ring last resumed is refused outright, rather than reaching past
    /// what `fresh` can vouch for and reading whatever the ring was
    /// carrying from a previous life -- a previous record, or the tail
    /// of a hold that only just let go. `reset` deliberately never
    /// touches the ring (that would be the bulk clear the echo commit's
    /// own review found unsafe on the audio thread), so old content is
    /// genuinely still sitting there to be misread if the clamp does
    /// not refuse in time.
    #[test]
    fn a_hold_pressed_too_soon_after_a_reset_is_refused_rather_than_reaching_into_stale_content() {
        let rate = 48_000.0f32;
        let xf = 480usize; // FREEZE_XFADE_SECS * 48 kHz
        let mut freeze = Freeze::new();
        // Old, distinctive content: what a previous life of this ring
        // left behind.
        for _ in 0..5_000 {
            freeze.process([-0.9, -0.9], rate);
        }
        freeze.reset();
        // Nothing written since: the ring is entirely old content.
        freeze.hold(100, rate);
        assert!(!freeze.held(), "no fresh content at all -- must refuse");
        // Still short of 2*xf frames of fresh content.
        for _ in 0..900 {
            assert_eq!(freeze.process([0.9, 0.9], rate), [0.9, 0.9], "still the bypass");
        }
        freeze.hold(100, rate);
        assert!(!freeze.held(), "still short of 2*xf frames since the reset");
        // Now exactly enough: fresh reaches 3*xf, leaving `available`
        // (fresh less the seam's own pre-roll) at exactly 2*xf.
        for _ in 0..(3 * xf - 900) {
            freeze.process([0.9, 0.9], rate);
        }
        freeze.hold(100, rate); // asks for far less; the floor takes over
        assert!(freeze.held(), "2*xf of certified content is exactly enough");
        let mut settled = false;
        for i in 0..1_000 {
            let out = freeze.process([0.0, 0.0], rate);
            assert!(out[0] > -1e-6, "the stale -0.9 must never surface: {out:?} at {i}");
            if i > 400 {
                assert!(out[0] > 0.5, "and once settled it reads the real +0.9: {out:?} at {i}");
                settled = true;
            }
        }
        assert!(settled);
    }

    /// Once the wet ramp has settled, a held lap repeats sample for
    /// sample, lap after lap, and (outside the seam it deliberately
    /// reshapes) reads the exact frames it was given to repeat.
    #[test]
    fn a_held_freeze_repeats_the_beat_it_just_played() {
        let rate = 48_000.0f32;
        let mut freeze = Freeze::new();
        let written: Vec<f32> = (0..40_000).map(|n| n as f32 / 1_000_000.0).collect();
        for &x in &written {
            freeze.process([x, x], rate);
        }
        freeze.hold(24_000, rate);
        let lap = |freeze: &mut Freeze| -> Vec<f32> {
            (0..24_000).map(|_| freeze.process([0.0, 0.0], rate)[0]).collect()
        };
        let lap1 = lap(&mut freeze);
        let lap2 = lap(&mut freeze);
        let lap3 = lap(&mut freeze);
        let xf = 480usize;
        let start = 40_000 - 24_000; // hold()'s own math: write - len
        for k in 0..24_000 - xf {
            assert_eq!(lap2[k], lap3[k], "steady state repeats sample for sample at {k}");
        }
        // Outside the wet ramp's own 384-frame settle and outside the
        // seam it reshapes, even the FIRST lap already reads the ring
        // directly.
        for k in 384..24_000 - xf {
            let expected = written[start + k];
            assert!((lap1[k] - expected).abs() < 1e-6, "at {k}: {} vs {expected}", lap1[k]);
        }
    }

    /// The last `xf` frames of a lap blend its own tail into its own
    /// head -- the loop wrap's own crossfade, so the lap lands on
    /// itself in phase instead of splicing. Pin the formula directly,
    /// and pin that the whole thing stays click-free across a wrap.
    #[test]
    fn the_seam_of_a_frozen_lap_is_a_blend_into_its_own_head() {
        let rate = 48_000.0f32;
        let mut freeze = Freeze::new();
        let mut phase = 0.0f32;
        // 443 Hz over a 24 000-frame lap is 221.5 cycles -- not a whole
        // number, so an unblended seam would genuinely step; kept at
        // 0.2 scale so the TONE's own slope stays well under the click
        // rule and any step measured is the seam's, not the signal's.
        let mut written = Vec::with_capacity(40_000);
        for _ in 0..40_000 {
            phase += 2.0 * std::f32::consts::PI * 443.0 / rate;
            let x = phase.sin() * 0.2;
            written.push(x);
            freeze.process([x, x], rate);
        }
        freeze.hold(24_000, rate);
        let mut out = Vec::with_capacity(2 * 24_000);
        for _ in 0..2 * 24_000 {
            out.push(freeze.process([0.0, 0.0], rate)[0]);
        }
        let worst = out[384..].windows(2).map(|p| (p[1] - p[0]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.02, "the seam (and the wrap into a second lap) must not click: {worst}");
        let xf = 480usize;
        let len = 24_000usize;
        let start = 40_000 - len; // 16 000
        // The blend is a PRE-ROLL, not the lap's own head: it reads from
        // just BEFORE `start` (the same shape the loop wrap's own
        // crossfade takes in mixer.rs), walking up to it as u grows, so
        // out[len - xf + u] == lerp(ring[start + len - xf + u], ring[start - xf + u], u / xf).
        for u in 0..xf {
            let tail = written[start + len - xf + u];
            let head = written[start - xf + u];
            let t = u as f32 / xf as f32;
            let expected = tail + (head - tail) * t;
            let got = out[len - xf + u];
            assert!((got - expected).abs() < 1e-6, "at u={u}: {got} vs {expected}");
        }
    }

    /// Press and release both ramp rather than switch, and once release
    /// has settled the output is exactly the live signal again. A tone
    /// stands in for the record continuing to play underneath the
    /// hold -- DC would make the frozen and the live sample identical
    /// and hide a switch pretending to be a ramp.
    #[test]
    fn a_freeze_comes_in_and_goes_out_on_a_ramp() {
        let rate = 48_000.0f32;
        let mut freeze = Freeze::new();
        let mut phase = 0.0f32;
        let mut tone = move || {
            phase += 2.0 * std::f32::consts::PI * 5.0 / rate;
            phase.sin() * 0.5
        };
        let mut out = Vec::with_capacity(24_000 + 2_000);
        for _ in 0..24_000 {
            let x = tone();
            out.push(freeze.process([x, x], rate)[0]);
        }
        assert!(!freeze.held());
        freeze.hold(12_000, rate);
        assert!(freeze.held());
        for _ in 0..800 {
            let x = tone(); // the record keeps running underneath
            out.push(freeze.process([x, x], rate)[0]);
        }
        freeze.release();
        for _ in 0..800 {
            let x = tone();
            out.push(freeze.process([x, x], rate)[0]);
        }
        let worst = out.windows(2).map(|p| (p[1] - p[0]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.02, "press and release must ramp, biggest step {worst}");
        assert!(!freeze.held(), "the release ramp has settled by now");
        for _ in 0..100 {
            let x = tone();
            assert_eq!(freeze.process([x, x], rate), [x, x], "exactly live again, bit-exact");
        }
    }

    /// A re-press landing while the release tail is still crossfading
    /// out re-latches onto the lap and heads back up, rather than being
    /// dropped the way a second press onto an ALREADY-sounding one is:
    /// the tail has not finished, so `held` is still `Some`, and a
    /// naive "already held, ignore" rule would silently swallow the
    /// re-press and leave the chip lit on a freeze that has actually
    /// gone quiet.
    #[test]
    fn a_re_press_during_the_release_tail_re_latches_rather_than_dropping() {
        let rate = 48_000.0f32;
        let mut freeze = Freeze::new();
        for n in 0..24_000 {
            let x = n as f32 / 24_000.0;
            freeze.process([x, x], rate);
        }
        freeze.hold(12_000, rate);
        for _ in 0..500 {
            freeze.process([-9.0, -9.0], rate);
        }
        freeze.release();
        // Only a little way into the release tail: well short of
        // FREEZE_BLEND_SECS's 384-frame settle.
        for _ in 0..50 {
            freeze.process([-9.0, -9.0], rate);
        }
        assert!(freeze.held(), "the tail has not settled yet");
        freeze.hold(12_000, rate); // the re-press
        // Dropped, this would go on decaying toward -9.0 and settle
        // there well inside 2 000 more frames; re-latched, it climbs
        // back toward the lap and keeps sounding.
        for _ in 0..2_000 {
            freeze.process([-9.0, -9.0], rate);
        }
        assert!(freeze.held(), "the re-press kept it sounding rather than letting it settle");
    }

    /// The resonance a sweep is allowed, and where it is taken away:
    /// at both ends of hearing, whichever side of the sweep put the
    /// corner there.
    /// The readout and the coefficients come from one function: the
    /// corner it names is the corner the first sweep section is built at.
    #[test]
    fn the_readout_corner_is_the_filter_corner() {
        assert_eq!(filter_corner_hz(0.5), None);
        assert_eq!(filter_corner_hz(0.52), None, "inside the dead zone");
        assert_eq!(filter_corner_hz(0.0), Some((false, 40.0)));
        assert_eq!(filter_corner_hz(1.0), Some((true, 9000.0)));
        let mut last = f32::INFINITY;
        for i in 0..=47 {
            let (highpass, hz) = filter_corner_hz(0.47 - i as f32 * 0.01).unwrap();
            assert!(!highpass && hz <= last, "the low-pass falls as the knob turns left");
            last = hz;
        }
        let mut last = 0.0;
        for i in 0..=47 {
            let (highpass, hz) = filter_corner_hz(0.53 + i as f32 * 0.01).unwrap();
            assert!(highpass && hz >= last, "the high-pass rises as the knob turns right");
            last = hz;
        }
        // A filter change hands over rather than replacing the running
        // coefficients on the spot (the click-free crossfade above), so
        // each corner is checked on its own fresh EQ instead of by
        // reusing one across a jump the handover would otherwise stall.
        let mut eq = DeckEq::new(48_000.0);
        eq.set_filter(0.3);
        eq.prepare_block();
        let (highpass, hz) = filter_corner_hz(0.3).unwrap();
        assert!(!highpass);
        let q = BUTTERWORTH_Q4[0] * eq.resonance.min(resonance_ceiling(hz));
        assert_eq!(eq.coeffs.sweep[0], Biquad::lowpass(hz, 48_000.0, q));
        let mut eq = DeckEq::new(48_000.0);
        eq.set_filter(0.8);
        eq.prepare_block();
        let (highpass, hz) = filter_corner_hz(0.8).unwrap();
        assert!(highpass);
        let q = BUTTERWORTH_Q4[0] * eq.resonance.min(resonance_ceiling(hz));
        assert_eq!(eq.coeffs.sweep[0], Biquad::highpass(hz, 48_000.0, q));
    }

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

    /// A handover armed just before the chain went quiet used to sit
    /// frozen: `process` returns before its own countdown while `wet` is
    /// zero, and `prepare_block` refuses to rebuild while a handover is
    /// running, so the first sweep after the filter came back played at
    /// the OLD position until the stale counter drained.
    #[test]
    fn a_sweep_after_a_dry_spell_rebuilds_at_once() {
        let rate = 48_000.0;
        let mut eq = DeckEq::new(rate);
        eq.set_sample_rate(rate);

        // Engage and sweep: this arms a handover.
        eq.set_filter(0.2);
        eq.prepare_block();
        for _ in 0..64 {
            eq.process([0.1, 0.1], rate);
        }
        assert!(eq.handover > 0, "a sweep should arm a handover");

        // Back to centre: the chain goes dry and `process` stops
        // counting the handover down.
        eq.set_filter(0.5);
        for _ in 0..8 {
            eq.prepare_block();
            for _ in 0..512 {
                eq.process([0.1, 0.1], rate);
            }
        }
        let dry = eq.wet.current();

        // A fresh sweep, the other way. It must take effect on this
        // block, not once a counter nobody is decrementing runs out.
        eq.set_filter(0.8);
        eq.prepare_block();
        assert_eq!(
            eq.filter_built, 0.8,
            "the sweep did not rebuild: wet {dry} handover {} filter_built {}",
            eq.handover, eq.filter_built
        );
    }

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
        let mut echo = DeckEcho::new();
        echo.set_fraction(Some((1, 2)));
        echo.set_feedback(0.4);
        let mut freeze = Freeze::new();
        let mut reader = RateReader::default();
        let mut scratch = ScratchRamp::default();
        // Warm the chain up so nothing lazily initializes inside the probe.
        eq.prepare_block();
        echo.prepare_block(24_000.0);
        for _ in 0..8_000 {
            let mut pull = || stretcher.next(&source, true);
            if let Some(frame) = reader.read(0.9188, &mut pull) {
                echo.process(freeze.process(eq.process(frame, 48_000.0), 48_000.0), 48_000.0);
            }
            scratch.tick(48_000.0, 1.0, 0.0);
        }
        freeze.hold(6_000, 48_000.0);

        let before = alloc_probe::count();
        // A retune every 512 frames, so the probe exercises the handover
        // and the parked-retune path too, not only the settled one; and a
        // release plus a second hold, so the freeze's own idle write,
        // held lap and both transitions are all inside the counted second.
        let mut beat = 24_000.0f64;
        for i in 0..48_000 {
            if i % 512 == 0 {
                beat = if beat > 20_000.0 { 15_000.0 } else { 24_000.0 };
                echo.prepare_block(beat);
            }
            if i == 20_000 {
                freeze.release();
            }
            if i == 24_000 {
                freeze.hold(6_000, 48_000.0);
            }
            let mut pull = || stretcher.next(&source, true);
            if let Some(frame) = reader.read(0.9188, &mut pull) {
                echo.process(freeze.process(eq.process(frame, 48_000.0), 48_000.0), 48_000.0);
            }
            scratch.tick(48_000.0, 1.0, 0.0);
        }
        eq.prepare_block();
        echo.prepare_block(beat);
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

    /// Off, a fresh unit is exactly its input -- the whole line untouched,
    /// same requirement as the echo's and for the same reason: every
    /// golden reference depends on this stage being invisible unengaged.
    #[test]
    fn a_flanger_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut flanger = Flanger::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(flanger.process([x, -x], rate), [x, -x]);
        }
    }

    /// Engaged, an impulse's reflection lands within the documented sweep
    /// range (centre +/- the full depth) and nowhere else -- the tap
    /// really is bounded to what the manifest promises, not merely
    /// "somewhere".
    #[test]
    fn an_engaged_flanger_reflects_within_its_documented_sweep() {
        let rate = 48_000.0f32;
        let mut flanger = Flanger::new();
        flanger.set_wet(1.0);
        flanger.set_depth(1.0);
        flanger.set_feedback(0.0);
        for _ in 0..SETTLE_FRAMES {
            flanger.process([0.0, 0.0], rate);
        }
        let min_frames = ((FLANGER_CENTER_MS - FLANGER_DEPTH_MS) * 0.001 * rate).floor() as usize;
        let max_frames = ((FLANGER_CENTER_MS + FLANGER_DEPTH_MS) * 0.001 * rate).ceil() as usize;
        // One impulse, then silence long enough to see every reflection
        // across several full LFO sweeps (the rate defaults slow, so this
        // covers less than one cycle -- fine, the bound must hold at
        // every phase the sweep actually reaches within that span).
        let mut out = vec![flanger.process([1.0, 1.0], rate)[0]];
        for _ in 1..(max_frames + 4_000) {
            out.push(flanger.process([0.0, 0.0], rate)[0]);
        }
        assert!(out[0] == 1.0, "the dry input passes through untouched");
        for (index, &value) in out.iter().enumerate().skip(1) {
            if value.abs() < 1e-4 {
                continue;
            }
            assert!(
                index >= min_frames.saturating_sub(2) && index <= max_frames + 2,
                "reflection at {index} outside the documented {min_frames}..{max_frames} sweep: {value}"
            );
        }
    }

    /// Cranked past any sane setting, the feedback clamp keeps the line
    /// bounded rather than climbing without end -- the same property the
    /// echo's feedback is held to, through the same [`pade_tanh`] clamp.
    #[test]
    fn cranked_flanger_feedback_saturates_instead_of_running_away() {
        let rate = 48_000.0f32;
        let mut flanger = Flanger::new();
        flanger.set_wet(1.0);
        flanger.set_feedback(40.0);
        assert!(
            (flanger.feedback.target() - FLANGER_FEEDBACK_MAX).abs() < 1e-6,
            "the setter itself clamps: {}",
            flanger.feedback.target()
        );
        for _ in 0..SETTLE_FRAMES {
            flanger.process([0.0, 0.0], rate);
        }
        let mut out = Vec::with_capacity(200_000);
        for i in 0..200_000usize {
            let x = if i % 4_000 == 0 { 1.0 } else { 0.0 };
            out.push(flanger.process([x, x], rate)[0]);
        }
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 3.0), "unbounded output");
    }

    /// Switching off does not cut the fed-back tail; it rings out, and
    /// only once it has genuinely decayed does the unit go back to a
    /// bit-exact bypass -- the same contract the echo's off switch keeps.
    #[test]
    fn switching_the_flanger_off_lets_the_tail_ring_out_then_goes_transparent() {
        let rate = 48_000.0f32;
        let mut flanger = Flanger::new();
        flanger.set_wet(1.0);
        flanger.set_feedback(0.7);
        for _ in 0..SETTLE_FRAMES {
            flanger.process([0.0, 0.0], rate);
        }
        let mut out = vec![flanger.process([1.0, 1.0], rate)[0]];
        for i in 1..400_000usize {
            if i == 100 {
                flanger.set_wet(0.0);
            }
            out.push(flanger.process([0.0, 0.0], rate)[0]);
        }
        assert!(!flanger.engaged(), "the operator's ask flips at once");
        let tail = &out[out.len() - 1_000..];
        assert!(tail.iter().all(|&v| v == 0.0), "silent, once the tail has actually decayed");
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(
                flanger.process([x, -x], rate),
                [x, -x],
                "a bit-exact bypass, not merely quiet"
            );
        }
    }

    /// `silence`, unlike a hypothetical reset, is safe from the audio
    /// thread precisely because it never writes to the line -- the same
    /// reason [`DeckEcho::silence`] is shaped this way. Prove it two
    /// ways: the raw memory is untouched, and yet nothing before the
    /// mark can be read back.
    #[test]
    fn flanger_silence_never_touches_the_line_only_the_reach_of_it() {
        let rate = 48_000.0f32;
        let mut flanger = Flanger::new();
        flanger.set_wet(1.0);
        flanger.set_feedback(0.0);
        for _ in 0..SETTLE_FRAMES {
            flanger.process([0.0, 0.0], rate);
        }
        flanger.process([0.8, -0.8], rate);
        let write_before = flanger.raw_write();
        let raw_before = flanger.raw_at(1);
        assert!(raw_before[0].abs() > 0.1, "a real value is there to protect: {raw_before:?}");
        flanger.silence();
        assert_eq!(flanger.raw_write(), write_before, "silence must not move the write cursor");
        assert_eq!(flanger.raw_at(1), raw_before, "and must not have memset the line either");
        // The setting the operator asked for is the strip's and stands.
        assert!(flanger.engaged());
        // But the record changed underneath it: for as long as the delay
        // would still be looking at the old content, nothing comes back.
        for _ in 0..500 {
            assert_eq!(
                flanger.process([0.0, 0.0], rate),
                [0.0, 0.0],
                "stale content must not resurface"
            );
        }
    }

    /// Every setter refuses a non-finite value and clamps everything else
    /// to its documented range -- the same [`knob`] contract every other
    /// per-deck control in this file relies on.
    #[test]
    fn flanger_setters_clamp_to_their_documented_ranges() {
        let mut flanger = Flanger::new();
        flanger.set_wet(f32::NAN);
        assert_eq!(flanger.wet.target(), 0.0, "a bad value moves nothing");
        flanger.set_wet(5.0);
        assert_eq!(flanger.wet.target(), 1.0);
        flanger.set_wet(-5.0);
        assert_eq!(flanger.wet.target(), 0.0);
        flanger.set_rate(f32::INFINITY);
        assert_eq!(flanger.rate.target(), FLANGER_RATE_DEFAULT, "unmoved by a bad value");
        flanger.set_rate(100.0);
        assert_eq!(flanger.rate.target(), FLANGER_RATE_MAX);
        flanger.set_rate(-1.0);
        assert_eq!(flanger.rate.target(), FLANGER_RATE_MIN);
        flanger.set_depth(2.0);
        assert_eq!(flanger.depth.target(), 1.0);
        flanger.set_depth(-1.0);
        assert_eq!(flanger.depth.target(), 0.0);
    }

    /// Off, a fresh unit is exactly its input -- the whole hold untouched.
    #[test]
    fn a_bitcrusher_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut bc = Bitcrusher::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(bc.process([x, -x], rate), [x, -x]);
        }
    }

    /// The raw held sample only ever changes once a whole hold period
    /// has passed -- the sample-and-hold stage's whole point. Reads
    /// `held()` directly rather than `process()`'s output: the output
    /// blend `frame + (crushed - frame) * wet` recomputes a difference
    /// against a DIFFERENT `frame` every single sample (the input tone
    /// never stops moving), and floating point does not guarantee that
    /// round-trip lands back on the exact same bits as `crushed` itself
    /// -- an exact-equality test on the blended output would be
    /// checking rounding noise, not the hold's actual timing.
    #[test]
    fn bitcrusher_holds_within_one_period_of_its_rate() {
        let rate = 48_000.0f32;
        let mut bc = Bitcrusher::new();
        bc.set_wet(1.0);
        for _ in 0..SETTLE_FRAMES {
            bc.process([0.0, 0.0], rate);
        }
        let period = (rate / BITCRUSHER_RATE_DEFAULT).round() as usize;
        let mut phase = 0.0f32;
        let mut held_track = Vec::with_capacity(1_200);
        for _ in 0..1_200 {
            phase += 2.0 * PI * 733.0 / rate;
            bc.process([phase.sin() * 0.5, 0.0], rate);
            held_track.push(bc.held()[0]);
        }
        let mut gaps = Vec::new();
        let mut last_change = 0usize;
        for i in 1..held_track.len() {
            if held_track[i] != held_track[i - 1] {
                gaps.push(i - last_change);
                last_change = i;
            }
        }
        assert!(!gaps.is_empty(), "the hold never captured a new value");
        // The first gap may be partial (the settle loop above already
        // left `hold_phase` at an unknown fraction); every gap after it
        // is a full, steady-state period.
        for gap in &gaps[1..] {
            assert_eq!(*gap, period, "hold period drifted");
        }
    }

    /// Every output sample lands on one of the `2^bits` levels the
    /// quantizer promises, spanning the full [-1, 1] range.
    #[test]
    fn bitcrusher_quantizes_to_the_documented_level_count() {
        let rate = 48_000.0f32;
        let mut bc = Bitcrusher::new();
        bc.set_wet(1.0);
        bc.set_bits(3.0);
        bc.set_rate(BITCRUSHER_RATE_MAX);
        for _ in 0..SETTLE_FRAMES {
            bc.process([0.0, 0.0], rate);
        }
        let step = 2.0f32 / 3.0f32.exp2();
        for i in 0..2_000usize {
            let x = -0.9 + 1.8 * (i as f32 / 2_000.0);
            let out = bc.process([x, x], rate)[0];
            let level = out / step;
            assert!((level - level.round()).abs() < 1e-4, "{out} is not a multiple of {step}");
        }
    }

    /// No feedback loop to run away, but the quantizer's own arithmetic
    /// must stay finite and bounded at the harshest settings, including
    /// on an over-scale input.
    #[test]
    fn bitcrusher_stays_bounded_at_the_harshest_settings() {
        let rate = 48_000.0f32;
        let mut bc = Bitcrusher::new();
        bc.set_wet(1.0);
        bc.set_bits(BITCRUSHER_BITS_MIN);
        bc.set_rate(BITCRUSHER_RATE_MIN);
        for _ in 0..SETTLE_FRAMES {
            bc.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..48_000usize {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 1.5;
            let out = bc.process([x, x], rate);
            assert!(out[0].is_finite() && out[0].abs() < 3.0, "unbounded: {out:?}");
            assert!(out[1].is_finite() && out[1].abs() < 3.0, "unbounded: {out:?}");
        }
    }

    /// `silence`, unlike the echo's or the flanger's, protects nothing
    /// but two floats -- there is no line here for a record change to
    /// leave stale, so it is a plain, unconditional reset rather than a
    /// bookkeeping-only move: the very next frame after it must reflect
    /// whatever plays next, not the hold from the record before it.
    #[test]
    fn bitcrusher_silence_forces_a_fresh_capture() {
        let rate = 48_000.0f32;
        let mut bc = Bitcrusher::new();
        bc.set_wet(1.0);
        for _ in 0..SETTLE_FRAMES {
            bc.process([0.0, 0.0], rate);
        }
        // A whole hold period, not one call, guarantees a capture lands
        // regardless of where `hold_phase` happened to settle.
        let period = (rate / BITCRUSHER_RATE_DEFAULT).round() as usize;
        for _ in 0..period {
            bc.process([0.8, -0.8], rate);
        }
        assert!(bc.held()[0].abs() > 0.1, "a real value is there to protect: {:?}", bc.held());
        bc.silence();
        bc.process([0.3, -0.3], rate);
        let held = bc.held();
        assert!((held[0] - 0.3).abs() < 1e-6, "must reflect the new content at once: {held:?}");
        assert!((held[1] + 0.3).abs() < 1e-6, "must reflect the new content at once: {held:?}");
    }

    #[test]
    fn bitcrusher_setters_clamp_to_their_documented_ranges() {
        let mut bc = Bitcrusher::new();
        bc.set_wet(f32::NAN);
        assert_eq!(bc.wet.target(), 0.0, "a bad value moves nothing");
        bc.set_wet(5.0);
        assert_eq!(bc.wet.target(), 1.0);
        bc.set_wet(-5.0);
        assert_eq!(bc.wet.target(), 0.0);
        bc.set_rate(f32::INFINITY);
        assert_eq!(bc.rate.target(), BITCRUSHER_RATE_DEFAULT, "unmoved by a bad value");
        bc.set_rate(100_000.0);
        assert_eq!(bc.rate.target(), BITCRUSHER_RATE_MAX);
        bc.set_rate(-1.0);
        assert_eq!(bc.rate.target(), BITCRUSHER_RATE_MIN);
        bc.set_bits(f32::NAN);
        assert_eq!(bc.bits.target(), BITCRUSHER_BITS_DEFAULT, "unmoved by a bad value");
        bc.set_bits(100.0);
        assert_eq!(bc.bits.target(), BITCRUSHER_BITS_MAX);
        // Let the handover the change above armed finish before asking
        // for another: `set_bits` now parks a request that arrives
        // mid-handover rather than jumping straight to it (see
        // `a_bit_depth_retune_that_arrives_mid_handover_waits_its_turn`),
        // so this drains it first to keep the clamp check itself
        // simple. `process` only advances the handover while engaged
        // (its own bypass returns before touching it otherwise), so
        // wet is engaged here purely to let the drain proceed.
        bc.set_wet(1.0);
        for _ in 0..BITCRUSHER_HANDOVER_FRAMES {
            bc.process([0.0, 0.0], 48_000.0);
        }
        bc.set_bits(-5.0);
        assert_eq!(bc.bits.target(), BITCRUSHER_BITS_MIN);
    }

    /// A second `set_bits` landing while the first is still crossfading
    /// is parked, not raced -- mirroring
    /// `a_retune_that_arrives_mid_handover_waits_its_turn`, the echo's
    /// own test for the identical hazard. Without the queue, the second
    /// call would reset `handover` to full while `bits_active` stayed at
    /// its PRE-FIRST-CALL value, snapping the blend back to zero and
    /// discarding whatever fraction of the interrupted transition had
    /// already played -- a real step, not a hypothetical one: reverting
    /// the fix reproduces a worst adjacent-sample jump of roughly 0.98
    /// on the harshest settings.
    #[test]
    fn a_bit_depth_retune_that_arrives_mid_handover_waits_its_turn() {
        let rate = 48_000.0f32;
        let mut bc = Bitcrusher::new();
        bc.set_wet(1.0);
        bc.set_bits(16.0);
        for _ in 0..SETTLE_FRAMES {
            bc.process([0.0, 0.0], rate);
        }
        // A constant, off-grid input (not a moving tone) isolates the
        // handover's own behavior from the hold stage's timing.
        let input = 12_345.0 / 32_768.0;
        for _ in 0..20 {
            bc.process([input, input], rate);
        }
        bc.set_bits(2.0);
        assert_eq!(bc.handover, BITCRUSHER_HANDOVER_FRAMES, "the first target lands at once");
        for _ in 0..100 {
            bc.process([input, input], rate);
        }
        let before_second_call = bc.handover;
        assert!(before_second_call > 0 && before_second_call < BITCRUSHER_HANDOVER_FRAMES);
        bc.set_bits(9.0);
        assert_eq!(bc.handover, before_second_call, "a running handover finishes first");
        assert_eq!(bc.pending, Some(9.0));

        let mut out = vec![bc.process([input, input], rate)[0]];
        for _ in 1..(before_second_call as usize + BITCRUSHER_HANDOVER_FRAMES as usize + 10) {
            out.push(bc.process([input, input], rate)[0]);
        }
        let worst =
            out.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.02, "a mid-handover retune must not click, biggest step {worst}");
        assert_eq!(bc.pending, None, "the parked target has been taken up");
    }

    /// Off, a fresh unit is exactly its input.
    #[test]
    fn a_tremolo_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut trem = Tremolo::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(trem.process([x, -x], rate), [x, -x]);
        }
    }

    /// Engaged, the gain the LFO applies never inverts the signal and
    /// never boosts past unity -- a strict, provable [1 - depth, 1.0].
    #[test]
    fn an_engaged_tremolo_stays_within_its_documented_gain_range() {
        let rate = 48_000.0f32;
        let mut trem = Tremolo::new();
        trem.set_wet(1.0);
        trem.set_depth(0.85);
        for _ in 0..SETTLE_FRAMES {
            trem.process([0.0, 0.0], rate);
        }
        let input = 0.5f32;
        for _ in 0..48_000usize {
            let out = trem.process([input, input], rate)[0];
            let gain = out / input;
            assert!(gain >= 1.0 - 0.85 - 1e-4 && gain <= 1.0 + 1e-4, "gain {gain} out of range");
        }
    }

    /// No feedback path to diverge, but the invariant is worth locking
    /// in against a future regression: cranked all the way, output
    /// never exceeds the input's own magnitude and stays finite.
    #[test]
    fn cranked_tremolo_depth_and_rate_stay_bounded() {
        let rate = 48_000.0f32;
        let mut trem = Tremolo::new();
        trem.set_wet(1.0);
        trem.set_depth(1.0);
        trem.set_rate(TREMOLO_RATE_MAX);
        for _ in 0..SETTLE_FRAMES {
            trem.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..48_000usize {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            let out = trem.process([x, x], rate);
            assert!(out[0].is_finite() && out[0].abs() <= x.abs() + 1e-6, "{out:?} vs {x}");
        }
    }

    /// No line, no tail: disengage completes exactly the ramp's own
    /// duration after `set_wet(0.0)`, unlike the echo's or the
    /// flanger's, which must wait out a real ring-out first.
    #[test]
    fn switching_the_tremolo_off_returns_to_bit_exact_bypass_after_its_ramp() {
        let rate = 48_000.0f32;
        let mut trem = Tremolo::new();
        trem.set_wet(1.0);
        for _ in 0..SETTLE_FRAMES {
            trem.process([0.5, 0.5], rate);
        }
        trem.set_wet(0.0);
        // EQ_ENGAGE_SECS is 0.012s; a couple hundred extra frames of
        // margin is comfortably past it without approaching anything
        // that would matter for a unit with no tail to wait out.
        for _ in 0..1_000 {
            trem.process([0.5, 0.5], rate);
        }
        assert!(!trem.engaged());
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(trem.process([x, -x], rate), [x, -x], "a bit-exact bypass, not merely quiet");
        }
    }

    #[test]
    fn tremolo_setters_clamp_to_their_documented_ranges() {
        let mut trem = Tremolo::new();
        trem.set_wet(f32::NAN);
        assert_eq!(trem.wet.target(), 0.0, "a bad value moves nothing");
        trem.set_wet(5.0);
        assert_eq!(trem.wet.target(), 1.0);
        trem.set_wet(-5.0);
        assert_eq!(trem.wet.target(), 0.0);
        trem.set_rate(f32::INFINITY);
        assert_eq!(trem.rate.target(), TREMOLO_RATE_DEFAULT, "unmoved by a bad value");
        trem.set_rate(1_000.0);
        assert_eq!(trem.rate.target(), TREMOLO_RATE_MAX);
        trem.set_rate(-1.0);
        assert_eq!(trem.rate.target(), TREMOLO_RATE_MIN);
        trem.set_depth(2.0);
        assert_eq!(trem.depth.target(), 1.0);
        trem.set_depth(-1.0);
        assert_eq!(trem.depth.target(), 0.0);
    }

    /// A settled slot on its default -- no correction asked for, all
    /// wet -- hands the effect's own output back untouched. Every
    /// bit-transparency test in this file sits downstream of this.
    #[test]
    fn a_default_slot_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut level = SlotLevel::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 233.0 / rate;
            let dry = [phase.sin() * 0.6, phase.cos() * 0.4];
            // A "wet" that is not the dry, so a blend would show.
            let wet = [dry[0] * 0.5 + 0.1, dry[1] * 1.5 - 0.2];
            assert_eq!(level.apply(dry, wet, rate, LevelMode::Off), wet);
        }
    }

    /// Mix at zero is the dry signal, whatever the effect did.
    #[test]
    fn a_slot_at_zero_mix_is_the_dry_signal() {
        let rate = 48_000.0f32;
        let mut level = SlotLevel::new();
        level.set_mix(0.0);
        let mut phase = 0.0f32;
        // Past the mix ramp.
        for _ in 0..SETTLE_FRAMES {
            level.apply([0.0, 0.0], [0.5, 0.5], rate, LevelMode::Off);
        }
        for _ in 0..4_000 {
            phase += 2.0 * PI * 233.0 / rate;
            let dry = [phase.sin() * 0.6, phase.cos() * 0.4];
            let wet = [dry[0] * 4.0, dry[1] * 4.0];
            let out = level.apply(dry, wet, rate, LevelMode::Off);
            assert!((out[0] - dry[0]).abs() < 1e-6, "{} vs {}", out[0], dry[0]);
            assert!((out[1] - dry[1]).abs() < 1e-6, "{} vs {}", out[1], dry[1]);
        }
    }

    /// Matching brings an effect that adds level back to what went in.
    /// A four-times boost is well past what any of the shipped effects
    /// does, so this is the mechanism under load, not a typical case.
    #[test]
    fn matching_holds_a_loud_effect_at_the_input_level() {
        let rate = 48_000.0f32;
        let mut level = SlotLevel::new();
        level.set_mode(LevelMode::MatchInput);
        let mut phase = 0.0f32;
        let mut tone = move || {
            phase += 2.0 * PI * 220.0 / rate;
            phase.sin() * 0.3
        };
        // The envelopes are slow on purpose; give them a second.
        for _ in 0..(rate as usize) {
            let x = tone();
            level.apply([x, x], [x * 4.0, x * 4.0], rate, LevelMode::Off);
        }
        let (mut dry_sq, mut out_sq) = (0.0f64, 0.0f64);
        for _ in 0..(rate as usize / 2) {
            let x = tone();
            let out = level.apply([x, x], [x * 4.0, x * 4.0], rate, LevelMode::Off)[0];
            dry_sq += (x as f64) * (x as f64);
            out_sq += (out as f64) * (out as f64);
        }
        let db = 20.0 * (out_sq / dry_sq).sqrt().log10();
        assert!(db.abs() < 0.5, "matching left {db:.2} dB on the table");
    }

    /// Matching leaves a slot that is already at the input level alone,
    /// rather than finding something to correct.
    #[test]
    fn matching_does_not_disturb_an_effect_that_holds_its_level() {
        let rate = 48_000.0f32;
        let mut level = SlotLevel::new();
        level.set_mode(LevelMode::MatchInput);
        let mut phase = 0.0f32;
        let mut tone = move || {
            phase += 2.0 * PI * 220.0 / rate;
            phase.sin() * 0.3
        };
        for _ in 0..(rate as usize) {
            let x = tone();
            level.apply([x, x], [x, x], rate, LevelMode::Off);
        }
        for _ in 0..1_000 {
            let x = tone();
            let out = level.apply([x, x], [x, x], rate, LevelMode::Off)[0];
            assert!((out - x).abs() < 1e-3, "{out} vs {x}");
        }
    }

    /// A ceiling holds the output under its bound and lets go again.
    #[test]
    fn a_ceiling_holds_the_output_under_its_bound() {
        let rate = 48_000.0f32;
        let mut level = SlotLevel::new();
        level.set_mode(LevelMode::Ceiling);
        level.set_ceiling(0.5);
        let mut phase = 0.0f32;
        let mut worst = 0.0f32;
        for n in 0..(rate as usize) {
            phase += 2.0 * PI * 220.0 / rate;
            let x = phase.sin() * 0.3;
            // Loud for the first half, quiet after.
            let wet = if n < rate as usize / 2 { x * 3.0 } else { x };
            let out = level.apply([x, x], [wet, wet], rate, LevelMode::Off)[0];
            // Skip the first few frames: the limiter catches the sample
            // that overshot, it cannot see it coming.
            if n > 64 {
                worst = worst.max(out.abs());
            }
        }
        assert!(worst <= 0.55, "the ceiling let {worst} through");
        // Once the loud passage is over the gain has come back up.
        for _ in 0..(rate as usize) {
            phase += 2.0 * PI * 220.0 / rate;
            let x = phase.sin() * 0.3;
            level.apply([x, x], [x, x], rate, LevelMode::Off);
        }
        phase += 2.0 * PI * 220.0 / rate;
        let x = phase.sin() * 0.3;
        let out = level.apply([x, x], [x, x], rate, LevelMode::Off)[0];
        assert!((out - x).abs() < 1e-3, "the ceiling never let go: {out} vs {x}");
    }

    /// A slot on Follow takes the deck's default, and a pinned one does
    /// not. Follow against a default of Follow resolves to Off rather
    /// than chasing itself.
    #[test]
    fn a_slot_follows_the_deck_until_it_is_pinned() {
        let rate = 48_000.0f32;
        let loud = |level: &mut SlotLevel, default: LevelMode| {
            let mut phase = 0.0f32;
            let mut last = 0.0f32;
            for _ in 0..(rate as usize) {
                phase += 2.0 * PI * 220.0 / rate;
                let x = phase.sin() * 0.3;
                last = level.apply([x, x], [x * 4.0, x * 4.0], rate, default)[0];
            }
            last
        };
        // Following a deck that is matching: corrected.
        let mut following = SlotLevel::new();
        assert_eq!(following.mode(), LevelMode::Follow);
        let matched = loud(&mut following, LevelMode::MatchInput);
        // Pinned off against the same deck: untouched, four times the dry.
        let mut pinned = SlotLevel::new();
        pinned.set_mode(LevelMode::Off);
        let untouched = loud(&mut pinned, LevelMode::MatchInput);
        assert!(
            matched.abs() < untouched.abs() * 0.5,
            "following {matched} against pinned {untouched}"
        );
        // Follow with nothing to follow is Off, not a loop.
        let mut orphan = SlotLevel::new();
        let out = loud(&mut orphan, LevelMode::Follow);
        assert!((out - untouched).abs() < 1e-3, "{out} vs {untouched}");
    }

    /// The mix is ramped, so moving it under a sounding deck cannot
    /// step the output.
    #[test]
    fn a_mix_change_does_not_step_the_output() {
        let rate = 48_000.0f32;
        let mut level = SlotLevel::new();
        let mut phase = 0.0f32;
        let mut prev: Option<f32> = None;
        let mut worst = 0.0f32;
        for n in 0..12_000usize {
            if n == 2_000 {
                level.set_mix(0.0);
            }
            if n == 7_000 {
                level.set_mix(1.0);
            }
            // A low tone on purpose: the measure is the biggest step
            // between neighbouring samples, and a tone's own slope
            // counts towards it. At 220 Hz and full scale that slope is
            // already 0.029 a sample, which would swamp what is being
            // looked for. At 40 Hz it is 0.005.
            phase += 2.0 * PI * 40.0 / rate;
            let x = phase.sin() * 0.5;
            let out = level.apply([x, x], [x * 2.0, x * 2.0], rate, LevelMode::Off)[0];
            if let Some(p) = prev {
                worst = worst.max((out - p).abs());
            }
            prev = Some(out);
        }
        assert!(worst < 0.02, "a mix change stepped by {worst}");
    }

    /// Its setters clamp, and a bad number moves nothing.
    #[test]
    fn slot_level_setters_clamp_to_their_documented_ranges() {
        let mut level = SlotLevel::new();
        level.set_mix(5.0);
        assert_eq!(level.mix(), 1.0);
        level.set_mix(-1.0);
        assert_eq!(level.mix(), 0.0);
        level.set_mix(f32::NAN);
        assert_eq!(level.mix(), 0.0, "a bad value moves nothing");
        level.set_ceiling(9.0);
        assert_eq!(level.ceiling(), 1.0);
        level.set_ceiling(0.0);
        assert_eq!(level.ceiling(), 0.01);
        level.set_ceiling(f32::INFINITY);
        assert_eq!(level.ceiling(), 0.01, "a bad value moves nothing");
    }

    /// A clock standing still at `beat` on a grid whose beats are
    /// `beat_secs` long, for the tests that only care about the rate a
    /// locked LFO picks.
    fn clock_at(beat: f64, beat_secs: f64) -> crate::wave_analysis::DeckClock {
        crate::wave_analysis::DeckClock {
            beat_secs_out: beat_secs,
            beat_frac_end: beat.rem_euclid(1.0),
            beat_at_end: beat,
            platter_rate: 1.0,
            has_grid: true,
        }
    }

    /// A clock with nothing measured behind it.
    fn clock_ungridded() -> crate::wave_analysis::DeckClock {
        crate::wave_analysis::DeckClock::default()
    }

    /// Quiet material comes out exactly as it went in, one look-ahead
    /// later. A limiter that colours what it never had to touch is a
    /// limiter nobody can leave switched on.
    #[test]
    fn a_limiter_under_its_ceiling_is_a_delay_and_nothing_else() {
        let rate = 48_000.0f32;
        let mut lim = Limiter::new(rate);
        let delay = lim.latency_frames();
        let mut fed = Vec::new();
        let mut out = Vec::new();
        let mut phase = 0.0f32;
        for _ in 0..8_000 {
            phase += 2.0 * PI * 220.0 / rate;
            let x = phase.sin() * 0.5;
            fed.push(x);
            out.push(lim.process([x, -x])[0]);
        }
        for n in delay..fed.len() {
            assert_eq!(out[n], fed[n - delay], "sample {n} was altered");
        }
        assert_eq!(lim.worst_reduction(), 1.0, "it should not have moved at all");
    }

    /// The guarantee: whatever goes in, nothing over the ceiling comes
    /// out. Not "usually" and not "after the attack" -- the look-ahead
    /// exists so the gain is already there when the peak arrives.
    #[test]
    fn nothing_leaves_the_limiter_above_its_ceiling() {
        let rate = 48_000.0f32;
        for amplitude in [1.0f32, 2.0, 4.0, 20.0] {
            let mut lim = Limiter::new(rate);
            let mut phase = 0.0f32;
            let mut worst = 0.0f32;
            for n in 0..48_000usize {
                phase += 2.0 * PI * 110.0 / rate;
                // Silence, then a wall, then silence again: the step into
                // the loud passage is the moment that matters.
                let x = match (8_000..24_000).contains(&n) {
                    true => phase.sin() * amplitude,
                    false => phase.sin() * 0.1,
                };
                let out = lim.process([x, x]);
                worst = worst.max(out[0].abs()).max(out[1].abs());
            }
            assert!(
                worst <= LIMITER_CEILING + 1e-4,
                "at amplitude {amplitude} it let {worst} through"
            );
        }
    }

    /// A lone sample far over the ceiling is caught too -- the case a
    /// slow attack would miss entirely and a clamp would flat-top.
    #[test]
    fn a_single_spike_is_caught_before_it_lands() {
        let rate = 48_000.0f32;
        let mut lim = Limiter::new(rate);
        let mut worst = 0.0f32;
        for n in 0..4_000usize {
            let x = if n == 1_000 { 8.0 } else { 0.2 };
            let out = lim.process([x, x]);
            worst = worst.max(out[0].abs());
        }
        assert!(worst <= LIMITER_CEILING + 1e-4, "a spike got out at {worst}");
    }

    /// It gives the gain back, so one loud passage does not duck the
    /// rest of the set.
    #[test]
    fn a_limiter_releases_after_the_loud_passage() {
        let rate = 48_000.0f32;
        let mut lim = Limiter::new(rate);
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 110.0 / rate;
            lim.process([phase.sin() * 6.0, phase.sin() * 6.0]);
        }
        assert!(lim.worst_reduction() < 0.3, "it should have ducked hard");
        // A second of quiet is several times the release. This stretch
        // still STARTS ducked, so what it measures is the release
        // happening, not the state it ended in.
        for _ in 0..48_000 {
            phase += 2.0 * PI * 110.0 / rate;
            lim.process([phase.sin() * 0.1, phase.sin() * 0.1]);
        }
        // Clear that history, then measure a fresh quiet stretch: the
        // meter reports the worst since it was last read, so it has to
        // be read before the window that is being asked about.
        lim.worst_reduction();
        for _ in 0..4_800 {
            phase += 2.0 * PI * 110.0 / rate;
            lim.process([phase.sin() * 0.1, phase.sin() * 0.1]);
        }
        let settled = lim.worst_reduction();
        assert!(settled > 0.99, "the gain never came back: {settled}");
    }

    /// Both channels duck together: ducking them apart would walk the
    /// stereo image around under a loud passage.
    #[test]
    fn a_limiter_keeps_the_stereo_image_still() {
        let rate = 48_000.0f32;
        let mut lim = Limiter::new(rate);
        let mut phase = 0.0f32;
        for n in 0..12_000usize {
            phase += 2.0 * PI * 110.0 / rate;
            // One channel loud enough to duck, the other quiet: their
            // ratio has to survive it.
            let l = phase.sin() * 4.0;
            let r = phase.sin() * 1.0;
            let out = lim.process([l, r]);
            if n > 1_000 && out[1].abs() > 1e-6 {
                let ratio = out[0] / out[1];
                assert!((ratio - 4.0).abs() < 1e-3, "the image moved: {ratio}");
            }
        }
    }

    /// A rate change re-windows the look-ahead without allocating and
    /// without losing the line.
    #[test]
    fn a_limiter_rewindows_for_the_device_rate() {
        let mut lim = Limiter::new(48_000.0);
        let at_48 = lim.latency_frames();
        lim.set_sample_rate(96_000.0);
        let at_96 = lim.latency_frames();
        assert!(
            (at_96 as f32 / at_48 as f32 - 2.0).abs() < 0.02,
            "{at_48} then {at_96}"
        );
        // The same look-ahead in SECONDS either way, which is the point.
        let secs = at_96 as f32 / 96_000.0;
        assert!((secs - LIMITER_LOOKAHEAD_SECS).abs() < 1e-4, "{secs}");
    }

    /// The corners move, and clamp to their documented ranges.
    #[test]
    fn the_crossovers_move_and_clamp() {
        let mut eq = DeckEq::new(48_000.0);
        assert_eq!(eq.crossovers(), (EQ_LOW_HZ, EQ_HIGH_HZ));
        eq.set_crossovers(150.0, 4_000.0);
        assert_eq!(eq.crossovers(), (150.0, 4_000.0));
        eq.set_crossovers(1.0, 100_000.0);
        assert_eq!(eq.crossovers(), (EQ_LOW_HZ_MIN, EQ_HIGH_HZ_MAX));
        eq.set_crossovers(f32::NAN, 4_000.0);
        assert_eq!(eq.crossovers(), (EQ_LOW_HZ_MIN, EQ_HIGH_HZ_MAX), "a bad value moves nothing");
    }

    /// The mid band always has something in it. Pushing the corners
    /// together pushes back rather than letting them cross -- a
    /// three-band EQ whose middle is empty is not a three-band EQ.
    #[test]
    fn the_crossovers_keep_the_mid_band_open() {
        let mut eq = DeckEq::new(48_000.0);
        // Ask for them on top of each other from below.
        eq.set_crossovers(800.0, 1_000.0);
        let (low, high) = eq.crossovers();
        assert!(high / low >= 2.0 - 1e-3, "{low} and {high} are too close");
        // And from above.
        eq.set_crossovers(800.0, EQ_HIGH_HZ_MAX);
        let (low, high) = eq.crossovers();
        assert!(high / low >= 2.0 - 1e-3, "{low} and {high} are too close");
        // Every pair the ranges allow keeps the gap.
        for low_ask in [80.0f32, 200.0, 500.0, 800.0] {
            for high_ask in [1_000.0f32, 2_500.0, 8_000.0] {
                let mut eq = DeckEq::new(48_000.0);
                eq.set_crossovers(low_ask, high_ask);
                let (low, high) = eq.crossovers();
                assert!(
                    high / low >= 2.0 - 1e-3,
                    "asked {low_ask}/{high_ask}, got {low}/{high}"
                );
            }
        }
    }

    /// A corner that is asked for is a corner that is reached. Gliding
    /// is only worth anything if it arrives -- a walk that closes a
    /// fraction of the remaining distance each block converges, and this
    /// is the test that says how long that takes in practice.
    #[test]
    fn a_moved_crossover_arrives_where_it_was_sent() {
        let rate = 48_000.0f32;
        let mut eq = DeckEq::new(rate);
        eq.set_sample_rate(rate);
        eq.set_crossovers(600.0, 6_000.0);
        // Half a second of blocks at an ordinary buffer size.
        for _ in 0..46 {
            eq.prepare_block();
        }
        let (low, high) = (eq.low_built, eq.high_built);
        assert!((low - 600.0).abs() < 6.0, "the low corner stalled at {low}");
        assert!((high - 6_000.0).abs() < 60.0, "the high corner stalled at {high}");
    }

    /// Moving a corner does not step the output. The band filters' own
    /// memory was built for where the corner USED to be, so it is
    /// dropped with the rebuild rather than fed through the new one.
    #[test]
    fn moving_a_crossover_does_not_step_the_output() {
        let rate = 48_000.0f32;
        let mut eq = DeckEq::new(rate);
        eq.set_sample_rate(rate);
        // Engaged, so the chain is actually in the path.
        eq.set_band(0, 1.6);
        let mut phase = 0.0f32;
        let mut prev: Option<f32> = None;
        let mut worst = 0.0f32;
        for n in 0..24_000usize {
            if n % 512 == 0 {
                eq.prepare_block();
            }
            if n == 8_000 {
                eq.set_crossovers(500.0, 5_000.0);
            }
            if n == 16_000 {
                eq.set_crossovers(120.0, 1_500.0);
            }
            // A low tone: its own slope must not swamp the measure.
            phase += 2.0 * PI * 40.0 / rate;
            let x = phase.sin() * 0.5;
            let out = eq.process([x, x], rate)[0];
            if let Some(p) = prev {
                worst = worst.max((out - p).abs());
            }
            prev = Some(out);
        }
        assert!(worst < 0.02, "a crossover move stepped by {worst}");
    }

    /// Unity is a wire. Three bells sit in the path whenever any band is
    /// lifted, so the thing that has to be true first is that at rest
    /// they do nothing at all.
    #[test]
    fn the_bells_are_a_wire_at_unity() {
        let rate = 48_000.0f32;
        let mut eq = DeckEq::new(rate);
        eq.set_sample_rate(rate);
        // Engaged through the filter, so the chain is in the path, but
        // every band knob at unity.
        eq.set_filter(0.3);
        let mut plain = DeckEq::new(rate);
        plain.set_sample_rate(rate);
        plain.set_filter(0.3);
        let mut phase = 0.0f32;
        for n in 0..8_000usize {
            if n % 512 == 0 {
                eq.prepare_block();
                plain.prepare_block();
            }
            phase += 2.0 * PI * 220.0 / rate;
            let x = phase.sin() * 0.5;
            let with = eq.process([x, x], rate)[0];
            let without = plain.process([x, x], rate)[0];
            assert_eq!(with, without, "the bells coloured a chain at unity");
        }
    }

    /// A cut is still a kill. The isolator half is what a DJ EQ is for
    /// and the boost law must not have touched it.
    #[test]
    fn a_killed_band_is_still_silent() {
        let rate = 48_000.0f32;
        let mut eq = DeckEq::new(rate);
        eq.set_sample_rate(rate);
        for band in 0..3 {
            eq.set_band(band, 0.0);
        }
        let mut phase = 0.0f32;
        // Once a BLOCK, the way the callback does it. Called every frame,
        // `prepare_block` re-slews `wet` every frame, and a ramp whose
        // step is recomputed from the distance still to go never arrives
        // -- so the chain sits a hair below fully wet and leaks dry.
        for n in 0..SETTLE_FRAMES {
            if n % 512 == 0 {
                eq.prepare_block();
            }
            phase += 2.0 * PI * 220.0 / rate;
            eq.process([phase.sin() * 0.5, phase.sin() * 0.5], rate);
        }
        let mut worst = 0.0f32;
        for n in 0..8_000usize {
            if n % 512 == 0 {
                eq.prepare_block();
            }
            phase += 2.0 * PI * 220.0 / rate;
            let x = phase.sin() * 0.5;
            worst = worst.max(eq.process([x, x], rate)[0].abs());
        }
        // Sixty decibels down, this file's own definition of silence.
        // What is left is the engage ramp's asymptote: `prepare_block`
        // re-slews `wet` every block, so it approaches fully-wet without
        // arriving, and a hair of dry rides along for ever.
        assert!(worst < 1e-3, "all three bands killed still passed {worst}");
    }

    /// A boost is a bell and not a brick: lifting the LOW band lifts a
    /// low tone much more than a high one. Scaling the crossover slice
    /// would lift everything inside it by the same amount and nothing
    /// outside, which is the shape this law exists to avoid.
    #[test]
    fn a_boost_lifts_its_own_band_and_mostly_leaves_the_others() {
        let rate = 48_000.0f32;
        let level = |hz: f32, boost: f32| -> f64 {
            let mut eq = DeckEq::new(rate);
            eq.set_sample_rate(rate);
            eq.set_band(0, boost);
            let mut phase = 0.0f32;
            for n in 0..12_000usize {
                if n % 512 == 0 {
                    eq.prepare_block();
                }
                phase += 2.0 * PI * hz / rate;
                eq.process([phase.sin() * 0.4, phase.sin() * 0.4], rate);
            }
            let mut sum = 0.0f64;
            for n in 0..12_000usize {
                if n % 512 == 0 {
                    eq.prepare_block();
                }
                phase += 2.0 * PI * hz / rate;
                let out = eq.process([phase.sin() * 0.4, phase.sin() * 0.4], rate)[0];
                sum += (out as f64) * (out as f64);
            }
            sum.sqrt()
        };
        // 125 Hz is the low band's bell centre at the default corners.
        let low_lift = level(125.0, 2.0) / level(125.0, 1.0);
        let high_lift = level(6_000.0, 2.0) / level(6_000.0, 1.0);
        assert!(low_lift > 1.4, "the low band was not lifted: {low_lift}");
        // The point of a bell: the far band barely moves. Scaling the
        // crossover slice instead would leave it at exactly 1.0 but lift
        // everything INSIDE the low band equally, corners and all --
        // which is the shape this law exists to avoid, and is what the
        // centre-versus-edge comparison below actually catches.
        assert!(
            high_lift < 1.1,
            "the lift reached the highs: low {low_lift}, high {high_lift}"
        );
        // And it is a hump, not a brick: the band's own centre is lifted
        // appreciably more than its edge.
        let edge_lift = level(EQ_LOW_HZ, 2.0) / level(EQ_LOW_HZ, 1.0);
        assert!(
            edge_lift < low_lift * 0.9,
            "the boost was flat across the band: centre {low_lift}, edge {edge_lift}"
        );
    }

    /// Crossing unity is where the two halves meet, and it must not
    /// step: the isolator stops scaling exactly where the bell starts
    /// lifting, and at the crossing both are a wire.
    #[test]
    fn crossing_unity_does_not_step_the_output() {
        let rate = 48_000.0f32;
        let mut eq = DeckEq::new(rate);
        eq.set_sample_rate(rate);
        let mut phase = 0.0f32;
        let mut prev: Option<f32> = None;
        let mut worst = 0.0f32;
        for n in 0..40_000usize {
            if n % 512 == 0 {
                eq.prepare_block();
            }
            // Walk the knob from a deep cut, through unity, to a boost.
            if n % 400 == 0 {
                let t = n as f32 / 40_000.0;
                eq.set_band(0, t * 2.0);
            }
            phase += 2.0 * PI * 40.0 / rate;
            let x = phase.sin() * 0.5;
            let out = eq.process([x, x], rate)[0];
            if let Some(p) = prev {
                worst = worst.max((out - p).abs());
            }
            prev = Some(out);
        }
        assert!(worst < 0.02, "crossing unity stepped by {worst}");
    }

    /// Off is exactly the input.
    #[test]
    fn a_compressor_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut comp = Compressor::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(comp.process([x, -x], rate), [x, -x]);
        }
    }

    /// The knee has no corner in it. A soft knee that does not actually
    /// meet the two straight sections is just a hard knee with extra
    /// arithmetic, so walk across it and check the curve stays smooth.
    #[test]
    fn the_compressor_knee_is_smooth_across_the_threshold() {
        let threshold = -18.0f32;
        let ratio = 4.0f32;
        let mut last: Option<(f32, f32)> = None;
        let mut worst = 0.0f32;
        for step in 0..2_000 {
            let input = -40.0 + step as f32 * 0.02;
            let out = knee_curve(input, threshold, ratio);
            if let Some((prev_in, prev_out)) = last {
                let slope = (out - prev_out) / (input - prev_in);
                // Below the knee the slope is 1, above it 1/ratio, and
                // nowhere may it leave that band.
                assert!(
                    slope <= 1.0 + 1e-3 && slope >= 1.0 / ratio - 1e-3,
                    "slope {slope} at {input} dB is outside the curve"
                );
                worst = worst.max((out - prev_out).abs());
            }
            last = Some((input, out));
        }
        // Far below and far above, the two straight sections.
        assert!((knee_curve(-40.0, threshold, ratio) - -40.0).abs() < 1e-4);
        let far = knee_curve(0.0, threshold, ratio);
        assert!((far - (threshold + 18.0 / ratio)).abs() < 1e-4, "{far}");
    }

    /// It squashes the loud and leaves the quiet, which is the whole
    /// job: a signal under the threshold comes out where it went in, one
    /// well over it comes out closer to the threshold than it started.
    #[test]
    fn a_compressor_narrows_the_gap_between_loud_and_quiet() {
        let rate = 48_000.0f32;
        let level = |amp: f32| -> f64 {
            let mut comp = Compressor::new();
            comp.set_wet(1.0);
            comp.set_threshold_db(-18.0);
            comp.set_ratio(8.0);
            let mut phase = 0.0f32;
            for _ in 0..(rate as usize / 2) {
                phase += 2.0 * PI * 220.0 / rate;
                comp.process([phase.sin() * amp, phase.sin() * amp], rate);
            }
            let mut sum = 0.0f64;
            for _ in 0..(rate as usize / 4) {
                phase += 2.0 * PI * 220.0 / rate;
                let out = comp.process([phase.sin() * amp, phase.sin() * amp], rate)[0];
                sum += (out as f64) * (out as f64);
            }
            sum.sqrt()
        };
        let quiet_in = 0.02f32;
        let loud_in = 0.8f32;
        let ratio_in = (loud_in / quiet_in) as f64;
        let ratio_out = level(loud_in) / level(quiet_in);
        assert!(
            ratio_out < ratio_in * 0.6,
            "the gap barely moved: {ratio_in} in, {ratio_out} out"
        );
    }

    /// Both channels duck together, or a loud left would walk the image.
    #[test]
    fn a_compressor_keeps_the_stereo_image_still() {
        let rate = 48_000.0f32;
        let mut comp = Compressor::new();
        comp.set_wet(1.0);
        comp.set_threshold_db(-24.0);
        comp.set_ratio(8.0);
        let mut phase = 0.0f32;
        for n in 0..24_000usize {
            phase += 2.0 * PI * 220.0 / rate;
            let l = phase.sin() * 0.8;
            let r = phase.sin() * 0.2;
            let out = comp.process([l, r], rate);
            if n > 4_000 && out[1].abs() > 1e-6 {
                let ratio = out[0] / out[1];
                assert!((ratio - 4.0).abs() < 1e-3, "the image moved: {ratio}");
            }
        }
    }

    /// Engaging and releasing it is a ramp, not a switch.
    #[test]
    fn engaging_the_compressor_does_not_step_the_output() {
        let rate = 48_000.0f32;
        let mut comp = Compressor::new();
        comp.set_threshold_db(-24.0);
        comp.set_ratio(8.0);
        let mut phase = 0.0f32;
        let mut prev: Option<f32> = None;
        let mut worst = 0.0f32;
        for n in 0..40_000usize {
            if n == 8_000 {
                comp.set_wet(1.0);
            }
            if n == 24_000 {
                comp.set_wet(0.0);
            }
            phase += 2.0 * PI * 40.0 / rate;
            let x = phase.sin() * 0.5;
            let out = comp.process([x, x], rate)[0];
            if let Some(p) = prev {
                worst = worst.max((out - p).abs());
            }
            prev = Some(out);
        }
        assert!(worst < 0.02, "engaging the compressor stepped by {worst}");
    }

    /// Its setters clamp to their documented ranges.
    #[test]
    fn compressor_setters_clamp_to_their_documented_ranges() {
        let mut comp = Compressor::new();
        comp.set_threshold_db(20.0);
        assert_eq!(comp.threshold_db.target(), COMPRESSOR_THRESHOLD_MAX_DB);
        comp.set_threshold_db(-200.0);
        assert_eq!(comp.threshold_db.target(), COMPRESSOR_THRESHOLD_MIN_DB);
        comp.set_ratio(100.0);
        assert_eq!(comp.ratio.target(), COMPRESSOR_RATIO_MAX);
        comp.set_ratio(0.0);
        assert_eq!(comp.ratio.target(), COMPRESSOR_RATIO_MIN);
        comp.set_ratio(f32::NAN);
        assert_eq!(comp.ratio.target(), COMPRESSOR_RATIO_MIN, "a bad value moves nothing");
    }

    /// The ladder is the whole set an operator can pick from: free
    /// first, then powers of two either side of one cycle a beat.
    #[test]
    fn the_sync_ladder_reads_as_its_labels_say() {
        assert_eq!(LFO_SYNC_ROWS[0], (LFO_SYNC_FREE, "Hz"));
        assert_eq!(LFO_SYNC_ROWS[LFO_SYNC_ROWS.len() - 1].0, LFO_SYNC_MAX_UNITS);
        // Every rung past the first is a power of two, and each label
        // says what its units actually mean in cycles per beat.
        for (units, label) in LFO_SYNC_ROWS.iter().skip(1) {
            assert!(units.is_power_of_two(), "{label} is not a power of two");
            let cycles = sync_cycles_per_beat(*units);
            let expected = match label.strip_prefix("1/") {
                Some(divisor) => 1.0 / divisor.parse::<f64>().unwrap(),
                None => label.parse::<f64>().unwrap(),
            };
            assert!((cycles - expected).abs() < 1e-9, "{label} reads as {cycles}");
        }
    }

    /// Locked, the rate is the division divided by a beat's length, so
    /// two cycles per beat at 120bpm (half a second a beat) is 4Hz.
    #[test]
    fn a_locked_tremolo_turns_its_division_into_hz() {
        let mut trem = Tremolo::new();
        // Sixteen eighths is two cycles a beat.
        trem.set_sync_units(16);
        // Ungridded: the rate is the division over the counted beat, with
        // no servo to trim it, which is what this test is about.
        trem.prepare_block(&clock_ungridded(), 512.0 / 48_000.0);
        assert!((trem.active_hz - 2.0).abs() < 1e-6, "{}", trem.active_hz);
        // On a grid whose beat is half a second, two cycles a beat is 4Hz.
        trem.prepare_block(&clock_at(0.0, 0.5), 512.0 / 48_000.0);
        assert!(trem.active_hz > 3.0 && trem.active_hz < 5.0, "{}", trem.active_hz);
        // Half the tempo, half the rate -- it tracks, buffer to buffer.
        trem.prepare_block(&clock_at(0.0, 1.0), 512.0 / 48_000.0);
        assert!(trem.active_hz > 1.5 && trem.active_hz < 2.5, "{}", trem.active_hz);
    }

    /// The whole ladder is reachable, including the rungs that sit far
    /// outside this effect's own Hz range -- the bug the shared rate
    /// field had, where a division was clamped to a rate limit.
    #[test]
    fn every_rung_reaches_the_engine_whatever_the_hz_range_is() {
        // The phaser's Hz range stops at 5, well under what the top
        // rungs ask for at any ordinary tempo.
        for (units, label) in LFO_SYNC_ROWS.iter().skip(1) {
            let mut ph = Phaser::new();
            ph.set_sync_units(*units);
            assert_eq!(ph.sync_units(), *units, "{label} did not survive its setter");
            // Ungridded, so the rate is the division over the counted
            // beat with no servo trim on top of it.
            ph.prepare_block(&clock_ungridded(), 512.0 / 48_000.0);
            let expected = sync_cycles_per_beat(*units) as f32;
            assert!(
                (ph.active_hz - expected).abs() < 1e-4,
                "{label} landed on {} not {expected}",
                ph.active_hz
            );
        }
        // The top rung at a fast tempo is a real audio-rate number, and
        // it is passed through rather than trimmed.
        let mut ph = Phaser::new();
        ph.set_sync_units(LFO_SYNC_MAX_UNITS);
        ph.prepare_block(&clock_ungridded(), 512.0 / 48_000.0);
        assert!((ph.active_hz - 64.0).abs() < 1e-3, "{}", ph.active_hz);
    }

    /// Free-running, `prepare_block` leaves the rate alone: the Hz
    /// slider's value is what plays, whatever the deck's tempo is.
    #[test]
    fn a_free_running_tremolo_ignores_the_beat_clock() {
        let mut trem = Tremolo::new();
        trem.set_rate(4.0);
        let before = trem.active_hz;
        trem.prepare_block(&clock_at(3.0, 0.5), 512.0 / 48_000.0);
        assert_eq!(trem.active_hz, before);
    }

    /// A locked LFO does not just run at the right SPEED, it sits at the
    /// right PLACE: the servo leans on the rate until the phase agrees
    /// with the grid, and then stops leaning.
    #[test]
    fn a_locked_lfo_closes_on_the_grid_and_stays_there() {
        let rate = 48_000.0f32;
        let mut trem = Tremolo::new();
        trem.set_wet(1.0);
        // One cycle a beat, and a beat half a second long.
        trem.set_sync_units(8);
        let beat_secs = 0.5f64;
        let frames_per_buffer = 512usize;
        let secs_per_buffer = frames_per_buffer as f64 / rate as f64;

        // Start deliberately out of phase and run for a few seconds of
        // buffers, advancing the grid the same way the deck would.
        trem.phase = std::f32::consts::PI;
        let mut beat = 0.0f64;
        let mut error = 1.0f32;
        for _ in 0..400 {
            beat += secs_per_buffer / beat_secs;
            trem.prepare_block(&clock_at(beat, beat_secs), 512.0 / 48_000.0);
            for _ in 0..frames_per_buffer {
                trem.process([0.2, 0.2], rate);
            }
            let want = beat.rem_euclid(1.0) as f32;
            let have = (trem.phase / std::f32::consts::TAU).rem_euclid(1.0);
            error = (want - have).abs().min(1.0 - (want - have).abs());
        }
        assert!(error < 0.002, "the lock settled {error} of a cycle out");
    }

    /// The offset is where in the cycle the effect sits ON the beat, and
    /// the lock holds it there rather than at zero.
    #[test]
    fn a_locked_lfo_holds_the_offset_it_was_given() {
        let rate = 48_000.0f32;
        let mut trem = Tremolo::new();
        trem.set_wet(1.0);
        trem.set_sync_units(8);
        trem.set_beat_offset(0.25);
        let beat_secs = 0.5f64;
        let frames = 512usize;
        let per_buffer = frames as f64 / rate as f64;
        let mut beat = 0.0f64;
        let mut error = 1.0f32;
        for _ in 0..400 {
            beat += per_buffer / beat_secs;
            trem.prepare_block(&clock_at(beat, beat_secs), 512.0 / 48_000.0);
            for _ in 0..frames {
                trem.process([0.2, 0.2], rate);
            }
            let want = (beat + 0.25).rem_euclid(1.0) as f32;
            let have = (trem.phase / std::f32::consts::TAU).rem_euclid(1.0);
            error = (want - have).abs().min(1.0 - (want - have).abs());
        }
        assert!(error < 0.002, "the offset settled {error} of a cycle out");
    }

    /// Closing the lock must not step the output. The servo leans on the
    /// rate, never on the phase, and this is what says so: a tremolo's
    /// gain is a direct function of its phase, so a phase nudge would
    /// show here at once.
    #[test]
    fn closing_the_lock_does_not_step_the_output() {
        let rate = 48_000.0f32;
        let mut trem = Tremolo::new();
        trem.set_wet(1.0);
        trem.set_sync_units(8);
        // Half a cycle out: the worst the servo can be asked to close.
        trem.phase = std::f32::consts::PI;
        let beat_secs = 0.5f64;
        let frames = 512usize;
        let per_buffer = frames as f64 / rate as f64;
        let mut beat = 0.0f64;
        let mut prev: Option<f32> = None;
        let mut worst = 0.0f32;
        // A low tone, so its own slope does not swamp the measure.
        let mut phase = 0.0f32;
        for _ in 0..200 {
            beat += per_buffer / beat_secs;
            trem.prepare_block(&clock_at(beat, beat_secs), 512.0 / 48_000.0);
            for _ in 0..frames {
                phase += 2.0 * PI * 40.0 / rate;
                let x = phase.sin() * 0.5;
                let out = trem.process([x, x], rate)[0];
                if let Some(p) = prev {
                    worst = worst.max((out - p).abs());
                }
                prev = Some(out);
            }
        }
        assert!(worst < 0.02, "closing the lock stepped by {worst}");
    }

    /// The offset lands on the moment of engage, not before and not
    /// live -- so the wobble starts where the operator chose.
    #[test]
    fn a_locked_offset_lands_on_engage() {
        let mut trem = Tremolo::new();
        trem.set_sync_units(8);
        trem.set_beat_offset(0.25);
        assert_eq!(trem.phase, 0.0, "nothing moves before it engages");
        trem.set_wet(1.0);
        let quarter = 0.25 * std::f32::consts::TAU;
        assert!((trem.phase - quarter).abs() < 1e-6, "{}", trem.phase);
        // Moving it while already engaged does not jump the running LFO.
        let running = trem.phase;
        trem.set_beat_offset(0.75);
        assert_eq!(trem.phase, running);
    }

    /// Free-running, engaging never touches the phase -- today's
    /// behaviour, unchanged.
    #[test]
    fn a_free_running_engage_does_not_move_the_phase() {
        let mut trem = Tremolo::new();
        trem.set_beat_offset(0.5);
        trem.set_wet(1.0);
        assert_eq!(trem.phase, 0.0);
    }

    /// The offset clamps to its documented range.
    #[test]
    fn the_beat_offset_clamps_to_its_documented_range() {
        let mut trem = Tremolo::new();
        trem.set_beat_offset(5.0);
        assert_eq!(trem.beat_offset, 1.0);
        trem.set_beat_offset(-1.0);
        assert_eq!(trem.beat_offset, 0.0);
        trem.set_beat_offset(f32::NAN);
        assert_eq!(trem.beat_offset, 0.0, "a bad value moves nothing");
    }

    /// A rung past the ladder's top is held at the top rather than
    /// running away with the phase accumulator.
    #[test]
    fn a_rung_past_the_ladder_is_held_at_its_top() {
        let mut trem = Tremolo::new();
        trem.set_sync_units(u32::MAX);
        assert_eq!(trem.sync_units(), LFO_SYNC_MAX_UNITS);
    }

    /// All four LFO effects carry the same lock, and each starts
    /// free-running so nothing an operator already set up changes.
    #[test]
    fn every_lfo_effect_starts_free_running() {
        assert_eq!(Tremolo::new().sync_units(), LFO_SYNC_FREE);
        assert_eq!(Autopan::new().sync_units(), LFO_SYNC_FREE);
        assert_eq!(Flanger::new().sync_units(), LFO_SYNC_FREE);
        assert_eq!(Phaser::new().sync_units(), LFO_SYNC_FREE);
    }

    /// Off, a fresh unit is exactly its input.
    #[test]
    fn a_distortion_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut dist = Distortion::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(dist.process([x, -x], rate), [x, -x]);
        }
    }

    /// The implementation must faithfully compute what it documents:
    /// `pade_tanh(frame * drive) * makeup`, with
    /// `makeup = 1 / pade_tanh(drive)`. Cross-checks the compiled output
    /// against the formula computed independently in the test, the same
    /// way `bitcrusher_quantizes_to_the_documented_level_count` checks
    /// its own unit's formula.
    #[test]
    fn distortion_output_matches_its_own_shaping_formula() {
        let rate = 48_000.0f32;
        let mut dist = Distortion::new();
        dist.set_wet(1.0);
        dist.set_drive(6.0);
        for _ in 0..SETTLE_FRAMES {
            dist.process([0.0, 0.0], rate);
        }
        let drive = 6.0f32;
        for i in 0..200usize {
            let x = -0.9 + 1.8 * (i as f32 / 200.0);
            let out = dist.process([x, x], rate)[0];
            // The makeup is measured now rather than derived from the
            // drive, so the shape is checked against the correction
            // actually in force after that frame -- `process` updates it
            // and then multiplies by it.
            let expected = pade_tanh(x * drive) * dist.makeup;
            assert!((out - expected).abs() < 1e-4, "{out} vs {expected} at x={x}");
        }
    }

    /// Drive is a control over character, not over volume. The makeup
    /// used to be derived from the drive rather than measured, which
    /// held a full-scale input at full scale and let the whole pre-gain
    /// through as loudness on everything quieter -- +1.9 dB at drive
    /// 1.0, where the effect should be transparent, and +9 dB at the
    /// default. Across the whole range the level now holds, while the
    /// PEAK falls as the shaper compresses the crest, which is the
    /// saturation doing its job.
    #[test]
    fn distortion_drive_changes_the_character_not_the_level() {
        let rate = 48_000.0f32;
        for drive in [
            DISTORTION_DRIVE_MIN,
            2.0,
            DISTORTION_DRIVE_DEFAULT,
            8.0,
            DISTORTION_DRIVE_MAX,
        ] {
            let mut dist = Distortion::new();
            dist.set_wet(1.0);
            dist.set_drive(drive);
            let mut phase = 0.0f32;
            let mut tone = move || {
                phase += 2.0 * PI * 220.0 / rate;
                phase.sin() * 0.35
            };
            // Let the engage ramp and the level meters settle.
            for _ in 0..(rate as usize / 4) {
                let x = tone();
                dist.process([x, x], rate);
            }
            let (mut dry_sq, mut wet_sq) = (0.0f64, 0.0f64);
            let frames = rate as usize / 2;
            for _ in 0..frames {
                let x = tone();
                let out = dist.process([x, x], rate)[0];
                dry_sq += (x as f64) * (x as f64);
                wet_sq += (out as f64) * (out as f64);
            }
            let db = 20.0 * (wet_sq / dry_sq).sqrt().log10();
            assert!(
                db.abs() < 1.0,
                "drive {drive} moved the level by {db:.2} dB"
            );
        }
    }

    /// No feedback path to diverge, but bounded and finite is worth
    /// locking in at the harshest settings, the same property every
    /// other effect in this file is held to.
    #[test]
    fn distortion_stays_bounded_at_the_harshest_settings() {
        let rate = 48_000.0f32;
        let mut dist = Distortion::new();
        dist.set_wet(1.0);
        dist.set_drive(DISTORTION_DRIVE_MAX);
        for _ in 0..SETTLE_FRAMES {
            dist.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..48_000usize {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 1.5;
            let out = dist.process([x, x], rate);
            assert!(out[0].is_finite() && out[0].abs() < 8.0, "unbounded: {out:?}");
            assert!(out[1].is_finite() && out[1].abs() < 8.0, "unbounded: {out:?}");
        }
    }

    /// No line, no history: disengage completes exactly the ramp's own
    /// duration after `set_wet(0.0)`.
    #[test]
    fn switching_the_distortion_off_returns_to_bit_exact_bypass_after_its_ramp() {
        let rate = 48_000.0f32;
        let mut dist = Distortion::new();
        dist.set_wet(1.0);
        for _ in 0..SETTLE_FRAMES {
            dist.process([0.5, 0.5], rate);
        }
        dist.set_wet(0.0);
        for _ in 0..1_000 {
            dist.process([0.5, 0.5], rate);
        }
        assert!(!dist.engaged());
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(
                dist.process([x, -x], rate),
                [x, -x],
                "a bit-exact bypass, not merely quiet"
            );
        }
    }

    #[test]
    fn distortion_setters_clamp_to_their_documented_ranges() {
        let mut dist = Distortion::new();
        dist.set_wet(f32::NAN);
        assert_eq!(dist.wet.target(), 0.0, "a bad value moves nothing");
        dist.set_wet(5.0);
        assert_eq!(dist.wet.target(), 1.0);
        dist.set_wet(-5.0);
        assert_eq!(dist.wet.target(), 0.0);
        dist.set_drive(f32::INFINITY);
        assert_eq!(dist.drive.target(), DISTORTION_DRIVE_DEFAULT, "unmoved by a bad value");
        dist.set_drive(1_000.0);
        assert_eq!(dist.drive.target(), DISTORTION_DRIVE_MAX);
        dist.set_drive(-1.0);
        assert_eq!(dist.drive.target(), DISTORTION_DRIVE_MIN);
    }

    /// The property a first-order allpass is built to have: unity gain
    /// at every frequency, only phase moves. If the coefficient formula
    /// were wrong this would show up as a gain drift, not just a wrong
    /// phase -- exactly the mistake that would otherwise slip past
    /// every other test here, which only look at the FULL phaser
    /// (dry + allpassed), where an allpass gain error would just read
    /// as "a different notch shape" rather than a clear failure.
    #[test]
    fn a_single_allpass_stage_has_unity_gain() {
        let rate = 48_000.0f32;
        let corner = 800.0f32;
        let t = (PI * corner / rate).tan();
        let a = (t - 1.0) / (t + 1.0);
        let mut stage = AllpassStage::default();
        let mut phase = 0.0f32;
        // Past the filter's own settling transient before measuring.
        for _ in 0..2_000 {
            phase += 2.0 * PI * 440.0 / rate;
            stage.process(phase.sin(), a, 0);
        }
        let mut in_energy = 0.0f64;
        let mut out_energy = 0.0f64;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 440.0 / rate;
            let x = phase.sin();
            let y = stage.process(x, a, 0);
            in_energy += (x as f64) * (x as f64);
            out_energy += (y as f64) * (y as f64);
        }
        let ratio = (out_energy / in_energy).sqrt();
        assert!((ratio - 1.0).abs() < 0.01, "allpass gain drifted from unity: {ratio}");
    }

    /// Off, a fresh unit is exactly its input.
    #[test]
    fn a_phaser_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut ph = Phaser::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(ph.process([x, -x], rate), [x, -x]);
        }
    }

    /// Cranked feedback saturates through the shared soft clip rather
    /// than diverging, the same property every other regeneration path
    /// in this file is held to.
    #[test]
    fn phaser_stays_bounded_at_the_harshest_feedback() {
        let rate = 48_000.0f32;
        let mut ph = Phaser::new();
        ph.set_wet(1.0);
        ph.set_feedback(PHASER_FEEDBACK_MAX);
        ph.set_rate(PHASER_RATE_MAX);
        for _ in 0..SETTLE_FRAMES {
            ph.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..48_000usize {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            let out = ph.process([x, x], rate);
            assert!(out[0].is_finite() && out[0].abs() < 8.0, "unbounded: {out:?}");
            assert!(out[1].is_finite() && out[1].abs() < 8.0, "unbounded: {out:?}");
        }
    }

    /// No ring-out tracking, the same simple bypass `DeckEq` itself
    /// uses despite carrying persistent filter state: disengage
    /// completes exactly the ramp's own duration after `set_wet(0.0)`.
    #[test]
    fn switching_the_phaser_off_returns_to_bit_exact_bypass_after_its_ramp() {
        let rate = 48_000.0f32;
        let mut ph = Phaser::new();
        ph.set_wet(1.0);
        for _ in 0..SETTLE_FRAMES {
            ph.process([0.5, 0.5], rate);
        }
        ph.set_wet(0.0);
        for _ in 0..1_000 {
            ph.process([0.5, 0.5], rate);
        }
        assert!(!ph.engaged());
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(
                ph.process([x, -x], rate),
                [x, -x],
                "a bit-exact bypass, not merely quiet"
            );
        }
    }

    /// `reset`, unlike the echo's or the flanger's `silence`, clears the
    /// filter memory directly -- a handful of floats, not a buffer, the
    /// same reason `DeckEq::reset` is shaped this way.
    #[test]
    fn phaser_reset_clears_its_filter_memory() {
        let rate = 48_000.0f32;
        let mut ph = Phaser::new();
        ph.set_wet(1.0);
        ph.set_feedback(0.3);
        for _ in 0..SETTLE_FRAMES {
            ph.process([0.6, -0.6], rate);
        }
        assert_ne!(ph.stages[0].x_prev, [0.0, 0.0], "real filter memory is there to clear");
        ph.reset();
        for stage in &ph.stages {
            assert_eq!(stage.x_prev, [0.0, 0.0]);
            assert_eq!(stage.y_prev, [0.0, 0.0]);
        }
        assert_eq!(ph.last_output, [0.0, 0.0]);
    }

    #[test]
    fn phaser_setters_clamp_to_their_documented_ranges() {
        let mut ph = Phaser::new();
        ph.set_wet(f32::NAN);
        assert_eq!(ph.wet.target(), 0.0, "a bad value moves nothing");
        ph.set_wet(5.0);
        assert_eq!(ph.wet.target(), 1.0);
        ph.set_wet(-5.0);
        assert_eq!(ph.wet.target(), 0.0);
        ph.set_rate(f32::INFINITY);
        assert_eq!(ph.rate.target(), PHASER_RATE_DEFAULT, "unmoved by a bad value");
        ph.set_rate(1_000.0);
        assert_eq!(ph.rate.target(), PHASER_RATE_MAX);
        ph.set_rate(-1.0);
        assert_eq!(ph.rate.target(), PHASER_RATE_MIN);
        ph.set_feedback(5.0);
        assert_eq!(ph.feedback.target(), PHASER_FEEDBACK_MAX);
        ph.set_feedback(-5.0);
        assert_eq!(ph.feedback.target(), 0.0);
    }

    /// Off, a fresh unit is exactly its input.
    #[test]
    fn an_autopan_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut ap = Autopan::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(ap.process([x, -x], rate), [x, -x]);
        }
    }

    /// The property the `sqrt(2)` normalization exists for: depth 0
    /// must be a true no-op, not merely "not moving" -- unlike the
    /// tremolo's rest gain (already 1.0 at the top of its range), a
    /// pan law's natural centre is -3 dB per channel, and a design that
    /// left that uncorrected would make `depth = 0` an audible, silent
    /// bug: a constant attenuation with no motion to explain it.
    #[test]
    fn autopan_at_zero_depth_is_transparent_regardless_of_wet() {
        let rate = 48_000.0f32;
        let mut ap = Autopan::new();
        ap.set_wet(1.0);
        ap.set_depth(0.0);
        for _ in 0..SETTLE_FRAMES {
            ap.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let x = phase.sin() * 0.6;
            let out = ap.process([x, -x], rate);
            assert!((out[0] - x).abs() < 1e-4, "{out:?} vs [{x}, {}]", -x);
            assert!((out[1] + x).abs() < 1e-4, "{out:?} vs [{x}, {}]", -x);
        }
    }

    /// The defining property of an equal-power pan law: identical
    /// content on both channels keeps the same TOTAL power as the pan
    /// sweeps between them -- it moves the sound, it does not change
    /// how loud it is.
    #[test]
    fn an_engaged_autopan_preserves_total_power() {
        let rate = 48_000.0f32;
        let mut ap = Autopan::new();
        ap.set_wet(1.0);
        ap.set_depth(1.0);
        ap.set_rate(3.0);
        for _ in 0..SETTLE_FRAMES {
            ap.process([0.0, 0.0], rate);
        }
        let x = 0.7f32;
        let expected = 2.0 * x * x;
        for _ in 0..48_000usize {
            let out = ap.process([x, x], rate);
            let power = out[0] * out[0] + out[1] * out[1];
            assert!((power - expected).abs() < 1e-4, "power drifted: {power} vs {expected}");
        }
    }

    /// No feedback path, but bounded and finite is worth locking in at
    /// the harshest settings, the same property every other effect in
    /// this file is held to.
    #[test]
    fn autopan_stays_bounded_at_the_harshest_settings() {
        let rate = 48_000.0f32;
        let mut ap = Autopan::new();
        ap.set_wet(1.0);
        ap.set_depth(1.0);
        ap.set_rate(AUTOPAN_RATE_MAX);
        for _ in 0..SETTLE_FRAMES {
            ap.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..48_000usize {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            let out = ap.process([x, x], rate);
            assert!(out[0].is_finite() && out[0].abs() < 3.0, "unbounded: {out:?}");
            assert!(out[1].is_finite() && out[1].abs() < 3.0, "unbounded: {out:?}");
        }
    }

    /// No line, no history: disengage completes exactly the ramp's own
    /// duration after `set_wet(0.0)`.
    #[test]
    fn switching_the_autopan_off_returns_to_bit_exact_bypass_after_its_ramp() {
        let rate = 48_000.0f32;
        let mut ap = Autopan::new();
        ap.set_wet(1.0);
        for _ in 0..SETTLE_FRAMES {
            ap.process([0.5, 0.5], rate);
        }
        ap.set_wet(0.0);
        for _ in 0..1_000 {
            ap.process([0.5, 0.5], rate);
        }
        assert!(!ap.engaged());
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(
                ap.process([x, -x], rate),
                [x, -x],
                "a bit-exact bypass, not merely quiet"
            );
        }
    }

    #[test]
    fn autopan_setters_clamp_to_their_documented_ranges() {
        let mut ap = Autopan::new();
        ap.set_wet(f32::NAN);
        assert_eq!(ap.wet.target(), 0.0, "a bad value moves nothing");
        ap.set_wet(5.0);
        assert_eq!(ap.wet.target(), 1.0);
        ap.set_wet(-5.0);
        assert_eq!(ap.wet.target(), 0.0);
        ap.set_rate(f32::INFINITY);
        assert_eq!(ap.rate.target(), AUTOPAN_RATE_DEFAULT, "unmoved by a bad value");
        ap.set_rate(1_000.0);
        assert_eq!(ap.rate.target(), AUTOPAN_RATE_MAX);
        ap.set_rate(-1.0);
        assert_eq!(ap.rate.target(), AUTOPAN_RATE_MIN);
        ap.set_depth(2.0);
        assert_eq!(ap.depth.target(), 1.0);
        ap.set_depth(-1.0);
        assert_eq!(ap.depth.target(), 0.0);
    }

    /// Off, a fresh unit is exactly its input.
    #[test]
    fn a_stereo_width_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut sw = StereoWidth::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let l = phase.sin() * 0.6;
            let r = (phase * 1.7).cos() * 0.4;
            assert_eq!(sw.process([l, r], rate), [l, r]);
        }
    }

    /// The property `width = 1.0` exists for: mid + side and mid - side
    /// reconstruct the original left and right exactly, so the neutral
    /// point of the knob is a true no-op regardless of how engaged the
    /// effect is -- unlike a filter or a pan law, there is no natural
    /// "centre" coloration to correct for here, so this has to hold
    /// exactly, not merely approximately.
    #[test]
    fn stereo_width_at_unity_is_transparent_when_engaged() {
        let rate = 48_000.0f32;
        let mut sw = StereoWidth::new();
        sw.set_wet(1.0);
        sw.set_width(1.0);
        for _ in 0..SETTLE_FRAMES {
            sw.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let l = phase.sin() * 0.6;
            let r = (phase * 1.7).cos() * 0.4;
            let out = sw.process([l, r], rate);
            assert!((out[0] - l).abs() < 1e-4, "{out:?} vs [{l}, {r}]");
            assert!((out[1] - r).abs() < 1e-4, "{out:?} vs [{l}, {r}]");
        }
    }

    /// The defining property at the other end of the range: `width = 0`
    /// collapses both channels onto the mid signal, a genuinely mono
    /// output, not merely a quieter or narrower one.
    #[test]
    fn stereo_width_at_zero_collapses_to_mono() {
        let rate = 48_000.0f32;
        let mut sw = StereoWidth::new();
        sw.set_wet(1.0);
        sw.set_width(0.0);
        for _ in 0..SETTLE_FRAMES {
            sw.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 37.0 / rate;
            let l = phase.sin() * 0.6;
            let r = (phase * 1.7).cos() * 0.4;
            let out = sw.process([l, r], rate);
            let mid = (l + r) * 0.5;
            assert!((out[0] - mid).abs() < 1e-4, "{out:?} vs mid {mid}");
            assert!((out[1] - mid).abs() < 1e-4, "{out:?} vs mid {mid}");
            assert!((out[0] - out[1]).abs() < 1e-6, "not mono: {out:?}");
        }
    }

    /// Bounded and finite at the harshest settings -- a fully
    /// out-of-phase source at the top of the width range, the one case
    /// the doc comment calls out as able to exceed unity on its own.
    #[test]
    fn stereo_width_stays_bounded_at_the_harshest_settings() {
        let rate = 48_000.0f32;
        let mut sw = StereoWidth::new();
        sw.set_wet(1.0);
        sw.set_width(STEREO_WIDTH_MAX);
        for _ in 0..SETTLE_FRAMES {
            sw.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..48_000usize {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            let out = sw.process([x, -x], rate);
            assert!(out[0].is_finite() && out[0].abs() < 4.0, "unbounded: {out:?}");
            assert!(out[1].is_finite() && out[1].abs() < 4.0, "unbounded: {out:?}");
        }
    }

    /// No line, no history: disengage completes exactly the ramp's own
    /// duration after `set_wet(0.0)`.
    #[test]
    fn switching_the_stereo_width_off_returns_to_bit_exact_bypass_after_its_ramp() {
        let rate = 48_000.0f32;
        let mut sw = StereoWidth::new();
        sw.set_wet(1.0);
        for _ in 0..SETTLE_FRAMES {
            sw.process([0.5, 0.2], rate);
        }
        sw.set_wet(0.0);
        for _ in 0..1_000 {
            sw.process([0.5, 0.2], rate);
        }
        assert!(!sw.engaged());
        for i in 0..1_000 {
            let l = (i as f32 * 0.037).sin() * 0.5;
            let r = (i as f32 * 0.061).cos() * 0.3;
            assert_eq!(
                sw.process([l, r], rate),
                [l, r],
                "a bit-exact bypass, not merely quiet"
            );
        }
    }

    #[test]
    fn stereo_width_setters_clamp_to_their_documented_ranges() {
        let mut sw = StereoWidth::new();
        sw.set_wet(f32::NAN);
        assert_eq!(sw.wet.target(), 0.0, "a bad value moves nothing");
        sw.set_wet(5.0);
        assert_eq!(sw.wet.target(), 1.0);
        sw.set_wet(-5.0);
        assert_eq!(sw.wet.target(), 0.0);
        sw.set_width(f32::INFINITY);
        assert_eq!(sw.width.target(), STEREO_WIDTH_DEFAULT, "unmoved by a bad value");
        sw.set_width(10.0);
        assert_eq!(sw.width.target(), STEREO_WIDTH_MAX);
        sw.set_width(-5.0);
        assert_eq!(sw.width.target(), STEREO_WIDTH_MIN);
    }

    /// Off, a fresh unit is exactly its input.
    #[test]
    fn a_plate_reverb_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut pr = PlateReverb::new(rate);
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(pr.process([x, -x], rate), [x, -x]);
        }
    }

    /// The two tanks are built from different comb lengths so the tail
    /// decorrelates between channels -- a mono send through identical
    /// tanks would read as a mono reverb panned to both speakers, not a
    /// stereo one. Feeding a mono source in should still come back with
    /// audibly different left and right tails.
    #[test]
    fn the_two_tanks_decorrelate_a_mono_source() {
        let rate = 48_000.0f32;
        let mut pr = PlateReverb::new(rate);
        pr.set_wet(1.0);
        pr.set_size(0.8);
        let mut phase = 0.0f32;
        let mut saw_difference = false;
        for _ in 0..24_000usize {
            phase += 2.0 * PI * 137.0 / rate;
            let x = phase.sin() * 0.5;
            let out = pr.process([x, x], rate);
            if (out[0] - out[1]).abs() > 1e-4 {
                saw_difference = true;
            }
        }
        assert!(saw_difference, "left and right tails never diverged");
    }

    /// Bounded and finite at the harshest setting the knob reaches,
    /// sustained well past any reasonable tail length -- the same
    /// property every other effect in this file is held to, and the one
    /// a comb/allpass network's stability actually depends on: every
    /// feedback coefficient in range stays under 1.0.
    #[test]
    fn plate_reverb_stays_bounded_at_the_harshest_settings() {
        let rate = 48_000.0f32;
        let mut pr = PlateReverb::new(rate);
        pr.set_wet(1.0);
        pr.set_size(PLATE_REVERB_SIZE_MAX);
        let mut phase = 0.0f32;
        for _ in 0..(rate as usize * 2) {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            let out = pr.process([x, x], rate);
            assert!(out[0].is_finite() && out[0].abs() < 8.0, "unbounded: {out:?}");
            assert!(out[1].is_finite() && out[1].abs() < 8.0, "unbounded: {out:?}");
        }
    }

    /// The property the `quiet` tracking exists for: releasing the
    /// reverb must not chop its tail off the instant the engage ramp
    /// finishes. Feed an impulse in while engaged, then disengage --
    /// the tank must still be audibly ringing for a while after the
    /// (short) engage ramp completes, and only reach bit-exact bypass
    /// once the tail has genuinely decayed away.
    #[test]
    fn switching_the_plate_reverb_off_lets_its_tail_ring_out_before_bypass() {
        let rate = 48_000.0f32;
        let mut pr = PlateReverb::new(rate);
        pr.set_size(0.9);
        pr.set_wet(1.0);
        // Let the engage ramp finish BEFORE firing the impulse: the
        // write is scaled by `wet`, so an impulse fired mid-ramp would
        // barely enter the tank at all and this test would be checking
        // almost nothing.
        for _ in 0..1_000 {
            pr.process([0.0, 0.0], rate);
        }
        // An impulse, not a tone: a short, sharp excitation gives the
        // tank something to ring with that a settle loop of silence
        // would not.
        pr.process([1.0, 1.0], rate);
        // Longer than the longest comb delay (~45 ms, ~2 155 frames at
        // this rate) so every tap has read the impulse back at least
        // once before disengaging.
        for _ in 0..3_000 {
            pr.process([0.0, 0.0], rate);
        }
        pr.set_wet(0.0);
        // Run PAST the engage ramp's own duration first, so `wet` itself
        // has already reached exactly zero -- only the `quiet` tracking
        // is left standing between here and bit-exact bypass. A version
        // that bypassed on `wet == 0.0` alone, ignoring `quiet`, behaves
        // identically to the correct one during the ramp itself (both
        // still have wet > 0 partway through it), so the ramp window is
        // not where this property is actually observable.
        let ramp_frames = (EQ_ENGAGE_SECS * rate) as usize + 8;
        for _ in 0..ramp_frames {
            pr.process([0.0, 0.0], rate);
        }
        assert!(!pr.engaged());
        // Immediately past the ramp: if the tail were cut the instant
        // `wet` reached zero rather than left to decay on its own, every
        // one of these frames would already be an exact zero.
        let mut still_ringing = false;
        for _ in 0..2_000 {
            let out = pr.process([0.0, 0.0], rate);
            if out[0].abs() > 1e-4 || out[1].abs() > 1e-4 {
                still_ringing = true;
            }
        }
        assert!(still_ringing, "the tail was cut the instant the ramp finished");
        // Long enough for this test's own size=0.9 (feedback ~0.93) to
        // genuinely fall under the quiet threshold, now that `quiet`
        // reflects a full window's peak rather than one lucky sample --
        // the honest version takes noticeably longer than the bug it
        // replaced did, which is the whole point of the fix.
        for _ in 0..(rate as usize * 6) {
            pr.process([0.0, 0.0], rate);
        }
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(
                pr.process([x, -x], rate),
                [x, -x],
                "a bit-exact bypass once the tail has actually rung out"
            );
        }
    }

    /// Regression: `quiet` used to be re-derived from a single raw
    /// sample every call. A comb/allpass tank's release-phase output
    /// oscillates as it decays -- sparse and gappy, not a smooth
    /// envelope -- so individual samples routinely dip under
    /// [`PLATE_REVERB_QUIET`] long before the true (windowed) envelope
    /// has. This proves `quiet` survives at least one such dip: it
    /// must still be `false` on some frame whose own raw output is
    /// already under threshold, since latching on that frame alone
    /// would freeze the tank mid-decay and chop off real tail content.
    #[test]
    fn quiet_survives_a_raw_sample_dipping_under_threshold() {
        let rate = 48_000.0f32;
        let mut pr = PlateReverb::new(rate);
        pr.set_size(0.9);
        pr.set_wet(1.0);
        for _ in 0..1_000 {
            pr.process([0.0, 0.0], rate);
        }
        pr.process([1.0, 1.0], rate);
        for _ in 0..3_000 {
            pr.process([0.0, 0.0], rate);
        }
        pr.set_wet(0.0);
        let ramp_frames = (EQ_ENGAGE_SECS * rate) as usize + 8;
        for _ in 0..ramp_frames {
            pr.process([0.0, 0.0], rate);
        }
        assert!(!pr.engaged());
        let mut saw_undercross_while_still_not_quiet = false;
        for _ in 0..4_000 {
            let out = pr.process([0.0, 0.0], rate);
            let raw_under = out[0].abs() < PLATE_REVERB_QUIET && out[1].abs() < PLATE_REVERB_QUIET;
            if raw_under && !pr.quiet {
                saw_undercross_while_still_not_quiet = true;
            }
            if pr.quiet {
                break;
            }
        }
        assert!(
            saw_undercross_while_still_not_quiet,
            "no frame had a raw under-threshold sample while quiet was still false -- \
             either this scenario never dips that low within the window (weak test \
             setup) or quiet is latching on the very first such sample, the bug this \
             test targets"
        );
    }

    /// Regression: `quiet` must not stay stale-true across a brief
    /// re-engage. A press-release-press-release cycle faster than one
    /// [`PLATE_REVERB_QUIET_PERIOD_MS`] window never gives the periodic
    /// check in `process` a chance to run, so `set_wet` has to clear a
    /// stale flag itself the moment it engages -- otherwise fresh,
    /// un-decayed energy from the second engagement would bypass
    /// immediately on the flag left over from the first release.
    #[test]
    fn a_brief_re_engage_clears_a_stale_quiet_flag() {
        let rate = 48_000.0f32;
        let mut pr = PlateReverb::new(rate);
        pr.set_size(0.9);
        // Reach genuine, fully-decayed quiet first -- no impulse fed
        // in, so this settles almost immediately.
        pr.set_wet(1.0);
        for _ in 0..1_000 {
            pr.process([0.0, 0.0], rate);
        }
        pr.set_wet(0.0);
        for _ in 0..rate as usize {
            pr.process([0.0, 0.0], rate);
        }
        assert!(pr.quiet, "test setup: the tank should have genuinely quieted by now");

        // Re-engage and feed an impulse, then hold long enough for
        // every comb tap to have read it back at least once (longer
        // than the longest comb's ~45ms/~2155-frame delay) before
        // disengaging again -- still comfortably under one
        // quiet-period window, since the point is a brief engagement,
        // not a long one.
        pr.set_wet(1.0);
        pr.process([1.0, 1.0], rate);
        for _ in 0..3_000 {
            pr.process([0.0, 0.0], rate);
        }
        pr.set_wet(0.0);
        let ramp_frames = (EQ_ENGAGE_SECS * rate) as usize + 8;
        for _ in 0..ramp_frames {
            pr.process([0.0, 0.0], rate);
        }
        assert!(!pr.engaged());
        let mut still_ringing = false;
        for _ in 0..500 {
            let out = pr.process([0.0, 0.0], rate);
            if out[0].abs() > 1e-4 || out[1].abs() > 1e-4 {
                still_ringing = true;
            }
        }
        assert!(still_ringing, "a stale quiet flag from before the re-engage froze the fresh tail");
    }

    /// `silence` drops the tank's content directly, without waiting for
    /// it to ring out on its own -- the record-change path every other
    /// delay-holding effect in this file relies on.
    #[test]
    fn silence_lets_a_ringing_tank_bypass_immediately() {
        let rate = 48_000.0f32;
        let mut pr = PlateReverb::new(rate);
        pr.set_wet(1.0);
        pr.set_size(0.9);
        pr.process([1.0, 1.0], rate);
        for _ in 0..2_000 {
            pr.process([0.0, 0.0], rate);
        }
        pr.set_wet(0.0);
        pr.silence();
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(pr.process([x, -x], rate), [x, -x], "silence must drop the tank at once");
        }
    }

    #[test]
    fn plate_reverb_setters_clamp_to_their_documented_ranges() {
        let mut pr = PlateReverb::new(48_000.0);
        pr.set_wet(f32::NAN);
        assert_eq!(pr.wet.target(), 0.0, "a bad value moves nothing");
        pr.set_wet(5.0);
        assert_eq!(pr.wet.target(), 1.0);
        pr.set_wet(-5.0);
        assert_eq!(pr.wet.target(), 0.0);
        pr.set_size(f32::INFINITY);
        assert_eq!(pr.size.target(), PLATE_REVERB_SIZE_DEFAULT, "unmoved by a bad value");
        pr.set_size(10.0);
        assert_eq!(pr.size.target(), PLATE_REVERB_SIZE_MAX);
        pr.set_size(-5.0);
        assert_eq!(pr.size.target(), PLATE_REVERB_SIZE_MIN);
    }

    /// A device rate change is a rare, hard-reset-worthy event: the
    /// tank's lines are rebuilt to the new rate's frame lengths and any
    /// stored content is gone, so a unit already ringing must land back
    /// on bit-exact bypass once its (now-silent) tank is asked to keep
    /// ringing at the old rate's stale content.
    #[test]
    fn a_sample_rate_change_rebuilds_the_tank_and_drops_its_content() {
        let mut pr = PlateReverb::new(48_000.0);
        pr.set_wet(1.0);
        pr.set_size(0.9);
        pr.process([1.0, 1.0], 48_000.0);
        for _ in 0..2_000 {
            pr.process([0.0, 0.0], 48_000.0);
        }
        pr.set_sample_rate(44_100.0);
        assert!(pr.quiet, "a rebuilt tank starts with nothing to ring");
    }

    /// Off, a fresh unit is exactly its input.
    #[test]
    fn a_moog_ladder_that_was_never_engaged_is_bit_transparent() {
        let rate = 48_000.0f32;
        let mut ml = MoogLadder::new();
        let mut phase = 0.0f32;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.6;
            assert_eq!(ml.process([x, -x], rate), [x, -x]);
        }
    }

    /// The defining property of a lowpass: with a low cutoff and no
    /// resonance to boost anything back up, a high tone comes out
    /// heavily attenuated relative to what went in.
    #[test]
    fn a_low_cutoff_attenuates_a_high_tone() {
        let rate = 48_000.0f32;
        let mut ml = MoogLadder::new();
        ml.set_wet(1.0);
        ml.set_cutoff(200.0);
        ml.set_resonance(0.0);
        for _ in 0..SETTLE_FRAMES {
            ml.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        let (mut in_energy, mut out_energy) = (0.0f64, 0.0f64);
        for _ in 0..4_000 {
            phase += 2.0 * PI * 8_000.0 / rate;
            let x = phase.sin() * 0.7;
            let out = ml.process([x, x], rate);
            in_energy += (x as f64) * (x as f64);
            out_energy += (out[0] as f64) * (out[0] as f64);
        }
        assert!(
            out_energy < in_energy * 0.05,
            "an 8kHz tone through a 200Hz lowpass barely attenuated: in {in_energy} out {out_energy}"
        );
    }

    /// Bounded and finite even at the edge of self-oscillation: the
    /// property the feedback path's `pade_tanh` saturation exists to
    /// guarantee, the same way the real transistor ladder's own
    /// saturation keeps IT from diverging.
    #[test]
    fn moog_ladder_stays_bounded_at_the_harshest_settings() {
        let rate = 48_000.0f32;
        let mut ml = MoogLadder::new();
        ml.set_wet(1.0);
        ml.set_cutoff(MOOG_LADDER_CUTOFF_MAX);
        ml.set_resonance(MOOG_LADDER_RESONANCE_MAX);
        for _ in 0..SETTLE_FRAMES {
            ml.process([0.0, 0.0], rate);
        }
        let mut phase = 0.0f32;
        for _ in 0..48_000usize {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            let out = ml.process([x, x], rate);
            assert!(out[0].is_finite() && out[0].abs() < 8.0, "unbounded: {out:?}");
            assert!(out[1].is_finite() && out[1].abs() < 8.0, "unbounded: {out:?}");
        }
    }

    /// Filter memory only, no line: disengage completes exactly the
    /// ramp's own duration after `set_wet(0.0)`.
    #[test]
    fn switching_the_moog_ladder_off_returns_to_bit_exact_bypass_after_its_ramp() {
        let rate = 48_000.0f32;
        let mut ml = MoogLadder::new();
        ml.set_wet(1.0);
        for _ in 0..SETTLE_FRAMES {
            ml.process([0.5, 0.5], rate);
        }
        ml.set_wet(0.0);
        for _ in 0..1_000 {
            ml.process([0.5, 0.5], rate);
        }
        assert!(!ml.engaged());
        for i in 0..1_000 {
            let x = (i as f32 * 0.037).sin() * 0.5;
            assert_eq!(
                ml.process([x, -x], rate),
                [x, -x],
                "a bit-exact bypass, not merely quiet"
            );
        }
    }

    #[test]
    fn moog_ladder_setters_clamp_to_their_documented_ranges() {
        let mut ml = MoogLadder::new();
        ml.set_wet(f32::NAN);
        assert_eq!(ml.wet.target(), 0.0, "a bad value moves nothing");
        ml.set_wet(5.0);
        assert_eq!(ml.wet.target(), 1.0);
        ml.set_wet(-5.0);
        assert_eq!(ml.wet.target(), 0.0);
        ml.set_cutoff(f32::INFINITY);
        assert_eq!(ml.cutoff.target(), MOOG_LADDER_CUTOFF_DEFAULT, "unmoved by a bad value");
        ml.set_cutoff(50_000.0);
        assert_eq!(ml.cutoff.target(), MOOG_LADDER_CUTOFF_MAX);
        ml.set_cutoff(-1.0);
        assert_eq!(ml.cutoff.target(), MOOG_LADDER_CUTOFF_MIN);
        ml.set_resonance(5.0);
        assert_eq!(ml.resonance.target(), MOOG_LADDER_RESONANCE_MAX);
        ml.set_resonance(-5.0);
        assert_eq!(ml.resonance.target(), MOOG_LADDER_RESONANCE_MIN);
    }

    /// Regression: a single large `set_cutoff()` call while engaged used
    /// to click, found by an adversarial review (worst step 0.052 against
    /// this file's own 0.02 CLICK budget, from an entirely ordinary
    /// slider drag). The coefficient itself is always continuous, but
    /// the cascade's own state can lag far enough behind a fast-moving
    /// coefficient that the OUTPUT still steps -- fixed by widening the
    /// ramp the same way `DeckEq::set_filter` already had to for its own
    /// swept corner. Reproduces the review's own worst case: settled at
    /// the floor cutoff (heavily attenuating) with resonance already at
    /// its max, then one jump straight to the ceiling.
    #[test]
    fn a_large_cutoff_jump_while_engaged_does_not_click() {
        let rate = 48_000.0f32;
        let mut ml = MoogLadder::new();
        ml.set_wet(1.0);
        ml.set_cutoff(MOOG_LADDER_CUTOFF_MIN);
        ml.set_resonance(MOOG_LADDER_RESONANCE_MAX);
        let mut phase = 0.0f32;
        for _ in 0..SETTLE_FRAMES {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            ml.process([x, x], rate);
        }
        ml.set_cutoff(MOOG_LADDER_CUTOFF_MAX);
        let mut worst = 0.0f32;
        let mut prev: Option<f32> = None;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            let out = ml.process([x, x], rate);
            if let Some(p) = prev {
                worst = worst.max((out[0] - p).abs());
            }
            prev = Some(out[0]);
        }
        assert!(worst < 0.02, "a large cutoff jump while engaged clicked, worst step {worst}");
    }

    /// Same property, the resonance knob: `k` scales the feedback tap
    /// directly, so a fast excursion while the tap is already large can
    /// also step the output.
    #[test]
    fn a_large_resonance_jump_while_engaged_does_not_click() {
        let rate = 48_000.0f32;
        let mut ml = MoogLadder::new();
        ml.set_wet(1.0);
        ml.set_cutoff(MOOG_LADDER_CUTOFF_MIN);
        ml.set_resonance(MOOG_LADDER_RESONANCE_MIN);
        let mut phase = 0.0f32;
        for _ in 0..SETTLE_FRAMES {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            ml.process([x, x], rate);
        }
        ml.set_resonance(MOOG_LADDER_RESONANCE_MAX);
        let mut worst = 0.0f32;
        let mut prev: Option<f32> = None;
        for _ in 0..4_000 {
            phase += 2.0 * PI * 233.0 / rate;
            let x = phase.sin() * 0.9;
            let out = ml.process([x, x], rate);
            if let Some(p) = prev {
                worst = worst.max((out[0] - p).abs());
            }
            prev = Some(out[0]);
        }
        assert!(worst < 0.02, "a large resonance jump while engaged clicked, worst step {worst}");
    }
}

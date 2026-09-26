//! Processors a host can hand to a route. Knobs are atomics, so the audio
//! thread never locks and the UI thread never waits.

use crate::{FrameInfo, Processor};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Leaves the tapped samples unchanged.
pub struct Passthrough;

impl Processor for Passthrough {
    fn process(&mut self, _frames: &mut [f32], _info: &FrameInfo) {}
}

/// Calls a closure. The closure has the same realtime rules as [`Processor`].
pub struct FnProcessor<F>(pub F);

impl<F> Processor for FnProcessor<F>
where
    F: FnMut(&mut [f32], &FrameInfo) + Send,
{
    fn process(&mut self, frames: &mut [f32], info: &FrameInfo) {
        (self.0)(frames, info);
    }
}

/// Linear gain. `1.0` is unity. A change of the knob is ramped across the
/// next block, frame by frame, so turning it never clicks.
pub struct Gain {
    linear: Arc<AtomicU32>,
    /// The gain the last block ended at.
    current: f32,
}

/// The knob for a [`Gain`], cloned before the gain is moved into a route.
#[derive(Clone)]
pub struct GainHandle(Arc<AtomicU32>);

impl Gain {
    pub fn new(linear: f32) -> (Self, GainHandle) {
        let knob = Arc::new(AtomicU32::new(linear.to_bits()));
        (Gain { linear: Arc::clone(&knob), current: linear }, GainHandle(knob))
    }

    /// A gain turned by a knob that already exists: a route opened again
    /// keeps the host's settings without handing it a new handle.
    pub fn with_handle(handle: &GainHandle) -> Self {
        Gain { linear: Arc::clone(&handle.0), current: handle.get() }
    }
}

impl GainHandle {
    pub fn set(&self, linear: f32) {
        self.0.store(linear.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

impl Processor for Gain {
    fn process(&mut self, frames: &mut [f32], info: &FrameInfo) {
        let target = f32::from_bits(self.linear.load(Ordering::Relaxed));
        if target == self.current {
            if target != 1.0 {
                for sample in frames {
                    *sample *= target;
                }
            }
            return;
        }
        // Ramp from where the last block ended to the knob, one step per
        // frame, all channels of a frame alike.
        let channels = (info.channels as usize).max(1);
        let count = (frames.len() / channels).max(1);
        let step = (target - self.current) / count as f32;
        let mut gain = self.current;
        for frame in frames.chunks_mut(channels) {
            gain += step;
            for sample in frame {
                *sample *= gain;
            }
        }
        self.current = target;
    }
}

/// A clip guard for the end of a chain: samples within `threshold` pass
/// untouched, louder ones bend smoothly toward full scale and never pass it.
/// Stateless, so it adds no latency and nothing to clear.
pub struct Limiter {
    pub threshold: f32,
}

impl Default for Limiter {
    /// Bending starts 1 dB below full scale.
    fn default() -> Self {
        Limiter { threshold: 0.891 }
    }
}

impl Processor for Limiter {
    fn process(&mut self, frames: &mut [f32], _info: &FrameInfo) {
        let knee = self.threshold.clamp(0.0, 0.999);
        let room = 1.0 - knee;
        for sample in frames {
            let level = sample.abs();
            if level > knee {
                *sample = (knee + room * ((level - knee) / room).tanh()).copysign(*sample);
            }
        }
    }
}

const MAX_CHANNELS: usize = 8;
const LOW_HZ: f32 = 120.0;
const MID_HZ: f32 = 1_000.0;
const HIGH_HZ: f32 = 8_000.0;
const MID_Q: f32 = 0.707;
/// How fast a band follows its knob: a big move glides over a fraction of a
/// second instead of jumping, which would click.
const DB_PER_SECOND: f32 = 60.0;

/// Three-band tone control: low shelf, peaking mid, high shelf.
///
/// Band gains are decibels, clamped to ±24. Zero is a bypass.
pub struct Equalizer {
    low: Arc<AtomicU32>,
    mid: Arc<AtomicU32>,
    high: Arc<AtomicU32>,
    cached_low: f32,
    cached_mid: f32,
    cached_high: f32,
    cached_rate: f32,
    coeffs: [Coeff; 3],
    state: [[BandState; MAX_CHANNELS]; 3],
}

/// Knobs for an [`Equalizer`].
#[derive(Clone)]
pub struct EqualizerHandle {
    low: Arc<AtomicU32>,
    mid: Arc<AtomicU32>,
    high: Arc<AtomicU32>,
}

impl Equalizer {
    pub fn new() -> (Self, EqualizerHandle) {
        let low = Arc::new(AtomicU32::new(0f32.to_bits()));
        let mid = Arc::new(AtomicU32::new(0f32.to_bits()));
        let high = Arc::new(AtomicU32::new(0f32.to_bits()));
        let handle = EqualizerHandle { low, mid, high };
        (Self::with_handle(&handle), handle)
    }

    /// An equalizer turned by knobs that already exist: a route opened
    /// again keeps the host's settings without handing it new handles.
    pub fn with_handle(handle: &EqualizerHandle) -> Self {
        Equalizer {
            low: Arc::clone(&handle.low),
            mid: Arc::clone(&handle.mid),
            high: Arc::clone(&handle.high),
            cached_low: 0.0,
            cached_mid: 0.0,
            cached_high: 0.0,
            cached_rate: 0.0,
            coeffs: [Coeff::unity(); 3],
            state: [[BandState::default(); MAX_CHANNELS]; 3],
        }
    }
}

impl Default for Equalizer {
    fn default() -> Self {
        Self::new().0
    }
}

impl EqualizerHandle {
    pub fn set_low_db(&self, db: f32) {
        store_db(&self.low, db);
    }

    pub fn set_mid_db(&self, db: f32) {
        store_db(&self.mid, db);
    }

    pub fn set_high_db(&self, db: f32) {
        store_db(&self.high, db);
    }

    pub fn low_db(&self) -> f32 {
        load_db(&self.low)
    }

    pub fn mid_db(&self) -> f32 {
        load_db(&self.mid)
    }

    pub fn high_db(&self) -> f32 {
        load_db(&self.high)
    }
}

impl Processor for Equalizer {
    fn process(&mut self, frames: &mut [f32], info: &FrameInfo) {
        let channels = info.channels as usize;
        if channels == 0 || channels > MAX_CHANNELS || info.sample_rate <= 0.0 {
            return;
        }
        let count = frames.len() / channels;
        self.retune(info.sample_rate as f32, count);
        for frame in 0..count {
            for channel in 0..channels {
                let index = frame * channels + channel;
                let mut sample = frames[index];
                for band in 0..3 {
                    sample = self.bands_apply(band, channel, sample);
                }
                frames[index] = sample;
            }
        }
    }
}

impl Equalizer {
    /// Coefficients for this block: each band moves toward its knob by at
    /// most [`DB_PER_SECOND`] over the block's `frames`.
    fn retune(&mut self, rate: f32, frames: usize) {
        let most = DB_PER_SECOND * frames as f32 / rate.max(1.0);
        let toward = |from: f32, to: f32| if (to - from).abs() <= most { to } else { from + most.copysign(to - from) };
        let low = toward(self.cached_low, load_db(&self.low));
        let mid = toward(self.cached_mid, load_db(&self.mid));
        let high = toward(self.cached_high, load_db(&self.high));
        if rate == self.cached_rate && low == self.cached_low && mid == self.cached_mid && high == self.cached_high {
            return;
        }
        self.coeffs[0] = low_shelf(rate, LOW_HZ, low);
        self.coeffs[1] = peaking(rate, MID_HZ, mid);
        self.coeffs[2] = high_shelf(rate, HIGH_HZ, high);
        if rate != self.cached_rate {
            self.state = [[BandState::default(); MAX_CHANNELS]; 3];
        }
        self.cached_rate = rate;
        self.cached_low = low;
        self.cached_mid = mid;
        self.cached_high = high;
    }

    fn bands_apply(&mut self, band: usize, channel: usize, input: f32) -> f32 {
        let coeff = self.coeffs[band];
        let state = &mut self.state[band][channel];
        let output = coeff.b0 * input + state.z1;
        state.z1 = flush(coeff.b1 * input - coeff.a1 * output + state.z2);
        state.z2 = flush(coeff.b2 * input - coeff.a2 * output);
        output
    }
}

fn store_db(slot: &AtomicU32, db: f32) {
    slot.store(db.clamp(-24.0, 24.0).to_bits(), Ordering::Relaxed);
}

fn load_db(slot: &AtomicU32) -> f32 {
    f32::from_bits(slot.load(Ordering::Relaxed))
}

#[derive(Clone, Copy)]
struct Coeff {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coeff {
    fn unity() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }
}

#[derive(Clone, Copy, Default)]
struct BandState {
    z1: f32,
    z2: f32,
}

fn flush(sample: f32) -> f32 {
    if sample.abs() < 1.0e-15 { 0.0 } else { sample }
}

/// Robert Bristow-Johnson peaking EQ. A near-zero gain is unity.
fn peaking(rate: f32, freq: f32, db: f32) -> Coeff {
    if db.abs() < 0.01 || rate <= 0.0 {
        return Coeff::unity();
    }
    let a = db_amp(db);
    let w0 = angular(rate, freq);
    let alpha = w0.sin() / (2.0 * MID_Q);
    let cos = w0.cos();
    normalize(
        1.0 + alpha * a,
        -2.0 * cos,
        1.0 - alpha * a,
        1.0 + alpha / a,
        -2.0 * cos,
        1.0 - alpha / a,
    )
}

/// Low shelf, slope S = 1. A near-zero gain is unity.
fn low_shelf(rate: f32, freq: f32, db: f32) -> Coeff {
    let Some((a, cos, two)) = shelf_terms(rate, freq, db) else {
        return Coeff::unity();
    };
    let ap = a + 1.0;
    let am = a - 1.0;
    normalize(
        a * (ap - am * cos + two),
        2.0 * a * (am - ap * cos),
        a * (ap - am * cos - two),
        ap + am * cos + two,
        -2.0 * (am + ap * cos),
        ap + am * cos - two,
    )
}

/// High shelf, slope S = 1.
fn high_shelf(rate: f32, freq: f32, db: f32) -> Coeff {
    let Some((a, cos, two)) = shelf_terms(rate, freq, db) else {
        return Coeff::unity();
    };
    let ap = a + 1.0;
    let am = a - 1.0;
    normalize(
        a * (ap + am * cos + two),
        -2.0 * a * (am + ap * cos),
        a * (ap + am * cos - two),
        ap - am * cos + two,
        2.0 * (am - ap * cos),
        ap - am * cos - two,
    )
}

fn shelf_terms(rate: f32, freq: f32, db: f32) -> Option<(f32, f32, f32)> {
    if db.abs() < 0.01 || rate <= 0.0 {
        return None;
    }
    let a = db_amp(db);
    let w0 = angular(rate, freq);
    // S = 1, so alpha = sin(w0)/2 * sqrt(2).
    let alpha = w0.sin() / 2.0 * 2.0f32.sqrt();
    Some((a, w0.cos(), 2.0 * a.sqrt() * alpha))
}

fn db_amp(db: f32) -> f32 {
    10f32.powf(db / 40.0)
}

fn angular(rate: f32, freq: f32) -> f32 {
    let nyquist = rate * 0.45;
    let freq = freq.clamp(20.0, nyquist.max(20.0));
    std::f32::consts::TAU * freq / rate
}

fn normalize(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Coeff {
    if a0 == 0.0 {
        return Coeff::unity();
    }
    Coeff { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A knob turned between blocks ramps across the next one instead of
    /// jumping (no click), and holds once there.
    #[test]
    fn a_gain_change_ramps_across_one_block() {
        let (mut gain, knob) = Gain::new(1.0);
        let info = FrameInfo { sample_rate: 48_000.0, channels: 2, frames: 4, host_time: 0 };
        knob.set(0.0);
        let mut block = vec![1.0f32; 8];
        gain.process(&mut block, &info);
        assert_eq!(block, [0.75, 0.75, 0.5, 0.5, 0.25, 0.25, 0.0, 0.0], "one step per frame, channels alike");
        let mut block = vec![1.0f32; 8];
        gain.process(&mut block, &info);
        assert!(block.iter().all(|&s| s == 0.0), "at the knob from then on");
        // A gain built on an existing knob starts where the knob is.
        let mut again = Gain::with_handle(&knob);
        let mut block = vec![1.0f32; 2];
        again.process(&mut block, &info);
        assert_eq!(block, [0.0, 0.0]);
    }

    fn rms_through(eq: &mut Equalizer, freq: f32) -> f32 {
        let rate = 44_100.0;
        let info = FrameInfo { sample_rate: rate as f64, channels: 1, frames: 0, host_time: 0 };
        let mut block: Vec<f32> = (0..44_100).map(|i| (std::f32::consts::TAU * freq * i as f32 / rate).sin()).collect();
        eq.process(&mut block, &info);
        // Past the filter's settling, over whole cycles.
        let tail = &block[22_050..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    }

    /// Turning a knob changes what the route plays: +4 dB on the low shelf
    /// lifts a 60 Hz tone by about 4 dB and leaves a 5 kHz tone alone; a knob
    /// set through a handle reaches an equalizer built later on it.
    #[test]
    fn a_low_boost_lifts_the_lows_only() {
        let (mut flat, knobs) = Equalizer::new();
        let unity = rms_through(&mut flat, 60.0);
        knobs.set_low_db(4.0);
        let mut boosted = Equalizer::with_handle(&knobs);
        let low = 20.0 * (rms_through(&mut boosted, 60.0) / unity).log10();
        assert!((low - 4.0).abs() < 0.6, "60 Hz rose {low:.2} dB");
        let high = 20.0 * (rms_through(&mut boosted, 5_000.0) / rms_through(&mut Equalizer::new().0, 5_000.0)).log10();
        assert!(high.abs() < 0.2, "5 kHz moved {high:.2} dB");
    }

    /// A big knob move glides: after one short block the band has only moved
    /// part of the way; after a quarter second it is there.
    #[test]
    fn a_band_glides_to_a_big_move() {
        let (mut eq, knobs) = Equalizer::new();
        let info = FrameInfo { sample_rate: 48_000.0, channels: 2, frames: 480, host_time: 0 };
        let mut block = vec![0.0f32; 960];
        eq.process(&mut block, &info);
        knobs.set_low_db(12.0);
        eq.process(&mut block, &info);
        assert!((eq.cached_low - 0.6).abs() < 1e-4, "10 ms moves 0.6 dB, not {}", eq.cached_low);
        for _ in 0..25 {
            eq.process(&mut block, &info);
        }
        assert_eq!(eq.cached_low, 12.0);
    }

    /// The limiter leaves quiet samples alone and bends loud ones below full
    /// scale, keeping their sign and order.
    #[test]
    fn the_limiter_never_passes_full_scale() {
        let mut limiter = Limiter::default();
        let info = FrameInfo { sample_rate: 48_000.0, channels: 1, frames: 5, host_time: 0 };
        let mut block = [0.5f32, -0.8, 1.0, -4.0, 8.0];
        limiter.process(&mut block, &info);
        assert_eq!(&block[..2], [0.5, -0.8]);
        assert!(block[2] > 0.891 && block[2] < 1.0);
        assert!(block[3] < -block[2] && block[3] >= -1.0);
        assert!(block[4] >= -block[3] && block[4] <= 1.0);
    }
}

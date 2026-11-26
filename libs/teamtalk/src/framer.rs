//! Device audio to gated mono frames. Each completed frame borrows the
//! framer's sample buffer for the duration of the caller's callback.

use crate::dsp::{fade_in, fade_out_tail, Gate, GateState, Limiter};
use crate::resample::Resampler;
use crate::{INTERNAL_RATE, MAX_FRAME};

/// Output format and voice gate settings for a [`Framer`].
#[derive(Clone, Copy, Debug)]
pub struct FramerConfig {
    /// Sample rate of the emitted mono frames, in Hz.
    pub out_rate: f64,
    /// Samples per frame, in `32..=MAX_FRAME`.
    pub frame_samples: usize,
    /// A frame above this RMS level opens the gate.
    pub gate_threshold_rms: f32,
    /// Time the gate stays open after the last active frame, in milliseconds.
    pub gate_hangover_ms: u32,
}

/// One completed mono frame, including its position in the output stream.
#[derive(Debug)]
pub struct Frame<'a> {
    /// Gated, limited audio. Empty when silent.
    pub samples: &'a [f32],
    /// The gate is closed or capture is muted.
    pub silent: bool,
    /// The gate opened on this frame.
    pub talk_start: bool,
    /// Frame number, including silent frames, wrapping from `u32::MAX` to zero.
    pub seq: u32,
    /// Index of the first sample at the output rate, modulo 2^32.
    pub timestamp: u32,
    /// Input-side capture time of the first sample on the caller's microsecond
    /// clock, at device-sample resolution. Ignores the resampler's group delay.
    pub capture_us: u64,
}

/// Folds device channels to mono and emits fixed-size frames without allocating.
/// For each push, `block_us` is the caller's clock at the first device sample
/// in the block. Use the same clock as any media synchronized with this audio.
pub struct Framer {
    resampler: Resampler,
    out_rate: f64,
    input_gain: f32,
    muted: bool,
    frames: FrameBuffer,
}

struct FrameBuffer {
    samples: [f32; MAX_FRAME],
    len: usize,
    fill: usize,
    seq: u32,
    timestamp: u32,
    capture_us: u64,
    gate: Gate,
    edge_fade: usize,
    limiter: Limiter,
}

impl Framer {
    /// Returns an error for an invalid frame size or a nonpositive or nonfinite rate.
    pub fn new(config: FramerConfig) -> Result<Self, &'static str> {
        if !(32..=MAX_FRAME).contains(&config.frame_samples) {
            return Err("frame_samples must be in 32..=MAX_FRAME");
        }
        if !config.out_rate.is_finite() || config.out_rate <= 0.0 {
            return Err("out_rate must be positive and finite");
        }
        let hangover =
            config.gate_hangover_ms as f64 * config.out_rate / 1000.0 / config.frame_samples as f64;
        // Limiter::new computes its coefficients at INTERNAL_RATE. Scale
        // its sample counts to keep a 0.5 ms attack and 60 ms release.
        let rate_scale = (config.out_rate / INTERNAL_RATE) as f32;
        Ok(Self {
            resampler: Resampler::new(),
            out_rate: config.out_rate,
            input_gain: 1.0,
            muted: false,
            frames: FrameBuffer {
                samples: [0.0; MAX_FRAME],
                len: config.frame_samples,
                fill: 0,
                seq: 0,
                timestamp: 0,
                capture_us: 0,
                gate: Gate::new(config.gate_threshold_rms, (hangover as u32).max(1)),
                edge_fade: (config.out_rate / 1000.0).round().max(1.0) as usize,
                limiter: Limiter::new(0.95, 0.5 * rate_scale, 60.0 * rate_scale),
            },
        })
    }

    /// Muted frames carry silence and advance the stream clock.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Gain applied before resampling, clamped to the link's range of 0 to 16.
    pub fn set_input_gain(&mut self, gain: f32) {
        self.input_gain = gain.clamp(0.0, 16.0);
    }

    /// Number of frames emitted, including silent frames, modulo 2^32.
    pub fn frames_out(&self) -> u32 {
        self.frames.seq
    }

    /// Feed a mono device block at `rate` Hz.
    pub fn push_mono(
        &mut self,
        rate: f64,
        samples: &[f32],
        block_us: u64,
        out: &mut impl FnMut(&Frame<'_>),
    ) {
        self.push(rate, samples.iter().copied(), block_us, out);
    }

    /// Feed a channel-major device block, averaging the channels to mono.
    pub fn push_planar(
        &mut self,
        rate: f64,
        frames: usize,
        channels: usize,
        data: &[f32],
        block_us: u64,
        out: &mut impl FnMut(&Frame<'_>),
    ) {
        self.push(rate, planar_samples(frames, channels, data), block_us, out);
    }

    /// Feed an interleaved device block, averaging the channels to mono.
    pub fn push_interleaved(
        &mut self,
        rate: f64,
        channels: usize,
        data: &[f32],
        block_us: u64,
        out: &mut impl FnMut(&Frame<'_>),
    ) {
        self.push(rate, interleaved_samples(channels, data), block_us, out);
    }

    fn push(
        &mut self,
        rate: f64,
        samples: impl ExactSizeIterator<Item = f32>,
        block_us: u64,
        out: &mut impl FnMut(&Frame<'_>),
    ) {
        let gain = self.input_gain;
        let muted = self.muted;
        self.push_with_controls(rate, samples, block_us, || gain, || muted, out);
    }

    pub(crate) fn push_with_controls(
        &mut self,
        rate: f64,
        samples: impl ExactSizeIterator<Item = f32>,
        block_us: u64,
        input_gain: impl Fn() -> f32,
        muted: impl Fn() -> bool,
        out: &mut impl FnMut(&Frame<'_>),
    ) {
        self.resampler
            .set_ratio(rate.max(1000.0), self.out_rate, 0.0);
        let mut samples = samples.enumerate();
        // Sample the link's gain every 512 device samples and its mute
        // switch at each frame boundary, even during a large device block.
        while samples.len() > 0 {
            let gain = input_gain();
            for (device_index, x) in samples.by_ref().take(512) {
                self.resampler.push(x * gain, |y| {
                    if self.frames.fill == 0 {
                        let offset_us = (device_index as f64 * 1_000_000.0 / rate).round() as u64;
                        self.frames.capture_us = block_us.saturating_add(offset_us);
                    }
                    self.frames.push(y, &muted, out);
                });
            }
        }
    }
}

pub(crate) fn planar_samples(
    frames: usize,
    channels: usize,
    data: &[f32],
) -> impl ExactSizeIterator<Item = f32> + '_ {
    let channels = channels.max(1);
    let count = if frames <= data.len() / channels {
        frames
    } else {
        0
    };
    let scale = 1.0 / channels as f32;
    (0..count).map(move |f| (0..channels).map(|c| data[c * frames + f]).sum::<f32>() * scale)
}

pub(crate) fn interleaved_samples(
    channels: usize,
    data: &[f32],
) -> impl ExactSizeIterator<Item = f32> + '_ {
    let channels = channels.max(1);
    let scale = 1.0 / channels as f32;
    data.chunks_exact(channels)
        .map(move |s| s.iter().sum::<f32>() * scale)
}

impl FrameBuffer {
    #[inline]
    fn push(&mut self, sample: f32, muted: &impl Fn() -> bool, out: &mut impl FnMut(&Frame<'_>)) {
        self.samples[self.fill] = sample;
        self.fill += 1;
        if self.fill < self.len {
            return;
        }
        self.fill = 0;
        let samples = &mut self.samples[..self.len];
        let state = if muted() {
            GateState::Silent
        } else {
            self.gate.process(samples)
        };
        let silent = state == GateState::Silent;
        if !silent {
            match state {
                GateState::Opened => fade_in(samples, self.edge_fade),
                GateState::Closing => fade_out_tail(samples, self.edge_fade),
                _ => {}
            }
            self.limiter.process(samples);
        }
        out(&Frame {
            samples: if silent { &[] } else { samples },
            silent,
            talk_start: state == GateState::Opened,
            seq: self.seq,
            timestamp: self.timestamp,
            capture_us: self.capture_us,
        });
        self.seq = self.seq.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(self.len as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> FramerConfig {
        FramerConfig {
            out_rate: 16_000.0,
            frame_samples: 320,
            gate_threshold_rms: 0.02,
            gate_hangover_ms: 40,
        }
    }

    #[test]
    fn uneven_planar_blocks_preserve_pitch_level_and_clock() {
        const START_US: u64 = 1_234_567;
        const GAP_US: u64 = 75_000;
        // The second frame starts at device sample 882 and spans the gap.
        const GAP_AT: usize = 441 + 512;
        let mut framer = Framer::new(config()).unwrap();
        let mut frames = 0;
        let mut crossings = 0;
        let mut previous = 0.0;
        let mut offset = 0;
        let mut stereo = [0.0; 2 * 1024];
        for block in [441, 512, 37, 1024].into_iter().cycle() {
            let n = block.min(44_100 - offset);
            for i in 0..n {
                let phase = (offset + i) as f64 * std::f64::consts::TAU * 1000.0 / 44_100.0;
                // The mono average of the two channels has amplitude 0.3.
                stereo[i] = 0.5 * phase.sin() as f32;
                stereo[n + i] = 0.1 * phase.sin() as f32;
            }
            let block_us = START_US + (offset as u64 * 1_000_000 + 22_050) / 44_100
                + if offset >= GAP_AT { GAP_US } else { 0 };
            framer.push_planar(44_100.0, n, 2, &stereo[..2 * n], block_us, &mut |frame| {
                assert_eq!((frame.seq, frame.timestamp), (frames, frames * 320));
                let expected_us = START_US + u64::from(frames) * 20_000
                    + if frames >= 2 { GAP_US } else { 0 };
                assert!(
                    frame.capture_us.abs_diff(expected_us) <= 23,
                    "frame {frames} captured at {} µs, expected {expected_us} µs", frame.capture_us
                );
                assert_eq!(frame.samples.len(), 320);
                assert!(!frame.silent);
                assert_eq!(frame.talk_start, frames == 0);
                let level = crate::dsp::rms(frame.samples);
                let expected = 0.3 * std::f32::consts::FRAC_1_SQRT_2;
                assert!(
                    (level - expected).abs() < expected * 0.1,
                    "frame {frames} RMS {level}"
                );
                for &x in frame.samples {
                    if (x < 0.0) != (previous < 0.0) {
                        crossings += 1;
                    }
                    previous = x;
                }
                frames += 1;
            });
            offset += n;
            if offset == 44_100 {
                break;
            }
        }
        assert!((49..=51).contains(&frames), "frame count {frames}");
        assert_eq!(frames, framer.frames_out());
        assert!(
            (crossings - frames as i32 * 40).abs() <= 2,
            "zero crossings {crossings}"
        );
    }

    #[test]
    fn gate_hangover_restart_and_mute_keep_the_clock_running() {
        let mut framer = Framer::new(config()).unwrap();
        // Two hangover frames, one closing frame, then header-only silence.
        let mut seq = 0;
        for (input, muted, silent, onset) in [
            (0.2, false, false, true),
            (0.0, false, false, false),
            (0.0, false, false, false),
            (0.0, false, false, false),
            (0.0, false, true, false),
            (0.2, false, false, true),
            (0.2, true, true, false),
            (0.2, true, true, false),
        ] {
            framer.set_muted(muted);
            let block_us = u64::from(seq) * 20_000;
            framer.push_mono(16_000.0, &[input; 320], block_us, &mut |f| {
                assert_eq!((f.seq, f.timestamp), (seq, seq * 320));
                assert_eq!(f.capture_us, block_us, "capture time through gate and mute");
                assert_eq!(
                    (f.silent, f.samples.is_empty(), f.talk_start),
                    (silent, silent, onset)
                );
                if onset {
                    assert_eq!(f.samples[0], 0.0);
                    assert_eq!(f.samples[8], 0.1, "middle of the 1 ms fade");
                    assert_eq!(f.samples[16], 0.2, "full level after 1 ms");
                }
                seq += 1;
            });
        }
        assert_eq!(seq, 8);
    }

    #[test]
    fn invalid_formats_are_rejected_and_large_upsampling_ratios_keep_every_sample() {
        for frame_samples in [31, MAX_FRAME + 1] {
            assert!(Framer::new(FramerConfig {
                frame_samples,
                ..config()
            })
            .is_err());
        }
        for out_rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(Framer::new(FramerConfig {
                out_rate,
                ..config()
            })
            .is_err());
        }
        for (out_rate, expected_samples) in [(48_000.0, 4096), (96_000.0, 8192)] {
            let mut f = Framer::new(FramerConfig {
                out_rate,
                frame_samples: 32,
                ..config()
            })
            .unwrap();
            f.push_mono(6000.0, &[0.2; 512], 0, &mut |frame| {
                assert_eq!(frame.samples.len(), 32)
            });
            assert_eq!(f.frames_out() * 32, expected_samples);
        }
    }
}

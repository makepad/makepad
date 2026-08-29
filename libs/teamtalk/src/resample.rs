//! A stateful 4-point Hermite (Catmull-Rom) resampler.
//!
//! The same struct works in two directions: [`Resampler::push`] feeds it one
//! input sample and emits however many output samples fall inside the new
//! interpolation window (capture side: device rate → 48 kHz); [`Resampler::pull`]
//! produces one output sample and asks for input samples on demand (playback
//! side: 48 kHz → device rate). The history carries across blocks, so there is
//! no phase reset or edge duplication at block boundaries, and the ratio can be
//! nudged per block for drift correction without a click.
//!
//! Group delay is two input samples. No allocation, no branches on size.

#[derive(Clone, Debug)]
pub struct Resampler {
    hist: [f32; 4],
    /// Position of the next output sample relative to `hist[1]`, in input samples.
    pos: f64,
    /// Input samples per output sample.
    step: f64,
}

impl Default for Resampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Resampler {
    pub const fn new() -> Self {
        Self {
            hist: [0.0; 4],
            pos: 1.0,
            step: 1.0,
        }
    }

    /// Set the conversion: `in_rate` samples in per `out_rate` samples out,
    /// times `1 + nudge` (a small playback-speed correction, e.g. ±0.005).
    pub fn set_ratio(&mut self, in_rate: f64, out_rate: f64, nudge: f64) {
        let step = (in_rate / out_rate) * (1.0 + nudge);
        self.step = if step.is_finite() && step > 0.0 { step } else { 1.0 };
    }

    pub fn step(&self) -> f64 {
        self.step
    }

    /// Forget the history (e.g. after a long gap) without touching the ratio.
    pub fn reset(&mut self) {
        self.hist = [0.0; 4];
        self.pos = 1.0;
    }

    #[inline]
    fn interpolate(&self) -> f32 {
        let [y0, y1, y2, y3] = self.hist;
        let t = self.pos as f32;
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * t + c2) * t + c1) * t + y1
    }

    #[inline]
    fn shift(&mut self, x: f32) {
        self.hist = [self.hist[1], self.hist[2], self.hist[3], x];
        self.pos -= 1.0;
    }

    /// Feed one input sample; `emit` is called for every output sample that
    /// becomes computable (zero or more).
    #[inline]
    pub fn push(&mut self, x: f32, mut emit: impl FnMut(f32)) {
        self.shift(x);
        while self.pos < 1.0 {
            emit(self.interpolate());
            self.pos += self.step;
        }
    }

    /// Produce one output sample, calling `next` for each input sample needed.
    #[inline]
    pub fn pull(&mut self, mut next: impl FnMut() -> f32) -> f32 {
        while self.pos >= 1.0 {
            let x = next();
            self.shift(x);
        }
        let y = self.interpolate();
        self.pos += self.step;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: f64, hz: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| ((i as f64) * hz * std::f64::consts::TAU / rate).sin() as f32)
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn unity_ratio_is_a_two_sample_delay() {
        let mut r = Resampler::new();
        r.set_ratio(48000.0, 48000.0, 0.0);
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let mut out = Vec::new();
        for &x in &input {
            r.push(x, |y| out.push(y));
        }
        assert_eq!(out.len(), 100);
        for i in 2..100 {
            assert!((out[i] - input[i - 2]).abs() < 1e-5, "{i}: {} vs {}", out[i], input[i - 2]);
        }
    }

    #[test]
    fn push_44100_to_48000_keeps_a_tone_intact() {
        let mut r = Resampler::new();
        r.set_ratio(44100.0, 48000.0, 0.0);
        let input = sine(44100.0, 440.0, 44100);
        let mut out = Vec::new();
        for &x in &input {
            r.push(x, |y| out.push(y));
        }
        // 1 s in → ~48000 out.
        assert!((out.len() as i64 - 48000).abs() <= 2, "{}", out.len());
        // Same amplitude, and the tone is still 440 Hz: compare against a
        // reference sine at 48 kHz, allowing the two-sample group delay.
        let reference = sine(48000.0, 440.0, out.len());
        let delay = 2.0 * 48000.0 / 44100.0; // output samples
        let mut err = 0.0f32;
        for i in 100..out.len() - 100 {
            let ref_val = ((i as f64 - delay) * 440.0 * std::f64::consts::TAU / 48000.0).sin() as f32;
            err = err.max((out[i] - ref_val).abs());
        }
        assert!(err < 0.01, "max error {err}");
        assert!((rms(&out[100..]) - rms(&reference[100..])).abs() < 0.01);
    }

    #[test]
    fn pull_48000_to_44100_matches_push_direction() {
        let mut r = Resampler::new();
        r.set_ratio(48000.0, 44100.0, 0.0);
        let input = sine(48000.0, 1000.0, 48000);
        let mut idx = 0usize;
        let mut out = Vec::new();
        for _ in 0..44100 {
            out.push(r.pull(|| {
                let v = input.get(idx).copied().unwrap_or(0.0);
                idx += 1;
                v
            }));
        }
        // Consumed ~48000 inputs for 44100 outputs.
        assert!((idx as i64 - 48000).abs() <= 3, "consumed {idx}");
        let mut err = 0.0f32;
        for i in 100..44000 {
            let ref_val = ((i as f64 * 48000.0 / 44100.0 - 2.0) * 1000.0 * std::f64::consts::TAU / 48000.0).sin() as f32;
            err = err.max((out[i] - ref_val).abs());
        }
        assert!(err < 0.02, "max error {err}");
    }

    #[test]
    fn nudge_changes_consumption_rate_smoothly() {
        let mut r = Resampler::new();
        r.set_ratio(48000.0, 48000.0, 0.005);
        let mut consumed = 0usize;
        for _ in 0..48000 {
            r.pull(|| {
                consumed += 1;
                0.0
            });
        }
        // +0.5 % faster: 240 more input samples per second of output.
        assert!((consumed as i64 - 48240).abs() <= 2, "consumed {consumed}");
    }

    #[test]
    fn block_boundaries_do_not_glitch() {
        // Feed the same tone in blocks of odd sizes and check the output is
        // identical to feeding it in one go: the state carries across blocks.
        let input = sine(44100.0, 700.0, 4410);
        let mut whole = Vec::new();
        let mut r = Resampler::new();
        r.set_ratio(44100.0, 48000.0, 0.0);
        for &x in &input {
            r.push(x, |y| whole.push(y));
        }
        let mut blocks = Vec::new();
        let mut r = Resampler::new();
        r.set_ratio(44100.0, 48000.0, 0.0);
        for chunk in input.chunks(37) {
            for &x in chunk {
                r.push(x, |y| blocks.push(y));
            }
        }
        assert_eq!(whole, blocks);
    }
}

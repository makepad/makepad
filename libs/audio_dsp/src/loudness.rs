//! Loudness by the broadcast rule (ITU-R BS.1770-4, EBU R128): the one
//! meter Stage's mixes, exports, records and music checks measure with.
//!
//! [`KWeighting`] weights a channel the way hearing does (a high shelf for
//! the head, then the RLB high pass), designed from the analogue prototypes
//! so any rate works and 48 kHz reproduces the standard's table.
//! [`LoudnessMeter`] streams interleaved stereo and keeps one energy per
//! 100 ms step, never the audio: integrated loudness gates its 400 ms
//! blocks at -70 LUFS and then 10 LU under their mean.

/// What puts a K-weighted mean square on the LUFS scale.
pub const LUFS_OFFSET: f64 = -0.691;
const ABSOLUTE_GATE: f64 = -70.0;
const RELATIVE_GATE: f64 = -10.0;

/// The loudness (LUFS) of a K-weighted mean square summed over channels.
pub fn lufs(mean_square: f64) -> f64 {
    LUFS_OFFSET + 10.0 * mean_square.log10()
}

/// The K-weighting of one channel at a rate: two biquads in direct form I.
#[derive(Clone, Copy, Debug)]
pub struct KWeighting {
    /// Per section: b0 b1 b2 a1 a2.
    sections: [[f64; 5]; 2],
    /// Per section: x1 x2 y1 y2.
    state: [[f64; 4]; 2],
}

impl KWeighting {
    pub fn new(rate: f64) -> Self {
        let rate = rate.max(1.0);
        let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        let shelf = [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];
        let (f0, q) = (38.13547087602444, 0.5003270373238773);
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let high_pass = [1.0, -2.0, 1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];
        Self { sections: [shelf, high_pass], state: [[0.0; 4]; 2] }
    }

    /// One sample through the weighting.
    #[inline]
    pub fn process(&mut self, mut x: f64) -> f64 {
        for (c, s) in self.sections.iter().zip(self.state.iter_mut()) {
            let y = c[0] * x + c[1] * s[0] + c[2] * s[1] - c[3] * s[2] - c[4] * s[3];
            *s = [x, s[0], y, s[2]];
            x = y;
        }
        x
    }
}

/// A streaming integrated-loudness meter for stereo at `rate`: push the
/// sound in any chunking, read the loudness at the end.
#[derive(Clone, Debug)]
pub struct LoudnessMeter {
    weighting: [KWeighting; 2],
    /// Frames per 100 ms step.
    step: usize,
    in_step: usize,
    energy: f64,
    steps: Vec<f64>,
}

impl LoudnessMeter {
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1) as f64;
        let weighting = KWeighting::new(rate);
        Self { weighting: [weighting, weighting], step: ((rate * 0.1).round() as usize).max(1), in_step: 0, energy: 0.0, steps: Vec::new() }
    }

    /// Interleaved stereo samples, full scale 1.
    pub fn push(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(2) {
            self.push_frame(frame[0] as f64, frame[1] as f64);
        }
    }

    /// One stereo frame, full scale 1.
    #[inline]
    pub fn push_frame(&mut self, left: f64, right: f64) {
        let l = self.weighting[0].process(left);
        let r = self.weighting[1].process(right);
        self.energy += l * l;
        self.energy += r * r;
        self.in_step += 1;
        if self.in_step == self.step {
            self.steps.push(self.energy);
            self.energy = 0.0;
            self.in_step = 0;
        }
    }

    /// The mean squares of the 400 ms blocks so far, 100 ms apart.
    pub fn blocks(&self) -> impl Iterator<Item = f64> + '_ {
        let block = 4.0 * self.step as f64;
        self.steps.windows(4).map(move |w| w.iter().sum::<f64>() / block)
    }

    /// The gated integrated loudness (LUFS); `None` when nothing passes
    /// the absolute gate (silence, or less than one block). A trailing
    /// partial step is left out, as the standard does.
    pub fn finish(&self) -> Option<f64> {
        let blocks: Vec<f64> = self.blocks().collect();
        let above = |gate: f64| blocks.iter().copied().filter(move |&z| z > 0.0 && lufs(z) > gate);
        let count = above(ABSOLUTE_GATE).count();
        if count == 0 {
            return None;
        }
        let relative = lufs(above(ABSOLUTE_GATE).sum::<f64>() / count as f64) + RELATIVE_GATE;
        let gated: Vec<f64> = above(ABSOLUTE_GATE).filter(|&z| lufs(z) > relative).collect();
        (!gated.is_empty()).then(|| lufs(gated.iter().sum::<f64>() / gated.len() as f64))
    }
}

/// The integrated loudness (LUFS) of interleaved stereo at `rate`.
pub fn integrated_lufs(samples: &[f32], rate: u32) -> Option<f64> {
    if rate == 0 {
        return None;
    }
    let mut meter = LoudnessMeter::new(rate);
    meter.push(samples);
    meter.finish()
}

/// K-weighted samples of one channel at `rate`.
pub fn k_weighted(x: &[f32], rate: f64) -> Vec<f64> {
    let mut k = KWeighting::new(rate);
    x.iter().map(|v| k.process(if v.is_finite() { *v as f64 } else { 0.0 })).collect()
}

/// Loudness (LUFS) of windows of `len` frames every `step` of two
/// K-weighted channels.
fn window_loudness(l: &[f64], r: &[f64], len: usize, step: usize) -> Vec<f64> {
    let (len, step) = (len.max(1), step.max(1));
    let mut out = Vec::new();
    let mut start = 0;
    while start + len <= l.len().min(r.len()) {
        let ms = (l[start..start + len].iter().map(|v| v * v).sum::<f64>() + r[start..start + len].iter().map(|v| v * v).sum::<f64>()) / len as f64;
        out.push(lufs(ms.max(1e-20)));
        start += step;
    }
    out
}

/// The loudness of the mean energy of windows given in LUFS.
fn mean_loudness(windows: &[f64]) -> f64 {
    let e = windows.iter().map(|l| 10f64.powf((l - LUFS_OFFSET) / 10.0)).sum::<f64>() / windows.len().max(1) as f64;
    lufs(e.max(1e-20))
}

/// How a master is judged: integrated loudness (LUFS), loudness range (EBU
/// Tech 3342, LU) and true peak (4x oversampled, dBTP).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    pub lufs: f64,
    pub lra: f64,
    pub true_peak: f64,
}

/// [`Loudness`] of two channels at `rate`. Silence reads -70 LUFS.
pub fn measure(left: &[f32], right: &[f32], rate: f64) -> Loudness {
    let mut meter = LoudnessMeter::new(rate.round() as u32);
    for (l, r) in left.iter().zip(right) {
        meter.push_frame(*l as f64, *r as f64);
    }
    let integrated = meter.finish().unwrap_or(ABSOLUTE_GATE);
    // Range: 3 s windows every 100 ms, gated at -70 and 20 LU under their
    // mean; the spread between the 10th and 95th percentiles.
    let (l, r) = (k_weighted(left, rate), k_weighted(right, rate));
    let short = window_loudness(&l, &r, (3.0 * rate) as usize, (0.1 * rate) as usize);
    let loud: Vec<f64> = short.iter().copied().filter(|b| *b > ABSOLUTE_GATE).collect();
    let relative = mean_loudness(&loud) - 20.0;
    let mut gated: Vec<f64> = loud.into_iter().filter(|b| *b > relative).collect();
    gated.sort_by(|a, b| a.total_cmp(b));
    let at = |p: f64| gated.get(((gated.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0.0);
    let tp = true_peak(left).max(true_peak(right));
    Loudness { lufs: integrated, lra: at(0.95) - at(0.10), true_peak: 20.0 * tp.max(1e-12).log10() }
}

/// Gated loudness (LUFS, the integrated method) of frames `a..b` of
/// K-weighted channels ([`k_weighted`]) at `rate`: a section's level;
/// -70 when nothing in it counts.
pub fn span_lufs(l: &[f64], r: &[f64], rate: f64, a: usize, b: usize) -> f64 {
    let (a, b) = (a.min(l.len()), b.min(l.len()));
    if b <= a {
        return ABSOLUTE_GATE;
    }
    let momentary = window_loudness(&l[a..b], &r[a..b], ((0.4 * rate) as usize).min(b - a), (0.1 * rate) as usize);
    let loud: Vec<f64> = momentary.into_iter().filter(|b| *b > ABSOLUTE_GATE).collect();
    if loud.is_empty() {
        return ABSOLUTE_GATE;
    }
    let relative = mean_loudness(&loud) + RELATIVE_GATE;
    let gated: Vec<f64> = loud.into_iter().filter(|b| *b > relative).collect();
    mean_loudness(&gated)
}

/// The largest absolute sample of `x` upsampled 4x (a windowed-sinc
/// interpolator, 32 taps per phase).
pub fn true_peak(x: &[f32]) -> f64 {
    const TAPS: usize = 32;
    let mut phases = [[0.0f64; TAPS]; 4];
    for (p, phase) in phases.iter_mut().enumerate() {
        for (k, tap) in phase.iter_mut().enumerate() {
            let t = k as f64 - (TAPS / 2) as f64 + p as f64 / 4.0;
            let sinc = if t.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t) };
            let w = 0.5 * (1.0 + (std::f64::consts::PI * t / (TAPS / 2) as f64).cos());
            *tap = sinc * w.max(0.0);
        }
    }
    let mut peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs() as f64));
    for i in TAPS..x.len() {
        for phase in &phases {
            let y: f64 = phase.iter().enumerate().map(|(k, tap)| x[i - k] as f64 * tap).sum();
            peak = peak.max(y.abs());
        }
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, seconds: f64, dbfs: f64) -> Vec<f32> {
        let a = 10f64.powf(dbfs / 20.0) * std::f64::consts::SQRT_2;
        (0..(seconds * rate as f64) as usize).flat_map(|i| {
            let v = (a * (i as f64 / rate as f64 * hz * std::f64::consts::TAU).sin()) as f32;
            [v, v]
        }).collect()
    }

    #[test]
    fn a_1khz_tone_at_minus_23_reads_minus_23_at_any_rate() {
        for rate in [44_100, 48_000, 96_000] {
            let lufs = integrated_lufs(&tone(1000.0, rate, 5.0, -23.0), rate).unwrap();
            // Both channels at -23 dBFS RMS sum to -20 LUFS + the K-weighting's lift.
            assert!((lufs - (-23.0 + 3.0103)).abs() < 0.05, "{rate}: {lufs}");
        }
    }

    #[test]
    fn at_48k_the_weighting_is_the_standards_table() {
        let k = KWeighting::new(48_000.0);
        let table = [[1.53512485958697, -2.69169618940638, 1.19839281085285, -1.69065929318241, 0.73248077421585], [1.0, -2.0, 1.0, -1.99004745483398, 0.99007225036621]];
        for (s, t) in k.sections.iter().zip(table) {
            for (a, b) in s.iter().zip(t) {
                assert!((a - b).abs() < 1e-12, "{a} vs {b}");
            }
        }
    }

    #[test]
    fn silence_and_short_sound_have_no_loudness() {
        assert_eq!(integrated_lufs(&vec![0.0; 96_000 * 2], 48_000), None);
        assert_eq!(integrated_lufs(&tone(1000.0, 48_000, 0.3, -10.0), 48_000), None);
    }

    #[test]
    fn a_master_reads_its_level_range_and_peak() {
        let quiet = tone(1000.0, 48_000, 6.0, -30.0);
        let loud = tone(1000.0, 48_000, 6.0, -15.0);
        let both: Vec<f32> = quiet.iter().chain(loud.iter()).copied().collect();
        let (l, r): (Vec<f32>, Vec<f32>) = both.chunks(2).map(|f| (f[0], f[1])).unzip();
        let m = measure(&l, &r, 48_000.0);
        assert!(m.lra > 12.0 && m.lra < 16.0, "range {}", m.lra);
        assert!((m.true_peak - (-15.0 + 3.0103)).abs() < 0.2, "peak {}", m.true_peak);
        let (kl, kr) = (k_weighted(&l, 48_000.0), k_weighted(&r, 48_000.0));
        let second = span_lufs(&kl, &kr, 48_000.0, 6 * 48_000, 12 * 48_000);
        assert!((second - (-15.0 + 3.0103)).abs() < 0.2, "span {second}");
    }

    #[test]
    fn chunking_does_not_change_the_reading() {
        let s = tone(440.0, 48_000, 3.0, -18.0);
        let whole = integrated_lufs(&s, 48_000);
        let mut meter = LoudnessMeter::new(48_000);
        for chunk in s.chunks(2 * 977) {
            meter.push(chunk);
        }
        assert_eq!(meter.finish(), whole);
    }
}

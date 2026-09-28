//! The one FFT of the audio pictures: a radix-2 transform over a fixed
//! power-of-two window with a Hann taper, and the fold of its bins into
//! log-spaced bands. The spectrogram bakes columns with it; a live spectrum
//! (a player's visualizer) reads windows of played samples through it.
//!
//! Every buffer is allocated by [`Fft::new`], so a caller that keeps one
//! transforms window after window without allocating.

/// A radix-2 real FFT over a fixed power-of-two window with a Hann taper.
pub struct Fft {
    size: usize,
    window: Vec<f32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
    reversed: Vec<usize>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Fft {
    /// `size` is rounded up to a power of two, at least 16.
    pub fn new(size: usize) -> Self {
        let size = size.max(16).next_power_of_two();
        let bits = size.trailing_zeros();
        let tau = std::f32::consts::TAU;
        let window = (0..size).map(|i| 0.5 - 0.5 * (tau * i as f32 / size as f32).cos()).collect();
        let cos = (0..size / 2).map(|i| (tau * i as f32 / size as f32).cos()).collect();
        let sin = (0..size / 2).map(|i| -(tau * i as f32 / size as f32).sin()).collect();
        let reversed = (0..size).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        Fft { size, window, cos, sin, reversed, re: vec![0.0; size], im: vec![0.0; size] }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// The magnitude of each bin `0..size/2` of the windowed `frame`,
    /// relative to a full-scale sine (which reads 1.0 in its bin). A frame
    /// shorter than the window is padded with silence after it: the tail of
    /// a track.
    pub fn magnitudes(&mut self, frame: &[f32], out: &mut [f32]) {
        let taken = frame.len().min(self.size);
        self.transform(|i| if i < taken { frame[i] } else { 0.0 });
        self.levels(out, |magnitude| magnitude);
    }

    /// The level of each bin `0..size/2` of the windowed `input` (its last
    /// `size` samples; a shorter input is padded with silence before it), in
    /// decibels relative to a full-scale sine, floored at -120: the newest
    /// window of a stream.
    pub fn magnitudes_db(&mut self, input: &[f32], out: &mut [f32]) {
        let n = self.size;
        let taken = input.len().min(n);
        let input = &input[input.len() - taken..];
        let pad = n - taken;
        self.transform(|i| if i < pad { 0.0 } else { input[i - pad] });
        self.levels(out, |magnitude| (20.0 * magnitude.max(1.0e-6).log10()).max(-120.0));
    }

    /// Window `sample(i)` for `i` in `0..size` and transform it in place.
    fn transform(&mut self, sample: impl Fn(usize) -> f32) {
        let n = self.size;
        for i in 0..n {
            let j = self.reversed[i];
            self.re[j] = sample(i) * self.window[i];
            self.im[j] = 0.0;
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (c, s) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = self.re[b] * c - self.im[b] * s;
                    let ti = self.re[b] * s + self.im[b] * c;
                    self.re[b] = self.re[a] - tr;
                    self.im[b] = self.im[a] - ti;
                    self.re[a] += tr;
                    self.im[a] += ti;
                }
            }
            len *= 2;
        }
    }

    fn levels(&self, out: &mut [f32], scale: impl Fn(f32) -> f32) {
        // A full-scale sine through a Hann window peaks at n/4.
        let reference = self.size as f32 / 4.0;
        for (bin, slot) in out.iter_mut().take(self.size / 2).enumerate() {
            let magnitude = (self.re[bin] * self.re[bin] + self.im[bin] * self.im[bin]).sqrt() / reference;
            *slot = scale(magnitude);
        }
    }
}

/// Folds FFT bin levels (`bins[k]` at `k * rate / (2 * bins.len())` Hz) into
/// `out.len()` bands spaced evenly in log frequency from `low_hz` to
/// `high_hz`. A band takes the loudest bin inside it; a band too narrow to
/// hold two bins reads between the bins around its centre, so neighbouring
/// low bands neither go empty nor repeat one bin as a flat step.
pub fn log_bands(bins: &[f32], sample_rate: f32, low_hz: f32, high_hz: f32, out: &mut [f32]) {
    if bins.is_empty() || out.is_empty() || sample_rate <= 0.0 {
        out.fill(-120.0);
        return;
    }
    let hz_per_bin = sample_rate / (2.0 * bins.len() as f32);
    let high_hz = high_hz.min(sample_rate * 0.5);
    let low_hz = low_hz.max(hz_per_bin * 0.5).min(high_hz);
    let ratio = (high_hz / low_hz).max(1.0);
    let bands = out.len();
    let edge = |band: usize| low_hz * ratio.powf(band as f32 / bands as f32);
    let last = bins.len() - 1;
    for (band, slot) in out.iter_mut().enumerate() {
        let from = ((edge(band) / hz_per_bin).floor() as usize).min(last);
        let to = ((edge(band + 1) / hz_per_bin).floor() as usize).clamp(from, last);
        *slot = if to <= from + 1 {
            let centre = (edge(band) * edge(band + 1)).sqrt() / hz_per_bin;
            let below = (centre.floor() as usize).min(last);
            let above = (below + 1).min(last);
            let t = centre - centre.floor();
            bins[below] + (bins[above] - bins[below]) * t
        } else {
            bins[from..=to].iter().copied().fold(f32::MIN, f32::max)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_scale_sine_reads_zero_db_in_its_bin() {
        let size = 1024;
        let rate = 48_000.0;
        let bin = 64;
        let freq = bin as f32 * rate / size as f32;
        let sine: Vec<f32> = (0..size).map(|i| (std::f32::consts::TAU * freq * i as f32 / rate).sin()).collect();
        let mut fft = Fft::new(size);
        let mut out = vec![0.0f32; size / 2];
        fft.magnitudes_db(&sine, &mut out);
        assert!(out[bin].abs() < 0.5, "peak {} dB", out[bin]);
        assert!(out[bin + 8] < -60.0, "far bins are quiet: {} dB", out[bin + 8]);
        let (loudest, _) = out.iter().enumerate().fold((0, f32::MIN), |best, (k, &v)| if v > best.1 { (k, v) } else { best });
        assert_eq!(loudest, bin);
        // The linear reading agrees: a full-scale sine is 1.0 in its bin.
        fft.magnitudes(&sine, &mut out);
        assert!((out[bin] - 1.0).abs() < 0.06, "linear peak {}", out[bin]);
        // Silence is the floor.
        fft.magnitudes_db(&[0.0; 1024], &mut out);
        assert!(out.iter().all(|&v| v == -120.0));
    }

    #[test]
    fn short_input_pads_before_a_stream_window_and_after_a_track_tail() {
        let mut fft = Fft::new(64);
        let mut out = vec![0.0f32; 32];
        // A lone impulse is flat in magnitude wherever the window puts it;
        // what differs is the taper it lands under.
        fft.magnitudes(&[1.0], &mut out);
        assert!(out.iter().all(|&v| v == 0.0), "the first sample sits at the taper's zero");
        fft.magnitudes_db(&[1.0], &mut out);
        assert!(out.iter().all(|&v| v > -120.0), "the newest sample sits near the taper's end");
    }

    #[test]
    fn log_bands_cover_the_range_and_take_the_loudest_bin() {
        // 512 bins at 48 kHz: 46.875 Hz per bin.
        let mut bins = vec![-120.0f32; 512];
        bins[64] = -3.0; // 3 kHz
        let mut bands = [0.0f32; 16];
        log_bands(&bins, 48_000.0, 20.0, 20_000.0, &mut bands);
        let loud: Vec<usize> = (0..16).filter(|&b| bands[b] > -120.0).collect();
        assert_eq!(loud.len(), 1, "one band holds the 3 kHz bin: {bands:?}");
        let band = loud[0];
        let low = 20.0f32 * 1000f32.powf(band as f32 / 16.0);
        let high = 20.0f32 * 1000f32.powf((band + 1) as f32 / 16.0);
        assert!(low <= 3_000.0 && 3_000.0 <= high + 46.875, "{low}..{high}");
        // Narrow low bands read the bins they fall between instead of
        // nothing.
        bins.fill(-40.0);
        log_bands(&bins, 48_000.0, 20.0, 20_000.0, &mut bands);
        assert!(bands.iter().all(|&v| v == -40.0));
        // ...and rise smoothly between two bins rather than in one step.
        bins.fill(-120.0);
        bins[1] = -60.0;
        bins[2] = -20.0;
        let mut narrow = [0.0f32; 8];
        log_bands(&bins, 48_000.0, 50.0, 90.0, &mut narrow);
        assert!(narrow.windows(2).all(|pair| pair[1] >= pair[0]), "{narrow:?}");
        assert!(narrow[0] > -60.0 && narrow[7] < -20.0, "{narrow:?}");
    }
}

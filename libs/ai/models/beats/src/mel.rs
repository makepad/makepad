//! Beat This! `LogMelSpect` front end.
//!
//! This reproduces torchaudio's configuration used by the released model:
//! periodic Hann, centered reflect padding, magnitude STFT normalized by
//! `sqrt(frame_length)`, 128 un-normalized Slaney-scale triangular filters,
//! and `ln(1 + 1000*x)`. Output is frame-major `[time, mel]`.

use crate::config::*;
use std::f64::consts::PI;

pub struct LogMelSpect {
    window: Vec<f64>,
    filterbank: Vec<f32>,
}

impl Default for LogMelSpect {
    fn default() -> Self {
        Self::new()
    }
}

impl LogMelSpect {
    pub fn new() -> Self {
        let window = (0..N_FFT)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / N_FFT as f64).cos())
            .collect();
        Self {
            window,
            filterbank: mel_filterbank(),
        }
    }

    /// `torch.stft(center=true)` frame count after `n_fft/2` padding at both
    /// ends: `1 + floor(samples / hop)`.
    pub fn frame_count(samples: usize) -> usize {
        1 + samples / HOP_LENGTH
    }

    pub fn compute(&self, samples: &[f32]) -> (Vec<f32>, usize) {
        if samples.is_empty() {
            return (Vec::new(), 0);
        }
        let frames = Self::frame_count(samples.len());
        let mut output = vec![0.0f32; frames * MEL_BINS];
        let mut re = vec![0.0f64; N_FFT];
        let mut im = vec![0.0f64; N_FFT];
        let mut magnitude = vec![0.0f64; FFT_BINS];
        let pad = (N_FFT / 2) as isize;
        let norm = (N_FFT as f64).sqrt();

        for frame in 0..frames {
            let start = (frame * HOP_LENGTH) as isize - pad;
            for i in 0..N_FFT {
                let at = reflect_index(start + i as isize, samples.len());
                re[i] = samples[at] as f64 * self.window[i];
            }
            im.fill(0.0);
            fft_radix2(&mut re, &mut im);
            for bin in 0..FFT_BINS {
                magnitude[bin] = (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() / norm;
            }
            for mel in 0..MEL_BINS {
                let weights = &self.filterbank[mel * FFT_BINS..(mel + 1) * FFT_BINS];
                let value = weights
                    .iter()
                    .zip(&magnitude)
                    .map(|(&weight, &mag)| weight as f64 * mag)
                    .sum::<f64>();
                output[frame * MEL_BINS + mel] =
                    (1.0 + LOG_MULTIPLIER as f64 * value).ln() as f32;
            }
        }
        (output, frames)
    }

    pub fn filterbank(&self) -> &[f32] {
        &self.filterbank
    }
}

#[inline]
fn reflect_index(index: isize, len: usize) -> usize {
    if len == 1 {
        return 0;
    }
    let period = 2 * (len as isize - 1);
    let mut folded = index % period;
    if folded < 0 {
        folded += period;
    }
    if folded >= len as isize {
        folded = period - folded;
    }
    folded as usize
}

fn hz_to_mel(hz: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    if hz >= MIN_LOG_HZ {
        MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / (6.4f64.ln() / 27.0)
    } else {
        hz / F_SP
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    if mel >= MIN_LOG_MEL {
        MIN_LOG_HZ * ((6.4f64.ln() / 27.0) * (mel - MIN_LOG_MEL)).exp()
    } else {
        F_SP * mel
    }
}

/// Torchaudio `melscale_fbanks(..., norm=None, mel_scale="slaney")`.
fn mel_filterbank() -> Vec<f32> {
    let fft_freqs: Vec<f64> = (0..FFT_BINS)
        .map(|bin| bin as f64 * SAMPLE_RATE as f64 / N_FFT as f64)
        .collect();
    let mel_min = hz_to_mel(F_MIN);
    let mel_max = hz_to_mel(F_MAX);
    let edges: Vec<f64> = (0..MEL_BINS + 2)
        .map(|i| {
            mel_to_hz(mel_min + (mel_max - mel_min) * i as f64 / (MEL_BINS + 1) as f64)
        })
        .collect();
    let mut bank = vec![0.0f32; MEL_BINS * FFT_BINS];
    for mel in 0..MEL_BINS {
        let lower_span = edges[mel + 1] - edges[mel];
        let upper_span = edges[mel + 2] - edges[mel + 1];
        for (bin, &frequency) in fft_freqs.iter().enumerate() {
            let lower = (frequency - edges[mel]) / lower_span;
            let upper = (edges[mel + 2] - frequency) / upper_span;
            bank[mel * FFT_BINS + bin] = lower.min(upper).max(0.0) as f32;
        }
    }
    bank
}

fn fft_radix2(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert_eq!(n, im.len());
    debug_assert!(n.is_power_of_two());
    let mut reverse = 0usize;
    for index in 0..n {
        if index < reverse {
            re.swap(index, reverse);
            im.swap(index, reverse);
        }
        let mut bit = n >> 1;
        while reverse & bit != 0 {
            reverse ^= bit;
            bit >>= 1;
        }
        reverse |= bit;
    }
    let mut span = 2usize;
    while span <= n {
        let angle = -2.0 * PI / span as f64;
        let (step_im, step_re) = angle.sin_cos();
        for start in (0..n).step_by(span) {
            let (mut tw_re, mut tw_im) = (1.0f64, 0.0f64);
            for offset in 0..span / 2 {
                let lo = start + offset;
                let hi = lo + span / 2;
                let mixed_re = re[hi] * tw_re - im[hi] * tw_im;
                let mixed_im = re[hi] * tw_im + im[hi] * tw_re;
                let keep_re = re[lo];
                let keep_im = im[lo];
                re[lo] = keep_re + mixed_re;
                im[lo] = keep_im + mixed_im;
                re[hi] = keep_re - mixed_re;
                im[hi] = keep_im - mixed_im;
                let next_re = tw_re * step_re - tw_im * step_im;
                tw_im = tw_re * step_im + tw_im * step_re;
                tw_re = next_re;
            }
        }
        span <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_matches_centered_torch_geometry() {
        assert_eq!(LogMelSpect::frame_count(22_050), 51);
        assert_eq!(LogMelSpect::frame_count(441 * 1500), 1501);
        assert_eq!(LogMelSpect::frame_count(441 * 90), 91);
    }

    #[test]
    fn synthetic_sine_lands_in_the_expected_mel_region() {
        let seconds = 2usize;
        let frequency = 440.0f64;
        let signal: Vec<f32> = (0..SAMPLE_RATE as usize * seconds)
            .map(|sample| {
                (2.0 * PI * frequency * sample as f64 / SAMPLE_RATE as f64).sin() as f32
            })
            .collect();
        let front = LogMelSpect::new();
        let (mel, frames) = front.compute(&signal);
        assert_eq!(frames, 101);
        let middle = &mel[(frames / 2) * MEL_BINS..(frames / 2 + 1) * MEL_BINS];
        let peak = middle
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        let mel_min = hz_to_mel(F_MIN);
        let mel_max = hz_to_mel(F_MAX);
        let center_hz = mel_to_hz(
            mel_min + (mel_max - mel_min) * (peak + 1) as f64 / (MEL_BINS + 1) as f64,
        );
        assert!(
            (400.0..=500.0).contains(&center_hz),
            "440 Hz peak mapped to mel {peak} centered at {center_hz:.1} Hz"
        );
        assert!(middle[peak].is_finite() && middle[peak] > 0.0);
    }

    #[test]
    fn silence_is_exactly_zero_after_log1p() {
        let signal = vec![0.0f32; SAMPLE_RATE as usize];
        let (mel, _) = LogMelSpect::new().compute(&signal);
        assert!(mel.iter().all(|&value| value == 0.0));
    }
}

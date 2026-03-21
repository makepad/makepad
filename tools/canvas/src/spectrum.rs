use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

pub const NUM_BANDS: usize = 16;

pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    fft_size: usize,
    buffer: Vec<Complex<f32>>,
    window: Vec<f32>,
    pub bands: [f32; NUM_BANDS],
}

impl SpectrumAnalyzer {
    pub fn new(fft_size: usize) -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);

        // Hann window
        let window: Vec<f32> = (0..fft_size)
            .map(|i| {
                let t = i as f32 / (fft_size - 1) as f32;
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * t).cos())
            })
            .collect();

        Self {
            fft,
            fft_size,
            buffer: vec![Complex::new(0.0, 0.0); fft_size],
            window,
            bands: [0.0; NUM_BANDS],
        }
    }

    /// Analyze mono samples (or first channel of interleaved stereo).
    /// `samples` should have at least `fft_size` elements.
    /// `channels` indicates interleaving (1=mono, 2=stereo).
    pub fn analyze(&mut self, samples: &[f32], channels: usize) {
        let needed = self.fft_size * channels;
        if samples.len() < needed {
            return;
        }

        // Fill buffer with windowed samples (take first channel)
        for i in 0..self.fft_size {
            let s = samples[i * channels];
            self.buffer[i] = Complex::new(s * self.window[i], 0.0);
        }

        self.fft.process(&mut self.buffer);

        // Map FFT bins to frequency bands (log-spaced)
        let nyquist = self.fft_size / 2;
        for band in 0..NUM_BANDS {
            // Log-spaced bin ranges
            let lo = ((band as f32 / NUM_BANDS as f32).powf(2.0) * nyquist as f32) as usize;
            let hi = (((band + 1) as f32 / NUM_BANDS as f32).powf(2.0) * nyquist as f32) as usize;
            let lo = lo.max(1);
            let hi = hi.max(lo + 1).min(nyquist);

            let mut sum = 0.0f32;
            let count = (hi - lo) as f32;
            for bin in lo..hi {
                sum += self.buffer[bin].norm();
            }
            let avg = sum / count.max(1.0);
            // Normalize: scale magnitude to roughly 0-1 range
            let normalized = (avg / (self.fft_size as f32).sqrt() * 4.0).min(1.0);

            // Smooth: decay slowly, rise quickly
            let prev = self.bands[band];
            self.bands[band] = if normalized > prev {
                prev * 0.3 + normalized * 0.7
            } else {
                prev * 0.85 + normalized * 0.15
            };
        }
    }
}

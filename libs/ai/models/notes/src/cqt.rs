//! nnAudio-style CQT and harmonic stacking used by the published checkpoint.
//!
//! The ONNX artifact contains the learned/exported 36-bin complex analysis
//! kernels and anti-alias downsampler. We execute that filter bank natively:
//! nine octave stages, reflection padding for analysis, octave-by-octave
//! low-pass/downsample, magnitude, `NormalizedLog`, scalar BatchNorm, then
//! the eight harmonic shifts. No ONNX runtime is involved.

use crate::config::*;
use crate::weights::CqtWeights;

#[derive(Clone, Debug)]
pub struct Cqt {
    weights: CqtWeights,
}

#[derive(Clone, Debug)]
pub struct CqtSpectrogram {
    pub frames: usize,
    pub bins: usize,
    /// Time-major `[frame][bin]`, after NormalizedLog and input BatchNorm.
    pub data: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct HarmonicFeatures {
    pub channels: usize,
    pub frames: usize,
    pub bins: usize,
    /// Channel-major `[channel][frame][bin]`.
    pub data: Vec<f32>,
}

impl Cqt {
    pub fn new(weights: CqtWeights) -> Self {
        Self { weights }
    }

    pub fn spectrogram(&self, audio: &[f32]) -> Result<CqtSpectrogram, String> {
        if audio.is_empty() {
            return Ok(CqtSpectrogram {
                frames: 0,
                bins: CQT_BINS,
                data: Vec::new(),
            });
        }
        let frames = frame_count(audio.len());
        let mut levels = Vec::with_capacity(CQT_OCTAVES);
        let mut signal = audio.to_vec();
        for level in 0..CQT_OCTAVES {
            let stride = FFT_HOP >> level;
            if stride == 0 {
                return Err("CQT octave count exceeds FFT hop".to_string());
            }
            let mut block = vec![(0.0f32, 0.0f32); frames * CQT_BLOCK_BINS];
            let available = (signal.len() + FFT_HOP) // reflection pad: 128 each side
                .saturating_sub(CQT_KERNEL)
                / stride
                + 1;
            for frame in 0..frames.min(available) {
                let origin = frame * stride;
                for bin in 0..CQT_BLOCK_BINS {
                    let kernel = bin * CQT_KERNEL;
                    let mut real = self.weights.bias[bin];
                    let mut imag = self.weights.bias[bin];
                    for tap in 0..CQT_KERNEL {
                        let index = reflect_index(origin as isize + tap as isize - 128, signal.len());
                        let sample = signal[index];
                        real += sample * self.weights.real[kernel + tap];
                        imag += sample * self.weights.imag[kernel + tap];
                    }
                    block[frame * CQT_BLOCK_BINS + bin] = (real, imag);
                }
            }
            levels.push(block);
            if level + 1 < CQT_OCTAVES {
                signal = downsample(&signal, &self.weights.downsample);
            }
        }

        // nnAudio concatenates the lowest octave first and crops the first 15
        // bins, leaving exactly the 309 bins whose first centre is 27.5 Hz.
        let cropped = CQT_OCTAVES * CQT_BLOCK_BINS - CQT_BINS;
        let mut data = vec![0.0f32; frames * CQT_BINS];
        for frame in 0..frames {
            for bin in 0..CQT_BINS {
                let all_bin = cropped + bin;
                let reverse_level = all_bin / CQT_BLOCK_BINS;
                let level = CQT_OCTAVES - 1 - reverse_level;
                let local_bin = all_bin % CQT_BLOCK_BINS;
                let (real, imag) = levels[level][frame * CQT_BLOCK_BINS + local_bin];
                let scale = self.weights.normalization[bin];
                let real = real * scale;
                let imag = imag * scale;
                data[frame * CQT_BINS + bin] = (real * real + imag * imag).sqrt();
            }
        }

        normalized_log(&mut data);
        for value in &mut data {
            *value = *value * self.weights.input_bn_scale + self.weights.input_bn_bias;
        }
        Ok(CqtSpectrogram {
            frames,
            bins: CQT_BINS,
            data,
        })
    }

    pub fn transform(&self, audio: &[f32]) -> Result<HarmonicFeatures, String> {
        let cqt = self.spectrogram(audio)?;
        let shifts = harmonic_shifts();
        let mut data = vec![0.0; shifts.len() * cqt.frames * CONTOUR_BINS];
        for (channel, shift) in shifts.into_iter().enumerate() {
            for frame in 0..cqt.frames {
                for bin in 0..CONTOUR_BINS {
                    let source = bin as isize + shift;
                    if (0..cqt.bins as isize).contains(&source) {
                        data[(channel * cqt.frames + frame) * CONTOUR_BINS + bin] =
                            cqt.data[frame * cqt.bins + source as usize];
                    }
                }
            }
        }
        Ok(HarmonicFeatures {
            channels: shifts.len(),
            frames: cqt.frames,
            bins: CONTOUR_BINS,
            data,
        })
    }
}

fn reflect_index(index: isize, len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let period = 2 * (len as isize - 1);
    let mut index = index % period;
    if index < 0 {
        index += period;
    }
    if index >= len as isize {
        index = period - index;
    }
    index as usize
}

fn downsample(input: &[f32], kernel: &[f32]) -> Vec<f32> {
    debug_assert_eq!(kernel.len(), CQT_KERNEL);
    let mut output = vec![0.0f32; input.len() / 2];
    for (out_index, value) in output.iter_mut().enumerate() {
        let origin = (out_index * 2) as isize - 127;
        let mut sum = 0.0;
        for (tap, &weight) in kernel.iter().enumerate() {
            let index = origin + tap as isize;
            if (0..input.len() as isize).contains(&index) {
                sum += input[index as usize] * weight;
            }
        }
        *value = sum;
    }
    output
}

fn normalized_log(values: &mut [f32]) {
    if values.is_empty() {
        return;
    }
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for value in values.iter_mut() {
        *value = 10.0 * (*value * *value + 1.0e-10).log10();
        min = min.min(*value);
        max = max.max(*value);
    }
    let range = max - min;
    if range > 0.0 && range.is_finite() {
        for value in values {
            *value = (*value - min) / range;
        }
    } else {
        values.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weights::NotesWeights;
    use std::path::Path;

    fn checkpoint() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../local/models/weights/basic_pitch/nmp.onnx")
    }

    #[test]
    fn frame_count_matches_checkpoint_and_arbitrary_lengths() {
        assert_eq!(frame_count(AUDIO_N_SAMPLES), 172);
        assert_eq!(frame_count(22_050), 87);
        assert_eq!(frame_count(256), 2);
        assert_eq!(frame_count(257), 2);
    }

    #[test]
    fn sine_440_lights_a4_and_harmonic_stack_alignment() {
        let weights = NotesWeights::load(checkpoint()).unwrap();
        let cqt = Cqt::new(weights.cqt);
        let audio: Vec<f32> = (0..AUDIO_N_SAMPLES)
            .map(|i| (std::f64::consts::TAU * 440.0 * i as f64 / SAMPLE_RATE as f64).sin() as f32)
            .collect();
        let spectrum = cqt.spectrogram(&audio).unwrap();
        assert_eq!(spectrum.frames, WINDOW_FRAMES);
        let mut energy = vec![0.0f32; CQT_BINS];
        for frame in 8..spectrum.frames - 8 {
            for bin in 0..CQT_BINS {
                energy[bin] += spectrum.data[frame * CQT_BINS + bin];
            }
        }
        let peak = energy
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!((peak as isize - 144).abs() <= 1, "A4 CQT peak was bin {peak}");

        let stacked = cqt.transform(&audio).unwrap();
        let frame = WINDOW_FRAMES / 2;
        let fundamental = stacked.data[(WINDOW_FRAMES + frame) * CONTOUR_BINS + 144];
        let h2_at_a3 = stacked.data[(2 * WINDOW_FRAMES + frame) * CONTOUR_BINS + 108];
        assert!((fundamental - h2_at_a3).abs() < 1e-6);
    }
}

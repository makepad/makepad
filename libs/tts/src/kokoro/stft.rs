//! The generator's forward STFT (of the harmonic source) and the ISTFTNet
//! inverse that turns the network's last 22 channels back into samples.
//!
//! Both transforms are tiny — `n_fft = 20` — and the export bakes them as fixed
//! buffers rather than ops. The buffers reduce to closed forms, checked against
//! the ONNX initializers to 2e-9, so nothing extra has to ship with the weights.

use std::f32::consts::PI;

use super::ops::Mat;

pub const N_FFT: usize = 20;
pub const HOP: usize = 5;
/// Bins kept by a onesided transform: `n_fft / 2 + 1`.
pub const BINS: usize = N_FFT / 2 + 1;

/// Periodic Hann, matching `torch.hann_window(20)`.
fn window() -> [f32; N_FFT] {
    let mut w = [0.0; N_FFT];
    for (n, value) in w.iter_mut().enumerate() {
        *value = 0.5 - 0.5 * (2.0 * PI * n as f32 / N_FFT as f32).cos();
    }
    w
}

/// Reflect around the edge sample without repeating it — `torch.stft`'s
/// `center=True, pad_mode='reflect'`.
fn reflect(signal: &[f32], pad: usize) -> Vec<f32> {
    let mut out = vec![0.0; signal.len() + 2 * pad];
    for i in 0..pad {
        out[i] = signal[pad - i];
    }
    out[pad..pad + signal.len()].copy_from_slice(signal);
    for j in 0..pad {
        out[pad + signal.len() + j] = signal[signal.len() - 2 - j];
    }
    out
}

/// `[magnitude (11 rows) | phase (11 rows)]`, the generator's `har` tensor.
pub fn stft_mag_phase(signal: &[f32]) -> Mat {
    let window = window();
    let padded = reflect(signal, N_FFT / 2);
    let frames = (padded.len() - N_FFT) / HOP + 1;

    let mut cos = [[0.0f32; N_FFT]; BINS];
    let mut sin = [[0.0f32; N_FFT]; BINS];
    for k in 0..BINS {
        for n in 0..N_FFT {
            let angle = 2.0 * PI * k as f32 * n as f32 / N_FFT as f32;
            cos[k][n] = angle.cos();
            sin[k][n] = angle.sin();
        }
    }

    let mut out = Mat::zeros(2 * BINS, frames);
    for t in 0..frames {
        let chunk = &padded[t * HOP..t * HOP + N_FFT];
        for k in 0..BINS {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for n in 0..N_FFT {
                let value = chunk[n] * window[n];
                re += value * cos[k][n];
                im -= value * sin[k][n];
            }
            out.data[k * frames + t] = (re * re + im * im).sqrt();
            // The export hand-rolls the quadrant fix as
            // `where(re < 0, atan ± pi, atan)`. The tie at `im == 0, re < 0`
            // matters: the DC bin of a real signal lives exactly on that edge
            // (its imaginary part is identically ±0 here), and both atan2 and
            // the reference dump resolve it to +pi. Sending it to -pi instead
            // put a near-constant 2*pi offset on the DC phase row — a raw
            // feature into `noise_convs` — which the network turned into a
            // steady hop-rate ring at 4.8/9.6 kHz and a collapsed 10-12 kHz
            // band. See `har_bisect`.
            let at = (im / re).atan();
            out.data[(BINS + k) * frames + t] = if re < 0.0 {
                if im < 0.0 {
                    at - PI
                } else {
                    at + PI
                }
            } else {
                at
            };
        }
    }
    out
}

/// The transposed-convolution kernel the export stores as `inverse_basis`,
/// `[2 * BINS, N_FFT]`.
///
/// It is the windowed irfft basis scaled by `hop / n_fft`: row `k` holds
/// `0.25 * a_k * cos(2*pi*k*n/N) * w[n] / N`, and row `BINS + k` the matching
/// `-sin` term. `a_k` is 2 except at DC and Nyquist, which are their own
/// conjugates.
fn inverse_basis() -> Vec<f32> {
    let window = window();
    let mut basis = vec![0.0; 2 * BINS * N_FFT];
    let scale = HOP as f32 / N_FFT as f32;
    for k in 0..BINS {
        let a = if k == 0 || k == N_FFT / 2 { 1.0 } else { 2.0 };
        for n in 0..N_FFT {
            let angle = 2.0 * PI * k as f32 * n as f32 / N_FFT as f32;
            let gain = scale * a * window[n] / N_FFT as f32;
            basis[k * N_FFT + n] = gain * angle.cos();
            basis[(BINS + k) * N_FFT + n] = -gain * angle.sin();
        }
    }
    basis
}

/// Overlap-add `[spec*cos(phase) | spec*sin(phase)]` back into samples.
///
/// Two quirks of the export, both reproduced here because they are what the
/// reference actually computes:
///
/// * `window_sum` is only `n_fft` long, so the window-envelope division lands on
///   the first 20 samples and nowhere else. A true iSTFT would divide
///   everywhere; the traced graph does not.
/// * The `hop / n_fft` folded into `inverse_basis` is then undone by a constant
///   `Mul` of 4.
///
/// Verified elementwise against the reference waveform: max |Δ| 5.3e-7.
pub fn istft(re_im: &Mat) -> Vec<f32> {
    let frames = re_im.cols;
    let basis = inverse_basis();

    let mut samples = vec![0.0f32; HOP * (frames - 1) + N_FFT];
    for channel in 0..2 * BINS {
        let taps = &basis[channel * N_FFT..(channel + 1) * N_FFT];
        let input = re_im.row(channel);
        for (t, value) in input.iter().enumerate() {
            let at = t * HOP;
            for n in 0..N_FFT {
                samples[at + n] += value * taps[n];
            }
        }
    }

    let window = window();
    for (value, w) in samples.iter_mut().zip(window.iter()) {
        let squared = w * w;
        if squared > f32::MIN_POSITIVE {
            *value /= squared;
        }
    }
    let gain = (N_FFT / HOP) as f32;
    for value in samples.iter_mut() {
        *value *= gain;
    }

    samples[N_FFT / 2..samples.len() - N_FFT / 2].to_vec()
}

//! IndexTTS-2.5 HiFiGAN-style log-mel front-end (voice prompt -> 80-band mel
//! @ 22.05 kHz), CPU f32.
//!
//! Port of `indextts/s2mel/modules/audio.py::mel_spectrogram` with the
//! IndexTTS2 `mel_fn_args`: n_fft 1024, win 1024, hop 256, 80 mels,
//! sr 22050, fmin 0, fmax None, center=False. The reference:
//!
//! 1. reflect-pads the waveform by (n_fft - hop)/2 = 384 on both sides,
//! 2. torch.stft(center=False) with a periodic Hann window,
//! 3. magnitude `sqrt(re^2 + im^2 + 1e-9)`,
//! 4. matmul with `librosa.filters.mel(sr=22050, n_fft=1024, n_mels=80,
//!    fmin=0, fmax=None)` — Slaney mel scale + Slaney area normalization
//!    (ported exactly from librosa/filters.py + librosa/core/convert.py),
//! 5. `log(clamp(x, min=1e-5))` (dynamic_range_compression).
//!
//! The FFT is a self-contained safe iterative radix-2 Cooley-Tukey (the
//! straightforward textbook form; same approach as libs/voice/src/cpu/mel.rs
//! but without the unsafe pointer recursion). Intermediates run in f64 and
//! are cast to f32 at the end — well inside the log-domain gate vs torch's
//! all-f32 pipeline.

pub const MEL_SAMPLE_RATE: usize = 22_050;
pub const MEL_N_FFT: usize = 1024;
pub const MEL_WIN_SIZE: usize = 1024;
pub const MEL_HOP_SIZE: usize = 256;
pub const MEL_BANDS: usize = 80;
/// One-sided spectrum bins: n_fft/2 + 1.
pub const MEL_FFT_BINS: usize = MEL_N_FFT / 2 + 1;

// ---------------------------------------------------------------------------
// librosa Slaney mel filter bank.
// ---------------------------------------------------------------------------

/// librosa hz_to_mel(htk=False): linear below 1 kHz (200/3 Hz per mel), log
/// above with step log(6.4)/27.
fn hz_to_mel_slaney(hz: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    if hz >= MIN_LOG_HZ {
        let logstep = 6.4f64.ln() / 27.0;
        MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / logstep
    } else {
        hz / F_SP
    }
}

/// librosa mel_to_hz(htk=False), inverse of [`hz_to_mel_slaney`].
fn mel_to_hz_slaney(mel: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    if mel >= MIN_LOG_MEL {
        let logstep = 6.4f64.ln() / 27.0;
        MIN_LOG_HZ * (logstep * (mel - MIN_LOG_MEL)).exp()
    } else {
        F_SP * mel
    }
}

/// `librosa.filters.mel(sr=.., n_fft=.., n_mels=.., fmin=.., fmax=..)` with
/// the defaults `htk=False` (Slaney scale) and `norm="slaney"` (2 / bandwidth
/// area normalization). Returns `(n_mels, n_fft/2 + 1)` row-major f32,
/// computed in f64 and cast (librosa stores into a float32 array the same
/// way). `fmax: None` means `sr / 2`.
pub fn mel_filterbank_slaney(
    sr: f64,
    n_fft: usize,
    n_mels: usize,
    fmin: f64,
    fmax: Option<f64>,
) -> Vec<f32> {
    let fmax = fmax.unwrap_or(sr / 2.0);
    let n_bins = n_fft / 2 + 1;

    // Center frequencies of FFT bins: linspace(0, sr/2, n_bins).
    let fftfreqs: Vec<f64> = (0..n_bins)
        .map(|k| k as f64 * (sr / 2.0) / (n_bins - 1) as f64)
        .collect();

    // Mel band edges: uniform in mel between fmin and fmax, n_mels + 2 points.
    let mel_min = hz_to_mel_slaney(fmin);
    let mel_max = hz_to_mel_slaney(fmax);
    let mel_f: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz_slaney(mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64))
        .collect();

    let mut weights = vec![0f32; n_mels * n_bins];
    for i in 0..n_mels {
        let fdiff_lo = mel_f[i + 1] - mel_f[i];
        let fdiff_hi = mel_f[i + 2] - mel_f[i + 1];
        // Slaney area normalization: 2 / band width.
        let enorm = 2.0 / (mel_f[i + 2] - mel_f[i]);
        let row = &mut weights[i * n_bins..(i + 1) * n_bins];
        for (k, w) in row.iter_mut().enumerate() {
            let lower = (fftfreqs[k] - mel_f[i]) / fdiff_lo;
            let upper = (mel_f[i + 2] - fftfreqs[k]) / fdiff_hi;
            let tri = lower.min(upper).max(0.0);
            *w = (tri * enorm) as f32;
        }
    }
    weights
}

/// The exact filter bank the IndexTTS mel front-end uses:
/// `librosa.filters.mel(sr=22050, n_fft=1024, n_mels=80, fmin=0, fmax=None)`,
/// `(80, 513)` row-major.
pub fn mel_filterbank_22k() -> Vec<f32> {
    mel_filterbank_slaney(
        MEL_SAMPLE_RATE as f64,
        MEL_N_FFT,
        MEL_BANDS,
        0.0,
        None,
    )
}

// ---------------------------------------------------------------------------
// FFT: safe iterative radix-2 Cooley-Tukey (power-of-two sizes).
// ---------------------------------------------------------------------------

/// In-place complex FFT over parallel re/im slices; `re.len()` must be a
/// power of two. Standard bit-reversal permutation + butterfly passes,
/// negative-exponent (forward / torch.stft) convention.
fn fft_radix2(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert_eq!(n, im.len());
    debug_assert!(n.is_power_of_two());
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
    }
    // Butterflies.
    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (w_re, w_im) = (ang.cos(), ang.sin());
        let mut start = 0usize;
        while start < n {
            let (mut cur_re, mut cur_im) = (1.0f64, 0.0f64);
            for k in start..start + len / 2 {
                let (ur, ui) = (re[k], im[k]);
                let (vr0, vi0) = (re[k + len / 2], im[k + len / 2]);
                let vr = vr0 * cur_re - vi0 * cur_im;
                let vi = vr0 * cur_im + vi0 * cur_re;
                re[k] = ur + vr;
                im[k] = ui + vi;
                re[k + len / 2] = ur - vr;
                im[k + len / 2] = ui - vi;
                let next_re = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = next_re;
            }
            start += len;
        }
        len <<= 1;
    }
}

// ---------------------------------------------------------------------------
// mel_spectrogram
// ---------------------------------------------------------------------------

/// The reference reflect pad: `F.pad(x, (pad, pad), mode="reflect")` — edge
/// sample not repeated. `samples.len()` must exceed `pad`.
fn reflect_pad(samples: &[f32], pad: usize) -> Vec<f64> {
    let n = samples.len();
    assert!(
        n > pad,
        "mel_spectrogram: need more than {pad} samples for reflect padding, got {n}"
    );
    let mut out = Vec::with_capacity(n + 2 * pad);
    for i in 0..pad {
        out.push(samples[pad - i] as f64);
    }
    out.extend(samples.iter().map(|&v| v as f64));
    for j in 0..pad {
        out.push(samples[n - 2 - j] as f64);
    }
    out
}

/// `audio.py::mel_spectrogram` @ the IndexTTS 22.05 kHz settings: waveform ->
/// `(80, frames)` log-mel, row-major `[mel][t]`. Returns the mel plane and
/// the frame count; `frames = (len + 2*384 - 1024)/256 + 1`.
pub fn mel_spectrogram_22k(samples: &[f32]) -> (Vec<f32>, usize) {
    let pad = (MEL_N_FFT - MEL_HOP_SIZE) / 2; // 384
    let padded = reflect_pad(samples, pad);
    let frames = (padded.len() - MEL_N_FFT) / MEL_HOP_SIZE + 1;

    // Periodic Hann window (torch.hann_window default).
    let window: Vec<f64> = (0..MEL_WIN_SIZE)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / MEL_WIN_SIZE as f64).cos())
        })
        .collect();
    let filterbank = mel_filterbank_22k();

    let mut mel = vec![0f32; MEL_BANDS * frames];
    let mut re = vec![0f64; MEL_N_FFT];
    let mut im = vec![0f64; MEL_N_FFT];
    let mut mag = vec![0f64; MEL_FFT_BINS];
    for t in 0..frames {
        let frame = &padded[t * MEL_HOP_SIZE..t * MEL_HOP_SIZE + MEL_N_FFT];
        for (i, (r, &x)) in re.iter_mut().zip(frame).enumerate() {
            *r = x * window[i];
        }
        im.fill(0.0);
        fft_radix2(&mut re, &mut im);
        for (k, m) in mag.iter_mut().enumerate() {
            *m = (re[k] * re[k] + im[k] * im[k] + 1e-9).sqrt();
        }
        for b in 0..MEL_BANDS {
            let row = &filterbank[b * MEL_FFT_BINS..(b + 1) * MEL_FFT_BINS];
            let mut sum = 0f64;
            for (&w, &m) in row.iter().zip(mag.iter()) {
                sum += w as f64 * m;
            }
            // spectral_normalize: log(clamp(x, min=1e-5)).
            mel[b * frames + t] = (sum.max(1e-5)).ln() as f32;
        }
    }
    (mel, frames)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins from the reference venv:
    /// `librosa.filters.mel(sr=22050, n_fft=1024, n_mels=80, fmin=0, fmax=None)`.
    #[test]
    fn filterbank_matches_librosa_pins() {
        let fb = mel_filterbank_22k();
        assert_eq!(fb.len(), 80 * 513);
        let row = |i: usize| &fb[i * 513..(i + 1) * 513];
        let sum = |i: usize| row(i).iter().map(|&v| v as f64).sum::<f64>();
        let argmax = |i: usize| {
            row(i)
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap()
        };
        // (row, sum, argmax bin, max value) from librosa 0.11 in the ref venv.
        for (i, want_sum, want_arg, want_max) in [
            (0usize, 4.633117467e-2, 2usize, 2.316558734e-2),
            (1, 4.633117467e-2, 4, 2.198764496e-2),
            (40, 4.664323851e-2, 94, 1.151498687e-2),
            (79, 4.643371701e-2, 491, 2.208118560e-3),
        ] {
            let s = sum(i);
            assert!((s - want_sum).abs() < 1e-7, "row {i} sum {s} vs {want_sum}");
            let (arg, &max) = argmax(i);
            assert_eq!(arg, want_arg, "row {i} argmax");
            assert!(
                (max as f64 - want_max).abs() < 1e-8,
                "row {i} max {max} vs {want_max}"
            );
        }
        let total: f64 = fb.iter().map(|&v| v as f64).sum();
        assert!((total - 3.714647055).abs() < 1e-6, "total sum {total}");
        // Every filter has support.
        for i in 0..80 {
            assert!(row(i).iter().any(|&v| v > 0.0), "row {i} empty");
        }
    }

    #[test]
    fn fft_matches_naive_dft() {
        let n = 256usize;
        // Deterministic pseudo-signal with several harmonics + "noise".
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64;
                (0.03 * t).sin() + 0.5 * (0.31 * t).cos() + 0.1 * ((i * 2654435761usize) % 1000) as f64 / 1000.0
            })
            .collect();
        let mut re = x.clone();
        let mut im = vec![0f64; n];
        fft_radix2(&mut re, &mut im);
        for k in (0..n).step_by(17) {
            let mut dr = 0f64;
            let mut di = 0f64;
            for (j, &v) in x.iter().enumerate() {
                let ang = -2.0 * std::f64::consts::PI * (k * j) as f64 / n as f64;
                dr += v * ang.cos();
                di += v * ang.sin();
            }
            assert!(
                (re[k] - dr).abs() < 1e-9 && (im[k] - di).abs() < 1e-9,
                "bin {k}: fft ({}, {}) vs dft ({dr}, {di})",
                re[k],
                im[k]
            );
        }
    }

    #[test]
    fn reflect_pad_matches_torch_semantics() {
        // F.pad([0,1,2,3,4], (3,3), reflect) = [3,2,1,0,1,2,3,4,3,2,1]
        let padded = reflect_pad(&[0.0, 1.0, 2.0, 3.0, 4.0], 3);
        let expect = [3.0, 2.0, 1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0];
        assert_eq!(padded.len(), expect.len());
        for (a, b) in padded.iter().zip(expect.iter()) {
            assert_eq!(*a, *b);
        }
    }

    #[test]
    fn frame_count_formula() {
        // len + 768 padded; frames = (padded - 1024)/256 + 1.
        let samples = vec![0f32; 66702];
        let (mel, frames) = mel_spectrogram_22k(&samples);
        assert_eq!(frames, 260);
        assert_eq!(mel.len(), 80 * 260);
        // All-zero input: magnitude sqrt(1e-9) per bin; mel = ln(clamp(fbank
        // row sum * sqrt(1e-9), 1e-5)) — every value equal per row, finite.
        assert!(mel.iter().all(|v| v.is_finite()));
    }

    /// Oracle smoke (skipped when the reference dumps are absent):
    /// audio_22k.npy -> ref_mel.npy at a loose threshold; the tight gate
    /// lives in the indextts-bigvgan-validate bin.
    #[test]
    fn oracle_smoke_ref_mel() {
        let dir = crate::indextts::reference_dumps_dir();
        let audio_path = dir.join("audio_22k.npy");
        if !audio_path.is_file() {
            eprintln!("skipping oracle_smoke_ref_mel: {audio_path:?} missing");
            return;
        }
        let audio = read_npy_f32(&audio_path);
        let reference = read_npy_f32(&dir.join("ref_mel.npy"));
        let (mel, frames) = mel_spectrogram_22k(&audio);
        assert_eq!(frames, reference.len() / 80);
        let mut dot = 0f64;
        let mut na = 0f64;
        let mut nb = 0f64;
        let mut max_abs = 0f32;
        for (&a, &b) in mel.iter().zip(reference.iter()) {
            dot += a as f64 * b as f64;
            na += a as f64 * a as f64;
            nb += b as f64 * b as f64;
            max_abs = max_abs.max((a - b).abs());
        }
        let cos = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
        eprintln!("oracle_smoke_ref_mel: cos {cos:.7} max_abs {max_abs:.3e}");
        assert!(cos >= 0.999, "mel cosine {cos} below 0.999");
        assert!(max_abs <= 2e-3, "mel max abs {max_abs} above 2e-3");
    }

    /// Minimal f32 .npy reader for the test above (little-endian '<f4' only).
    fn read_npy_f32(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..6], b"\x93NUMPY");
        let (header_len, start) = if bytes[6] == 1 {
            (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
        } else {
            (
                u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
                12,
            )
        };
        let header = String::from_utf8_lossy(&bytes[start..start + header_len]).to_string();
        assert!(header.contains("<f4"), "expected f32 npy: {header}");
        assert!(!header.contains("'fortran_order': True"));
        bytes[start + header_len..]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }
}

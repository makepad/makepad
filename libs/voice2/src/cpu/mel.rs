use crate::model::MelFilters;

pub const WHISPER_SAMPLE_RATE: usize = 16000;
pub const WHISPER_N_FFT: usize = 400;
pub const WHISPER_HOP_LENGTH: usize = 160;
pub const WHISPER_CHUNK_SIZE: usize = 30; // seconds

/// Precomputed sine/cosine tables and Hann window for FFT.
pub struct MelCache {
    sin_vals: [f32; WHISPER_N_FFT],
    cos_vals: [f32; WHISPER_N_FFT],
    hann: [f32; WHISPER_N_FFT],
}

impl MelCache {
    pub fn new() -> Self {
        let mut sin_vals = [0.0f32; WHISPER_N_FFT];
        let mut cos_vals = [0.0f32; WHISPER_N_FFT];
        let mut hann = [0.0f32; WHISPER_N_FFT];

        for i in 0..WHISPER_N_FFT {
            let theta = 2.0 * std::f64::consts::PI * i as f64 / WHISPER_N_FFT as f64;
            sin_vals[i] = theta.sin() as f32;
            cos_vals[i] = theta.cos() as f32;
        }

        // Periodic Hann window
        for i in 0..WHISPER_N_FFT {
            hann[i] = (0.5
                * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / WHISPER_N_FFT as f64).cos()))
                as f32;
        }

        MelCache {
            sin_vals,
            cos_vals,
            hann,
        }
    }
}

/// Naive DFT for non-power-of-2 sizes (fallback).
fn dft(input: &[f32], n: usize, out: &mut [f32], cache: &MelCache) {
    let step = WHISPER_N_FFT / n;
    for k in 0..n {
        let mut re = 0.0f32;
        let mut im = 0.0f32;
        for j in 0..n {
            let idx = (k * j * step) % WHISPER_N_FFT;
            re += input[j] * cache.cos_vals[idx];
            im -= input[j] * cache.sin_vals[idx];
        }
        out[k * 2] = re;
        out[k * 2 + 1] = im;
    }
}

/// Cooley-Tukey FFT. Input is real-valued, output is complex (interleaved re, im).
/// `work` must have space for at least 4*N floats beyond `input`.
fn fft(input: &mut [f32], n: usize, out: &mut [f32], cache: &MelCache) {
    if n == 1 {
        out[0] = input[0];
        out[1] = 0.0;
        return;
    }

    let half = n / 2;
    if n - half * 2 == 1 {
        // Odd size, fallback to DFT
        dft(input, n, out, cache);
        return;
    }

    // Split even/odd into temp space after input
    let input_ptr = input.as_mut_ptr();
    unsafe {
        let even_ptr = input_ptr.add(n);
        for i in 0..half {
            *even_ptr.add(i) = *input_ptr.add(2 * i);
        }
        let even_fft_ptr = out.as_mut_ptr().add(2 * n);
        let even_slice = std::slice::from_raw_parts_mut(even_ptr, half + 4 * half);
        let even_out = std::slice::from_raw_parts_mut(even_fft_ptr, 2 * half + 4 * half);
        fft(even_slice, half, even_out, cache);

        let odd_ptr = even_ptr; // reuse
        for i in 0..half {
            *odd_ptr.add(i) = *input_ptr.add(2 * i + 1);
        }
        let odd_fft_ptr = even_fft_ptr.add(n); // after even_fft
        let odd_slice = std::slice::from_raw_parts_mut(odd_ptr, half + 4 * half);
        let odd_out = std::slice::from_raw_parts_mut(odd_fft_ptr, 2 * half + 4 * half);
        fft(odd_slice, half, odd_out, cache);

        // Combine
        let step = WHISPER_N_FFT / n;
        for k in 0..half {
            let idx = k * step;
            let re = cache.cos_vals[idx];
            let im = -cache.sin_vals[idx];

            let re_odd = *odd_fft_ptr.add(2 * k);
            let im_odd = *odd_fft_ptr.add(2 * k + 1);

            let out_ptr = out.as_mut_ptr();
            *out_ptr.add(2 * k) = *even_fft_ptr.add(2 * k) + re * re_odd - im * im_odd;
            *out_ptr.add(2 * k + 1) = *even_fft_ptr.add(2 * k + 1) + re * im_odd + im * re_odd;

            *out_ptr.add(2 * (k + half)) = *even_fft_ptr.add(2 * k) - re * re_odd + im * im_odd;
            *out_ptr.add(2 * (k + half) + 1) =
                *even_fft_ptr.add(2 * k + 1) - re * im_odd - im * re_odd;
        }
    }
}

/// Compute log mel spectrogram from PCM samples.
/// Returns mel data in shape [n_mel, n_len].
pub fn log_mel_spectrogram(
    samples: &[f32],
    filters: &MelFilters,
    n_threads: usize,
) -> (Vec<f32>, usize, usize, usize) {
    let cache = MelCache::new();
    let n_samples = samples.len();
    let frame_size = WHISPER_N_FFT;
    let frame_step = WHISPER_HOP_LENGTH;
    let n_mel = filters.n_mel as usize;
    let n_fft = filters.n_fft as usize; // should be 1 + WHISPER_N_FFT/2 = 201

    // Padding: 30 seconds of zeros + reflective pad at edges
    let stage_1_pad = WHISPER_SAMPLE_RATE * WHISPER_CHUNK_SIZE; // 480000
    let stage_2_pad = frame_size / 2; // 200

    let padded_len = n_samples + stage_1_pad + stage_2_pad * 2;
    let mut padded = vec![0.0f32; padded_len];

    // Copy audio with stage_2_pad offset
    padded[stage_2_pad..stage_2_pad + n_samples].copy_from_slice(samples);

    // Reflective pad at beginning
    for i in 0..stage_2_pad.min(n_samples) {
        padded[stage_2_pad - 1 - i] = samples[i + 1.min(n_samples - 1)];
    }

    // Zero pad the rest (already zeroed)

    let n_len = (padded_len - frame_size) / frame_step;
    let n_len_org = 1 + (n_samples + stage_2_pad - frame_size) / frame_step;

    let mut mel_data = vec![0.0f32; n_mel * n_len];

    // Process each frame (single-threaded for now, n_threads ignored)
    let _ = n_threads;
    let n_samples_padded = n_samples + stage_2_pad;

    // Work buffers for FFT
    let mut fft_in = vec![0.0f32; frame_size * 4]; // extra space for FFT recursion
    let mut fft_out = vec![0.0f32; frame_size * 8];

    for i in 0..n_len {
        let offset = i * frame_step;

        // Apply Hann window
        let end = (n_samples_padded).min(offset + frame_size);
        for j in 0..frame_size {
            if offset + j < end {
                fft_in[j] = cache.hann[j] * padded[offset + j];
            } else {
                fft_in[j] = 0.0;
            }
        }
        // Clear extra space
        for j in frame_size..fft_in.len() {
            fft_in[j] = 0.0;
        }

        // FFT
        fft(&mut fft_in, frame_size, &mut fft_out, &cache);

        // Magnitude squared
        for j in 0..n_fft {
            fft_out[j] = fft_out[2 * j] * fft_out[2 * j] + fft_out[2 * j + 1] * fft_out[2 * j + 1];
        }

        // Apply mel filterbank
        for j in 0..n_mel {
            let mut sum = 0.0f64;
            let filter_row = &filters.data[j * n_fft..(j + 1) * n_fft];
            // Unrolled by 4
            let mut k = 0;
            while k + 3 < n_fft {
                sum += fft_out[k] as f64 * filter_row[k] as f64
                    + fft_out[k + 1] as f64 * filter_row[k + 1] as f64
                    + fft_out[k + 2] as f64 * filter_row[k + 2] as f64
                    + fft_out[k + 3] as f64 * filter_row[k + 3] as f64;
                k += 4;
            }
            while k < n_fft {
                sum += fft_out[k] as f64 * filter_row[k] as f64;
                k += 1;
            }
            mel_data[j * n_len + i] = sum.max(1e-10).log10() as f32;
        }
    }

    // Fill remaining frames (beyond audio) with log(1e-10)
    let log_min = (1e-10f64).log10() as f32;
    for i in 0..n_len {
        if i * frame_step >= n_samples_padded {
            for j in 0..n_mel {
                mel_data[j * n_len + i] = log_min;
            }
        }
    }

    // Clamping and normalization
    let mut max_val = f32::NEG_INFINITY;
    for &v in &mel_data {
        if v > max_val {
            max_val = v;
        }
    }
    max_val -= 8.0;

    for v in &mut mel_data {
        if *v < max_val {
            *v = max_val;
        }
        *v = (*v + 4.0) / 4.0;
    }

    (mel_data, n_mel, n_len, n_len_org)
}

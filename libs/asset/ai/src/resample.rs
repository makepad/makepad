//! Small rational polyphase resampler (windowed-sinc) for feeding platform
//! AAC encoders: the Windows Media Foundation AAC MFT only accepts 44.1/48 kHz
//! PCM, while e.g. the MiniMax H3 audio VAE emits 32 kHz — a clean 2:3
//! upsample. Linear interpolation is deliberately avoided (audible imaging);
//! this is a proper lowpass-interpolating kernel, still only ~25 lines of hot
//! loop.

/// Resample one mono f32 channel from `in_rate` to `out_rate`.
/// Output length = floor(len * out_rate / in_rate). Identity when rates match.
pub fn resample_channel(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    assert!(in_rate > 0 && out_rate > 0);
    if in_rate == out_rate || input.is_empty() {
        return input.to_vec();
    }
    let g = gcd(in_rate as u64, out_rate as u64);
    let up = (out_rate as u64 / g) as usize; // L: phases
    let down = (in_rate as u64 / g) as usize; // M: input step per L outputs

    // Windowed-sinc kernel, HALF taps each side, cutoff at the lower of the
    // two Nyquists (with a little rolloff margin so the transition band stays
    // inside).
    const HALF: i64 = 16;
    let cutoff = 0.5 * 0.92 * (out_rate.min(in_rate) as f64 / in_rate as f64);

    // Per-phase FIR: phase p covers fractional offset p/up (in input samples).
    let mut kernels = Vec::with_capacity(up);
    for phase in 0..up {
        let frac = phase as f64 / up as f64;
        let mut taps = Vec::with_capacity((2 * HALF) as usize);
        let mut sum = 0.0f64;
        for k in -HALF + 1..=HALF {
            let t = k as f64 - frac;
            let sinc = if t == 0.0 {
                1.0
            } else {
                let x = std::f64::consts::PI * 2.0 * cutoff * t;
                x.sin() / x
            };
            // Blackman window over the tap span.
            let wx = (t + HALF as f64) / (2.0 * HALF as f64);
            let window = if (0.0..=1.0).contains(&wx) {
                0.42 - 0.5 * (2.0 * std::f64::consts::PI * wx).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * wx).cos()
            } else {
                0.0
            };
            let tap = 2.0 * cutoff * sinc * window;
            sum += tap;
            taps.push(tap);
        }
        // Normalize each phase to unity DC gain (windowing slightly detunes it).
        for tap in &mut taps {
            *tap /= sum;
        }
        kernels.push(taps);
    }

    let out_len = input.len() * up / down;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let num = n * down;
        let base = (num / up) as i64;
        let phase = num % up;
        let taps = &kernels[phase];
        let mut acc = 0.0f64;
        for (i, k) in (-HALF + 1..=HALF).enumerate() {
            let idx = base + k;
            if idx >= 0 && (idx as usize) < input.len() {
                acc += input[idx as usize] as f64 * taps[i];
            }
        }
        out.push(acc as f32);
    }
    out
}

/// Interleave two equal-length channels and quantize to i16 with clamping.
pub fn interleave_stereo_i16(left: &[f32], right: &[f32]) -> Vec<i16> {
    let len = left.len().min(right.len());
    let mut out = Vec::with_capacity(len * 2);
    for i in 0..len {
        out.push(quantize_i16(left[i]));
        out.push(quantize_i16(right[i]));
    }
    out
}

pub fn quantize_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_when_rates_match() {
        let x = vec![0.1, -0.2, 0.3];
        assert_eq!(resample_channel(&x, 32000, 32000), x);
    }

    #[test]
    fn length_ratio_32k_to_48k() {
        let x = vec![0.0f32; 32000];
        let y = resample_channel(&x, 32000, 48000);
        assert_eq!(y.len(), 48000);
    }

    #[test]
    fn dc_preserved() {
        let x = vec![0.7f32; 4000];
        let y = resample_channel(&x, 32000, 48000);
        // Interior samples (away from the kernel edges).
        for &v in &y[100..y.len() - 100] {
            assert!((v - 0.7).abs() < 1e-3, "dc drifted: {v}");
        }
    }

    #[test]
    fn sine_reconstruction_32k_to_48k() {
        let in_rate = 32000u32;
        let out_rate = 48000u32;
        let freq = 1000.0f64;
        let x: Vec<f32> = (0..3200)
            .map(|n| (2.0 * std::f64::consts::PI * freq * n as f64 / in_rate as f64).sin() as f32)
            .collect();
        let y = resample_channel(&x, in_rate, out_rate);
        let mut err = 0.0f64;
        let mut count = 0usize;
        for n in 100..y.len() - 100 {
            let t = n as f64 / out_rate as f64;
            let want = (2.0 * std::f64::consts::PI * freq * t).sin();
            err += (y[n] as f64 - want).powi(2);
            count += 1;
        }
        let rms = (err / count as f64).sqrt();
        assert!(rms < 2e-3, "sine rms error {rms}");
    }

    #[test]
    fn interleave_and_quantize() {
        let out = interleave_stereo_i16(&[0.5, -2.0], &[-0.5, 2.0]);
        assert_eq!(out, vec![16384, -16384, -32767, 32767]);
    }
}

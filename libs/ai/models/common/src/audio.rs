//! Audio helpers shared by the model families (reference-clip conditioning:
//! IndexTTS voice clips, Music3 reference audio).

/// Rational polyphase windowed-sinc resampler (Blackman window), mono. Used
/// on reference clips (any rate -> a model's native rate). Matches the
/// reference chains (librosa/soxr, torchaudio sinc) to feature-level
/// tolerance, not sample-exactly — stage validation injects dumped audio.
pub fn resample_mono(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = in_rate as f64 / out_rate as f64;
    // Anti-aliasing cutoff in cycles per INPUT sample: half the lower of the
    // two Nyquists.
    let cutoff = 0.5f64 * (out_rate as f64 / in_rate as f64).min(1.0);
    let half_taps = 32usize; // input samples per side
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let center = n as f64 * ratio;
        let start = (center.floor() as isize - half_taps as isize).max(0) as usize;
        let end = ((center.floor() as usize) + half_taps + 1).min(input.len());
        let mut acc = 0f64;
        let mut norm = 0f64;
        for (m, &sample) in input.iter().enumerate().take(end).skip(start) {
            let t = m as f64 - center;
            let x = 2.0 * cutoff * t;
            let sinc = if x == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            // Blackman window over [-half_taps, half_taps].
            let w = 0.42
                + 0.5 * (std::f64::consts::PI * t / half_taps as f64).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * t / half_taps as f64).cos();
            let tap = sinc * w;
            acc += sample as f64 * tap;
            norm += tap;
        }
        // Per-sample unity-DC normalization corrects both the 2*cutoff gain
        // factor and window truncation at the edges.
        out.push(if norm.abs() > 1e-12 { (acc / norm) as f32 } else { 0.0 });
    }
    out
}

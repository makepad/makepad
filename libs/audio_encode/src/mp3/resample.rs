//! Windowed-sinc sample-rate conversion, for sources at rates MPEG-1 has no
//! frame header for (22.05 kHz voice memos, 96 kHz masters).

const HALF_TAPS: i64 = 16;
const PHASES: usize = 256;

/// Convert one channel from `from` Hz to `to` Hz.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    // Low-pass at 95% of the lower Nyquist, Blackman-windowed.
    let cutoff = 0.95 * (to as f64 / from as f64).min(1.0);
    let taps = (2 * HALF_TAPS) as usize;
    let mut table = vec![0.0f32; (taps + 1) * PHASES + 1];
    for (i, slot) in table.iter_mut().enumerate() {
        let d = i as f64 / PHASES as f64 - HALF_TAPS as f64;
        let x = d * cutoff;
        let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
        let t = (d / HALF_TAPS as f64 + 1.0) * 0.5; // 0..1 across the window
        let window = if (0.0..=1.0).contains(&t) {
            0.42 - 0.5 * (2.0 * std::f64::consts::PI * t).cos() + 0.08 * (4.0 * std::f64::consts::PI * t).cos()
        } else {
            0.0
        };
        *slot = (cutoff * sinc * window) as f32;
    }
    let out_len = (input.len() as u64 * to as u64 / from as u64) as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        // exact source position as a rational, split into sample and phase
        let num = n as u64 * from as u64;
        let centre = (num / to as u64) as i64;
        let frac = (num % to as u64) as f64 / to as f64;
        let mut acc = 0.0f32;
        for k in (1 - HALF_TAPS)..=HALF_TAPS {
            let at = centre + k;
            if at < 0 || at as usize >= input.len() {
                continue;
            }
            // filter offset d = k - frac, in [-HALF_TAPS, HALF_TAPS]
            let pos = (k as f64 - frac + HALF_TAPS as f64) * PHASES as f64;
            let i = pos as usize;
            let f = (pos - i as f64) as f32;
            let h = table[i] + (table[(i + 1).min(table.len() - 1)] - table[i]) * f;
            acc += input[at as usize] * h;
        }
        out.push(acc);
    }
    out
}

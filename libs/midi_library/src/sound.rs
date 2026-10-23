//! How a part sounds, measured: each loop of a Score rendered alone and
//! read for what a brief can be checked against (its loudness, brightness,
//! where its energy sits, how fast it attacks and how long it rings), and
//! the mix read for mud, harshness and balance. The arranger shows the
//! model these numbers beside its own sound brief per part, so it can fix
//! a patch that does not sound like the brief.

use makepad_score_live::RenderOpts;
use makepad_audio_dsp::loudness;

/// Band edges in Hz: sub/bass, low-mid (mud), mid, presence (harsh), air.
pub const BANDS: [(&str, f32, f32); 5] = [("low", 20.0, 200.0), ("lowmid", 200.0, 500.0), ("mid", 500.0, 2000.0), ("presence", 2000.0, 5000.0), ("air", 5000.0, 16000.0)];

/// What a stem (or the mix) measures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measure {
    pub lufs: f64,
    /// Spectral centroid, Hz (energy-weighted mean over frames).
    pub centroid: f64,
    /// Energy per band, dB relative to the stem's loudest band.
    pub bands: [f64; 5],
    /// Median attack, ms (10% to 90% of an onset's peak).
    pub attack_ms: f64,
    /// Median level 150 ms after an onset's peak, dB under the peak (0 is
    /// a held sound, -30 a short pluck).
    pub ring_db: f64,
}

impl Measure {
    pub fn line(&self) -> String {
        let bands: Vec<String> = BANDS.iter().zip(self.bands).map(|((n, _, _), v)| format!("{n} {v:.0}")).collect();
        format!("{:.1} LUFS, centroid {:.0} Hz, attack {:.0} ms, ring {:.0} dB at 150 ms, bands dB {}", self.lufs, self.centroid, self.attack_ms, self.ring_db, bands.join(" "))
    }
}

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f32).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// Measure a stereo signal.
pub fn measure(left: &[f32], right: &[f32], rate: f32) -> Measure {
    let lufs = loudness::measure(left, right, rate as f64).lufs;
    let mono: Vec<f32> = left.iter().zip(right).map(|(l, r)| 0.5 * (l + r)).collect();
    // Spectrum: 4096-point frames every 2048 samples, Hann windowed.
    const N: usize = 4096;
    let window: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).collect();
    let mut bands = [0f64; 5];
    let (mut weighted, mut total) = (0f64, 0f64);
    let mut at = 0;
    while at + N <= mono.len() {
        let mut re: Vec<f32> = mono[at..at + N].iter().zip(&window).map(|(s, w)| s * w).collect();
        let mut im = vec![0f32; N];
        fft(&mut re, &mut im);
        for k in 1..N / 2 {
            let hz = k as f32 * rate / N as f32;
            let p = (re[k] * re[k] + im[k] * im[k]) as f64;
            weighted += p * hz as f64;
            total += p;
            if let Some(b) = BANDS.iter().position(|(_, lo, hi)| hz >= *lo && hz < *hi) {
                bands[b] += p;
            }
        }
        at += N / 2;
    }
    let top = bands.iter().cloned().fold(1e-30, f64::max);
    let bands = bands.map(|b| 10.0 * (b.max(1e-30) / top).log10());
    // Onsets on a 5 ms RMS envelope: a rise of 9 dB over 50 ms.
    let hop = (rate * 0.005) as usize;
    let env: Vec<f64> = mono.chunks(hop.max(1)).map(|c| (c.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / c.len() as f64).sqrt()).collect();
    let db = |v: f64| 20.0 * v.max(1e-9).log10();
    let mut attacks = Vec::new();
    let mut rings = Vec::new();
    let mut i = 10;
    while i + 40 < env.len() {
        if db(env[i]) - db(env[i - 10]) > 9.0 && db(env[i]) > -50.0 {
            let start = i - 10;
            let peak_at = (i..i + 20).max_by(|a, b| env[*a].total_cmp(&env[*b])).unwrap_or(i);
            let (floor, peak) = (env[start], env[peak_at]);
            let level = |f: f64| floor + (peak - floor) * f;
            let t10 = (start..=peak_at).find(|k| env[*k] >= level(0.1)).unwrap_or(start);
            let t90 = (start..=peak_at).find(|k| env[*k] >= level(0.9)).unwrap_or(peak_at);
            attacks.push((t90 - t10) as f64 * 5.0);
            if let Some(after) = env.get(peak_at + 30) {
                rings.push(db(*after) - db(peak));
            }
            i = peak_at + 31;
        } else {
            i += 1;
        }
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    Measure { lufs, centroid: if total > 0.0 { weighted / total } else { 0.0 }, bands, attack_ms: median(&mut attacks), ring_db: median(&mut rings) }
}

/// What a mix's measure says is wrong: mud (low-mids over the mids),
/// harshness (presence over the mids), parts out of balance.
pub fn mix_problems(mix: &Measure, stems: &[(String, Measure)]) -> Vec<String> {
    let mut out = Vec::new();
    let (lowmid, mid, presence) = (mix.bands[1], mix.bands[2], mix.bands[3]);
    if lowmid - mid > 3.0 {
        out.push(format!("muddy: the low-mids (200-500 Hz) sit {:.1} dB over the mids; high-pass and scoop the parts that fill them", lowmid - mid));
    }
    if presence - mid > -1.0 {
        out.push(format!("harsh: the presence band (2-5 kHz) is {:.1} dB against the mids; soften the parts that are bright there", presence - mid));
    }
    if let Some((name, loudest)) = stems.iter().filter(|(_, m)| m.lufs.is_finite()).max_by(|a, b| a.1.lufs.total_cmp(&b.1.lufs)) {
        for (other, m) in stems.iter().filter(|(n, m)| n != name && m.lufs.is_finite()) {
            if loudest.lufs - m.lufs > 18.0 {
                out.push(format!("{other} is {:.0} dB under {name}: it will not be heard", loudest.lufs - m.lufs));
            }
        }
    }
    out
}

/// Render `source` for `seconds` from beat `start`, every loop alone and
/// the mix: (loop, measure) per loop that sounds, and the mix's measure.
pub fn stems(source: &str, start: f64, seconds: f64) -> Result<(Vec<(String, Measure)>, Measure), String> {
    let doc = makepad_score_live::model::load(source).map_err(|d| d.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n"))?.doc;
    let opts = RenderOpts { seconds: Some(seconds), start: Some(start), ..RenderOpts::default() };
    let mix = makepad_music_iter::render::render_mix(source, &opts)?;
    let mix_measure = measure(&mix.left, &mix.right, mix.rate);
    let mut out = Vec::new();
    for l in &doc.loops {
        let solo = makepad_music_iter::render::only(source, &doc, std::slice::from_ref(&l.name))?;
        let r = makepad_music_iter::render::render_mix(&solo, &opts)?;
        if r.left.iter().any(|v| v.abs() > 1e-4) {
            out.push((l.name.clone(), measure(&r.left, &r.right, r.rate)));
        }
    }
    Ok((out, mix_measure))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_and_a_noise_burst_read_as_what_they_are() {
        let rate = 48_000.0;
        let tone: Vec<f32> = (0..rate as usize).map(|i| 0.3 * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / rate).sin()).collect();
        let m = measure(&tone, &tone, rate);
        assert!((m.centroid - 300.0).abs() < 60.0, "{m:?}");
        assert_eq!(m.bands[1], 0.0, "{m:?}");
        // Clicks every 250 ms that decay fast: short attack, short ring.
        let mut seed = 1u32;
        let clicks: Vec<f32> = (0..rate as usize * 2).map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let t = (i % 12_000) as f32 / rate;
            ((seed >> 9) as f32 / 8_388_608.0 - 1.0) * (-t * 60.0).exp()
        }).collect();
        let c = measure(&clicks, &clicks, rate);
        assert!(c.attack_ms <= 10.0 && c.ring_db < -20.0 && c.centroid > 3000.0, "{c:?}");
        let mix = Measure { bands: [0.0, 0.0, -6.0, -3.0, -10.0], ..Default::default() };
        let p = mix_problems(&mix, &[]);
        assert!(p.iter().any(|l| l.starts_with("muddy")) && p.iter().any(|l| l.starts_with("harsh")), "{p:?}");
    }
}

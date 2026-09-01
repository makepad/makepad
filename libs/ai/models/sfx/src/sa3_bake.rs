//! Prompt -> playable sampler bank: the audio analysis and bank assembly
//! behind `sa3-bake`. SA3 has no pitch or velocity control — a prompt is the
//! only input — so the baker generates takes, DETECTS what pitch each take
//! actually landed on, corrects it onto the nearest pitch class (<= 6
//! semitones), and lets the sampler cover the octaves from there. Every key
//! of a pitch class descends from the same take, which is what keeps a bank
//! sounding like one instrument.
//!
//! Everything here is std-only DSP on mono/stereo f32 at 44.1 kHz: YIN f0
//! detection, onset/decay trimming, band spectral envelopes for take-to-take
//! consistency scoring, windowed-sinc resampling for the semitone correction,
//! and an SFZ + wav writer whose output plays on libs/soundfont's documented
//! SFZ subset (key, pitch_keycenter, tune, volume, ampeg_*).

use std::f32::consts::PI;
use std::path::Path;

pub const BAKE_SAMPLE_RATE: u32 = 44_100;

// ---------------------------------------------------------------------------
// Small complex FFT (iterative radix-2), enough for envelopes and f0 refine
// ---------------------------------------------------------------------------

fn fft_radix2(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    // bit-reversal permutation
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut m = n >> 1;
        while m >= 1 && j & m != 0 {
            j ^= m;
            m >>= 1;
        }
        j |= m;
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ar, ai) = (re[i + k], im[i + k]);
                let (br, bi) = (re[i + k + len / 2], im[i + k + len / 2]);
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                re[i + k] = ar + tr;
                im[i + k] = ai + ti;
                re[i + k + len / 2] = ar - tr;
                im[i + k + len / 2] = ai - ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos())
        .collect()
}

// ---------------------------------------------------------------------------
// Envelope / onset
// ---------------------------------------------------------------------------

/// 5 ms-hop RMS envelope.
pub fn rms_envelope(mono: &[f32], sr: u32) -> (Vec<f32>, usize) {
    let hop = (sr as usize / 200).max(1); // 5 ms
    let win = hop * 2;
    let mut out = Vec::with_capacity(mono.len() / hop + 1);
    let mut i = 0;
    while i < mono.len() {
        let end = (i + win).min(mono.len());
        let sum: f32 = mono[i..end].iter().map(|v| v * v).sum();
        out.push((sum / (end - i).max(1) as f32).sqrt());
        i += hop;
    }
    (out, hop)
}

/// Onset sample, end-of-sound sample, and the count of distinct attack events
/// (a usable one-shot has exactly one).
pub struct Extent {
    pub onset: usize,
    pub end: usize,
    pub events: usize,
}

pub fn find_extent(mono: &[f32], sr: u32) -> Extent {
    let (env, hop) = rms_envelope(mono, sr);
    let peak = env.iter().cloned().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return Extent { onset: 0, end: mono.len(), events: 0 };
    }
    let floor = peak * 0.01; // -40 dB from peak
    let first = env.iter().position(|v| *v > floor).unwrap_or(0);
    let last = env.iter().rposition(|v| *v > floor).unwrap_or(env.len() - 1);
    // Attack events: envelope rising through 25% of peak after having been
    // below 10% for at least 50 ms.
    let mut events = 0usize;
    let mut quiet_hops = usize::MAX / 2;
    let quiet_needed = (sr as usize / 20) / hop; // 50 ms
    for &v in &env {
        if v < peak * 0.10 {
            quiet_hops += 1;
        } else {
            if v > peak * 0.25 && quiet_hops >= quiet_needed {
                events += 1;
            }
            quiet_hops = 0;
        }
    }
    Extent {
        onset: (first * hop).saturating_sub(hop),
        end: ((last + 2) * hop).min(mono.len()),
        events: events.max(1),
    }
}

// ---------------------------------------------------------------------------
// YIN f0 detection
// ---------------------------------------------------------------------------

pub struct PitchEstimate {
    /// Median f0 over voiced frames, Hz.
    pub f0_hz: f32,
    /// Fractional MIDI note for `f0_hz`.
    pub midi: f32,
    /// 0..1; mean (1 - CMNDF minimum) over voiced frames.
    pub confidence: f32,
    /// Std-dev of per-frame f0 around the median, in cents.
    pub stability_cents: f32,
    /// Fraction of analysis frames that were voiced.
    pub voiced_ratio: f32,
}

pub fn midi_from_hz(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

pub fn hz_from_midi(midi: f32) -> f32 {
    440.0 * ((midi - 69.0) / 12.0).exp2()
}

/// YIN over `mono` (already trimmed to the sounding region). `None` when no
/// stable fundamental exists (noise, texture, percussion).
pub fn detect_pitch(mono: &[f32], sr: u32) -> Option<PitchEstimate> {
    const F_MIN: f32 = 40.0;
    const F_MAX: f32 = 2200.0;
    const THRESHOLD: f32 = 0.18;
    let tau_max = (sr as f32 / F_MIN) as usize;
    let tau_min = (sr as f32 / F_MAX).max(2.0) as usize;
    let win = (tau_max * 2).next_power_of_two().max(2048);
    let hop = win / 4;
    if mono.len() < win + tau_max {
        return None;
    }
    let mut f0s: Vec<f32> = Vec::new();
    let mut confs: Vec<f32> = Vec::new();
    let mut frames = 0usize;
    let mut start = 0usize;
    let mut diff = vec![0.0f32; tau_max + 1];
    while start + win + tau_max <= mono.len() && frames < 64 {
        frames += 1;
        let x = &mono[start..];
        // difference function d(tau) over the window
        for (tau, d) in diff.iter_mut().enumerate() {
            if tau < tau_min {
                *d = 0.0;
                continue;
            }
            let mut acc = 0.0f32;
            let mut i = 0;
            while i < win {
                let delta = x[i] - x[i + tau];
                acc += delta * delta;
                i += 1;
            }
            *d = acc;
        }
        // cumulative-mean-normalised difference
        let mut cmndf = vec![1.0f32; tau_max + 1];
        let mut running = 0.0f32;
        for tau in 1..=tau_max {
            running += diff[tau];
            cmndf[tau] = if running > 0.0 {
                diff[tau] * tau as f32 / running
            } else {
                1.0
            };
        }
        // first dip under threshold; else global min
        let mut tau_pick = 0usize;
        for tau in tau_min..tau_max {
            if cmndf[tau] < THRESHOLD && cmndf[tau] <= cmndf[tau + 1] {
                tau_pick = tau;
                break;
            }
        }
        if tau_pick == 0 {
            let (tau, _) = cmndf[tau_min..tau_max]
                .iter()
                .enumerate()
                .fold((0usize, f32::MAX), |best, (i, v)| {
                    if *v < best.1 { (i, *v) } else { best }
                });
            tau_pick = tau + tau_min;
        }
        let quality = 1.0 - cmndf[tau_pick].min(1.0);
        if quality > 1.0 - THRESHOLD * 2.0 {
            // parabolic refinement around the dip
            let tau_f = if tau_pick > tau_min && tau_pick + 1 < tau_max {
                let (a, b, c) = (cmndf[tau_pick - 1], cmndf[tau_pick], cmndf[tau_pick + 1]);
                let denom = a - 2.0 * b + c;
                let shift = if denom.abs() > 1e-9 { 0.5 * (a - c) / denom } else { 0.0 };
                tau_pick as f32 + shift.clamp(-0.5, 0.5)
            } else {
                tau_pick as f32
            };
            f0s.push(sr as f32 / tau_f);
            confs.push(quality);
        }
        start += hop;
    }
    if f0s.len() < 3 || frames == 0 {
        return None;
    }
    let voiced_ratio = f0s.len() as f32 / frames as f32;
    let mut sorted = f0s.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    // spread in cents around the median, octave-error tolerant: fold each
    // frame estimate to within half an octave of the median first
    let cents: Vec<f32> = f0s
        .iter()
        .map(|f| {
            let mut c = 1200.0 * (f / median).log2();
            while c > 600.0 {
                c -= 1200.0;
            }
            while c < -600.0 {
                c += 1200.0;
            }
            c
        })
        .collect();
    let mean_c = cents.iter().sum::<f32>() / cents.len() as f32;
    let var = cents.iter().map(|c| (c - mean_c) * (c - mean_c)).sum::<f32>() / cents.len() as f32;
    let confidence = confs.iter().sum::<f32>() / confs.len() as f32;
    Some(PitchEstimate {
        f0_hz: median,
        midi: midi_from_hz(median),
        confidence,
        stability_cents: var.sqrt(),
        voiced_ratio,
    })
}

// ---------------------------------------------------------------------------
// Spectral envelope (timbre fingerprint) + similarity
// ---------------------------------------------------------------------------

pub const ENVELOPE_BANDS: usize = 40;

/// Mean log-magnitude spectrum over the sounding region folded into
/// `ENVELOPE_BANDS` log-spaced bands 60 Hz..16 kHz, in dB, mean-removed.
pub fn spectral_envelope(mono: &[f32], sr: u32) -> Vec<f32> {
    const N: usize = 4096;
    let window = hann(N);
    let mut acc = vec![0.0f64; N / 2];
    let mut frames = 0usize;
    let hop = N / 2;
    let mut start = 0usize;
    let mut re = vec![0.0f32; N];
    let mut im = vec![0.0f32; N];
    while start + N <= mono.len() && frames < 128 {
        for i in 0..N {
            re[i] = mono[start + i] * window[i];
            im[i] = 0.0;
        }
        fft_radix2(&mut re, &mut im);
        for (i, a) in acc.iter_mut().enumerate() {
            *a += (re[i] * re[i] + im[i] * im[i]) as f64;
        }
        frames += 1;
        start += hop;
    }
    if frames == 0 {
        return vec![0.0; ENVELOPE_BANDS];
    }
    let f_lo = 60.0f64;
    let f_hi = 16_000.0f64.min(sr as f64 * 0.45);
    let mut bands = vec![0.0f32; ENVELOPE_BANDS];
    for (b, out) in bands.iter_mut().enumerate() {
        let lo = f_lo * (f_hi / f_lo).powf(b as f64 / ENVELOPE_BANDS as f64);
        let hi = f_lo * (f_hi / f_lo).powf((b + 1) as f64 / ENVELOPE_BANDS as f64);
        let bin_lo = ((lo * N as f64 / sr as f64) as usize).max(1);
        let bin_hi = ((hi * N as f64 / sr as f64) as usize).max(bin_lo + 1).min(N / 2);
        let sum: f64 = acc[bin_lo..bin_hi].iter().sum();
        let mean = sum / (bin_hi - bin_lo) as f64 / frames as f64;
        *out = 10.0 * (mean.max(1e-20)).log10() as f32;
    }
    let mean = bands.iter().sum::<f32>() / bands.len() as f32;
    for v in bands.iter_mut() {
        *v -= mean;
    }
    bands
}

/// RMS distance in dB between two mean-removed band envelopes. ~0 identical
/// timbre; > ~8 dB audibly different instruments.
pub fn envelope_distance(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let sum: f32 = a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum();
    (sum / a.len() as f32).sqrt()
}

// ---------------------------------------------------------------------------
// Loop-point search for sustained material
// ---------------------------------------------------------------------------

/// Looks for a loopable sustain region: two f0-period-aligned points in the
/// second half of the sound whose surrounding windows correlate strongly and
/// whose levels match. Returns (start, end) in samples, end exclusive.
pub fn find_loop(mono: &[f32], sr: u32, f0_hz: f32) -> Option<(usize, usize)> {
    let period = (sr as f32 / f0_hz) as usize;
    if period == 0 || mono.len() < sr as usize / 2 {
        return None;
    }
    let min_loop = (sr as usize / 5).max(period * 8); // >= 200 ms
    let win = (period * 4).min(4096).max(256);
    // candidate loop start: 55% into the sound
    let s = mono.len() * 55 / 100;
    if s + min_loop + win >= mono.len() {
        return None;
    }
    let a = &mono[s..s + win];
    let rms_a = (a.iter().map(|v| v * v).sum::<f32>() / win as f32).sqrt();
    if rms_a < 1e-4 {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    // candidate loop ends stepped by whole periods to stay phase-aligned
    let mut e = s + min_loop;
    while e + win < mono.len() {
        let b = &mono[e..e + win];
        let rms_b = (b.iter().map(|v| v * v).sum::<f32>() / win as f32).sqrt();
        if rms_b > 1e-4 {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let corr = dot / (rms_a * rms_b * win as f32);
            let level_match = (rms_a.min(rms_b) / rms_a.max(rms_b)).powi(2);
            let score = corr * level_match;
            if best.map(|(_, s0)| score > s0).unwrap_or(true) {
                best = Some((e, score));
            }
        }
        e += period;
    }
    match best {
        Some((e, score)) if score > 0.80 => Some((s, e)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Resampling (pitch correction) and conditioning
// ---------------------------------------------------------------------------

/// Windowed-sinc resample by `ratio` (>1 = shift up = shorter output).
/// Anti-aliases on downward-in-frequency reads (ratio > 1).
pub fn resample(input: &[f32], ratio: f64) -> Vec<f32> {
    const TAPS: i64 = 32;
    if input.is_empty() || !(0.01..=100.0).contains(&ratio) {
        return input.to_vec();
    }
    let out_len = (input.len() as f64 / ratio) as usize;
    let cutoff = if ratio > 1.0 { 1.0 / ratio } else { 1.0 } * 0.945;
    let mut out = Vec::with_capacity(out_len);
    for j in 0..out_len {
        let center = j as f64 * ratio;
        let i0 = center.floor() as i64;
        let mut acc = 0.0f64;
        let mut norm = 0.0f64;
        for i in (i0 - TAPS / 2 + 1)..=(i0 + TAPS / 2) {
            if i < 0 || i as usize >= input.len() {
                continue;
            }
            let x = (i as f64 - center) * cutoff;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (PI as f64 * x).sin() / (PI as f64 * x)
            };
            // Blackman window over the tap span
            let t = (i as f64 - center) / (TAPS as f64 / 2.0);
            if t.abs() >= 1.0 {
                continue;
            }
            let w = 0.42 + 0.5 * (PI as f64 * t).cos() + 0.08 * (2.0 * PI as f64 * t).cos();
            let tap = sinc * w;
            acc += input[i as usize] as f64 * tap;
            norm += tap;
        }
        // dividing by the tap-sum normalises DC gain per output sample
        out.push(if norm.abs() > 1e-9 { (acc / norm) as f32 } else { 0.0 });
    }
    out
}

/// 5 ms fade-in at the trimmed onset, 30 ms fade-out at the end, in place.
pub fn apply_fades(mono: &mut [f32], sr: u32) {
    let fade_in = (sr as usize / 200).min(mono.len()); // 5 ms
    for i in 0..fade_in {
        mono[i] *= i as f32 / fade_in as f32;
    }
    let fade_out = (sr as usize * 3 / 100).min(mono.len()); // 30 ms
    let n = mono.len();
    for i in 0..fade_out {
        mono[n - 1 - i] *= i as f32 / fade_out as f32;
    }
}

/// Scale to `target_peak`, returning the gain applied.
pub fn normalize_peak(mono: &mut [f32], target_peak: f32) -> f32 {
    let peak = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak <= 1e-9 {
        return 1.0;
    }
    let gain = target_peak / peak;
    for v in mono.iter_mut() {
        *v *= gain;
    }
    gain
}

// ---------------------------------------------------------------------------
// WAV I/O (PCM16 + f32, mono/stereo) — the bank's takes are stored as wavs
// ---------------------------------------------------------------------------

pub struct WavData {
    pub sample_rate: u32,
    /// Planar channels.
    pub channels: Vec<Vec<f32>>,
}

impl WavData {
    pub fn mono(&self) -> Vec<f32> {
        match self.channels.len() {
            0 => Vec::new(),
            1 => self.channels[0].clone(),
            _ => self.channels[0]
                .iter()
                .zip(&self.channels[1])
                .map(|(l, r)| 0.5 * (l + r))
                .collect(),
        }
    }
}

pub fn read_wav(path: &Path) -> Result<WavData, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_wav(&bytes)
}

pub fn parse_wav(bytes: &[u8]) -> Result<WavData, String> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut pos = 12usize;
    let mut format = 0u16;
    let mut channels = 0usize;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_end = (pos + 8 + size).min(bytes.len());
        let body = &bytes[pos + 8..body_end];
        match id {
            b"fmt " if body.len() >= 16 => {
                format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap()) as usize;
                sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos = pos + 8 + size + (size & 1);
    }
    let data = data.ok_or("wav has no data chunk")?;
    if channels == 0 || sample_rate == 0 {
        return Err("wav has no fmt chunk".into());
    }
    let mut out = vec![Vec::new(); channels];
    match (format, bits) {
        (1, 16) => {
            let frames = data.len() / (2 * channels);
            for c in out.iter_mut() {
                c.reserve(frames);
            }
            for f in 0..frames {
                for (c, ch) in out.iter_mut().enumerate() {
                    let o = (f * channels + c) * 2;
                    let v = i16::from_le_bytes(data[o..o + 2].try_into().unwrap());
                    ch.push(v as f32 / 32768.0);
                }
            }
        }
        (1, 24) => {
            let frames = data.len() / (3 * channels);
            for f in 0..frames {
                for (c, ch) in out.iter_mut().enumerate() {
                    let o = (f * channels + c) * 3;
                    let v = i32::from_le_bytes([0, data[o], data[o + 1], data[o + 2]]) >> 8;
                    ch.push(v as f32 / 8_388_608.0);
                }
            }
        }
        (3, 32) => {
            let frames = data.len() / (4 * channels);
            for f in 0..frames {
                for (c, ch) in out.iter_mut().enumerate() {
                    let o = (f * channels + c) * 4;
                    ch.push(f32::from_le_bytes(data[o..o + 4].try_into().unwrap()));
                }
            }
        }
        _ => return Err(format!("unsupported wav format {format}/{bits}-bit")),
    }
    Ok(WavData { sample_rate, channels: out })
}

pub fn write_wav_stereo16(
    path: &Path,
    left: &[f32],
    right: &[f32],
    sr: u32,
) -> Result<(), String> {
    let frames = left.len().min(right.len());
    let data_len = (frames * 4) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&sr.to_le_bytes());
    out.extend_from_slice(&(sr * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for f in 0..frames {
        out.extend_from_slice(&((left[f].clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        out.extend_from_slice(&((right[f].clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write_wav_mono16(path: &Path, mono: &[f32], sr: u32) -> Result<(), String> {
    let data_len = (mono.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sr.to_le_bytes());
    out.extend_from_slice(&(sr * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for v in mono {
        out.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Take analysis record
// ---------------------------------------------------------------------------

pub struct TakeAnalysis {
    pub pitch: Option<PitchEstimate>,
    pub extent: Extent,
    pub envelope: Vec<f32>,
    pub peak: f32,
    pub duration_s: f32,
    pub loop_region: Option<(usize, usize)>,
}

pub fn analyze_take(mono: &[f32], sr: u32) -> TakeAnalysis {
    let extent = find_extent(mono, sr);
    let sounding = &mono[extent.onset..extent.end];
    let pitch = detect_pitch(sounding, sr);
    let envelope = spectral_envelope(sounding, sr);
    let peak = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let loop_region = pitch
        .as_ref()
        .and_then(|p| find_loop(sounding, sr, p.f0_hz))
        .map(|(s, e)| (s + extent.onset, e + extent.onset));
    TakeAnalysis {
        pitch,
        envelope,
        peak,
        duration_s: mono.len() as f32 / sr as f32,
        extent,
        loop_region,
    }
}

// ---------------------------------------------------------------------------
// Bank assembly: detected-pitch anchors -> 12 pitch-class roots -> SFZ
// ---------------------------------------------------------------------------

/// One SFZ region in the bank: a take (possibly octave-shifted at bake time)
/// placed at its DETECTED pitch and spanning the keys up to the midpoints of
/// its neighbouring anchors.
pub struct AnchorRegion {
    /// Which generated take the audio comes from.
    pub take_index: usize,
    /// `pitch_keycenter`: the detected pitch, rounded.
    pub root_key: u8,
    /// Cents correction for the fractional part of the detected pitch.
    pub tune_cents: i16,
    /// Inclusive key span this region covers.
    pub lo: u8,
    pub hi: u8,
    /// 0 for a natural take; +-12/24 for a pre-rendered octave extension.
    pub octave_shift: i32,
}

pub struct TakeForBank {
    pub index: usize,
    pub midi: f32,
    pub envelope: Vec<f32>,
}

/// Multi-sample layout from detected pitches (the strategy the take
/// measurements force: takes collapse onto a few favourite notes per
/// register, so a per-pitch-class layout would shift one take by two octaves
/// right next to a key shifted the other way — audible smear; instead every
/// key plays the take nearest to it in pitch, sampler-shifted at most about
/// half the largest anchor gap, with exact octave copies of the extreme
/// anchors extending the compass one octave each way).
///
/// When several takes land on the same rounded key, the one whose spectral
/// envelope agrees best with its register neighbours wins.
pub fn plan_anchor_layout(takes: &[TakeForBank]) -> Vec<AnchorRegion> {
    // Coherence = mean envelope distance to other takes within 9 semitones
    // (cross-register envelope distances confound pitch with timbre; local
    // comparison keeps the criterion honest). Global mean as fallback.
    let coherence = |t: &TakeForBank| {
        let mut local = Vec::new();
        let mut global = Vec::new();
        for other in takes {
            if other.index == t.index {
                continue;
            }
            let d = envelope_distance(&t.envelope, &other.envelope);
            global.push(d);
            if (other.midi - t.midi).abs() <= 9.0 {
                local.push(d);
            }
        }
        let pool = if local.is_empty() { &global } else { &local };
        if pool.is_empty() {
            0.0
        } else {
            pool.iter().sum::<f32>() / pool.len() as f32
        }
    };

    // one anchor per rounded key: best local coherence wins
    let mut anchors: Vec<(i32, usize, f32)> = Vec::new(); // (key, take, midi)
    for take in takes {
        let key = take.midi.round() as i32;
        if !(21..=108).contains(&key) {
            continue;
        }
        match anchors.iter_mut().find(|(k, _, _)| *k == key) {
            Some(slot) => {
                let incumbent = takes.iter().find(|t| t.index == slot.1).unwrap();
                if coherence(take) < coherence(incumbent) {
                    *slot = (key, take.index, take.midi);
                }
            }
            None => anchors.push((key, take.index, take.midi)),
        }
    }
    if anchors.is_empty() {
        return Vec::new();
    }
    anchors.sort_by_key(|(key, _, _)| *key);

    // octave extensions: one octave below the lowest anchor and above the
    // highest, and octave copies dropped into any interior gap wider than 13
    // semitones so no key ever sits more than ~7 semitones from its root
    // (all exact factor-of-two resamples, pre-rendered by the baker)
    let mut extended: Vec<(i32, usize, f32, i32)> = anchors
        .iter()
        .map(|&(key, take, midi)| (key, take, midi, 0i32))
        .collect();
    let &(lowest, low_take, low_midi) = anchors.first().unwrap();
    let &(highest, high_take, high_midi) = anchors.last().unwrap();
    if lowest - 12 >= 21 {
        extended.insert(0, (lowest - 12, low_take, low_midi - 12.0, -12));
    }
    if highest + 12 <= 108 {
        extended.push((highest + 12, high_take, high_midi + 12.0, 12));
    }
    let mut index = 0;
    while index + 1 < extended.len() {
        let (a_key, a_take, a_midi, a_oct) = extended[index];
        let b_key = extended[index + 1].0;
        if b_key - a_key > 13 && a_key + 12 < b_key {
            extended.insert(index + 1, (a_key + 12, a_take, a_midi + 12.0, a_oct + 12));
        }
        index += 1;
    }

    // spans = midpoints between neighbouring anchors; the compass ends half
    // an anchor-gap (max 7 semitones) beyond the extreme anchors
    let mut out = Vec::new();
    for (i, &(key, take, midi, octave_shift)) in extended.iter().enumerate() {
        let lo = if i == 0 {
            (key - 7).max(21)
        } else {
            (extended[i - 1].0 + key) / 2 + 1 // one past the previous hi
        };
        let hi = if i + 1 == extended.len() {
            (key + 7).min(108)
        } else {
            (key + extended[i + 1].0) / 2
        };
        if lo > hi {
            continue;
        }
        let tune = ((key as f32 - midi as f32) * 100.0).round() as i16;
        out.push(AnchorRegion {
            take_index: take,
            root_key: key as u8,
            tune_cents: tune.clamp(-99, 99),
            lo: lo.clamp(0, 127) as u8,
            hi: hi.clamp(0, 127) as u8,
            octave_shift,
        });
    }
    out
}

/// Write the SFZ mapping: one region per anchor spanning `lo..=hi`, rooted
/// at the anchor's detected pitch with a cents correction, so the sampler's
/// Hermite resampler never shifts more than about half an anchor gap.
pub fn write_sfz(
    path: &Path,
    display_name: &str,
    prompt: &str,
    regions: &[(AnchorRegion, String)],
    release_s: f32,
) -> Result<(), String> {
    use std::fmt::Write as _;
    let mut text = String::new();
    let _ = writeln!(text, "// {display_name}");
    let _ = writeln!(text, "// generated by sa3-bake from prompt: {prompt}");
    let _ = writeln!(text, "// stable-audio-3-small-sfx | Stability Community License");
    let _ = writeln!(text, "<global> ampeg_attack=0.001 ampeg_release={release_s:.3}");
    for (region, file) in regions {
        let _ = writeln!(
            text,
            "<region> sample={file} lokey={} hikey={} pitch_keycenter={} tune={}",
            region.lo, region.hi, region.root_key, region.tune_cents
        );
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, seconds: f32, sr: u32) -> Vec<f32> {
        (0..(seconds * sr as f32) as usize)
            .map(|i| (2.0 * PI * freq * i as f32 / sr as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn yin_finds_a_sine_fundamental() {
        for freq in [55.0f32, 220.0, 440.0, 987.77] {
            let x = sine(freq, 0.5, BAKE_SAMPLE_RATE);
            let p = detect_pitch(&x, BAKE_SAMPLE_RATE).expect("pitched");
            assert!(
                (p.f0_hz - freq).abs() / freq < 0.01,
                "{freq} Hz detected as {}",
                p.f0_hz
            );
            assert!(p.confidence > 0.9);
            assert!(p.stability_cents < 10.0);
        }
    }

    #[test]
    fn yin_rejects_noise() {
        let mut state = 0x12345u64;
        let x: Vec<f32> = (0..44_100)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((state >> 33) as f32 / 2_147_483_648.0) - 0.5
            })
            .collect();
        let p = detect_pitch(&x, BAKE_SAMPLE_RATE);
        assert!(
            p.map(|p| p.confidence < 0.75 || p.stability_cents > 50.0)
                .unwrap_or(true),
            "white noise must not read as a confident stable pitch"
        );
    }

    #[test]
    fn resample_shifts_a_sine_by_the_requested_ratio() {
        let x = sine(440.0, 0.5, BAKE_SAMPLE_RATE);
        let ratio = 2.0f64.powf(3.0 / 12.0); // up 3 semitones
        let y = resample(&x, ratio);
        let p = detect_pitch(&y, BAKE_SAMPLE_RATE).expect("pitched");
        let expected = 440.0 * ratio as f32;
        assert!(
            (p.f0_hz - expected).abs() / expected < 0.01,
            "expected {expected}, got {}",
            p.f0_hz
        );
    }

    #[test]
    fn envelope_distance_separates_timbres() {
        let a = sine(220.0, 0.5, BAKE_SAMPLE_RATE);
        // square-ish: add odd harmonics
        let b: Vec<f32> = (0..22_050)
            .map(|i| {
                let t = i as f32 / BAKE_SAMPLE_RATE as f32;
                let mut v = 0.0;
                for h in [1.0f32, 3.0, 5.0, 7.0, 9.0] {
                    v += (2.0 * PI * 220.0 * h * t).sin() / h;
                }
                v * 0.3
            })
            .collect();
        let ea = spectral_envelope(&a, BAKE_SAMPLE_RATE);
        let eb = spectral_envelope(&b, BAKE_SAMPLE_RATE);
        let same = envelope_distance(&ea, &ea);
        let diff = envelope_distance(&ea, &eb);
        assert!(same < 0.01);
        assert!(diff > 4.0, "distinct timbres must be far apart, got {diff}");
    }

    #[test]
    fn extent_counts_two_plucks_as_two_events() {
        let sr = BAKE_SAMPLE_RATE;
        let mut x = vec![0.0f32; sr as usize * 2];
        for (start, len) in [(1000usize, 8000usize), (sr as usize + 1000, 8000)] {
            for i in 0..len {
                let t = i as f32 / sr as f32;
                x[start + i] += (2.0 * PI * 330.0 * t).sin() * (1.0 - i as f32 / len as f32) * 0.5;
            }
        }
        let e = find_extent(&x, sr);
        assert_eq!(e.events, 2);
    }

    #[test]
    fn anchor_layout_spans_are_gapless_and_shifts_stay_small() {
        // takes clumped the way SA3 actually behaves: favourite notes with
        // duplicates, spread across three registers
        let takes = vec![
            TakeForBank { index: 0, midi: 43.06, envelope: vec![0.0; ENVELOPE_BANDS] },
            TakeForBank { index: 1, midi: 43.01, envelope: vec![0.0; ENVELOPE_BANDS] },
            TakeForBank { index: 2, midi: 55.09, envelope: vec![0.0; ENVELOPE_BANDS] },
            TakeForBank { index: 3, midi: 70.01, envelope: vec![0.0; ENVELOPE_BANDS] },
            TakeForBank { index: 4, midi: 72.02, envelope: vec![0.0; ENVELOPE_BANDS] },
            TakeForBank { index: 5, midi: 84.03, envelope: vec![0.0; ENVELOPE_BANDS] },
        ];
        let regions = plan_anchor_layout(&takes);
        // 5 distinct anchors + one octave extension each way + an interior
        // octave copy inside the 15-semitone 55..70 gap
        assert_eq!(regions.len(), 8);
        // spans tile the compass with no gaps or overlaps
        for pair in regions.windows(2) {
            assert_eq!(pair[0].hi + 1, pair[1].lo);
        }
        // sampler shift from any key to its region root stays within ~half
        // the largest anchor gap (13 semis here -> 7)
        for region in &regions {
            for key in region.lo..=region.hi {
                let shift = (key as i32 - region.root_key as i32).abs();
                assert!(shift <= 7, "key {key} shifted {shift} semis");
            }
            // fractional detection becomes a cents correction
            assert!(region.tune_cents.abs() < 50);
        }
        // extensions reuse the extreme takes exactly one octave out
        assert_eq!(regions.first().unwrap().octave_shift, -12);
        assert_eq!(regions.first().unwrap().root_key, 31);
        assert_eq!(regions.last().unwrap().octave_shift, 12);
        assert_eq!(regions.last().unwrap().root_key, 96);
        // duplicate-key takes (43.06 and 43.01) collapse to one anchor
        assert_eq!(
            regions.iter().filter(|r| r.root_key == 43 && r.octave_shift == 0).count(),
            1
        );
    }

    #[test]
    fn wide_anchor_gaps_get_interior_octave_copies() {
        // two anchors 23 semitones apart (a real alien-music-box run):
        // without a copy the middle keys would shift 11 semitones
        let takes = vec![
            TakeForBank { index: 0, midi: 43.79, envelope: vec![0.0; ENVELOPE_BANDS] },
            TakeForBank { index: 1, midi: 67.15, envelope: vec![0.0; ENVELOPE_BANDS] },
        ];
        let regions = plan_anchor_layout(&takes);
        // 44 and 67 natural, 32/56 interior+low copies, 79/91... extensions
        assert!(
            regions.iter().any(|r| r.root_key == 56 && r.octave_shift == 12),
            "interior octave copy expected at 56"
        );
        for region in &regions {
            for key in region.lo..=region.hi {
                let shift = (key as i32 - region.root_key as i32).abs();
                assert!(shift <= 7, "key {key} shifted {shift} semis from {}", region.root_key);
            }
        }
        for pair in regions.windows(2) {
            assert_eq!(pair[0].hi + 1, pair[1].lo);
        }
    }

    #[test]
    fn wav_roundtrip_mono16() {
        let x = sine(440.0, 0.1, BAKE_SAMPLE_RATE);
        let dir = std::env::temp_dir().join("sa3_bake_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.wav");
        write_wav_mono16(&path, &x, BAKE_SAMPLE_RATE).unwrap();
        let back = read_wav(&path).unwrap();
        assert_eq!(back.sample_rate, BAKE_SAMPLE_RATE);
        assert_eq!(back.channels.len(), 1);
        assert_eq!(back.channels[0].len(), x.len());
        let err: f32 = x
            .iter()
            .zip(&back.channels[0])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1.0 / 16000.0);
        let _ = std::fs::remove_file(&path);
    }
}

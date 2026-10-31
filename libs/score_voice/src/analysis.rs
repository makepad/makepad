//! Vocal analysis: speech or a sung vocal to frames of {f0, harmonic
//! envelope, band noise}, every 5 ms.
//!
//! - f0: a cumulative-mean-normalised difference (the YIN recipe) with
//!   parabolic refinement, then a median over five frames.
//! - Envelope: a Blackman-windowed spectrum; each harmonic's peak level
//!   (parabolic on the log magnitude) is a sample of the envelope, which is
//!   read between harmonics on the mel axis (the envelope that passes
//!   through the harmonic peaks, a "true envelope").
//! - Noise: the power between harmonics (all of it in unvoiced frames), as
//!   the level of each noise band the voicer plays.
//!
//! From the frames: a [`VoiceProfile`] (per-phoneme medians, given phone
//! timings, e.g. Kokoro's), or a [`VocalTrack`] (the frames themselves,
//! coded, for re-pitching, retiming and harmonising a recorded vocal).

use crate::bands::*;
use crate::fft::Fft;
use crate::harmony::harmonize;
use crate::phoneme::{self, Class, Ph, COUNT};
use crate::pitch::{hz_to_midi, midi_to_hz};
use crate::profile::{Builtin, VoiceProfile};
use crate::voicer::{Frame, Voicer, BLOCK};
use std::sync::Arc;

pub const HOP_SECS: f32 = 0.005;
pub const F0_MIN: f32 = 55.0;
pub const F0_MAX: f32 = 1000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalysisFrame {
    /// Centre (s).
    pub t: f32,
    /// Hz, 0 when unvoiced.
    pub f0: f32,
    /// Periodicity 0..1.
    pub voicing: f32,
    pub env: [f32; ENV_POINTS],
    pub noise: [f32; NOISE_BANDS],
    /// Frame RMS (dB).
    pub level: f32,
}

fn yin(x: &[f32], rate: f32, w: usize) -> (f32, f32) {
    let tmin = (rate / F0_MAX) as usize;
    let tmax = ((rate / F0_MIN) as usize).min(x.len().saturating_sub(w + 1));
    if tmax <= tmin + 2 {
        return (0.0, 0.0);
    }
    let mut d = vec![0.0f32; tmax + 1];
    for (tau, dt) in d.iter_mut().enumerate().skip(1) {
        let mut s = 0.0;
        for j in 0..w {
            let e = x[j] - x[j + tau];
            s += e * e;
        }
        *dt = s;
    }
    // Cumulative mean normalisation.
    let mut cm = vec![1.0f32; tmax + 1];
    let mut run = 0.0;
    for tau in 1..=tmax {
        run += d[tau];
        cm[tau] = if run > 0.0 { d[tau] * tau as f32 / run } else { 1.0 };
    }
    let mut best = None;
    let mut tau = tmin.max(2);
    while tau < tmax {
        if cm[tau] < 0.15 {
            while tau + 1 < tmax && cm[tau + 1] < cm[tau] {
                tau += 1;
            }
            best = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = best.unwrap_or_else(|| (tmin.max(2)..tmax).min_by(|a, b| cm[*a].partial_cmp(&cm[*b]).unwrap()).unwrap());
    let (a, b, c) = (cm[tau - 1], cm[tau], cm[tau + 1]);
    let den = a - 2.0 * b + c;
    let shift = if den.abs() > 1e-9 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
    (rate / (tau as f32 + shift), (1.0 - b).clamp(0.0, 1.0))
}

/// Envelope from harmonic peaks (Hz, dB), read on the grid.
fn envelope_from_peaks(peaks: &[(f32, f32)]) -> [f32; ENV_POINTS] {
    let mut env = [FLOOR_DB; ENV_POINTS];
    if peaks.is_empty() {
        return env;
    }
    let pos: Vec<f32> = peaks.iter().map(|p| env_pos(p.0)).collect();
    for (g, e) in env.iter_mut().enumerate() {
        let x = g as f32;
        *e = if x <= pos[0] {
            peaks[0].1
        } else if x >= *pos.last().unwrap() {
            let (f, db) = *peaks.last().unwrap();
            db - 12.0 * (env_hz(g) / f).log2().max(0.0)
        } else {
            let k = pos.partition_point(|p| *p <= x).max(1);
            let u = (x - pos[k - 1]) / (pos[k] - pos[k - 1]).max(1e-6);
            peaks[k - 1].1 + u * (peaks[k].1 - peaks[k - 1].1)
        };
    }
    env
}

/// Frames every [`HOP_SECS`] over `pcm` (mono, any rate).
pub fn analyze(pcm: &[f32], rate: f32) -> Vec<AnalysisFrame> {
    let hop = (HOP_SECS * rate).round().max(1.0) as usize;
    let yin_w = (0.025 * rate) as usize;
    let yin_span = yin_w + (rate / F0_MIN) as usize + 2;
    let n = ((0.042 * rate) as usize).next_power_of_two();
    let fft = Fft::new(n);
    let window: Vec<f32> = (0..n)
        .map(|i| {
            let x = std::f32::consts::TAU * i as f32 / (n - 1) as f32;
            0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
        })
        .collect();
    let wsum: f32 = window.iter().sum();
    let w2sum: f32 = window.iter().map(|w| w * w).sum();
    let bin_hz = rate / n as f32;
    let top = (rate * 0.48).min(ENV_HI_HZ);
    let frames_n = pcm.len() / hop + 1;
    let at = |i: isize| if i >= 0 && (i as usize) < pcm.len() { pcm[i as usize] } else { 0.0 };
    let mut raw: Vec<(f32, f32)> = Vec::with_capacity(frames_n);
    let mut levels = Vec::with_capacity(frames_n);
    for f in 0..frames_n {
        let c = (f * hop) as isize;
        let seg: Vec<f32> = (0..yin_span).map(|j| at(c - yin_w as isize / 2 + j as isize)).collect();
        let rms = (seg[..yin_w].iter().map(|v| v * v).sum::<f32>() / yin_w as f32).sqrt();
        levels.push(lin_to_db(rms));
        raw.push(if rms > 1e-4 { yin(&seg, rate, yin_w) } else { (0.0, 0.0) });
    }
    // Voiced: periodic enough and not far below the loudest frames.
    let loud = levels.iter().cloned().fold(FLOOR_DB, f32::max);
    let voiced: Vec<bool> = raw.iter().zip(&levels).map(|((f0, v), l)| *f0 > 0.0 && *v > 0.65 && *l > loud - 45.0).collect();
    // Median of five over voiced neighbours.
    let f0s: Vec<f32> = (0..frames_n)
        .map(|i| {
            if !voiced[i] {
                return 0.0;
            }
            let mut near: Vec<f32> = (i.saturating_sub(2)..(i + 3).min(frames_n)).filter(|j| voiced[*j]).map(|j| raw[j].0).collect();
            near.sort_by(|a, b| a.partial_cmp(b).unwrap());
            near[near.len() / 2]
        })
        .collect();

    let (mut re, mut im, mut power) = (Vec::new(), Vec::new(), Vec::new());
    let mut out = Vec::with_capacity(frames_n);
    for f in 0..frames_n {
        let c = (f * hop) as isize;
        let frame: Vec<f32> = (0..n).map(|j| at(c - n as isize / 2 + j as isize) * window[j]).collect();
        fft.power(&frame, &mut re, &mut im, &mut power);
        let f0 = f0s[f];
        let db_of = |p: f32| 10.0 * (p.max(1e-20)).log10();
        let mut peaks = Vec::new();
        let mut harmonic_bin = vec![false; power.len()];
        if f0 > 0.0 {
            let mut k = 1;
            while k as f32 * f0 < top {
                let bin = k as f32 * f0 / bin_hz;
                let b = bin.round() as usize;
                let lo = b.saturating_sub(1).max(1);
                let hi = (b + 1).min(power.len() - 2);
                let m = (lo..=hi).max_by(|x, y| power[*x].partial_cmp(&power[*y]).unwrap()).unwrap();
                let (a, bb, cc) = (db_of(power[m - 1]), db_of(power[m]), db_of(power[m + 1]));
                let den = a - 2.0 * bb + cc;
                let shift = if den.abs() > 1e-9 { (0.5 * (a - cc) / den).clamp(-0.5, 0.5) } else { 0.0 };
                let peak_db = bb - 0.25 * (a - cc) * shift;
                // Sine amplitude: 2|X| / sum(w).
                let amp_db = peak_db + 20.0 * (2.0 / wsum).log10();
                peaks.push((k as f32 * f0, amp_db));
                let guard = (0.3 * f0 / bin_hz).max(3.0);
                let (from, to) = ((bin - guard).max(0.0) as usize, ((bin + guard).ceil() as usize).min(power.len() - 1));
                for h in harmonic_bin.iter_mut().take(to + 1).skip(from) {
                    *h = true;
                }
                k += 1;
            }
        }
        let env = envelope_from_peaks(&peaks);
        let mut noise = [FLOOR_DB; NOISE_BANDS];
        for b in 0..NOISE_BANDS {
            let fc = NOISE_CENTERS[b];
            let bw = noise_bandwidth(b);
            if fc > rate * 0.47 {
                continue;
            }
            let (lo, hi) = (((fc - bw * 0.5) / bin_hz).floor().max(1.0) as usize, (((fc + bw * 0.5) / bin_hz).ceil() as usize).min(power.len() - 1));
            let (mut sum, mut count) = (0.0f32, 0usize);
            for k in lo..=hi {
                if !harmonic_bin[k] {
                    sum += power[k];
                    count += 1;
                }
            }
            if count > 0 {
                // The white-noise variance with this spectral density, then
                // the RMS of a unit-peak band-pass of this band over it.
                let var = sum / count as f32 / w2sum;
                noise[b] = lin_to_db((var * std::f32::consts::PI * bw / rate).sqrt());
            } else if b > 0 {
                noise[b] = noise[b - 1];
            }
        }
        out.push(AnalysisFrame { t: f as f32 * hop as f32 / rate, f0, voicing: raw[f].1, env, noise, level: levels[f] });
    }
    out
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return FLOOR_DB;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// The local maximum of `env` within `[lo, hi]` Hz (parabolic) that is
/// strongest once its distance from `expect` is charged (30 dB an octave).
fn peak_between(env: &[f32; ENV_POINTS], lo: f32, hi: f32, expect: f32) -> Option<f32> {
    let (a, b) = (env_pos(lo).ceil() as usize, (env_pos(hi).floor() as usize).min(ENV_POINTS - 2));
    let mut best: Option<(f32, f32)> = None;
    for g in a.max(1)..=b {
        if env[g] > env[g - 1] && env[g] >= env[g + 1] {
            let den = env[g - 1] - 2.0 * env[g] + env[g + 1];
            let s = if den.abs() > 1e-9 { (0.5 * (env[g - 1] - env[g + 1]) / den).clamp(-0.5, 0.5) } else { 0.0 };
            let f = mel_to_hz(mel(ENV_LO_HZ) + (mel(ENV_HI_HZ) - mel(ENV_LO_HZ)) * (g as f32 + s) / (ENV_POINTS - 1) as f32);
            let score = env[g] - 30.0 * (f / expect).log2().abs();
            if best.is_none_or(|(_, l)| score > l) {
                best = Some((f, score));
            }
        }
    }
    best.map(|b| b.0)
}

/// Formants of an envelope, searched near an expected set.
pub fn find_formants(env: &[f32; ENV_POINTS], expect: [f32; 4]) -> [f32; 4] {
    let mut out = expect;
    let mut floor = 0.0;
    for i in 0..4 {
        let lo = (expect[i] * 0.72).max(floor + 80.0);
        let hi = expect[i] * 1.35;
        if let Some(f) = peak_between(env, lo, hi, expect[i]) {
            out[i] = f;
        }
        out[i] = out[i].max(floor + 80.0);
        floor = out[i];
    }
    out
}

/// A phoneme sounding from `start` to `end` seconds (IPA, as Kokoro's
/// phone timings write it).
#[derive(Clone, Debug, PartialEq)]
pub struct PhoneSpan {
    pub phone: String,
    pub start: f64,
    pub end: f64,
}

/// How far (s) the phone timings sit from the audio: the shift that puts
/// the most vowels on voiced frames and fricatives on hissing ones. Timings
/// from a TTS can run a constant step behind its audio.
pub fn timing_offset(frames: &[AnalysisFrame], phones: &[PhoneSpan]) -> f64 {
    // Vowels should sit on loud voiced frames, voiceless fricatives on hiss.
    let score = |shift: f64| {
        let mut sum = 0.0f64;
        for span in phones {
            let t = (span.start + span.end) * 0.5 + shift;
            let Some(f) = frames.get((t / HOP_SECS as f64).round().max(0.0) as usize) else { continue };
            let parsed = phoneme::parse(&span.phone);
            let Some(ph) = parsed.first() else { continue };
            let info = phoneme::info(*ph);
            sum += match info.class {
                Class::Vowel => (if f.f0 > 0.0 { f.level } else { f.level - 30.0 }).max(-100.0) as f64,
                Class::Fricative if !info.voiced => ((f.noise[10] + f.noise[11]) * 0.5 - if f.f0 > 0.0 { 20.0 } else { 0.0 }).max(-100.0) as f64,
                _ => 0.0,
            };
        }
        sum
    };
    // Steps of 5 ms from -300 to +100 ms; ties go to the smallest shift.
    let mut best = (0.0, score(0.0));
    for k in -60..=20 {
        let shift = k as f64 * HOP_SECS as f64;
        let s = score(shift);
        if s > best.1 + 1e-6 || (s - best.1).abs() <= 1e-6 && shift.abs() < f64::abs(best.0) {
            best = (shift, s);
        }
    }
    best.0
}

/// A voice profile from speech and its phone timings (their constant
/// offset from the audio is found and removed first): each phoneme's shape
/// is the median of its central frames; phonemes the speech lacks come from
/// the nearest built-in voice, resized to this speaker's vocal tract and
/// recoloured by the speaker's average difference from it.
pub fn profile_from_speech(pcm: &[f32], rate: f32, phones: &[PhoneSpan], name: &str) -> VoiceProfile {
    let frames = analyze(pcm, rate);
    let f0_ref = median(frames.iter().filter(|f| f.f0 > 0.0).map(|f| f.f0).collect());
    let f0_ref = if f0_ref > 0.0 { f0_ref } else { 150.0 };
    let base = VoiceProfile::builtin(if f0_ref > 165.0 { Builtin::Alto } else { Builtin::Baritone });

    let offset = timing_offset(&frames, phones);
    // Collect each phoneme's frames.
    let mut per: Vec<Vec<usize>> = vec![Vec::new(); COUNT];
    for span in phones {
        let span = &PhoneSpan { phone: span.phone.clone(), start: span.start + offset, end: span.end + offset };
        let parsed = phoneme::parse(&span.phone);
        if parsed.is_empty() {
            continue;
        }
        // A diphthong's timing: the first element takes the first 60%.
        let len = span.end - span.start;
        for (k, ph) in parsed.iter().enumerate() {
            let (s, e) = match (parsed.len(), k) {
                (1, _) => (span.start, span.end),
                (_, 0) => (span.start, span.start + len * 0.6),
                _ => (span.start + len * 0.6, span.end),
            };
            let (s, e) = (s + (e - s) * 0.25, e - (e - s) * 0.25);
            let mut got = false;
            for (i, f) in frames.iter().enumerate() {
                if (f.t as f64) >= s && (f.t as f64) <= e {
                    per[*ph as usize].push(i);
                    got = true;
                }
            }
            if !got {
                let mid = (s + e) * 0.5;
                if let Some(i) = frames.iter().position(|f| f.t as f64 >= mid) {
                    per[*ph as usize].push(i);
                }
            }
        }
    }

    let mut shapes = base.shapes.clone();
    let mut measured = vec![false; COUNT];
    for ph in 0..COUNT {
        let info = phoneme::info(ph as Ph);
        let idx = &per[ph];
        if idx.is_empty() || info.class == Class::Silence {
            continue;
        }
        let sung = matches!(info.class, Class::Vowel | Class::Nasal | Class::Liquid | Class::Glide);
        let voiced_idx: Vec<usize> = idx.iter().cloned().filter(|i| frames[*i].f0 > 0.0).collect();
        if sung && voiced_idx.len() < 2 {
            continue;
        }
        let shape = &mut shapes[ph];
        if sung || info.voiced && !voiced_idx.is_empty() {
            for g in 0..ENV_POINTS {
                shape.env[g] = median(voiced_idx.iter().map(|i| frames[*i].env[g]).collect());
            }
        }
        for b in 0..NOISE_BANDS {
            shape.noise[b] = median(idx.iter().map(|i| frames[*i].noise[b]).collect());
        }
        if matches!(info.class, Class::Stop | Class::Affricate) {
            // The burst: the frame with the most high-band noise.
            let hi = |i: &usize| frames[*i].noise[8..].iter().map(|v| db_to_lin(*v)).sum::<f32>();
            if let Some(i) = idx.iter().max_by(|a, b| hi(a).partial_cmp(&hi(b)).unwrap()) {
                shape.burst = frames[*i].noise;
            }
        }
        if sung {
            shape.formants = find_formants(&shape.env, base.shapes[ph].formants);
        }
        measured[ph] = sung || !idx.is_empty();
    }

    // The speaker against the built-in: vocal tract size (formant ratio over
    // measured vowels) and colour (median level difference per point).
    let vowels: Vec<usize> = (0..COUNT).filter(|p| measured[*p] && phoneme::is_vowel(*p as Ph)).collect();
    let mut ratios = Vec::new();
    for p in &vowels {
        for i in 1..3 {
            ratios.push(shapes[*p].formants[i] / base.shapes[*p].formants[i]);
        }
    }
    let tract = if ratios.is_empty() { 1.0 } else { median(ratios).clamp(0.8, 1.25) };
    let mut colour = [0.0f32; ENV_POINTS];
    for (g, c) in colour.iter_mut().enumerate() {
        let f = env_hz(g);
        *c = median(vowels.iter().map(|p| shapes[*p].env[g] - env_at(&base.shapes[*p].env, f / tract)).collect());
        if !c.is_finite() || vowels.is_empty() {
            *c = 0.0;
        }
    }
    for ph in 0..COUNT {
        let info = phoneme::info(ph as Ph);
        let sung = matches!(info.class, Class::Vowel | Class::Nasal | Class::Liquid | Class::Glide);
        let src = &base.shapes[ph];
        if !measured[ph] || !sung {
            // Not measured, or a consonant whose harmonic part is a locus:
            // the built-in shape, resized and recoloured.
            let keep_noise = measured[ph];
            let noise = shapes[ph].noise;
            let burst = shapes[ph].burst;
            let shape = &mut shapes[ph];
            for g in 0..ENV_POINTS {
                shape.env[g] = env_at(&src.env, env_hz(g) / tract) + colour[g];
            }
            shape.formants = src.formants.map(|f| f * tract);
            if !keep_noise {
                shape.noise = src.noise;
                shape.burst = src.burst;
                if phoneme::is_vowel(ph as Ph) {
                    shape.noise = crate::profile::aperiodic_noise(&shape.env, 0.0);
                }
            } else {
                shape.noise = noise;
                shape.burst = burst;
            }
        }
    }
    // Sung sibilants are gentler than spoken ones (a singer de-esses): no
    // noise band of a consonant within 16 dB of the loudest vowel harmonic.
    let ah = phoneme::by_ipa("ɑ").unwrap() as usize;
    let cap = shapes[ah].env.iter().cloned().fold(FLOOR_DB, f32::max) - 16.0;
    for (ph, shape) in shapes.iter_mut().enumerate() {
        if !phoneme::is_vowel(ph as Ph) {
            for v in shape.noise.iter_mut().chain(shape.burst.iter_mut()) {
                *v = v.min(cap);
            }
        }
    }
    VoiceProfile { name: name.to_string(), shapes, f0_ref, range: [hz_to_midi(f0_ref) - 7.0, hz_to_midi(f0_ref) + 14.0], breath: 0.1 }
}

// ---------------------------------------------------------------------------
// Vocal tracks: a recorded vocal as data.

/// One 5 ms frame of a vocal track: the sung pitch (MIDI, 0 = unvoiced),
/// the pitch it was analysed at, the envelope and noise as half-dB codes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackFrame {
    pub note: f32,
    pub src_note: f32,
    pub env: [u8; ENV_POINTS],
    pub noise: [u8; NOISE_BANDS],
}

#[derive(Clone, Debug, PartialEq)]
pub struct VocalTrack {
    pub hop: f32,
    pub frames: Vec<TrackFrame>,
}

impl VocalTrack {
    /// Analyse a vocal (a stem, a take).
    pub fn analyze(pcm: &[f32], rate: f32) -> VocalTrack {
        let frames = analyze(pcm, rate)
            .into_iter()
            .map(|f| {
                let note = if f.f0 > 0.0 { hz_to_midi(f.f0) } else { 0.0 };
                TrackFrame { note, src_note: note, env: f.env.map(db_to_code), noise: f.noise.map(db_to_code) }
            })
            .collect();
        VocalTrack { hop: HOP_SECS, frames }
    }

    pub fn duration(&self) -> f32 {
        self.frames.len() as f32 * self.hop
    }

    const MAGIC: &'static [u8; 8] = b"MKVTRK\x01\0";

    /// About 16 KB a second.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.frames.len() * (4 + ENV_POINTS + NOISE_BANDS));
        out.extend_from_slice(Self::MAGIC);
        out.extend_from_slice(&self.hop.to_le_bytes());
        out.extend_from_slice(&(self.frames.len() as u32).to_le_bytes());
        for f in &self.frames {
            out.extend_from_slice(&((f.note * 100.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes());
            out.extend_from_slice(&((f.src_note * 100.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes());
            out.extend_from_slice(&f.env);
            out.extend_from_slice(&f.noise);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<VocalTrack, String> {
        if bytes.len() < 16 || &bytes[..8] != Self::MAGIC {
            return Err("vocal track: not an MKVTRK v1 file".into());
        }
        let hop = f32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let size = 4 + ENV_POINTS + NOISE_BANDS;
        if bytes.len() < 16 + n * size {
            return Err("vocal track: truncated".into());
        }
        let frames = (0..n)
            .map(|i| {
                let b = &bytes[16 + i * size..16 + (i + 1) * size];
                TrackFrame {
                    note: u16::from_le_bytes([b[0], b[1]]) as f32 / 100.0,
                    src_note: u16::from_le_bytes([b[2], b[3]]) as f32 / 100.0,
                    env: b[4..4 + ENV_POINTS].try_into().unwrap(),
                    noise: b[4 + ENV_POINTS..].try_into().unwrap(),
                }
            })
            .collect();
        Ok(VocalTrack { hop, frames })
    }

    /// Every voiced frame up or down by `semitones`.
    pub fn transpose(&mut self, semitones: f32) {
        for f in self.frames.iter_mut().filter(|f| f.note > 0.0) {
            f.note += semitones;
        }
    }

    /// Pull the pitch toward the nearest note of `scale` (pitch classes,
    /// 0 = C): `strength` 1 snaps, less keeps some of the inflection.
    pub fn snap_to_scale(&mut self, scale: &[u8], strength: f32) {
        if scale.is_empty() {
            return;
        }
        for f in self.frames.iter_mut().filter(|f| f.note > 0.0) {
            let near = (f.note.round() as i32 - 6..=f.note.round() as i32 + 6)
                .filter(|n| scale.contains(&(n.rem_euclid(12) as u8)))
                .min_by(|a, b| (*a as f32 - f.note).abs().partial_cmp(&(*b as f32 - f.note).abs()).unwrap())
                .unwrap_or(f.note.round() as i32);
            f.note += (near as f32 - f.note) * strength.clamp(0.0, 1.0);
        }
    }

    /// The same vocal at `factor` times the length (frames resampled; the
    /// pitch is untouched).
    pub fn retime(&self, factor: f32) -> VocalTrack {
        let n = ((self.frames.len() as f32) * factor.max(0.05)).round() as usize;
        let frames = (0..n)
            .map(|i| {
                let src = (i as f32 / factor.max(0.05)) as usize;
                self.frames[src.min(self.frames.len().saturating_sub(1))]
            })
            .collect();
        VocalTrack { hop: self.hop, frames }
    }

    /// A harmony part of this vocal: each voiced frame moved to the chord
    /// tone `part` places below the melody (chords as (seconds, pitches)),
    /// its inflections kept.
    pub fn harmony(&self, chords: &[(f32, Vec<f32>)], part: usize) -> VocalTrack {
        let mut out = self.clone();
        for (i, f) in out.frames.iter_mut().enumerate() {
            if f.note <= 0.0 {
                continue;
            }
            let t = i as f32 * self.hop;
            let Some((_, chord)) = chords.iter().rev().find(|(at, _)| *at <= t).or(chords.first()) else { continue };
            let melody = f.note.round();
            let parts = harmonize(melody, chord, part + 1, &[]);
            f.note += parts[part] - melody;
        }
        out
    }
}

/// Plays a [`VocalTrack`] through a voicer: realtime-safe once built (the
/// track is shared), at any speed, transposition and formant scale.
pub struct TrackPlayer {
    track: Arc<VocalTrack>,
    voicer: Voicer,
    rate: f32,
    /// Read position (frames of the track).
    pos: f64,
    pub speed: f64,
    pub transpose: f32,
    pub formant_scale: f32,
    pub gain: f32,
    f0: f32,
    buf: [f32; BLOCK],
    buf_at: usize,
}

impl TrackPlayer {
    pub fn new(track: Arc<VocalTrack>, rate: f32, seed: u32) -> TrackPlayer {
        TrackPlayer {
            track, voicer: Voicer::new(rate, seed), rate, pos: 0.0, speed: 1.0, transpose: 0.0, formant_scale: 1.0, gain: 1.0, f0: 200.0,
            buf: [0.0; BLOCK], buf_at: BLOCK,
        }
    }

    pub fn finished(&self) -> bool {
        self.pos >= self.track.frames.len() as f64 && self.voicer.is_silent()
    }

    fn block(&mut self) {
        let frames = &self.track.frames;
        let mut frame = Frame::SILENT;
        let mut src_f0 = self.f0;
        if (self.pos as usize) + 1 < frames.len() {
            let i = self.pos as usize;
            let u = (self.pos - i as f64) as f32;
            let (a, b) = (&frames[i], &frames[i + 1]);
            for g in 0..ENV_POINTS {
                frame.env[g] = code_to_db(a.env[g]) + u * (code_to_db(b.env[g]) - code_to_db(a.env[g]));
            }
            let voiced = a.note > 0.0 && b.note > 0.0 || a.note > 0.0 && u < 0.5 || b.note > 0.0 && u >= 0.5;
            let noise: [f32; NOISE_BANDS] =
                std::array::from_fn(|k| db_to_lin(code_to_db(a.noise[k])) * (1.0 - u) + db_to_lin(code_to_db(b.noise[k])) * u);
            if voiced {
                let (na, nb) = (if a.note > 0.0 { a.note } else { b.note }, if b.note > 0.0 { b.note } else { a.note });
                let (sa, sb) = (if a.src_note > 0.0 { a.src_note } else { b.src_note }, if b.src_note > 0.0 { b.src_note } else { a.src_note });
                self.f0 = midi_to_hz(na + u * (nb - na) + self.transpose);
                src_f0 = midi_to_hz(sa + u * (sb - sa));
                frame.harm = 1.0;
                frame.pulsed = noise;
            } else {
                frame.noise = noise;
            }
        }
        self.voicer.render_block(&mut self.buf, self.f0, &frame, self.formant_scale, self.gain, src_f0);
        self.pos += self.speed * BLOCK as f64 / (self.rate as f64 * self.track.hop as f64);
    }

    /// ADD into `out`.
    pub fn render(&mut self, out: &mut [f32]) {
        let mut i = 0;
        while i < out.len() {
            if self.buf_at == BLOCK {
                self.block();
                self.buf_at = 0;
            }
            let n = (BLOCK - self.buf_at).min(out.len() - i);
            for (o, s) in out[i..i + n].iter_mut().zip(&self.buf[self.buf_at..self.buf_at + n]) {
                *o += *s;
            }
            self.buf_at += n;
            i += n;
        }
    }
}

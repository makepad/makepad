//! Offline music loop scanner: finds the N most seamlessly loopable,
//! beat-aligned sections of a track.
//!
//! Pure analysis — PCM + beat times in, scored spans out — so everything is
//! unit-testable without audio hardware.  The seam score follows LoopMaker
//! (Shi & Mysore, CHI 2018): the wrap `i+L -> i` sounds seamless iff the
//! beats after the loop resemble the beats at its head, because the track
//! itself proves the transition into `i+L` sounds natural.  Gates and their
//! adaptive relaxation follow PyMusicLooper; the relax-until-N idea is the
//! Infinite Jukebox's.  This is AUDIO; the video analyzer lives in
//! `loop_detect.rs` and shares nothing with this module.

use crate::mixer::TrackPcm;
use makepad_ai_stems::stft::Stft;
use makepad_widgets::makepad_platform::thread::ThreadSpawner;

/// STFT geometry for the feature pass.  2048/512 at the track's native rate
/// gives ~86 Hz feature frames — a dozen per beat at any sane tempo.
const N_FFT: usize = 2048;
const HOP: usize = 512;
/// Mel filterbank size feeding the MFCCs.  64 keeps every triangle wider
/// than a bin at this FFT size, so no filter comes back empty.
const N_MELS: usize = 64;
const MEL_FMAX: f32 = 8000.0;
/// Chroma folds bins in this range onto the 12 pitch classes.
const CHROMA_FMIN: f32 = 55.0;
const CHROMA_FMAX: f32 = 8000.0;

/// Mono stem lanes for scoring, at their own rate (the stem cache's model
/// rate, not the track's).
pub struct ScanStems {
    pub drums: Vec<f32>,
    pub vocals: Vec<f32>,
    pub sample_rate: u32,
}

/// Everything the scorer reads, one entry per beat interval.
pub(crate) struct BeatFeatures {
    /// Log-chroma, mean-removed and unit-L2 per beat (zeros when silent).
    pub chroma: Vec<[f32; 12]>,
    /// MFCC coefficients 1..=13, median over the beat's frames.
    pub mfcc: Vec<[f32; 13]>,
    pub rms_db: Vec<f32>,
    pub drums_db: Option<Vec<f32>>,
    pub vocals_db: Option<Vec<f32>>,
    /// Median adjacent-beat MFCC distance — the track's own unit of timbre
    /// change, so the score is level- and mastering-independent.
    pub mfcc_norm: f32,
}

fn mono_of_pcm(pcm: &TrackPcm) -> Vec<f32> {
    pcm.frames
        .iter()
        .map(|f| (f[0] as f32 + f[1] as f32) * 0.5 / 32768.0)
        .collect()
}

fn rms_db_of(samples: &[f32], rate: f64, start_secs: f64, end_secs: f64) -> f32 {
    let a = ((start_secs * rate) as usize).min(samples.len());
    let b = ((end_secs * rate) as usize).clamp(a, samples.len());
    if b <= a {
        return -120.0;
    }
    let sum: f32 = samples[a..b].iter().map(|s| s * s).sum();
    20.0 * ((sum / (b - a) as f32).sqrt() + 1e-6).log10()
}

/// Triangular mel filterbank as (first_bin, weights) per filter.
fn mel_filterbank(rate: f32, bins: usize) -> Vec<(usize, Vec<f32>)> {
    let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let inv_mel = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    let lo = mel(0.0);
    let hi = mel(MEL_FMAX.min(rate * 0.5));
    let points: Vec<f32> = (0..N_MELS + 2)
        .map(|i| inv_mel(lo + (hi - lo) * i as f32 / (N_MELS + 1) as f32))
        .collect();
    let bin_hz = rate / N_FFT as f32;
    let mut filters = Vec::with_capacity(N_MELS);
    for m in 0..N_MELS {
        let (a, c, b) = (points[m], points[m + 1], points[m + 2]);
        let first = (a / bin_hz).ceil().max(0.0) as usize;
        let last = ((b / bin_hz).floor() as usize).min(bins - 1);
        let mut weights = Vec::new();
        for k in first..=last.max(first) {
            let f = k as f32 * bin_hz;
            let w = if f <= c {
                (f - a) / (c - a).max(1e-6)
            } else {
                (b - f) / (b - c).max(1e-6)
            };
            weights.push(w.max(0.0));
        }
        filters.push((first, weights));
    }
    filters
}

/// Pitch class per bin, `None` outside the chroma range.
fn chroma_classes(rate: f32, bins: usize) -> Vec<Option<usize>> {
    let bin_hz = rate / N_FFT as f32;
    (0..bins)
        .map(|k| {
            let f = k as f32 * bin_hz;
            if f < CHROMA_FMIN || f > CHROMA_FMAX {
                return None;
            }
            let midi = 69.0 + 12.0 * (f / 440.0).log2();
            Some((midi.round() as i64).rem_euclid(12) as usize)
        })
        .collect()
}

fn median(scratch: &mut Vec<f32>) -> f32 {
    if scratch.is_empty() {
        return 0.0;
    }
    scratch.sort_by(|a, b| a.total_cmp(b));
    scratch[scratch.len() / 2]
}

pub(crate) fn cos_dist(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na < 1e-6 || nb < 1e-6 {
        return 1.0;
    }
    (1.0 - dot / (na * nb)).clamp(0.0, 2.0)
}

pub(crate) fn euclid13(a: &[f32; 13], b: &[f32; 13]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

pub(crate) fn compute_features(
    pcm: &TrackPcm,
    stems: Option<&ScanStems>,
    beats: &[f64],
    duration_secs: f64,
) -> BeatFeatures {
    let rate = pcm.sample_rate.max(1) as f32;
    let mono = mono_of_pcm(pcm);
    let stft = Stft::new(N_FFT, HOP, N_FFT);
    let (spec, frames) = stft.forward(&mono);
    let bins = N_FFT / 2 + 1;

    // Per-frame power -> mel-log -> MFCC, and chroma power.
    let filters = mel_filterbank(rate, bins);
    let classes = chroma_classes(rate, bins);
    let mut frame_mfcc = vec![[0.0f32; 13]; frames];
    let mut frame_chroma = vec![[0.0f32; 12]; frames];
    let mut power = vec![0.0f32; bins];
    for frame in 0..frames {
        for (k, p) in power.iter_mut().enumerate() {
            let at = (k * frames + frame) * 2;
            let (re, im) = (spec[at], spec[at + 1]);
            *p = re * re + im * im;
        }
        let mut log_mel = [0.0f32; N_MELS];
        for (m, (first, weights)) in filters.iter().enumerate() {
            let mut sum = 0.0;
            for (j, w) in weights.iter().enumerate() {
                sum += w * power.get(first + j).copied().unwrap_or(0.0);
            }
            log_mel[m] = (sum + 1e-10).ln();
        }
        // DCT-II, coefficients 1..=13.
        for c in 1..=13 {
            let mut acc = 0.0;
            for (m, v) in log_mel.iter().enumerate() {
                acc += v
                    * ((std::f32::consts::PI / N_MELS as f32)
                        * (m as f32 + 0.5)
                        * c as f32)
                        .cos();
            }
            frame_mfcc[frame][c - 1] = acc;
        }
        for (k, class) in classes.iter().enumerate() {
            if let Some(pc) = class {
                frame_chroma[frame][*pc] += power[k];
            }
        }
    }

    // Beat-synchronize by median over each beat's frames.
    let n = beats.len();
    let mut chroma = Vec::with_capacity(n);
    let mut mfcc = Vec::with_capacity(n);
    let mut rms_db = Vec::with_capacity(n);
    let mut scratch = Vec::new();
    for i in 0..n {
        let start = beats[i];
        let end = if i + 1 < n { beats[i + 1] } else { duration_secs.max(start) };
        let mut first = ((start * rate as f64 / HOP as f64).ceil() as usize).min(frames);
        let mut last = ((end * rate as f64 / HOP as f64).ceil() as usize).min(frames);
        if first >= last {
            // A beat narrower than a hop: borrow the nearest frame.
            first = ((start * rate as f64 / HOP as f64) as usize).min(frames.saturating_sub(1));
            last = (first + 1).min(frames);
        }
        let mut c = [0.0f32; 12];
        for (pc, out) in c.iter_mut().enumerate() {
            scratch.clear();
            scratch.extend((first..last).map(|f| frame_chroma[f][pc]));
            *out = median(&mut scratch);
        }
        // Log-compress, remove the mean, unit-L2: cosine distance then
        // compares harmonic SHAPE, not level.
        let mut v = c.map(|x| (x + 1e-9).ln());
        let mean = v.iter().sum::<f32>() / 12.0;
        v.iter_mut().for_each(|x| *x -= mean);
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-6 {
            v.iter_mut().for_each(|x| *x /= norm);
        } else {
            v = [0.0; 12];
        }
        chroma.push(v);
        let mut m = [0.0f32; 13];
        for (d, out) in m.iter_mut().enumerate() {
            scratch.clear();
            scratch.extend((first..last).map(|f| frame_mfcc[f][d]));
            *out = median(&mut scratch);
        }
        mfcc.push(m);
        rms_db.push(rms_db_of(&mono, rate as f64, start, end));
    }

    let stem_lane = |lane: &[f32], lane_rate: u32| -> Vec<f32> {
        (0..n)
            .map(|i| {
                let start = beats[i];
                let end = if i + 1 < n { beats[i + 1] } else { duration_secs.max(start) };
                rms_db_of(lane, lane_rate.max(1) as f64, start, end)
            })
            .collect()
    };
    let (drums_db, vocals_db) = match stems {
        Some(s) => (
            Some(stem_lane(&s.drums, s.sample_rate)),
            Some(stem_lane(&s.vocals, s.sample_rate)),
        ),
        None => (None, None),
    };

    scratch.clear();
    scratch.extend((1..n).map(|i| euclid13(&mfcc[i - 1], &mfcc[i])));
    let mfcc_norm = median(&mut scratch).max(1e-3);

    BeatFeatures { chroma, mfcc, rms_db, drums_db, vocals_db, mfcc_norm }
}

use crate::decks::LoopSpan;
use crate::wave_analysis::TrackAnalysis;

/// Beats of context compared across the seam, with geometric decay.
const CONTEXT_BEATS: usize = 8;
const CHROMA_W: f32 = 1.0;
const MFCC_W: f32 = 0.6;
const RMS_W: f32 = 0.2;
const DRUMS_W: f32 = 0.2;
/// Highest stem weight: a vocal present on one side of the seam and absent
/// on the other is the most audible looping artifact there is.
const VOCALS_W: f32 = 0.4;
/// A 6 dB step counts as one full unit of seam damage.
const DB_NORM: f32 = 6.0;
/// Per structural change point strictly inside the loop body.
const STRUCT_PENALTY: f32 = 0.05;
/// PyMusicLooper's gates, doubled until enough candidates survive.
const LOUD_GATE_DB: f32 = 0.5;
const LOUD_GATE_CAP_DB: f32 = 8.0;
const CHROMA_GATE: f32 = 0.25;
/// Kept INs stay at least a bar apart.
const MIN_IN_SPACING_BEATS: i64 = 4;
/// Power-of-two candidate lengths, in beats.
const LENGTHS: [usize; 11] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LengthBounds {
    Secs { min: f64, max: f64 },
    Beats { min: u32, max: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopScanConfig {
    pub bounds: LengthBounds,
    pub count: usize,
}

impl Default for LoopScanConfig {
    fn default() -> Self {
        Self { bounds: LengthBounds::Secs { min: 4.0, max: 10.0 }, count: 10 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoredLoop {
    pub span: LoopSpan,
    pub beats: u32,
    pub score: f32,
}

/// The scan dialog's settings — the whole of what the operator can set, in
/// one value that outlives the dialog. AUTOMATIC scans with no dialog open,
/// so these cannot live in the widgets the way they first did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScanSettings {
    /// false = seconds (`min_secs`/`max_secs`), true = beats (ladder indices).
    pub unit_beats: bool,
    pub min_secs: f64,
    pub max_secs: f64,
    pub min_beats_ix: usize,
    pub max_beats_ix: usize,
    pub count: usize,
    pub automatic: bool,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            unit_beats: false,
            min_secs: 4.0,
            max_secs: 10.0,
            min_beats_ix: 0,
            // The 64-beat rung: a phrase at any tempo this app plays.
            max_beats_ix: 3,
            count: 10,
            automatic: false,
        }
    }
}

impl ScanSettings {
    /// `key value` lines, one per setting — the shape that goes to disk.
    pub fn to_text(&self) -> String {
        format!(
            "unit {}\nmin_secs {}\nmax_secs {}\nmin_beats {}\nmax_beats {}\ncount {}\nautomatic {}\n",
            if self.unit_beats { "beats" } else { "secs" },
            self.min_secs,
            self.max_secs,
            self.min_beats_ix,
            self.max_beats_ix,
            self.count,
            u8::from(self.automatic),
        )
    }

    /// Per-key fallback: a line that will not parse, a number that is not
    /// finite, or one outside its range is treated as absent — so a mangled
    /// file costs the operator one setting rather than all of them.
    pub fn from_text(body: &str) -> ScanSettings {
        let mut out = ScanSettings::default();
        for line in body.lines() {
            let mut parts = line.split_whitespace();
            let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
                continue;
            };
            match key {
                "unit" => out.unit_beats = value == "beats",
                "min_secs" | "max_secs" => {
                    if let Ok(secs) = value.parse::<f64>() {
                        if secs.is_finite() && secs > 0.0 {
                            if key == "min_secs" {
                                out.min_secs = secs;
                            } else {
                                out.max_secs = secs;
                            }
                        }
                    }
                }
                "min_beats" | "max_beats" => {
                    if let Ok(index) = value.parse::<usize>() {
                        if index < LENGTHS.len() {
                            if key == "min_beats" {
                                out.min_beats_ix = index;
                            } else {
                                out.max_beats_ix = index;
                            }
                        }
                    }
                }
                "count" => {
                    if let Ok(count) = value.parse::<usize>() {
                        if count >= 1 && count <= crate::decks::FOUND_LOOP_CAP {
                            out.count = count;
                        }
                    }
                }
                "automatic" => out.automatic = value == "1",
                _ => {}
            }
        }
        out
    }

    /// What the scanner is actually asked for. Bounds given the wrong way
    /// round swap themselves; the count is clamped to the marker row.
    pub fn to_config(&self) -> LoopScanConfig {
        let bounds = if self.unit_beats {
            let min = LENGTHS[self.min_beats_ix.min(LENGTHS.len() - 1)] as u32;
            let max = LENGTHS[self.max_beats_ix.min(LENGTHS.len() - 1)] as u32;
            LengthBounds::Beats { min: min.min(max), max: min.max(max) }
        } else {
            LengthBounds::Secs {
                min: self.min_secs.min(self.max_secs),
                max: self.min_secs.max(self.max_secs),
            }
        };
        LoopScanConfig {
            bounds,
            count: self.count.clamp(1, crate::decks::FOUND_LOOP_CAP),
        }
    }
}

fn context_weights() -> [f32; CONTEXT_BEATS] {
    // 100 -> 1 geometric decay, normalized to sum 1: the beat right at the
    // seam matters most, the tail keeps a jump from landing mid-phrase.
    let ratio = (1.0f32 / 100.0).powf(1.0 / (CONTEXT_BEATS as f32 - 1.0));
    let mut w = [0.0f32; CONTEXT_BEATS];
    let mut value = 1.0;
    let mut sum = 0.0;
    for slot in w.iter_mut() {
        *slot = value;
        sum += value;
        value *= ratio;
    }
    w.iter_mut().for_each(|x| *x /= sum);
    w
}

fn beat_dist(f: &BeatFeatures, a: usize, b: usize) -> f32 {
    let base = CHROMA_W + MFCC_W + RMS_W;
    let mut d = CHROMA_W * cos_dist(&f.chroma[a], &f.chroma[b])
        + MFCC_W * euclid13(&f.mfcc[a], &f.mfcc[b]) / f.mfcc_norm
        + RMS_W * (f.rms_db[a] - f.rms_db[b]).abs() / DB_NORM;
    let mut total = base;
    if let (Some(drums), Some(vocals)) = (&f.drums_db, &f.vocals_db) {
        d += DRUMS_W * (drums[a] - drums[b]).abs() / DB_NORM
            + VOCALS_W * (vocals[a] - vocals[b]).abs() / DB_NORM;
        total += DRUMS_W + VOCALS_W;
    }
    // Renormalized so stem-aware and mix-only scores share one scale.
    d * base / total
}

/// The LoopMaker seam score, lower = more seamless.  `i + l + CONTEXT_BEATS`
/// must fit inside the feature vectors.
pub(crate) fn seam_score(f: &BeatFeatures, i: usize, l: usize) -> f32 {
    let w = context_weights();
    let mut ahead = 0.0;
    for (k, wk) in w.iter().enumerate() {
        ahead += wk * beat_dist(f, i + k, i + l + k);
    }
    if i >= CONTEXT_BEATS {
        let mut behind = 0.0;
        for (k, wk) in w.iter().enumerate() {
            behind += wk * beat_dist(f, i - 1 - k, i + l - 1 - k);
        }
        0.5 * (ahead + behind)
    } else {
        ahead
    }
}

/// The lengths the bounds admit.  In seconds mode an empty admissible set
/// widens to the single nearest length rather than returning nothing.
fn admissible_lengths(bounds: LengthBounds, typical_beat_secs: f64) -> Vec<usize> {
    match bounds {
        LengthBounds::Beats { min, max } => LENGTHS
            .iter()
            .copied()
            .filter(|l| *l as u32 >= min.min(max) && *l as u32 <= min.max(max))
            .collect(),
        LengthBounds::Secs { min, max } => {
            let (lo, hi) = (min.min(max), min.max(max));
            let fits: Vec<usize> = LENGTHS
                .iter()
                .copied()
                .filter(|l| {
                    let dur = *l as f64 * typical_beat_secs;
                    dur >= lo && dur <= hi
                })
                .collect();
            if !fits.is_empty() {
                return fits;
            }
            let target = (lo * hi).sqrt().max(1e-3);
            let nearest = LENGTHS
                .iter()
                .copied()
                .min_by(|a, b| {
                    let da = (*a as f64 * typical_beat_secs - target).abs();
                    let db = (*b as f64 * typical_beat_secs - target).abs();
                    da.total_cmp(&db)
                })
                .unwrap_or(16);
            vec![nearest]
        }
    }
}

pub fn scan(
    pcm: &TrackPcm,
    stems: Option<&ScanStems>,
    analysis: &TrackAnalysis,
    config: &LoopScanConfig,
) -> Vec<ScoredLoop> {
    let beats = analysis.beats();
    let n = beats.len();
    if n < 8 + CONTEXT_BEATS + 1 || !analysis.grid.has_grid() {
        return Vec::new();
    }
    let f = compute_features(pcm, stems, &beats, analysis.duration_secs);
    // `beats()` starts at beat number `beat_at(0).ceil()`; index i is that
    // beat plus i, which is what `is_downbeat` wants.
    let first_beat = analysis.beat_at(0.0).ceil() as i64;
    let typical_beat = (beats[n - 1] - beats[0]) / (n - 1) as f64;
    let lengths = admissible_lengths(config.bounds, typical_beat);
    if lengths.is_empty() {
        return Vec::new();
    }

    // Every bar-phase-aligned (start, length) whose seam context fits.
    // `beats_ref` is a `&Vec<f64>` (Copy), so `move` closures below can
    // capture it without taking `beats` itself, which the rest of `scan`
    // still needs to own.
    let beats_ref = &beats;
    let raw: Vec<(usize, usize)> = (0..n)
        .filter(|i| analysis.grid.is_downbeat(first_beat + *i as i64))
        .flat_map(|i| {
            lengths
                .iter()
                .copied()
                .filter(move |l| i + l + CONTEXT_BEATS <= n)
                .filter(move |l| match config.bounds {
                    // Seconds mode measures the REAL duration, so a tempo
                    // map cannot smuggle a wrong length through.
                    LengthBounds::Secs { min, max } => {
                        let dur = beats_ref[i + l] - beats_ref[i];
                        dur >= min.min(max) - 1e-9 && dur <= min.max(max) + 1e-9
                    }
                    LengthBounds::Beats { .. } => true,
                })
                .map(move |l| (i, l))
        })
        .collect();
    // A widened seconds set may still fail the exact-duration check above;
    // fall back to the raw admissible lengths without the duration gate.
    let raw: Vec<(usize, usize)> = if raw.is_empty() {
        (0..n)
            .filter(|i| analysis.grid.is_downbeat(first_beat + *i as i64))
            .flat_map(|i| {
                lengths
                    .iter()
                    .copied()
                    .filter(move |l| i + l + CONTEXT_BEATS <= n)
                    .map(move |l| (i, l))
            })
            .collect()
    } else {
        raw
    };

    // Quality gates, doubled until enough survive — every track must yield.
    let target = (config.count * 3).max(12);
    let mut loud_gate = LOUD_GATE_DB;
    let mut chroma_gate = CHROMA_GATE;
    let mut passing: Vec<(usize, usize)>;
    loop {
        passing = raw
            .iter()
            .copied()
            .filter(|(i, l)| {
                (f.rms_db[*i] - f.rms_db[i + l]).abs() <= loud_gate
                    && cos_dist(&f.chroma[*i], &f.chroma[i + l]) <= chroma_gate
            })
            .collect();
        if passing.len() >= target.min(raw.len()) || loud_gate >= LOUD_GATE_CAP_DB {
            break;
        }
        loud_gate = (loud_gate * 2.0).min(LOUD_GATE_CAP_DB);
        chroma_gate = (chroma_gate * 2.0).min(2.0);
    }

    let bar_secs = analysis.grid.beat_secs * 4.0;
    let mut scored: Vec<(usize, usize, f32)> = passing
        .into_iter()
        .map(|(i, l)| {
            let start = beats[i];
            let end = beats[i + l];
            let inside = analysis
                .changes_secs
                .iter()
                .filter(|c| **c > start + bar_secs && **c < end - bar_secs)
                .count();
            (i, l, seam_score(&f, i, l) + STRUCT_PENALTY * inside as f32)
        })
        .collect();
    scored.sort_by(|a, b| a.2.total_cmp(&b.2));

    // Greedy pick, INs at least a bar apart, best first.
    let mut out: Vec<ScoredLoop> = Vec::new();
    let mut kept: Vec<i64> = Vec::new();
    for (i, l, score) in scored {
        let beat = first_beat + i as i64;
        if kept.iter().any(|k| (k - beat).abs() < MIN_IN_SPACING_BEATS) {
            continue;
        }
        kept.push(beat);
        out.push(ScoredLoop {
            span: LoopSpan { start_secs: beats[i], end_secs: beats[i + l] },
            beats: l as u32,
            score,
        });
        if out.len() >= config.count.max(1) {
            break;
        }
    }
    out
}

use crate::decks::DeckId;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;

pub struct ScanJob {
    pub deck: DeckId,
    pub gen: u64,
    pub pcm: Arc<TrackPcm>,
    pub analysis: Arc<crate::wave_analysis::TrackAnalysis>,
    pub config: LoopScanConfig,
    pub stems_root: Option<PathBuf>,
    pub digest: Option<String>,
}

pub struct ScanDone {
    pub deck: DeckId,
    pub gen: u64,
    pub loops: Vec<ScoredLoop>,
}

/// Cached stems for scoring, when the whole track has them.  Read-only in
/// spirit: `cache_is_complete` is asked BEFORE `open_cache`, because opening
/// an entry to ask about it would leave an empty one behind in the budget.
/// The scan never separates — a track without cached stems scores the mix.
fn load_scan_stems(root: &PathBuf, pcm: &TrackPcm, digest: Option<&str>) -> Option<ScanStems> {
    let frames = crate::stems::model_frames(pcm);
    if frames == 0 {
        return None;
    }
    let owned;
    let digest = match digest {
        Some(d) => d,
        None => {
            owned = crate::stems::track_digest(pcm);
            &owned
        }
    };
    if !crate::stems::cache_is_complete(root, digest, frames as u64) {
        return None;
    }
    let mut cache = crate::stems::open_cache(root, pcm, digest)?;
    let set = cache.read_all().ok()?;
    let mono = |buf: &makepad_ai_stems::StereoBuf| -> Vec<f32> {
        buf.left
            .iter()
            .zip(&buf.right)
            .map(|(l, r)| (l + r) * 0.5)
            .collect()
    };
    Some(ScanStems {
        drums: mono(&set[0]),
        vocals: mono(&set[3]),
        sample_rate: crate::stems::STEMS_RATE,
    })
}

/// One scan thread.  A whole-track STFT is seconds of work on a long file
/// and must never touch the UI thread or the audio callback — the same law
/// as `AnalysisPool`, and the same shape.
pub struct LoopScanPool {
    tx: Sender<ScanJob>,
    jobs: Option<Receiver<ScanJob>>,
    done_tx: Sender<ScanDone>,
    rx: Receiver<ScanDone>,
}

impl Default for LoopScanPool {
    fn default() -> Self {
        LoopScanPool::new()
    }
}

impl LoopScanPool {
    pub fn new() -> LoopScanPool {
        let (tx, jobs) = channel::<ScanJob>();
        let (done_tx, rx) = channel::<ScanDone>();
        LoopScanPool { tx, jobs: Some(jobs), done_tx, rx }
    }

    pub fn start(&mut self, spawner: ThreadSpawner) {
        let Some(jobs) = self.jobs.take() else { return };
        let done_tx = self.done_tx.clone();
        match spawner.spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let stems = job.stems_root.as_ref().and_then(|root| {
                        load_scan_stems(root, &job.pcm, job.digest.as_deref())
                    });
                    let loops = scan(&job.pcm, stems.as_ref(), &job.analysis, &job.config);
                    if done_tx
                        .send(ScanDone { deck: job.deck, gen: job.gen, loops })
                        .is_err()
                    {
                        return;
                    }
                }
            }) {
            Ok(handle) => handle.detach(),
            Err(error) => makepad_widgets::log!("vj loop-scan worker unavailable: {error}"),
        }
    }

    pub fn submit(&self, job: ScanJob) {
        let _ = self.tx.send(job);
    }

    pub fn poll(&self) -> Vec<ScanDone> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(done) => out.push(done),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// A stereo i16 track of `beats` half-second beats at `rate`, built
    /// from `tone(beat_index, t_in_beat) -> sample in [-1, 1]`.
    fn synth(rate: u32, beats: usize, tone: impl Fn(usize, f32) -> f32) -> TrackPcm {
        let beat_len = rate as usize / 2; // 120 BPM
        let mut frames = Vec::with_capacity(beats * beat_len);
        for beat in 0..beats {
            for s in 0..beat_len {
                let t = s as f32 / rate as f32;
                let v = (tone(beat, t) * 20000.0) as i16;
                frames.push([v, v]);
            }
        }
        TrackPcm { frames, sample_rate: rate }
    }

    fn beat_times(beats: usize) -> Vec<f64> {
        (0..beats).map(|b| b as f64 * 0.5).collect()
    }

    fn sine(freq: f32, t: f32) -> f32 {
        (t * freq * TAU).sin()
    }

    #[test]
    fn chroma_tells_pitches_apart_and_matches_octaves() {
        let rate = 22050;
        // Beats 0..4 play A4, 4..8 play D5, 8..12 play A5 (A4's octave).
        let pcm = synth(rate, 12, |beat, t| match beat / 4 {
            0 => sine(440.0, t),
            1 => sine(587.33, t),
            _ => sine(880.0, t),
        });
        let beats = beat_times(12);
        let f = compute_features(&pcm, None, &beats, 6.0);
        assert_eq!(f.chroma.len(), 12);
        let a_vs_d = cos_dist(&f.chroma[1], &f.chroma[5]);
        let a_vs_a_octave = cos_dist(&f.chroma[1], &f.chroma[9]);
        assert!(a_vs_a_octave < 0.1, "same pitch class: {a_vs_a_octave}");
        assert!(a_vs_d > a_vs_a_octave + 0.2, "different pitch class: {a_vs_d}");
    }

    #[test]
    fn mfcc_tells_timbres_apart() {
        let rate = 22050;
        // Same pitch class, different spectra: pure sine vs odd harmonics.
        let pcm = synth(rate, 8, |beat, t| {
            if beat < 4 {
                sine(440.0, t)
            } else {
                0.6 * sine(440.0, t) + 0.3 * sine(1320.0, t) + 0.15 * sine(2200.0, t)
            }
        });
        let beats = beat_times(8);
        let f = compute_features(&pcm, None, &beats, 4.0);
        let same = euclid13(&f.mfcc[1], &f.mfcc[2]);
        let diff = euclid13(&f.mfcc[1], &f.mfcc[6]);
        assert!(diff > same * 3.0, "timbre change must dominate: {same} vs {diff}");
        assert!(f.mfcc_norm > 0.0 && f.mfcc_norm.is_finite());
    }

    #[test]
    fn rms_tracks_level_and_stems_ride_their_own_rate() {
        let rate = 22050;
        let pcm = synth(rate, 8, |beat, t| {
            let g = if beat < 4 { 1.0 } else { 0.1 };
            g * sine(330.0, t)
        });
        let beats = beat_times(8);
        // A stem lane at a DIFFERENT rate: loud only in beats 4..8.
        let stem_rate = 16000u32;
        let mut vocals = vec![0.0f32; stem_rate as usize * 4];
        for (i, v) in vocals.iter_mut().enumerate().skip(stem_rate as usize * 2) {
            *v = sine(220.0, i as f32 / stem_rate as f32) * 0.5;
        }
        let stems = ScanStems { drums: vocals.clone(), vocals, sample_rate: stem_rate };
        let f = compute_features(&pcm, Some(&stems), &beats, 4.0);
        assert!(f.rms_db[1] > f.rms_db[6] + 10.0, "mix got quieter");
        let vocals_db = f.vocals_db.as_ref().expect("stem lane present");
        assert!(vocals_db[6] > vocals_db[1] + 10.0, "stem got louder");
        assert_eq!(vocals_db.len(), f.rms_db.len(), "one value per beat");
    }

    use crate::wave_analysis::{TempoMap, TrackAnalysis, TrackGrid, WaveTiles};

    fn analysis_120bpm(beats: usize) -> TrackAnalysis {
        TrackAnalysis {
            duration_secs: beats as f64 * 0.5,
            sample_rate: 22050,
            grid: TrackGrid {
                bpm: 120.0,
                beat_secs: 0.5,
                first_beat_secs: 0.0,
                downbeat_phase: 0,
                confidence: 1.0,
            },
            tempo_map: TempoMap::default(),
            tiles: WaveTiles::default(),
            changes_secs: Vec::new(),
        }
    }

    /// Chord per beat from a repeating 16-beat progression, with a noisy
    /// intro and a clashing outro so the seams outside the body are bad.
    fn body_track(rate: u32) -> TrackPcm {
        let prog = [0, 0, 5, 5, 7, 7, 5, 5, 3, 3, 8, 8, 10, 10, 8, 8]; // semitones
        synth(rate, 96, move |beat, t| {
            if beat < 16 {
                // Intro: pitchless noise-ish sweep.
                sine(90.0 + beat as f32 * 55.0, t) * 0.6
            } else if beat < 80 {
                // Body: the 16-beat progression, four times over.
                let root = 220.0 * 2f32.powf(prog[(beat - 16) % 16] as f32 / 12.0);
                0.5 * sine(root, t) + 0.3 * sine(root * 1.5, t)
            } else {
                // Outro: unrelated cluster.
                0.4 * (sine(311.0, t) + sine(370.0, t) + sine(466.0, t))
            }
        })
    }

    #[test]
    fn the_repeating_body_wins_with_its_own_period() {
        let pcm = body_track(22050);
        let analysis = analysis_120bpm(96);
        let config = LoopScanConfig {
            bounds: LengthBounds::Beats { min: 8, max: 64 },
            count: 3,
        };
        let loops = scan(&pcm, None, &analysis, &config);
        assert!(!loops.is_empty());
        let best = &loops[0];
        assert_eq!(best.beats % 16, 0, "period must be a body multiple: {best:?}");
        assert!(best.span.start_secs >= 7.0 && best.span.end_secs <= 41.0,
            "best loop inside the body: {best:?}");
        assert!(loops.windows(2).all(|w| w[0].score <= w[1].score), "sorted best-first");
    }

    #[test]
    fn seconds_bounds_pick_the_lengths() {
        let pcm = body_track(22050);
        let analysis = analysis_120bpm(96);
        let config = LoopScanConfig {
            bounds: LengthBounds::Secs { min: 7.0, max: 9.0 }, // only 16 beats = 8 s fits
            count: 4,
        };
        let loops = scan(&pcm, None, &analysis, &config);
        assert!(!loops.is_empty());
        assert!(loops.iter().all(|l| l.beats == 16), "{loops:?}");
    }

    #[test]
    fn impossible_seconds_bounds_widen_to_the_nearest_length() {
        let pcm = body_track(22050);
        let analysis = analysis_120bpm(96);
        let config = LoopScanConfig {
            bounds: LengthBounds::Secs { min: 4.4, max: 5.6 }, // no power of two lands here
            count: 2,
        };
        let loops = scan(&pcm, None, &analysis, &config);
        assert!(!loops.is_empty(), "widening must rescue an empty length set");
    }

    #[test]
    fn hostile_material_still_returns_something_and_dedup_spaces_the_ins() {
        // Pseudo-random chatter: every seam is bad, gates must relax.
        let seed = 0x2545f491u32;
        let pcm = synth(22050, 64, move |beat, t| {
            let mut s = seed.wrapping_add(beat as u32).wrapping_mul(747796405);
            s ^= s >> 13;
            sine(180.0 + (s % 700) as f32, t) * 0.5
        });
        let analysis = analysis_120bpm(64);
        let config = LoopScanConfig {
            bounds: LengthBounds::Beats { min: 8, max: 16 },
            count: 5,
        };
        let loops = scan(&pcm, None, &analysis, &config);
        assert!(!loops.is_empty(), "relaxation must always yield candidates");
        for pair in loops.windows(2) {
            let a = analysis.beat_at(pair[0].span.start_secs).round() as i64;
            let b = analysis.beat_at(pair[1].span.start_secs).round() as i64;
            assert!((a - b).abs() >= 4, "INs at least a bar apart: {loops:?}");
        }
        assert!(loops.iter().all(|l| l.score.is_finite()));
    }

    #[test]
    fn a_vocal_cut_at_the_seam_costs() {
        let pcm = body_track(22050);
        let analysis = analysis_120bpm(96);
        let beats = analysis.beats();
        // Vocals lane: silent everywhere except beats 32..40 — a candidate
        // seam at beat 24 wrapping 16 beats has vocals entering mid-context.
        let rate = 22050u32;
        let mut vocals = vec![0.0f32; rate as usize * 48];
        for s in (rate as usize * 16)..(rate as usize * 20) {
            vocals[s] = sine(440.0, s as f32 / rate as f32) * 0.5;
        }
        let stems = ScanStems { drums: vec![0.0; vocals.len()], vocals, sample_rate: rate };
        let with = compute_features(&pcm, Some(&stems), &beats, analysis.duration_secs);
        // Seam beat 24 -> 40: vocals differ across the pair (32..40 loud vs
        // 40..48 silent). Seam beat 48 -> 64: vocals silent on both sides.
        let cut = seam_score(&with, 24, 16);
        let clean = seam_score(&with, 48, 16);
        assert!(cut > clean, "vocal discontinuity must cost: {cut} vs {clean}");
    }

    #[test]
    fn the_pool_answers_with_the_job_identity() {
        let mut pool = LoopScanPool::new();
        pool.start(crate::test_thread_spawner());
        let pcm = std::sync::Arc::new(body_track(22050));
        let analysis = std::sync::Arc::new(analysis_120bpm(96));
        pool.submit(ScanJob {
            deck: crate::decks::DeckId::B,
            gen: 7,
            pcm,
            analysis,
            config: LoopScanConfig {
                bounds: LengthBounds::Beats { min: 8, max: 32 },
                count: 4,
            },
            stems_root: None,
            digest: None,
        });
        let deadline = crate::clock::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let done = pool.poll();
            if let Some(done) = done.into_iter().next() {
                assert_eq!(done.deck, crate::decks::DeckId::B);
                assert_eq!(done.gen, 7);
                assert!(!done.loops.is_empty());
                break;
            }
            assert!(crate::clock::Instant::now() < deadline, "worker never answered");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Real-material sweep: point VJ_LOOP_CORPUS at a folder of MP3s.
    /// Ignored by default — run with:
    /// cargo test -p makepad-vj --release scan_corpus -- --ignored --nocapture
    #[test]
    #[ignore]
    fn scan_corpus_yields_sane_loops() {
        let dir = std::env::var("VJ_LOOP_CORPUS").unwrap_or_else(|_| {
            r"D:\torrent\Dua Lipa - Radical Optimism (HMV Deluxe Edition) - 2025 - mp3 320kbps-EICHBAUM".into()
        });
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("corpus dir") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("mp3"))
                != Some(true)
            {
                continue;
            }
            // Same decode path the local-file explorer uses for a real
            // track: extension -> MediaType -> media::decode_audio_clip.
            // See wave_analysis::decode_audio_file, media.rs:2004-2072.
            let pcm = crate::wave_analysis::decode_audio_file(&path)
                .unwrap_or_else(|e| panic!("{path:?}: decode failed: {e}"));
            let analysis = crate::wave_analysis::analyze(&pcm);
            if !analysis.grid.has_grid() {
                println!("{path:?}: no grid, skipped");
                continue;
            }
            let loops = scan(&pcm, None, &analysis, &LoopScanConfig::default());
            println!(
                "{:?}: bpm {:.1}, {} loops",
                path.file_name(),
                analysis.grid.bpm,
                loops.len()
            );
            assert!(!loops.is_empty(), "{path:?} yielded nothing");
            for l in &loops {
                println!(
                    "   {:>7.2}s .. {:>7.2}s  {} beats  score {:.4}",
                    l.span.start_secs, l.span.end_secs, l.beats, l.score
                );
                assert!(l.score.is_finite());
                assert!(
                    l.span.start_secs >= 0.0
                        && l.span.end_secs <= analysis.duration_secs + 1e-6
                );
                let dur = l.span.len_secs();
                assert!(
                    dur >= 3.0 && dur <= 12.0,
                    "default 4-10s bounds (one admissible-L widening allowed): {dur}"
                );
            }
            checked += 1;
        }
        assert!(checked > 0, "no mp3s found in {dir}");
    }

    #[test]
    fn scan_settings_round_trip_through_text() {
        let settings = ScanSettings {
            unit_beats: true,
            min_secs: 6.5,
            max_secs: 12.0,
            min_beats_ix: 2,
            max_beats_ix: 5,
            count: 7,
            automatic: true,
        };
        assert_eq!(ScanSettings::from_text(&settings.to_text()), settings);
    }

    #[test]
    fn malformed_settings_fall_back_key_by_key() {
        let default = ScanSettings::default();
        let body = "unit beats\n\
                    min_secs nonsense\n\
                    max_secs inf\n\
                    count 0\n\
                    min_beats 99\n\
                    automatic 1\n\
                    a_line_with_no_value\n";
        let settings = ScanSettings::from_text(body);
        assert!(settings.unit_beats, "a good key still lands");
        assert!(settings.automatic, "and so does the one after the bad ones");
        assert_eq!(settings.min_secs, default.min_secs, "unparsable falls back");
        assert_eq!(settings.max_secs, default.max_secs, "non-finite falls back");
        assert_eq!(settings.count, default.count, "out of range falls back");
        assert_eq!(settings.min_beats_ix, default.min_beats_ix, "off-ladder falls back");
    }

    #[test]
    fn settings_become_a_config_with_bounds_in_order() {
        let swapped = ScanSettings {
            unit_beats: false,
            min_secs: 12.0,
            max_secs: 4.0,
            count: 99,
            ..ScanSettings::default()
        };
        let config = swapped.to_config();
        assert_eq!(config.bounds, LengthBounds::Secs { min: 4.0, max: 12.0 });
        assert_eq!(config.count, crate::decks::FOUND_LOOP_CAP, "count clamps to the cap");
        let beats = ScanSettings {
            unit_beats: true,
            min_beats_ix: 3,
            max_beats_ix: 1,
            ..ScanSettings::default()
        };
        assert_eq!(
            beats.to_config().bounds,
            LengthBounds::Beats { min: 16, max: 64 },
            "the ladder is read in order too"
        );
    }
}

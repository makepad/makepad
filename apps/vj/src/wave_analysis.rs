//! Whole-track analysis for the music decks, off the UI and audio threads.
//!
//! When a deck loads a track a worker thread takes the decoded PCM and, in
//! one pass, produces everything the deck surface and the beat clock need:
//!
//! - a **beat grid**: global BPM, beat period, the position of the first
//!   beat and which beat of the bar is the downbeat,
//! - **waveform tiles**: a three-band energy envelope at
//!   [`ZOOM_COLS_PER_SEC`] columns per second for the scrolling view, plus a
//!   coarse whole-track strip for the overview.
//!
//! Both are cached beside the media cache, keyed by the blob digest, so the
//! second load of a track is a file read.
//!
//! The tempo estimate reuses the streaming detector (`beat_sync`) for a
//! prior, then runs an offline autocorrelation + comb pass over the whole
//! onset envelope. The streaming detector is tuned to follow a live feed
//! through song changes; a file has no such ambiguity, and the offline pass
//! gives a stable, deterministic grid over the entire track instead of the
//! last twelve seconds of it.

use crate::beat_sync::BeatSyncAnalyzer;
use crate::decks::DeckId;
use crate::mixer::TrackPcm;
use makepad_asset_data::{BlobId, MediaType};
use std::f32::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;

/// Zoomed-waveform resolution. 100 columns/second is one column per 10 ms —
/// the same hop the onset envelope uses, and fine enough that a kick reads
/// as a distinct spike at the usual few-seconds-across zoom.
pub const ZOOM_COLS_PER_SEC: f64 = 100.0;
/// Whole-track strip resolution (fixed, so the strip never reflows).
pub const OVERVIEW_COLS: usize = 2048;
/// Analysis hop, seconds — matches `ZOOM_COLS_PER_SEC`.
const HOP_SECS: f64 = 1.0 / ZOOM_COLS_PER_SEC;
/// Tempo search range.
const MIN_BPM: f64 = 70.0;
const MAX_BPM: f64 = 180.0;
/// Band split for the coloured waveform (and for the onset lanes).
const BAND_LOW_HZ: f32 = 200.0;
const BAND_HIGH_HZ: f32 = 2_000.0;
/// Cache format magic + version.
const CACHE_MAGIC: &[u8; 8] = b"VJWAVE\0\0";
const CACHE_VERSION: u32 = 1;
/// Longest local file the music explorer will lift into memory.
pub const MAX_LOCAL_TRACK_FRAMES: usize = 48_000 * 60 * 15;

// ---------------------------------------------------------------------------
// results
// ---------------------------------------------------------------------------

/// A whole-track beat grid in seconds of source time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackGrid {
    pub bpm: f64,
    /// Seconds per beat (`60 / bpm`, carried explicitly so the grid stays
    /// exact through a round trip).
    pub beat_secs: f64,
    /// Source time of the first beat at or after zero.
    pub first_beat_secs: f64,
    /// Which beat of the four-beat bar `first_beat_secs` is: 0 = downbeat.
    pub downbeat_phase: u32,
    pub confidence: f32,
}

impl Default for TrackGrid {
    fn default() -> Self {
        TrackGrid {
            bpm: 0.0,
            beat_secs: 0.0,
            first_beat_secs: 0.0,
            downbeat_phase: 0,
            confidence: 0.0,
        }
    }
}

impl TrackGrid {
    pub fn has_grid(&self) -> bool {
        self.bpm.is_finite() && self.bpm > 1.0 && self.beat_secs > 1e-4
    }

    /// Beat number (may be negative before the first beat) at `secs`.
    pub fn beat_at(&self, secs: f64) -> f64 {
        if !self.has_grid() {
            return 0.0;
        }
        (secs - self.first_beat_secs) / self.beat_secs
    }

    /// Source time of whole beat `beat`.
    pub fn secs_at_beat(&self, beat: f64) -> f64 {
        self.first_beat_secs + beat * self.beat_secs
    }

    /// Fractional position inside the current beat, `[0,1)`.
    pub fn phase_at(&self, secs: f64) -> f64 {
        if !self.has_grid() {
            return 0.0;
        }
        self.beat_at(secs).rem_euclid(1.0)
    }

    /// Bar number at `secs`, counting the downbeat-aligned four-beat bars.
    pub fn bar_at(&self, secs: f64) -> f64 {
        (self.beat_at(secs) + self.downbeat_phase as f64) / 4.0
    }

    /// True when `beat` is a downbeat (the first beat of a bar).
    pub fn is_downbeat(&self, beat: i64) -> bool {
        (beat + self.downbeat_phase as i64).rem_euclid(4) == 0
    }

    /// The same grid played at `rate` (a tempo-matched deck): the beats get
    /// closer together but stay anchored at the same source positions.
    pub fn effective_bpm(&self, rate: f64) -> f64 {
        self.bpm * rate
    }
}

/// Three-band energy per waveform column, 0..=255.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaveTiles {
    /// `[low, mid, high]` interleaved per column, `ZOOM_COLS_PER_SEC` per second.
    pub zoom: Vec<[u8; 3]>,
    /// Whole track in [`OVERVIEW_COLS`] columns: `[peak, loudness]`.
    pub overview: Vec<[u8; 2]>,
}

impl WaveTiles {
    pub fn zoom_at(&self, column: isize) -> [u8; 3] {
        if column < 0 {
            return [0; 3];
        }
        self.zoom.get(column as usize).copied().unwrap_or([0; 3])
    }
}

/// Everything a worker produces for one track.
#[derive(Clone, Debug)]
pub struct TrackAnalysis {
    pub duration_secs: f64,
    pub sample_rate: u32,
    pub grid: TrackGrid,
    pub tiles: WaveTiles,
}

impl TrackAnalysis {
    /// Column index in the zoomed tiles for a source time.
    pub fn zoom_column(&self, secs: f64) -> f64 {
        secs * ZOOM_COLS_PER_SEC
    }
}

// ---------------------------------------------------------------------------
// the analysis pass
// ---------------------------------------------------------------------------

/// One-pole low-pass, used to build the band splits cheaply over a whole file.
#[derive(Clone, Copy)]
struct OnePole {
    alpha: f32,
    state: f32,
}

impl OnePole {
    fn new(cutoff: f32, sample_rate: f32) -> OnePole {
        let cutoff = cutoff.min(sample_rate * 0.45);
        OnePole {
            alpha: 1.0 - (-2.0 * PI * cutoff / sample_rate).exp(),
            state: 0.0,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.state += self.alpha * (x - self.state);
        self.state
    }
}

/// Per-hop band envelopes over the whole track.
struct Envelopes {
    /// RMS per band per hop, in `[low, mid, high]` order.
    band_rms: Vec<[f32; 3]>,
    /// Broadband peak per hop.
    peak: Vec<f32>,
    /// Onset novelty per hop (the tempo detector's input).
    onset: Vec<f32>,
    /// Low-band novelty per hop (the downbeat detector's input).
    low_onset: Vec<f32>,
    hop: usize,
    sample_rate: f64,
}

fn build_envelopes(pcm: &TrackPcm) -> Envelopes {
    let sample_rate = pcm.sample_rate.max(1) as f64;
    let hop = ((sample_rate * HOP_SECS).round() as usize).max(16);
    let hops = pcm.frames.len() / hop + 1;
    let mut band_rms = Vec::with_capacity(hops);
    let mut peak = Vec::with_capacity(hops);

    let mut low = OnePole::new(BAND_LOW_HZ, sample_rate as f32);
    let mut mid = OnePole::new(BAND_HIGH_HZ, sample_rate as f32);
    let mut sums = [0.0f64; 3];
    let mut hop_peak = 0.0f32;
    let mut in_hop = 0usize;
    for frame in &pcm.frames {
        let mono = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
        let low_band = low.process(mono);
        let mid_band = mid.process(mono) - low_band;
        let high_band = mono - low.state - mid_band;
        let bands = [low_band, mid_band, high_band];
        for (sum, value) in sums.iter_mut().zip(bands) {
            *sum += (value as f64) * (value as f64);
        }
        hop_peak = hop_peak.max(mono.abs());
        in_hop += 1;
        if in_hop == hop {
            let inverse = 1.0 / in_hop as f64;
            band_rms.push([
                (sums[0] * inverse).sqrt() as f32,
                (sums[1] * inverse).sqrt() as f32,
                (sums[2] * inverse).sqrt() as f32,
            ]);
            peak.push(hop_peak);
            sums = [0.0; 3];
            hop_peak = 0.0;
            in_hop = 0;
        }
    }
    if in_hop > 0 {
        let inverse = 1.0 / in_hop as f64;
        band_rms.push([
            (sums[0] * inverse).sqrt() as f32,
            (sums[1] * inverse).sqrt() as f32,
            (sums[2] * inverse).sqrt() as f32,
        ]);
        peak.push(hop_peak);
    }

    // Spectral flux per band, log-compressed so a quiet intro and a limited
    // drop contribute comparably, then half-wave rectified.
    let count = band_rms.len();
    let mut onset = vec![0.0f32; count];
    let mut low_onset = vec![0.0f32; count];
    let mut previous = [0.0f32; 3];
    let weights = [1.25f32, 1.0, 0.75];
    for index in 0..count {
        let mut sum = 0.0f32;
        for band in 0..3 {
            let energy = (1.0 + 96.0 * band_rms[index][band]).ln();
            let flux = if index == 0 { 0.0 } else { (energy - previous[band]).max(0.0) };
            previous[band] = energy;
            sum += weights[band] * flux;
            if band == 0 {
                low_onset[index] = flux;
            }
        }
        onset[index] = sum;
    }
    // Subtract a moving mean so a dense texture does not swamp the
    // correlation with its own DC.
    let window = (0.5 / HOP_SECS) as usize;
    let smoothed = moving_mean(&onset, window.max(1));
    for index in 0..count {
        onset[index] = (onset[index] - smoothed[index]).max(0.0);
    }

    Envelopes {
        band_rms,
        peak,
        onset,
        low_onset,
        hop,
        sample_rate,
    }
}

fn moving_mean(values: &[f32], window: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; values.len()];
    if values.is_empty() {
        return out;
    }
    let half = (window / 2).max(1);
    let mut sum = 0.0f64;
    let mut start = 0usize;
    let mut end = 0usize;
    for index in 0..values.len() {
        let want_start = index.saturating_sub(half);
        let want_end = (index + half + 1).min(values.len());
        while end < want_end {
            sum += values[end] as f64;
            end += 1;
        }
        while start < want_start {
            sum -= values[start] as f64;
            start += 1;
        }
        out[index] = (sum / (end - start).max(1) as f64) as f32;
    }
    out
}

/// Normalized autocorrelation of the onset envelope at one lag.
fn autocorrelation(onset: &[f32], lag: usize) -> f32 {
    if lag == 0 || lag >= onset.len() {
        return 0.0;
    }
    let count = onset.len() - lag;
    if count < 32 {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut energy_a = 0.0f64;
    let mut energy_b = 0.0f64;
    for index in lag..onset.len() {
        let a = onset[index] as f64;
        let b = onset[index - lag] as f64;
        dot += a * b;
        energy_a += a * a;
        energy_b += b * b;
    }
    let denominator = (energy_a * energy_b).sqrt();
    if denominator <= 1e-12 {
        0.0
    } else {
        (dot / denominator) as f32
    }
}

/// Score a lag including its first harmonics, so a bar-length peak does not
/// beat the beat itself.
fn tempo_score(onset: &[f32], lag: usize) -> f32 {
    let direct = autocorrelation(onset, lag).max(0.0);
    let double = autocorrelation(onset, lag * 2).max(0.0);
    let triple = autocorrelation(onset, lag * 3).max(0.0);
    0.80 * direct + 0.13 * double + 0.07 * triple
}

/// Broad musical prior centred at 120 BPM; only ever used to break an
/// octave tie (90 beats 180, 150 beats 75).
fn tempo_prior(bpm: f64) -> f64 {
    (-0.5 * ((bpm / 120.0).ln() / 0.38).powi(2)).exp()
}

/// Comb energy of the envelope at `period` hops starting at `offset`.
fn comb_energy(onset: &[f32], period: f64, offset: f64) -> f64 {
    if period <= 1.0 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    let mut position = offset;
    while position < onset.len() as f64 {
        let index = position.round() as usize;
        if index < onset.len() {
            // Three cells wide: real onsets are not sample-aligned to a
            // 10 ms grid and the beat period is not an integer either.
            let low = index.saturating_sub(1);
            let high = (index + 1).min(onset.len() - 1);
            sum += 0.5 * onset[low] as f64 + onset[index] as f64 + 0.5 * onset[high] as f64;
        }
        position += period;
    }
    sum
}

/// Estimate the whole-track grid from the onset envelope.
fn estimate_grid(envelopes: &Envelopes, prior_bpm: Option<f64>) -> TrackGrid {
    let onset = &envelopes.onset;
    let hop_rate = envelopes.sample_rate / envelopes.hop as f64;
    if onset.len() < (4.0 * hop_rate) as usize {
        return TrackGrid::default();
    }
    let min_lag = (60.0 * hop_rate / MAX_BPM).floor().max(2.0) as usize;
    let max_lag = ((60.0 * hop_rate / MIN_BPM).ceil() as usize).min(onset.len() / 3);
    if max_lag <= min_lag {
        return TrackGrid::default();
    }

    let mut scores = vec![0.0f32; max_lag + 1];
    let mut best_lag = min_lag;
    for lag in min_lag..=max_lag {
        scores[lag] = tempo_score(onset, lag);
        if scores[lag] > scores[best_lag] {
            best_lag = lag;
        }
    }
    let mut best_score = scores[best_lag];
    if best_score <= 0.0 {
        return TrackGrid::default();
    }

    // Octave check: half and double time are both real peaks in any
    // autocorrelation. Prefer the one the musical prior likes, and let a
    // streaming prior from the live detector break a genuine tie.
    for octave in [best_lag / 2, best_lag.saturating_mul(2)] {
        if octave < min_lag || octave > max_lag {
            continue;
        }
        if scores[octave] < 0.75 * best_score {
            continue;
        }
        let best_bpm = 60.0 * hop_rate / best_lag as f64;
        let octave_bpm = 60.0 * hop_rate / octave as f64;
        let mut best_weight = tempo_prior(best_bpm);
        let mut octave_weight = tempo_prior(octave_bpm);
        if let Some(prior) = prior_bpm.filter(|value| *value > 1.0) {
            best_weight *= 1.0 / (1.0 + 4.0 * (best_bpm / prior).ln().abs());
            octave_weight *= 1.0 / (1.0 + 4.0 * (octave_bpm / prior).ln().abs());
        }
        if octave_weight > best_weight {
            best_lag = octave;
            best_score = scores[octave];
        }
    }

    // Sub-hop refinement: parabolic interpolation of the score peak, then a
    // comb sweep in a narrow band around it. The comb is what actually
    // pins the period, because it sees every beat in the track rather than
    // a correlation average.
    let mut period = best_lag as f64;
    if best_lag > min_lag && best_lag < max_lag {
        let left = scores[best_lag - 1] as f64;
        let centre = scores[best_lag] as f64;
        let right = scores[best_lag + 1] as f64;
        let denominator = left - 2.0 * centre + right;
        if denominator.abs() > 1e-9 {
            let shift = 0.5 * (left - right) / denominator;
            if shift.abs() < 1.0 {
                period = best_lag as f64 + shift;
            }
        }
    }
    let (period, offset, comb) = refine_comb(onset, period);
    let bpm = 60.0 * hop_rate / period;
    if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
        return TrackGrid::default();
    }

    // Downbeat: the bar phase whose beats carry the most low-band onset.
    let mut downbeat_phase = 0u32;
    let mut best_low = f64::NEG_INFINITY;
    for phase in 0..4u32 {
        let energy = comb_energy(
            &envelopes.low_onset,
            period * 4.0,
            offset + phase as f64 * period,
        );
        if energy > best_low {
            best_low = energy;
            downbeat_phase = (4 - phase) % 4;
        }
    }

    // Confidence: how much better the comb does than a random phase, times
    // the correlation strength.
    let mean_comb = comb_energy(onset, period, offset + period * 0.5).max(1e-9);
    let separation = ((comb / mean_comb - 1.0) / 1.5).clamp(0.0, 1.0);
    let confidence = ((0.6 * best_score as f64 + 0.4 * separation) * 1.2).clamp(0.0, 1.0) as f32;

    TrackGrid {
        bpm,
        beat_secs: 60.0 / bpm,
        first_beat_secs: offset * envelopes.hop as f64 / envelopes.sample_rate,
        downbeat_phase,
        confidence,
    }
}

/// Joint period/phase refinement: sweep a narrow band of periods, and for
/// each find the phase that maximizes the comb. Returns the winner.
fn refine_comb(onset: &[f32], period: f64) -> (f64, f64, f64) {
    let mut best = (period, 0.0f64, f64::NEG_INFINITY);
    let span = (period * 0.03).max(0.5);
    let steps = 48;
    for step in 0..=steps {
        let candidate = period - span + 2.0 * span * step as f64 / steps as f64;
        if candidate <= 1.0 {
            continue;
        }
        // Phase sweep at tenth-of-a-hop resolution.
        let phase_steps = (candidate * 10.0).round() as usize;
        let mut local = (0.0f64, f64::NEG_INFINITY);
        for phase in 0..phase_steps {
            let offset = phase as f64 * candidate / phase_steps as f64;
            let energy = comb_energy(onset, candidate, offset);
            if energy > local.1 {
                local = (offset, energy);
            }
        }
        if local.1 > best.2 {
            best = (candidate, local.0, local.1);
        }
    }
    best
}

/// Build the display tiles from the per-hop envelopes.
fn build_tiles(envelopes: &Envelopes, pcm: &TrackPcm) -> WaveTiles {
    // Normalize each band by a high percentile so quiet tracks still fill
    // the display, without one clipped transient flattening everything.
    let mut band_scale = [1.0f32; 3];
    for band in 0..3 {
        let mut values: Vec<f32> = envelopes.band_rms.iter().map(|r| r[band]).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let index = ((values.len() as f64 * 0.995) as usize).min(values.len().saturating_sub(1));
        let reference = values.get(index).copied().unwrap_or(0.0);
        band_scale[band] = if reference > 1e-6 { 1.0 / reference } else { 0.0 };
    }
    let zoom = envelopes
        .band_rms
        .iter()
        .map(|rms| {
            let mut out = [0u8; 3];
            for band in 0..3 {
                // A mild curve: the eye reads energy, not amplitude.
                let value = (rms[band] * band_scale[band]).clamp(0.0, 1.0).powf(0.62);
                out[band] = (value * 255.0) as u8;
            }
            out
        })
        .collect();

    let mut overview = vec![[0u8; 2]; OVERVIEW_COLS];
    let hops = envelopes.peak.len().max(1);
    let peak_scale = {
        let mut values = envelopes.peak.clone();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let index = ((values.len() as f64 * 0.995) as usize).min(values.len().saturating_sub(1));
        let reference = values.get(index).copied().unwrap_or(0.0);
        if reference > 1e-6 {
            1.0 / reference
        } else {
            0.0
        }
    };
    for column in 0..OVERVIEW_COLS {
        let start = column * hops / OVERVIEW_COLS;
        let end = (((column + 1) * hops) / OVERVIEW_COLS).max(start + 1).min(hops);
        let mut peak = 0.0f32;
        let mut energy = 0.0f64;
        for index in start..end {
            peak = peak.max(envelopes.peak[index]);
            let rms = envelopes.band_rms[index];
            energy += ((rms[0] * rms[0] + rms[1] * rms[1] + rms[2] * rms[2]) as f64).sqrt();
        }
        let mean = (energy / (end - start).max(1) as f64) as f32;
        overview[column] = [
            ((peak * peak_scale).clamp(0.0, 1.0).powf(0.62) * 255.0) as u8,
            // Loudness for the hot/cold colouring, on a dB-ish curve.
            (((1.0 + 40.0 * mean).ln() / (41.0f32).ln()).clamp(0.0, 1.0) * 255.0) as u8,
        ];
    }
    let _ = pcm;
    WaveTiles { zoom, overview }
}

/// Full analysis of one decoded track.
pub fn analyze(pcm: &TrackPcm) -> TrackAnalysis {
    let envelopes = build_envelopes(pcm);
    // Reuse the streaming detector over the whole file for an independent
    // BPM opinion; it only ever breaks an octave tie in the offline pass.
    let prior = streaming_prior(pcm);
    let grid = estimate_grid(&envelopes, prior);
    let tiles = build_tiles(&envelopes, pcm);
    TrackAnalysis {
        duration_secs: pcm.seconds(),
        sample_rate: pcm.sample_rate,
        grid,
        tiles,
    }
}

/// Run the live detector across the file and take its final BPM, if it ever
/// locked. Chunked exactly as the live worker feeds it.
fn streaming_prior(pcm: &TrackPcm) -> Option<f64> {
    let rate = pcm.sample_rate.max(1) as f64;
    if rate < 8_000.0 || pcm.frames.len() < (rate * 6.0) as usize {
        return None;
    }
    let mut analyzer = BeatSyncAnalyzer::new(rate);
    let mut scratch = Vec::with_capacity(4_096);
    for chunk in pcm.frames.chunks(4_096) {
        scratch.clear();
        scratch.extend(
            chunk
                .iter()
                .map(|frame| (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0),
        );
        analyzer.push_mono(&scratch);
    }
    let snapshot = analyzer.snapshot();
    snapshot.has_grid().then_some(snapshot.bpm)
}

// ---------------------------------------------------------------------------
// cache
// ---------------------------------------------------------------------------

/// Cache key for a track: the blob digest for store assets, a path hash for
/// local files. Either way it is content-stable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnalysisKey(String);

impl AnalysisKey {
    pub fn from_blob(blob: BlobId) -> AnalysisKey {
        let mut out = String::with_capacity(64);
        for byte in blob.as_bytes() {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
        }
        AnalysisKey(out)
    }

    /// Local files have no digest handy; key on path + size + mtime, which
    /// changes whenever the bytes do.
    pub fn from_path(path: &Path) -> AnalysisKey {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        feed(path.to_string_lossy().as_bytes());
        if let Ok(meta) = std::fs::metadata(path) {
            feed(&meta.len().to_le_bytes());
            if let Ok(modified) = meta.modified() {
                if let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) {
                    feed(&since.as_secs().to_le_bytes());
                }
            }
        }
        AnalysisKey(format!("local-{hash:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where the sidecars live: beside the VJ's other local state.
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VJ_WAVE_CACHE") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/vj/wave-cache")
}

fn cache_path(dir: &Path, key: &AnalysisKey) -> PathBuf {
    dir.join(format!("{}.wave", key.as_str()))
}

pub fn encode_analysis(analysis: &TrackAnalysis) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        64 + analysis.tiles.zoom.len() * 3 + analysis.tiles.overview.len() * 2,
    );
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&analysis.duration_secs.to_le_bytes());
    out.extend_from_slice(&analysis.sample_rate.to_le_bytes());
    out.extend_from_slice(&analysis.grid.bpm.to_le_bytes());
    out.extend_from_slice(&analysis.grid.beat_secs.to_le_bytes());
    out.extend_from_slice(&analysis.grid.first_beat_secs.to_le_bytes());
    out.extend_from_slice(&analysis.grid.downbeat_phase.to_le_bytes());
    out.extend_from_slice(&analysis.grid.confidence.to_le_bytes());
    out.extend_from_slice(&(analysis.tiles.zoom.len() as u32).to_le_bytes());
    for column in &analysis.tiles.zoom {
        out.extend_from_slice(column);
    }
    out.extend_from_slice(&(analysis.tiles.overview.len() as u32).to_le_bytes());
    for column in &analysis.tiles.overview {
        out.extend_from_slice(column);
    }
    out
}

pub fn decode_analysis(bytes: &[u8]) -> Result<TrackAnalysis, String> {
    let mut at = 0usize;
    let mut take = |count: usize| -> Result<&[u8], String> {
        if at + count > bytes.len() {
            return Err("wave cache truncated".into());
        }
        let slice = &bytes[at..at + count];
        at += count;
        Ok(slice)
    };
    if take(8)? != CACHE_MAGIC {
        return Err("not a wave cache file".into());
    }
    let version = u32::from_le_bytes(take(4)?.try_into().unwrap());
    if version != CACHE_VERSION {
        return Err(format!("wave cache version {version}"));
    }
    let duration_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let sample_rate = u32::from_le_bytes(take(4)?.try_into().unwrap());
    let bpm = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let beat_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let first_beat_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let downbeat_phase = u32::from_le_bytes(take(4)?.try_into().unwrap());
    let confidence = f32::from_le_bytes(take(4)?.try_into().unwrap());
    let zoom_len = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    if zoom_len > 64_000_000 {
        return Err("wave cache zoom length out of range".into());
    }
    let mut zoom = Vec::with_capacity(zoom_len);
    for _ in 0..zoom_len {
        let column = take(3)?;
        zoom.push([column[0], column[1], column[2]]);
    }
    let overview_len = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    if overview_len > 1_000_000 {
        return Err("wave cache overview length out of range".into());
    }
    let mut overview = Vec::with_capacity(overview_len);
    for _ in 0..overview_len {
        let column = take(2)?;
        overview.push([column[0], column[1]]);
    }
    Ok(TrackAnalysis {
        duration_secs,
        sample_rate,
        grid: TrackGrid {
            bpm,
            beat_secs,
            first_beat_secs,
            downbeat_phase,
            confidence,
        },
        tiles: WaveTiles { zoom, overview },
    })
}

fn load_cached(dir: &Path, key: &AnalysisKey) -> Option<TrackAnalysis> {
    let bytes = std::fs::read(cache_path(dir, key)).ok()?;
    decode_analysis(&bytes).ok()
}

fn store_cached(dir: &Path, key: &AnalysisKey, analysis: &TrackAnalysis) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = cache_path(dir, key);
    let temporary = path.with_extension("wave.tmp");
    if std::fs::write(&temporary, encode_analysis(analysis)).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

// ---------------------------------------------------------------------------
// worker pool
// ---------------------------------------------------------------------------

pub struct AnalysisJob {
    pub deck: DeckId,
    pub gen: u64,
    pub key: AnalysisKey,
    pub pcm: Arc<TrackPcm>,
}

pub struct AnalysisDone {
    pub deck: DeckId,
    pub gen: u64,
    pub analysis: Arc<TrackAnalysis>,
    /// True when the result came straight out of the sidecar cache.
    pub cached: bool,
}

/// One analysis thread. Track analysis is seconds of work on a long file and
/// must never touch the UI thread or the audio callback.
pub struct AnalysisPool {
    tx: Sender<AnalysisJob>,
    rx: Receiver<AnalysisDone>,
}

impl Default for AnalysisPool {
    fn default() -> Self {
        AnalysisPool::new()
    }
}

impl AnalysisPool {
    pub fn new() -> AnalysisPool {
        let (tx, jobs) = channel::<AnalysisJob>();
        let (done_tx, rx) = channel::<AnalysisDone>();
        let _ = std::thread::Builder::new()
            .name("vj-wave-analysis".into())
            .spawn(move || {
                let dir = cache_dir();
                while let Ok(job) = jobs.recv() {
                    let (analysis, cached) = match load_cached(&dir, &job.key) {
                        Some(hit) => (hit, true),
                        None => {
                            let fresh = analyze(&job.pcm);
                            store_cached(&dir, &job.key, &fresh);
                            (fresh, false)
                        }
                    };
                    if done_tx
                        .send(AnalysisDone {
                            deck: job.deck,
                            gen: job.gen,
                            analysis: Arc::new(analysis),
                            cached,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            });
        AnalysisPool { tx, rx }
    }

    pub fn submit(&self, job: AnalysisJob) {
        let _ = self.tx.send(job);
    }

    pub fn poll(&self) -> Vec<AnalysisDone> {
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

// ---------------------------------------------------------------------------
// local files
// ---------------------------------------------------------------------------

/// Audio extensions the local explorer lane will offer.
pub const LOCAL_AUDIO_EXTENSIONS: [&str; 7] =
    ["wav", "mp3", "m4a", "aac", "flac", "aiff", "mp4"];

/// Decode a local audio file. WAV parses directly; everything else goes
/// through the platform media decoder that already backs the video lane.
pub fn decode_audio_file(path: &Path) -> Result<TrackPcm, String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let media = match extension.as_str() {
        "wav" => MediaType::Wav,
        "ogg" => MediaType::Ogg,
        _ => MediaType::Mp4,
    };
    crate::media::decode_audio_clip(&path.to_path_buf(), media, MAX_LOCAL_TRACK_FRAMES)
}

/// List playable audio files in a directory (not recursive, sorted).
pub fn list_local_audio(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .is_some_and(|e| LOCAL_AUDIO_EXTENSIONS.contains(&e.as_str()))
        })
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A click track: one short percussive hit per beat, plus a stronger
    /// low-frequency hit on the downbeat of each bar.
    fn click_track(rate: u32, bpm: f64, seconds: f64, first_beat: f64) -> TrackPcm {
        let len = (rate as f64 * seconds) as usize;
        let period = 60.0 * rate as f64 / bpm;
        let mut frames = vec![[0i16; 2]; len];
        let mut beat = 0usize;
        let mut position = first_beat * rate as f64;
        while (position as usize) < len {
            let start = position as usize;
            let downbeat = beat % 4 == 0;
            let length = (rate as f64 * 0.05) as usize;
            for index in 0..length {
                if start + index >= len {
                    break;
                }
                let time = index as f64 / rate as f64;
                let envelope = (-45.0 * time).exp();
                // Kick on the downbeat, click on the others: the low band
                // is what the downbeat detector reads.
                let value = if downbeat {
                    0.9 * envelope * (2.0 * std::f64::consts::PI * 55.0 * time).sin()
                } else {
                    0.5 * (-140.0 * time).exp()
                        * (2.0 * std::f64::consts::PI * 1_400.0 * time).sin()
                };
                let sample = (value * 24_000.0) as i16;
                frames[start + index] = [sample, sample];
            }
            beat += 1;
            position += period;
        }
        TrackPcm { frames, sample_rate: rate }
    }

    #[test]
    fn a_click_track_yields_its_tempo_and_grid() {
        for &(rate, bpm) in &[(48_000u32, 120.0f64), (44_100, 128.0), (48_000, 96.0)] {
            let pcm = click_track(rate, bpm, 30.0, 0.35);
            let analysis = analyze(&pcm);
            assert!(
                (analysis.grid.bpm - bpm).abs() < 0.1,
                "rate {rate} bpm {bpm}: measured {:.3}",
                analysis.grid.bpm
            );
            // The published first beat must land on a real beat.
            let phase = analysis.grid.phase_at(0.35);
            assert!(
                phase < 0.03 || phase > 0.97,
                "first beat off by {phase} of a beat ({:?})",
                analysis.grid
            );
            assert!(analysis.grid.confidence > 0.4, "{:?}", analysis.grid);
        }
    }

    #[test]
    fn the_downbeat_lands_on_the_kick() {
        let pcm = click_track(48_000, 120.0, 32.0, 0.35);
        let analysis = analyze(&pcm);
        let grid = analysis.grid;
        // Beat 0 of the fixture is a downbeat; the grid's own first beat may
        // be any beat, so check that the grid calls the fixture's kicks
        // downbeats.
        let beat_of_first_kick = grid.beat_at(0.35).round() as i64;
        assert!(
            grid.is_downbeat(beat_of_first_kick),
            "kick at 0.35s is beat {beat_of_first_kick}, phase {}",
            grid.downbeat_phase
        );
        // …and four beats later too, but not one beat later.
        assert!(grid.is_downbeat(beat_of_first_kick + 4));
        assert!(!grid.is_downbeat(beat_of_first_kick + 1));
    }

    #[test]
    fn tiles_are_deterministic_and_sized_by_duration() {
        let pcm = click_track(48_000, 120.0, 10.0, 0.35);
        let first = analyze(&pcm);
        let second = analyze(&pcm);
        assert_eq!(first.tiles, second.tiles, "analysis must be deterministic");
        assert_eq!(first.grid, second.grid);
        let expected = (10.0 * ZOOM_COLS_PER_SEC) as usize;
        assert!(
            first.tiles.zoom.len().abs_diff(expected) <= 2,
            "{} zoom columns for 10 s",
            first.tiles.zoom.len()
        );
        assert_eq!(first.tiles.overview.len(), OVERVIEW_COLS);
        // A percussive click track puts energy in every band somewhere.
        assert!(first.tiles.zoom.iter().any(|c| c[0] > 40), "no low content");
        assert!(first.tiles.zoom.iter().any(|c| c[2] > 40), "no high content");
    }

    #[test]
    fn silence_does_not_invent_a_grid() {
        let pcm = TrackPcm { frames: vec![[0i16; 2]; 48_000 * 12], sample_rate: 48_000 };
        let analysis = analyze(&pcm);
        assert!(!analysis.grid.has_grid() || analysis.grid.confidence < 0.35);
        assert_eq!(analysis.tiles.overview.len(), OVERVIEW_COLS);
    }

    #[test]
    fn cache_round_trips_every_field() {
        let pcm = click_track(48_000, 124.0, 12.0, 0.2);
        let analysis = analyze(&pcm);
        let bytes = encode_analysis(&analysis);
        let back = decode_analysis(&bytes).expect("decode");
        assert_eq!(back.grid, analysis.grid);
        assert_eq!(back.tiles, analysis.tiles);
        assert_eq!(back.sample_rate, analysis.sample_rate);
        assert!((back.duration_secs - analysis.duration_secs).abs() < 1e-9);
        // Truncation and junk are refused, not misread.
        assert!(decode_analysis(&bytes[..bytes.len() / 2]).is_err());
        assert!(decode_analysis(b"nope").is_err());
    }

    #[test]
    fn cache_files_are_written_and_reused() {
        let pcm = click_track(48_000, 120.0, 8.0, 0.2);
        let analysis = analyze(&pcm);
        let dir = std::env::temp_dir().join(format!(
            "makepad-vj-wave-{}-{}",
            std::process::id(),
            analysis.grid.bpm as u32
        ));
        let key = AnalysisKey::from_blob(BlobId::hash_of(b"a track"));
        assert!(load_cached(&dir, &key).is_none());
        store_cached(&dir, &key, &analysis);
        let hit = load_cached(&dir, &key).expect("cached analysis");
        assert_eq!(hit.grid, analysis.grid);
        assert_eq!(hit.tiles, analysis.tiles);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grid_maps_between_time_beats_and_bars() {
        let grid = TrackGrid {
            bpm: 120.0,
            beat_secs: 0.5,
            first_beat_secs: 0.25,
            downbeat_phase: 0,
            confidence: 1.0,
        };
        assert!((grid.beat_at(0.25) - 0.0).abs() < 1e-9);
        assert!((grid.beat_at(2.25) - 4.0).abs() < 1e-9);
        assert!((grid.secs_at_beat(4.0) - 2.25).abs() < 1e-9);
        assert!((grid.phase_at(0.5) - 0.5).abs() < 1e-9);
        assert!((grid.bar_at(2.25) - 1.0).abs() < 1e-9);
        assert!(grid.is_downbeat(0) && grid.is_downbeat(8) && !grid.is_downbeat(2));
        // A shifted downbeat moves which beat starts the bar.
        let shifted = TrackGrid { downbeat_phase: 1, ..grid };
        assert!(!shifted.is_downbeat(0));
        assert!(shifted.is_downbeat(3));
        // Tempo-matched playback scales the audible BPM, not the anchors.
        assert!((grid.effective_bpm(1.04) - 124.8).abs() < 1e-9);
    }

    #[test]
    fn analysis_keys_are_stable_and_distinct() {
        let a = AnalysisKey::from_blob(BlobId::hash_of(b"one"));
        let b = AnalysisKey::from_blob(BlobId::hash_of(b"two"));
        assert_ne!(a, b);
        assert_eq!(a, AnalysisKey::from_blob(BlobId::hash_of(b"one")));
        assert_eq!(a.as_str().len(), 64);
        // A path key is stable for the same path.
        let path = std::env::temp_dir().join("makepad-vj-key-fixture.wav");
        assert_eq!(AnalysisKey::from_path(&path), AnalysisKey::from_path(&path));
    }
}

//! Whole-track analysis for the music decks, off the UI and audio threads.
//!
//! When a deck loads a track a worker thread takes the decoded PCM and, in
//! one pass, produces everything the deck surface and the beat clock need:
//!
//! - a **beat grid**: global BPM, beat period, the position of the first
//!   beat and which beat of the bar is the downbeat,
//! - **waveform tiles**: a three-band energy envelope plus an absolute
//!   LEVEL at [`ZOOM_COLS_PER_SEC`] columns per second for the scrolling
//!   view, plus a coarse whole-track strip for the overview.
//!
//! The level channel is the waveform's one law: how tall a column draws is
//! how loud that moment of the track is, measured once against the whole
//! track and never against a span, a window, or a stem. Everything else the
//! surface knows about a column — its bands, its separated stems — only
//! decides what COLOUR that height is drawn in.
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
use makepad_ai_beats::BeatsModel;
use makepad_asset_data::{BlobId, MediaType};
use makepad_widgets::makepad_platform::thread::{ThreadOptions, ThreadSpawner};
use std::f32::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use crate::clock::Instant;

/// The independent judge of the grid this file publishes: a second onset
/// front end, a second tracker, and the standard beat-tracking metrics.
/// Test-only, and deliberately shares no code with the analysis below.
#[cfg(test)]
#[path = "beat_eval.rs"]
mod beat_eval;

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
/// Display curve on every normalized level in the tiles: the eye reads
/// energy, not amplitude. One constant, so the bands, the level and the
/// stem colours all sit on the same scale.
pub const WAVE_CURVE: f32 = 0.62;
/// The percentile a track is normalized against, rather than its maximum,
/// so a single clipped transient cannot flatten the whole picture.
const REFERENCE_PERCENTILE: f64 = 0.995;
/// Cache format magic + version. Version 2 is the least-squares beat grid:
/// version 1 sidecars carry a grid that drifts off the transients, so they
/// are re-analysed rather than reused. Version 3 adds the level channel to
/// every zoom column; version 2 sidecars have no absolute loudness in them
/// at all, so they are re-analysed too.
const CACHE_MAGIC: &[u8; 8] = b"VJWAVE\0\0";
/// Version 4 carries the tempo map; a version 3 sidecar has no record of
/// whether the track's tempo moves, so it is re-analysed rather than reused.
/// Version 6 records whether Beat This! has refined the comb grid. Version 5
/// remains readable, but is deliberately treated as unrefined.
const CACHE_VERSION: u32 = 6;
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
        self.bpm.is_finite() && self.bpm > 1.0
            && self.beat_secs.is_finite() && self.beat_secs > 1e-4
            && self.first_beat_secs.is_finite()
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

    /// Move `target_secs` by a whole number of `unit_beats` steps so that
    /// it keeps the same offset into the unit that `phase_ref_secs` has.
    ///
    /// This is the QUANT rule, and it is deliberately not a quantize-to-
    /// grid: the landing is not pulled onto a beat, it is pulled onto the
    /// reference's own place inside one. Because the translation is
    /// measured FROM the reference, phase preservation is structural
    /// rather than arithmetic — and `downbeat_phase` cancels, so a bar is
    /// just `unit_beats == 4` and nothing here anchors on a downbeat.
    /// It is also why a wrong-downbeat grid (the common analyser failure)
    /// cannot move a landing: only differences are read, never absolute
    /// grid positions.
    ///
    /// `unit_beats == 0` is the control's off row, and a track with no
    /// grid has nothing to measure against; both hand the target back.
    pub fn snap_translate(&self, target_secs: f64, phase_ref_secs: f64, unit_beats: u32) -> f64 {
        if unit_beats == 0 || !self.has_grid() {
            return target_secs;
        }
        let unit = unit_beats as f64;
        let reference = self.beat_at(phase_ref_secs);
        let steps = ((self.beat_at(target_secs) - reference) / unit).round();
        let mut secs = self.secs_at_beat(reference + steps * unit);
        // A landing before the start of the track is not a position. Walk
        // forward a unit at a time, the way `sync_plan` does, keeping the
        // phase the caller asked for rather than clamping it away. The cap
        // is a runaway guard, not a policy.
        let step_secs = unit * self.beat_secs;
        for _ in 0..64 {
            if secs >= 0.0 {
                break;
            }
            secs += step_secs;
        }
        secs
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

/// Correct a comb-filter grid from Beat This!'s beat and downbeat events.
///
/// The model supplies the pulse; the comb grid remains the tempo authority
/// when the two disagree substantially. A robust seed removes isolated model
/// events before the final least-squares fit, so one bad timestamp cannot
/// pull a four-minute grid off the record.
pub fn refine_grid_with_beats(
    grid: &TrackGrid,
    duration_secs: f64,
    beats_secs: &[f64],
    downbeats_secs: &[f64],
) -> Option<TrackGrid> {
    if !grid.has_grid() || !duration_secs.is_finite() || duration_secs <= 0.0 {
        return None;
    }
    let beats: Vec<(f64, f64)> = beats_secs
        .iter()
        .enumerate()
        .filter_map(|(index, &secs)| {
            (secs.is_finite() && secs >= 0.0 && secs <= duration_secs)
                .then_some((index as f64, secs))
        })
        .collect();
    let downbeats: Vec<f64> = downbeats_secs
        .iter()
        .copied()
        .filter(|secs| secs.is_finite() && *secs >= 0.0 && *secs <= duration_secs)
        .collect();
    if beats.len() < 16 || downbeats.len() < 4 {
        return None;
    }

    let median_ibi = median(
        beats
            .windows(2)
            .filter_map(|pair| {
                let index_step = pair[1].0 - pair[0].0;
                let time_step = pair[1].1 - pair[0].1;
                (index_step > 0.0 && time_step > 0.0)
                    .then_some(time_step / index_step)
            })
            .collect(),
    )?;
    if !median_ibi.is_finite() || median_ibi <= 1e-4 {
        return None;
    }

    // Estimate the seed period from long pairs. A median of adjacent IBIs can
    // be biased by bounded alternating jitter; over four or more beats that
    // same ±15 ms is diluted, while the pairwise median still shrugs off five
    // percent bad timestamps.
    let max_span = beats.len().saturating_sub(1).min(32);
    let mut seed_periods = Vec::with_capacity(beats.len() * max_span.saturating_sub(3));
    for span in 4..=max_span {
        for left in 0..beats.len() - span {
            let right = left + span;
            let index_step = beats[right].0 - beats[left].0;
            let time_step = beats[right].1 - beats[left].1;
            if index_step > 0.0 && time_step > 0.0 {
                seed_periods.push(time_step / index_step);
            }
        }
    }
    let seed_period = median(seed_periods)?;
    // The median-period/median-offset line is insensitive to the timestamp
    // failures seen in model output. Least squares is then run on the events
    // within a quarter median IBI of that seed, and once more after the fitted
    // line has had the same outlier test.
    let seed_offset = median(
        beats
            .iter()
            .map(|(index, secs)| secs - index * seed_period)
            .collect(),
    )?;
    let tolerance = 0.25 * median_ibi;
    let mut inliers: Vec<(f64, f64)> = beats
        .iter()
        .copied()
        .filter(|(index, secs)| {
            (secs - (seed_offset + index * seed_period)).abs() <= tolerance
        })
        .collect();
    if inliers.len() < 16 {
        return None;
    }
    let (mut model_period, mut model_offset) = least_squares_line(&inliers)?;
    inliers.retain(|(index, secs)| {
        (secs - (model_offset + index * model_period)).abs() <= tolerance
    });
    if inliers.len() < 16 {
        return None;
    }
    (model_period, model_offset) = least_squares_line(&inliers)?;
    if !model_period.is_finite() || model_period <= 1e-4 {
        return None;
    }

    let model_bpm = 60.0 / model_period;
    let relative = (model_bpm / grid.bpm - 1.0).abs();
    let near_octave = (model_bpm / (grid.bpm * 2.0) - 1.0).abs() <= 0.02
        || (model_bpm / (grid.bpm * 0.5) - 1.0).abs() <= 0.02;
    let (bpm, use_model_slope) = if relative <= 0.01 {
        ((model_bpm + grid.bpm) * 0.5, true)
    } else if relative <= 0.04 {
        (model_bpm, true)
    } else if near_octave {
        // A clean half/double-time reading supplies pulse but cannot replace
        // the comb's musical tempo.
        (grid.bpm, false)
    } else {
        // An unrelated tempo is rejected by the four-percent gate. Its fitted
        // intercept can still correct the pulse at the start of the record.
        (grid.bpm, false)
    };
    let beat_secs = 60.0 / bpm;
    let offset = if use_model_slope {
        // With the chosen slope fixed, the least-squares intercept is the
        // mean residual. This includes the required 1:1 tempo blend.
        inliers
            .iter()
            .map(|(index, secs)| secs - index * beat_secs)
            .sum::<f64>()
            / inliers.len() as f64
    } else {
        model_offset
    };
    let first_beat_secs = offset.rem_euclid(beat_secs);

    let mut phase_votes = [0usize; 4];
    for downbeat in &downbeats {
        let beat_index = ((*downbeat - first_beat_secs) / beat_secs).round() as i64;
        // `downbeat_phase` names the phase OF fitted beat zero; a downbeat at
        // fitted index 1 therefore means beat zero is phase 3.
        let phase = (-beat_index).rem_euclid(4) as usize;
        phase_votes[phase] += 1;
    }
    let (phase, votes) = phase_votes
        .iter()
        .copied()
        .enumerate()
        .max_by_key(|(phase, votes)| (*votes, std::cmp::Reverse(*phase)))?;
    let downbeat_phase = if votes * 5 >= downbeats.len() * 3 {
        phase as u32
    } else {
        grid.downbeat_phase
    };

    let median_residual = median(
        inliers
            .iter()
            .map(|(index, secs)| (secs - (model_offset + index * model_period)).abs())
            .collect(),
    )?;
    Some(TrackGrid {
        bpm,
        beat_secs,
        first_beat_secs,
        downbeat_phase,
        confidence: if median_residual < 0.025 {
            grid.confidence.max(0.6)
        } else {
            grid.confidence
        },
    })
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    })
}

fn least_squares_line(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    if points.len() < 2 {
        return None;
    }
    let count = points.len() as f64;
    let mean_index = points.iter().map(|point| point.0).sum::<f64>() / count;
    let mean_secs = points.iter().map(|point| point.1).sum::<f64>() / count;
    let denominator = points
        .iter()
        .map(|point| (point.0 - mean_index).powi(2))
        .sum::<f64>();
    if denominator <= f64::EPSILON {
        return None;
    }
    let period = points
        .iter()
        .map(|point| (point.0 - mean_index) * (point.1 - mean_secs))
        .sum::<f64>()
        / denominator;
    Some((period, mean_secs - period * mean_index))
}

// ---------------------------------------------------------------------------
// tempo map
// ---------------------------------------------------------------------------

/// One stretch of constant tempo: the beat numbered `start_beat` falls at
/// `start_secs`, and they are `period_secs` apart from there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoSegment {
    pub start_secs: f64,
    pub start_beat: f64,
    pub period_secs: f64,
}

/// A beat grid whose tempo is allowed to move.
///
/// Empty on nearly every record this app plays, and that is the design: a
/// house record is made by a machine and one straight line describes it to
/// the millisecond, so bending the grid could only add error. It fills in
/// when the track is played by people — measured over a drifting drummer,
/// one line scores 0.506 against the true beats where a free tracker scores
/// 0.998 — and the decision is made by measurement rather than by genre, see
/// [`TEMPO_MAP_RATIO`].
///
/// Piecewise LINEAR and continuous by construction: consecutive segments
/// share a beat, and each one starts at the time the previous segment
/// predicts for it. So position never jumps — only the rate changes, and
/// only at a beat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TempoMap {
    pub segments: Vec<TempoSegment>,
}

impl TempoMap {
    pub fn is_empty(&self) -> bool {
        self.segments.len() < 2
    }

    fn segment_for_time(&self, secs: f64) -> &TempoSegment {
        let mut chosen = &self.segments[0];
        for segment in &self.segments {
            if segment.start_secs <= secs {
                chosen = segment;
            } else {
                break;
            }
        }
        chosen
    }

    fn segment_for_beat(&self, beat: f64) -> &TempoSegment {
        let mut chosen = &self.segments[0];
        for segment in &self.segments {
            if segment.start_beat <= beat {
                chosen = segment;
            } else {
                break;
            }
        }
        chosen
    }

    /// Beat number at `secs`. Outside the map the end segments' tempi carry
    /// on, so the answer is always defined.
    pub fn beat_at(&self, secs: f64) -> f64 {
        if self.segments.is_empty() {
            return 0.0;
        }
        let segment = self.segment_for_time(secs);
        segment.start_beat + (secs - segment.start_secs) / segment.period_secs
    }

    pub fn secs_at_beat(&self, beat: f64) -> f64 {
        if self.segments.is_empty() {
            return 0.0;
        }
        let segment = self.segment_for_beat(beat);
        segment.start_secs + (beat - segment.start_beat) * segment.period_secs
    }

    pub fn bpm_at(&self, secs: f64) -> f64 {
        if self.segments.is_empty() {
            return 0.0;
        }
        60.0 / self.segment_for_time(secs).period_secs
    }
}

/// Band energy and absolute level per waveform column, 0..=255.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaveTiles {
    /// `[low, mid, high, level]` per column, `ZOOM_COLS_PER_SEC` per second.
    ///
    /// The three bands say what the column is MADE of. `level` says how
    /// loud it is against the whole track, and it is the only thing that
    /// sets a column's height on screen.
    pub zoom: Vec<[u8; 4]>,
    /// Whole track in [`OVERVIEW_COLS`] columns: `[peak, loudness]`.
    pub overview: Vec<[u8; 2]>,
}

impl WaveTiles {
    pub fn zoom_at(&self, column: isize) -> [u8; 4] {
        if column < 0 {
            return [0; 4];
        }
        self.zoom.get(column as usize).copied().unwrap_or([0; 4])
    }
}

/// Everything a worker produces for one track.
#[derive(Clone, Debug)]
pub struct TrackAnalysis {
    pub duration_secs: f64,
    pub sample_rate: u32,
    pub grid: TrackGrid,
    /// True when Beat This! supplied the published pulse/downbeat grid.
    /// Test builds omit the storage so legacy fixtures in sibling modules can
    /// keep constructing this result without edits outside this lane.
    #[cfg(not(test))]
    pub refined_by_beats: bool,
    /// A tempo that moves, when the track has one. Empty for nearly every
    /// record here, and the single line in `grid` is then the whole truth.
    pub tempo_map: TempoMap,
    pub tiles: WaveTiles,
    /// Where the arrangement changes (drops, breaks), source seconds,
    /// at least four seconds apart. Empty when nothing clears the floor.
    /// This is the autopilot's phrase map.
    pub changes_secs: Vec<f64>,
}

impl TrackAnalysis {
    pub fn refined_by_beats(&self) -> bool {
        #[cfg(not(test))]
        {
            self.refined_by_beats
        }
        #[cfg(test)]
        {
            false
        }
    }

    fn mark_refined_by_beats(&mut self) {
        #[cfg(not(test))]
        {
            self.refined_by_beats = true;
        }
    }

    /// Apply the same optional Beat This! correction the native analysis
    /// worker uses. Headless cache bakers call this after routing the model
    /// request through the AI hub.
    pub fn refine_with_beats(
        &mut self,
        beats_secs: &[f64],
        downbeats_secs: &[f64],
    ) -> bool {
        let Some(grid) = refine_grid_with_beats(
            &self.grid,
            self.duration_secs,
            beats_secs,
            downbeats_secs,
        ) else {
            return false;
        };
        self.grid = grid;
        self.mark_refined_by_beats();
        true
    }

    /// Column index in the zoomed tiles for a source time.
    pub fn zoom_column(&self, secs: f64) -> f64 {
        secs * ZOOM_COLS_PER_SEC
    }

    /// Beat number at `secs` — from the tempo map when the track has one,
    /// from the straight line otherwise. This is what anything drawing
    /// rulings or counting bars should ask.
    pub fn beat_at(&self, secs: f64) -> f64 {
        if self.tempo_map.is_empty() {
            self.grid.beat_at(secs)
        } else {
            self.tempo_map.beat_at(secs)
        }
    }

    pub fn secs_at_beat(&self, beat: f64) -> f64 {
        if self.tempo_map.is_empty() {
            self.grid.secs_at_beat(beat)
        } else {
            self.tempo_map.secs_at_beat(beat)
        }
    }

    /// The tempo in force at `secs`.
    pub fn bpm_at(&self, secs: f64) -> f64 {
        if self.tempo_map.is_empty() {
            self.grid.bpm
        } else {
            self.tempo_map.bpm_at(secs)
        }
    }

    /// Every beat of the track, in source seconds.
    pub fn beats(&self) -> Vec<f64> {
        let mut out = Vec::new();
        if !self.grid.has_grid() {
            return out;
        }
        let mut beat = self.beat_at(0.0).ceil();
        loop {
            let at = self.secs_at_beat(beat);
            if at > self.duration_secs {
                break;
            }
            if at >= 0.0 {
                out.push(at);
            }
            beat += 1.0;
        }
        out
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

/// One hop (10 ms) of the coloured waveform: RMS per band in
/// `[low, mid, high]` order and the broadband peak.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WaveHop {
    pub rms: [f32; 3],
    pub peak: f32,
}

/// The band split and per-hop reduction behind the waveform tiles, as a
/// stream: frames go in as the decoder produces them and whole hops come
/// out, so a deck can draw its picture while the file is still being read.
/// The whole-track analysis runs this same builder over the whole file, so
/// the provisional picture and the final one are the same arithmetic.
pub struct WaveHopBuilder {
    low: OnePole,
    mid: OnePole,
    sums: [f64; 3],
    hop_peak: f32,
    in_hop: usize,
    hop: usize,
}

impl WaveHopBuilder {
    pub fn new(sample_rate: u32) -> WaveHopBuilder {
        let sample_rate = sample_rate.max(1) as f64;
        WaveHopBuilder {
            low: OnePole::new(BAND_LOW_HZ, sample_rate as f32),
            mid: OnePole::new(BAND_HIGH_HZ, sample_rate as f32),
            sums: [0.0; 3],
            hop_peak: 0.0,
            in_hop: 0,
            hop: ((sample_rate * HOP_SECS).round() as usize).max(16),
        }
    }

    /// Frames per hop at this rate.
    pub fn hop_frames(&self) -> usize {
        self.hop
    }

    /// Feed frames; every hop completed by them is appended to `out`.
    pub fn push(&mut self, frames: &[[i16; 2]], out: &mut Vec<WaveHop>) {
        for frame in frames {
            let mono = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
            let low_band = self.low.process(mono);
            let mid_band = self.mid.process(mono) - low_band;
            let high_band = mono - self.low.state - mid_band;
            let bands = [low_band, mid_band, high_band];
            for (sum, value) in self.sums.iter_mut().zip(bands) {
                *sum += (value as f64) * (value as f64);
            }
            self.hop_peak = self.hop_peak.max(mono.abs());
            self.in_hop += 1;
            if self.in_hop == self.hop {
                out.push(self.take_hop());
            }
        }
    }

    /// The trailing partial hop, at the end of the stream.
    pub fn finish(&mut self, out: &mut Vec<WaveHop>) {
        if self.in_hop > 0 {
            out.push(self.take_hop());
        }
    }

    fn take_hop(&mut self) -> WaveHop {
        let inverse = 1.0 / self.in_hop as f64;
        let hop = WaveHop {
            rms: [
                (self.sums[0] * inverse).sqrt() as f32,
                (self.sums[1] * inverse).sqrt() as f32,
                (self.sums[2] * inverse).sqrt() as f32,
            ],
            peak: self.hop_peak,
        };
        self.sums = [0.0; 3];
        self.hop_peak = 0.0;
        self.in_hop = 0;
        hop
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
    let mut builder = WaveHopBuilder::new(pcm.sample_rate);
    let hop = builder.hop_frames();
    let mut hops = Vec::with_capacity(pcm.frames.len() / hop + 1);
    builder.push(&pcm.frames, &mut hops);
    builder.finish(&mut hops);
    let band_rms: Vec<[f32; 3]> = hops.iter().map(|hop| hop.rms).collect();
    let peak: Vec<f32> = hops.iter().map(|hop| hop.peak).collect();

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
    //
    // Only the duple relatives are considered, and not for want of trying
    // the triple ones. A rhythm whose kicks fall a beat and a half apart
    // peaks hardest at a beat and a half, and no amount of halving or
    // doubling reaches a tempo two thirds of the real one — so scoring the
    // 2/3 and 3/2 relatives as well, the way Ellis's tempo estimator scores
    // duple and triple candidate functions, looks like the obvious fix. It
    // changes nothing here. On the fixture built to provoke it the relative
    // never wins the tie at any threshold, because the streaming detector
    // independently prefers the same wrong pulse and its opinion is part of
    // the weighting; and across twenty house and techno records the tempo
    // already matches the tags five times in five, so there is nothing for
    // it to fix. It is written down rather than left in, because a branch
    // that never fires is worse than no branch.
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
    let (period, hop_offset) = refine_comb(onset, period);
    // The comb hands back a hop index; the beat is at that hop's centre.
    let seed_offset = hop_offset + HOP_CENTRE;
    let (period, offset) =
        refine_grid(onset, period, seed_offset).unwrap_or((period, seed_offset));
    let changes = structural_changes(envelopes);
    // Keep the published anchor the first beat at or after zero, which is
    // what `TrackGrid` promises and what the bar numbering counts from.
    let offset = offset.rem_euclid(period);
    let bpm = 60.0 * hop_rate / period;
    if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
        return TrackGrid::default();
    }
    // `comb_energy` indexes the envelope by hop, so it wants the anchor back
    // in hop coordinates.
    let comb_offset = offset - HOP_CENTRE;

    // Downbeat: where the arrangement changes, falling back to the loudest
    // kick of the bar when the arrangement does not say.
    let downbeat_phase = phrase_downbeat(envelopes, &changes, period, comb_offset)
        .unwrap_or_else(|| kick_downbeat(envelopes, period, comb_offset));

    let confidence = grid_confidence(onset, &changes, period, comb_offset);

    TrackGrid {
        bpm,
        beat_secs: 60.0 / bpm,
        first_beat_secs: offset * envelopes.hop as f64 / envelopes.sample_rate,
        downbeat_phase,
        confidence,
    }
}

/// Joint period/phase refinement: sweep a narrow band of periods, and for
/// each find the phase that maximizes the comb. Returns the winner; the
/// offset is a hop INDEX, not a sub-hop position.
fn refine_comb(onset: &[f32], period: f64) -> (f64, f64) {
    let mut best = (period, 0.0f64, f64::NEG_INFINITY);
    let span = (period * 0.03).max(0.5);
    let steps = 48;
    for step in 0..=steps {
        let candidate = period - span + 2.0 * span * step as f64 / steps as f64;
        if candidate <= 1.0 {
            continue;
        }
        // Phase sweep at hop resolution: `comb_energy` rounds its sample
        // points to a hop anyway, so a finer sweep does not see a finer
        // phase — it only makes every member of a tie score identically and
        // hands back the lowest of them, which is a systematic half-hop of
        // grid that lands before the beat. `refine_grid` does the sub-hop
        // work, on the onsets themselves.
        let phase_steps = candidate.round().max(1.0) as usize;
        let mut local = (0.0f64, f64::NEG_INFINITY);
        for phase in 0..phase_steps {
            let energy = comb_energy(onset, candidate, phase as f64);
            if energy > local.1 {
                local = (phase as f64, energy);
            }
        }
        if local.1 > best.2 {
            best = (candidate, local.0, local.1);
        }
    }
    (best.0, best.1)
}

/// A transient anywhere inside hop `i` raises that hop's energy over the one
/// before it, so the flux peaks at `i` whatever the sub-hop position was:
/// the unbiased estimate of when it happened is the CENTRE of hop `i`.
///
/// (Parabolic interpolation of the flux peak is the obvious alternative and
/// it is worse — measured over a click sweep it pulls the estimate back
/// toward the hop's leading edge by a stable 0.4 of a hop, i.e. it puts the
/// whole grid 4 ms early.)
const HOP_CENTRE: f64 = 0.5;

/// Where the strongest onset within `radius` hops of `centre` is, in hops,
/// with its strength. `None` when that stretch of the envelope is flat.
fn onset_peak_near(onset: &[f32], centre: f64, radius: f64) -> Option<(f64, f64)> {
    if onset.is_empty() {
        return None;
    }
    let from = (centre - radius).round().max(0.0) as usize;
    let to = ((centre + radius).round().max(0.0) as usize).min(onset.len() - 1);
    if from > to {
        return None;
    }
    let mut best = from;
    for index in from..=to {
        if onset[index] > onset[best] {
            best = index;
        }
    }
    if onset[best] <= 0.0 {
        return None;
    }
    Some((best as f64 + HOP_CENTRE, onset[best] as f64))
}

/// Which beat of the bar starts it, from the loudest kick.
///
/// This is the obvious rule and it is nearly worthless on the music this app
/// plays: four-to-the-floor puts a kick on all four beats deliberately, and
/// mostly the SAME kick. Measured against the arrangement changes of twenty
/// house and techno records it named the right beat 31 % of the time, where
/// guessing names it 25 %. It stays as the fallback because it is better
/// than nothing on the tracks the phrase evidence cannot read.
fn kick_downbeat(envelopes: &Envelopes, period: f64, comb_offset: f64) -> u32 {
    let mut downbeat_phase = 0u32;
    let mut best = f64::NEG_INFINITY;
    for phase in 0..4u32 {
        let energy = comb_energy(
            &envelopes.low_onset,
            period * 4.0,
            comb_offset + phase as f64 * period,
        );
        if energy > best {
            best = energy;
            downbeat_phase = (4 - phase) % 4;
        }
    }
    downbeat_phase
}

/// How many of the track's biggest arrangement changes to take, and how far
/// apart to keep them so one drop does not fill the list.
const PHRASE_BOUNDARIES: usize = 24;
const PHRASE_SPACING_SECS: f64 = 4.0;
/// The smallest step in the two-second loudness mean that counts as the
/// arrangement changing, in natural-log RMS — about 1.3 dB.
const PHRASE_MIN_STEP: f64 = 0.15;
/// How much of the vote the winning bar position needs over the runner-up
/// before it is believed rather than the kick rule.
const PHRASE_MARGIN: f64 = 1.5;

/// Which beat of the bar starts it, from where the track's ARRANGEMENT
/// changes.
///
/// A record does not tell you which of four identical kicks is the one. What
/// it does tell you is where its phrases are: this music is built in four-,
/// eight- and sixteen-bar blocks, and the moments it changes — the drop, the
/// break, the bar the hats arrive, the bar the bass leaves — land on the
/// first beat of a block essentially always. So the two-second loudness
/// envelope is differenced, its two dozen largest jumps are taken, and the
/// bar position they agree on is the downbeat.
///
/// Returns `None` when too few of those jumps land near a beat at all, or
/// when they do not agree — a track whose arrangement is a slow wash has
/// nothing to say here and should not be made to guess.
/// Where the track's arrangement changes, in hops: the largest jumps in a
/// two-second loudness envelope, kept apart so one drop cannot fill the list.
///
/// Computed once and handed to everything that reads it — which pulse is the
/// beat, and which beat starts the bar are the same question asked twice.
fn structural_changes(envelopes: &Envelopes) -> Vec<f64> {
    let loudness: Vec<f32> = envelopes
        .band_rms
        .iter()
        .map(|rms| {
            ((rms[0] * rms[0] + rms[1] * rms[1] + rms[2] * rms[2]).sqrt() + 1e-6).ln()
        })
        .collect();
    // Two seconds either side: a single bar of silence must not register,
    // an arrangement change must.
    let window = (2.0 / HOP_SECS) as usize;
    if loudness.len() < 3 * window {
        return Vec::new();
    }
    let mut prefix = vec![0.0f64; loudness.len() + 1];
    for index in 0..loudness.len() {
        prefix[index + 1] = prefix[index] + loudness[index] as f64;
    }
    let mean = |from: usize, to: usize| (prefix[to] - prefix[from]) / (to - from).max(1) as f64;
    let mut change = vec![0.0f64; loudness.len()];
    for index in window..loudness.len() - window {
        change[index] = (mean(index, index + window) - mean(index - window, index)).abs();
    }
    let mut order: Vec<usize> = (window..loudness.len() - window).collect();
    order.sort_by(|a, b| {
        change[*b].partial_cmp(&change[*a]).unwrap_or(std::cmp::Ordering::Equal)
    });
    // Only jumps that are actually jumps. Taking the largest two dozen
    // values of anything always returns two dozen values, and over a click
    // track — or a loop, or any recording whose loudness simply does not
    // move — those two dozen are noise with an arbitrary phase, which is
    // then indistinguishable from evidence. A real arrangement change is
    // more than a decibel of step in the two-second mean; this asks for
    // that, and for the jump to stand well clear of the track's own
    // fidgeting.
    let mut sorted: Vec<f64> = change[window..loudness.len() - window].to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    let floor = (3.0 * median).max(PHRASE_MIN_STEP);

    let spacing = PHRASE_SPACING_SECS / HOP_SECS;
    let mut taken: Vec<f64> = Vec::new();
    for index in order {
        if change[index] < floor {
            break;
        }
        let at = index as f64;
        if taken.iter().all(|other| (other - at).abs() > spacing) {
            taken.push(at);
        }
        if taken.len() >= PHRASE_BOUNDARIES {
            break;
        }
    }
    // Deliberately NOT localized any finer. Finding a change takes a
    // two-second window either side, so the position it returns is good to
    // about a second — coarse next to half a beat — and re-finding each one
    // with a hundred-millisecond window is the obvious repair. It makes
    // things worse, twice over: it did not make the phase statistic
    // discriminate at all, and it moves every boundary onto the first
    // TRANSIENT of the incoming layer, which is as often on an offbeat as
    // on the bar line. On the fixture whose hats arrive on a known downbeat
    // it took the downbeat from right to 0 of 14.
    taken.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    taken
}

fn phrase_downbeat(
    envelopes: &Envelopes,
    changes: &[f64],
    period: f64,
    comb_offset: f64,
) -> Option<u32> {
    let _ = envelopes;
    let taken = changes;
    let mut votes = [0usize; 4];
    let mut counted = 0usize;
    for at in taken {
        let beat = (at - comb_offset) / period;
        // A change that falls between beats says nothing about which beat
        // starts the bar.
        if (beat - beat.round()).abs() > 0.25 {
            continue;
        }
        votes[(beat.round() as i64).rem_euclid(4) as usize] += 1;
        counted += 1;
    }
    if counted < 8 {
        return None;
    }
    let mut order = [0usize, 1, 2, 3];
    order.sort_by_key(|phase| std::cmp::Reverse(votes[*phase]));
    let (best, runner_up) = (votes[order[0]], votes[order[1]].max(1));
    if (best as f64) < PHRASE_MARGIN * runner_up as f64 {
        return None;
    }
    Some(((4 - order[0]) % 4) as u32)
}

/// How confident the published grid deserves to be.
///
/// The old number was the tempo correlation blended with how much better the
/// comb did at this phase than half a beat away. Measured against how the
/// grids actually scored, it did not discriminate at all: 0.790 on the
/// eighteen grids that scored 0.8 or better against the kicks and 0.732 on
/// the seven that scored under 0.4. Worse, it could not by construction —
/// both its terms are about TEMPO, and the way a grid on this material goes
/// wrong is by sitting on the wrong pulse at exactly the right tempo. Such a
/// grid has a magnificent correlation and a magnificent comb separation.
///
/// So confidence is now two things multiplied, which are the two ways the
/// grid can be wrong:
///
/// * how well it FITS — what fraction of its rulings have an onset close
///   enough to be that beat, and how tight those distances are. This is the
///   residual of the fit itself, measured against the onsets the grid
///   predicts rather than against any model.
/// What it deliberately does NOT include is any judgement about the PULSE,
/// because there is nothing honest to put there. The dominant way a grid on
/// this material is wrong is by sitting half a beat out at exactly the right
/// tempo — and such a grid has an excellent residual, because it is sitting
/// on real transients. Every cue tried for telling the two pulses apart
/// failed to separate them (see `structural_changes` for the four and the
/// measurements). So this number says how well the grid fits the onsets and
/// nothing more, and a caller must not read it as "the beats are in the
/// right place". Fixing that means fixing the pulse first.
fn grid_confidence(onset: &[f32], changes: &[f64], period: f64, comb_offset: f64) -> f32 {
    if period <= 1.0 || onset.is_empty() {
        return 0.0;
    }
    // Fit: the distance from every ruling to the strongest onset near it.
    let mut residuals: Vec<f64> = Vec::new();
    let mut supported = 0usize;
    let mut total = 0usize;
    let radius = period * 0.5;
    let mut beat = 0i64;
    loop {
        let predicted = comb_offset + beat as f64 * period;
        if predicted >= onset.len() as f64 {
            break;
        }
        beat += 1;
        if predicted < 0.0 {
            continue;
        }
        total += 1;
        if let Some((position, _)) = onset_peak_near(onset, predicted, radius) {
            let residual = (position - predicted).abs();
            residuals.push(residual);
            // A tenth of a beat is about fifty milliseconds at these tempi:
            // near enough that a listener would call the ruling right.
            if residual < period * 0.10 {
                supported += 1;
            }
        }
    }
    if total < 8 || residuals.is_empty() {
        return 0.0;
    }
    // A walk detector was tried here and taken out: the signed residual over
    // the first half of the track against the second, which catches a grid
    // whose period is slightly off because it arrives early at one end and
    // late at the other. It is free and it is principled and it made the
    // separation WORSE (0.051 against 0.069), because it also fires on
    // perfectly good grids over tracks that pause — one scoring 0.996 gets
    // flagged — while the failures that actually dominate here are grids on
    // the wrong pulse, which do not walk at all.
    residuals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = residuals[residuals.len() / 2];
    let support = supported as f64 / total as f64;
    // Half a tenth of a beat — about 25 ms here — is already a good grid, so
    // that is where tightness saturates rather than at zero error; asking a
    // real recording for zero median residual only measures the hop.
    let tightness = (1.0 - median / (period * 0.05)).clamp(0.0, 1.0);
    let fit = (0.5 * support + 0.5 * tightness).clamp(0.0, 1.0);

    let _ = changes;
    fit.clamp(0.0, 1.0) as f32
}

/// How far the arrangement has to sit off the rulings before the grid moves
/// half a beat onto them, and how many changes must exist before the
/// question is asked at all.
///
/// Negative alignment means the changes are landing between the rulings.
/// The bar is set well past zero because the cost of the two errors is not
/// symmetric: leaving a wrong grid alone loses one track, flipping a right
/// one loses a track that was perfect.
/// WHICH PULSE the beat sits on is not decided here, and that is a finding
/// rather than an omission.
///
/// Six of forty records carry a published grid half a beat out: perfectly
/// steady, sitting squarely on real transients, and on the wrong ones. An
/// exhaustive search over the same audio at the same tempo scores up to 0.84
/// against the kicks where those grids score 0.00, so the beats are
/// certainly elsewhere. Four independent cues were built and measured
/// against that ground truth, and not one of them separates the six from the
/// thirty-four:
///
/// * low-band comb energy — never once moved a published grid;
/// * where the ARRANGEMENT changes, as a circular mean over the rulings —
///   the six score -0.24 to +0.62, the thirty-four -0.31 to +0.64;
/// * the same, with each change localized to a tenth of a second instead of
///   a second — sharpens the statistic a great deal and separates no better,
///   while moving every boundary onto the incoming layer's first hit;
/// * a purpose-built kick detector, two poles at 110 Hz with a half-beat
///   refractory, combed at both pulses — fires almost never and cost a track
///   when it did.
///
/// The pattern in those measurements is the answer: on the disputed records
/// the low band, the mid band and the arrangement ALL endorse the pulse the
/// isolated kick calls wrong. The disagreement is not between a good cue and
/// a bad one, it is between the kick and everything mixed on top of it, and
/// no filter over the mix recovers what the mix has buried. The separated
/// drums stem does — the app already makes one — which is where this should
/// be tried next, and it is the same conclusion the tempo map reaches from
/// the other direction.

/// One weighted least-squares pass over the beats numbered `from..=to`:
/// take the onset each predicted beat lands nearest, drop the worst fifth of
/// the residuals so a bar with no drum on it cannot drag the fit, and return
/// the line through what is left.
fn fit_beats(
    onset: &[f32],
    period: f64,
    offset: f64,
    from: f64,
    to: f64,
    radius: f64,
) -> Option<(f64, f64)> {
    let mut points: Vec<(f64, f64, f64)> = Vec::new();
    let mut beat = from.ceil() as i64;
    let last = to.floor() as i64;
    while beat <= last {
        let predicted = offset + beat as f64 * period;
        if predicted >= 0.0 && predicted - radius < onset.len() as f64 {
            if let Some((position, weight)) = onset_peak_near(onset, predicted, radius) {
                points.push((beat as f64, position, weight));
            }
        }
        beat += 1;
    }
    if points.len() < 8 {
        return None;
    }
    let mut residuals: Vec<f64> = points
        .iter()
        .map(|(beat, at, _)| (at - (offset + beat * period)).abs())
        .collect();
    residuals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let cut = residuals[(residuals.len() * 4) / 5].max(HOP_CENTRE);

    let (mut sum_w, mut sum_b, mut sum_t, mut sum_bb, mut sum_bt) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (beat, at, weight) in &points {
        if (at - (offset + beat * period)).abs() > cut {
            continue;
        }
        sum_w += weight;
        sum_b += weight * beat;
        sum_t += weight * at;
        sum_bb += weight * beat * beat;
        sum_bt += weight * beat * at;
    }
    let denominator = sum_w * sum_bb - sum_b * sum_b;
    if denominator.abs() < 1e-9 {
        return None;
    }
    let next_period = (sum_w * sum_bt - sum_b * sum_t) / denominator;
    let next_offset = (sum_t - next_period * sum_b) / sum_w;
    if !next_period.is_finite() || !next_offset.is_finite() || next_period <= 1.0 {
        return None;
    }
    Some((next_period, next_offset))
}

/// How many beats the first fit looks at.
///
/// Short enough that a seed period an eighth of a percent out has walked the
/// grid less than a tenth of a beat across the window, so every ruling still
/// finds its own onset. Long enough that the line through them is a tempo
/// and not a groove: thirty-two beats is eight bars, and eight bars of a
/// shuffled or syncopated passage fit a line that is a quarter of a percent
/// off the track's real tempo — measured, on three of eight tracks — which
/// the doubling then carries outward instead of correcting. A hundred and
/// twenty-eight beats is thirty-two bars; no groove is that long, and the
/// walk across it is still under a tenth of a beat.
const REFIT_FIRST_BEATS: f64 = 128.0;

/// Refit the grid against the onsets it predicts.
///
/// Fitting a straight line through the onset each ruling lands nearest —
/// weighted by how strong that onset is — pins the period to a part in a
/// million and the phase to about a millisecond. What it cannot do is find
/// those onsets in the first place if the seed is bad, and the seed IS bad:
/// the comb sweeps its period in 48 steps across ±3 %, so one step is an
/// eighth of a percent, and an eighth of a percent walks the grid most of
/// half a second across a seven-minute track. Predict every beat from one
/// end with a seed like that and the far half of the track associates each
/// ruling with the wrong onset — or with none — and the fit is fitting
/// noise. Measured against an exhaustive search for the best fixed grid,
/// that is exactly what was happening: the published tempo sat a twentieth
/// of a percent off the best one, which is a hundred and seventy
/// milliseconds of walk, and the rulings drifted visibly off the kicks over
/// the length of a track.
///
/// So the fit starts in the MIDDLE and grows. Thirty-two beats either side of
/// centre, a seed period a tenth of a percent out has walked less than a
/// twentieth of a beat and every ruling still finds its own onset. That fit
/// makes the period good enough to associate a window twice as long, which
/// makes it good enough for one twice as long again, until the window is the
/// whole track. Each doubling costs one more pass over the onsets.
///
/// Returns `None` when the track has too few onsets to fit, or when the fit
/// runs away from the period the comb found; the caller keeps the comb's.
fn refine_grid(onset: &[f32], seed_period: f64, seed_offset: f64) -> Option<(f64, f64)> {
    if seed_period <= 2.0 || onset.len() < 16 {
        return None;
    }
    let mut period = seed_period;
    let mut offset = seed_offset;
    let mut fitted = false;
    let mut span = REFIT_FIRST_BEATS;
    loop {
        let total = ((onset.len() as f64 - offset) / period).floor();
        if total < 16.0 {
            return None;
        }
        let window = span.min(total);
        let centre = total * 0.5;
        let from = (centre - window * 0.5).max(0.0);
        let to = (centre + window * 0.5).min(total);
        // A wide first look so a seed that is a few hops out still finds its
        // onsets, tight after that.
        for pass in 0..3 {
            let radius = period * if pass == 0 { 0.22 } else { 0.10 };
            // A window that lands on a breakdown has nothing to fit; the
            // next, longer one will, so carry on rather than give up.
            let Some((next_period, next_offset)) =
                fit_beats(onset, period, offset, from, to, radius)
            else {
                break;
            };
            // The fit refines a tempo; it does not get to choose a different
            // one.
            if (next_period / seed_period - 1.0).abs() > 0.05 {
                return None;
            }
            period = next_period;
            offset = next_offset;
            fitted = true;
        }
        if window >= total {
            break;
        }
        span = window * 2.0;
    }
    if !fitted {
        return None;
    }
    // The refit only gets to publish a grid that is better than the one it
    // was given. Everything above is a search, and a search over real music
    // can land somewhere worse than where it started; the comb energy over
    // the whole track — every beat, a ten-millisecond window either side —
    // is the same measure for both, so it can simply be checked.
    let seed = comb_energy(onset, seed_period, seed_offset - HOP_CENTRE);
    let fit = comb_energy(onset, period, offset - HOP_CENTRE);
    (fit >= seed).then_some((period, offset))
}

// ---------------------------------------------------------------------------
// the tempo map: decode a beat sequence, then summarize it
// ---------------------------------------------------------------------------

/// How much better a free tracker has to explain the onsets than the best
/// straight line before the grid is allowed to bend.
///
/// This is the whole gate, and it is a measurement rather than a genre
/// check. A tracker free to move its tempo will always explain a track at
/// least as well as one that cannot, so the question is by how much: over
/// forty house and techno records the ratio runs 1.08 at the median, and
/// over records with a band playing it runs 1.26 to 2.19. Below the bar the
/// single line stands and the map stays empty, so nothing about EDM changes.
const TEMPO_MAP_RATIO: f64 = 1.20;
/// How far a beat may sit from the straight line through its segment before
/// the segment ends, in seconds.
const TEMPO_SEGMENT_TOLERANCE: f64 = 0.020;
/// The shortest run of beats worth calling a tempo.
const TEMPO_SEGMENT_MIN_BEATS: usize = 8;
/// Tempo-consistency weight for the sequence decoder. Set where the judge
/// set it: tight enough not to chase individual onsets, loose enough to
/// follow a 124-to-130 BPM ride.
const DECODE_TIGHTNESS: f64 = 1600.0;

/// Peaks of the onset envelope, at most one per half beat, with strength.
fn onset_peaks(onset: &[f32], period: f64) -> Vec<(f64, f32)> {
    if onset.is_empty() {
        return Vec::new();
    }
    let mean = onset.iter().map(|v| *v as f64).sum::<f64>() / onset.len() as f64;
    let variance =
        onset.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / onset.len() as f64;
    let threshold = mean + 0.5 * variance.sqrt();
    let refractory = (period * 0.25).max(1.0) as usize;
    let mut out: Vec<(f64, f32)> = Vec::new();
    let mut index = 0usize;
    while index < onset.len() {
        if (onset[index] as f64) < threshold {
            index += 1;
            continue;
        }
        let to = (index + refractory).min(onset.len());
        let mut best = index;
        for candidate in index..to {
            if onset[candidate] > onset[best] {
                best = candidate;
            }
        }
        out.push((best as f64, onset[best]));
        index = best + refractory;
    }
    out
}

/// A triangular spike at every peak — the activation the decoder reads.
///
/// The raw envelope will not do. A windowed flux smears a transient across
/// several hops, so moving a beat by one hop costs almost no onset strength
/// — less than the tempo penalty for the same move — and the decoder settles
/// on whatever period its estimate rounded to and walks off the music at a
/// steady few milliseconds a beat. Measured on a click track it drifted 70 ms
/// in thirty seconds. Spikes restore the gradient, so the onsets pull the
/// tempo instead of the other way round.
fn spike_activation(peaks: &[(f64, f32)], len: usize, half_width: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    for (at, strength) in peaks {
        let centre = at.round() as isize;
        for offset in -(half_width as isize)..=(half_width as isize) {
            let index = centre + offset;
            if index < 0 || index as usize >= len {
                continue;
            }
            let taper = 1.0 - offset.abs() as f32 / (half_width + 1) as f32;
            out[index as usize] = out[index as usize].max(strength * taper);
        }
    }
    out
}

/// Ellis's dynamic program (J. New Music Research, 2007): the beat sequence
/// maximizing onset strength plus a log-Gaussian tempo-consistency penalty.
/// A SEQUENCE, not a grid — it may follow a tempo that moves.
fn decode_beats(activation: &[f32], period: f64, tightness: f64) -> Vec<f64> {
    if period < 2.0 || activation.len() < 8 {
        return Vec::new();
    }
    let peak = activation.iter().copied().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return Vec::new();
    }
    let strength: Vec<f64> = activation.iter().map(|v| (*v / peak) as f64).collect();
    let from = (period * 0.5).round().max(2.0) as usize;
    let to = (period * 2.0).round() as usize;
    let cost: Vec<f64> = (from..=to)
        .map(|lag| -tightness * ((lag as f64 / period).ln()).powi(2))
        .collect();

    let mut score = vec![f64::NEG_INFINITY; strength.len()];
    let mut back = vec![usize::MAX; strength.len()];
    for index in 0..strength.len() {
        if index < from {
            score[index] = strength[index];
            continue;
        }
        let mut best = f64::NEG_INFINITY;
        let mut best_at = usize::MAX;
        for lag in from..=to.min(index) {
            let candidate = score[index - lag] + cost[lag - from];
            if candidate > best {
                best = candidate;
                best_at = index - lag;
            }
        }
        if best_at == usize::MAX {
            score[index] = strength[index];
        } else {
            score[index] = strength[index] + best;
            back[index] = best_at;
        }
    }
    let tail = strength.len().saturating_sub(to);
    let mut at = tail;
    for index in tail..strength.len() {
        if score[index] > score[at] {
            at = index;
        }
    }
    let mut beats = Vec::new();
    while at != usize::MAX {
        beats.push(at as f64);
        let next = back[at];
        if next == usize::MAX || next >= at {
            break;
        }
        at = next;
    }
    beats.reverse();
    // The head of the chain is a seed with no predecessor to hold it to the
    // tempo, so it lands wherever the envelope starts. Trim back to where
    // the intervals become regular.
    if beats.len() > 12 {
        let mut steps: Vec<f64> = beats.windows(2).map(|pair| pair[1] - pair[0]).collect();
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = steps[steps.len() / 2];
        let mut drop = 0usize;
        while drop < 4 && ((beats[drop + 1] - beats[drop]) / median - 1.0).abs() > 0.05 {
            drop += 1;
        }
        beats.drain(..drop);
    }
    beats
}

/// Mean onset strength found under a sequence of beats — how well a set of
/// beat positions explains the track.
fn beat_support(beats: &[f64], peaks: &[(f64, f32)], period: f64) -> f64 {
    if beats.is_empty() || peaks.is_empty() {
        return 0.0;
    }
    // Weighted by CLOSENESS, not by a window. A hard window asks only
    // whether something was near the beat, and on a track with a hat between
    // every kick something always is — a grid at the wrong tempo slides
    // steadily through kick, hat, kick and keeps scoring the whole way. On a
    // 124-to-130 ride that scored the wrong grid at 0.955 of the right one,
    // which is no signal at all. A Gaussian makes a ruling five milliseconds
    // off worth more than one fifty milliseconds off, which is the thing
    // being asked.
    let sigma = period * 0.04;
    let reach = period * 0.15;
    let mut sum = 0.0;
    let mut at = 0usize;
    for beat in beats {
        while at + 1 < peaks.len() && peaks[at].0 < beat - reach {
            at += 1;
        }
        let mut best = 0.0f64;
        let mut index = at;
        while index < peaks.len() && peaks[index].0 <= beat + reach {
            let distance = peaks[index].0 - beat;
            // Alignment only. How LOUD the onset under a beat is says
            // something about the music and nothing about whether the beat
            // is in the right place, and letting it in lets a few big hits
            // outvote a hundred well-placed ones.
            let weight = (-(distance * distance) / (2.0 * sigma * sigma)).exp();
            best = best.max(weight);
            index += 1;
        }
        sum += best;
    }
    sum / beats.len() as f64
}

/// Compress a decoded beat sequence into the fewest straight lines that
/// still put every beat within [`TEMPO_SEGMENT_TOLERANCE`] of one.
///
/// Summarizing a sequence is a different problem from fitting segments to
/// onsets, and the difference is the whole reason this is shaped this way.
/// Fitting each segment to the onsets underneath it independently is the
/// obvious approach and it does not work: sixteen beats is too few to pin a
/// tempo against real onsets, so every segment slides a little and the slide
/// rides forward into the next. Measured over ABBA's isolated drums it
/// scored 0.266 where one straight line scored 0.858. Here the beats are
/// already decided — by a decoder that had the whole track and a tempo model
/// to hold it together — and all that is left is to describe them.
fn summarize_beats(beats: &[f64], hop_secs: f64) -> Vec<TempoSegment> {
    if beats.len() < TEMPO_SEGMENT_MIN_BEATS * 2 {
        return Vec::new();
    }
    let tolerance = TEMPO_SEGMENT_TOLERANCE / hop_secs;
    let line = |from: usize, to: usize| -> (f64, f64) {
        // Least squares of time against beat index over `from..=to`.
        let count = (to - from + 1) as f64;
        let mean_index = (from + to) as f64 * 0.5;
        let mean_time = beats[from..=to].iter().sum::<f64>() / count;
        let mut numerator = 0.0;
        let mut denominator = 0.0;
        for index in from..=to {
            let delta = index as f64 - mean_index;
            numerator += delta * (beats[index] - mean_time);
            denominator += delta * delta;
        }
        let slope = if denominator > 1e-9 { numerator / denominator } else { 0.0 };
        (slope, mean_time - slope * mean_index)
    };
    let worst = |from: usize, to: usize, slope: f64, intercept: f64| -> f64 {
        (from..=to)
            .map(|index| (beats[index] - (intercept + slope * index as f64)).abs())
            .fold(0.0f64, f64::max)
    };

    let mut segments: Vec<TempoSegment> = Vec::new();
    let mut start = 0usize;
    let mut carry: Option<f64> = None;
    while start + TEMPO_SEGMENT_MIN_BEATS <= beats.len() - 1 {
        let mut end = (start + TEMPO_SEGMENT_MIN_BEATS).min(beats.len() - 1);
        let (mut slope, mut intercept) = line(start, end);
        while end + 1 < beats.len() {
            let (next_slope, next_intercept) = line(start, end + 1);
            if worst(start, end + 1, next_slope, next_intercept) > tolerance {
                break;
            }
            end += 1;
            slope = next_slope;
            intercept = next_intercept;
        }
        // Continuity: a segment begins where the last one ended, so position
        // never jumps — only the rate changes, and only on a beat.
        //
        // Pinning the start is not enough on its own. Moving a segment's
        // start onto the previous segment's end without re-fitting shifts
        // every beat in it by that difference, and those shifts accumulate
        // down the track — measured, it cost a fifth of the F-measure on a
        // drifting fixture. So the start is pinned and the SLOPE is fitted
        // again around it, which is the least-squares line through the
        // segment's beats that also passes through the point it has to.
        let anchor = carry.unwrap_or(intercept + slope * start as f64);
        if carry.is_some() {
            let (mut numerator, mut denominator) = (0.0, 0.0);
            for index in start..=end {
                let step = (index - start) as f64;
                numerator += step * (beats[index] - anchor);
                denominator += step * step;
            }
            if denominator > 1e-9 {
                slope = numerator / denominator;
            }
        }
        segments.push(TempoSegment {
            start_secs: anchor * hop_secs,
            start_beat: start as f64,
            period_secs: slope * hop_secs,
        });
        carry = Some(anchor + slope * (end - start) as f64);
        if end >= beats.len() - 1 {
            break;
        }
        start = end;
    }
    segments
}

/// Build a tempo map for a track whose tempo actually moves — or leave it
/// empty, which is the answer for nearly everything.
fn build_tempo_map(envelopes: &Envelopes, period: f64, offset: f64) -> TempoMap {
    let hop_secs = envelopes.hop as f64 / envelopes.sample_rate;
    let peaks = onset_peaks(&envelopes.onset, period);
    if peaks.len() < 32 {
        return TempoMap::default();
    }
    let activation =
        spike_activation(&peaks, envelopes.onset.len(), (period * 0.06).max(1.0) as usize);
    let decoded = decode_beats(&activation, period, DECODE_TIGHTNESS);
    if decoded.len() < TEMPO_SEGMENT_MIN_BEATS * 2 {
        return TempoMap::default();
    }
    // What the straight line already achieves, over the same beats.
    let fixed: Vec<f64> = {
        let mut out = Vec::with_capacity(decoded.len());
        let mut beat = ((decoded[0] - offset) / period).round();
        while out.len() < decoded.len() {
            out.push(offset + beat * period);
            beat += 1.0;
        }
        out
    };
    let free_support = beat_support(&decoded, &peaks, period);
    let fixed_support = beat_support(&fixed, &peaks, period);
    if free_support < TEMPO_MAP_RATIO * fixed_support {
        return TempoMap::default();
    }
    // The map is allowed to bend the TEMPO. It is not allowed to quietly
    // move the beat onto a different pulse: which pulse the beat sits on is
    // decided once, by the grid, and a decoder that disagrees about it is
    // not describing the same track's drift — it is overruling a decision
    // taken elsewhere, on evidence no better than the evidence that took it.
    // Measured without this guard, one record went from 0.553 against the
    // kicks to 0.000 while its support ratio looked healthy the whole time.
    //
    // The check has to isolate PHASE from tempo, which is fiddlier than it
    // looks: comparing decoded beats against the fixed grid directly also
    // fails whenever the two simply disagree about tempo, which is the very
    // case the map exists to serve — measured, it blocked a 124-to-130 ride
    // outright. So the decoded sequence is compared against a grid at the
    // DECODED tempo, anchored at the published grid's phase. A tempo
    // difference then cancels and only a difference of pulse is left.
    let checked = decoded.len().min(24);
    let local = {
        let mut steps: Vec<f64> = decoded[..checked]
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect();
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        steps.get(steps.len() / 2).copied().unwrap_or(period)
    };
    let mut phases: Vec<f64> = decoded[..checked]
        .iter()
        .map(|at| {
            let phase = ((at - offset) / local).rem_euclid(1.0);
            if phase >= 0.5 {
                phase - 1.0
            } else {
                phase
            }
        })
        .collect();
    phases.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if phases[phases.len() / 2].abs() > 0.15 {
        return TempoMap::default();
    }
    let segments = summarize_beats(&decoded, hop_secs);
    if segments.len() < 2 {
        return TempoMap::default();
    }
    TempoMap { segments }
}

/// The scale that maps a track's own loudness onto the display: one over a
/// high percentile of `values`, rather than over their maximum, so a single
/// clipped transient cannot flatten the whole picture. Zero for silence.
///
/// This is the ONLY normalization the waveform is allowed. It is taken over
/// the whole track, so a column's height means the same thing wherever it
/// sits and whatever else has been computed by the time it is drawn.
fn track_scale(values: impl Iterator<Item = f32>) -> f32 {
    let mut values: Vec<f32> = values.collect();
    if values.is_empty() {
        return 0.0;
    }
    let index =
        ((values.len() as f64 * REFERENCE_PERCENTILE) as usize).min(values.len().saturating_sub(1));
    // A selection, not a sort: this runs again for every chunk of a track
    // still streaming in, over every hop so far.
    let (_, reference, _) = values.select_nth_unstable_by(index, |a, b| {
        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
    });
    let reference = *reference;
    if reference > 1e-6 {
        1.0 / reference
    } else {
        0.0
    }
}

/// How loud one hop is, in linear amplitude: the peak keeps the transients,
/// the broadband RMS keeps the body. Both are linear in level, so halving
/// the audio halves this — which is what makes a quiet intro draw short
/// beside a loud drop instead of being lifted to meet it.
fn hop_level(peak: f32, rms: [f32; 3]) -> f32 {
    let broadband = (rms[0] * rms[0] + rms[1] * rms[1] + rms[2] * rms[2]).sqrt();
    0.5 * peak + 0.5 * broadband
}

/// Build the display tiles from the per-hop envelopes.
fn build_tiles(envelopes: &Envelopes, pcm: &TrackPcm) -> WaveTiles {
    let _ = pcm;
    tiles_from_hops(&envelopes.band_rms, &envelopes.peak)
}

/// The tiles of a track still being decoded: the hops in hand, normalized
/// against themselves, laid over the length the decoder expects so the
/// picture fills in from the left instead of reflowing with every chunk.
/// Columns past the decoded edge are empty. The whole-track analysis
/// replaces this the moment it lands.
pub fn provisional_tiles(hops: &[WaveHop], columns: usize) -> WaveTiles {
    let band_rms: Vec<[f32; 3]> = hops.iter().map(|hop| hop.rms).collect();
    let peak: Vec<f32> = hops.iter().map(|hop| hop.peak).collect();
    let mut tiles = tiles_from_hops(&band_rms, &peak);
    if tiles.zoom.len() < columns {
        tiles.zoom.resize(columns, [0; 4]);
    }
    tiles
}

/// The tiles from per-hop band RMS and peaks — the one place the picture's
/// normalization lives, for the final analysis and the streaming preview
/// alike.
fn tiles_from_hops(band_rms: &[[f32; 3]], peak: &[f32]) -> WaveTiles {
    if peak.is_empty() || band_rms.is_empty() {
        return WaveTiles { zoom: Vec::new(), overview: vec![[0u8; 2]; OVERVIEW_COLS] };
    }
    // Normalize each band by a high percentile so quiet tracks still fill
    // the display, without one clipped transient flattening everything.
    // These are the COLOUR of a column, never its height.
    let mut band_scale = [1.0f32; 3];
    for (band, scale) in band_scale.iter_mut().enumerate() {
        *scale = track_scale(band_rms.iter().map(|rms| rms[band]));
    }
    // The height of a column is its level against the whole track — one
    // scale for the entire file, computed here, applied nowhere else.
    let levels: Vec<f32> = peak
        .iter()
        .zip(band_rms)
        .map(|(peak, rms)| hop_level(*peak, *rms))
        .collect();
    let level_scale = track_scale(levels.iter().copied());
    let zoom = band_rms
        .iter()
        .zip(&levels)
        .map(|(rms, level)| {
            let mut out = [0u8; 4];
            for band in 0..3 {
                // A mild curve: the eye reads energy, not amplitude.
                let value = (rms[band] * band_scale[band]).clamp(0.0, 1.0).powf(WAVE_CURVE);
                out[band] = (value * 255.0) as u8;
            }
            let value = (level * level_scale).clamp(0.0, 1.0).powf(WAVE_CURVE);
            out[3] = (value * 255.0) as u8;
            out
        })
        .collect();

    let mut overview = vec![[0u8; 2]; OVERVIEW_COLS];
    let hops = peak.len().min(band_rms.len()).max(1);
    let peak_scale = track_scale(peak.iter().copied());
    for column in 0..OVERVIEW_COLS {
        let start = column * hops / OVERVIEW_COLS;
        let end = (((column + 1) * hops) / OVERVIEW_COLS).max(start + 1).min(hops);
        let mut hop_peak = 0.0f32;
        let mut energy = 0.0f64;
        for index in start..end {
            hop_peak = hop_peak.max(peak[index]);
            let rms = band_rms[index];
            energy += ((rms[0] * rms[0] + rms[1] * rms[1] + rms[2] * rms[2]) as f64).sqrt();
        }
        let peak = hop_peak;
        let mean = (energy / (end - start).max(1) as f64) as f32;
        overview[column] = [
            ((peak * peak_scale).clamp(0.0, 1.0).powf(WAVE_CURVE) * 255.0) as u8,
            // Loudness for the hot/cold colouring, on a dB-ish curve.
            (((1.0 + 40.0 * mean).ln() / (41.0f32).ln()).clamp(0.0, 1.0) * 255.0) as u8,
        ];
    }
    WaveTiles { zoom, overview }
}

/// Full analysis of one decoded track.
pub fn analyze(pcm: &TrackPcm) -> TrackAnalysis {
    let envelopes = build_envelopes(pcm);
    // Reuse the streaming detector over the whole file for an independent
    // BPM opinion; it only ever breaks an octave tie in the offline pass.
    let prior = streaming_prior(pcm);
    let grid = estimate_grid(&envelopes, prior);
    let tempo_map = if grid.has_grid() {
        let hop_rate = envelopes.sample_rate / envelopes.hop as f64;
        build_tempo_map(
            &envelopes,
            grid.beat_secs * hop_rate,
            grid.first_beat_secs * hop_rate - HOP_CENTRE,
        )
    } else {
        TempoMap::default()
    };
    let tiles = build_tiles(&envelopes, pcm);
    // The phrase map: the same change points the grid estimator consumes,
    // published in seconds instead of being thrown away with the hops.
    let hop_secs = envelopes.hop as f64 / envelopes.sample_rate;
    let changes_secs = structural_changes(&envelopes)
        .into_iter()
        .map(|hop| hop * hop_secs)
        .collect();
    TrackAnalysis {
        duration_secs: pcm.seconds(),
        sample_rate: pcm.sample_rate,
        grid,
        #[cfg(not(test))]
        refined_by_beats: false,
        tempo_map,
        tiles,
        changes_secs,
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

    /// A decoded-audio SHA-256 used by the stem/lyrics/analysis bakers.
    pub fn from_digest(digest: &str) -> Result<AnalysisKey, String> {
        if digest.len() != 64
            || !digest
                .as_bytes()
                .iter()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("analysis digest must be 64 lowercase hexadecimal characters".into());
        }
        Ok(AnalysisKey(digest.to_string()))
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
        64 + analysis.tiles.zoom.len() * 4 + analysis.tiles.overview.len() * 2,
    );
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&analysis.duration_secs.to_le_bytes());
    out.extend_from_slice(&analysis.sample_rate.to_le_bytes());
    out.push(u8::from(analysis.refined_by_beats()));
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
    out.extend_from_slice(&(analysis.tempo_map.segments.len() as u32).to_le_bytes());
    for segment in &analysis.tempo_map.segments {
        out.extend_from_slice(&segment.start_secs.to_le_bytes());
        out.extend_from_slice(&segment.start_beat.to_le_bytes());
        out.extend_from_slice(&segment.period_secs.to_le_bytes());
    }
    out.extend_from_slice(&(analysis.changes_secs.len() as u32).to_le_bytes());
    for change in &analysis.changes_secs {
        out.extend_from_slice(&change.to_le_bytes());
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
    if version != 5 && version != CACHE_VERSION {
        return Err(format!("wave cache version {version}"));
    }
    let duration_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let sample_rate = u32::from_le_bytes(take(4)?.try_into().unwrap());
    let refined_by_beats = if version >= 6 {
        match take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err("wave cache refinement flag out of range".into()),
        }
    } else {
        false
    };
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
        let column = take(4)?;
        zoom.push([column[0], column[1], column[2], column[3]]);
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
    let segment_count = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    if segment_count > 100_000 {
        return Err("wave cache tempo map out of range".into());
    }
    let mut segments = Vec::with_capacity(segment_count);
    for _ in 0..segment_count {
        segments.push(TempoSegment {
            start_secs: f64::from_le_bytes(take(8)?.try_into().unwrap()),
            start_beat: f64::from_le_bytes(take(8)?.try_into().unwrap()),
            period_secs: f64::from_le_bytes(take(8)?.try_into().unwrap()),
        });
    }
    let change_count = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    if change_count > 100_000 {
        return Err("wave cache change list out of range".into());
    }
    let mut changes_secs = Vec::with_capacity(change_count);
    for _ in 0..change_count {
        changes_secs.push(f64::from_le_bytes(take(8)?.try_into().unwrap()));
    }
    #[cfg(test)]
    let _ = refined_by_beats;
    Ok(TrackAnalysis {
        duration_secs,
        sample_rate,
        #[cfg(not(test))]
        refined_by_beats,
        grid: TrackGrid {
            bpm,
            beat_secs,
            first_beat_secs,
            downbeat_phase,
            confidence,
        },
        changes_secs,
        tempo_map: TempoMap { segments },
        tiles: WaveTiles { zoom, overview },
    })
}

fn load_cached(dir: &Path, key: &AnalysisKey) -> Option<TrackAnalysis> {
    let bytes = std::fs::read(cache_path(dir, key)).ok()?;
    decode_analysis(&bytes).ok()
}

/// Re-publish an analysis the operator corrected (a flipped beat pulse), so
/// the next load of the same record starts from the corrected grid.
pub fn store_analysis(key: &AnalysisKey, analysis: &TrackAnalysis) {
    store_cached(&cache_dir(), key, analysis);
}

fn store_cached(dir: &Path, key: &AnalysisKey, analysis: &TrackAnalysis) {
    let _ = store_analysis_in(dir, key, analysis);
}

/// Write the native cache format into an explicitly selected cache root.
/// Headless bakers use this entry point so their files are interchangeable
/// with caches produced by the app.
pub fn store_analysis_in(
    dir: &Path,
    key: &AnalysisKey,
    analysis: &TrackAnalysis,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let path = cache_path(dir, key);
    let temporary = path.with_extension("wave.tmp");
    std::fs::write(&temporary, encode_analysis(analysis))
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

/// Downmix deck PCM and band-limited resample it to Beat This!'s 22.05 kHz
/// input rate. The small rational polyphase kernel is the same shape used by
/// the AI hub's audio resampler, kept local so track analysis adds no runtime
/// dependency or intermediate stereo buffers.
fn mono_22k(pcm: &TrackPcm) -> Result<Vec<f32>, String> {
    const OUT_RATE: u32 = 22_050;
    if pcm.sample_rate == 0 {
        return Err("source sample rate is zero".into());
    }
    let mono: Vec<f32> = pcm
        .frames
        .iter()
        .map(|frame| (frame[0] as f32 + frame[1] as f32) * (0.5 / 32768.0))
        .collect();
    if pcm.sample_rate == OUT_RATE || mono.is_empty() {
        return Ok(mono);
    }

    let divisor = gcd_u32(pcm.sample_rate, OUT_RATE);
    let up = (OUT_RATE / divisor) as usize;
    let down = (pcm.sample_rate / divisor) as usize;
    const HALF: i64 = 16;
    let cutoff = 0.5 * 0.92 * (OUT_RATE.min(pcm.sample_rate) as f64 / pcm.sample_rate as f64);
    let mut kernels = Vec::with_capacity(up);
    for phase in 0..up {
        let fraction = phase as f64 / up as f64;
        let mut taps = Vec::with_capacity((2 * HALF) as usize);
        let mut sum = 0.0;
        for tap_index in -HALF + 1..=HALF {
            let distance = tap_index as f64 - fraction;
            let sinc = if distance.abs() <= f64::EPSILON {
                1.0
            } else {
                let angle = std::f64::consts::PI * 2.0 * cutoff * distance;
                angle.sin() / angle
            };
            let window_position = (distance + HALF as f64) / (2.0 * HALF as f64);
            let window = if (0.0..=1.0).contains(&window_position) {
                0.42 - 0.5 * (2.0 * std::f64::consts::PI * window_position).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * window_position).cos()
            } else {
                0.0
            };
            let tap = 2.0 * cutoff * sinc * window;
            sum += tap;
            taps.push(tap);
        }
        for tap in &mut taps {
            *tap /= sum;
        }
        kernels.push(taps);
    }

    let output_len = mono.len() * up / down;
    let mut output = Vec::with_capacity(output_len);
    for output_index in 0..output_len {
        let numerator = output_index * down;
        let input_base = (numerator / up) as i64;
        let taps = &kernels[numerator % up];
        let mut sample = 0.0;
        for (tap, offset) in taps.iter().zip(-HALF + 1..=HALF) {
            let input_index = input_base + offset;
            if input_index >= 0 && (input_index as usize) < mono.len() {
                sample += mono[input_index as usize] as f64 * tap;
            }
        }
        output.push(sample as f32);
    }
    Ok(output)
}

fn gcd_u32(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

// ---------------------------------------------------------------------------
// worker pool
// ---------------------------------------------------------------------------

pub struct AnalysisJob {
    pub deck: DeckId,
    pub gen: u64,
    pub key: AnalysisKey,
    pub pcm: Arc<TrackPcm>,
    pub beats_model: Option<PathBuf>,
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
    jobs: Option<Receiver<AnalysisJob>>,
    done_tx: Sender<AnalysisDone>,
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
        AnalysisPool { tx, jobs: Some(jobs), done_tx, rx }
    }

    pub fn start(&mut self, spawner: ThreadSpawner) {
        let Some(jobs) = self.jobs.take() else { return };
        let done_tx = self.done_tx.clone();
        let options = ThreadOptions { name: Some("vj-analysis".into()), ..Default::default() };
        match spawner.spawn_worker(options, move || {
                let dir = cache_dir();
                let mut beats_checkpoint: Option<PathBuf> = None;
                let mut beats_model: Option<BeatsModel> = None;
                let mut beats_model_error: Option<String> = None;
                while let Ok(job) = jobs.recv() {
                    let (mut analysis, cached) = match load_cached(&dir, &job.key) {
                        Some(hit) => (hit, true),
                        None => (analyze(&job.pcm), false),
                    };
                    let mut straight_from_cache = cached;
                    let mut should_store = !cached;
                    if let Some(checkpoint) = job.beats_model.as_ref() {
                        if !analysis.refined_by_beats() {
                            if beats_checkpoint.as_ref() != Some(checkpoint) {
                                beats_checkpoint = Some(checkpoint.clone());
                                beats_model = None;
                                beats_model_error = None;
                                match BeatsModel::load(checkpoint) {
                                    Ok(model) => beats_model = Some(model),
                                    Err(error) => beats_model_error = Some(error.to_string()),
                                }
                            }
                            if let Some(error) = beats_model_error.as_ref() {
                                makepad_widgets::log!(
                                    "beats: kept comb grid; model load failed: {error}"
                                );
                            } else if let Some(model) = beats_model.as_mut() {
                                let started = Instant::now();
                                match mono_22k(&job.pcm) {
                                    Err(error) => makepad_widgets::log!(
                                        "beats: kept comb grid; resample failed: {error}"
                                    ),
                                    Ok(mono) => match model.analyze(&mono) {
                                        Err(error) => makepad_widgets::log!(
                                            "beats: kept comb grid; analysis failed: {error}"
                                        ),
                                        Ok(beats) => match refine_grid_with_beats(
                                            &analysis.grid,
                                            analysis.duration_secs,
                                            &beats.beats_secs,
                                            &beats.downbeats_secs,
                                        ) {
                                            None => makepad_widgets::log!(
                                                "beats: kept comb grid; refinement rejected ({} beats, {} downbeats)",
                                                beats.beats_secs.len(),
                                                beats.downbeats_secs.len(),
                                            ),
                                            Some(refined) => {
                                                let previous = analysis.grid;
                                                analysis.grid = refined;
                                                analysis.mark_refined_by_beats();
                                                straight_from_cache = false;
                                                should_store = true;
                                                makepad_widgets::log!(
                                                    "beats: {:.2} → {:.2} bpm, phase {} → {}, {} beats {} downbeats, {} ms",
                                                    previous.bpm,
                                                    refined.bpm,
                                                    previous.downbeat_phase,
                                                    refined.downbeat_phase,
                                                    beats.beats_secs.len(),
                                                    beats.downbeats_secs.len(),
                                                    started.elapsed().as_millis(),
                                                );
                                            }
                                        },
                                    },
                                }
                            }
                        }
                    }
                    if should_store {
                        store_cached(&dir, &job.key, &analysis);
                    }
                    if done_tx
                        .send(AnalysisDone {
                            deck: job.deck,
                            gen: job.gen,
                            analysis: Arc::new(analysis),
                            cached: straight_from_cache,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }) {
            Ok(handle) => handle.detach(),
            Err(error) => makepad_widgets::log!("vj analysis worker unavailable: {error}"),
        }
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
pub const LOCAL_AUDIO_EXTENSIONS: [&str; 9] =
    ["wav", "mp3", "ogg", "oga", "m4a", "aac", "flac", "aiff", "mp4"];

/// Decode a local audio file. WAV, MP3 and Ogg Vorbis parse in-process
/// (`makepad-audio-decode`); everything else goes through the platform media
/// decoder that already backs the video lane.
pub fn decode_audio_file(path: &Path) -> Result<TrackPcm, String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let media = match extension.as_str() {
        "wav" => MediaType::Wav,
        "ogg" | "oga" => MediaType::Ogg,
        "mp3" => MediaType::Mp3,
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

    fn synthetic_beats(bpm: f64, first: f64, count: usize) -> Vec<f64> {
        let period = 60.0 / bpm;
        (0..count).map(|index| first + index as f64 * period).collect()
    }

    fn synthetic_downbeats(beats: &[f64], stride: usize) -> Vec<f64> {
        beats.iter().step_by(stride).copied().collect()
    }

    fn synthetic_grid(bpm: f64, first: f64, phase: u32) -> TrackGrid {
        TrackGrid {
            bpm,
            beat_secs: 60.0 / bpm,
            first_beat_secs: first,
            downbeat_phase: phase,
            confidence: 0.25,
        }
    }

    #[test]
    fn beats_refinement_fits_an_exact_grid() {
        let beats = synthetic_beats(120.0, 0.2, 96);
        let refined = refine_grid_with_beats(
            &synthetic_grid(120.0, 0.45, 2),
            50.0,
            &beats,
            &synthetic_downbeats(&beats, 4),
        )
        .expect("exact model grid");
        assert!((refined.bpm - 120.0).abs() < 1e-9);
        assert!((refined.first_beat_secs - 0.2).abs() < 1e-9);
        assert_eq!(refined.downbeat_phase, 0);
        assert_eq!(refined.confidence, 0.6);
    }

    #[test]
    fn beats_refinement_tolerates_fifteen_ms_jitter() {
        let mut beats = synthetic_beats(126.0, 0.17, 100);
        for (index, beat) in beats.iter_mut().enumerate() {
            *beat += match index % 3 {
                0 => -0.015,
                1 => 0.0,
                _ => 0.015,
            };
        }
        let downbeats = synthetic_downbeats(&beats, 4);
        let refined = refine_grid_with_beats(
            &synthetic_grid(126.0, 0.4, 3),
            50.0,
            &beats,
            &downbeats,
        )
        .expect("jittered model grid");
        assert!((refined.bpm - 126.0).abs() < 0.02, "{refined:?}");
        assert!(refined.first_beat_secs < 0.20, "{refined:?}");
        assert_eq!(refined.downbeat_phase, 0);
        assert_eq!(refined.confidence, 0.6);
    }

    #[test]
    fn beats_refinement_removes_five_percent_outliers() {
        let clean = synthetic_beats(124.0, 0.11, 100);
        let mut beats = clean.clone();
        for index in [9usize, 29, 49, 69, 89] {
            beats[index] += 0.31;
        }
        let refined = refine_grid_with_beats(
            &synthetic_grid(124.0, 0.3, 1),
            50.0,
            &beats,
            &synthetic_downbeats(&clean, 4),
        )
        .expect("model grid with outliers");
        assert!((refined.bpm - 124.0).abs() < 1e-6, "{refined:?}");
        assert!((refined.first_beat_secs - 0.11).abs() < 1e-6, "{refined:?}");
        assert_eq!(refined.downbeat_phase, 0);
    }

    #[test]
    fn beats_refinement_corrects_a_half_beat_shifted_comb_pulse() {
        let beats = synthetic_beats(120.0, 0.13, 96);
        let refined = refine_grid_with_beats(
            &synthetic_grid(120.0, 0.38, 3),
            50.0,
            &beats,
            &synthetic_downbeats(&beats, 4),
        )
        .expect("half-beat correction");
        assert!((refined.first_beat_secs - 0.13).abs() < 1e-9, "{refined:?}");
        assert_eq!(refined.downbeat_phase, 0);
    }

    #[test]
    fn beats_refinement_keeps_comb_tempo_for_double_time_model() {
        let beats = synthetic_beats(240.0, 0.19, 160);
        let refined = refine_grid_with_beats(
            &synthetic_grid(120.0, 0.44, 2),
            41.0,
            &beats,
            &synthetic_downbeats(&beats, 8),
        )
        .expect("double-time model grid");
        assert!((refined.bpm - 120.0).abs() < 1e-9, "{refined:?}");
        assert!((refined.first_beat_secs - 0.19).abs() < 1e-9, "{refined:?}");
        assert_eq!(refined.downbeat_phase, 0);
    }

    /// End-to-end deck load over a real file on this machine, which is the
    /// only way to exercise the compressed formats without committing audio:
    ///
    /// ```text
    /// VJ_AUDIO_SAMPLE=/path/to/track.mp3 cargo test -p makepad-vj --release \
    ///     -- --nocapture local_audio_file
    /// ```
    ///
    /// It decodes through the same path the LOCAL FILES browser uses and then
    /// runs the analysis a deck would run, so a regression in either the
    /// decoder wiring or the PCM shape it hands over shows up here.
    #[test]
    fn local_audio_file_decodes_and_analyses() {
        let Ok(sample) = std::env::var("VJ_AUDIO_SAMPLE") else {
            eprintln!("VJ_AUDIO_SAMPLE not set; skipping the real-file deck load");
            return;
        };
        for path in sample.split(':').filter(|p| !p.is_empty()) {
            let pcm = decode_audio_file(Path::new(path)).expect("deck decode");
            assert!(pcm.sample_rate >= 8_000, "{path}: rate {}", pcm.sample_rate);
            assert!(!pcm.frames.is_empty(), "{path}: no frames");
            let peak = pcm.frames.iter().fold(0i32, |m, f| m.max(f[0].abs() as i32));
            assert!(peak > 1_000, "{path}: peak {peak} is not audio");
            let analysis = analyze(&pcm);
            let seconds = pcm.frames.len() as f64 / pcm.sample_rate as f64;
            eprintln!(
                "{path}: {seconds:.1}s {} Hz, {:.1} BPM, {} overview columns",
                pcm.sample_rate,
                analysis.grid.bpm,
                analysis.tiles.overview.len(),
            );
            assert!(analysis.grid.bpm > 40.0 && analysis.grid.bpm < 220.0, "{path}: bpm");
            assert!(!analysis.tiles.overview.is_empty(), "{path}: no waveform");
            assert!(!analysis.tiles.zoom.is_empty(), "{path}: no zoom waveform");
        }
    }

    /// Exactly when a click fixture puts its hits down, in source seconds.
    /// The grid the analysis publishes has to land on THESE, which is a
    /// stronger statement than "the tempo is right".
    fn click_onsets(rate: u32, bpm: f64, seconds: f64, first_beat: f64) -> Vec<f64> {
        let len = (rate as f64 * seconds) as usize;
        let period = 60.0 * rate as f64 / bpm;
        let mut out = Vec::new();
        let mut position = first_beat * rate as f64;
        while (position as usize) < len {
            out.push(position as usize as f64 / rate as f64);
            position += period;
        }
        out
    }

    /// A click track: one short percussive hit per beat, plus a stronger
    /// low-frequency hit on the downbeat of each bar.
    fn click_track(rate: u32, bpm: f64, seconds: f64, first_beat: f64) -> TrackPcm {
        let len = (rate as f64 * seconds) as usize;
        let mut frames = vec![[0i16; 2]; len];
        for (beat, onset) in click_onsets(rate, bpm, seconds, first_beat).iter().enumerate() {
            let start = (onset * rate as f64).round() as usize;
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
        }
        TrackPcm { frames, sample_rate: rate }
    }

    /// A millisecond-resolution onset envelope, built independently of the
    /// analysis (finer hop, longer look-back, no baseline subtraction) so it
    /// can be used as ground truth for where the transients of a real
    /// recording actually are.
    fn reference_onsets(pcm: &TrackPcm) -> (Vec<f32>, f64) {
        let rate = pcm.sample_rate.max(1) as f64;
        let hop = (rate * 0.001).round().max(1.0) as usize;
        let mut low = OnePole::new(BAND_LOW_HZ, rate as f32);
        let mut mid = OnePole::new(BAND_HIGH_HZ, rate as f32);
        let mut energy: Vec<[f32; 3]> = Vec::with_capacity(pcm.frames.len() / hop + 1);
        let mut sums = [0.0f64; 3];
        let mut in_hop = 0usize;
        for frame in &pcm.frames {
            let mono = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
            let low_band = low.process(mono);
            let mid_band = mid.process(mono) - low_band;
            let high_band = mono - low.state - mid_band;
            for (sum, value) in sums.iter_mut().zip([low_band, mid_band, high_band]) {
                *sum += (value as f64) * (value as f64);
            }
            in_hop += 1;
            if in_hop == hop {
                let inverse = 1.0 / in_hop as f64;
                energy.push([
                    (sums[0] * inverse).sqrt() as f32,
                    (sums[1] * inverse).sqrt() as f32,
                    (sums[2] * inverse).sqrt() as f32,
                ]);
                sums = [0.0; 3];
                in_hop = 0;
            }
        }
        // A ten-millisecond look-back, so a one-millisecond hop still sees a
        // whole attack rather than the noise inside one.
        let look = 10usize;
        let mut onset = vec![0.0f32; energy.len()];
        for index in look..energy.len() {
            let mut sum = 0.0f32;
            for band in 0..3 {
                let now = (1.0 + 96.0 * energy[index][band]).ln();
                let before = (1.0 + 96.0 * energy[index - look][band]).ln();
                sum += [1.25f32, 1.0, 0.75][band] * (now - before).max(0.0);
            }
            onset[index] = sum;
        }
        (onset, hop as f64 / rate)
    }

    /// Median distance from every ruling of `grid` to the strongest
    /// transient in the half-beat around it, and the spread of those
    /// distances, in milliseconds. A grid that is merely at the right TEMPO
    /// but drifting shows up as a large spread; one that is offset shows up
    /// in the median.
    fn grid_vs_transients(pcm: &TrackPcm, grid: &TrackGrid) -> (f64, f64, usize) {
        let (onset, step) = reference_onsets(pcm);
        let duration = pcm.frames.len() as f64 / pcm.sample_rate.max(1) as f64;
        let mut offsets: Vec<f64> = Vec::new();
        let mut beat = grid.beat_at(0.0).ceil() as i64;
        while grid.secs_at_beat(beat as f64) < duration {
            let at = grid.secs_at_beat(beat as f64);
            let half = grid.beat_secs * 0.25;
            let from = ((at - half) / step).round().max(0.0) as usize;
            let to = ((at + half) / step).round().max(0.0) as usize;
            beat += 1;
            if to >= onset.len() {
                break;
            }
            let mut best = from;
            for index in from..=to {
                if onset[index] > onset[best] {
                    best = index;
                }
            }
            if onset[best] > 0.0 {
                offsets.push((best as f64 * step - at) * 1_000.0);
            }
        }
        if offsets.is_empty() {
            return (0.0, 0.0, 0);
        }
        let mut sorted = offsets.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[sorted.len() / 2];
        let mut spread: Vec<f64> = offsets.iter().map(|v| (v - median).abs()).collect();
        spread.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        (median, spread[spread.len() / 2], offsets.len())
    }

    /// Worst and median distance from a click to the nearest ruling of
    /// `grid`, in milliseconds.
    fn grid_error_ms(grid: &TrackGrid, onsets: &[f64]) -> (f64, f64) {
        let mut errors: Vec<f64> = onsets
            .iter()
            .map(|onset| {
                let nearest = grid.secs_at_beat(grid.beat_at(*onset).round());
                (nearest - onset).abs() * 1_000.0
            })
            .collect();
        errors.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let worst = errors.last().copied().unwrap_or(0.0);
        (worst, errors[errors.len() / 2])
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

    /// The grid has to land on the hits, not merely count them at the right
    /// rate. A tempo-only check passes with the whole grid a hop early, and
    /// passes just as happily when a period a thousandth of a hop out walks
    /// the rulings off the transients over the length of a track — which is
    /// what the beat lines did on real music. So: every click of a long
    /// fixture, at several tempi, sample rates and starting phases.
    #[test]
    fn every_ruling_lands_on_its_click() {
        for &(rate, bpm, first) in &[
            (48_000u32, 120.0f64, 0.35f64),
            (48_000, 120.0, 0.0),
            (44_100, 128.0, 0.123),
            (44_100, 128.0, 0.257),
            (48_000, 96.0, 0.72),
            (44_100, 140.0, 0.399),
        ] {
            let seconds = 180.0;
            let pcm = click_track(rate, bpm, seconds, first);
            let grid = analyze(&pcm).grid;
            assert!(
                (grid.bpm - bpm).abs() < 0.02,
                "rate {rate} bpm {bpm} first {first}: measured {:.4}",
                grid.bpm
            );
            let onsets = click_onsets(rate, bpm, seconds, first);
            let (worst, median) = grid_error_ms(&grid, &onsets);
            assert!(
                worst < 10.0,
                "rate {rate} bpm {bpm} first {first}: worst click is {worst:.1} ms off the \
                 grid (median {median:.1} ms) over {} beats — {grid:?}",
                onsets.len()
            );
        }
    }

    /// The same statement over real recordings, which is where the drift
    /// actually showed. Opt in with a colon-separated list of tracks:
    ///
    /// ```text
    /// VJ_AUDIO_SAMPLE=/a.mp3:/b.mp3 cargo test -p makepad-vj --release \
    ///     -- --nocapture the_grid_sits_on_the_transients
    /// ```
    ///
    /// When a track has separated `stems/drums.wav` beside it the drums are
    /// the reference; otherwise the full mix is, which for four-to-the-floor
    /// material is the same transients.
    #[test]
    fn the_grid_sits_on_the_transients_of_a_real_track() {
        let Ok(sample) = std::env::var("VJ_AUDIO_SAMPLE") else {
            eprintln!("VJ_AUDIO_SAMPLE not set; skipping the real-track beat grid");
            return;
        };
        for path in sample.split(':').filter(|p| !p.is_empty()) {
            let path = Path::new(path);
            let pcm = decode_audio_file(path).expect("deck decode");
            let grid = analyze(&pcm).grid;
            let drums = path.parent().map(|dir| dir.join("stems/drums.wav"));
            let reference = drums
                .filter(|drums| drums.is_file())
                .and_then(|drums| decode_audio_file(&drums).ok());
            let against = reference.as_ref().unwrap_or(&pcm);
            let (median, spread, beats) = grid_vs_transients(against, &grid);
            eprintln!(
                "{}: {:.2} BPM, first beat {:.4}s, {beats} beats vs {} — median {median:+.1} ms, \
                 spread {spread:.1} ms",
                path.file_name().unwrap_or_default().to_string_lossy(),
                grid.bpm,
                grid.first_beat_secs,
                if reference.is_some() { "the drum stem" } else { "the mix" },
            );
            assert!(
                median.abs() < 15.0,
                "{}: the grid sits {median:+.1} ms off the transients",
                path.display()
            );
        }
    }

    /// The same fixture cut in half must produce the same grid: a period
    /// fitted to the first half and one fitted to the whole track only agree
    /// when the period is right, so this is the drift check on its own.
    #[test]
    fn the_grid_does_not_drift_across_a_long_track() {
        let short = analyze(&click_track(44_100, 128.0, 60.0, 0.41)).grid;
        let long = analyze(&click_track(44_100, 128.0, 300.0, 0.41)).grid;
        assert!(
            (short.bpm - long.bpm).abs() < 0.01,
            "sixty seconds says {:.4} BPM, five minutes says {:.4}",
            short.bpm,
            long.bpm
        );
        // Five minutes in, the two grids must still name the same beat.
        let drift = (short.secs_at_beat(short.beat_at(280.0).round())
            - long.secs_at_beat(long.beat_at(280.0).round()))
        .abs();
        assert!(drift < 0.010, "the two grids are {:.1} ms apart at 280 s", drift * 1e3);
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

    /// Four-to-the-floor, where the kick rule is blind by construction: the
    /// same kick on all four beats of every bar, so no bar position carries
    /// more low end than any other. What DOES say where the bar starts is
    /// the arrangement — a hat layer that switches on and off every eight
    /// bars — and it is deliberately put on a bar whose first beat is beat 2
    /// of the fixture, so the answer is not the default.
    #[test]
    fn the_downbeat_comes_from_the_arrangement_when_every_beat_has_a_kick() {
        let rate = 44_100u32;
        let bpm = 128.0f64;
        let seconds = 200.0;
        let period = 60.0 / bpm;
        let len = (rate as f64 * seconds) as usize;
        let mut frames = vec![[0i16; 2]; len];
        // The arrangement changes every 32 beats, starting at beat 2.
        let change_beat = |beat: usize| beat >= 2 && (beat - 2) % 32 == 0;
        let mut hats_on = false;
        let mut beat = 0usize;
        let mut changes: Vec<f64> = Vec::new();
        loop {
            let at = beat as f64 * period + 0.2;
            if at >= seconds {
                break;
            }
            if change_beat(beat) {
                hats_on = !hats_on;
                changes.push(at);
            }
            let mut put = |offset: f64, gain: f64, hz: f64, decay: f64| {
                let start = ((at + offset) * rate as f64) as usize;
                for index in 0..(rate as f64 * 0.07) as usize {
                    if start + index >= len {
                        break;
                    }
                    let time = index as f64 / rate as f64;
                    let value = gain
                        * (-decay * time).exp()
                        * (2.0 * std::f64::consts::PI * hz * time).sin();
                    let sample = (value * 18_000.0) as i16;
                    frames[start + index] = [
                        frames[start + index][0].saturating_add(sample),
                        frames[start + index][1].saturating_add(sample),
                    ];
                }
            };
            // The identical kick, every beat.
            put(0.0, 1.0, 55.0, 38.0);
            if hats_on {
                put(period * 0.5, 0.5, 7_000.0, 220.0);
                put(period * 0.25, 0.3, 7_000.0, 220.0);
            }
            beat += 1;
        }
        let pcm = TrackPcm { frames, sample_rate: rate };
        let grid = analyze(&pcm).grid;
        assert!((grid.bpm - bpm).abs() < 0.5, "bpm {:.2}", grid.bpm);
        assert!(changes.len() >= 8, "{} arrangement changes", changes.len());
        let on_the_one = changes
            .iter()
            .filter(|at| grid.is_downbeat(grid.beat_at(**at).round() as i64))
            .count();
        assert!(
            on_the_one * 2 > changes.len(),
            "only {on_the_one} of {} arrangement changes land on a downbeat \
             (phase {})",
            changes.len(),
            grid.downbeat_phase,
        );
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
        assert!(!back.refined_by_beats());
        assert_eq!(back.tiles, analysis.tiles);
        assert_eq!(back.sample_rate, analysis.sample_rate);
        assert!((back.duration_secs - analysis.duration_secs).abs() < 1e-9);
        assert_eq!(back.changes_secs, analysis.changes_secs);
        // A non-empty change list survives the trip even when the fixture's
        // own detection came back empty.
        let mut phrased = analysis.clone();
        phrased.changes_secs = vec![8.0, 24.5, 40.0];
        let back = decode_analysis(&encode_analysis(&phrased)).expect("decode");
        assert_eq!(back.changes_secs, vec![8.0, 24.5, 40.0]);
        // Truncation and junk are refused, not misread.
        assert!(decode_analysis(&bytes[..bytes.len() / 2]).is_err());
        assert!(decode_analysis(b"nope").is_err());
        // Version 5 had every field except the refinement marker. It remains
        // reusable, but must run Beat This! once when weights are available.
        let mut version_five = encode_analysis(&analysis);
        version_five[8..12].copy_from_slice(&5u32.to_le_bytes());
        version_five.remove(24);
        let old = decode_analysis(&version_five).expect("version 5 decode");
        assert!(!old.refined_by_beats());
        // Still older layouts are re-analysed, never misread.
        let mut old = encode_analysis(&analysis);
        old[8..12].copy_from_slice(&4u32.to_le_bytes());
        assert!(decode_analysis(&old).is_err());
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

    // -----------------------------------------------------------------
    // snapping: whole-unit translation that preserves the reference phase
    // -----------------------------------------------------------------

    fn snap_grid(bpm: f64, first_beat_secs: f64, downbeat_phase: u32) -> TrackGrid {
        TrackGrid {
            bpm,
            beat_secs: 60.0 / bpm,
            first_beat_secs,
            downbeat_phase,
            confidence: 0.9,
        }
    }

    #[test]
    fn snap_keeps_the_references_offset_into_the_unit() {
        // 120 BPM: a beat is 0.5 s, first beat at 0.25 s.
        let g = snap_grid(120.0, 0.25, 0);
        // The reference sits 0.2 s into its beat (40% of the way).
        let reference = 0.25 + 3.0 * 0.5 + 0.2;
        // Aim somewhere with a completely different phase.
        let landed = g.snap_translate(0.25 + 20.0 * 0.5 + 0.37, reference, 1);
        let phase_ref = g.beat_at(reference).rem_euclid(1.0);
        let phase_landed = g.beat_at(landed).rem_euclid(1.0);
        assert!(
            (phase_ref - phase_landed).abs() < 1e-9,
            "offset into the beat must survive: {phase_ref} vs {phase_landed}"
        );
        // And the move must be a WHOLE number of beats from the reference.
        let steps = g.beat_at(landed) - g.beat_at(reference);
        assert!((steps - steps.round()).abs() < 1e-9, "moved {steps} beats");
    }

    #[test]
    fn snap_moves_in_whole_units_for_every_size() {
        let g = snap_grid(128.0, 0.1, 2);
        let reference = 12.345;
        for unit in [1u32, 2, 4, 8, 16] {
            let landed = g.snap_translate(60.0, reference, unit);
            let steps = (g.beat_at(landed) - g.beat_at(reference)) / unit as f64;
            assert!(
                (steps - steps.round()).abs() < 1e-9,
                "unit {unit}: moved {steps} units, which is not whole"
            );
            // The landing is the nearest such step to the target, so never
            // more than half a unit from where the finger asked for.
            let half = unit as f64 * g.beat_secs * 0.5;
            assert!((landed - 60.0).abs() <= half + 1e-9, "unit {unit}: {landed}");
        }
    }

    #[test]
    fn snap_is_blind_to_the_downbeat() {
        // The whole reason there is no bar special case: a relative
        // translation cancels downbeat_phase, so all four phases agree.
        let reference = 9.1;
        let target = 41.7;
        let first = snap_grid(120.0, 0.25, 0).snap_translate(target, reference, 4);
        for phase in [1u32, 2, 3] {
            let other = snap_grid(120.0, 0.25, phase).snap_translate(target, reference, 4);
            assert!((first - other).abs() < 1e-9, "phase {phase}: {first} vs {other}");
        }
    }

    #[test]
    fn snap_off_and_gridless_pass_straight_through() {
        let g = snap_grid(120.0, 0.25, 0);
        assert_eq!(g.snap_translate(33.7, 9.1, 0), 33.7, "unit 0 is off");
        let none = TrackGrid::default();
        assert!(!none.has_grid());
        assert_eq!(none.snap_translate(33.7, 9.1, 4), 33.7, "no grid, no snap");
    }

    #[test]
    fn snap_leaves_an_already_in_phase_target_alone() {
        let g = snap_grid(120.0, 0.25, 0);
        let reference = 0.25 + 3.0 * 0.5 + 0.2;
        let target = reference + 8.0 * 0.5; // exactly 8 beats later
        assert!((g.snap_translate(target, reference, 4) - target).abs() < 1e-9);
    }

    #[test]
    fn snap_steps_forward_when_the_landing_falls_before_zero() {
        let g = snap_grid(120.0, 0.25, 0);
        // Reference near the top of the track, target dragged off the front.
        let landed = g.snap_translate(-30.0, 40.3, 4);
        assert!(landed >= 0.0, "a landing before zero is not a position: {landed}");
        // Still a whole number of units from the reference.
        let steps = (g.beat_at(landed) - g.beat_at(40.3)) / 4.0;
        assert!((steps - steps.round()).abs() < 1e-9, "moved {steps} units");
    }
}

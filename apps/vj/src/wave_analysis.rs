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
use crate::track_key::KeyEstimate;
use makepad_asset_data::BlobId;
use std::f32::consts::PI;
use std::path::{Path, PathBuf};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
/// The level a sample has to reach to count as sound: -60 dB of full
/// scale. ABSOLUTE on purpose, where every other measure in this module is
/// relative — the question is where the file starts making a noise, not
/// where it gets loud compared with itself.
const SOUND_FLOOR: f32 = 0.001;
/// Cache format magic + version. Version 2 is the least-squares beat grid:
/// version 1 sidecars carry a grid that drifts off the transients, so they
/// are re-analysed rather than reused. Version 3 adds the level channel to
/// every zoom column; version 2 sidecars have no absolute loudness in them
/// at all, so they are re-analysed too.
const CACHE_MAGIC: &[u8; 8] = b"VJWAVE\0\0";
/// Version 4 carries the tempo map; a version 3 sidecar has no record of
/// whether the track's tempo moves, so it is re-analysed rather than reused.
/// Version 6 was claimed twice: once for the musical key, once for whether a
/// neural beat pass refined the comb grid. Version 7 carries both. A version
/// 5 sidecar was written before the chroma pass existed, so it has no key in
/// it at all — and an absent key is indistinguishable from "this track has no
/// tonal centre" once it is on disk; a version 6 sidecar carries only one of
/// the two fields, under either layout. All of them are re-analysed rather
/// than reused, as every earlier bump did.
///
/// Version 8 is the same FIELDS with a different chroma behind them: the fold
/// was made pitch-class neutral, the bass harmonics that were reading a minor
/// triad as its relative major are subtracted, and noise now returns no key
/// instead of a confident wrong one. A version 7 key is not wrong-format, it
/// is wrong — which is worse, because nothing about it looks stale.
///
/// Version 9 carries the first and last sounding sample. A version 8
/// sidecar has no record of them, and "no span" is the honest answer for a
/// file that never makes a sound — indistinguishable from "nobody looked" —
/// so it is re-analysed rather than reused.
/// Version 10 pulls the estimated tempo onto a musical value inside a
/// phase budget, so a grid written by 9 was measured under a different
/// rule and is re-analysed rather than reused. The pull is deliberately
/// not reproducible from the stored grid -- it is not idempotent, and a
/// second application can reach a coarser rung the first was too far
/// from -- so a change to the ladder is a version bump, which is exactly
/// what this number is for.
/// Version 14 says whether the record was measured whole or only its
/// first minute. A version 13 sidecar was measured whole, and reading it
/// as partial would re-measure the library for nothing -- so, like every
/// other bump, it is re-analysed rather than guessed about.
///
/// Version 13 carries a version PER PRODUCT, so a change to one detector
/// no longer throws away the others' work. The format version still
/// guards the bytes -- a layout change re-analyses everything, because
/// nothing can be read -- and the product versions guard the content.
///
/// Version 12 says whether the grid and key were read off the separated
/// stems rather than the mix. A version 11 sidecar was measured on the
/// mix, and there is no way to tell from the bytes, so it is re-analysed
/// like every other bump.
///
/// Version 11 carries the record's measured loudness. A version 10
/// sidecar has none, and there is no way to derive one without the
/// samples, so it is re-analysed like every other bump.
const CACHE_VERSION: u32 = 14;

/// How much of a record a fast pass looks at.
///
/// A minute is enough for the columns -- a tempo and a key from the first
/// minute of a dance record are almost always the record's -- and it is
/// not enough to be trusted on a deck, which is why a partial result says
/// so and is measured again the moment a deck asks for it.
pub const FAST_SECS: f64 = 60.0;

/// What each product was measured by, so a change to one detector costs
/// only that product's work.
///
/// The rule for touching these: bump the one whose ANSWER would change.
/// A faster tempo search that lands on the same grid changes nothing and
/// needs no bump; a different onset function does. Bumping too eagerly
/// costs a re-measure, bumping too late leaves stale answers on disk, and
/// of the two the second is the one that misleads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProductVersions {
    /// The comb search, the least-squares fit, the tempo pull, the tempo
    /// map and the downbeat.
    pub grid: u16,
    /// The waveform texture pyramid.
    pub tiles: u16,
    /// The chroma pass and the key profiles.
    pub key: u16,
    /// The first-and-last-sound scan.
    pub sound: u16,
    /// The loudness measurement.
    pub loudness: u16,
}

/// What this build measures with.
const PRODUCTS: ProductVersions =
    ProductVersions { grid: 1, tiles: 1, key: 1, sound: 1, loudness: 1 };

/// Which of a stored record's products were measured by something this
/// build no longer agrees with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stale {
    pub grid: bool,
    pub tiles: bool,
    pub key: bool,
    pub sound: bool,
    pub loudness: bool,
}

impl Stale {
    fn between(stored: ProductVersions, now: ProductVersions) -> Stale {
        Stale {
            grid: stored.grid != now.grid,
            tiles: stored.tiles != now.tiles,
            key: stored.key != now.key,
            sound: stored.sound != now.sound,
            loudness: stored.loudness != now.loudness,
        }
    }

    pub fn any(self) -> bool {
        self.grid || self.tiles || self.key || self.sound || self.loudness
    }

    /// Whether repairing this needs the three-band envelopes rebuilt --
    /// which is most of an analysis, and the reason a grid change is not
    /// cheap while a key change is.
    fn needs_envelopes(self) -> bool {
        self.grid || self.tiles || self.sound
    }
}
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

/// The record's beat, as the audio thread reads it: worked out once per
/// callback per deck and handed to every stage that wants to count.
///
/// A beat here is an OUTPUT length -- source seconds per beat divided by
/// how fast the platter is turning -- so an echo set to a beat stays a
/// beat when the record is pitched up, and a freeze the size of a beat
/// is the size of a beat under a hand. The fraction is where the beat
/// will be when the buffer being rendered ENDS, because that is the
/// moment the next buffer's first sample belongs to.
///
/// Pure arithmetic on `Copy` values: nothing here allocates, locks or can
/// fail, which is the whole contract of the thread that calls it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeckClock {
    /// Output seconds one beat takes at the platter's current speed. Zero
    /// without a grid, and zero when the platter is stopped -- a brake's
    /// bottom, a hand holding the record still -- because a beat that
    /// never arrives has no length. Read it through [`beat_len`], which
    /// refuses to hand that zero to anything that would divide by it.
    ///
    /// [`beat_len`]: Self::beat_len
    pub beat_secs_out: f64,
    /// Where in the beat, `[0, 1)`, the playhead will be at the end of
    /// this buffer. Zero without a grid.
    pub beat_frac_end: f64,
    /// Which beat, counted from the grid's first and fractional, the
    /// playhead will be on at the end of this buffer. Zero without a
    /// grid, and negative before the first beat.
    ///
    /// The fraction above cannot stand in for this. A cycle that spans
    /// several beats -- a sweep an eighth of a cycle per beat is one
    /// sweep every two bars -- has to know WHICH beat it is on, not just
    /// how far into it.
    pub beat_at_end: f64,
    /// Source seconds per output second: the tempo fader's rate normally,
    /// the platter's under a hand or a motor, negative under a reverse
    /// hold, exactly one under a running splat, zero with no record.
    pub platter_rate: f64,
    /// Whether there is a measured grid behind the numbers at all.
    pub has_grid: bool,
}

impl DeckClock {
    /// The clock for a playhead at `pos_secs` turning at `platter_rate`,
    /// about to travel `travel_secs` of source before the buffer ends.
    ///
    /// The caller decides the travel, because only it knows whether the
    /// deck will read this buffer: a paused deck keeps its TEMPO (the
    /// beat is still half a second long) and goes nowhere.
    pub fn at(
        grid: Option<&TrackGrid>,
        pos_secs: f64,
        platter_rate: f64,
        travel_secs: f64,
    ) -> DeckClock {
        let platter_rate = if platter_rate.is_finite() { platter_rate } else { 0.0 };
        let Some(grid) = grid.filter(|grid| grid.has_grid()) else {
            return DeckClock { platter_rate, ..DeckClock::default() };
        };
        let speed = platter_rate.abs();
        let beat_secs_out = if speed > 1e-6 { grid.beat_secs / speed } else { 0.0 };
        let travel = if travel_secs.is_finite() { travel_secs } else { 0.0 };
        DeckClock {
            beat_secs_out,
            beat_frac_end: grid.phase_at(pos_secs + travel),
            beat_at_end: grid.beat_at(pos_secs + travel),
            platter_rate,
            has_grid: true,
        }
    }

    /// One beat in output seconds, or nothing: never the zero that stands
    /// for "no grid" or "stopped", so no stage builds a zero-length delay
    /// out of a record that is not moving.
    pub fn beat_len(&self) -> Option<f64> {
        (self.has_grid && self.beat_secs_out > 0.0).then_some(self.beat_secs_out)
    }
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
    /// Pull `secs` onto the grid's own subdivision: the nearer of the
    /// `parts` slots inside each beat.
    ///
    /// A SEPARATE law from `snap_translate`, and deliberately its opposite.
    /// That one preserves the phase a reference already had inside the
    /// unit -- it is a translation, not a quantise, and the whole tab's
    /// QUANT behaviour rests on it. This one lands on the grid itself,
    /// which is right for a sub-beat loop, because a sub-beat loop IS a
    /// subdivision of the beat and starting it off the subdivision is what
    /// makes a stutter arrive late.
    pub fn snap_to_subdivision(&self, secs: f64, parts: u32) -> f64 {
        if parts == 0 || !self.has_grid() || !secs.is_finite() {
            return secs;
        }
        let slots = parts as f64;
        let landed = self.secs_at_beat((self.beat_at(secs) * slots).round() / slots);
        if landed >= 0.0 {
            landed
        } else {
            secs
        }
    }

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

    /// The same line re-cut to another tempo, hinged where the record is.
    ///
    /// The beat number at `secs` and the fraction of a beat past it come
    /// back exactly, so `beat_at`, `phase_at`, `bar_at` and `is_downbeat`
    /// all answer at that second what they answered before. What changes
    /// is the LENGTH of a beat, which is what a local tempo is.
    ///
    /// The first beat is re-reduced into its own period with the bar phase
    /// moved to match, the same law the tempo pull follows: the published
    /// first beat is the first at or after zero and the phase names the
    /// bar position OF that beat.
    pub fn hinged_at(&self, secs: f64, bpm: f64) -> TrackGrid {
        if !self.has_grid() || !bpm.is_finite() || bpm <= 1.0 {
            return *self;
        }
        let beat_secs = 60.0 / bpm;
        if (beat_secs - self.beat_secs).abs() < 1e-12 {
            return *self;
        }
        let beat = self.beat_at(secs);
        let mut first = secs - beat * beat_secs;
        let mut phase = self.downbeat_phase as i64;
        // Dropping whole periods off the anchor RAISES every beat number
        // by as many, so the bar phase comes down by the same amount and
        // the second that was a downbeat still is one. Counted rather than
        // stepped: a long record and a real tempo change move the anchor
        // by many beats, not by one.
        let steps = (first / beat_secs).floor();
        first -= steps * beat_secs;
        phase -= steps as i64;
        TrackGrid {
            bpm,
            beat_secs,
            first_beat_secs: first,
            downbeat_phase: phase.rem_euclid(4) as u32,
            confidence: self.confidence,
        }
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

    /// The tempo AROUND `secs`: the eight beats centred on it, as one
    /// number.
    ///
    /// The same tempo [`bpm_at`] holds, box-filtered over the window --
    /// and that filtering is the whole point. Segments are at least eight
    /// beats long and usually far longer, so `bpm_at` is piecewise
    /// CONSTANT and steps at a segment edge; a step in a leader's tempo is
    /// a step in a follower's rate, and the room hears it. This is
    /// continuous in `secs`, because the beat coordinate is.
    ///
    /// Only ever a LENGTH, never a beat number: the difference cancels the
    /// map's own numbering, which is the decoder's array index and not the
    /// published grid's counting from its first beat.
    ///
    /// [`bpm_at`]: Self::bpm_at
    pub fn local_bpm(&self, secs: f64) -> Option<f64> {
        if self.is_empty() || !secs.is_finite() {
            return None;
        }
        let beat = self.beat_at(secs);
        let span = self.secs_at_beat(beat + 4.0) - self.secs_at_beat(beat - 4.0);
        (span > 1e-6).then(|| 60.0 * 8.0 / span)
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
/// The stretch of a file that actually makes a sound: the first sample at
/// or above [`SOUND_FLOOR`] and the last. A silent stretch in between never
/// shortens it — the last is refreshed by every sounding sample, so a
/// break in the middle of a track cannot truncate the end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundSpan {
    pub first_secs: f64,
    pub last_secs: f64,
}

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
    /// The track's musical key, when the chroma had enough shape to name
    /// one. `None` is a real answer — too short, too quiet, or no tonal
    /// centre — and reads as an empty cell rather than as a guess.
    pub key: Option<KeyEstimate>,
    /// Where this file's sound begins and ends. `None` when nothing in it
    /// reaches the floor, which is a real answer and the one a wholly
    /// silent import gives.
    ///
    /// A plain field rather than the `cfg(not(test))` the refinement flag
    /// uses: the round-trip test has to set it and read it back, which a
    /// test-invisible field cannot do.
    pub sound: Option<SoundSpan>,
    /// How loud this record is, in LUFS, by the broadcast rule. `None`
    /// when there was nothing to measure -- which is not "quiet" and must
    /// never be turned into a gain.
    pub loudness_lufs: Option<f64>,
    /// The grid and key were read off the SEPARATED stems: the pulse from
    /// the drums alone, where nothing is masking the kick, and the chroma
    /// from everything but the drums, where nothing broadband is voting
    /// for every pitch class at once.
    ///
    /// Kept so the work is done once. It is also the reason a stem answer
    /// is never overwritten by a mix answer: the mix pass runs on every
    /// load, and without this it would undo the better measurement every
    /// time the record came back.
    pub from_stems: bool,
    /// Only the first [`FAST_SECS`] of the record were looked at.
    ///
    /// Good enough to fill a library's columns and never good enough for
    /// a deck: a partial result is measured again in full the moment a
    /// deck asks for the record.
    pub partial: bool,
    /// Which of these products this build would measure differently.
    ///
    /// About the LOAD rather than the record, like the timing beside it,
    /// and never written down: a freshly measured analysis has nothing
    /// stale in it by construction.
    pub stale: Stale,
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

/// Per-hop band envelopes over the whole track.
struct Envelopes {
    /// First and last frame index whose magnitude reaches [`SOUND_FLOOR`].
    /// Sample-exact rather than hop-exact: it rides the same walk over the
    /// samples the envelopes are built from, so it costs no second pass.
    sound: Option<(usize, usize)>,
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
    // The floor in the file's own units. A `let` rather than a const so no
    // float arithmetic sits in a const context.
    let floor_units = (SOUND_FLOOR * 32_768.0).ceil() as u16;
    let mut first_sound: Option<usize> = None;
    let mut last_sound = 0usize;
    for (index, frame) in pcm.frames.iter().enumerate() {
        // Per CHANNEL, not the mono fold below: an anti-phase stereo
        // passage folds to zero there and would read as silence.
        // `unsigned_abs` because `i16::MIN.abs()` panics.
        if frame[0].unsigned_abs().max(frame[1].unsigned_abs()) >= floor_units {
            first_sound.get_or_insert(index);
            last_sound = index;
        }
        let mono = crate::dsp_math::mono(*frame);
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
        sound: first_sound.map(|first| (first, last_sound)),
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
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index =
        ((values.len() as f64 * REFERENCE_PERCENTILE) as usize).min(values.len().saturating_sub(1));
    let reference = values.get(index).copied().unwrap_or(0.0);
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
    // Normalize each band by a high percentile so quiet tracks still fill
    // the display, without one clipped transient flattening everything.
    // These are the COLOUR of a column, never its height.
    let mut band_scale = [1.0f32; 3];
    for (band, scale) in band_scale.iter_mut().enumerate() {
        *scale = track_scale(envelopes.band_rms.iter().map(|rms| rms[band]));
    }
    // The height of a column is its level against the whole track — one
    // scale for the entire file, computed here, applied nowhere else.
    let levels: Vec<f32> = envelopes
        .peak
        .iter()
        .zip(&envelopes.band_rms)
        .map(|(peak, rms)| hop_level(*peak, *rms))
        .collect();
    let level_scale = track_scale(levels.iter().copied());
    let zoom = envelopes
        .band_rms
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
    let hops = envelopes.peak.len().max(1);
    let peak_scale = track_scale(envelopes.peak.iter().copied());
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
            ((peak * peak_scale).clamp(0.0, 1.0).powf(WAVE_CURVE) * 255.0) as u8,
            // Loudness for the hot/cold colouring, on a dB-ish curve.
            (((1.0 + 40.0 * mean).ln() / (41.0f32).ln()).clamp(0.0, 1.0) * 255.0) as u8,
        ];
    }
    let _ = pcm;
    WaveTiles { zoom, overview }
}

/// How far the whole record's grid may be walked to reach a musical
/// tempo, end to end.
///
/// Stated as phase rather than as a percentage, because phase is what a
/// pull costs: moving the tempo from b to c walks the grid by
/// `span * |b - c| / c`, and the far end of the record pays for all of
/// it. Twenty-five milliseconds is about a third of what beat-tracking
/// metrics count as a hit, and a pull costing more than that is not a
/// correction -- it is a different tempo.
///
/// The window this leaves is tight on real material by design: the
/// allowed pull is `0.025 * bpm / span`, which is a tenth of a BPM on a
/// thirty-second fixture at 128 and a hundredth on a five-minute record.
const SNAP_PHASE_BUDGET_SECS: f64 = 0.025;

/// The tempos a record is more likely to have been made at, coarsest
/// first: `(rungs per BPM, low, high)`.
///
/// Rungs per BPM rather than a step size, so the candidate is
/// `round(bpm * per) / per` with no float slop -- 1, 2, 1.5, 3 and 12 are
/// all exact in binary.
///
/// Every rung is a subset of the twelfths, so the ORDER is what makes a
/// whole number win rather than what makes a value reachable: the first
/// rung whose candidate is inside the budget takes it. The two banded
/// rungs cannot both apply, so their order relative to each other decides
/// nothing; the bands are read against the tempo the record was HEARD at,
/// not against the candidate.
const TEMPO_LADDER: [(f64, f64, f64); 5] = [
    (1.0, MIN_BPM, MAX_BPM),  // whole numbers
    (2.0, MIN_BPM, 85.0),     // halves, where a half is a slow record's half
    (1.5, 127.0, MAX_BPM),    // two thirds, where fast records live
    (3.0, MIN_BPM, MAX_BPM),  // thirds
    (12.0, MIN_BPM, MAX_BPM), // twelfths
];

/// Pull a measured tempo onto a musical one, if it is close enough that
/// the whole record's grid barely moves.
///
/// The pivot is the beat nearest the record's centre, so the pull is
/// anchored where the least-squares fit is most trustworthy and no end
/// pays more than the other.
///
/// Applied ONCE per grid, at analysis time. It is NOT idempotent --
/// moving to a nearer rung can bring a coarser one inside the budget --
/// so the rule every caller keeps is that its argument is a grid fresh
/// off `estimate_grid` and never one that has already been pulled. A
/// cached grid is not re-snapped when it is read back; a repair
/// re-measures from the envelopes first and snaps that.
fn snap_tempo(grid: TrackGrid, span_secs: f64) -> TrackGrid {
    if !grid.has_grid() || !span_secs.is_finite() || span_secs <= 0.0 {
        return grid;
    }
    let beats = span_secs / grid.beat_secs;
    let mut winner = None;
    for (per, low, high) in TEMPO_LADDER {
        if grid.bpm < low || grid.bpm > high {
            continue;
        }
        let candidate = (grid.bpm * per).round() / per;
        if !(MIN_BPM..=MAX_BPM).contains(&candidate) {
            continue;
        }
        let walk = beats * (60.0 / candidate - grid.beat_secs).abs();
        if walk <= SNAP_PHASE_BUDGET_SECS {
            winner = Some(candidate);
            break;
        }
    }
    let Some(candidate) = winner else { return grid };
    let index = ((span_secs * 0.5 - grid.first_beat_secs) / grid.beat_secs).round();
    let anchor = grid.first_beat_secs + index * grid.beat_secs;
    let beat_secs = 60.0 / candidate;
    let mut first = anchor - index * beat_secs;
    // `first_beat_secs` is the first beat at or after zero and
    // `downbeat_phase` names the bar phase OF THAT BEAT, so a beat has to
    // be added or dropped with the phase moved to match. A bare
    // `rem_euclid` would land in range and quietly rotate the bar. Each
    // loop runs at most once: the pivot moves `first` by less than the
    // budget.
    let mut phase = grid.downbeat_phase as i64;
    while first < 0.0 {
        first += beat_secs;
        phase += 1;
    }
    while first >= beat_secs {
        first -= beat_secs;
        phase -= 1;
    }
    TrackGrid {
        bpm: candidate,
        beat_secs,
        first_beat_secs: first,
        downbeat_phase: phase.rem_euclid(4) as u32,
        confidence: grid.confidence,
    }
}

/// Full analysis of one decoded track.
/// A grid and a key read off the separated stems.
///
/// Only the two products separation can improve, and neither of the two
/// it cannot: the waveform tiles are of the MIX, which is what the
/// operator sees, and the loudness is of the mix, which is what the room
/// hears.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StemReading {
    pub grid: TrackGrid,
    pub key: Option<KeyEstimate>,
}

/// Re-read the pulse and the chroma from separated lanes.
///
/// The pulse comes off the DRUMS alone, where nothing is masking the
/// kick; the chroma off everything BUT the drums, where a broadband hit
/// is not voting for every pitch class at once. Both are the same
/// estimators the mix goes through -- this changes what they are shown,
/// not how they think.
///
/// `None` when either lane is missing: half a reading is not a reading.
pub fn stem_reading(
    drums: &TrackPcm,
    rest: &TrackPcm,
    span_secs: f64,
) -> Option<StemReading> {
    if drums.frames.is_empty() || rest.frames.is_empty() {
        return None;
    }
    let envelopes = build_envelopes(drums);
    let grid = snap_tempo(estimate_grid(&envelopes, None), span_secs);
    if !grid.has_grid() {
        return None;
    }
    Some(StemReading {
        grid,
        key: crate::track_key::estimate_key(&rest.frames, rest.sample_rate),
    })
}

/// Whether a stem reading is worth taking over the mix's.
///
/// The pulse is: separation is the point, and a grid measured where
/// nothing masks the kick is the better measurement -- unless it is
/// LESS sure of itself, which happens on a record the separator could
/// make nothing of. The key is taken only when the mix had none or the
/// stem reading is at least as sure, for the same reason.
pub fn stem_reading_wins(mix: &TrackGrid, stems: &TrackGrid) -> bool {
    stems.has_grid() && (!mix.has_grid() || stems.confidence >= mix.confidence * 0.9)
}

/// Where one analysis spent its time, in milliseconds.
///
/// Deliberately NOT part of the result and never written to the sidecar:
/// it is about this RUN, and a cached load would otherwise report times
/// it never spent. Five stages rather than a total, because the total
/// alone cannot answer the question anyone actually asks -- a track that
/// took four seconds took them somewhere, and knowing where is the
/// difference between a slow machine and a pathological file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnalysisTiming {
    /// The three-band envelopes, and the sound scan riding them.
    pub envelopes: u64,
    /// The live detector run over the whole file for its second opinion.
    pub prior: u64,
    /// The comb search, the fit, the tempo pull and the tempo map.
    pub grid: u64,
    /// The texture pyramid the waveform is drawn from.
    pub tiles: u64,
    /// The chroma pass, which reads the samples again.
    pub key: u64,
    /// The loudness pass, which reads them a third time.
    pub loudness: u64,
}

impl AnalysisTiming {
    pub fn total(&self) -> u64 {
        self.envelopes + self.prior + self.grid + self.tiles + self.key + self.loudness
    }
}

pub fn analyze(pcm: &TrackPcm) -> TrackAnalysis {
    analyze_timed(pcm, None).0
}

/// Measure again only what this build no longer agrees with.
///
/// The point of versioning the products separately: a change to the key
/// profiles costs the chroma pass and nothing else, where before it threw
/// away the waveform, the tempo map and the loudness too. A grid change
/// is still most of an analysis, because the envelopes it reads are.
pub fn repair(
    analysis: &mut TrackAnalysis,
    pcm: &TrackPcm,
    tag_bpm: Option<f64>,
) -> bool {
    let stale = analysis.stale;
    if !stale.any() {
        return false;
    }
    if stale.needs_envelopes() {
        let envelopes = build_envelopes(pcm);
        if stale.grid {
            let prior = streaming_prior(pcm).or(tag_bpm);
            analysis.grid = snap_tempo(estimate_grid(&envelopes, prior), pcm.seconds());
            analysis.tempo_map = match analysis.grid.has_grid() {
                true => {
                    let hop_rate = envelopes.sample_rate / envelopes.hop as f64;
                    build_tempo_map(
                        &envelopes,
                        analysis.grid.beat_secs * hop_rate,
                        analysis.grid.first_beat_secs * hop_rate - HOP_CENTRE,
                    )
                }
                false => TempoMap::default(),
            };
            let hop_secs = envelopes.hop as f64 / envelopes.sample_rate;
            analysis.changes_secs = structural_changes(&envelopes)
                .into_iter()
                .map(|hop| hop * hop_secs)
                .collect();
            // A grid measured again by the mix is a mix answer, whatever
            // the stems said last time.
            analysis.from_stems = false;
        }
        if stale.tiles {
            analysis.tiles = build_tiles(&envelopes, pcm);
        }
        if stale.sound {
            analysis.sound = envelopes.sound.map(|(first, last)| {
                let rate = pcm.sample_rate.max(1) as f64;
                SoundSpan {
                    first_secs: first as f64 / rate,
                    last_secs: last as f64 / rate,
                }
            });
        }
    }
    if stale.key {
        analysis.key = crate::track_key::estimate_key(&pcm.frames, pcm.sample_rate);
        analysis.from_stems = false;
    }
    if stale.loudness {
        analysis.loudness_lufs =
            crate::loudness::integrated_lufs(&pcm.frames, pcm.sample_rate);
    }
    analysis.stale = Stale::default();
    true
}

/// The same analysis, saying where its time went.
/// The first minute of a record, for a pass whose answer only has to
/// fill a column.
///
/// Marked partial, which is what stops a deck trusting it: the whole
/// point of looking at less is that it is less, and a grid from one
/// minute of a record played by people is not that record's grid.
pub fn analyze_fast(pcm: &TrackPcm, tag_bpm: Option<f64>) -> TrackAnalysis {
    let rate = pcm.sample_rate.max(1) as usize;
    let head = (rate as f64 * FAST_SECS) as usize;
    if pcm.frames.len() <= head {
        // Nothing to save: a record shorter than the window is measured
        // whole, and says so, so nothing re-measures it later for nothing.
        return analyze_timed(pcm, tag_bpm).0;
    }
    let head_pcm = TrackPcm {
        frames: pcm.frames[..head].to_vec(),
        sample_rate: pcm.sample_rate,
    };
    let mut analysis = analyze_timed(&head_pcm, tag_bpm).0;
    // The DURATION is the record's, not the window's: it is read off the
    // file rather than measured, and a column saying every long record is
    // one minute long would be worse than a blank one.
    analysis.duration_secs = pcm.seconds();
    analysis.partial = true;
    analysis
}

/// `tag_bpm` is what the FILE claims its tempo is, when it claims one.
/// It is used for one thing and nothing else: breaking the octave tie,
/// which is the detector's own weakest decision and the one a human
/// already answered when they tagged the record. It never becomes the
/// published tempo -- the record is still measured.
pub fn analyze_timed(
    pcm: &TrackPcm,
    tag_bpm: Option<f64>,
) -> (TrackAnalysis, AnalysisTiming) {
    let mut timing = AnalysisTiming::default();
    let mut clock = std::time::Instant::now();
    let mut lap = |timing: &mut u64, clock: &mut std::time::Instant| {
        *timing = clock.elapsed().as_millis() as u64;
        *clock = std::time::Instant::now();
    };
    let envelopes = build_envelopes(pcm);
    lap(&mut timing.envelopes, &mut clock);
    // Reuse the streaming detector over the whole file for an independent
    // BPM opinion; it only ever breaks an octave tie in the offline pass.
    // The live detector's own lock first: it is a measurement of THIS
    // record, and a tag is somebody's word about it.
    let prior = streaming_prior(pcm).or(tag_bpm);
    lap(&mut timing.prior, &mut clock);
    // Pulled onto a musical tempo before anything downstream reads it:
    // the tempo map is fitted against this period, and a map built on the
    // unpulled one would describe a different record. Here rather than
    // inside `estimate_grid` because the pull has to come after the octave
    // choice, the tempo fence and the phase, and because this is where the
    // record's LENGTH is known -- the budget is a length.
    let grid = snap_tempo(estimate_grid(&envelopes, prior), pcm.seconds());
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
    lap(&mut timing.grid, &mut clock);
    let tiles = build_tiles(&envelopes, pcm);
    lap(&mut timing.tiles, &mut clock);
    // The phrase map: the same change points the grid estimator consumes,
    // published in seconds instead of being thrown away with the hops.
    let hop_secs = envelopes.hop as f64 / envelopes.sample_rate;
    let changes_secs = structural_changes(&envelopes)
        .into_iter()
        .map(|hop| hop * hop_secs)
        .collect();
    // The key runs off the SAMPLES, not off the envelopes above: pitch class
    // is the one thing a three-band energy envelope has already thrown away.
    let key = crate::track_key::estimate_key(&pcm.frames, pcm.sample_rate);
    lap(&mut timing.key, &mut clock);
    let loudness_lufs = crate::loudness::integrated_lufs(&pcm.frames, pcm.sample_rate);
    lap(&mut timing.loudness, &mut clock);
    let analysis = TrackAnalysis {
        duration_secs: pcm.seconds(),
        sample_rate: pcm.sample_rate,
        grid,
        #[cfg(not(test))]
        refined_by_beats: false,
        tempo_map,
        tiles,
        changes_secs,
        key,
        sound: envelopes.sound.map(|(first, last)| {
            let rate = pcm.sample_rate.max(1) as f64;
            SoundSpan {
                first_secs: first as f64 / rate,
                last_secs: last as f64 / rate,
            }
        }),
        loudness_lufs,
        from_stems: false,
        partial: false,
        stale: Stale::default(),
    };
    (analysis, timing)
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
                .map(|frame| crate::dsp_math::mono(*frame)),
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
        AnalysisKey::from_path_with(path, crate::media::container_trim_enabled())
    }

    /// The same, with the container-edit gate folded in: a grid measured
    /// on a sound cut to its edit must not answer for the sound as it
    /// decodes without it, or the beat sits a padding's worth off.
    pub fn from_path_with(path: &Path, container_trim: bool) -> AnalysisKey {
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
        if container_trim {
            feed(b"container-edit");
        }
        AnalysisKey(format!("local-{hash:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Rebuild a key from what [`AnalysisKey::as_str`] handed out. The
    /// preprocessing lane's decode jobs carry the key as a plain string
    /// through the media worker, which knows nothing about analysis.
    pub fn from_raw(raw: String) -> AnalysisKey {
        AnalysisKey(raw)
    }
}

#[cfg(test)]
mod key_tests {
    use super::AnalysisKey;

    /// One file, two ways of decoding it, two records: the gate is part
    /// of the key, so a stale grid never answers for the other decode.
    #[test]
    fn the_container_edit_gate_is_part_of_the_key() {
        let path = std::path::Path::new("some/track.m4a");
        let plain = AnalysisKey::from_path_with(path, false);
        let cut = AnalysisKey::from_path_with(path, true);
        assert_ne!(plain, cut);
        assert_eq!(plain, AnalysisKey::from_path_with(path, false), "and each is stable");
    }
}

/// Where the sidecars live: the operator's chosen cache root when the
/// preprocessing dialog has been given one, otherwise beside the VJ's other
/// local state. `VJ_WAVE_CACHE` still wins over both — it is how a test or a
/// packaging script pins a directory, and that must not be overruled by a
/// preference the machine knows nothing about.
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VJ_WAVE_CACHE") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = crate::preprocess::cache_subdir(crate::preprocess::WAVE_SUBDIR) {
        return dir;
    }
    crate::service::data_root().join("wave-cache")
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
    // The key rides in the FIXED-SIZE header, not after the tiles, so the
    // explorer can read a track's tempo and key out of a sidecar with a
    // 64-byte read instead of paging in megabytes of waveform it will not
    // draw. See `decode_summary`.
    match analysis.key {
        Some(key) => {
            out.push(1);
            out.push(key.tonic);
            out.push(key.minor as u8);
            out.extend_from_slice(&key.confidence.to_le_bytes());
        }
        None => out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]),
    }
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
    // At the END on purpose: the summary header's offsets are hand-indexed
    // and its length is a seek target, and neither wants this field.
    match analysis.sound {
        Some(span) => {
            out.push(1);
            out.extend_from_slice(&span.first_secs.to_le_bytes());
            out.extend_from_slice(&span.last_secs.to_le_bytes());
        }
        None => out.extend_from_slice(&[0u8; 17]),
    }
    // Behind the sound span, for the same reason it is behind everything
    // else: the summary header's offsets are hand-indexed, and a field
    // added in front of them would move every one.
    match analysis.loudness_lufs {
        Some(lufs) => {
            out.push(1);
            out.extend_from_slice(&lufs.to_le_bytes());
        }
        None => out.extend_from_slice(&[0u8; 9]),
    }
    out.push(u8::from(analysis.from_stems));
    out.push(u8::from(analysis.partial));
    for version in [
        PRODUCTS.grid,
        PRODUCTS.tiles,
        PRODUCTS.key,
        PRODUCTS.sound,
        PRODUCTS.loudness,
    ] {
        out.extend_from_slice(&version.to_le_bytes());
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
    let refined_by_beats = match take(1)?[0] {
        0 => false,
        1 => true,
        _ => return Err("wave cache refinement flag out of range".into()),
    };
    let bpm = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let beat_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let first_beat_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let downbeat_phase = u32::from_le_bytes(take(4)?.try_into().unwrap());
    let confidence = f32::from_le_bytes(take(4)?.try_into().unwrap());
    let key = decode_key_field(take(KEY_FIELD_LEN)?);
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
    let sound = {
        let field = take(17)?;
        match field[0] {
            0 => None,
            1 => Some(SoundSpan {
                first_secs: f64::from_le_bytes(field[1..9].try_into().unwrap()),
                last_secs: f64::from_le_bytes(field[9..17].try_into().unwrap()),
            }),
            _ => return Err("wave cache sound flag out of range".into()),
        }
    };
    let loudness_lufs = {
        let field = take(9)?;
        match field[0] {
            0 => None,
            1 => Some(f64::from_le_bytes(field[1..9].try_into().unwrap()))
                .filter(|lufs: &f64| lufs.is_finite()),
            _ => return Err("wave cache loudness flag out of range".into()),
        }
    };
    let from_stems = take(1)?[0] != 0;
    let partial = take(1)?[0] != 0;
    // A sidecar written before the products were versioned reads as all
    // zeroes, which differs from every real version and so re-measures
    // everything -- the honest answer for bytes that never said.
    let stored = {
        let field = take(10)?;
        let word = |at: usize| u16::from_le_bytes(field[at..at + 2].try_into().unwrap());
        ProductVersions {
            grid: word(0),
            tiles: word(2),
            key: word(4),
            sound: word(6),
            loudness: word(8),
        }
    };
    #[cfg(test)]
    let _ = refined_by_beats;
    Ok(TrackAnalysis {
        stale: Stale::between(stored, PRODUCTS),
        partial,
        sound,
        loudness_lufs,
        from_stems,
        duration_secs,
        sample_rate,
        #[cfg(not(test))]
        refined_by_beats,
        key,
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

/// The presence byte, the tonic, the mode and the confidence.
const KEY_FIELD_LEN: usize = 7;
/// Everything before the first variable-length run: magic, version, duration,
/// sample rate, the refinement flag, the five grid fields and the key field. A read of this many
/// bytes answers "what tempo and key is this track" without touching the
/// waveform tiles behind it.
const SUMMARY_LEN: usize = 8 + 4 + 8 + 4 + 1 + 8 + 8 + 8 + 4 + 4 + KEY_FIELD_LEN;

/// A tonic outside the octave is a corrupt sidecar, not a key: drop it rather
/// than hand the wheel an index it cannot spell.
fn decode_key_field(field: &[u8]) -> Option<KeyEstimate> {
    match (field.first()?, field.get(1)?) {
        (1, tonic) if *tonic < 12 => Some(KeyEstimate {
            tonic: *tonic,
            minor: *field.get(2)? != 0,
            confidence: f32::from_le_bytes(field.get(3..7)?.try_into().ok()?),
        }),
        _ => None,
    }
}

/// What the explorer's columns need from a sidecar, and nothing else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackSummary {
    pub duration_secs: f64,
    pub grid: TrackGrid,
    pub key: Option<KeyEstimate>,
}

/// Read just the header of a sidecar. This runs on the UI thread once per
/// track the explorer shows, so it must never pull the tiles in: a long
/// record's zoom channel is megabytes, and the columns want sixty-four
/// bytes of it.
pub fn decode_summary(bytes: &[u8]) -> Option<TrackSummary> {
    if bytes.len() < SUMMARY_LEN || &bytes[0..8] != CACHE_MAGIC {
        return None;
    }
    if u32::from_le_bytes(bytes[8..12].try_into().ok()?) != CACHE_VERSION {
        return None;
    }
    Some(TrackSummary {
        duration_secs: f64::from_le_bytes(bytes[12..20].try_into().ok()?),
        grid: TrackGrid {
            bpm: f64::from_le_bytes(bytes[25..33].try_into().ok()?),
            beat_secs: f64::from_le_bytes(bytes[33..41].try_into().ok()?),
            first_beat_secs: f64::from_le_bytes(bytes[41..49].try_into().ok()?),
            downbeat_phase: u32::from_le_bytes(bytes[49..53].try_into().ok()?),
            confidence: f32::from_le_bytes(bytes[53..57].try_into().ok()?),
        },
        key: decode_key_field(&bytes[57..SUMMARY_LEN]),
    })
}

/// The header of the sidecar for `key`, when one is on disk for this cache
/// version. `None` covers "never analysed", "analysed by an older build" and
/// "unreadable" alike — all of which mean the same thing to a column: blank.
pub fn load_cached_summary(dir: &Path, key: &AnalysisKey) -> Option<TrackSummary> {
    use std::io::Read;
    let mut file = std::fs::File::open(cache_path(dir, key)).ok()?;
    let mut head = [0u8; SUMMARY_LEN];
    file.read_exact(&mut head).ok()?;
    decode_summary(&head)
}

/// The 2048-column overview for `key`, without paging in the waveform.
///
/// A picker ranking a whole pool needs each record's loudness envelope and
/// nothing else. The zoom channel in front of it is a hundred columns a
/// second at four bytes each — about 140 KB for a six-minute record against
/// the overview's four — so reading the whole sidecar to answer "how loud
/// does this run" costs some thirty times what the answer needs. The layout
/// puts a length in front of each run, so the overview sits at an offset
/// that can be computed from two four-byte reads and a seek rather than a
/// whole decode — which is why ranking a library needs no change to the
/// format and no re-analysis of anything.
pub fn load_cached_overview(dir: &Path, key: &AnalysisKey) -> Option<Vec<[u8; 2]>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(cache_path(dir, key)).ok()?;
    let mut head = [0u8; SUMMARY_LEN];
    file.read_exact(&mut head).ok()?;
    decode_summary(&head)?;

    let mut count = [0u8; 4];
    file.read_exact(&mut count).ok()?;
    let zoom_len = u32::from_le_bytes(count) as u64;
    file.seek(SeekFrom::Current(zoom_len.checked_mul(4)? as i64)).ok()?;

    file.read_exact(&mut count).ok()?;
    let overview_len = u32::from_le_bytes(count) as usize;
    // A corrupt length must not ask for a gigabyte: the overview is a
    // fixed size the writer chose, and anything else is not one.
    if overview_len > OVERVIEW_COLS {
        return None;
    }
    let mut bytes = vec![0u8; overview_len * 2];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes.chunks_exact(2).map(|pair| [pair[0], pair[1]]).collect())
}

fn load_cached(dir: &Path, key: &AnalysisKey) -> Option<TrackAnalysis> {
    let bytes = std::fs::read(cache_path(dir, key)).ok()?;
    decode_analysis(&bytes).ok()
}

/// Re-publish an analysis the operator corrected (a flipped beat pulse), so
/// the next load of the same record starts from the corrected grid.
/// Throw away one record's stored analysis, so the next look at it is a
/// fresh one.
///
/// Returns whether there was anything to throw away. Only the derived
/// cache: the operator's marks and their corrected grid live elsewhere
/// on purpose, and a re-scan must not touch them.
pub fn forget_cached(dir: &Path, key: &AnalysisKey) -> bool {
    std::fs::remove_file(cache_path(dir, key)).is_ok()
}

pub fn store_analysis(key: &AnalysisKey, analysis: &TrackAnalysis) {
    store_cached(&cache_dir(), key, analysis);
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
        .map(|frame| crate::dsp_math::mono(*frame))
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

/// A second look at a record whose stems have arrived: the two products
/// separation can improve, and nothing else.
///
/// Its own pool rather than a second kind of job on the analysis one,
/// because the two must not queue behind each other -- a deck waiting to
/// be loaded must never wait for a refinement of a record that is
/// already playing.
pub struct StemGridJob {
    pub deck: DeckId,
    pub gen: u64,
    pub key: AnalysisKey,
    pub drums: Arc<TrackPcm>,
    pub rest: Arc<TrackPcm>,
    pub span_secs: f64,
}

pub struct StemGridDone {
    pub deck: DeckId,
    pub gen: u64,
    pub key: AnalysisKey,
    pub reading: StemReading,
}

/// One worker, one job at a time: a refinement is never urgent, and two
/// of them at once would take cores off the separation that feeds them.
pub struct StemGridPool {
    tx: Sender<StemGridJob>,
    rx: Receiver<StemGridDone>,
    busy: Option<(DeckId, u64)>,
}

impl Default for StemGridPool {
    fn default() -> Self {
        Self::new()
    }
}

impl StemGridPool {
    pub fn new() -> StemGridPool {
        let (tx, jobs) = channel::<StemGridJob>();
        let (done_tx, rx) = channel::<StemGridDone>();
        let _ = std::thread::Builder::new()
            .name("vj-stem-grid".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let started = std::time::Instant::now();
                    let Some(reading) = stem_reading(&job.drums, &job.rest, job.span_secs)
                    else {
                        continue;
                    };
                    makepad_widgets::log!(
                        "stems: grid {:.2} bpm conf {:.2}, key {}, {} ms",
                        reading.grid.bpm,
                        reading.grid.confidence,
                        reading.key.map_or("none".to_string(), |key| key.camelot()),
                        started.elapsed().as_millis(),
                    );
                    if done_tx
                        .send(StemGridDone {
                            deck: job.deck,
                            gen: job.gen,
                            key: job.key,
                            reading,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            });
        StemGridPool { tx, rx, busy: None }
    }

    /// Ask for one, unless this deck's generation is already in flight.
    pub fn submit(&mut self, job: StemGridJob) -> bool {
        if self.busy == Some((job.deck, job.gen)) {
            return false;
        }
        let key = (job.deck, job.gen);
        if self.tx.send(job).is_err() {
            return false;
        }
        self.busy = Some(key);
        true
    }

    pub fn poll(&mut self) -> Option<StemGridDone> {
        let done = self.rx.try_recv().ok()?;
        if self.busy == Some((done.deck, done.gen)) {
            self.busy = None;
        }
        Some(done)
    }
}

pub struct AnalysisJob {
    /// The deck waiting on this, when one is. `None` is the preprocessing
    /// lane: the result is filed against the track and nothing on screen is
    /// waiting for it.
    pub deck: Option<DeckId>,
    pub gen: u64,
    pub key: AnalysisKey,
    pub pcm: Arc<TrackPcm>,
    pub beats_model: Option<PathBuf>,
    /// What the file claims its tempo is, for the octave tie only.
    pub tag_bpm: Option<f64>,
    /// Look at the first minute only. Ignored for a deck's own load: a
    /// deck is where the answer has to be right.
    pub fast: bool,
}

pub struct AnalysisDone {
    /// The deck that asked, when one did; `None` for the preprocessing lane.
    pub deck: Option<DeckId>,
    pub gen: u64,
    /// The key the job was submitted under, handed back so a result can be
    /// filed against the TRACK rather than only against the deck that
    /// happened to ask for it. The explorer's columns read that filing.
    pub key: AnalysisKey,
    pub analysis: Arc<TrackAnalysis>,
    /// True when the result came straight out of the sidecar cache.
    pub cached: bool,
}

/// How the two analysis lanes are staffed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneSchedule {
    /// Workers standing ready for a deck's own load. They never stand
    /// down: a deck load is where the wait is felt, and the model a worker
    /// holds is loaded once and kept.
    pub deck_workers: usize,
    /// The most workers the background pass may have at once. They are
    /// spawned as its jobs queue up and stood down when the lane drains.
    pub batch_workers: usize,
    /// How long an idle pass worker waits for more before it goes.
    pub batch_idle: Duration,
}

impl LaneSchedule {
    /// For a machine with `cores`: one worker per deck, so both can load at
    /// once, and up to half the cores for the pass, capped at four. An
    /// analysis holds the whole decoded record and its own buffers, and a
    /// worker holds its own copy of the beats model, so the pass is sized
    /// by memory rather than by cores.
    pub fn for_cores(cores: usize) -> LaneSchedule {
        LaneSchedule {
            deck_workers: if cores >= 2 { 2 } else { 1 },
            batch_workers: (cores / 2).clamp(1, 4),
            batch_idle: Duration::from_secs(60),
        }
    }

    pub fn for_this_machine() -> LaneSchedule {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        LaneSchedule::for_cores(cores)
    }
}

/// A lock that survives a worker panicking while it held it: the pool has
/// to keep serving the decks whatever one job did.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct LaneState {
    jobs: VecDeque<AnalysisJob>,
    /// Workers on this lane, free or busy.
    alive: usize,
    /// Workers running a job right now. A worker just brought up counts as
    /// free from the moment it exists, so a poll that lands before it has
    /// reached the queue does not bring up another for the same job.
    busy: usize,
    /// The pool is gone: every worker leaves at its next look.
    closed: bool,
    /// The machine refused a thread the last time one was asked for. Said
    /// once in the log, not once a frame.
    spawn_refused: bool,
}

/// One lane of the pool: its queue, and the rules for the workers on it.
struct Lane {
    state: Mutex<LaneState>,
    more: Condvar,
    max_workers: usize,
    /// Workers that stay however long the lane is idle. The deck lane keeps
    /// them all; the pass lane keeps one, with the model it has loaded, so a
    /// pass fed a record a minute does not reload the model every time.
    keep_workers: usize,
    /// How long a worker above `keep_workers` waits for a job before it
    /// leaves.
    idle: Duration,
    /// The pass lane: a job taken here is not started while a deck is
    /// waiting on the other lane.
    yields_to_decks: bool,
}

/// A worker's seat on its lane, given back on any exit -- an idle leave,
/// the pool closing, or a panic in a job -- so the lane can be staffed
/// again. The idle leave gives it back itself, under the lock the decision
/// is made under; the drop is for the exits nobody decided. (A build that
/// aborts on panic never unwinds to here: the process ends with the job.)
struct Seat {
    lane: Arc<Lane>,
    given_back: bool,
    /// Holding a job: counted busy on the lane.
    has_job: bool,
}

impl Drop for Seat {
    fn drop(&mut self) {
        if !self.given_back {
            let mut state = lock(&self.lane.state);
            state.alive = state.alive.saturating_sub(1);
            if self.has_job {
                state.busy = state.busy.saturating_sub(1);
            }
        }
    }
}

impl Lane {
    /// The next job for a worker, or `None` when the worker should leave:
    /// the pool closed, or the lane's idle time passed with nothing queued
    /// and more workers than it keeps. Leaving is decided under the same
    /// lock `submit` pushes under, so a job pushed as a worker leaves finds
    /// either that worker or a fresh one.
    fn next_job(&self, seat: &mut Seat) -> Option<AnalysisJob> {
        let mut state = lock(&self.state);
        // The idle clock starts when the worker first finds nothing to do
        // with more workers on the lane than it keeps, not when it arrives:
        // a kept worker woken for a job another worker took must not leave
        // at once, with its model, over a wait it never had.
        let mut deadline: Option<Instant> = None;
        if seat.has_job {
            state.busy -= 1;
            seat.has_job = false;
        }
        loop {
            if state.closed {
                state.alive -= 1;
                seat.given_back = true;
                return None;
            }
            if let Some(job) = state.jobs.pop_front() {
                state.busy += 1;
                seat.has_job = true;
                return Some(job);
            }
            if state.alive <= self.keep_workers || self.idle == Duration::MAX {
                state = self.more.wait(state).unwrap_or_else(|p| p.into_inner());
                continue;
            }
            let now = Instant::now();
            let deadline = *deadline.get_or_insert_with(|| now.checked_add(self.idle).unwrap_or(now));
            if now >= deadline {
                state.alive -= 1;
                seat.given_back = true;
                return None;
            }
            state = self
                .more
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }
}

/// What the lanes share.
struct PoolShared {
    /// Jobs a deck is waiting on, pending or running, so the pass lane can
    /// hold back while there are any.
    deck_busy: Mutex<usize>,
    deck_idle: Condvar,
    /// The longest the pass waits for the decks before it goes ahead
    /// anyway: a deck job that never came back must not starve the pass
    /// for the rest of the night.
    deck_wait_cap: Duration,
    /// The beats model runs on the show's own graphics device. Its loads
    /// and runs take turns through this, however many workers the lanes
    /// have, so the pass never puts two inferences on the device the
    /// picture is drawn with.
    model_gate: Mutex<()>,
    /// The pool is gone.
    closed: AtomicBool,
    /// Where the sidecars go when not the operator's cache: the tests'.
    cache_root: Option<PathBuf>,
}

/// A deck job in flight, counted from the moment it is taken until the
/// moment its worker is done with it -- finished, refused, or panicked.
struct DeckBusy<'a>(&'a PoolShared);

impl Drop for DeckBusy<'_> {
    fn drop(&mut self) {
        self.0.deck_finished();
    }
}

impl PoolShared {
    fn deck_started(&self) {
        *lock(&self.deck_busy) += 1;
        self.deck_idle.notify_all();
    }

    fn deck_finished(&self) {
        let mut busy = lock(&self.deck_busy);
        *busy = busy.saturating_sub(1);
        if *busy == 0 {
            self.deck_idle.notify_all();
        }
    }

    /// Block while a deck is waiting on the deck lane, up to the cap, or
    /// until the pool closes. True when the decks were idle on return.
    fn wait_deck_idle(&self) -> bool {
        let deadline = Instant::now() + self.deck_wait_cap;
        let mut busy = lock(&self.deck_busy);
        while *busy > 0 {
            if self.closed.load(Ordering::Relaxed) {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            busy = self
                .deck_idle
                .wait_timeout(busy, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        true
    }

    /// A turn at the model. The deck lane takes the next one. The pass lane
    /// takes one only while no deck is waiting: it waits for the decks
    /// first, and a turn it got while a deck arrived behind it is given
    /// back untaken, so a deck's run never queues behind pass workers
    /// already in line. Past the cap on the wait, or with the pool
    /// closing, it goes ahead.
    fn model_turn(&self, yields_to_decks: bool) -> MutexGuard<'_, ()> {
        loop {
            let decks_idle = !yields_to_decks || self.wait_deck_idle();
            let turn = lock(&self.model_gate);
            if !decks_idle
                || !yields_to_decks
                || *lock(&self.deck_busy) == 0
                || self.closed.load(Ordering::Relaxed)
            {
                return turn;
            }
            drop(turn);
            std::thread::yield_now();
        }
    }
}

/// A record a worker could not measure: the job faulted, and the caller
/// has a slot or a deck waiting on an answer that is not coming.
pub struct AnalysisFailed {
    pub deck: Option<DeckId>,
    pub gen: u64,
    pub key: AnalysisKey,
    pub error: String,
}

/// What a panic said, for the log.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return text.to_string();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "a fault with no message".into()
}

/// The analysis workers. Track analysis is seconds of work on a long file
/// and must never touch the UI thread or the audio callback.
///
/// Two lanes: the decks' own loads on one, the background pass over the
/// library on the other. A deck's answer never queues behind the pass, and
/// the pass does not start a job while a deck is waiting for its own.
pub struct AnalysisPool {
    deck: Arc<Lane>,
    batch: Arc<Lane>,
    shared: Arc<PoolShared>,
    done_tx: Sender<AnalysisDone>,
    rx: Receiver<AnalysisDone>,
    failed_tx: Sender<AnalysisFailed>,
    failed_rx: Receiver<AnalysisFailed>,
}

impl Default for AnalysisPool {
    fn default() -> Self {
        AnalysisPool::new()
    }
}

/// One analysis worker: take the lane's jobs until it says to leave.
///
/// A free function rather than a closure so the pool can run more than
/// one of it. The model is loaded per THREAD (and reloaded when the
/// checkpoint changes), so every worker holds its own copy -- which is the
/// price of not making a deck load wait, and is paid only when a model is
/// installed at all.
fn run_analysis_jobs(
    lane: Arc<Lane>,
    shared: Arc<PoolShared>,
    done_tx: Sender<AnalysisDone>,
    failed_tx: Sender<AnalysisFailed>,
) {
            let mut seat = Seat { lane: lane.clone(), given_back: false, has_job: false };
            let mut beats_checkpoint: Option<PathBuf> = None;
            let mut beats_model: Option<BeatsModel> = None;
            let mut beats_model_error: Option<String> = None;
            while let Some(job) = lane.next_job(&mut seat) {
                // Deck first: a job the pass queued is held, not started,
                // while a deck is waiting on the other lane for the CPU it
                // would take.
                if lane.yields_to_decks {
                    shared.wait_deck_idle();
                }
                if shared.closed.load(Ordering::Relaxed) {
                    return;
                }
                let _busy = job.deck.is_some().then(|| DeckBusy(&shared));
                // Per job, not once per thread: the operator can move the
                // cache root mid-session, and a worker holding the old one
                // would keep writing sidecars where nothing reads them.
                let dir = shared.cache_root.clone().unwrap_or_else(cache_dir);
                // One bad record must not cost the lane: a fault in the
                // measurement is caught here, reported, and the worker goes
                // on to the next job.
                let measured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (mut analysis, cached) = match load_cached(&dir, &job.key) {
                    // A partial result is not something a deck may have:
                    // it was measured to fill a column, and this is the
                    // moment the record has to be right.
                    Some(hit) if hit.partial && job.deck.is_some() => {
                        (analyze_timed(&job.pcm, job.tag_bpm).0, false)
                    }
                    // A stored record whose products this build would
                    // measure differently is repaired in place rather than
                    // thrown away: only what changed is measured again,
                    // and it counts as freshly measured from then on.
                    Some(mut hit) => {
                        let repaired = repair(&mut hit, &job.pcm, job.tag_bpm);
                        if repaired {
                            makepad_widgets::log!(
                                "analysis: repaired what this build measures differently"
                            );
                        }
                        (hit, !repaired)
                    }
                    None => {
                        let fast = job.fast && job.deck.is_none();
                        let (analysis, timing) = match fast {
                            true => (analyze_fast(&job.pcm, job.tag_bpm), AnalysisTiming::default()),
                            false => analyze_timed(&job.pcm, job.tag_bpm),
                        };
                        // Where the time went, per stage. A track that
                        // took four seconds took them somewhere, and
                        // the total alone cannot say whether that is a
                        // slow machine or one pathological file.
                        makepad_widgets::log!(
                            "analysis: {:.0}s of audio in {} ms                                  (envelopes {}, prior {}, grid {}, tiles {}, key {}, loudness {})",
                            analysis.duration_secs,
                            timing.total(),
                            timing.envelopes,
                            timing.prior,
                            timing.grid,
                            timing.tiles,
                            timing.key,
                            timing.loudness,
                        );
                        (analysis, false)
                    }
                };
                let mut straight_from_cache = cached;
                let mut should_store = !cached;
                if let Some(checkpoint) = job.beats_model.as_ref() {
                    if !analysis.refined_by_beats() {
                        if beats_checkpoint.as_ref() != Some(checkpoint) {
                            beats_checkpoint = Some(checkpoint.clone());
                            beats_model = None;
                            beats_model_error = None;
                            // A load compiles the graph on the device: a
                            // turn like any run.
                            let _turn = shared.model_turn(lane.yields_to_decks);
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
                                // The resample above is CPU work and takes
                                // no turn; the run on the device does.
                                Ok(mono) => match {
                                    let _turn = shared.model_turn(lane.yields_to_decks);
                                    model.analyze(&mono)
                                } {
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
                // Nothing is written for a pool that has gone.
                if should_store && !shared.closed.load(Ordering::Relaxed) {
                    store_cached(&dir, &job.key, &analysis);
                }
                (analysis, straight_from_cache)
                }));
                let sent = match measured {
                    Ok((analysis, cached)) => done_tx
                        .send(AnalysisDone {
                            deck: job.deck,
                            gen: job.gen,
                            key: job.key,
                            analysis: Arc::new(analysis),
                            cached,
                        })
                        .is_ok(),
                    Err(payload) => {
                        let error = panic_message(payload.as_ref());
                        makepad_widgets::log!("analysis: a record could not be measured: {error}");
                        failed_tx
                            .send(AnalysisFailed { deck: job.deck, gen: job.gen, key: job.key, error })
                            .is_ok()
                    }
                };
                if !sent {
                    return;
                }
            }
}

impl AnalysisPool {
    pub fn new() -> AnalysisPool {
        AnalysisPool::with_schedule(LaneSchedule::for_this_machine())
    }

    /// Two lanes, and the whole reason for two: a deck waiting to be
    /// loaded must never queue behind a background pass over the library.
    /// One queue would make it, and the wait is the length of a whole
    /// analysis -- seconds, with a record on the way in. The deck lane is
    /// staffed now and stays; the pass lane is staffed as its jobs arrive.
    pub fn with_schedule(schedule: LaneSchedule) -> AnalysisPool {
        AnalysisPool::with_schedule_in(schedule, None)
    }

    /// The same, with the sidecars going under `cache_root` rather than
    /// the operator's cache. For the tests, which must not leave records
    /// in a real library.
    pub fn with_schedule_in(schedule: LaneSchedule, cache_root: Option<PathBuf>) -> AnalysisPool {
        let (done_tx, rx) = channel::<AnalysisDone>();
        let (failed_tx, failed_rx) = channel::<AnalysisFailed>();
        let shared = Arc::new(PoolShared {
            deck_busy: Mutex::new(0),
            deck_idle: Condvar::new(),
            deck_wait_cap: Duration::from_secs(30),
            model_gate: Mutex::new(()),
            closed: AtomicBool::new(false),
            cache_root,
        });
        let lane = |max_workers: usize, keep_workers, idle, yields_to_decks| {
            Arc::new(Lane {
                state: Mutex::new(LaneState {
                    jobs: VecDeque::new(),
                    alive: 0,
                    busy: 0,
                    closed: false,
                    spawn_refused: false,
                }),
                more: Condvar::new(),
                max_workers,
                keep_workers,
                idle,
                yields_to_decks,
            })
        };
        let deck_workers = schedule.deck_workers.max(1);
        let deck = lane(deck_workers, deck_workers, Duration::MAX, false);
        let batch = lane(schedule.batch_workers.max(1), 1, schedule.batch_idle, true);
        let pool = AnalysisPool { deck, batch, shared, done_tx, rx, failed_tx, failed_rx };
        {
            let mut state = lock(&pool.deck.state);
            for _ in 0..pool.deck.max_workers {
                pool.spawn_worker(&pool.deck, &mut state);
            }
        }
        pool
    }

    /// One more worker on `lane`, counted under the lane's lock so the
    /// count and the thread agree. A machine that refuses the thread is
    /// logged once; the job waits, and every poll asks again.
    fn spawn_worker(&self, lane: &Arc<Lane>, state: &mut LaneState) {
        let (lane_ref, shared) = (lane.clone(), self.shared.clone());
        let (done_tx, failed_tx) = (self.done_tx.clone(), self.failed_tx.clone());
        let name = match lane.yields_to_decks {
            true => "vj-wave-batch",
            false => "vj-wave-analysis",
        };
        match std::thread::Builder::new()
            .name(name.into())
            .spawn(move || run_analysis_jobs(lane_ref, shared, done_tx, failed_tx))
        {
            Ok(_) => {
                state.alive += 1;
                state.spawn_refused = false;
            }
            Err(error) => {
                if !state.spawn_refused {
                    makepad_widgets::log!("analysis: no thread for the {name} lane: {error}");
                }
                state.spawn_refused = true;
            }
        }
    }

    /// Bring up a worker when `lane` has more jobs queued than workers free
    /// to take one, to its limit. Called on every submit, and on every poll
    /// for the jobs a lost worker or a refused thread left behind.
    fn staff(&self, lane: &Arc<Lane>, state: &mut LaneState) {
        let free = state.alive.saturating_sub(state.busy);
        if state.alive < lane.max_workers && state.jobs.len() > free {
            self.spawn_worker(lane, state);
        }
    }

    /// Queue a job on the lane its asker belongs to: a deck's own load
    /// on the deck lane, the background pass on the pass lane. A pass job
    /// with no worker free to take it brings one up, to the lane's limit.
    pub fn submit(&self, job: AnalysisJob) {
        let lane = match job.deck {
            Some(_) => {
                self.shared.deck_started();
                &self.deck
            }
            None => &self.batch,
        };
        let mut state = lock(&lane.state);
        state.jobs.push_back(job);
        self.staff(lane, &mut state);
        drop(state);
        lane.more.notify_one();
    }

    /// Workers on the pass lane right now, waiting or busy.
    pub fn batch_workers_alive(&self) -> usize {
        lock(&self.batch.state).alive
    }

    /// Workers on the pass lane not running a job right now.
    pub fn batch_workers_free(&self) -> usize {
        let state = lock(&self.batch.state);
        state.alive.saturating_sub(state.busy)
    }

    /// Workers standing ready for the decks.
    pub fn deck_workers_alive(&self) -> usize {
        lock(&self.deck.state).alive
    }

    /// The records no worker could measure since the last poll.
    pub fn poll_failed(&self) -> Vec<AnalysisFailed> {
        let mut out = Vec::new();
        while let Ok(failed) = self.failed_rx.try_recv() {
            out.push(failed);
        }
        out
    }

    pub fn poll(&self) -> Vec<AnalysisDone> {
        for lane in [&self.deck, &self.batch] {
            let mut state = lock(&lane.state);
            self.staff(lane, &mut state);
        }
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

/// The pool going away takes its workers with it: what is queued is
/// dropped unread, a worker waiting for a job or for the decks leaves at
/// once, and one in the middle of a job leaves when the job is done.
impl Drop for AnalysisPool {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        for lane in [&self.deck, &self.batch] {
            let mut state = lock(&lane.state);
            state.closed = true;
            state.jobs.clear();
            drop(state);
            lane.more.notify_all();
        }
        // Under the lock the waiters check `closed` under, or a worker
        // between its check and its wait would sleep on to the cap.
        let busy = lock(&self.shared.deck_busy);
        self.shared.deck_idle.notify_all();
        drop(busy);
    }
}

// ---------------------------------------------------------------------------
// local files
// ---------------------------------------------------------------------------

/// Audio extensions the local explorer lane will offer. Both spellings of
/// an AIFF: the four-letter one has been listed all along and could not be
/// loaded, and the three-letter one is what the desktop tools write.
pub const LOCAL_AUDIO_EXTENSIONS: [&str; 10] =
    ["wav", "mp3", "ogg", "oga", "m4a", "aac", "flac", "aiff", "aif", "mp4"];

/// Decode a local audio file. WAV, MP3 and Ogg Vorbis parse in-process
/// (`makepad-audio-decode`); everything else goes to `decode_audio_clip`,
/// which looks at the file own first bytes before handing it to the
/// platform decoder. That is how a `.flac` reaches the FLAC decoder this
/// repo already has, and how an `.aiff` reaches the parser next to the WAV
/// one: there is no `MediaType` that names either.
pub fn decode_audio_file(path: &Path) -> Result<TrackPcm, String> {
    let media = crate::media::local_media_type(path);
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

    /// Both spellings of an AIFF are offered now. The four-letter one was
    /// listed all along and could not be loaded; the three-letter one is
    /// what the desktop tools write and was never offered at all.
    #[test]
    fn the_local_lane_offers_both_spellings_of_an_aiff() {
        assert!(LOCAL_AUDIO_EXTENSIONS.contains(&"aiff"));
        assert!(LOCAL_AUDIO_EXTENSIONS.contains(&"aif"));
        for kept in ["wav", "mp3", "ogg", "oga", "m4a", "aac", "flac", "mp4"] {
            assert!(LOCAL_AUDIO_EXTENSIONS.contains(&kept), "{kept} was dropped");
        }
        let dir = std::env::temp_dir()
            .join(format!("makepad-vj-local-lane-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("make dir");
        for name in ["a.aif", "b.aiff", "c.wav", "d.txt"] {
            std::fs::write(dir.join(name), b"stand in").expect("write");
        }
        let listed: Vec<String> = list_local_audio(&dir)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(listed, vec!["a.aif", "b.aiff", "c.wav"], "and a text file is not audio");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
            let mono = crate::dsp_math::mono(*frame);
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

    /// A grid at `bpm` whose first beat is a tenth of a second in, on the
    /// second beat of the bar.
    fn grid_at(bpm: f64) -> TrackGrid {
        TrackGrid {
            bpm,
            beat_secs: 60.0 / bpm,
            first_beat_secs: 0.1,
            downbeat_phase: 1,
            confidence: 0.8,
        }
    }

    /// How far the grid's beats move, worst case, over a record of this
    /// length.
    fn beat_walk(before: TrackGrid, after: TrackGrid, span: f64) -> f64 {
        let mut worst: f64 = 0.0;
        let mut at = 0.0;
        while at <= span {
            let n = before.beat_at(at).round();
            let was = before.first_beat_secs + n * before.beat_secs;
            let now = after.first_beat_secs + n * after.beat_secs;
            if (0.0..=span).contains(&was) {
                worst = worst.max((now - was).abs());
            }
            at += before.beat_secs;
        }
        worst
    }

    #[test]
    fn a_tempo_a_hair_off_a_whole_number_is_pulled_onto_it() {
        let seed = grid_at(127.994);
        let out = snap_tempo(seed, 300.0);
        assert_eq!(out.bpm, 128.0);
        assert_eq!(out.beat_secs, 60.0 / 128.0);
        assert_eq!(seed.bpm, 127.994, "taken by value");
    }

    #[test]
    fn a_tempo_too_far_from_any_rung_is_left_where_the_estimator_put_it() {
        // The nearest twelfth is 127.9167, 0.017 away; the budget over
        // five minutes is 0.0107.
        let seed = grid_at(127.90);
        assert_eq!(snap_tempo(seed, 300.0), seed);
    }

    #[test]
    fn the_pull_never_walks_the_grid_past_its_phase_budget() {
        for bpm in [72.4, 77.3975, 84.42, 96.2, 99.8925, 120.65, 130.45, 174.07] {
            for span in [30.0, 120.0, 420.0] {
                let seed = grid_at(bpm);
                let out = snap_tempo(seed, span);
                let walk = beat_walk(seed, out, span);
                assert!(
                    walk <= SNAP_PHASE_BUDGET_SECS + 1e-9,
                    "{bpm} over {span}s walked {walk}"
                );
            }
        }
    }

    #[test]
    fn a_whole_number_wins_over_a_nearer_twelfth() {
        // 128.0833 is three times closer than 128.0, and the budget over
        // thirty seconds (0.107) admits both. The ladder is ordered by
        // musical plausibility, not by distance.
        assert_eq!(snap_tempo(grid_at(128.07), 30.0).bpm, 128.0);
    }

    #[test]
    fn the_banded_rungs_only_answer_inside_their_own_band() {
        // Halves, below eighty-five: the same fractional offset reaches
        // .5 inside the band and only the nearer twelfth outside it.
        assert_eq!(snap_tempo(grid_at(84.45), 25.0).bpm, 84.5);
        let out = snap_tempo(grid_at(90.45), 25.0).bpm;
        assert!((out - 90.0 - 5.0 / 12.0).abs() < 1e-9, "{out}");

        // Two thirds, above a hundred and twenty-seven: 130.6667 rather
        // than the nearer 130.3333.
        let out = snap_tempo(grid_at(130.45), 12.0).bpm;
        assert!((out - 130.0 - 2.0 / 3.0).abs() < 1e-9, "{out}");
        // Below the band the plain third answers instead.
        let out = snap_tempo(grid_at(120.45), 12.0).bpm;
        assert!((out - 120.0 - 1.0 / 3.0).abs() < 1e-9, "{out}");
    }

    #[test]
    fn the_pull_keeps_the_bar_where_the_downbeat_was() {
        let seed = TrackGrid {
            bpm: 128.07,
            beat_secs: 60.0 / 128.07,
            first_beat_secs: 0.0004,
            downbeat_phase: 2,
            confidence: 0.8,
        };
        let out = snap_tempo(seed, 30.0);
        assert_eq!(out.bpm, 128.0);
        assert!(
            (0.0..out.beat_secs).contains(&out.first_beat_secs),
            "first beat {} outside [0, {})",
            out.first_beat_secs,
            out.beat_secs
        );
        // The same source second is still a downbeat: a bare rem_euclid on
        // the phase would rotate the bar by a beat.
        let was = (0..64)
            .map(|n| seed.first_beat_secs + n as f64 * seed.beat_secs)
            .find(|at| seed.is_downbeat(seed.beat_at(*at).round() as i64))
            .expect("a downbeat in the first sixteen bars");
        assert!(out.is_downbeat(out.beat_at(was).round() as i64), "the bar moved");
    }

    #[test]
    fn a_click_track_at_a_fractional_tempo_is_not_dragged_to_a_whole_one() {
        let pcm = click_track(44_100, 128.5, 30.0, 0.41);
        let bpm = analyze(&pcm).grid.bpm;
        assert!((bpm - 128.5).abs() < 1e-6, "{bpm}");
    }

    fn ramp_map(from_bpm: f64, to_bpm: f64, beats_each: f64) -> TempoMap {
        let mut segments = Vec::new();
        let mut at = 0.0;
        let mut beat = 0.0;
        for step in 0..4 {
            let bpm = from_bpm + (to_bpm - from_bpm) * step as f64 / 3.0;
            let period = 60.0 / bpm;
            segments.push(TempoSegment { start_secs: at, start_beat: beat, period_secs: period });
            at += period * beats_each;
            beat += beats_each;
        }
        TempoMap { segments }
    }

    /// The window is the same tempo the map holds, box-filtered over eight
    /// beats -- so it is continuous where the map itself steps.
    #[test]
    fn the_local_tempo_is_a_window_rather_than_a_step() {
        let map = ramp_map(120.0, 132.0, 16.0);
        // Deep inside the first segment, the window sees only that tempo.
        let inside = map.local_bpm(1.0).expect("a tempo");
        assert!((inside - 120.0).abs() < 1e-9, "{inside}");

        // Across a segment edge the window blends rather than jumping. The
        // map's own answer steps by four BPM at that edge.
        let edge_secs = map.secs_at_beat(16.0);
        let step = map.bpm_at(edge_secs + 1e-6) - map.bpm_at(edge_secs - 1e-6);
        assert!(step > 3.9, "the map itself steps by {step}");
        let before = map.local_bpm(edge_secs - 0.01).expect("a tempo");
        let after = map.local_bpm(edge_secs + 0.01).expect("a tempo");
        assert!((after - before).abs() < 0.05, "{before} -> {after}");

        // An empty map has no local answer at all.
        assert!(TempoMap::default().local_bpm(1.0).is_none());
    }

    /// The hinge re-cuts the line to a new tempo and pins the record where
    /// it is: the beat number and the fraction of a beat at that second
    /// come back exactly.
    #[test]
    fn a_hinged_grid_keeps_the_beat_it_is_on() {
        let grid = TrackGrid {
            bpm: 120.0,
            beat_secs: 0.5,
            first_beat_secs: 0.1,
            downbeat_phase: 2,
            confidence: 0.9,
        };
        let at = 61.35;
        let hinged = grid.hinged_at(at, 126.0);
        assert!((hinged.bpm - 126.0).abs() < 1e-9);
        assert!((hinged.beat_secs - 60.0 / 126.0).abs() < 1e-12);
        // The FRACTION of a beat is what a hinge promises, not the beat's
        // absolute number: the published anchor has to stay inside its own
        // period, so the numbering slides and the bar phase slides back.
        let (was, now) = (grid.beat_at(at), hinged.beat_at(at));
        assert!((was.fract() - now.fract()).abs() < 1e-9, "{was} vs {now}");
        // And the second that was a downbeat still is one.
        let downbeat = grid.first_beat_secs + was.floor() * grid.beat_secs;
        assert!(grid.is_downbeat(grid.beat_at(downbeat).round() as i64));
        assert!(
            hinged.is_downbeat(hinged.beat_at(downbeat).round() as i64),
            "the bar moved"
        );
        assert!(
            (0.0..hinged.beat_secs).contains(&hinged.first_beat_secs),
            "first beat {}",
            hinged.first_beat_secs
        );
        // A tempo that is not one, or the same tempo, changes nothing.
        assert_eq!(grid.hinged_at(at, 120.0), grid);
        assert_eq!(grid.hinged_at(at, f64::NAN), grid);
        assert_eq!(grid.hinged_at(at, 0.0), grid);
    }

    /// The stages add up to the whole, and each one is really measured.
    /// The loudness rides the sidecar, so a record played before is
    /// level-matched the moment it lands rather than after a re-measure.
    #[test]
    fn the_measured_loudness_survives_the_cache() {
        let pcm = click_track(48_000, 128.0, 8.0, 0.0);
        let mut analysis = analyze(&pcm);
        analysis.loudness_lufs = Some(-17.25);
        let back = decode_analysis(&encode_analysis(&analysis)).expect("a round trip");
        assert_eq!(back.loudness_lufs, Some(-17.25));
        // And a record with nothing to measure comes back with nothing.
        analysis.loudness_lufs = None;
        let back = decode_analysis(&encode_analysis(&analysis)).expect("a round trip");
        assert_eq!(back.loudness_lufs, None);
        // A stored number that cannot mean a level is refused, not used.
        analysis.loudness_lufs = Some(f64::NAN);
        let back = decode_analysis(&encode_analysis(&analysis)).expect("a round trip");
        assert_eq!(back.loudness_lufs, None, "a level that is not a number is none");
    }

    /// A grid read off a drums lane is the same estimator shown a
    /// cleaner signal: on a click track with a mixed-in pad, the drums
    /// alone still find the tempo.
    #[test]
    fn a_grid_can_be_read_off_the_drums_alone() {
        let drums = click_track(48_000, 128.0, 12.0, 0.25);
        // The "rest": a steady A minor triad, which is what the chroma
        // pass is being handed while the drums keep the pulse.
        let rate = 48_000;
        let mut rest = Vec::with_capacity(rate * 12);
        for n in 0..(rate * 12) {
            let t = n as f64 / rate as f64;
            let mut value = 0.0;
            for hz in [220.0, 261.63, 329.63] {
                value += (2.0 * std::f64::consts::PI * hz * t).sin();
            }
            let sample = (value / 3.0 * 8000.0) as i16;
            rest.push([sample, sample]);
        }
        let rest = TrackPcm { frames: rest, sample_rate: rate as u32 };
        let reading = stem_reading(&drums, &rest, 12.0).expect("a reading");
        assert!((reading.grid.bpm - 128.0).abs() < 0.5, "{}", reading.grid.bpm);
        let key = reading.key.expect("a key");
        assert_eq!(key.camelot(), "8A", "A minor off the pitched lanes");

        // Half a reading is not a reading.
        let empty = TrackPcm { frames: Vec::new(), sample_rate: 48_000 };
        assert!(stem_reading(&drums, &empty, 12.0).is_none());
        assert!(stem_reading(&empty, &rest, 12.0).is_none());
    }

    /// Separation is the point, so its answer is preferred -- but never
    /// when it is markedly less sure of itself than the mix's.
    #[test]
    fn a_stem_reading_gives_way_when_it_is_less_sure() {
        let grid = |confidence: f32| TrackGrid {
            bpm: 128.0,
            beat_secs: 60.0 / 128.0,
            first_beat_secs: 0.0,
            downbeat_phase: 0,
            confidence,
        };
        assert!(stem_reading_wins(&grid(0.6), &grid(0.6)));
        assert!(stem_reading_wins(&grid(0.6), &grid(0.55)), "a hair under still wins");
        assert!(!stem_reading_wins(&grid(0.8), &grid(0.3)), "but not a shrug");
        // A mix with no grid at all takes anything measured.
        assert!(stem_reading_wins(&TrackGrid::default(), &grid(0.2)));
        // And a stem reading that is not a grid is never taken.
        assert!(!stem_reading_wins(&grid(0.6), &TrackGrid::default()));
    }

    #[test]
    fn an_analysis_says_where_its_time_went() {
        let pcm = click_track(44_100, 128.0, 12.0, 0.41);
        let (analysis, timing) = analyze_timed(&pcm, None);
        assert!(analysis.grid.has_grid(), "and it still analysed the track");
        // Every stage is a real number of milliseconds or a zero, and the
        // total is exactly their sum -- no stage is left out of it.
        let sum = timing.envelopes
            + timing.prior
            + timing.grid
            + timing.tiles
            + timing.key
            + timing.loudness;
        assert_eq!(timing.total(), sum);
        // The two that read every sample cannot both be free.
        assert!(
            timing.envelopes + timing.key + timing.prior > 0,
            "twelve seconds of audio measured as no time at all: {timing:?}"
        );
    }

    fn clock_grid() -> TrackGrid {
        TrackGrid {
            bpm: 120.0,
            beat_secs: 0.5,
            first_beat_secs: 0.0,
            downbeat_phase: 0,
            confidence: 0.9,
        }
    }

    /// No grid is no clock -- but the platter is still reported, because
    /// a stage that only wants the speed should not have to know whether
    /// the record was ever measured.
    #[test]
    fn a_clock_without_a_grid_says_so_and_still_reports_the_platter() {
        let clock = DeckClock::at(None, 3.0, 1.25, 0.01);
        assert!(!clock.has_grid);
        assert_eq!(clock.beat_secs_out, 0.0);
        assert_eq!(clock.beat_len(), None);
        assert_eq!(clock.beat_frac_end, 0.0);
        assert_eq!(clock.platter_rate, 1.25);
        // A grid with no beats is no grid.
        let none = DeckClock::at(Some(&TrackGrid::default()), 3.0, 1.0, 0.01);
        assert!(!none.has_grid);
        assert_eq!(none.beat_len(), None);
        // And a speed that is not a number is a stopped platter.
        assert_eq!(DeckClock::at(Some(&clock_grid()), 3.0, f64::NAN, 0.0).platter_rate, 0.0);
    }

    /// A beat is an OUTPUT length: pitched up it is shorter, backwards it
    /// is the same length, and stopped it has none.
    #[test]
    fn a_clock_reads_the_beat_length_at_the_platters_speed() {
        let grid = clock_grid();
        assert_eq!(DeckClock::at(Some(&grid), 0.0, 1.0, 0.0).beat_secs_out, 0.5);
        let fast = DeckClock::at(Some(&grid), 0.0, 1.25, 0.0);
        assert!((fast.beat_secs_out - 0.4).abs() < 1e-12, "{}", fast.beat_secs_out);
        assert_eq!(fast.beat_len(), Some(fast.beat_secs_out));
        let back = DeckClock::at(Some(&grid), 0.0, -1.0, 0.0);
        assert_eq!(back.beat_secs_out, 0.5, "a length has no sign");
        assert_eq!(back.platter_rate, -1.0);
        let still = DeckClock::at(Some(&grid), 0.0, 0.0, 0.0);
        assert_eq!(still.beat_secs_out, 0.0);
        assert_eq!(still.beat_len(), None, "a beat that never arrives has no length");
        assert!(still.has_grid, "but the grid is still there");
    }

    /// The fraction is for the END of the buffer: the same arithmetic the
    /// grid uses everywhere, pushed forward by the travel the caller says
    /// the deck will make.
    #[test]
    fn a_clock_says_where_the_beat_will_be_when_the_buffer_ends() {
        let grid = clock_grid();
        let travel = 512.0 / 48_000.0;
        let ahead = DeckClock::at(Some(&grid), 0.1, 1.0, travel);
        assert_eq!(ahead.beat_frac_end, grid.phase_at(0.1 + travel));
        let faster = DeckClock::at(Some(&grid), 0.1, 2.0, 2.0 * travel);
        assert_eq!(faster.beat_frac_end, grid.phase_at(0.1 + 2.0 * travel));
        // Backwards stays inside [0, 1): the grid's own wrap does that.
        let back = DeckClock::at(Some(&grid), 0.1, -1.0, -travel);
        assert_eq!(back.beat_frac_end, grid.phase_at(0.1 - travel));
        assert!((0.0..1.0).contains(&back.beat_frac_end));
        // And a deck told it will not travel answers for where it IS.
        let parked = DeckClock::at(Some(&grid), 0.1, 1.0, 0.0);
        assert_eq!(parked.beat_frac_end, grid.phase_at(0.1));
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


    // ---- where the file starts and stops making a sound -----------------

    #[test]
    fn the_scan_finds_the_first_and_last_sample_that_makes_a_sound() {
        let rate = 48_000u32;
        let mut frames = vec![[0i16; 2]; rate as usize * 6];
        // Two seconds of nothing, then a second of tone, then nothing, then
        // half a second more, then nothing again.
        for (index, frame) in frames.iter_mut().enumerate() {
            let secs = index as f64 / rate as f64;
            let sounding = (2.0..3.0).contains(&secs) || (4.5..5.0).contains(&secs);
            if sounding {
                let value = ((secs * 440.0 * std::f64::consts::TAU).sin() * 8_000.0) as i16;
                *frame = [value, value];
            }
        }
        let envelopes = build_envelopes(&TrackPcm { frames, sample_rate: rate });
        let (first, last) = envelopes.sound.expect("the file makes a sound");
        assert!(
            (first as f64 / rate as f64 - 2.0).abs() < 0.01,
            "the first sound is where the tone starts, at {}",
            first as f64 / rate as f64,
        );
        // The silence in the middle must not truncate it: the LAST sound is
        // the last one, not the end of the first run.
        assert!(
            (last as f64 / rate as f64 - 5.0).abs() < 0.01,
            "and the last is the end of the second run, at {}",
            last as f64 / rate as f64,
        );
    }

    #[test]
    fn a_silent_file_has_no_sound_at_all_rather_than_a_span_at_zero() {
        let rate = 48_000u32;
        let envelopes = build_envelopes(&TrackPcm {
            frames: vec![[0i16; 2]; rate as usize * 3],
            sample_rate: rate,
        });
        assert!(envelopes.sound.is_none(), "nothing in it reaches the floor");
    }

    #[test]
    fn a_passage_that_folds_to_silence_in_mono_is_still_a_sound() {
        // Anti-phase stereo: the mono fold is zero everywhere, which is why
        // the scan reads the channels rather than the fold.
        let rate = 48_000u32;
        let mut frames = vec![[0i16; 2]; rate as usize];
        for (index, frame) in frames.iter_mut().enumerate().skip(rate as usize / 2) {
            let value = ((index as f64 * 0.05).sin() * 8_000.0) as i16;
            *frame = [value, -value];
        }
        let envelopes = build_envelopes(&TrackPcm { frames, sample_rate: rate });
        let (first, _) = envelopes.sound.expect("a sound the fold cannot hear");
        assert!(first >= rate as usize / 2 - 64);
    }

    #[test]
    fn the_quietest_thing_that_counts_is_sixty_decibels_down() {
        let rate = 48_000u32;
        // A hair under the floor is silence; a hair over it is a sound.
        let under = ((SOUND_FLOOR * 32_768.0).ceil() as i16) - 1;
        let over = (SOUND_FLOOR * 32_768.0).ceil() as i16;
        for (value, want) in [(under, false), (over, true)] {
            let mut frames = vec![[0i16; 2]; rate as usize];
            frames[rate as usize / 2] = [value, value];
            let envelopes = build_envelopes(&TrackPcm { frames, sample_rate: rate });
            assert_eq!(
                envelopes.sound.is_some(),
                want,
                "a sample of {value} must{} count",
                if want { "" } else { " not" },
            );
        }
    }

    #[test]
    fn the_sound_span_survives_the_cache_as_itself() {
        let mut analysis = analyze(&click_track(48_000, 120.0, 8.0, 0.0));
        analysis.sound = Some(SoundSpan { first_secs: 1.25, last_secs: 7.5 });
        let back = decode_analysis(&encode_analysis(&analysis)).expect("round trip");
        assert_eq!(back.sound, analysis.sound);
        analysis.sound = None;
        let back = decode_analysis(&encode_analysis(&analysis)).expect("round trip");
        assert_eq!(back.sound, None, "no sound at all is a value too");
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
        // The fixed header answers the explorer's columns without the tiles,
        // and it must agree with the full decode byte for byte.
        let summary = decode_summary(&bytes).expect("summary");
        assert_eq!(summary.grid, analysis.grid);
        assert_eq!(summary.key, analysis.key);
        assert!((summary.duration_secs - analysis.duration_secs).abs() < 1e-9);
        // Versions 5 and 6 each lack a field this layout carries (the key, or
        // the refinement marker) and version 7 carries a key the old chroma
        // got wrong, so they are re-analysed, never misread.
        for version in [4u32, 5, 6, 7] {
            let mut old = encode_analysis(&analysis);
            old[8..12].copy_from_slice(&version.to_le_bytes());
            assert!(decode_analysis(&old).is_err(), "version {version}");
            assert!(decode_summary(&old).is_none(), "version {version}");
        }
    }

    #[test]
    fn the_key_survives_the_cache_as_itself() {
        let pcm = click_track(48_000, 124.0, 12.0, 0.2);
        let mut analysis = analyze(&pcm);
        // Both answers have to round trip, and they have to stay DIFFERENT
        // answers: "no tonal centre" must not come back as C major, which is
        // exactly what a zeroed field would read as.
        analysis.key = None;
        assert_eq!(decode_analysis(&encode_analysis(&analysis)).expect("decode").key, None);
        for (tonic, minor) in [(0u8, false), (9, true), (11, true), (6, false)] {
            analysis.key = Some(KeyEstimate { tonic, minor, confidence: 0.42 });
            let back = decode_analysis(&encode_analysis(&analysis)).expect("decode");
            assert_eq!(back.key, analysis.key, "tonic {tonic} minor {minor}");
        }
        // A tonic outside the octave is corruption, not a twelve-and-a-half.
        let mut corrupt = encode_analysis(&analysis);
        corrupt[57] = 12;
        assert_eq!(decode_analysis(&corrupt).expect("decode").key, None);
    }

    #[test]
    fn a_summary_reads_the_header_without_the_tiles() {
        let pcm = click_track(48_000, 124.0, 12.0, 0.2);
        let mut analysis = analyze(&pcm);
        analysis.key = Some(KeyEstimate { tonic: 9, minor: true, confidence: 0.5 });
        let bytes = encode_analysis(&analysis);
        // The whole point of the header layout: the first SUMMARY_LEN bytes
        // are enough. If the key ever moves back behind the tiles this fails.
        let summary = decode_summary(&bytes[..SUMMARY_LEN]).expect("summary");
        assert_eq!(summary.grid, analysis.grid);
        assert_eq!(summary.key, analysis.key);
        assert!((summary.duration_secs - analysis.duration_secs).abs() < 1e-9);
        assert!(bytes.len() > SUMMARY_LEN * 4, "the fixture must have real tiles behind it");
        // Short, junk and stale-version reads are refused rather than guessed.
        assert!(decode_summary(&bytes[..SUMMARY_LEN - 1]).is_none());
        assert!(decode_summary(b"nope").is_none());
        let mut old = bytes.clone();
        old[8..12].copy_from_slice(&5u32.to_le_bytes());
        assert!(decode_summary(&old).is_none());
    }

    #[test]
    fn the_overview_comes_off_disk_without_the_waveform_behind_it() {
        // The whole point: rank a pool by loudness without paging in the
        // zoom channel, which is some thirty times the size and which a
        // picker will never draw.
        let pcm = click_track(48_000, 120.0, 8.0, 0.2);
        let analysis = analyze(&pcm);
        let dir = std::env::temp_dir()
            .join(format!("makepad-vj-overview-{}", std::process::id()));
        let key = AnalysisKey::from_blob(BlobId::hash_of(b"an overviewed track"));
        assert!(load_cached_overview(&dir, &key).is_none(), "nothing stored yet");

        store_cached(&dir, &key, &analysis);
        let overview = load_cached_overview(&dir, &key).expect("overview off disk");
        assert_eq!(overview, analysis.tiles.overview, "byte for byte");
        assert!(!overview.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn pool_job(deck: Option<DeckId>, gen: u64, pcm: &Arc<TrackPcm>) -> AnalysisJob {
        AnalysisJob {
            deck,
            gen,
            key: AnalysisKey::from_blob(BlobId::hash_of(format!("pool job {gen}").as_bytes())),
            pcm: pcm.clone(),
            beats_model: None,
            tag_bpm: None,
            fast: false,
        }
    }

    /// A pool for a test: its sidecars go to a scratch directory, never
    /// the operator's cache. Hand the directory back to `done_with`.
    fn test_pool(deck: usize, batch: usize, idle: Duration) -> (AnalysisPool, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "vj-pool-{}-{}",
            std::process::id(),
            crate::wave_analysis::tests::POOL_DIRS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let schedule = LaneSchedule { deck_workers: deck, batch_workers: batch, batch_idle: idle };
        (AnalysisPool::with_schedule_in(schedule, Some(dir.clone())), dir)
    }

    static POOL_DIRS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn done_with(pool: AnalysisPool, dir: PathBuf) {
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Wait up to `secs` for `holds` to be true.
    fn settle(secs: u64, mut holds: impl FnMut() -> bool) -> bool {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(secs) {
            if holds() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        holds()
    }

    /// Poll the pool for up to `secs`, handing every result to `each`;
    /// stops early when `each` returns true.
    fn poll_until(pool: &AnalysisPool, secs: u64, mut each: impl FnMut(AnalysisDone) -> bool) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(secs) {
            for done in pool.poll() {
                if each(done) {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// One worker per deck so both can load at once; the pass gets half
    /// the cores and never more than four, whatever the machine has.
    #[test]
    fn the_lanes_are_staffed_by_the_core_count_with_the_pass_capped() {
        let sizes = |cores| {
            let s = LaneSchedule::for_cores(cores);
            (s.deck_workers, s.batch_workers)
        };
        assert_eq!(sizes(0), (1, 1), "a machine that will not say has one of each");
        assert_eq!(sizes(1), (1, 1));
        assert_eq!(sizes(2), (2, 1));
        assert_eq!(sizes(4), (2, 2));
        assert_eq!(sizes(8), (2, 4));
        assert_eq!(sizes(32), (2, 4), "capped: each worker holds a whole record and a model");
        let (pool, dir) = test_pool(2, 4, Duration::from_secs(60));
        assert_eq!(pool.deck_workers_alive(), 2, "standing ready for both decks");
        assert_eq!(pool.batch_workers_alive(), 0, "and nobody for a pass that has not started");
        done_with(pool, dir);
        // A schedule of nothing still runs: one worker a lane.
        let (pool, dir) = test_pool(0, 0, Duration::from_secs(60));
        assert_eq!(pool.deck_workers_alive(), 1);
        let pcm = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        pool.submit(pool_job(None, 1, &pcm));
        let mut back = false;
        poll_until(&pool, 20, |_| {
            back = true;
            back
        });
        assert!(back, "a pass job still came back");
        assert_eq!(pool.batch_workers_alive(), 1);
        done_with(pool, dir);
    }

    /// A deck's own load and the background pass over the library go
    /// down different lanes, so neither can be stuck behind the other; and
    /// the pass holds its next job while a deck is waiting, so the only
    /// pass jobs that finish ahead of the deck are the ones already running
    /// when it asked.
    #[test]
    fn a_deck_load_does_not_queue_behind_the_background_pass() {
        let (pool, dir) = test_pool(1, 2, Duration::from_secs(60));
        // Ten pass jobs, then one a deck is waiting on. Only pass jobs
        // already running when the deck asked can finish ahead of it: the
        // rest are held, however fast or slow this machine is.
        let pcm = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        for n in 0..10 {
            pool.submit(pool_job(None, n, &pcm));
        }
        pool.submit(pool_job(Some(DeckId::A), 99, &pcm));
        let mut deck_at = None;
        let mut batches = 0;
        poll_until(&pool, 40, |done| {
            match done.deck {
                Some(_) => deck_at = Some(batches),
                None => batches += 1,
            }
            deck_at.is_some()
        });
        let deck_at = deck_at.expect("the deck's analysis came back");
        assert!(deck_at <= 2, "it waited for {deck_at} pass jobs; at most the two already running");
        assert!(pool.batch_workers_alive() <= 2, "never more workers than the lane allows");
        // And the deck's job is no longer counted against the pass once
        // its worker is done with it.
        assert!(settle(5, || *lock(&pool.shared.deck_busy) == 0), "the deck count went back to zero");
        done_with(pool, dir);
    }

    /// The pass lane brings up a worker only for a job no waiting worker
    /// will take: a second job after the first has come back is taken by
    /// the worker that is waiting, not by a new one.
    #[test]
    fn the_pass_lane_never_brings_up_a_worker_it_does_not_need() {
        let (pool, dir) = test_pool(1, 2, Duration::from_secs(60));
        let pcm = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        pool.submit(pool_job(None, 1, &pcm));
        assert_eq!(pool.batch_workers_alive(), 1);
        poll_until(&pool, 20, |_| true);
        assert!(settle(5, || pool.batch_workers_free() == 1), "the worker is free again");
        pool.submit(pool_job(None, 2, &pcm));
        let mut back = false;
        poll_until(&pool, 20, |_| {
            back = true;
            back
        });
        assert!(back);
        assert_eq!(pool.batch_workers_alive(), 1, "the waiting worker took it");
        // A worker running a job is not free: the next job brings up another.
        pool.submit(pool_job(None, 3, &pcm));
        assert!(settle(5, || pool.batch_workers_free() == 0), "the worker took it");
        pool.submit(pool_job(None, 4, &pcm));
        assert_eq!(pool.batch_workers_alive(), 2, "a second worker for the second job");
        done_with(pool, dir);
    }

    /// While a deck is waiting on its lane, the pass holds the job it took
    /// and starts nothing; the moment the deck is served, the pass goes on.
    /// The deck's wait is raised through the pool's own state so this
    /// depends on no analysis being faster or slower than another.
    #[test]
    fn the_pass_holds_its_job_while_a_deck_is_waiting() {
        // The cap on the wait (30 s) is longer than the poll below (20 s):
        // if the release did not wake the workers, the test would time out
        // rather than pass on the cap.
        let (pool, dir) = test_pool(1, 2, Duration::from_secs(60));
        let pcm = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        pool.shared.deck_started();
        for n in 0..4 {
            pool.submit(pool_job(None, n, &pcm));
        }
        let mut early = 0;
        poll_until(&pool, 1, |_| {
            early += 1;
            false
        });
        assert_eq!(early, 0, "a pass job finished while a deck was waiting");
        assert!(pool.batch_workers_alive() >= 1, "the workers are up, holding their jobs");
        pool.shared.deck_finished();
        let mut later = 0;
        poll_until(&pool, 20, |_| {
            later += 1;
            later == 4
        });
        assert_eq!(later, 4, "and every held job ran once the deck was served");
        done_with(pool, dir);
    }

    /// The pass lane is staffed as jobs arrive; when it has drained, the
    /// extra workers stand down after the idle time and one stays, with
    /// the model it loaded, for whatever comes next.
    #[test]
    fn the_pass_lane_stands_down_to_one_when_it_drains() {
        let (pool, dir) = test_pool(1, 2, Duration::from_millis(150));
        let pcm = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        for n in 0..3 {
            pool.submit(pool_job(None, n, &pcm));
        }
        assert_eq!(pool.batch_workers_alive(), 2, "three jobs at once is two workers");
        let mut done = 0;
        poll_until(&pool, 20, |_| {
            done += 1;
            done == 3
        });
        assert_eq!(done, 3, "the three pass jobs came back");
        assert!(settle(10, || pool.batch_workers_alive() == 1), "one stayed, the other left");
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(pool.batch_workers_alive(), 1, "and the one that stays does not leave");
        pool.submit(pool_job(None, 7, &pcm));
        let mut back = false;
        poll_until(&pool, 20, |result| {
            back = result.gen == 7;
            back
        });
        assert!(back, "the kept worker took the job");
        assert_eq!(pool.batch_workers_alive(), 1);
        done_with(pool, dir);
    }

    /// The pool going away takes its workers with it, the ones waiting for
    /// a job and the ones ready for the decks alike.
    #[test]
    fn dropping_the_pool_lets_its_workers_go() {
        let (pool, dir) = test_pool(2, 2, Duration::from_secs(60));
        let pcm = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        pool.submit(pool_job(None, 1, &pcm));
        poll_until(&pool, 20, |_| true);
        let (deck, batch) = (pool.deck.clone(), pool.batch.clone());
        assert_eq!(lock(&deck.state).alive, 2);
        assert_eq!(lock(&batch.state).alive, 1);
        drop(pool);
        assert!(
            settle(5, || lock(&deck.state).alive == 0 && lock(&batch.state).alive == 0),
            "every worker left: deck {} pass {}",
            lock(&deck.state).alive,
            lock(&batch.state).alive
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn test_shared(deck_wait_cap: Duration) -> PoolShared {
        PoolShared {
            deck_busy: Mutex::new(0),
            deck_idle: Condvar::new(),
            deck_wait_cap,
            model_gate: Mutex::new(()),
            closed: AtomicBool::new(false),
            cache_root: None,
        }
    }

    /// A pass worker holding its job for the decks leaves at once when the
    /// pool goes, not when the cap on its wait runs out.
    #[test]
    fn a_pass_worker_waiting_for_the_decks_leaves_when_the_pool_does() {
        let (pool, dir) = test_pool(1, 2, Duration::from_secs(60));
        let pcm = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        pool.shared.deck_started();
        pool.submit(pool_job(None, 1, &pcm));
        assert!(
            settle(5, || pool.batch_workers_alive() == 1 && pool.batch_workers_free() == 0),
            "the worker took the job and is holding it for the deck"
        );
        let lane = pool.batch.clone();
        drop(pool);
        // The cap is 30 s; a lost wake-up would take that long.
        assert!(settle(5, || lock(&lane.state).alive == 0), "it left when the pool did");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A worker that panics with a job in hand gives its seat and its
    /// busy count back, so the lane can be staffed again.
    #[test]
    fn a_panicking_worker_gives_its_seat_back() {
        let lane = Arc::new(Lane {
            state: Mutex::new(LaneState {
                jobs: VecDeque::new(),
                alive: 1,
                busy: 1,
                closed: false,
                spawn_refused: false,
            }),
            more: Condvar::new(),
            max_workers: 2,
            keep_workers: 1,
            idle: Duration::from_secs(60),
            yields_to_decks: true,
        });
        let seated = lane.clone();
        let _ = std::thread::spawn(move || {
            let _seat = Seat { lane: seated, given_back: false, has_job: true };
            panic!("the record was bad");
        })
        .join();
        let state = lock(&lane.state);
        assert_eq!((state.alive, state.busy), (0, 0), "seat and busy count both given back");
    }

    /// A deck job that panics frees the count the pass is waiting on.
    #[test]
    fn a_panicking_deck_job_frees_the_pass() {
        let shared = Arc::new(test_shared(Duration::from_secs(30)));
        shared.deck_started();
        let inner = shared.clone();
        let _ = std::thread::spawn(move || {
            let _busy = DeckBusy(&inner);
            panic!("the record was bad");
        })
        .join();
        assert_eq!(*lock(&shared.deck_busy), 0);
        assert!(shared.wait_deck_idle(), "the pass may go on");
    }

    /// The pass lane's turn at the model steps back for a deck that
    /// arrived while it was in line; the deck lane's turn waits for
    /// nothing but the model.
    #[test]
    fn a_pass_turn_at_the_model_steps_back_for_a_deck() {
        let shared = Arc::new(test_shared(Duration::from_secs(30)));
        // The test plays a worker holding the model; a pass worker lines up.
        let held = lock(&shared.model_gate);
        let in_line = shared.clone();
        let pass = std::thread::spawn(move || {
            let _turn = in_line.model_turn(true);
        });
        std::thread::sleep(Duration::from_millis(150));
        assert!(!pass.is_finished(), "in line behind the held model");
        // A deck arrives while the pass is in line, and the model frees.
        shared.deck_started();
        drop(held);
        // The pass does not take the turn -- given time to have taken it
        // wrongly -- and the model is free for the deck.
        std::thread::sleep(Duration::from_millis(300));
        assert!(!pass.is_finished(), "the pass is still waiting for the deck");
        assert!(
            settle(5, || shared.model_gate.try_lock().is_ok()),
            "the model is there for the deck"
        );
        {
            let _deck_turn = shared.model_turn(false);
            assert_eq!(*lock(&shared.deck_busy), 1, "the deck's turn waited for nothing but the model");
        }
        shared.deck_finished();
        pass.join().expect("the pass took its turn once the deck was done");
    }

    /// Leaving is decided under the lock a job is pushed under: a worker
    /// whose idle time ran out while the lock was held finds the job pushed
    /// meanwhile and takes it, rather than leaving it to nobody.
    #[test]
    fn a_job_pushed_as_a_worker_leaves_is_taken_not_stranded() {
        let (pool, dir) = test_pool(1, 2, Duration::from_millis(100));
        let pcm = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        pool.submit(pool_job(None, 1, &pcm));
        pool.submit(pool_job(None, 2, &pcm));
        let mut done = 0;
        poll_until(&pool, 20, |_| {
            done += 1;
            done == 2
        });
        assert_eq!(done, 2);
        assert!(settle(5, || pool.batch_workers_free() == 2), "both waiting");
        // Hold the lane while the extra worker's idle time runs out, then
        // push a job the way submit does, under the same lock.
        {
            let mut state = lock(&pool.batch.state);
            std::thread::sleep(Duration::from_millis(300));
            state.jobs.push_back(pool_job(None, 3, &pcm));
            pool.staff(&pool.batch, &mut state);
            assert!(state.alive <= 2, "no third worker for a job two can take");
        }
        pool.batch.more.notify_one();
        let mut back = false;
        poll_until(&pool, 20, |result| {
            back = result.gen == 3;
            back
        });
        assert!(back, "the job was taken");
        assert!(settle(10, || pool.batch_workers_alive() == 1), "and the extra worker then left");
        done_with(pool, dir);
    }

    /// A record the worker cannot measure comes back as a failure, so the
    /// pass gets its slot back, and the lane goes on to the next record.
    #[test]
    fn a_record_that_cannot_be_measured_is_reported_and_the_lane_goes_on() {
        let (pool, dir) = test_pool(1, 1, Duration::from_secs(60));
        let bad = Arc::new(TrackPcm { frames: Vec::new(), sample_rate: 0 });
        pool.submit(pool_job(None, 1, &bad));
        let good = Arc::new(click_track(48_000, 128.0, 2.0, 0.0));
        pool.submit(pool_job(None, 2, &good));
        let started = Instant::now();
        let (mut failed, mut back) = (None, false);
        while started.elapsed() < Duration::from_secs(20) && !(failed.is_some() && back) {
            for f in pool.poll_failed() {
                failed = Some(f);
            }
            for done in pool.poll() {
                back |= done.gen == 2;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let failed = failed.expect("the bad record was reported");
        assert_eq!(failed.gen, 1);
        assert!(!failed.error.is_empty());
        assert!(back, "the next record was measured by the same lane");
        assert_eq!(pool.batch_workers_alive(), 1);
        done_with(pool, dir);
    }

    /// A lock a panicking job poisoned is still a lock: the pool keeps
    /// serving the decks whatever one job did.
    #[test]
    fn a_poisoned_lock_is_still_a_lock() {
        let shared = Arc::new(Mutex::new(3usize));
        let poisoner = shared.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("a job went wrong while it held the lane");
        })
        .join();
        assert!(shared.lock().is_err(), "std says poisoned");
        assert_eq!(*lock(&shared), 3, "and the pool reads it anyway");
    }

    /// The pass waits for the decks, and not forever: a deck job that never
    /// reported back would otherwise starve the pass for the night.
    #[test]
    fn the_pass_waits_for_the_decks_but_not_past_the_cap() {
        let shared = test_shared(Duration::from_millis(80));
        assert!(shared.wait_deck_idle(), "nothing to wait for");
        shared.deck_started();
        let started = Instant::now();
        assert!(!shared.wait_deck_idle(), "still busy when the cap ran out");
        assert!(started.elapsed() >= Duration::from_millis(80));
        assert!(started.elapsed() < Duration::from_secs(5));
        shared.deck_finished();
        assert!(shared.wait_deck_idle(), "and free again once the deck is done");
        shared.deck_finished();
        assert!(shared.wait_deck_idle(), "a finish with nothing started does not go negative");
    }

    /// A change to one detector costs that product's work and no more.
    /// A file that says its tempo answers the detector's weakest
    /// question -- which octave -- and nothing else.
    /// The first minute is enough to fill a column, and says that is all
    /// it looked at.
    #[test]
    fn a_fast_pass_looks_at_the_first_minute_and_says_so() {
        // Three minutes at 128, so the window is a third of it.
        let pcm = click_track(48_000, 128.0, 180.0, 0.41);
        let fast = analyze_fast(&pcm, None);
        assert!(fast.partial, "and it says so");
        assert!((fast.grid.bpm - 128.0).abs() < 0.2, "{}", fast.grid.bpm);
        // The DURATION is the record's, not the window's: it is read off
        // the file, and a column saying every long record is a minute
        // long would be worse than a blank one.
        assert!((fast.duration_secs - 180.0).abs() < 0.1, "{}", fast.duration_secs);
        // A record shorter than the window is measured whole, and does
        // not claim to be partial -- nothing should re-measure it later
        // for nothing.
        let short = analyze_fast(&click_track(48_000, 128.0, 20.0, 0.41), None);
        assert!(!short.partial);
        // And a partial answer survives the sidecar as partial.
        let back = decode_analysis(&encode_analysis(&fast)).expect("a round trip");
        assert!(back.partial);
    }

    #[test]
    fn a_tagged_tempo_breaks_the_octave_tie() {
        // A pulse with an even, weaker beat between each pair: the comb
        // can read it at either 96 or 192, which is exactly the tie a
        // tagged tempo exists to settle.
        let rate = 48_000u32;
        let seconds = 20.0;
        let mut frames = vec![[0i16; 2]; (rate as f64 * seconds) as usize];
        let beat = 60.0 / 96.0;
        let mut at = 0.05;
        let mut strong = true;
        while at < seconds {
            let start = (at * rate as f64) as usize;
            let level = if strong { 22_000 } else { 17_000 };
            for n in 0..600 {
                if let Some(frame) = frames.get_mut(start + n) {
                    let fade = 1.0 - n as f64 / 600.0;
                    let value = (level as f64 * fade) as i16;
                    frame[0] = value;
                    frame[1] = value;
                }
            }
            at += beat / 2.0;
            strong = !strong;
        }
        let pcm = TrackPcm { frames, sample_rate: rate };
        let slow = analyze_timed(&pcm, Some(96.0)).0.grid.bpm;
        let fast = analyze_timed(&pcm, Some(192.0)).0.grid.bpm;
        assert!(
            (slow - 96.0).abs() < 1.0 || (fast - 192.0).abs() < 1.0,
            "neither hint was taken: {slow} and {fast}"
        );
        assert!(fast >= slow, "the faster hint never reads slower: {slow} vs {fast}");
    }

    #[test]
    fn only_the_product_that_changed_is_measured_again() {
        let pcm = click_track(48_000, 128.0, 8.0, 0.0);
        let fresh = analyze(&pcm);
        assert!(!fresh.stale.any(), "a fresh measurement is never stale");

        // A stored record read back by this build is agreed with entirely.
        let bytes = encode_analysis(&fresh);
        let back = decode_analysis(&bytes).expect("a round trip");
        assert!(!back.stale.any());

        // One whose key was measured by something else: only the key.
        let mut older = back.clone();
        older.stale = Stale { key: true, ..Default::default() };
        let before = older.grid;
        let tiles = older.tiles.overview.len();
        older.key = None;
        assert!(repair(&mut older, &pcm, None), "there was something to repair");
        assert!(older.key.is_some(), "the key was measured again");
        assert_eq!(older.grid, before, "and the grid was not touched");
        assert_eq!(older.tiles.overview.len(), tiles, "nor the waveform");
        assert!(!older.stale.any(), "and it is agreed with now");

        // Nothing stale is nothing to do.
        assert!(!repair(&mut older, &pcm, None));
    }

    /// A sidecar written before the products were versioned says nothing
    /// about them, and "nothing" has to mean "measure it all again".
    #[test]
    fn a_sidecar_that_never_said_is_treated_as_stale() {
        let pcm = click_track(48_000, 128.0, 8.0, 0.0);
        let mut bytes = encode_analysis(&analyze(&pcm));
        let tail = bytes.len() - 10;
        bytes[tail..].fill(0);
        let back = decode_analysis(&bytes).expect("a round trip");
        assert!(back.stale.grid && back.stale.key && back.stale.loudness);
    }

    #[test]
    fn a_record_can_be_forgotten_on_its_own() {
        let dir = std::env::temp_dir().join(format!("vj-forget-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mine = AnalysisKey::from_blob(BlobId::hash_of(b"forget me"));
        let other = AnalysisKey::from_blob(BlobId::hash_of(b"keep me"));
        let analysis = analyze(&click_track(48_000, 128.0, 8.0, 0.0));
        store_cached(&dir, &mine, &analysis);
        store_cached(&dir, &other, &analysis);
        assert!(load_cached_summary(&dir, &mine).is_some());

        assert!(forget_cached(&dir, &mine), "there was something to forget");
        assert!(load_cached_summary(&dir, &mine).is_none(), "and it is gone");
        assert!(load_cached_summary(&dir, &other).is_some(), "and only it");
        assert!(!forget_cached(&dir, &mine), "forgetting it twice is not a lie");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_summary_comes_off_disk_for_a_stored_track() {
        let pcm = click_track(48_000, 120.0, 8.0, 0.2);
        let mut analysis = analyze(&pcm);
        analysis.key = Some(KeyEstimate { tonic: 4, minor: false, confidence: 0.3 });
        let dir = std::env::temp_dir()
            .join(format!("makepad-vj-summary-{}", std::process::id()));
        let key = AnalysisKey::from_blob(BlobId::hash_of(b"a summarised track"));
        assert!(load_cached_summary(&dir, &key).is_none());
        store_cached(&dir, &key, &analysis);
        let summary = load_cached_summary(&dir, &key).expect("summary off disk");
        assert_eq!(summary.key, analysis.key);
        assert_eq!(summary.grid, analysis.grid);
        let _ = std::fs::remove_dir_all(&dir);
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

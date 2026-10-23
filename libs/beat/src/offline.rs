//! Whole-file mode: a stable, deterministic grid over an entire record.
//!
//! The same onset front end the streaming tracker reads
//! ([`crate::onset::OnsetFrontEnd`]) runs over the whole file; the lane flux
//! is weighted, its local mean removed, and then, because a file has no
//! song changes to follow:
//!
//! 1. **Tempo** — autocorrelation over every lag in the shared range, each
//!    lag scored with its first harmonics; an octave pair is resolved by the
//!    same rule the streaming tracker uses ([`crate::tempo::prefer_fast`]),
//!    with the alternation read off the whole record and a known tempo (a
//!    tag) given its say.
//! 2. **Grid** — parabolic peak interpolation, a comb sweep that pins period
//!    and phase against every beat in the track, and a least-squares refit
//!    against the onsets the grid predicts, growing from the middle of the
//!    record outward. The tempo is then pulled onto a musical value when
//!    doing so walks no beat further than 25 ms ([`snap_tempo`]).
//! 3. **Tempo map** — Ellis's dynamic-programming beat decoder produces a
//!    free beat SEQUENCE; when it explains the onsets markedly better than
//!    the straight line, the sequence is summarised into piecewise-linear
//!    tempo segments. Empty for machine-made records, which is the design.
//! 4. **Downbeat** — where the arrangement changes (phrase boundaries vote
//!    for a bar position), falling back to the loudest kick of the bar.
//! 5. **Confidence** — how well the rulings fit the onsets they predict.
//!
//! An optional external beat tracker (a neural model, when one is
//! installed) can verify the result through [`BeatEventSource`]; the comb
//! grid stays the tempo authority when the two disagree. This crate has no
//! dependency on any model: the caller implements the trait.

use crate::onset::Envelopes;
use crate::tempo::{fold_into_range, selection_weight, MAX_BPM, MIN_BPM};

// ---------------------------------------------------------------------------
// the entry points
// ---------------------------------------------------------------------------

/// What the caller knows about the record before it is measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OfflineOptions {
    /// A tempo the record claims (a `TBPM` tag, a library's own reading).
    /// It decides the octave and nothing else; the record is still measured.
    pub prior_bpm: Option<f64>,
    /// Pull the measured tempo onto a musical value when that walks no beat
    /// further than [`SNAP_PHASE_BUDGET_SECS`] across the record.
    pub snap_tempo: bool,
}

impl Default for OfflineOptions {
    fn default() -> Self {
        OfflineOptions { prior_bpm: None, snap_tempo: true }
    }
}

/// Everything the whole-file pass produces for one record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BeatAnalysis {
    pub duration_secs: f64,
    pub grid: TrackGrid,
    /// A tempo that moves, when the record has one. Empty for nearly every
    /// machine-made record, and the single line in `grid` is then the truth.
    pub tempo_map: TempoMap,
    /// Where the arrangement changes (drops, breaks), source seconds, at
    /// least four seconds apart. The phrase map.
    pub changes_secs: Vec<f64>,
    /// True when an external beat tracker corrected the pulse/downbeat.
    pub refined_by_model: bool,
}

impl BeatAnalysis {
    /// Beat number at `secs` — from the tempo map when the record has one,
    /// from the straight line otherwise.
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

    /// Every beat of the record, in source seconds.
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

    /// Every downbeat of the record, in source seconds.
    pub fn downbeats(&self) -> Vec<f64> {
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
            if at >= 0.0 && self.grid.is_downbeat(beat as i64) {
                out.push(at);
            }
            beat += 1.0;
        }
        out
    }

    /// Correct the pulse and downbeat from an external tracker's events
    /// (see [`refine_grid_with_beats`]). True when the grid changed.
    pub fn refine_with_beats(&mut self, beats_secs: &[f64], downbeats_secs: &[f64]) -> bool {
        let Some(grid) =
            refine_grid_with_beats(&self.grid, self.duration_secs, beats_secs, downbeats_secs)
        else {
            return false;
        };
        self.grid = grid;
        self.refined_by_model = true;
        true
    }
}

/// Beats and downbeats from an external tracker, source seconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BeatEvents {
    pub beats_secs: Vec<f64>,
    pub downbeats_secs: Vec<f64>,
}

/// An optional second opinion — typically a neural beat tracker, available
/// only when its model is installed. Implemented by the caller so this crate
/// never depends on a model runtime.
pub trait BeatEventSource {
    fn beat_events(&mut self, mono: &[f32], sample_rate: f64) -> Result<BeatEvents, String>;
}

/// Measure a whole mono record.
pub fn analyze(samples: &[f32], sample_rate: f64, options: OfflineOptions) -> BeatAnalysis {
    analyze_with(samples, sample_rate, options, None)
}

/// Measure a whole mono record, then let `verifier` correct the pulse and
/// downbeat when it can. A verifier error keeps the measured grid.
pub fn analyze_with(
    samples: &[f32],
    sample_rate: f64,
    options: OfflineOptions,
    verifier: Option<&mut dyn BeatEventSource>,
) -> BeatAnalysis {
    let sample_rate = sample_rate.max(1.0);
    let envelopes = Envelopes::from_mono(samples, sample_rate);
    let duration_secs = samples.len() as f64 / sample_rate;
    let mut analysis = analyze_envelopes(&envelopes, duration_secs, options);
    if let Some(verifier) = verifier {
        if analysis.grid.has_grid() {
            if let Ok(events) = verifier.beat_events(samples, sample_rate) {
                analysis.refine_with_beats(&events.beats_secs, &events.downbeats_secs);
            }
        }
    }
    analysis
}

/// Measure from envelopes already built (a caller that also draws them).
pub fn analyze_envelopes(
    envelopes: &Envelopes,
    duration_secs: f64,
    options: OfflineOptions,
) -> BeatAnalysis {
    let prior = options.prior_bpm.and_then(fold_into_range);
    let mut grid = estimate_grid(envelopes, prior);
    if options.snap_tempo {
        grid = snap_tempo(grid, duration_secs);
    }
    let tempo_map = if grid.has_grid() {
        let hop_rate = envelopes.hop_rate();
        build_tempo_map(
            envelopes,
            grid.beat_secs * hop_rate,
            grid.first_beat_secs * hop_rate - HOP_CENTRE,
        )
    } else {
        TempoMap::default()
    };
    let hop_secs = envelopes.hop_secs();
    let changes_secs =
        structural_changes(envelopes).into_iter().map(|hop| hop * hop_secs).collect();
    BeatAnalysis {
        duration_secs,
        grid,
        tempo_map,
        changes_secs,
        refined_by_model: false,
    }
}

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

/// Comb energy of the envelope at `period` hops starting at `offset`.
pub(crate) fn comb_energy(onset: &[f32], period: f64, offset: f64) -> f64 {
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
    let hop_rate = envelopes.hop_rate();
    if onset.len() < (4.0 * hop_rate) as usize {
        return TrackGrid::default();
    }
    let min_lag = (60.0 * hop_rate / MAX_BPM).floor().max(2.0) as usize;
    let max_lag = ((60.0 * hop_rate / MIN_BPM).ceil() as usize).min(onset.len() / 3);
    if max_lag <= min_lag {
        return TrackGrid::default();
    }

    // The correlation reads a copy with each onset smeared over its
    // neighbours: a transient lands in one 10 ms cell, the beat period is
    // not a whole number of cells, and without the smear a lag one cell off
    // the period sees half its beats miss — which is how a click at 174
    // lost to its own half. The comb and the refit below read the sharp
    // envelope.
    let smeared: Vec<f32> = (0..onset.len())
        .map(|i| {
            let before = if i > 0 { onset[i - 1] } else { 0.0 };
            let after = onset.get(i + 1).copied().unwrap_or(0.0);
            onset[i] + 0.5 * (before + after)
        })
        .collect();
    // Chosen by correlation times the shared selection weight: a mild
    // preference around 120 BPM (and a known tempo's say), which is what
    // keeps a kick-snare groove at 90 from reading as its beat-and-a-half
    // grouping at 60.
    let bpm_of = |lag: usize| 60.0 * hop_rate / lag as f64;
    let mut scores = vec![0.0f32; max_lag + 1];
    let mut weighted = vec![0.0f64; max_lag + 1];
    let mut best_lag = min_lag;
    for lag in min_lag..=max_lag {
        scores[lag] = tempo_score(&smeared, lag);
        weighted[lag] = scores[lag] as f64 * selection_weight(bpm_of(lag), prior_bpm);
        if weighted[lag] > weighted[best_lag] {
            best_lag = lag;
        }
    }
    let best_score = weighted[best_lag];
    if best_score <= 0.0 {
        return TrackGrid::default();
    }

    // Triple relatives: a groove whose kicks fall a beat and a half apart
    // correlates as well at a beat and a half as at the beat, and no
    // halving or doubling gets from one to the other. The comb decides:
    // the beat is the reading whose rulings land on the STRONGER onsets on
    // average — a grid at the wrong triple relative spends two of every
    // three rulings on hats.
    let best_lag = {
        let salience = |(period, offset): (f64, f64)| -> f64 {
            let start = (offset - HOP_CENTRE).rem_euclid(period);
            let rulings = ((onset.len() as f64 - start) / period).ceil().max(1.0);
            comb_energy(onset, period, start) / rulings
        };
        let located = locate(onset, &scores, best_lag, min_lag, max_lag);
        let mut chosen = best_lag;
        let mut value = salience(located) * selection_weight(60.0 * hop_rate / located.0, prior_bpm);
        for ratio in [2.0 / 3.0, 1.5] {
            let lag = (best_lag as f64 * ratio).round() as usize;
            if lag < min_lag || lag > max_lag || weighted[lag] < 0.6 * best_score {
                continue;
            }
            let located = locate(onset, &scores, lag, min_lag, max_lag);
            let candidate =
                salience(located) * selection_weight(60.0 * hop_rate / located.0, prior_bpm);
            if candidate > value {
                chosen = lag;
                value = candidate;
            }
        }
        chosen
    };
    let best_score = weighted[best_lag];

    // Octave check: half and double time are both real peaks in any
    // autocorrelation. A competitive octave mate (or any mate at all, when a
    // known tempo is there to say which) is resolved by the shared rule: the
    // musical preference, how alike the fast reading's alternate beats are
    // over the whole record, and the prior.
    let competitive = 0.5;
    let mate = |lag: usize| -> bool {
        lag >= min_lag && lag <= max_lag && weighted[lag] >= competitive * best_score
    };
    // Half a lag is rarely a whole cell: the better of the two either side.
    let half = {
        let (low, high) = (best_lag / 2, (best_lag + 1) / 2);
        let score = |lag: usize| weighted.get(lag).copied().unwrap_or(0.0);
        if score(high) > score(low) { high } else { low }
    };
    let mut chosen = best_lag;
    let mut located = None;
    // Faster first: if the fast mate wins, the slower one is not asked.
    if mate(half) {
        let fast = half;
        let (fast_located, balance) =
            locate_with_balance(onset, &envelopes.flux, &scores, fast, min_lag, max_lag);
        if prefer_octave_fast(hop_rate, fast_located.0, balance, prior_bpm) {
            chosen = fast;
            located = Some(fast_located);
        }
    }
    if chosen == best_lag && mate(best_lag.saturating_mul(2)) {
        let (fast_located, balance) =
            locate_with_balance(onset, &envelopes.flux, &scores, best_lag, min_lag, max_lag);
        if prefer_octave_fast(hop_rate, fast_located.0, balance, prior_bpm) {
            located = Some(fast_located);
        } else {
            chosen = best_lag * 2;
        }
    }
    let (period, offset) =
        located.unwrap_or_else(|| locate(onset, &scores, chosen, min_lag, max_lag));

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
        first_beat_secs: offset * envelopes.hop_secs(),
        downbeat_phase,
        confidence,
    }
}

/// Period and anchor (hops, anchor at a hop centre) for the reading at
/// `lag`: parabolic interpolation of the score peak, then a comb sweep in a
/// narrow band around it — the comb is what actually pins the period,
/// because it sees every beat in the track rather than a correlation
/// average — then the least-squares refit against the predicted onsets.
fn locate(onset: &[f32], scores: &[f32], lag: usize, min_lag: usize, max_lag: usize) -> (f64, f64) {
    let mut period = lag as f64;
    if lag > min_lag && lag < max_lag {
        let left = scores[lag - 1] as f64;
        let centre = scores[lag] as f64;
        let right = scores[lag + 1] as f64;
        let denominator = left - 2.0 * centre + right;
        if denominator.abs() > 1e-9 {
            let shift = 0.5 * (left - right) / denominator;
            if shift.abs() < 1.0 {
                period = lag as f64 + shift;
            }
        }
    }
    let (period, hop_offset) = refine_comb(onset, period);
    // The comb hands back a hop index; the beat is at that hop's centre.
    let seed_offset = hop_offset + HOP_CENTRE;
    refine_grid(onset, period, seed_offset).unwrap_or((period, seed_offset))
}

/// [`locate`] the FAST reading of an octave pair, and measure how alike its
/// alternate beats are over the whole record: the weaker of the even and
/// odd beats' comb energy over the stronger. Measured on the refined fast
/// grid because only a grid pinned to a part in a million stays on its
/// beats for a whole record.
fn locate_with_balance(
    onset: &[f32],
    flux: &[f32],
    scores: &[f32],
    fast_lag: usize,
    min_lag: usize,
    max_lag: usize,
) -> ((f64, f64), f64) {
    let (period, offset) = locate(onset, scores, fast_lag, min_lag, max_lag);
    let comb = offset - HOP_CENTRE;
    let even = comb_energy(flux, period * 2.0, comb.rem_euclid(period * 2.0));
    let odd = comb_energy(flux, period * 2.0, (comb + period).rem_euclid(period * 2.0));
    let balance = if even.max(odd) <= 1e-9 { 0.0 } else { even.min(odd) / even.max(odd) };
    ((period, offset), balance)
}

/// The shared octave rule ([`crate::tempo::prefer_fast`]) for a fast
/// reading of `fast_period` hops.
fn prefer_octave_fast(hop_rate: f64, fast_period: f64, balance: f64, prior: Option<f64>) -> bool {
    let fast_bpm = 60.0 * hop_rate / fast_period;
    let slow_bpm = fast_bpm * 0.5;
    if fast_bpm > MAX_BPM {
        return false;
    }
    if slow_bpm < MIN_BPM {
        return true;
    }
    crate::tempo::prefer_fast(fast_bpm, slow_bpm, balance, prior)
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
pub(crate) fn structural_changes(envelopes: &Envelopes) -> Vec<f64> {
    let loudness = &envelopes.loudness;
    let hop_secs = envelopes.hop_secs();
    // Two seconds either side: a single bar of silence must not register,
    // an arrangement change must.
    let window = (2.0 / hop_secs) as usize;
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

    let spacing = PHRASE_SPACING_SECS / hop_secs;
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
        // A change is found at the first transient of the layer that
        // arrives, which is ON the bar line or shortly after it (a hat layer
        // entering on its first off-beat sixteenth), never before. So a
        // change up to 0.45 of a beat late belongs to the beat before it, one
        // up to a quarter early to the beat after, and anything else falls
        // between beats and says nothing about which beat starts the bar.
        let into = beat - beat.floor();
        let on = if into <= 0.45 {
            beat.floor()
        } else if into >= 0.75 {
            beat.ceil()
        } else {
            continue;
        };
        votes[(on as i64).rem_euclid(4) as usize] += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

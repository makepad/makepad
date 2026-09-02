//! What a planner needs to know about a record, folded out of the analysis
//! it already has.
//!
//! Nothing here re-reads audio or adds a pass. The waveform tiles the
//! analysis already computes, caches and hands to the deck carry all of it;
//! this module is the arithmetic that turns them into the two or three
//! numbers a picker can compare records by.
//!
//! One property of that input decides the whole shape below. The three band
//! bytes of the zoom tiles are scaled to each record's own loudest moment,
//! so they say what a column is MADE of and never how loud it is against
//! another record. The overview's second byte is not scaled: it is a plain
//! log of the column's own level. So anything cross-track comes from the
//! overview, and the bands are only ever asked about a record relative to
//! itself.

use crate::wave_analysis::{TempoMap, TrackGrid};

/// Loudness byte at or below which a column is silence rather than signal.
/// The same floor the intro/outro shape uses, for the same reason: a record
/// fading out is not a record playing quietly.
const FLOOR: u8 = 6;

/// How far a record's tempo may wander before a long beatmatched blend is
/// no longer a good bet, as a share of its own tempo. Ours to measure.
const DRIFT_LIMIT: f64 = 0.10;

/// Where in the body the loud half is taken from, when asking how hot a
/// record runs at its peak rather than on average.
const HOT_PERCENTILE: f64 = 0.90;

/// How hot a record runs, on a scale two different records can be compared
/// on: 0.0 for silence, 1.0 for a record mastered into the ceiling.
///
/// Half the body's average and half its loud end, so a record that is
/// mostly a breakdown with one enormous drop does not read as hot as one
/// that runs hard the whole way, and neither does a uniformly loud record
/// read as cold because it never peaks.
///
/// Silence at either end is left out. A record with a minute of fade is not
/// a quieter record, and including the fade would say it was.
pub fn comparable_energy(overview: &[[u8; 2]]) -> f32 {
    let mut body: Vec<u8> =
        overview.iter().map(|column| column[1]).filter(|loud| *loud > FLOOR).collect();
    if body.is_empty() {
        return 0.0;
    }
    let total: u32 = body.iter().map(|loud| *loud as u32).sum();
    let mean = total as f64 / body.len() as f64;
    body.sort_unstable();
    let hot = body[((body.len() as f64 * HOT_PERCENTILE) as usize).min(body.len() - 1)];
    (((mean + hot as f64) / 2.0) / 255.0).clamp(0.0, 1.0) as f32
}

/// How much a transition may lean on this record's grid: 0.0 when there is
/// nothing to lean on, 1.0 when the grid is confident and the tempo holds
/// still.
///
/// Two different doubts, and only one of them is the detector's. A record
/// with a fine grid whose tempo wanders — played by people rather than made
/// by a machine — will drift out of a long blend however sure the detector
/// was about where the beats are, so the fitted tempo map has a vote.
pub fn grid_trust(grid: &TrackGrid, tempo: &TempoMap) -> f32 {
    if !grid.has_grid() {
        return 0.0;
    }
    let confidence = grid.confidence.clamp(0.0, 1.0);
    if tempo.is_empty() {
        // No map is the usual answer and the confident one: the analysis
        // fits one only where a straight line measurably failed.
        return confidence;
    }
    let slowest = tempo
        .segments
        .iter()
        .map(|segment| segment.period_secs)
        .fold(f64::MIN, f64::max);
    let fastest = tempo
        .segments
        .iter()
        .map(|segment| segment.period_secs)
        .fold(f64::MAX, f64::min);
    if !(fastest > 0.0) || !slowest.is_finite() {
        return confidence;
    }
    let wander = slowest / fastest - 1.0;
    let steady = (1.0 - wander / DRIFT_LIMIT).clamp(0.0, 1.0) as f32;
    confidence * steady
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An overview whose body sits at `loud`, with `silent` columns of
    /// nothing at each end.
    fn overview(loud: u8, body: usize, silent: usize) -> Vec<[u8; 2]> {
        let mut out = vec![[0u8, 0u8]; silent];
        out.extend(std::iter::repeat([loud, loud]).take(body));
        out.extend(std::iter::repeat([0u8, 0u8]).take(silent));
        out
    }

    fn grid(confidence: f32) -> TrackGrid {
        TrackGrid {
            bpm: 120.0,
            beat_secs: 0.5,
            first_beat_secs: 0.0,
            downbeat_phase: 0,
            confidence,
        }
    }

    fn tempo(periods: &[f64]) -> TempoMap {
        TempoMap {
            segments: periods
                .iter()
                .enumerate()
                .map(|(index, period)| crate::wave_analysis::TempoSegment {
                    start_secs: index as f64 * 10.0,
                    start_beat: index as f64 * 20.0,
                    period_secs: *period,
                })
                .collect(),
        }
    }

    #[test]
    fn a_loud_record_reads_hotter_than_a_quiet_one() {
        // The whole point of this number: it has to order two DIFFERENT
        // records, which is exactly what the per-track scaled band bytes
        // cannot do.
        let quiet = comparable_energy(&overview(60, 400, 0));
        let loud = comparable_energy(&overview(200, 400, 0));
        assert!(loud > quiet, "{loud} should beat {quiet}");
        assert!(quiet > 0.0 && loud <= 1.0);
    }

    #[test]
    fn silence_at_either_end_does_not_cool_the_record() {
        // A record with a long fade is not a quieter record. Averaging the
        // fade in would say it was, and every record with a tail would read
        // colder than the same music without one.
        let bare = comparable_energy(&overview(180, 400, 0));
        let padded = comparable_energy(&overview(180, 400, 300));
        assert!((bare - padded).abs() < 1e-6, "{bare} vs {padded}");
    }

    #[test]
    fn nothing_to_measure_is_cold_rather_than_a_panic() {
        assert_eq!(comparable_energy(&[]), 0.0);
        assert_eq!(comparable_energy(&overview(0, 100, 0)), 0.0);
    }

    #[test]
    fn a_record_with_no_grid_is_never_trusted() {
        let none = TrackGrid::default();
        assert!(!none.has_grid());
        assert_eq!(grid_trust(&none, &TempoMap::default()), 0.0);
    }

    #[test]
    fn a_steady_record_is_trusted_as_far_as_its_detector_was() {
        let trust = grid_trust(&grid(0.8), &TempoMap::default());
        assert!((trust - 0.8).abs() < 1e-6, "{trust}");
    }

    #[test]
    fn a_wandering_tempo_is_trusted_less_than_a_steady_one() {
        // A drummer who drifts will walk out of a long blend however sure
        // the detector was about where the beats are.
        let steady = grid_trust(&grid(1.0), &tempo(&[0.5, 0.5, 0.5]));
        let wandering = grid_trust(&grid(1.0), &tempo(&[0.5, 0.52, 0.54]));
        assert!((steady - 1.0).abs() < 1e-6, "a map that does not move: {steady}");
        assert!(wandering < steady, "{wandering} should sit under {steady}");
        // Far enough gone and there is nothing left to lean on.
        assert_eq!(grid_trust(&grid(1.0), &tempo(&[0.5, 0.75])), 0.0);
    }
}

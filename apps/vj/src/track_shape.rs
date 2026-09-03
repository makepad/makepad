//! Where a track's intro ends and its outro starts — the autopilot's map
//! of one record.
//!
//! Pure arithmetic over data the deck already holds: the 2048-column
//! loudness overview the analyser produces and the beat grid. The energy
//! detection is relative to the track's own loudness distribution, so the
//! overview byte's absolute scale cancels out; every grid call is gated on
//! `has_grid()` because `secs_at_beat` on a default grid silently returns
//! 0.0. When the envelope is unusable the shape falls back to duration
//! fractions in SECONDS — the fallback exists precisely for tracks with no
//! bars to count.

use crate::wave_analysis::{SoundSpan, TrackGrid};

/// Which of the shape's four edges a hand is moving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeEdge {
    IntroStart,
    IntroEnd,
    OutroStart,
    OutroEnd,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackShape {
    /// Source seconds. Start of the intro: where the file first sounds.
    pub intro_start_secs: f64,
    /// Source seconds. End of the intro: where the body's energy arrives.
    pub intro_end_secs: f64,
    /// Source seconds. Start of the outro: where the body's energy leaves.
    pub outro_start_secs: f64,
    /// Source seconds. End of the outro: where the file last sounds.
    pub outro_end_secs: f64,
    /// The detection ran on real signal (false = duration-fraction fallback).
    pub detected: bool,
}

impl TrackShape {
    pub fn edge(&self, edge: ShapeEdge) -> f64 {
        match edge {
            ShapeEdge::IntroStart => self.intro_start_secs,
            ShapeEdge::IntroEnd => self.intro_end_secs,
            ShapeEdge::OutroStart => self.outro_start_secs,
            ShapeEdge::OutroEnd => self.outro_end_secs,
        }
    }

    /// One edge moved by hand, validated: ordered, inside the track, and a
    /// number that is one.
    ///
    /// Deliberately NOT the detector's own rails. Those cap the intro at a
    /// fraction of the track and hold the outro off its end, which are
    /// sanity checks on a MEASUREMENT -- a hand that wants the intro at
    /// sixty percent is not making a mistake, it is making a decision.
    /// The only rules left are the ones that keep the shape a shape.
    pub fn with_edge(self, edge: ShapeEdge, secs: f64, duration: f64) -> Option<TrackShape> {
        if !secs.is_finite() || !duration.is_finite() || duration <= 0.0 {
            return None;
        }
        let secs = secs.clamp(0.0, duration);
        let mut out = self;
        match edge {
            ShapeEdge::IntroStart => out.intro_start_secs = secs,
            ShapeEdge::IntroEnd => out.intro_end_secs = secs,
            ShapeEdge::OutroStart => out.outro_start_secs = secs,
            ShapeEdge::OutroEnd => out.outro_end_secs = secs,
        }
        let ordered = out.intro_start_secs <= out.intro_end_secs
            && out.intro_end_secs <= out.outro_start_secs
            && out.outro_start_secs <= out.outro_end_secs;
        ordered.then_some(out)
    }
}

/// Loudness byte below which a column is treated as silence; a track whose
/// 80th percentile sits under this has no usable envelope at all.
const FLOOR: u8 = 6;
const BODY_PERCENTILE: f64 = 0.80;
const THRESH_FRACTION: f64 = 0.55;
/// A run above threshold must sustain this long to count as "the body".
const MIN_RUN_SECS: f64 = 2.0;
/// Anything shorter than this between intro-end and outro-start is a
/// degenerate detection, not a song structure.
const MIN_BODY_SECS: f64 = 8.0;
const INTRO_MAX_FRACTION: f64 = 0.40;
const OUTRO_MIN_FRACTION: f64 = 0.50;
const FALLBACK_INTRO_SECS: f64 = 30.0;
const FALLBACK_INTRO_FRACTION: f64 = 0.25;
const FALLBACK_OUTRO_SECS: f64 = 60.0;

pub fn track_shape(
    overview: &[[u8; 2]],
    duration_secs: f64,
    grid: &TrackGrid,
    sound: Option<SoundSpan>,
) -> TrackShape {
    if duration_secs <= 0.0 || overview.is_empty() {
        return TrackShape {
            intro_start_secs: 0.0,
            intro_end_secs: 0.0,
            outro_start_secs: 0.0,
            outro_end_secs: 0.0,
            detected: false,
        };
    }
    let col_secs = duration_secs / overview.len() as f64;
    let mut loud: Vec<u8> = overview.iter().map(|c| c[1]).collect();
    loud.sort_unstable();
    let body = loud[((loud.len() as f64 * BODY_PERCENTILE) as usize).min(loud.len() - 1)];
    if body < FLOOR {
        return fallback(duration_secs, grid).with_sound(sound, duration_secs);
    }
    let thresh = ((body as f64 * THRESH_FRACTION) as u8).max(FLOOR);
    let min_run = ((MIN_RUN_SECS / col_secs).ceil() as usize).max(1);
    let Some((first_start, last_end)) = body_extent(overview, thresh, min_run) else {
        return fallback(duration_secs, grid).with_sound(sound, duration_secs);
    };
    let intro_end = (first_start as f64 * col_secs).min(INTRO_MAX_FRACTION * duration_secs);
    let outro_start = (last_end as f64 * col_secs).max(OUTRO_MIN_FRACTION * duration_secs);
    if outro_start - intro_end < MIN_BODY_SECS {
        return fallback(duration_secs, grid).with_sound(sound, duration_secs);
    }
    finish(intro_end, outro_start, true, duration_secs, grid).with_sound(sound, duration_secs)
}

/// `(start_col_of_first_run, end_col_of_last_run)` for runs of at least
/// `min_run` columns whose loudness exceeds `thresh`.
fn body_extent(overview: &[[u8; 2]], thresh: u8, min_run: usize) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = None;
    let mut run_start = None;
    for (col, cell) in overview.iter().enumerate() {
        if cell[1] > thresh {
            if run_start.is_none() {
                run_start = Some(col);
            }
        } else if let Some(start) = run_start.take() {
            if col - start >= min_run {
                first.get_or_insert(start);
                last = Some(col);
            }
        }
    }
    if let Some(start) = run_start {
        if overview.len() - start >= min_run {
            first.get_or_insert(start);
            last = Some(overview.len());
        }
    }
    Some((first?, last?))
}

impl TrackShape {
    /// The two OUTER edges: where the recording itself starts and stops.
    /// Without a measurement they are the whole file, which is the honest
    /// answer and the one every reader had before the scan existed.
    fn with_sound(mut self, sound: Option<SoundSpan>, duration: f64) -> TrackShape {
        let (first, last) = match sound {
            Some(span) => (span.first_secs, span.last_secs),
            None => (0.0, duration),
        };
        // The SCAN wins where the two disagree: the envelope cannot be
        // loud before there is any sound, so a body that claims to start
        // before the first sample makes a noise is the measurement that is
        // wrong. The inner pair is pushed to keep the four in order.
        self.intro_start_secs = first.clamp(0.0, duration);
        self.intro_end_secs = self.intro_end_secs.clamp(self.intro_start_secs, duration);
        self.outro_end_secs = last.clamp(self.intro_end_secs, duration);
        self.outro_start_secs =
            self.outro_start_secs.clamp(self.intro_end_secs, self.outro_end_secs);
        self
    }
}

fn fallback(duration: f64, grid: &TrackGrid) -> TrackShape {
    finish(
        FALLBACK_INTRO_SECS.min(FALLBACK_INTRO_FRACTION * duration),
        (OUTRO_MIN_FRACTION * duration).max(duration - FALLBACK_OUTRO_SECS),
        false,
        duration,
        grid,
    )
}

fn finish(
    intro_end: f64,
    outro_start: f64,
    detected: bool,
    duration: f64,
    grid: &TrackGrid,
) -> TrackShape {
    let mut intro = intro_end.max(0.0);
    let mut outro = outro_start;
    if grid.has_grid() {
        intro = nearest_downbeat(grid, intro).clamp(0.0, INTRO_MAX_FRACTION * duration);
        outro = nearest_downbeat(grid, outro).max(OUTRO_MIN_FRACTION * duration);
    }
    outro = outro.min(duration - 1.0).max(0.0);
    if intro >= outro {
        // Snapping (or a very short track) broke the ordering: fall back to
        // plain fractions, unsnapped — order beats bar alignment.
        intro = FALLBACK_INTRO_SECS
            .min(FALLBACK_INTRO_FRACTION * duration)
            .min(outro.max(0.0));
        let outro = outro.max(intro);
        return TrackShape {
            intro_start_secs: 0.0,
            intro_end_secs: intro,
            outro_start_secs: outro,
            outro_end_secs: duration.max(outro),
            detected: false,
        };
    }
    TrackShape {
        intro_start_secs: 0.0,
        intro_end_secs: intro,
        outro_start_secs: outro,
        outro_end_secs: duration.max(outro),
        detected,
    }
}

/// The downbeat nearest `secs` — the idiom from decks.rs `plan_sync`.
/// Callers gate on `has_grid()`; `secs_at_beat` has no guard of its own.
fn nearest_downbeat(grid: &TrackGrid, secs: f64) -> f64 {
    let bar = grid.bar_at(secs).round();
    grid.secs_at_beat(bar * 4.0 - grid.downbeat_phase as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shaped() -> TrackShape {
        TrackShape {
            intro_start_secs: 2.0,
            intro_end_secs: 20.0,
            outro_start_secs: 200.0,
            outro_end_secs: 240.0,
            detected: true,
        }
    }

    #[test]
    fn a_hand_may_put_an_edge_where_the_detector_never_would() {
        // The detector's own rails cap the intro at a fraction of the
        // track and hold the outro off its end. Those are sanity checks on
        // a MEASUREMENT: a hand that wants the intro at sixty percent is
        // making a decision, not a mistake.
        let moved = shaped().with_edge(ShapeEdge::IntroEnd, 180.0, 300.0).expect("allowed");
        assert!((moved.intro_end_secs - 180.0).abs() < 1e-9);
        assert!((moved.outro_start_secs - 200.0).abs() < 1e-9, "the others hold");
        let moved = shaped().with_edge(ShapeEdge::OutroEnd, 300.0, 300.0).expect("allowed");
        assert!((moved.outro_end_secs - 300.0).abs() < 1e-9, "right to the end");
    }

    #[test]
    fn an_edge_that_would_leave_the_four_out_of_order_is_refused() {
        assert!(shaped().with_edge(ShapeEdge::IntroEnd, 210.0, 300.0).is_none());
        assert!(shaped().with_edge(ShapeEdge::IntroStart, 30.0, 300.0).is_none());
        assert!(shaped().with_edge(ShapeEdge::OutroStart, 10.0, 300.0).is_none());
        assert!(shaped().with_edge(ShapeEdge::OutroEnd, 100.0, 300.0).is_none());
        // Touching neighbours is allowed: a zero-length intro is a track
        // that starts in its body.
        assert!(shaped().with_edge(ShapeEdge::IntroStart, 20.0, 300.0).is_some());
    }

    #[test]
    fn an_edge_is_kept_inside_the_track_and_a_number_that_is_not_one_moves_nothing() {
        let moved = shaped().with_edge(ShapeEdge::OutroEnd, 1e9, 300.0).expect("clamped in");
        assert!((moved.outro_end_secs - 300.0).abs() < 1e-9);
        let moved = shaped().with_edge(ShapeEdge::IntroStart, -5.0, 300.0).expect("clamped in");
        assert!(moved.intro_start_secs.abs() < 1e-9);
        assert!(shaped().with_edge(ShapeEdge::IntroEnd, f64::NAN, 300.0).is_none());
        assert!(shaped().with_edge(ShapeEdge::IntroEnd, 10.0, 0.0).is_none());
    }

    #[test]
    fn the_outer_edges_come_from_the_sound_scan_and_fall_back_to_the_whole_file() {
        let overview = vec![[0u8, 200u8]; 300];
        let grid = TrackGrid::default();
        let plain = track_shape(&overview, 300.0, &grid, None);
        assert!(plain.intro_start_secs.abs() < 1e-9, "no scan: the file's own start");
        assert!((plain.outro_end_secs - 300.0).abs() < 1e-9, "and its own end");

        let scanned = track_shape(
            &overview,
            300.0,
            &grid,
            Some(crate::wave_analysis::SoundSpan { first_secs: 3.5, last_secs: 290.0 }),
        );
        assert!((scanned.intro_start_secs - 3.5).abs() < 1e-9);
        assert!((scanned.outro_end_secs - 290.0).abs() < 1e-9);
        // And the four are ordered whatever the scan says.
        let odd = track_shape(
            &overview,
            300.0,
            &grid,
            Some(crate::wave_analysis::SoundSpan { first_secs: 299.0, last_secs: 1.0 }),
        );
        assert!(odd.intro_start_secs <= odd.intro_end_secs);
        assert!(odd.outro_start_secs <= odd.outro_end_secs);
    }


    fn grid(bpm: f64) -> TrackGrid {
        TrackGrid {
            bpm,
            beat_secs: 60.0 / bpm,
            first_beat_secs: 0.0,
            downbeat_phase: 0,
            confidence: 0.9,
        }
    }

    /// A track shaped quiet / loud / quiet on the 2048-column overview grid.
    fn shaped_overview(duration: f64, intro: f64, outro: f64) -> Vec<[u8; 2]> {
        (0..2048)
            .map(|col| {
                let t = duration * col as f64 / 2048.0;
                let loud = if t >= intro && t < outro { 180 } else { 20 };
                [loud, loud]
            })
            .collect()
    }

    #[test]
    fn a_quiet_intro_and_outro_are_found_within_a_column() {
        let overview = shaped_overview(300.0, 30.0, 270.0);
        let shape = track_shape(&overview, 300.0, &TrackGrid::default(), None);
        assert!(shape.detected);
        let col = 300.0 / 2048.0;
        assert!(
            (shape.intro_end_secs - 30.0).abs() < col * 2.0,
            "intro at {}",
            shape.intro_end_secs
        );
        assert!(
            (shape.outro_start_secs - 270.0).abs() < col * 2.0,
            "outro at {}",
            shape.outro_start_secs
        );
    }

    #[test]
    fn the_snap_moves_both_edges_onto_downbeats() {
        let overview = shaped_overview(300.0, 31.3, 268.9);
        let grid = grid(120.0); // bar = 2 s
        let shape = track_shape(&overview, 300.0, &grid, None);
        assert!(shape.detected);
        let bar = 2.0;
        let intro_bars = shape.intro_end_secs / bar;
        let outro_bars = shape.outro_start_secs / bar;
        assert!(
            (intro_bars - intro_bars.round()).abs() < 1e-6,
            "intro {} is not on a downbeat",
            shape.intro_end_secs
        );
        assert!(
            (outro_bars - outro_bars.round()).abs() < 1e-6,
            "outro {} is not on a downbeat",
            shape.outro_start_secs
        );
    }

    #[test]
    fn silence_falls_back_to_duration_fractions() {
        let overview = vec![[0u8, 0u8]; 2048];
        let shape = track_shape(&overview, 300.0, &TrackGrid::default(), None);
        assert!(!shape.detected);
        assert!((shape.intro_end_secs - 30.0).abs() < 1e-9);
        assert!((shape.outro_start_secs - 240.0).abs() < 1e-9);
    }

    #[test]
    fn a_flat_loud_track_is_clamped_into_a_legal_shape() {
        // Loud from the first sample to the last: no quiet edges to find,
        // so the sanity clamps are the whole answer — the intro may not eat
        // past 40%, the outro may not start before 50% or after the last
        // second.
        let overview = vec![[200u8, 200u8]; 2048];
        let shape = track_shape(&overview, 300.0, &TrackGrid::default(), None);
        assert!(shape.intro_end_secs <= 0.40 * 300.0 + 1e-9);
        assert!(shape.outro_start_secs >= 0.50 * 300.0 - 1e-9);
        assert!(shape.outro_start_secs <= 299.0 + 1e-9);
        assert!(shape.intro_end_secs < shape.outro_start_secs);
    }

    #[test]
    fn a_no_grid_track_never_snaps_and_still_orders_its_edges() {
        let overview = shaped_overview(90.0, 10.0, 80.0);
        let shape = track_shape(&overview, 90.0, &TrackGrid::default(), None);
        assert!(shape.intro_end_secs < shape.outro_start_secs);
        assert!(shape.outro_start_secs <= 89.0 + 1e-9);
    }

    #[test]
    fn a_transient_blip_is_not_a_body() {
        // A half-second hit inside the intro must not drag intro_end
        // earlier: runs shorter than MIN_RUN_SECS are noise, not structure.
        let mut overview = shaped_overview(300.0, 30.0, 270.0);
        overview[34] = [255, 255]; // ~5 s in, one 0.15 s column
        let shape = track_shape(&overview, 300.0, &TrackGrid::default(), None);
        assert!(shape.detected);
        assert!(
            (shape.intro_end_secs - 30.0).abs() < 0.5,
            "the blip pulled the intro to {}",
            shape.intro_end_secs
        );
    }

    #[test]
    fn an_empty_overview_or_zero_duration_is_inert() {
        let shape = track_shape(&[], 300.0, &TrackGrid::default(), None);
        assert!(!shape.detected);
        let overview = shaped_overview(300.0, 30.0, 270.0);
        let shape = track_shape(&overview, 0.0, &TrackGrid::default(), None);
        assert!(!shape.detected);
    }
}

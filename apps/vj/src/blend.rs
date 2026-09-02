//! Transition choreography: the arithmetic of mixing like a hand.
//!
//! Pure and clock-free. The autopilot decides WHEN a transition runs; this
//! module decides WHAT the hands do during it — which medium carries the
//! blend, when the basslines swap, where the fire point lands so it
//! respects the track's phrases and neither record sings over the other.
//! Everything here returns data (steps, times); the autopilot emits them
//! as commands and the mixer's blend overlay performs them.
//!
//! Units are explicit at every boundary: `_wall` is seconds of listening
//! time from the fade's start, `_src` is the OUT deck's source seconds.
//! The caller converts between them with the rates it observed — this
//! module never sees a rate.

/// The operator's ceiling for how smart a transition may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MixBrain {
    /// Plain equal-power volume crossfade.
    Fade,
    /// Bass-swap through the 3-band kill EQ.
    Eq,
    /// Bass-stem swap plus vocal ducking, when both decks carry stems.
    #[default]
    Stems,
}

/// What one particular transition actually runs — the ceiling degraded to
/// what the decks can deliver at fire time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medium {
    Fade,
    Eq,
    Stems,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Out,
    In,
}

/// What a step moves. Read a step's value accordingly: a FACTOR on the
/// operator's own setting for the bands and the stems, and an OFFSET from
/// it for the sweep, whose knob is bipolar and has no meaningful factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// One 3-band EQ band (0 = low).
    Band(usize),
    /// One separated stem lane, `STEM_ORDER` indexing.
    Stem(usize),
    /// The sweep filter, as an offset from wherever the hand left it.
    Filter,
}

/// Stem lane indices, pinned to `stems::STEM_ORDER` (vocals, drums, bass,
/// other) by a test below.
pub const VOCALS: usize = 0;
pub const BASS: usize = 2;

/// How far the incoming mid band gives way when the EQ medium has to dodge
/// a singer. Roughly -6 dB: enough to let the outgoing phrase finish on
/// top, and not so much that the incoming record arrives hollow.
pub const EQ_VOCAL_DUCK: f32 = 0.5;

/// One gain move in a transition's schedule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendStep {
    /// Wall seconds after the fade starts (0.0 = with the fade itself; the
    /// autopilot emits In-side 0.0 steps earlier, at cue time, so the
    /// incoming deck never leaks).
    pub at_wall: f64,
    pub role: Role,
    pub lane: Lane,
    pub gain: f32,
}

/// Sung intervals on one deck, source seconds, ascending.
#[derive(Clone, Debug, Default)]
pub struct SungMap(pub Vec<(f64, f64)>);

impl SungMap {
    fn covers(&self, t: f64) -> bool {
        self.0.iter().any(|(start, end)| t >= *start && t < *end)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The medium a transition runs: the brain is a ceiling, never a promise.
/// Stems needs BOTH decks' lanes live; EQ needs nothing.
pub fn medium(brain: MixBrain, out_stems: bool, in_stems: bool) -> Medium {
    match brain {
        MixBrain::Fade => Medium::Fade,
        MixBrain::Eq => Medium::Eq,
        MixBrain::Stems if out_stems && in_stems => Medium::Stems,
        MixBrain::Stems => Medium::Eq,
    }
}

/// The gain schedule for one transition. Empty = a plain fade (either the
/// medium is Fade, or the fade is too short for a swap to breathe).
///
/// The shape both smart media share: the incoming low is held silent from
/// the start; at the swap bar — the bar boundary nearest the fade's middle
/// — the basslines exchange. Stems additionally holds the incoming VOCALS
/// silent until `duck_vocals_until` when the guard asked for it.
pub fn choreography(
    medium: Medium,
    fade_wall: f64,
    bar_wall: f64,
    duck_vocals_until: Option<f64>,
) -> Vec<BlendStep> {
    let bar = bar_wall.max(0.5);
    if fade_wall < 2.0 * bar {
        // A three-second blend has no room for a bass swap.
        return Vec::new();
    }
    let low: Lane = match medium {
        Medium::Fade => return Vec::new(),
        Medium::Eq => Lane::Band(0),
        Medium::Stems => Lane::Stem(BASS),
    };
    // The multiple of a bar nearest the fade's middle, clamped to leave at
    // least one bar either side.
    let swap = ((fade_wall / 2.0 / bar).round() * bar).clamp(bar, fade_wall - bar);
    let mut steps = vec![
        BlendStep { at_wall: 0.0, role: Role::In, lane: low, gain: 0.0 },
        BlendStep { at_wall: swap, role: Role::Out, lane: low, gain: 0.0 },
        BlendStep { at_wall: swap, role: Role::In, lane: low, gain: 1.0 },
    ];
    // The guard measures a clash on every transition, whatever the medium
    // carries the blend. Stems can hold the singer down and nothing else;
    // the EQ can only lean on the band a voice lives in, so it leans rather
    // than kills — the mids carry the whole record.
    if let Some(duck) = duck_vocals_until {
        let (lane, held) = match medium {
            Medium::Stems => (Lane::Stem(VOCALS), 0.0),
            Medium::Eq => (Lane::Band(1), EQ_VOCAL_DUCK),
            Medium::Fade => unreachable!("a plain fade returned above"),
        };
        let release = duck.clamp(bar, fade_wall);
        steps.push(BlendStep { at_wall: 0.0, role: Role::In, lane, gain: held });
        steps.push(BlendStep { at_wall: release, role: Role::In, lane, gain: 1.0 });
    }
    steps.sort_by(|a, b| a.at_wall.total_cmp(&b.at_wall));
    steps
}

/// Where a transition could leave the outgoing record, and how good the
/// moment is.
#[derive(Clone, Debug, PartialEq)]
pub struct Exit {
    /// Source seconds, on a bar line.
    pub at_secs: f64,
    /// 0..=1. Higher is a better place to go.
    pub score: f32,
    /// The longest fade this exit can carry before it runs into whatever
    /// happens next in the outgoing record.
    pub allowance_secs: f64,
    /// Why, for the panel and the log.
    pub reason: String,
}

/// How far into a record the earliest allowed exit sits. A record that has
/// played a third of itself has said what it came to say; leaving before
/// that is cutting it off, whatever the energy is doing.
const LISTENED: f64 = 0.35;

/// Where the lateness penalty starts to bite, as a share of the record.
const LATE: f64 = 0.80;

/// Rank the places this record could be left.
///
/// The candidate set is the bar grid, not the detected change points: the
/// analysis keeps at most a couple of dozen of those, four seconds apart at
/// the closest, which over a six-minute record is one every fifteen seconds
/// and far too coarse to choose an exit from. The changes anchor the grid
/// instead — a bar that lands on one scores for it — and a record that
/// yielded no changes at all still has every bar to offer.
///
/// Empty only when there are no bars, which means no grid.
pub fn exits(
    bars: &[crate::mix_facts::BarRow],
    changes_secs: &[f64],
    duration_secs: f64,
    bar_secs: f64,
) -> Vec<Exit> {
    if bars.is_empty() || !(duration_secs > 0.0) || !(bar_secs > 0.0) {
        return Vec::new();
    }
    let floor = duration_secs * LISTENED;
    let mut out: Vec<Exit> = Vec::new();
    for (index, bar) in bars.iter().enumerate() {
        if bar.start_secs < floor || bar.start_secs >= duration_secs {
            continue;
        }
        let mut why: Vec<&str> = Vec::new();
        let mut score = 0.5f32;

        // Where the record lets go is where a hand would leave. Comparing
        // against the bar behind rather than an absolute level keeps this
        // about the shape of the arrangement and not the mastering.
        let previous = bars[index.saturating_sub(1)].energy;
        let drop = previous - bar.energy;
        if drop > 0.15 {
            score += 0.30 * (drop / 0.5).clamp(0.0, 1.0);
            why.push("the energy lets go");
        } else if drop < -0.15 {
            // Leaving as it climbs throws away the build the record just
            // spent its time on.
            score -= 0.20;
            why.push("still climbing");
        }

        // A bar that lands on something the analysis heard change is worth
        // more than one that merely counts.
        if changes_secs
            .iter()
            .any(|change| (change - bar.start_secs).abs() <= bar_secs * 0.5)
        {
            score += 0.25;
            why.push("on a change the record makes");
        }

        // Late is a risk, not a rule: the further past the point of no
        // return, the less room whatever comes next has to breathe.
        let through = bar.start_secs / duration_secs;
        if through > LATE {
            score -= 0.40 * ((through - LATE) / (1.0 - LATE)) as f32;
            why.push("late in the record");
        }

        if why.is_empty() {
            why.push("nothing in the way");
        }
        out.push(Exit {
            at_secs: bar.start_secs,
            score: score.clamp(0.0, 1.0),
            allowance_secs: allowance(bars, index, duration_secs),
            reason: why.join(", "),
        });
    }
    out.sort_by(|a, b| {
        b.score.total_cmp(&a.score).then(a.at_secs.total_cmp(&b.at_secs))
    });
    out
}

/// How long a fade from this bar can run before the record does something
/// else — the next bar whose energy moves against what it is doing now, or
/// the end of the record.
fn allowance(bars: &[crate::mix_facts::BarRow], from: usize, duration_secs: f64) -> f64 {
    let here = bars[from].energy;
    for bar in bars.iter().skip(from + 1) {
        if (bar.energy - here).abs() > 0.25 {
            return (bar.start_secs - bars[from].start_secs).max(0.0);
        }
    }
    (duration_secs - bars[from].start_secs).max(0.0)
}

/// Move a fire point onto the track's phrase structure: the nearest
/// detected change within two bars wins; failing that, the nearest 8-bar
/// grid line within the same slack; failing both, the point stands.
/// Always clamped into `limit_src`.
pub fn snap_to_phrase(
    fire_src: f64,
    changes_secs: &[f64],
    bar_secs_src: Option<f64>,
    limit_src: (f64, f64),
) -> f64 {
    let bar = bar_secs_src.unwrap_or(2.0);
    let slack = 2.0 * bar;
    let clamp = |t: f64| t.clamp(limit_src.0, limit_src.1.max(limit_src.0));
    let nearest_change = changes_secs
        .iter()
        .copied()
        .filter(|c| (c - fire_src).abs() <= slack)
        .min_by(|a, b| (a - fire_src).abs().total_cmp(&(b - fire_src).abs()));
    if let Some(change) = nearest_change {
        return clamp(change);
    }
    if let Some(bar) = bar_secs_src {
        let phrase = bar * 8.0;
        let snapped = (fire_src / phrase).round() * phrase;
        if (snapped - fire_src).abs() <= slack {
            return clamp(snapped);
        }
    }
    clamp(fire_src)
}

/// Keep two singers out of each other's way. Tries the fire point, then
/// ±1 and ±2 bars, and takes the first candidate where nobody sings over
/// anybody inside the fade window — else the least-overlapping one. When
/// overlap survives, also reports how far into the fade the OUTGOING
/// phrase lasts, so the stems medium can hold the incoming vocals down
/// until it ends.
///
/// Returns `(fire_src, duck_until_wall_offset_src)` — the duck end is an
/// OFFSET from the (new) fire point, in OUT source seconds; the caller
/// divides by the rate for wall time.
pub fn vocal_guard(
    fire_src: f64,
    fade_src: f64,
    out_sung: &SungMap,
    in_sung: &SungMap,
    in_cue_secs: f64,
    bar_secs_src: f64,
    limit_src: (f64, f64),
) -> (f64, Option<f64>) {
    if out_sung.is_empty() || in_sung.is_empty() || fade_src <= 0.0 {
        // One silent side cannot clash.
        return (fire_src, None);
    }
    let clash = |fire: f64| -> f64 {
        // Sample the overlap at 100 ms: both decks singing at the same
        // listening moment is a clash. IN's timeline starts at its cue.
        let mut total = 0.0;
        let step = 0.1;
        let mut offset = 0.0;
        while offset < fade_src {
            if out_sung.covers(fire + offset) && in_sung.covers(in_cue_secs + offset) {
                total += step;
            }
            offset += step;
        }
        total
    };
    let bar = bar_secs_src.max(0.25);
    let candidates = [0.0, -bar, bar, -2.0 * bar, 2.0 * bar];
    let mut best = fire_src.clamp(limit_src.0, limit_src.1.max(limit_src.0));
    let mut best_clash = f64::MAX;
    for shift in candidates {
        let fire = (fire_src + shift).clamp(limit_src.0, limit_src.1.max(limit_src.0));
        let c = clash(fire);
        if c <= 1e-9 {
            return (fire, None);
        }
        if c < best_clash {
            best_clash = c;
            best = fire;
        }
    }
    // Overlap survives every shift: duck until the outgoing phrase that
    // reaches into the fade lets go.
    let duck = out_sung
        .0
        .iter()
        .filter(|(start, end)| *start < best + fade_src && *end > best)
        .map(|(_, end)| end - best)
        .fold(f64::NAN, f64::max);
    let duck = if duck.is_nan() { None } else { Some(duck.clamp(0.0, fade_src)) };
    (best, duck)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lane_indices_match_the_stem_order() {
        assert_eq!(crate::stems::STEM_ORDER[VOCALS], makepad_ai_stems::Stem::Vocals);
        assert_eq!(crate::stems::STEM_ORDER[BASS], makepad_ai_stems::Stem::Bass);
    }

    #[test]
    fn the_brain_is_a_ceiling_never_a_promise() {
        assert_eq!(medium(MixBrain::Fade, true, true), Medium::Fade);
        assert_eq!(medium(MixBrain::Eq, false, false), Medium::Eq);
        assert_eq!(medium(MixBrain::Stems, true, true), Medium::Stems);
        assert_eq!(
            medium(MixBrain::Stems, true, false),
            Medium::Eq,
            "one deck without stems degrades the pair to EQ"
        );
        assert_eq!(medium(MixBrain::Stems, false, true), Medium::Eq);
    }

    #[test]
    fn the_basslines_swap_on_the_bar_nearest_the_middle() {
        // 8 s fade, 2 s bars: the middle IS a bar line — swap at 4.
        let steps = choreography(Medium::Eq, 8.0, 2.0, None);
        assert_eq!(steps.len(), 3);
        assert_eq!(
            steps[0],
            BlendStep { at_wall: 0.0, role: Role::In, lane: Lane::Band(0), gain: 0.0 }
        );
        let swap: Vec<&BlendStep> =
            steps.iter().filter(|s| (s.at_wall - 4.0).abs() < 1e-9).collect();
        assert_eq!(swap.len(), 2, "out-kill and in-release land together");
        // 7 s fade, 2 s bars: middle 3.5 rounds to the bar at 4, clamped
        // inside [2, 5].
        let steps = choreography(Medium::Eq, 7.0, 2.0, None);
        let swap_at = steps.iter().find(|s| s.role == Role::Out).unwrap().at_wall;
        assert!((swap_at - 4.0).abs() < 1e-9, "swap at {swap_at}");
    }

    #[test]
    fn a_short_fade_has_no_room_to_swap() {
        assert!(choreography(Medium::Stems, 3.0, 2.0, None).is_empty());
        assert!(choreography(Medium::Fade, 30.0, 2.0, None).is_empty());
    }

    #[test]
    fn the_stems_medium_ducks_the_incoming_singer_until_released() {
        let steps = choreography(Medium::Stems, 12.0, 2.0, Some(5.0));
        let duck: Vec<&BlendStep> = steps
            .iter()
            .filter(|s| s.lane == Lane::Stem(VOCALS))
            .collect();
        assert_eq!(duck.len(), 2);
        assert!((duck[0].at_wall - 0.0).abs() < 1e-9);
        assert!((duck[0].gain - 0.0).abs() < 1e-6);
        assert!((duck[1].at_wall - 5.0).abs() < 1e-9);
        assert!((duck[1].gain - 1.0).abs() < 1e-6);
        // The release clamps inside the fade.
        let steps = choreography(Medium::Stems, 12.0, 2.0, Some(40.0));
        let release = steps
            .iter()
            .filter(|s| s.lane == Lane::Stem(VOCALS))
            .last()
            .unwrap();
        assert!((release.at_wall - 12.0).abs() < 1e-9);
        // The bass swap uses the stem lane, not the EQ band.
        assert!(steps.iter().any(|s| s.lane == Lane::Stem(BASS)));
        assert!(!steps.iter().any(|s| matches!(s.lane, Lane::Band(_))));
    }

    #[test]
    fn the_eq_medium_ducks_the_incoming_mid_when_the_singers_would_stack() {
        // Without stems there is no vocal lane to hold down, but the guard
        // has still measured a clash and the fire point could not dodge it.
        // A voice lives in the mid band, so the incoming one gives way there
        // — partly, never fully: the mids carry the whole record, and
        // killing them would leave a hole where a duck was wanted.
        let steps = choreography(Medium::Eq, 12.0, 2.0, Some(5.0));
        let duck: Vec<&BlendStep> =
            steps.iter().filter(|s| s.lane == Lane::Band(1)).collect();
        assert_eq!(duck.len(), 2, "a duck and its release");
        assert!((duck[0].at_wall - 0.0).abs() < 1e-9);
        assert!(
            duck[0].gain > 0.0 && duck[0].gain < 1.0,
            "a duck, not a kill: {}",
            duck[0].gain
        );
        assert!((duck[1].at_wall - 5.0).abs() < 1e-9);
        assert!((duck[1].gain - 1.0).abs() < 1e-6);

        // The release clamps inside the fade, as the stem duck does.
        let long = choreography(Medium::Eq, 12.0, 2.0, Some(40.0));
        let release =
            long.iter().filter(|s| s.lane == Lane::Band(1)).last().unwrap();
        assert!((release.at_wall - 12.0).abs() < 1e-9);

        // No clash measured, no duck: an untouched mid band stays untouched.
        let clean = choreography(Medium::Eq, 12.0, 2.0, None);
        assert!(!clean.iter().any(|s| s.lane == Lane::Band(1)));
    }

    #[test]
    fn the_snap_prefers_a_detected_change_over_the_grid() {
        // A change 1.5 s away beats the 8-bar line 2 s away.
        let fire = snap_to_phrase(100.0, &[98.5, 250.0], Some(2.0), (0.0, 300.0));
        assert!((fire - 98.5).abs() < 1e-9, "snapped to {fire}");
        // No change in reach: the 8-bar grid (16 s at 2 s bars) catches a
        // point within two bars of a line.
        let fire = snap_to_phrase(94.0, &[], Some(2.0), (0.0, 300.0));
        assert!((fire - 96.0).abs() < 1e-9, "snapped to {fire}");
        // Nothing in reach: the point stands.
        let fire = snap_to_phrase(88.0, &[40.0], Some(2.0), (0.0, 300.0));
        assert!((fire - 88.0).abs() < 1e-9);
        // No grid, no changes: untouched, still clamped.
        let fire = snap_to_phrase(10.0, &[], None, (20.0, 300.0));
        assert!((fire - 20.0).abs() < 1e-9);
    }

    #[test]
    fn the_guard_finds_the_gap_a_bar_away() {
        // OUT sings 100..104; the fire at 102 clashes, one bar earlier the
        // window 100..108 still clashes, one bar later (104..112) is clean.
        let out = SungMap(vec![(100.0, 104.0)]);
        let incoming = SungMap(vec![(16.0, 30.0)]);
        let (fire, duck) =
            vocal_guard(102.0, 8.0, &out, &incoming, 16.0, 2.0, (0.0, 300.0));
        assert!((fire - 104.0).abs() < 1e-9, "shifted to {fire}");
        assert!(duck.is_none());
    }

    #[test]
    fn an_unavoidable_clash_ducks_until_the_outgoing_phrase_ends() {
        // OUT sings continuously through every candidate window; the best
        // fire stands and the duck runs to the phrase end.
        let out = SungMap(vec![(90.0, 106.0)]);
        let incoming = SungMap(vec![(0.0, 300.0)]);
        let (fire, duck) =
            vocal_guard(100.0, 8.0, &out, &incoming, 0.0, 2.0, (0.0, 300.0));
        let duck = duck.expect("a clash that cannot shift away must duck");
        assert!((duck - (106.0 - fire)).abs() < 1e-6, "duck {duck} from {fire}");
    }

    #[test]
    fn a_silent_side_never_moves_the_fire_point() {
        let sung = SungMap(vec![(0.0, 300.0)]);
        let (fire, duck) =
            vocal_guard(100.0, 8.0, &SungMap::default(), &sung, 0.0, 2.0, (0.0, 300.0));
        assert!((fire - 100.0).abs() < 1e-9);
        assert!(duck.is_none());
    }

    fn bars(energies: &[f32], bar_secs: f64) -> Vec<crate::mix_facts::BarRow> {
        energies
            .iter()
            .enumerate()
            .map(|(index, energy)| crate::mix_facts::BarRow {
                start_secs: index as f64 * bar_secs,
                energy: *energy,
                low: 0.5,
            })
            .collect()
    }

    #[test]
    fn a_record_is_never_left_before_it_has_been_heard() {
        // Sixteen bars of two seconds: half a minute of record. Nothing
        // before a third of the way in is on offer, however quiet it is.
        let quiet_then_loud: Vec<f32> =
            (0..16).map(|bar| if bar < 6 { 0.1 } else { 0.9 }).collect();
        let found = exits(&bars(&quiet_then_loud, 2.0), &[], 32.0, 2.0);
        assert!(!found.is_empty());
        let floor = 32.0 * 0.35;
        assert!(
            found.iter().all(|exit| exit.at_secs >= floor),
            "left too early: {:?}",
            found.iter().map(|e| e.at_secs).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_place_the_energy_drops_away_beats_the_middle_of_a_loud_run() {
        // A record that runs hard and then falls: the fall is where a hand
        // would leave, so it has to outrank the plateau before it.
        let shape: Vec<f32> =
            (0..24).map(|bar| if bar < 16 { 0.9 } else { 0.2 }).collect();
        let found = exits(&bars(&shape, 2.0), &[], 48.0, 2.0);
        let best = found.first().expect("a record this long has exits");
        assert!(
            (best.at_secs - 32.0).abs() < 2.5,
            "the fall is at 32 s, best was {} ({})",
            best.at_secs,
            best.reason
        );
        assert!(best.reason.to_lowercase().contains("energy"), "{}", best.reason);
    }

    #[test]
    fn an_exit_carries_the_room_it_has_before_the_record_moves_again() {
        // Eight quiet bars after the fall: a fade may run into them, and
        // the allowance says how far.
        let shape: Vec<f32> =
            (0..24).map(|bar| if bar < 8 { 0.9 } else { 0.2 }).collect();
        let found = exits(&bars(&shape, 2.0), &[], 48.0, 2.0);
        let best = found.first().unwrap();
        assert!(best.allowance_secs > 0.0, "an exit with no room is not an exit");
        assert!(
            best.at_secs + best.allowance_secs <= 48.0 + 1e-6,
            "the allowance runs off the end: {} + {}",
            best.at_secs,
            best.allowance_secs
        );
    }

    #[test]
    fn a_bar_on_a_detected_change_is_preferred_to_its_neighbours() {
        // A flat record gives the scorer nothing but the change list.
        let flat = bars(&[0.6f32; 24], 2.0);
        let found = exits(&flat, &[30.0], 48.0, 2.0);
        let best = found.first().unwrap();
        assert!(
            (best.at_secs - 30.0).abs() <= 2.0,
            "the change at 30 s should win a flat record, got {} ({})",
            best.at_secs,
            best.reason
        );
    }

    #[test]
    fn leaving_it_very_late_is_worse_than_leaving_it_in_good_time() {
        let flat = bars(&[0.6f32; 40], 2.0);
        let found = exits(&flat, &[], 80.0, 2.0);
        let last = found.iter().max_by(|a, b| a.at_secs.total_cmp(&b.at_secs)).unwrap();
        let best = found.first().unwrap();
        assert!(last.at_secs > 80.0 * 0.8, "there is a late candidate to compare");
        assert!(
            last.score < best.score,
            "late {} should score under best {}",
            last.score,
            best.score
        );
    }

    #[test]
    fn a_record_with_no_bars_offers_nothing_rather_than_guessing() {
        assert!(exits(&[], &[], 48.0, 2.0).is_empty());
    }
}

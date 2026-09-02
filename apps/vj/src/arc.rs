//! Where the set wants its energy to be, right now.
//!
//! A target rather than a running order. The operator gives a length and a
//! shape; this reads that shape at whatever fraction of the night has
//! passed and hands back one number the picker leans on. Because it is a
//! target and not a plan, a skip, a record loaded by hand or a re-ordered
//! set list needs no replanning at all — the next pick simply aims at
//! wherever the curve now says.
//!
//! A curve alone permits absurdities, so a few rails ride alongside it: the
//! first record of a night is not the loudest thing in it, and a run at the
//! top has to come down before it can go up again. Those live with the
//! picker, which is the thing that can act on them.

/// The shape of a night.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Curve {
    /// Up all evening, levelling off near the end. The safe default and
    /// the one a warm-up slot wants.
    #[default]
    Build,
    /// Up to a peak around three quarters through, then settling for the
    /// last stretch so the room can leave rather than be pushed out.
    PeakAndSettle,
    /// Held about halfway all night: a bar, a dinner, a background set.
    FlatWarm,
    /// Peaks and troughs throughout, for a long set that would tire the
    /// room if it only ever climbed.
    Rollercoaster,
}

impl Curve {
    pub const ALL: [Curve; 4] =
        [Curve::Build, Curve::PeakAndSettle, Curve::FlatWarm, Curve::Rollercoaster];

    /// What the panel calls it.
    pub fn label(self) -> &'static str {
        match self {
            Curve::Build => "BUILD",
            Curve::PeakAndSettle => "PEAK",
            Curve::FlatWarm => "WARM",
            Curve::Rollercoaster => "WAVES",
        }
    }
}

/// The lowest a curve asks for, so a set never opens on silence.
const FLOOR: f32 = 0.25;
/// The highest, so there is somewhere left to go on the night that needs it.
const CEILING: f32 = 0.95;

/// Where the energy should sit at `through` of the way into the set.
///
/// `through` is clamped, so a set that has run past its own length holds at
/// its ending rather than wrapping round to the start.
pub fn target(curve: Curve, through: f32) -> f32 {
    let t = through.clamp(0.0, 1.0);
    let shape = match curve {
        Curve::Build => {
            // Fast at first, then levelling: the room fills early and the
            // last stretch has nowhere to climb to anyway.
            1.0 - (1.0 - t) * (1.0 - t)
        }
        Curve::PeakAndSettle => {
            // Up to a peak at three quarters, then down a third of the way.
            const PEAK: f32 = 0.75;
            if t <= PEAK {
                t / PEAK
            } else {
                1.0 - ((t - PEAK) / (1.0 - PEAK)) * 0.35
            }
        }
        Curve::FlatWarm => 0.45,
        Curve::Rollercoaster => {
            // Three swells over the night, riding a slow climb, so each
            // peak is higher than the last and each trough is a rest.
            let swell = (t * std::f32::consts::PI * 3.0).sin() * 0.25;
            (t * 0.7 + 0.2 + swell).clamp(0.0, 1.0)
        }
    };
    FLOOR + shape.clamp(0.0, 1.0) * (CEILING - FLOOR)
}

/// How far into the set we are, from a length in minutes and the seconds
/// elapsed. Zero-length means the operator never said, so the set sits at
/// its opening rather than pretending to be over.
pub fn through(length_mins: u32, elapsed_secs: u64) -> f32 {
    if length_mins == 0 {
        return 0.0;
    }
    (elapsed_secs as f32 / (length_mins as f32 * 60.0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_curve_starts_low_and_stays_inside_its_rails() {
        for curve in Curve::ALL {
            for step in 0..=100 {
                let value = target(curve, step as f32 / 100.0);
                assert!(
                    (FLOOR..=CEILING).contains(&value),
                    "{curve:?} at {step} gave {value}"
                );
            }
            let opening = target(curve, 0.0);
            assert!(opening <= 0.7, "{curve:?} opens at {opening}, too hot to start");
        }
    }

    #[test]
    fn a_build_climbs_and_never_turns_back() {
        let mut last = -1.0f32;
        for step in 0..=100 {
            let value = target(Curve::Build, step as f32 / 100.0);
            assert!(value >= last - 1e-6, "went backwards at {step}");
            last = value;
        }
        assert!(target(Curve::Build, 1.0) > target(Curve::Build, 0.0) + 0.3);
    }

    #[test]
    fn a_peak_set_comes_down_before_the_lights() {
        let peak = target(Curve::PeakAndSettle, 0.75);
        let ending = target(Curve::PeakAndSettle, 1.0);
        assert!(peak > target(Curve::PeakAndSettle, 0.4), "it climbs to the peak");
        assert!(ending < peak, "and settles after it: {ending} against {peak}");
    }

    #[test]
    fn a_warm_set_holds_still() {
        let early = target(Curve::FlatWarm, 0.1);
        let late = target(Curve::FlatWarm, 0.9);
        assert!((early - late).abs() < 1e-6, "{early} vs {late}");
    }

    #[test]
    fn waves_rise_and_fall_more_than_once() {
        let values: Vec<f32> =
            (0..=100).map(|step| target(Curve::Rollercoaster, step as f32 / 100.0)).collect();
        let falls = values.windows(2).filter(|pair| pair[1] < pair[0] - 1e-4).count();
        assert!(falls > 5, "a rollercoaster that only climbs is a build");
        assert!(values.last().unwrap() > values.first().unwrap(), "it still travels");
    }

    #[test]
    fn a_set_with_no_length_sits_at_its_opening() {
        assert_eq!(through(0, 9_999), 0.0);
    }

    #[test]
    fn a_set_that_runs_long_holds_at_its_ending_rather_than_starting_over() {
        assert!((through(60, 60 * 60) - 1.0).abs() < 1e-6);
        assert!((through(60, 99 * 60) - 1.0).abs() < 1e-6, "no wrapping round");
        assert!((through(60, 30 * 60) - 0.5).abs() < 1e-6);
    }
}

//! The metrical decisions both modes share: the tempo range, the musical
//! preference, a known tempo's say, and which of two octave-related pulses
//! is the beat.

/// The slowest tempo either mode reports.
pub const MIN_BPM: f64 = 60.0;
/// The fastest tempo either mode reports.
pub const MAX_BPM: f64 = 200.0;

/// Broad musical preference centred at 120 BPM. Only ever breaks an octave
/// tie; it never picks a tempo on its own.
pub fn musical_preference(bpm: f64) -> f64 {
    (-0.5 * ((bpm / 120.0).ln() / 0.38).powi(2)).exp()
}

/// The weight a lag's correlation gets when the best tempo is CHOSEN:
/// a log-normal preference one octave wide around 120 BPM (the tempo
/// listeners tap most), times the known tempo's say when there is one.
/// Correlation alone cannot tell a beat from a grouping of one and a half
/// beats (a kick-snare groove at 90 correlates as well at 60); a mild
/// preference settles that without ever overruling a clear winner.
pub fn selection_weight(bpm: f64, prior: Option<f64>) -> f64 {
    let octaves = (bpm / 120.0).log2();
    (-0.5 * octaves * octaves).exp() * prior_weight(bpm, prior)
}

/// How much a KNOWN tempo (a previous whole-file pass, a tag, the
/// operator) favours `bpm`: one at the prior, falling off with the log
/// distance, a quarter at an octave away. It decides octaves; it never
/// becomes the measured tempo.
pub fn prior_weight(bpm: f64, prior: Option<f64>) -> f64 {
    match prior.filter(|p| p.is_finite() && *p > 1.0) {
        Some(prior) => 1.0 / (1.0 + 4.0 * (bpm / prior).ln().abs()),
        None => 1.0,
    }
}

/// Fold a known tempo into the range the detector reports, by octaves, so a
/// tag of 87 on a record the detector hears at 174 still points at the
/// right family. `None` for nonsense.
pub fn fold_into_range(bpm: f64) -> Option<f64> {
    if !bpm.is_finite() || bpm <= 1.0 {
        return None;
    }
    let mut bpm = bpm;
    while bpm < MIN_BPM {
        bpm *= 2.0;
    }
    while bpm > MAX_BPM {
        bpm *= 0.5;
    }
    Some(bpm)
}

/// How strongly the pulse BETWEEN the slow reading's beats speaks for the
/// fast reading: 0.6 when the in-between pulse is weak (it is a
/// subdivision), 2 when it is as strong as the on-beat pulse (every beat
/// of the fast reading is the same kind of event, so the slow reading is a
/// grouping, not a tempo).
///
/// `balance` is the in-between comb energy over the on-beat comb energy of
/// the slow reading, in `[0, 1]`.
pub fn balance_factor(balance: f64) -> f64 {
    let t = ((balance - 0.4) / 0.55).clamp(0.0, 1.0);
    0.6 + 1.4 * t
}

/// Decide between an octave pair.
///
/// Autocorrelation cannot tell a period from an equally regular two-period
/// grouping, so the choice needs evidence from outside the correlation
/// peak. Three pieces, multiplied:
///
/// * the musical preference around 120 BPM (the old tie-breaker alone —
///   which is why a click at 174 read as 87);
/// * the alternation of the slow reading's beats: a click track or a
///   four-to-the-floor kick puts the same event on every fast beat, so the
///   slow reading is only a bar-level grouping and the fast one is the
///   tempo; a hip-hop groove with hats between the kicks does not;
/// * a known tempo, when one is given, which outweighs both.
///
/// Returns true when the FAST reading is the beat.
pub fn prefer_fast(fast_bpm: f64, slow_bpm: f64, balance: f64, prior: Option<f64>) -> bool {
    let fast = musical_preference(fast_bpm) * prior_weight(fast_bpm, prior) * balance_factor(balance);
    let slow = musical_preference(slow_bpm) * prior_weight(slow_bpm, prior);
    fast > slow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_at_174_is_174_and_a_groove_at_90_stays_90() {
        // Every fast beat identical: the fast reading wins even at 174.
        assert!(prefer_fast(174.0, 87.0, 0.98, None));
        // Hats between kicks (balance well under the on-beat): 90 stays.
        assert!(!prefer_fast(180.0, 90.0, 0.55, None));
        // A known tempo overrides the groove evidence.
        assert!(prefer_fast(180.0, 90.0, 0.55, Some(180.0)));
        assert!(!prefer_fast(174.0, 87.0, 0.98, Some(87.0)));
    }

    #[test]
    fn folding_keeps_the_family() {
        assert_eq!(fold_into_range(43.5), Some(87.0));
        assert_eq!(fold_into_range(348.0), Some(174.0));
        assert_eq!(fold_into_range(128.0), Some(128.0));
        assert_eq!(fold_into_range(f64::NAN), None);
    }
}

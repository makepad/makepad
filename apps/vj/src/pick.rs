//! Which record goes next.
//!
//! Until now that answer was the head of the queue, or a uniform draw from
//! it when SHUFFLE was lit — no tempo, no key, no memory of the night. This
//! is the arithmetic that reads what the analysis already knows and puts the
//! candidates in an order, with a sentence saying why for each.
//!
//! Pure, like the rest of the planning side: the caller assembles the
//! candidates and hands in the clock, so a test plays a whole evening in a
//! microsecond. It ranks and explains; it never loads anything. The engine's
//! queue keeps its own rules — this only decides what belongs at the front.
//!
//! Two habits come from records that were never analysed. A missing tempo or
//! key scores in the middle rather than last: a picker that punished them
//! would quietly refuse half a fresh library and look broken on the day it
//! is most needed. And nothing is ever rejected without the reason travelling
//! with it, because "nothing fits" and "nothing fits yet" are different
//! problems for the operator.

use crate::decks::tempo_fit;
use crate::set_history::SetHistory;
use crate::track_key::{key_fit, KeyEstimate};

/// What an unmeasured record scores where a measured one would score its
/// fit. The middle of the road: never chosen for a quality it has not been
/// shown to have, never refused for one it has not been shown to lack.
const UNKNOWN: f32 = 0.5;

/// How far behind the leader a record may sit and still be in the draw.
/// Inside this the two are the same answer, and always taking the first
/// would play the same set every night out of the same library.
const A_TIE: f32 = 0.05;

/// What is known about one record that could go next.
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    /// The asset id in its text form: what the history is keyed by.
    pub key: String,
    /// Empty when the tags said nothing.
    pub artist: String,
    /// Zero when the record has not been analysed.
    pub bpm: f64,
    pub musical_key: Option<KeyEstimate>,
    /// Zero when unknown.
    pub energy: f32,
}

/// What is playing, for the candidates to be measured against.
#[derive(Clone, Copy, Debug, Default)]
pub struct Outgoing {
    pub bpm: f64,
    pub musical_key: Option<KeyEstimate>,
}

/// The operator's dials.
#[derive(Clone, Copy, Debug)]
pub struct PickSettings {
    /// How long a record is off the table after it has played.
    pub replay_window_secs: u64,
    /// How long an artist is off the table after being heard.
    pub artist_window_secs: u64,
    /// Where the arc wants the energy, 0.0..=1.0.
    pub target_energy: f32,
    pub tempo_weight: f32,
    pub key_weight: f32,
    pub energy_weight: f32,
}

impl Default for PickSettings {
    fn default() -> PickSettings {
        PickSettings {
            replay_window_secs: 3 * 60 * 60,
            artist_window_secs: 30 * 60,
            target_energy: 0.5,
            tempo_weight: 1.0,
            key_weight: 0.7,
            energy_weight: 0.5,
        }
    }
}

/// How far the picker had to bend its own rules to find anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Relaxed {
    /// Everything the operator asked for held.
    Not,
    /// An artist had to be heard twice inside their window.
    ArtistRepeat,
    /// A record had to come back sooner than the operator wanted.
    Replay,
}

/// One ranked candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct Scored {
    /// Index into the candidate list the caller passed in.
    pub index: usize,
    pub score: f32,
    /// Why this record, in the operator's words.
    pub reason: String,
}

/// The candidates in order, best first, and how far the rules had to bend.
///
/// The ladder gives up the artist window before the replay window: hearing
/// an artist twice in a night is a smaller sin than hearing a record twice,
/// and both beat standing there with nothing to play.
pub fn rank(
    candidates: &[Candidate],
    outgoing: Outgoing,
    history: &SetHistory,
    now_secs: u64,
    settings: PickSettings,
) -> (Vec<Scored>, Relaxed) {
    // Loosen one rung at a time and stop at the first that finds anything.
    for relaxed in [Relaxed::Not, Relaxed::ArtistRepeat, Relaxed::Replay] {
        let mut scored: Vec<Scored> = candidates
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                let replayed = history
                    .played_since(
                        &candidate.key,
                        now_secs.saturating_sub(settings.replay_window_secs),
                    )
                    .is_some();
                if replayed && relaxed < Relaxed::Replay {
                    return None;
                }
                let artist_repeat = history.artist_since(
                    &candidate.artist,
                    now_secs.saturating_sub(settings.artist_window_secs),
                );
                if artist_repeat && relaxed < Relaxed::ArtistRepeat {
                    return None;
                }
                let (score, reason) =
                    weigh(candidate, outgoing, settings, replayed, artist_repeat);
                Some(Scored { index, score, reason })
            })
            .collect();
        if scored.is_empty() {
            continue;
        }
        // Ties keep the caller's order, which is the operator's own.
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        return (scored, relaxed);
    }
    (Vec::new(), Relaxed::Not)
}

/// One candidate's score, and the sentence that goes with it.
fn weigh(
    candidate: &Candidate,
    outgoing: Outgoing,
    settings: PickSettings,
    replayed: bool,
    artist_repeat: bool,
) -> (f32, String) {
    let mut why: Vec<String> = Vec::new();

    let tempo = if candidate.bpm > 0.0 && outgoing.bpm > 0.0 {
        let fit = tempo_fit(outgoing.bpm, candidate.bpm);
        why.push(if fit > 0.8 {
            "same pulse".to_string()
        } else if fit > 0.0 {
            format!("{:.0} BPM", candidate.bpm)
        } else {
            "a tempo away".to_string()
        });
        fit
    } else {
        why.push("tempo unknown".to_string());
        UNKNOWN
    };

    let key = match (outgoing.musical_key, candidate.musical_key) {
        (Some(out), Some(inc)) => {
            let fit = key_fit(out, inc);
            // A detector that was unsure should not swing the answer far
            // either way, so its confidence pulls the fit to the middle.
            let sure = out.confidence.min(inc.confidence).clamp(0.0, 1.0);
            why.push(if fit > 0.9 {
                format!("in key ({})", inc.camelot())
            } else if fit > 0.7 {
                format!("a step round the wheel ({})", inc.camelot())
            } else {
                format!("keys pull apart ({})", inc.camelot())
            });
            UNKNOWN + (fit - UNKNOWN) * sure
        }
        _ => {
            why.push("key unknown".to_string());
            UNKNOWN
        }
    };

    let energy = if candidate.energy > 0.0 {
        let miss = (candidate.energy - settings.target_energy).abs();
        if miss < 0.1 {
            why.push("sits where the set wants it".to_string());
        } else if candidate.energy > settings.target_energy {
            why.push("lifts".to_string());
        } else {
            why.push("cools".to_string());
        }
        (1.0 - miss).clamp(0.0, 1.0)
    } else {
        UNKNOWN
    };

    if replayed {
        why.push("played earlier".to_string());
    }
    if artist_repeat {
        why.push("same artist again".to_string());
    }

    let weights = settings.tempo_weight + settings.key_weight + settings.energy_weight;
    let score = if weights > 0.0 {
        (tempo * settings.tempo_weight
            + key * settings.key_weight
            + energy * settings.energy_weight)
            / weights
    } else {
        UNKNOWN
    };
    (score, why.join(", "))
}

/// Which of the ranked candidates to take: the leader when it leads
/// clearly, otherwise a draw from the ones bunched with it.
pub fn choose(ranked: &[Scored], seed: u64) -> Option<usize> {
    let best = ranked.first()?;
    let tied = ranked.iter().take_while(|s| best.score - s.score <= A_TIE).count().max(1);
    // xorshift64*, the same generator the queue's shuffle draws with.
    let mut x = seed.max(1);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    let draw = x.wrapping_mul(0x2545_F491_4F6C_DD1D) % tied as u64;
    Some(ranked[draw as usize].index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set_history::Played;

    fn key_of(tonic: u8, minor: bool) -> Option<KeyEstimate> {
        Some(KeyEstimate { tonic, minor, confidence: 1.0 })
    }

    fn candidate(key: &str, bpm: f64, tonic: u8, energy: f32) -> Candidate {
        Candidate {
            key: key.to_string(),
            artist: format!("artist of {key}"),
            bpm,
            musical_key: key_of(tonic, false),
            energy,
        }
    }

    fn playing() -> Outgoing {
        Outgoing { bpm: 128.0, musical_key: key_of(0, false) }
    }

    fn played(key: &str, artist: &str, at: u64) -> Played {
        Played { key: key.to_string(), artist: artist.to_string(), at_secs: at }
    }

    fn order<'a>(scored: &[Scored], candidates: &'a [Candidate]) -> Vec<&'a str> {
        scored.iter().map(|s| candidates[s.index].key.as_str()).collect()
    }

    #[test]
    fn a_record_on_the_same_pulse_and_in_key_comes_first() {
        let candidates = vec![
            candidate("far", 150.0, 6, 0.5),
            candidate("near", 128.5, 7, 0.5),
            candidate("exact", 128.0, 0, 0.5),
        ];
        let (scored, relaxed) =
            rank(&candidates, playing(), &SetHistory::new(), 0, PickSettings::default());
        assert_eq!(relaxed, Relaxed::Not);
        assert_eq!(order(&scored, &candidates), ["exact", "near", "far"]);
        assert!(!scored[0].reason.is_empty(), "the pick explains itself");
    }

    #[test]
    fn a_record_played_inside_its_window_is_not_offered_again() {
        let candidates =
            vec![candidate("a", 128.0, 0, 0.5), candidate("b", 128.0, 0, 0.5)];
        let mut history = SetHistory::new();
        history.note(played("a", "someone", 1_000));
        let (scored, relaxed) =
            rank(&candidates, playing(), &history, 1_100, PickSettings::default());
        assert_eq!(relaxed, Relaxed::Not);
        assert_eq!(order(&scored, &candidates), ["b"], "a played too recently");
    }

    #[test]
    fn an_artist_heard_too_recently_stands_down() {
        let mut candidates =
            vec![candidate("a", 128.0, 0, 0.5), candidate("b", 128.0, 0, 0.5)];
        candidates[0].artist = "Repeat".to_string();
        let mut history = SetHistory::new();
        history.note(played("other", "Repeat", 1_000));
        let (scored, _) =
            rank(&candidates, playing(), &history, 1_100, PickSettings::default());
        assert_eq!(order(&scored, &candidates), ["b"]);
    }

    #[test]
    fn the_ladder_gives_up_the_artist_before_the_record() {
        let mut candidates = vec![candidate("only", 128.0, 0, 0.5)];
        candidates[0].artist = "Repeat".to_string();
        let mut history = SetHistory::new();
        history.note(played("other", "Repeat", 1_000));
        let (scored, relaxed) =
            rank(&candidates, playing(), &history, 1_100, PickSettings::default());
        assert_eq!(relaxed, Relaxed::ArtistRepeat);
        assert_eq!(scored.len(), 1);
        assert!(
            scored[0].reason.to_lowercase().contains("artist"),
            "the operator is told why: {}",
            scored[0].reason
        );
    }

    #[test]
    fn a_record_can_come_back_when_it_is_the_only_one_left() {
        let candidates = vec![candidate("only", 128.0, 0, 0.5)];
        let mut history = SetHistory::new();
        history.note(played("only", "artist of only", 1_000));
        let (scored, relaxed) =
            rank(&candidates, playing(), &history, 1_100, PickSettings::default());
        assert_eq!(relaxed, Relaxed::Replay);
        assert_eq!(scored.len(), 1);
    }

    #[test]
    fn an_unanalysed_record_scores_in_the_middle_rather_than_last() {
        let candidates = vec![
            candidate("clash", 150.0, 6, 0.5),
            Candidate {
                key: "unknown".to_string(),
                artist: "nobody".to_string(),
                bpm: 0.0,
                musical_key: None,
                energy: 0.0,
            },
            candidate("match", 128.0, 0, 0.5),
        ];
        let (scored, _) =
            rank(&candidates, playing(), &SetHistory::new(), 0, PickSettings::default());
        assert_eq!(
            order(&scored, &candidates),
            ["match", "unknown", "clash"],
            "the unknown sits between the match and the clash"
        );
    }

    #[test]
    fn the_arc_pulls_the_pick_towards_the_energy_it_wants() {
        let candidates =
            vec![candidate("cool", 128.0, 0, 0.2), candidate("hot", 128.0, 0, 0.9)];
        let lift = PickSettings { target_energy: 0.9, ..PickSettings::default() };
        let (scored, _) = rank(&candidates, playing(), &SetHistory::new(), 0, lift);
        assert_eq!(order(&scored, &candidates), ["hot", "cool"]);

        let settle = PickSettings { target_energy: 0.2, ..PickSettings::default() };
        let (scored, _) = rank(&candidates, playing(), &SetHistory::new(), 0, settle);
        assert_eq!(order(&scored, &candidates), ["cool", "hot"]);
    }

    #[test]
    fn a_clear_leader_is_taken_and_a_tie_is_drawn_from() {
        let leader = vec![
            Scored { index: 0, score: 0.9, reason: String::new() },
            Scored { index: 1, score: 0.2, reason: String::new() },
        ];
        for seed in [1u64, 2, 3, 99] {
            assert_eq!(choose(&leader, seed), Some(0), "a clear leader is not a lottery");
        }

        let tied = vec![
            Scored { index: 0, score: 0.80, reason: String::new() },
            Scored { index: 1, score: 0.79, reason: String::new() },
            Scored { index: 2, score: 0.78, reason: String::new() },
        ];
        let drawn: std::collections::BTreeSet<usize> =
            (0..64u64).filter_map(|seed| choose(&tied, seed)).collect();
        assert!(drawn.len() > 1, "records that score alike should take turns");
        assert!(drawn.iter().all(|index| *index < 3));

        assert_eq!(choose(&[], 1), None);
    }
}

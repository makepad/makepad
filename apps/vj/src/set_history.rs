//! What the night has already played.
//!
//! The set list says what is coming; nothing until now said what has been.
//! A picker that draws from a pool rather than a running order needs to
//! know, or it will offer the record that closed the last hour and the
//! artist who has had three of the last ten.
//!
//! Pure and clock-free like the rest of the planning side: the host hands
//! in the wall clock, so a test can play a whole evening in a microsecond.
//! This is the operator's own record of their night, so it lives beside
//! their other work and never in the analysis cache, where a version bump
//! would erase it.

/// One record, as the set remembers it.
#[derive(Clone, Debug, PartialEq)]
pub struct Played {
    /// The asset id, in its text form.
    pub key: String,
    /// Empty when the tags had nothing to say.
    pub artist: String,
    /// Wall clock, seconds. The host's, not ours.
    pub at_secs: u64,
    /// What the operator thought of the transition INTO this record, when
    /// they said. Most of a night goes unrated and that is the normal
    /// case: a rating is a remark, not a form to fill in.
    pub rated: Option<bool>,
}

/// The night so far, oldest first.
#[derive(Clone, Debug, Default)]
pub struct SetHistory {
    plays: Vec<Played>,
}

/// How many records back the log keeps. Several nights of a busy set: long
/// enough that no window a picker asks about can run off the end, short
/// enough that the file stays a file.
const KEEP: usize = 2_000;

impl SetHistory {
    pub fn new() -> SetHistory {
        SetHistory::default()
    }

    /// Everything played, oldest first.
    pub fn plays(&self) -> &[Played] {
        &self.plays
    }

    /// Remember a record. The caller has already decided it sounded.
    pub fn note(&mut self, played: Played) {
        self.plays.push(played);
        if self.plays.len() > KEEP {
            let over = self.plays.len() - KEEP;
            self.plays.drain(..over);
        }
    }

    /// When this record last played, if it did at or after `since_secs`.
    pub fn played_since(&self, key: &str, since_secs: u64) -> Option<u64> {
        self.plays
            .iter()
            .filter(|play| play.key == key && play.at_secs >= since_secs)
            .map(|play| play.at_secs)
            .max()
    }

    /// Whether this artist has been heard at or after `since_secs`. An
    /// empty artist is nobody and never matches: a library with half its
    /// tags missing must not collapse into one artist who blocks the pool.
    pub fn artist_since(&self, artist: &str, since_secs: u64) -> bool {
        if artist.is_empty() {
            return false;
        }
        self.plays
            .iter()
            .any(|play| play.artist == artist && play.at_secs >= since_secs)
    }

    /// Mark what the operator thought of the last transition.
    pub fn rate_last(&mut self, good: bool) -> bool {
        match self.plays.last_mut() {
            Some(play) => {
                play.rated = Some(good);
                true
            }
            None => false,
        }
    }

    /// One record per line: when, then a mark for the transition into it,
    /// then the key, then the artist, which is the only field that can hold
    /// a space and so goes last.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for play in &self.plays {
            out.push_str(&play.at_secs.to_string());
            out.push(' ');
            out.push(match play.rated {
                Some(true) => '+',
                Some(false) => '-',
                None => '.',
            });
            out.push(' ');
            out.push_str(&play.key);
            out.push(' ');
            // The artist is the tail of the line, so a name with spaces in
            // it needs no quoting and cannot break the fields before it.
            out.push_str(play.artist.trim());
            out.push('\n');
        }
        out
    }

    /// A line that cannot be read is dropped and the rest of the night
    /// survives: a log is a convenience, and losing all of it because one
    /// line was truncated by a power cut would be the wrong trade.
    pub fn from_text(text: &str) -> SetHistory {
        let mut log = SetHistory::new();
        for line in text.lines() {
            let line = line.trim_end();
            let Some((when, rest)) = line.split_once(' ') else {
                continue;
            };
            let Ok(at_secs) = when.parse() else {
                continue;
            };
            let (mark, rest) = match rest.split_once(' ') {
                Some((first, rest)) => (first, rest),
                None => (rest, ""),
            };
            // The mark was added after the first nights were written, so a
            // line without one is still a line: an asset id is never a
            // single mark character, which is what tells the two apart.
            let (rated, key, artist) = match mark {
                "+" | "-" | "." => {
                    let rated = match mark {
                        "+" => Some(true),
                        "-" => Some(false),
                        _ => None,
                    };
                    match rest.split_once(' ') {
                        Some((key, artist)) => (rated, key, artist),
                        None => (rated, rest, ""),
                    }
                }
                key => (None, key, rest),
            };
            if key.is_empty() {
                continue;
            }
            // The artist is whatever is left, so a name with spaces in it
            // arrives whole.
            log.note(Played {
                key: key.to_string(),
                artist: artist.to_string(),
                at_secs,
                rated,
            });
        }
        log
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn played(key: &str, artist: &str, at: u64) -> Played {
        Played {
            key: key.to_string(),
            artist: artist.to_string(),
            at_secs: at,
            rated: None,
        }
    }

    #[test]
    fn a_record_inside_the_window_is_remembered_and_one_before_it_is_not() {
        let mut log = SetHistory::new();
        log.note(played("ast_a", "Someone", 1_000));
        log.note(played("ast_b", "Another", 5_000));

        assert_eq!(log.played_since("ast_b", 4_000), Some(5_000));
        assert_eq!(log.played_since("ast_a", 4_000), None, "older than the window");
        assert_eq!(log.played_since("ast_a", 500), Some(1_000));
        assert_eq!(log.played_since("ast_never", 0), None);
    }

    #[test]
    fn the_same_record_twice_reports_the_later_time() {
        let mut log = SetHistory::new();
        log.note(played("ast_a", "Someone", 100));
        log.note(played("ast_a", "Someone", 900));
        assert_eq!(log.played_since("ast_a", 0), Some(900));
    }

    #[test]
    fn an_artist_is_remembered_across_different_records() {
        let mut log = SetHistory::new();
        log.note(played("ast_a", "Someone", 1_000));
        assert!(log.artist_since("Someone", 500));
        assert!(!log.artist_since("Someone", 2_000), "older than the window");
        assert!(!log.artist_since("Nobody", 0));
    }

    #[test]
    fn an_untagged_record_never_blocks_the_pool() {
        // Half a library can arrive without tags. If the empty artist
        // matched itself, the first untagged record played would lock out
        // every other untagged record for the rest of the window.
        let mut log = SetHistory::new();
        log.note(played("ast_a", "", 1_000));
        assert!(!log.artist_since("", 0));
    }

    #[test]
    fn a_rating_lands_on_the_record_it_was_about_and_survives_the_file() {
        let mut log = SetHistory::new();
        assert!(!log.rate_last(true), "nothing has played to rate");

        log.note(played("ast_a", "Someone", 100));
        log.note(played("ast_b", "Another", 200));
        assert!(log.rate_last(false), "the last record takes the mark");
        assert_eq!(log.plays()[0].rated, None, "and only that one");
        assert_eq!(log.plays()[1].rated, Some(false));

        let back = SetHistory::from_text(&log.to_text());
        assert_eq!(back.plays(), log.plays());
    }

    #[test]
    fn a_night_written_before_ratings_existed_still_reads() {
        // The mark was added later. An asset id cannot be one character, so
        // the two shapes of line tell themselves apart.
        let log = SetHistory::from_text("1000 ast_a Someone Else\n");
        assert_eq!(log.plays().len(), 1);
        assert_eq!(log.plays()[0].key, "ast_a");
        assert_eq!(log.plays()[0].artist, "Someone Else");
        assert_eq!(log.plays()[0].rated, None);
    }

    #[test]
    fn the_log_round_trips_through_its_text() {
        let mut log = SetHistory::new();
        log.note(played("ast_a", "Someone With Spaces", 1_000));
        log.note(played("ast_b", "", 2_000));
        let back = SetHistory::from_text(&log.to_text());
        assert_eq!(back.plays(), log.plays());
    }

    #[test]
    fn a_broken_line_does_not_take_the_night_with_it() {
        let text = concat!(
            "1000 ast_a Someone\n",
            "nonsense\n",
            "\n",
            "2000 ast_b Another\n",
        );
        let log = SetHistory::from_text(text);
        assert_eq!(log.plays().len(), 2, "{:?}", log.plays());
        assert_eq!(log.played_since("ast_b", 0), Some(2_000));
    }

    #[test]
    fn the_log_does_not_grow_without_bound() {
        let mut log = SetHistory::new();
        for index in 0..(KEEP + 50) {
            log.note(played(&format!("ast_{index}"), "Someone", index as u64));
        }
        assert_eq!(log.plays().len(), KEEP);
        // The oldest went, the newest stayed.
        assert_eq!(log.played_since("ast_0", 0), None);
        assert!(log.played_since(&format!("ast_{}", KEEP + 49), 0).is_some());
    }
}

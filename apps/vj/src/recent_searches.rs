//! What the search boxes have already been asked for.
//!
//! A search box is the one control on the library surface with no memory at
//! all: the query that found the record an hour ago is gone the moment it is
//! typed over, and the operator retypes it from the top. This module is that
//! memory, and nothing else -- it is pure, it reads no clock and touches no
//! file, so the walk through it can be tested as the sequence of key presses
//! it actually is.
//!
//! ONE LIST PER LANE, not one shared list. The three boxes search three
//! different catalogs, and a query that belongs to the music lane is noise in
//! the mesh lane: recalling "dua lipa" while looking for a mesh would be a
//! memory that costs more than it gives. The lane is a plain name here rather
//! than a type, because a lane this build has never heard of must survive
//! being read and written back -- the same rule the settings store follows,
//! so an older build cannot throw away what a newer one recorded.
//!
//! Newest first, throughout. The list is walked from the front, written from
//! the front and read from the front, so there is no place where an "oldest
//! first" reading could creep in and quietly reverse the recall.

/// How many queries one lane remembers.
///
/// Fifty is a night's worth of asking. The cap is enforced on the way in and
/// again on the way back off disk, so a hand-edited file cannot grow it.
pub const KEEP: usize = 50;

/// One lane's queries, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecentSearches {
    queries: Vec<String>,
}

impl RecentSearches {
    pub fn queries(&self) -> &[String] {
        &self.queries
    }

    pub fn is_empty(&self) -> bool {
        self.queries.is_empty()
    }

    pub fn get(&self, at: usize) -> Option<&str> {
        self.queries.get(at).map(String::as_str)
    }

    /// Remember a query the operator actually asked for.
    ///
    /// Trimmed, because the box's own text carries whatever whitespace the
    /// typing left and the search itself trims before it runs -- a remembered
    /// query that differs from the query that ran by a space would recall as a
    /// second, identical-looking entry. Empty is not a query and is not kept.
    /// A repeat MOVES to the front rather than taking a second slot: a night
    /// spent going back to the same three crates would otherwise fill the list
    /// with them and push out everything else.
    pub fn note(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        self.queries.retain(|held| held != query);
        self.queries.insert(0, query.to_string());
        self.queries.truncate(KEEP);
    }

    /// Where the recall walk goes next.
    ///
    /// `at` is where it is now, and `None` is the unfinished text the operator
    /// had typed before they started walking -- which is a real place in the
    /// walk, not the absence of one: stepping forward off the newest entry has
    /// to give them back what they were writing, or the walk eats it.
    ///
    /// Walking back stops on the oldest rather than wrapping. A list that
    /// wraps means a held key silently returns to the top and the operator
    /// runs a query from an hour ago believing it is the one they just passed.
    pub fn step(&self, at: Option<usize>, older: bool) -> Option<usize> {
        if self.queries.is_empty() {
            return None;
        }
        let last = self.queries.len() - 1;
        match (at, older) {
            (None, true) => Some(0),
            (Some(at), true) => Some((at + 1).min(last)),
            (None, false) => None,
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
        }
    }

}

/// Every lane's list, as one file's worth.
///
/// Lanes are held as pairs rather than as a fixed set of fields so that a name
/// this build does not know still reads, still writes back, and still keeps
/// its order. The file is one `lane query` line per query, newest first within
/// each lane -- the same shape the set list's own file uses, for the same
/// reason: one line is one thing, and a torn line costs that line only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recalls {
    lanes: Vec<(String, RecentSearches)>,
}

impl Recalls {
    pub fn lane(&self, name: &str) -> Option<&RecentSearches> {
        self.lanes.iter().find(|(held, _)| held == name).map(|(_, list)| list)
    }

    /// The lane's list, made if this is the first thing it has been asked.
    pub fn lane_mut(&mut self, name: &str) -> &mut RecentSearches {
        if let Some(at) = self.lanes.iter().position(|(held, _)| held == name) {
            return &mut self.lanes[at].1;
        }
        self.lanes.push((name.to_string(), RecentSearches::default()));
        let last = self.lanes.len() - 1;
        &mut self.lanes[last].1
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (lane, list) in &self.lanes {
            for query in list.queries() {
                out.push_str(lane);
                out.push(' ');
                out.push_str(query);
                out.push('\n');
            }
        }
        out
    }

    /// Read one back.
    ///
    /// Deliberately unfailing, like every other operator-owned file here: a
    /// line with no lane, an empty query, or a query this file already holds
    /// for that lane is passed over, and everything around it still reads. A
    /// query may contain spaces, so only the FIRST space separates the lane
    /// from the query -- splitting on all of them would quietly truncate every
    /// multi-word search the operator ever ran.
    pub fn from_text(text: &str) -> Recalls {
        let mut out = Recalls::default();
        for line in text.lines() {
            let Some((lane, query)) = line.trim_end_matches('\r').split_once(' ') else {
                continue;
            };
            if lane.is_empty() || query.trim().is_empty() {
                continue;
            }
            let list = out.lane_mut(lane);
            if list.queries.len() == KEEP || list.queries.iter().any(|held| held == query) {
                continue;
            }
            list.queries.push(query.to_string());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noted(queries: &[&str]) -> RecentSearches {
        let mut list = RecentSearches::default();
        for query in queries {
            list.note(query);
        }
        list
    }

    #[test]
    fn a_query_searched_again_moves_to_the_front_instead_of_taking_a_second_slot() {
        let list = noted(&["drums", "vocal", "drums"]);
        assert_eq!(list.queries(), ["drums", "vocal"], "one slot, at the front");
    }

    #[test]
    fn a_query_that_is_only_whitespace_is_not_a_query() {
        let list = noted(&["", "   ", "\t", "drums", "  drums  "]);
        assert_eq!(
            list.queries(),
            ["drums"],
            "and the padded repeat is the same query, not a second one",
        );
    }

    #[test]
    fn the_list_never_grows_past_the_fifty_it_promises_and_drops_the_oldest_first() {
        let mut list = RecentSearches::default();
        for n in 0..(KEEP + 10) {
            list.note(&format!("q{n}"));
        }
        assert_eq!(list.queries().len(), KEEP);
        assert_eq!(list.get(0), Some(format!("q{}", KEEP + 9).as_str()), "newest leads");
        assert_eq!(list.get(KEEP - 1), Some("q10"), "the ten oldest fell off the end");
    }

    #[test]
    fn stepping_back_stops_at_the_oldest_and_stepping_forward_returns_to_the_unfinished_text() {
        // Noted oldest-to-newest, so the list reads ["c", "b", "a"].
        let list = noted(&["a", "b", "c"]);

        let mut at = None;
        for expect in [Some(0), Some(1), Some(2), Some(2), Some(2)] {
            at = list.step(at, true);
            assert_eq!(at, expect, "walking back");
        }
        for expect in [Some(1), Some(0), None, None] {
            at = list.step(at, false);
            assert_eq!(at, expect, "walking forward");
        }
        assert_eq!(list.get(0), Some("c"), "back one step is the newest query");
    }

    #[test]
    fn an_empty_list_has_nowhere_to_walk() {
        let list = RecentSearches::default();
        assert_eq!(list.step(None, true), None);
        assert_eq!(list.step(None, false), None);
    }

    #[test]
    fn a_search_written_out_and_read_back_is_the_same_search_in_the_same_order() {
        let mut recalls = Recalls::default();
        for query in ["drums", "deep house", "  padded  "] {
            recalls.lane_mut("music").note(query);
        }
        recalls.lane_mut("mesh").note("chair");

        let back = Recalls::from_text(&recalls.to_text());
        assert_eq!(back, recalls, "the file is the whole of what was held");
        assert_eq!(
            back.lane("music").expect("music lane").queries(),
            ["padded", "deep house", "drums"],
            "newest first, and a multi-word query survives its spaces",
        );
        assert_eq!(back.lane("mesh").expect("mesh lane").queries(), ["chair"]);
        assert!(back.lane("sfx").is_none(), "a lane nothing was asked of is not invented");
    }

    #[test]
    fn a_hand_edited_or_torn_file_loses_the_bad_line_and_keeps_the_rest() {
        let back = Recalls::from_text(
            "music drums\n\
             \n\
             nolanespace\n\
             music \n\
             music deep house\n\
             music drums\n\
             \u{1f3a7}\n\
             mesh chair\n",
        );
        assert_eq!(
            back.lane("music").expect("music lane").queries(),
            ["drums", "deep house"],
            "the blank, the lane-less, the empty query and the repeat all fall out",
        );
        assert_eq!(back.lane("mesh").expect("mesh lane").queries(), ["chair"]);
    }

    #[test]
    fn a_lane_this_build_does_not_know_still_reads_and_is_written_back() {
        let back = Recalls::from_text("music drums\nlighting strobe\n");
        assert_eq!(
            back.lane("lighting").expect("the unknown lane").queries(),
            ["strobe"],
            "an older build must not throw away what a newer one recorded",
        );
        assert_eq!(Recalls::from_text(&back.to_text()), back);
    }

    #[test]
    fn a_file_longer_than_the_cap_is_cut_to_it_on_the_way_back_in() {
        let mut text = String::new();
        for n in 0..(KEEP + 20) {
            text.push_str(&format!("music q{n}\n"));
        }
        let back = Recalls::from_text(&text);
        let held = back.lane("music").expect("music lane").queries();
        assert_eq!(held.len(), KEEP, "a hand-edited file cannot raise the cap");
        assert_eq!(held[0], "q0", "and it keeps the newest end of the file");
    }
}

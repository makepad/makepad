//! What a record's own menu offers, decided from what is true about it.
//!
//! The row is out of width. Every per-track action that earns a chip costs
//! the title the space to be read, and the title is what the operator is
//! actually scanning -- so the verbs move under the secondary button and the
//! row keeps its width for the record.
//!
//! The gating is the point, not decoration. A menu row that would do nothing
//! is worse than no row: it reads as an offer, the operator takes it, and
//! nothing happens. So a verb appears only when it has something to do --
//! "add to the set list" is not offered for a record already on it, because
//! the queue de-dupes and the add would be a no-op; the deck a record is
//! already playing on is not offered it again, because that is either nothing
//! or a reload under the needle.
//!
//! Built as a list from facts rather than as a bitmask on the row entry: the
//! row entry is compared wholesale to decide whether the listing changed, so
//! a field recomputed every pump would make every row look dirty every pump.
//! Nothing here has a `Cx`, a widget or a clock, so the whole decision is
//! testable as the plain question it is.

use crate::decks::DeckId;

/// What the menu can ask for. Each one already exists as a named verb the
/// mouse and the keyboard share; the menu is a third caller, not a third
/// implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackVerb {
    LoadDeck(DeckId),
    Queue,
    PlayNext,
    ReplaceSet,
    Unqueue,
    Preview,
}

/// One line of the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackMenuRow {
    pub verb: TrackVerb,
    pub label: String,
    /// The modifier that reaches the same verb from the row's own chip, so
    /// the menu teaches the shortcut rather than hiding it.
    pub hint: &'static str,
    /// Starts a group: the rule is applied to whichever row actually LEADS
    /// the group, so a gated-away row never leaves a hairline hanging over
    /// nothing.
    pub separator: bool,
}

/// Everything the menu needs to know about the record it was opened on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackMenuFacts {
    /// Opened from the set list rather than from the listing.
    pub from_set_list: bool,
    /// The deck already holding this record, if one is.
    pub on_deck: Option<DeckId>,
    /// Already waiting on the set list.
    pub queued: bool,
    /// How many records the set list holds.
    pub set_len: usize,
    /// This is the record currently in the phones.
    pub previewing: bool,
}

/// The most rows any menu can build. The shell declares this many slots, so
/// a new verb costs a line here and a line there rather than a redesign; the
/// spare slots are why the number is not exactly the longest menu.
pub const MAX_ROWS: usize = 10;

/// The menu for one record.
pub fn track_menu(facts: TrackMenuFacts) -> Vec<TrackMenuRow> {
    let mut rows: Vec<TrackMenuRow> = Vec::new();
    fn row(rows: &mut Vec<TrackMenuRow>, verb: TrackVerb, label: String, hint: &'static str) {
        rows.push(TrackMenuRow { verb, label, hint, separator: false });
    }

    for deck in [DeckId::A, DeckId::B] {
        // Not the deck it is already on: that is either nothing at all or a
        // reload under the needle, and neither is what the operator asked
        // for by opening a menu.
        if facts.on_deck == Some(deck) {
            continue;
        }
        let name = match deck {
            DeckId::A => "A",
            DeckId::B => "B",
        };
        row(&mut rows, TrackVerb::LoadDeck(deck), format!("Load to deck {name}"), "");
    }

    let set_group = rows.len();
    if !facts.queued {
        // The set list de-dupes, so this row would do nothing for a record
        // already waiting on it.
        row(&mut rows, TrackVerb::Queue, "Add to the set list".to_string(), "");
    }
    // Always: playing a record next MOVES one that is already waiting, so it
    // is a real action either way.
    row(&mut rows, TrackVerb::PlayNext, "Play it next".to_string(), "Shift");
    if facts.set_len > 0 {
        // With nothing on the set list this is the Add row wearing a
        // frightening name.
        row(&mut rows, TrackVerb::ReplaceSet, "Start the set list over with this".to_string(), "Ctrl");
    }
    if facts.from_set_list {
        // The listing has no remove: the set list is the only place a record
        // can be taken off it.
        row(&mut rows, TrackVerb::Unqueue, "Take off the set list".to_string(), "");
    }

    let phones_group = rows.len();
    let listen = match facts.previewing {
        true => "Stop the pre-listen",
        false => "Pre-listen",
    };
    row(&mut rows, TrackVerb::Preview, listen.to_string(), "");

    for at in [set_group, phones_group] {
        if let Some(row) = rows.get_mut(at) {
            row.separator = true;
        }
    }
    // The first row never wears one: a rule above the top of a menu is a
    // stray mark.
    if let Some(first) = rows.first_mut() {
        first.separator = false;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verbs(facts: TrackMenuFacts) -> Vec<TrackVerb> {
        track_menu(facts).into_iter().map(|row| row.verb).collect()
    }

    #[test]
    fn every_track_menu_fits_the_slots_the_shell_carries() {
        // Every combination of the facts, so no reachable menu can be longer
        // than the shell can draw.
        for from_set_list in [false, true] {
            for on_deck in [None, Some(DeckId::A), Some(DeckId::B)] {
                for queued in [false, true] {
                    for set_len in [0, 1, 9] {
                        for previewing in [false, true] {
                            let facts = TrackMenuFacts {
                                from_set_list,
                                on_deck,
                                queued,
                                set_len,
                                previewing,
                            };
                            let rows = track_menu(facts);
                            assert!(
                                rows.len() <= MAX_ROWS,
                                "{facts:?} builds {} rows, the shell has {MAX_ROWS}",
                                rows.len(),
                            );
                            assert!(!rows.is_empty(), "{facts:?} builds an empty menu");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_deck_a_record_is_already_on_is_not_offered_it_again() {
        let on_a = verbs(TrackMenuFacts { on_deck: Some(DeckId::A), ..Default::default() });
        assert!(!on_a.contains(&TrackVerb::LoadDeck(DeckId::A)), "it is already there");
        assert!(on_a.contains(&TrackVerb::LoadDeck(DeckId::B)), "the other deck still stands");

        let free = verbs(TrackMenuFacts::default());
        assert!(free.contains(&TrackVerb::LoadDeck(DeckId::A)));
        assert!(free.contains(&TrackVerb::LoadDeck(DeckId::B)));
    }

    #[test]
    fn a_record_already_waiting_is_not_offered_a_second_seat() {
        let waiting = verbs(TrackMenuFacts { queued: true, set_len: 3, ..Default::default() });
        assert!(!waiting.contains(&TrackVerb::Queue), "the set list would de-dupe it away");
        assert!(
            waiting.contains(&TrackVerb::PlayNext),
            "but playing it next MOVES it, which is a real action",
        );
    }

    #[test]
    fn starting_the_set_list_over_needs_a_set_list_to_start_over() {
        let empty = verbs(TrackMenuFacts { set_len: 0, ..Default::default() });
        assert!(!empty.contains(&TrackVerb::ReplaceSet), "with nothing queued it IS the add row");
        let full = verbs(TrackMenuFacts { set_len: 1, ..Default::default() });
        assert!(full.contains(&TrackVerb::ReplaceSet));
    }

    #[test]
    fn only_the_set_list_offers_to_take_a_record_off_it() {
        let listing = verbs(TrackMenuFacts { queued: true, ..Default::default() });
        assert!(!listing.contains(&TrackVerb::Unqueue));
        let set_list =
            verbs(TrackMenuFacts { from_set_list: true, queued: true, ..Default::default() });
        assert!(set_list.contains(&TrackVerb::Unqueue));
    }

    #[test]
    fn the_phones_row_says_which_way_it_will_go() {
        let idle = track_menu(TrackMenuFacts::default());
        let playing = track_menu(TrackMenuFacts { previewing: true, ..Default::default() });
        let label = |rows: &[TrackMenuRow]| {
            rows.iter()
                .find(|row| row.verb == TrackVerb::Preview)
                .map(|row| row.label.clone())
                .expect("a phones row")
        };
        assert_eq!(label(&idle), "Pre-listen");
        assert_eq!(label(&playing), "Stop the pre-listen");
    }

    #[test]
    fn a_rule_never_opens_a_menu_and_always_has_a_row_above_it() {
        // Two single-row groups in a row are legitimate and DO carry two
        // rules -- there is a row between them. What can never happen is a
        // rule with nothing above it, which reads as a stray mark.
        for from_set_list in [false, true] {
            for on_deck in [None, Some(DeckId::A), Some(DeckId::B)] {
                for queued in [false, true] {
                    for set_len in [0, 3] {
                        let facts = TrackMenuFacts {
                            from_set_list,
                            on_deck,
                            queued,
                            set_len,
                            ..Default::default()
                        };
                        let rows = track_menu(facts);
                        assert!(!rows[0].separator, "{facts:?} rules above its own first row");
                        for (at, row) in rows.iter().enumerate() {
                            assert!(
                                !row.separator || at > 0,
                                "{facts:?} rules above nothing at row {at}",
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_gated_away_row_hands_its_rule_to_whatever_leads_the_group_instead() {
        // With the Add row gated away, the set-list group starts at Play
        // next -- and that is the row that must wear the rule.
        let rows = track_menu(TrackMenuFacts { queued: true, ..Default::default() });
        let lead = rows
            .iter()
            .find(|row| row.verb == TrackVerb::PlayNext)
            .expect("a play-next row");
        assert!(lead.separator, "the group still starts, so it still gets its rule");
    }

    #[test]
    fn every_row_says_something_and_says_it_once() {
        let rows = track_menu(TrackMenuFacts {
            from_set_list: true,
            on_deck: Some(DeckId::A),
            queued: false,
            set_len: 2,
            previewing: false,
        });
        let mut labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "two rows read the same");
        assert!(rows.iter().all(|row| !row.label.trim().is_empty()));
    }
}

//! The library's verbs, named, and the keys that ask for them.
//!
//! Until now every one of these existed only as the body of a mouse handler,
//! so there was nothing for a key -- or, later, a controller -- to ask for
//! except by growing a second copy of the same sequence beside the first.
//! This module is the layer that stops that: the verbs have names, the keys
//! map onto the names, and both the hand and the keyboard reach the same
//! implementation. The MIDI half of the item this comes from is deliberately
//! not built (see the commit), but it becomes a binding table over these
//! names rather than a rewrite.
//!
//! Everything here is pure: no `Cx`, no widgets, no clock. The cursor walk,
//! the modifier rules and the re-mapping of a cursor across a rebuilt listing
//! are exactly the parts that are easy to get subtly wrong and easy to test,
//! so they live here and the widget keeps only the drawing.

use crate::decks::DeckId;
use makepad_widgets::*;

/// Where a cursor step goes. Rows and pages are signed so one arm handles
/// both directions and the clamp cannot disagree between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorMove {
    Rows(i32),
    Pages(i32),
    First,
    Last,
}

/// What the library can be asked to do.
///
/// `LoadDeck` carries a [`DeckId`] rather than a deck TARGET, so a command
/// cannot ask for a deck that is not a deck -- the target's third state means
/// "load nothing", which is a setting, not a verb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryCmd {
    /// Move where the keys are standing. `extend` takes the pick with it,
    /// the way a shift-click does.
    Cursor { step: CursorMove, extend: bool },
    /// Load the row under the cursor onto this deck, named outright.
    LoadDeck(DeckId),
    /// Load it wherever a plain click would have sent it.
    LoadTarget,
    PlayNext,
    FocusSearch,
}

impl LibraryCmd {
    /// The stable name of this verb.
    ///
    /// These are the item's "named commands": what a binding binds to, and
    /// what makes this a layer rather than a key match inlined in a handler.
    /// Treated as settings keys the day anything binds them, so renaming one
    /// is renaming a slug.
    ///
    /// `extend` is deliberately NOT part of the name. Taking the pick along
    /// is the shift modifier of the same verb, not a twelfth verb.
    pub fn name(self) -> &'static str {
        match self {
            LibraryCmd::Cursor { step, .. } => match step {
                CursorMove::Rows(n) if n >= 0 => "library_next_row",
                CursorMove::Rows(_) => "library_prev_row",
                CursorMove::Pages(n) if n >= 0 => "library_next_page",
                CursorMove::Pages(_) => "library_prev_page",
                CursorMove::First => "library_first_row",
                CursorMove::Last => "library_last_row",
            },
            LibraryCmd::LoadDeck(DeckId::A) => "library_load_a",
            LibraryCmd::LoadDeck(DeckId::B) => "library_load_b",
            LibraryCmd::LoadTarget => "library_load",
            LibraryCmd::PlayNext => "library_play_next",
            LibraryCmd::FocusSearch => "library_focus_search",
        }
    }
}

/// The keys, and the verb each one asks for unshifted.
pub const LIBRARY_KEYS: [(KeyCode, LibraryCmd); 11] = [
    (KeyCode::ArrowDown, LibraryCmd::Cursor { step: CursorMove::Rows(1), extend: false }),
    (KeyCode::ArrowUp, LibraryCmd::Cursor { step: CursorMove::Rows(-1), extend: false }),
    (KeyCode::PageDown, LibraryCmd::Cursor { step: CursorMove::Pages(1), extend: false }),
    (KeyCode::PageUp, LibraryCmd::Cursor { step: CursorMove::Pages(-1), extend: false }),
    (KeyCode::Home, LibraryCmd::Cursor { step: CursorMove::First, extend: false }),
    (KeyCode::End, LibraryCmd::Cursor { step: CursorMove::Last, extend: false }),
    (KeyCode::KeyA, LibraryCmd::LoadDeck(DeckId::A)),
    (KeyCode::KeyB, LibraryCmd::LoadDeck(DeckId::B)),
    (KeyCode::ReturnKey, LibraryCmd::LoadTarget),
    (KeyCode::KeyN, LibraryCmd::PlayNext),
    (KeyCode::Slash, LibraryCmd::FocusSearch),
];

/// Which verb a key press asks for, if any.
///
/// Control, alt and the platform key mean this is not a library command at
/// all: every OS combination and every existing shortcut stays free, and a
/// stray Ctrl+A while the listing has focus must not select a track.
///
/// Shift takes a RANGE. It turns a cursor move into one that carries the
/// pick, and it turns everything else into no command at all -- a shift that
/// loaded a deck would be a range gesture that put a record on air.
pub fn command_for_key(code: KeyCode, mods: KeyModifiers) -> Option<LibraryCmd> {
    if mods.control || mods.alt || mods.logo {
        return None;
    }
    let cmd = LIBRARY_KEYS.iter().find(|(key, _)| *key == code).map(|(_, cmd)| *cmd)?;
    match (cmd, mods.shift) {
        (cmd, false) => Some(cmd),
        (LibraryCmd::Cursor { step, .. }, true) => Some(LibraryCmd::Cursor { step, extend: true }),
        (_, true) => None,
    }
}

/// The page step when the list has not drawn yet and has no count to give.
pub const DEFAULT_PAGE_ROWS: usize = 8;

/// How many rows one page step moves, from the number of rows the list
/// actually DREW.
///
/// Measured rather than computed from a row height on purpose: the rows are
/// not a uniform height. The previewing row wears a taller template, so a
/// page worked out from a fixed stride over-counts by whatever the open
/// player is worth -- exactly when the operator is walking the library with
/// something in the phones.
///
/// One row of overlap is kept, so the row that was at the far edge is still
/// on screen after the step and the operator keeps their place.
pub fn page_rows(drawn: usize) -> usize {
    if drawn == 0 {
        return DEFAULT_PAGE_ROWS;
    }
    drawn.saturating_sub(1).max(1)
}

/// Where the cursor lands after one step.
///
/// The arithmetic is done in `i64` and clamped there, before anything is cast
/// back: a page step near the top of a long listing goes far negative first,
/// and clamping after a cast to `usize` would have wrapped it to the end.
pub fn step_cursor(
    cursor: Option<usize>,
    len: usize,
    page: usize,
    step: CursorMove,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len as i64 - 1;
    let at = match (cursor, step) {
        // Nothing is standing anywhere yet: the first move lands on the end
        // it came from, so a first press of Down reaches the top row rather
        // than the second one.
        (None, CursorMove::Rows(n) | CursorMove::Pages(n)) if n >= 0 => 0,
        (None, CursorMove::Rows(_) | CursorMove::Pages(_)) => last,
        (_, CursorMove::First) => 0,
        (_, CursorMove::Last) => last,
        (Some(at), CursorMove::Rows(n)) => at as i64 + n as i64,
        (Some(at), CursorMove::Pages(n)) => at as i64 + n as i64 * page.max(1) as i64,
    };
    Some(at.clamp(0, last) as usize)
}

/// Where the cursor goes when the listing under it is rebuilt.
///
/// A row number means a different record after a rebuild, and the explorer is
/// rebuilt for every badge and every keystroke. `None` when the record it was
/// standing on is gone -- the keys then start again from the end they are
/// moving towards, which is truer than guessing a neighbour.
pub fn remap_cursor<K: PartialEq>(was: Option<&K>, after: &[K]) -> Option<usize> {
    let was = was?;
    after.iter().position(|held| held == was)
}

/// The same, for the picks: every one whose record survived, in the new
/// order.
pub fn remap_picks<K: PartialEq>(was: &[K], after: &[K]) -> Vec<usize> {
    after
        .iter()
        .enumerate()
        .filter(|(_, held)| was.iter().any(|key| key == *held))
        .map(|(row, _)| row)
        .collect()
}

/// Which end of the set list a track is added to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueHow {
    Tail,
    Next,
    Replace,
}

/// The set-list modifier rule, in one place so the row's `+` chip and the
/// keyboard cannot drift apart: control starts the list over, shift plays
/// this one next, a plain press adds to the tail.
pub fn queue_how(mods: KeyModifiers) -> QueueHow {
    if mods.control {
        QueueHow::Replace
    } else if mods.shift {
        QueueHow::Next
    } else {
        QueueHow::Tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> KeyModifiers {
        KeyModifiers::default()
    }

    fn shifted() -> KeyModifiers {
        KeyModifiers { shift: true, ..Default::default() }
    }

    #[test]
    fn every_named_command_answers_to_one_name() {
        let mut seen: Vec<&'static str> = Vec::new();
        for (_, cmd) in LIBRARY_KEYS {
            let name = cmd.name();
            assert!(name.starts_with("library_"), "{name} is not in the library's namespace");
            assert!(!seen.contains(&name), "{name} names two different commands");
            seen.push(name);
        }
        assert_eq!(seen.len(), LIBRARY_KEYS.len(), "eleven keys, eleven verbs");
    }

    #[test]
    fn taking_the_pick_along_is_the_same_verb_under_shift() {
        let walk = LibraryCmd::Cursor { step: CursorMove::Rows(1), extend: false };
        let drag = LibraryCmd::Cursor { step: CursorMove::Rows(1), extend: true };
        assert_eq!(walk.name(), drag.name(), "extend is a modifier, not a twelfth verb");
    }

    #[test]
    fn shift_and_an_arrow_takes_the_pick_with_it() {
        assert_eq!(
            command_for_key(KeyCode::ArrowDown, shifted()),
            Some(LibraryCmd::Cursor { step: CursorMove::Rows(1), extend: true }),
        );
        assert_eq!(
            command_for_key(KeyCode::ArrowDown, plain()),
            Some(LibraryCmd::Cursor { step: CursorMove::Rows(1), extend: false }),
        );
    }

    #[test]
    fn a_shift_that_is_not_a_range_is_not_a_command_at_all() {
        for code in [KeyCode::KeyA, KeyCode::KeyB, KeyCode::ReturnKey, KeyCode::KeyN] {
            assert_eq!(
                command_for_key(code, shifted()),
                None,
                "{code:?} under shift is a range gesture, and must not reach a deck",
            );
        }
    }

    #[test]
    fn a_key_with_a_modifier_the_library_did_not_ask_for_is_not_a_command() {
        for mods in [
            KeyModifiers { control: true, ..Default::default() },
            KeyModifiers { alt: true, ..Default::default() },
            KeyModifiers { logo: true, ..Default::default() },
            KeyModifiers { control: true, shift: true, ..Default::default() },
        ] {
            for (code, _) in LIBRARY_KEYS {
                assert_eq!(command_for_key(code, mods), None, "{code:?} with {mods:?}");
            }
        }
    }

    #[test]
    fn a_key_the_library_does_not_use_is_left_alone() {
        for code in [KeyCode::KeyZ, KeyCode::Escape, KeyCode::F3, KeyCode::Space] {
            assert_eq!(command_for_key(code, plain()), None);
        }
    }

    #[test]
    fn a_first_arrow_lands_on_the_end_it_came_from() {
        assert_eq!(step_cursor(None, 10, 5, CursorMove::Rows(1)), Some(0), "down reaches the top");
        assert_eq!(step_cursor(None, 10, 5, CursorMove::Rows(-1)), Some(9), "up reaches the last");
        assert_eq!(step_cursor(None, 10, 5, CursorMove::Pages(1)), Some(0));
    }

    #[test]
    fn walking_past_either_end_stays_on_it() {
        assert_eq!(step_cursor(Some(9), 10, 5, CursorMove::Rows(1)), Some(9));
        assert_eq!(step_cursor(Some(0), 10, 5, CursorMove::Rows(-1)), Some(0));
        // The clamp has to survive the page arithmetic going far past both
        // ends, which is where an unsigned cast would have wrapped it.
        assert_eq!(step_cursor(Some(1), 10, 900, CursorMove::Pages(-1)), Some(0));
        assert_eq!(step_cursor(Some(1), 10, 900, CursorMove::Pages(1)), Some(9));
    }

    #[test]
    fn the_ends_are_the_ends_whatever_the_page_size() {
        for page in [1, 5, 999] {
            assert_eq!(step_cursor(Some(4), 10, page, CursorMove::First), Some(0));
            assert_eq!(step_cursor(Some(4), 10, page, CursorMove::Last), Some(9));
        }
    }

    #[test]
    fn an_empty_listing_has_no_cursor_to_move() {
        for step in
            [CursorMove::Rows(1), CursorMove::Rows(-1), CursorMove::First, CursorMove::Last]
        {
            assert_eq!(step_cursor(Some(3), 0, 5, step), None);
            assert_eq!(step_cursor(None, 0, 5, step), None);
        }
    }

    #[test]
    fn a_page_keeps_one_row_of_the_last_page_and_is_never_nothing() {
        assert_eq!(page_rows(12), 11, "one row of overlap, so the edge row stays on screen");
        assert_eq!(page_rows(2), 1);
        assert_eq!(page_rows(1), 1, "a page never moves by nothing");
        assert_eq!(page_rows(0), DEFAULT_PAGE_ROWS, "nothing drawn yet: a sane guess");
    }

    #[test]
    fn a_rebuilt_listing_keeps_the_cursor_on_the_record_it_was_on() {
        let now = ["e", "c", "a"];
        assert_eq!(remap_cursor(Some(&"c"), &now), Some(1), "the record moved, the cursor with it");
        assert_eq!(remap_cursor(Some(&"b"), &now), None, "its record is gone");
        assert_eq!(remap_cursor(None::<&&str>, &now), None);
    }

    #[test]
    fn a_pick_and_the_cursor_survive_a_rebuild_together() {
        let was = ["b", "d"];
        let now = ["a", "b", "c", "e"];
        assert_eq!(remap_picks(&was, &now), vec![1], "d is gone, b keeps its pick");
        assert_eq!(remap_picks(&[] as &[&str], &now), Vec::<usize>::new());
        assert_eq!(remap_picks(&was, &[] as &[&str]), Vec::<usize>::new());
    }

    #[test]
    fn control_replaces_the_set_list_and_shift_plays_next() {
        assert_eq!(queue_how(plain()), QueueHow::Tail);
        assert_eq!(queue_how(shifted()), QueueHow::Next);
        assert_eq!(
            queue_how(KeyModifiers { control: true, ..Default::default() }),
            QueueHow::Replace,
        );
        assert_eq!(
            queue_how(KeyModifiers { control: true, shift: true, ..Default::default() }),
            QueueHow::Replace,
            "control wins, as the chip has always read it",
        );
    }
}

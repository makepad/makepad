//! Which parts of a deck panel are open when the console is too short to
//! show all of them.
//!
//! The panel stacks three blocks — the equalizer, the stem mix and the
//! karaoke reader — and on a short console they cannot all have room. What
//! gives today is the karaoke box, which is Fill and so absorbs every point
//! the window is short by, down to a useless 46. An accordion gives instead.
//!
//! The console decides HOW MANY blocks may be open; the operator decides
//! WHICH. A chevron is a plain toggle of the block it sits on:
//!
//! * Pressing an OPEN block folds it, and the room it leaves stays empty.
//! * Pressing a FOLDED block opens it, and if the panel is then over its
//!   allowance the block wanted longest ago gives way.
//! * The last open block cannot be folded — a panel of headings over dead
//!   space is not a state worth being able to reach.
//!
//! The empty room is the point, and it is what this module got wrong the
//! first time. Filling every slot sounds tidier, but with three blocks and
//! room for two it means folding a knob block HANDS its place to the
//! transcript — which the operator never asked for, and which then sits
//! there for good while the equalizer and the stem mix trade places
//! underneath it. Folding is the operator saying "not this one"; answering
//! it with a block they did not ask for is the panel arguing back.
//!
//! The transcript therefore starts folded whenever the panel folds at all:
//! it is the block a short console can most afford to lose, and the one
//! that has to be ASKED for rather than inherited.

/// A block of the deck panel, in the order they stand in the column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckSection {
    /// HIGH / MID / LOW / FILTER, and their kills.
    Equalizer,
    /// DRUMS / BASS / VOCALS / OTHER, and their kills and solos.
    Stems,
    /// The transcript.
    Karaoke,
}

impl DeckSection {
    pub const ALL: [DeckSection; 3] =
        [DeckSection::Equalizer, DeckSection::Stems, DeckSection::Karaoke];

    fn index(self) -> usize {
        match self {
            DeckSection::Equalizer => 0,
            DeckSection::Stems => 1,
            DeckSection::Karaoke => 2,
        }
    }
}

/// How many blocks the console has room for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fold {
    /// All three; the chevrons are not even on screen.
    #[default]
    None,
    /// Two of the three.
    Pairs,
    /// One of the three.
    Singles,
}

impl Fold {
    /// How many blocks may stand open at this stage.
    fn room(self) -> usize {
        match self {
            Fold::None => 3,
            Fold::Pairs => 2,
            Fold::Singles => 1,
        }
    }
}

/// The panel's folding state. Not per deck: on a console short enough to
/// fold, both panels are equally short, and two decks disagreeing about
/// which block is open would make the pair unreadable at a glance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeckSections {
    /// What the operator has asked to have open. A block can be wanted and
    /// still not show, when the console has run out of room for it — and
    /// then it comes back on its own as the window grows.
    wanted: [bool; 3],
    /// Most recently wanted first. Decides which of the wanted blocks the
    /// console can still afford.
    order: [DeckSection; 3],
    fold: Fold,
}

impl Default for DeckSections {
    fn default() -> Self {
        Self {
            // The transcript is the block that gives way first on a short
            // console, so a folded panel starts without it.
            wanted: [true, true, false],
            order: DeckSection::ALL,
            fold: Fold::None,
        }
    }
}

impl DeckSections {
    pub fn fold(&self) -> Fold {
        self.fold
    }

    pub fn folded(&self) -> bool {
        self.fold != Fold::None
    }

    pub fn set_fold(&mut self, fold: Fold) -> bool {
        let changed = self.fold != fold;
        self.fold = fold;
        changed
    }

    fn rank(&self, section: DeckSection) -> usize {
        self.order.iter().position(|s| *s == section).unwrap_or(0)
    }

    /// Whether a block shows its contents.
    ///
    /// Unfolded, everything does — the chevrons are not on screen at that
    /// size, so there is no state behind them to honour.
    pub fn shows(&self, section: DeckSection) -> bool {
        if self.fold == Fold::None {
            return true;
        }
        if !self.wanted[section.index()] {
            return false;
        }
        // Wanted blocks fill the room most-recently-wanted first; the rest
        // wait for the window to grow.
        let ahead = self
            .order
            .iter()
            .take(self.rank(section))
            .filter(|other| self.wanted[other.index()])
            .count();
        ahead < self.fold.room()
    }

    /// A chevron was pressed: that block opens if it was folded, and folds
    /// if it was open. Returns whether the panel changed.
    pub fn press(&mut self, section: DeckSection) -> bool {
        if self.fold == Fold::None {
            return false;
        }
        let before = self.showing();
        if self.shows(section) {
            // Never the last one: a panel has to be showing something.
            if before.len() > 1 {
                self.wanted[section.index()] = false;
                self.demote(section);
            }
        } else {
            self.wanted[section.index()] = true;
            self.promote(section);
        }
        self.showing() != before
    }

    fn promote(&mut self, section: DeckSection) {
        let at = self.rank(section);
        self.order[..=at].rotate_right(1);
    }

    fn demote(&mut self, section: DeckSection) {
        let at = self.rank(section);
        self.order[at..].rotate_left(1);
    }

    /// The blocks on screen, in the panel's own top-to-bottom order.
    pub fn showing(&self) -> Vec<DeckSection> {
        DeckSection::ALL.into_iter().filter(|s| self.shows(*s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeckSection::*;

    #[test]
    fn an_unfolded_panel_shows_everything_and_ignores_its_chevrons() {
        let mut s = DeckSections::default();
        assert!(!s.folded());
        assert_eq!(s.showing(), DeckSection::ALL);
        // The chevrons are not even on screen at this size; a stray press
        // must not quietly rearrange the panel behind them.
        assert!(!s.press(Karaoke));
        assert_eq!(s.showing(), DeckSection::ALL);
    }

    #[test]
    fn folding_a_block_never_summons_the_transcript() {
        // The bug this rule exists for: folding one knob block used to hand
        // its room to the karaoke box, which then sat there for good while
        // the two knob blocks traded places underneath it.
        let mut s = DeckSections::default();
        s.set_fold(Fold::Pairs);
        assert_eq!(s.showing(), vec![Equalizer, Stems]);

        s.press(Equalizer);
        assert_eq!(s.showing(), vec![Stems], "the room stays empty, not filled");

        // And it stays gone however long the operator works the other two.
        s.press(Equalizer);
        s.press(Stems);
        s.press(Equalizer);
        assert!(
            !s.showing().contains(&Karaoke),
            "the transcript opens when it is ASKED for, never as a side effect"
        );
    }

    #[test]
    fn a_chevron_toggles_its_own_block() {
        let mut s = DeckSections::default();
        s.set_fold(Fold::Pairs);
        // Closed by default at this size: press it and it opens, at the
        // cost of the stem mix — open, but wanted longer ago.
        s.press(Karaoke);
        assert_eq!(s.showing(), vec![Equalizer, Karaoke]);
        // Press it again and it closes again — the same mark, the same
        // block, both ways — and the stem mix, which was only ever crowded
        // out, comes back on its own.
        s.press(Karaoke);
        assert_eq!(s.showing(), vec![Equalizer, Stems]);
    }

    #[test]
    fn opening_a_third_block_costs_the_one_wanted_longest_ago() {
        let mut s = DeckSections::default();
        s.set_fold(Fold::Pairs);
        assert_eq!(s.showing(), vec![Equalizer, Stems]);
        // Both knob blocks are open; asking for the transcript has to cost
        // something, and it costs the block wanted longest ago — the stem
        // mix, which stands below the equalizer in the panel's own order.
        s.press(Karaoke);
        assert_eq!(s.showing(), vec![Equalizer, Karaoke]);
    }

    #[test]
    fn with_room_for_one_the_chevron_you_press_is_the_one_that_opens() {
        let mut s = DeckSections::default();
        s.set_fold(Fold::Singles);
        for chosen in [Karaoke, Stems, Equalizer, Karaoke] {
            s.press(chosen);
            assert_eq!(s.showing(), vec![chosen], "one at a time, and the one pressed");
        }
    }

    #[test]
    fn a_console_that_loosens_hands_the_blocks_back() {
        let mut s = DeckSections::default();
        s.set_fold(Fold::Pairs);
        s.press(Karaoke);
        assert_eq!(s.showing(), vec![Equalizer, Karaoke]);

        // Tightening keeps the block most recently wanted of those two.
        s.set_fold(Fold::Singles);
        assert_eq!(s.showing(), vec![Karaoke]);

        // Loosening returns the panel to what it looked like rather than to
        // a default.
        s.set_fold(Fold::Pairs);
        assert_eq!(s.showing(), vec![Equalizer, Karaoke]);
        s.set_fold(Fold::None);
        assert_eq!(s.showing(), DeckSection::ALL);
    }

    #[test]
    fn the_last_open_block_cannot_be_folded_away() {
        let mut s = DeckSections::default();
        s.set_fold(Fold::Singles);
        s.press(Karaoke);
        // Pressing the only open block leaves it alone rather than leaving
        // the panel with nothing but headings.
        assert!(!s.press(Karaoke), "nothing changed");
        assert_eq!(s.showing(), vec![Karaoke]);
    }

    #[test]
    fn no_press_at_any_stage_overfills_or_empties_the_panel() {
        // The invariant: never more than the console has room for, and
        // never nothing at all. Fewer IS allowed — that is the operator
        // folding a block and the panel taking them at their word.
        for fold in [Fold::Pairs, Fold::Singles] {
            let mut s = DeckSections::default();
            s.set_fold(fold);
            for round in 0..12 {
                s.press(DeckSection::ALL[round % 3]);
                let showing = s.showing();
                assert!(
                    !showing.is_empty() && showing.len() <= fold.room(),
                    "{fold:?} after {round} presses: {showing:?}"
                );
            }
        }
    }
}

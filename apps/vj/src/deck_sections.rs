//! Which parts of a deck panel are open when the console is too short to
//! show all of them.
//!
//! The panel stacks three blocks — the equalizer, the stem mix and the
//! karaoke reader — and on a short console they cannot all have room. What
//! gives today is the karaoke box, which is Fill and so absorbs every point
//! the window is short by, down to a useless 46. An accordion gives instead.
//!
//! The console decides HOW MANY blocks fit; the operator decides WHICH. So
//! this is not a stack of independent switches but a running order: the
//! blocks the hand asked for most recently, and the console takes as many
//! off the top as it has room for.
//!
//! That makes the two stages read the way an operator expects without any
//! special cases:
//!
//! * Room for two of three — pressing a chevron FOLDS that block. It goes to
//!   the back of the order, and the other two are the two that fit.
//! * Room for one of three — pressing a chevron OPENS that block. It goes to
//!   the front, and it is the one that fits.
//!
//! One gesture, one meaning ("I want this block dealt with"), and the same
//! order carries across when the window changes size: a console that
//! tightens keeps the blocks the hand reached for most recently, and one
//! that loosens hands them back in the order they were given up.

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
    /// How many blocks stay open at this stage.
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
    /// Most recently wanted first. The console shows as many off the front
    /// as it has room for.
    order: [DeckSection; 3],
    fold: Fold,
}

impl Default for DeckSections {
    fn default() -> Self {
        // The transcript is last by default: it is the block that gives way
        // first on a short console today, and the one the panel can most
        // afford to lose.
        Self { order: DeckSection::ALL, fold: Fold::None }
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
    pub fn shows(&self, section: DeckSection) -> bool {
        self.rank(section) < self.fold.room()
    }

    /// A chevron was pressed.
    ///
    /// With room for more than one block the press FOLDS what it points at —
    /// the operator is saying which one they can do without, and the rest
    /// stay. With room for only one it OPENS instead, because folding the
    /// last block would leave a panel of headings over dead space.
    pub fn press(&mut self, section: DeckSection) -> bool {
        if self.fold == Fold::None {
            return false;
        }
        let before = self.showing();
        if self.fold.room() > 1 && self.shows(section) {
            self.demote(section);
        } else {
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
    fn with_room_for_two_the_chevron_you_press_is_the_one_that_folds() {
        let mut s = DeckSections::default();
        s.set_fold(Fold::Pairs);
        // The transcript starts folded — the block a short panel can most
        // afford to lose.
        assert_eq!(s.showing(), vec![Equalizer, Stems]);

        // Press the equalizer: IT folds, and the other two are the two that
        // fit — including the transcript, which comes back.
        assert!(s.press(Equalizer));
        assert_eq!(s.showing(), vec![Stems, Karaoke]);

        // Press the stem mix: same again.
        assert!(s.press(Stems));
        assert_eq!(s.showing(), vec![Equalizer, Karaoke]);

        // And pressing the one that is ALREADY folded opens it, folding
        // whichever has gone longest unwanted.
        assert!(s.press(Stems));
        assert_eq!(s.showing(), vec![Stems, Karaoke]);
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
    fn the_running_order_carries_across_a_console_that_changes_size() {
        let mut s = DeckSections::default();
        // Two of three, and the operator folds the equalizer away.
        s.set_fold(Fold::Pairs);
        s.press(Equalizer);
        assert_eq!(s.showing(), vec![Stems, Karaoke]);

        // Tightening keeps the block most recently wanted of those two.
        s.set_fold(Fold::Singles);
        assert_eq!(s.showing(), vec![Stems]);

        // Loosening hands them back in the order they were given up, so the
        // panel returns to what it looked like rather than to a default.
        s.set_fold(Fold::Pairs);
        assert_eq!(s.showing(), vec![Stems, Karaoke]);
        s.set_fold(Fold::None);
        assert_eq!(s.showing(), DeckSection::ALL);
    }

    #[test]
    fn the_last_open_block_cannot_be_folded_away() {
        let mut s = DeckSections::default();
        s.set_fold(Fold::Singles);
        s.press(Karaoke);
        // Pressing the only open block re-opens it rather than leaving the
        // panel with nothing but headings.
        assert!(!s.press(Karaoke), "nothing changed");
        assert_eq!(s.showing(), vec![Karaoke]);
    }

    #[test]
    fn every_press_at_every_stage_leaves_the_panel_showing_what_it_has_room_for() {
        // The invariant the whole thing rests on: as many blocks as the
        // console has room for, never more and never fewer.
        for fold in [Fold::Pairs, Fold::Singles] {
            let mut s = DeckSections::default();
            s.set_fold(fold);
            for round in 0..12 {
                s.press(DeckSection::ALL[round % 3]);
                assert_eq!(
                    s.showing().len(),
                    fold.room(),
                    "{fold:?} after {round} presses: {:?}",
                    s.showing()
                );
            }
        }
    }
}

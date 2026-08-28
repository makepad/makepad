//! Which deck the tabbed control panel is showing, and what may move it.
//!
//! When the console is too narrow to stand its deck panels side by side, one
//! panel shows at a time behind a strip of tabs. That is a real hazard in a
//! live set: half the knobs are off screen, so if the visible deck changes
//! under the operator's hands they will turn the wrong one. The mode is
//! therefore the operator's to choose, and it is on screen — a hand, a
//! reticle, a crossfader, a pin — rather than a rule they have to remember.
//!
//! Deliberately counted, not lettered. The console has two decks today, but
//! the whole point of tabbing is that it keeps working when there are four
//! or six, so nothing here knows about A and B — a deck is an index, and how
//! many there are is a number that can change.

/// What is allowed to change the deck on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TabFollow {
    /// Nothing but a tab press. What is under the hands never moves on its
    /// own — the safe choice, and the reason it is offered first.
    #[default]
    Manual,
    /// The load target: the tab and the target are one control, so pressing
    /// a tab also aims the library at that deck.
    Target,
    /// Whichever deck the room can hear. Follows the crossfader, which means
    /// it moves DURING a mix — the most automatic and the least safe.
    Audible,
    /// The load target, until a tab is pressed; then it holds there until
    /// the target next moves.
    Pinned,
}

/// What a tab press asks of the rest of the console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Press {
    /// The deck now on screen.
    pub shown: usize,
    /// Point the load target at it too. Only in [`TabFollow::Target`], where
    /// the tab IS the target — otherwise a press would spring back and the
    /// tab would look broken.
    pub aim_target: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeckTabs {
    decks: usize,
    shown: usize,
    follow: TabFollow,
    /// [`TabFollow::Pinned`] only: a press is holding the tab against the
    /// target it would otherwise follow.
    held: bool,
}

impl Default for DeckTabs {
    fn default() -> Self {
        Self { decks: 2, shown: 0, follow: TabFollow::default(), held: false }
    }
}

impl DeckTabs {
    pub fn new(decks: usize) -> Self {
        Self { decks: decks.max(1), ..Self::default() }
    }

    pub fn shown(&self) -> usize {
        self.shown
    }

    pub fn follow(&self) -> TabFollow {
        self.follow
    }

    pub fn decks(&self) -> usize {
        self.decks
    }

    /// A deck arriving or leaving. The shown tab follows it inside the range
    /// rather than pointing at a deck that is no longer there.
    pub fn set_decks(&mut self, decks: usize) {
        self.decks = decks.max(1);
        self.shown = self.shown.min(self.decks - 1);
    }

    fn show(&mut self, deck: usize) -> bool {
        let deck = deck.min(self.decks - 1);
        let moved = self.shown != deck;
        self.shown = deck;
        moved
    }

    /// The operator pressed a tab.
    pub fn press(&mut self, deck: usize) -> Press {
        self.show(deck);
        if self.follow == TabFollow::Pinned {
            self.held = true;
        }
        Press { shown: self.shown, aim_target: self.follow == TabFollow::Target }
    }

    /// The operator picked a mode. It takes effect at once — a mode that
    /// only came true at the next mix would leave the icons lying about what
    /// is on screen.
    pub fn set_follow(
        &mut self,
        follow: TabFollow,
        target: Option<usize>,
        audible: usize,
    ) -> bool {
        self.follow = follow;
        self.held = false;
        match follow {
            TabFollow::Manual => false,
            TabFollow::Target | TabFollow::Pinned => {
                target.map(|deck| self.show(deck)).unwrap_or(false)
            }
            TabFollow::Audible => self.show(audible),
        }
    }

    /// The library is now aimed at a deck — or at none of them, which the
    /// crossfader's own targets can be, and which names no tab to move to.
    pub fn target_moved(&mut self, target: Option<usize>) -> bool {
        let Some(deck) = target else { return false };
        match self.follow {
            TabFollow::Target => self.show(deck),
            TabFollow::Pinned => {
                // The target moving is what releases a held pin: the
                // operator has said where they are working next.
                self.held = false;
                self.show(deck)
            }
            _ => false,
        }
    }

    /// The mix moved and a different deck is audible.
    pub fn audible_moved(&mut self, audible: usize) -> bool {
        if self.follow == TabFollow::Audible {
            self.show(audible)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: usize = 0;
    const B: usize = 1;

    #[test]
    fn a_hand_is_the_only_thing_that_moves_a_manual_tab() {
        let mut tabs = DeckTabs::new(2);
        assert_eq!(tabs.follow(), TabFollow::Manual, "the safe mode is the default");
        assert!(!tabs.target_moved(Some(B)), "the target does not move it");
        assert!(!tabs.audible_moved(B), "nor does the mix");
        assert_eq!(tabs.shown(), A);
        assert_eq!(tabs.press(B), Press { shown: B, aim_target: false });
        assert_eq!(tabs.shown(), B);
        // And it stays there through anything the console does on its own.
        assert!(!tabs.target_moved(Some(A)));
        assert!(!tabs.audible_moved(A));
        assert_eq!(tabs.shown(), B);
    }

    #[test]
    fn the_target_mode_makes_the_tab_and_the_target_one_control() {
        let mut tabs = DeckTabs::new(2);
        tabs.set_follow(TabFollow::Target, Some(A), A);
        assert!(tabs.target_moved(Some(B)));
        assert_eq!(tabs.shown(), B, "aiming at a deck shows it");
        // Pressing a tab aims the library rather than springing back.
        assert_eq!(tabs.press(A), Press { shown: A, aim_target: true });
        // The mix is not one of its sources.
        assert!(!tabs.audible_moved(B));
        assert_eq!(tabs.shown(), A);
        // A target that names no deck (the mix, or nothing) moves nothing.
        assert!(!tabs.target_moved(None));
        assert_eq!(tabs.shown(), A);
    }

    #[test]
    fn the_audible_mode_follows_the_mix_and_a_press_does_not_hold() {
        let mut tabs = DeckTabs::new(2);
        tabs.set_follow(TabFollow::Audible, Some(A), A);
        assert!(tabs.audible_moved(B));
        assert_eq!(tabs.shown(), B);
        // A press shows the other deck, but the next crossfade takes it
        // back — which is exactly why the pin exists.
        assert_eq!(tabs.press(A), Press { shown: A, aim_target: false });
        assert!(tabs.audible_moved(B));
        assert_eq!(tabs.shown(), B);
        assert!(!tabs.target_moved(Some(A)), "the target is not its source");
    }

    #[test]
    fn a_pin_holds_the_tab_until_the_target_next_moves() {
        let mut tabs = DeckTabs::new(2);
        tabs.set_follow(TabFollow::Pinned, Some(A), A);
        assert!(tabs.target_moved(Some(B)));
        assert_eq!(tabs.shown(), B, "unpinned, it follows the target");

        // Pressed: now it holds, and the target it was following is ignored.
        assert_eq!(tabs.press(A), Press { shown: A, aim_target: false });
        assert_eq!(tabs.shown(), A);

        // The target MOVING is what releases it — the operator has said
        // where they are working next.
        assert!(tabs.target_moved(Some(B)));
        assert_eq!(tabs.shown(), B);
        assert!(tabs.target_moved(Some(A)), "and it follows freely again");
        assert_eq!(tabs.shown(), A);
    }

    #[test]
    fn picking_a_mode_takes_effect_at_once() {
        let mut tabs = DeckTabs::new(2);
        tabs.press(A);
        // Target and Pinned snap to the target; Audible snaps to the mix.
        assert!(tabs.set_follow(TabFollow::Target, Some(B), A));
        assert_eq!(tabs.shown(), B);
        assert!(tabs.set_follow(TabFollow::Audible, Some(B), A));
        assert_eq!(tabs.shown(), A);
        assert!(tabs.set_follow(TabFollow::Pinned, Some(B), A));
        assert_eq!(tabs.shown(), B);
        // Manual takes what is already there rather than jumping.
        assert!(!tabs.set_follow(TabFollow::Manual, Some(A), A));
        assert_eq!(tabs.shown(), B);

        // Choosing a mode also releases a pin, so the icons never disagree
        // with what the panel is doing.
        tabs.set_follow(TabFollow::Pinned, Some(B), A);
        tabs.press(A);
        tabs.set_follow(TabFollow::Pinned, Some(B), A);
        assert_eq!(tabs.shown(), B, "the pin did not survive being re-chosen");
    }

    #[test]
    fn it_counts_decks_rather_than_lettering_them() {
        // Four decks, which is where this is going.
        let mut tabs = DeckTabs::new(4);
        tabs.set_follow(TabFollow::Target, Some(0), 0);
        for deck in 0..4 {
            assert!(tabs.target_moved(Some(deck)) || tabs.shown() == deck);
            assert_eq!(tabs.shown(), deck);
        }
        // A deck that is not there cannot be shown.
        assert_eq!(tabs.press(9).shown, 3);

        // Decks going away take the tab back inside the range with them.
        tabs.set_decks(2);
        assert_eq!(tabs.shown(), 1);
        tabs.set_decks(1);
        assert_eq!(tabs.shown(), 0);
        // And a console can never have no decks at all to tab between.
        tabs.set_decks(0);
        assert_eq!(tabs.decks(), 1);
        assert_eq!(tabs.shown(), 0);
    }
}

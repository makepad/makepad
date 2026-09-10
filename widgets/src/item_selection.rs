//! Which of an ordered set of things are chosen, and what a press or an
//! arrow key does to that.
//!
//! This is the half of a selectable list that has no pixels: no area, no
//! shader, no actions, no widget. `ListItem` and its neighbours own the
//! other half — they have a `selected` flag to set and nothing that says
//! what to set it to — and three widgets in this library have each grown
//! their own private answer, which is how a shift-press came to mean
//! three different things in one library.
//!
//! The display order is handed in on every call and never stored. A list
//! that sorts, filters or folds is a different order between one press
//! and the next, and a model holding its own stale copy would resolve a
//! range against rows that have moved. The cost is a scan of the order
//! per press, which is nothing next to laying the rows out.
//!
//! The rules in one paragraph. A plain press takes one thing and drops
//! the rest. The primary key (command on one family of machines, control
//! on the rest) flips one thing and leaves the others. Shift sweeps the
//! range between the anchor and the thing pressed, laid over the *ground*
//! — what was chosen at the moment the anchor was put down — so a second
//! shift-press nearer the anchor shortens the range instead of only ever
//! growing it, and whatever was picked before the anchor survives. Only
//! the presses that are not sweeps move the anchor; a sweep moves the
//! cursor, which is the end that follows the finger.

use {
    crate::makepad_platform::KeyModifiers,
    std::{collections::HashSet, hash::Hash},
};

/// How many things a list may hold at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    /// At most one. Ranges have nothing to span, so every gesture but a
    /// toggle lands on the thing under the finger.
    One,
    #[default]
    Many,
}

impl SelectionMode {
    /// What a gesture means in this mode.
    fn resolve(self, gesture: SelectionGesture) -> SelectionGesture {
        match (self, gesture) {
            // A toggle survives into a one-of list because letting go of
            // the single choice is otherwise unreachable: there is no
            // second thing to press instead.
            (SelectionMode::One, SelectionGesture::Toggle) => SelectionGesture::Toggle,
            (SelectionMode::One, _) => SelectionGesture::Replace,
            (SelectionMode::Many, gesture) => gesture,
        }
    }
}

/// What a press of the pointer asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionGesture {
    /// This thing and nothing else.
    Replace,
    /// This thing on or off, the rest untouched.
    Toggle,
    /// The range from the anchor to here, over the ground.
    Extend,
    /// The range from the anchor to here, added to what is chosen now,
    /// and that becomes the ground the next sweep is laid over.
    ToggleExtend,
}

impl SelectionGesture {
    /// The gesture a press with these modifiers means.
    ///
    /// The host hands the modifiers over rather than naming a key,
    /// because which key is the additive one is a property of the
    /// machine and [`KeyModifiers::is_primary`] is where that already
    /// lives. Alt is ignored: it belongs to whatever the list does with
    /// a press, not to what the press selects.
    pub fn from_modifiers(modifiers: KeyModifiers) -> Self {
        match (modifiers.shift, modifiers.is_primary()) {
            (true, true) => SelectionGesture::ToggleExtend,
            (true, false) => SelectionGesture::Extend,
            (false, true) => SelectionGesture::Toggle,
            (false, false) => SelectionGesture::Replace,
        }
    }
}

/// Where a key press sends the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionStep {
    Prev,
    Next,
    First,
    Last,
    /// So far, for a page key. Clamps at the ends like a single step.
    By(isize),
}

/// What a key press does to the selection while it moves the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMove {
    /// Take whatever it lands on. What a bare arrow does: a list whose
    /// arrows move a ring and leave the selection behind makes the
    /// reader press a second key to mean what they already meant.
    Replace,
    /// Sweep from the anchor to whatever it lands on.
    Extend,
    /// Move and choose nothing, so a reader can walk past things they
    /// have already picked without losing them. Pairs with
    /// [`ItemSelection::toggle_cursor`], which is what makes the walk
    /// worth anything.
    Cursor,
}

impl SelectionMove {
    /// What a key press with these modifiers does.
    ///
    /// Shift with the primary key held reads as a plain sweep. Keeping a
    /// second range alive across a walk would need the ground to be put
    /// down again mid-walk, and the ground is only ever put down by a
    /// press.
    pub fn from_modifiers(modifiers: KeyModifiers) -> Self {
        if modifiers.shift {
            SelectionMove::Extend
        } else if modifiers.is_primary() {
            SelectionMove::Cursor
        } else {
            SelectionMove::Replace
        }
    }
}

/// What one call moved.
///
/// Two flags rather than one because a host wants different things from
/// them: it redraws on [`SelectionChange::any`], and reports a new
/// selection on `chosen`. A walk with the primary key held moves the
/// cursor and chooses nothing, and reporting that as a selection change
/// would make every arrow press look like a fresh answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionChange {
    pub chosen: bool,
    pub cursor: bool,
}

impl SelectionChange {
    pub fn any(&self) -> bool {
        self.chosen || self.cursor
    }
}

/// Which of an ordered set of things are chosen.
///
/// `Id` is whatever the host calls a thing — an interned id, a row
/// number, a date — so long as two of them can be compared and hashed.
/// Nothing here knows how the things are drawn or how many there are.
#[derive(Clone, Debug)]
pub struct ItemSelection<Id> {
    mode: SelectionMode,
    chosen: HashSet<Id>,
    /// What was chosen at the moment the anchor was put down. A sweep is
    /// laid over this rather than over the live set, which is what lets a
    /// second shift-press nearer the anchor shorten the range: laying it
    /// over the live set would make every sweep a union, and the gesture
    /// people use to correct an over-long drag would do nothing.
    ground: HashSet<Id>,
    anchor: Option<Id>,
    cursor: Option<Id>,
}

impl<Id: Copy + Eq + Hash> Default for ItemSelection<Id> {
    fn default() -> Self {
        Self::new(SelectionMode::Many)
    }
}

impl<Id: Copy + Eq + Hash> ItemSelection<Id> {
    pub fn new(mode: SelectionMode) -> Self {
        Self {
            mode,
            chosen: HashSet::new(),
            ground: HashSet::new(),
            anchor: None,
            cursor: None,
        }
    }

    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// The end a sweep measures from, if there is one.
    pub fn anchor(&self) -> Option<Id> {
        self.anchor
    }

    /// The thing the keys are standing on. Not the same as the
    /// selection: a swept range has one cursor and many chosen things.
    pub fn cursor(&self) -> Option<Id> {
        self.cursor
    }

    pub fn is_selected(&self, id: Id) -> bool {
        self.chosen.contains(&id)
    }

    pub fn len(&self) -> usize {
        self.chosen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chosen.is_empty()
    }

    /// The chosen things, in the order handed in.
    ///
    /// Filtered by that order, so anything chosen but not currently on
    /// show is left out of the answer without being let go of. A fold or
    /// a filter hides a thing; it does not deselect it, and dropping it
    /// would make a stray press quietly throw away work.
    pub fn chosen(&self, order: &[Id]) -> Vec<Id> {
        order.iter().copied().filter(|id| self.chosen.contains(id)).collect()
    }

    /// Press a thing.
    ///
    /// A press on something not in `order` does nothing at all: the
    /// order is the list as it stands, and acting on a thing that is not
    /// in it would resolve a range against a position that does not
    /// exist.
    pub fn click(&mut self, order: &[Id], id: Id, gesture: SelectionGesture) -> SelectionChange {
        if index_of(order, id).is_none() {
            return SelectionChange::default();
        }
        let before = self.snapshot();
        match self.mode.resolve(gesture) {
            SelectionGesture::Replace => {
                self.chosen.clear();
                self.chosen.insert(id);
                self.drop_anchor(id);
            }
            SelectionGesture::Toggle => {
                if !self.chosen.remove(&id) {
                    if self.mode == SelectionMode::One {
                        self.chosen.clear();
                    }
                    self.chosen.insert(id);
                }
                // The anchor moves whether the press chose the thing or
                // let go of it: it marks where the finger last was, and
                // that is the end a following sweep should measure from.
                self.drop_anchor(id);
            }
            SelectionGesture::Extend => {
                let ground = self.ground.clone();
                self.sweep(order, id, ground);
            }
            SelectionGesture::ToggleExtend => {
                // The live set becomes the ground before the range goes
                // on, so the range this press makes can still be swept
                // shorter afterwards while the ranges under it stay.
                let ground = self.chosen.clone();
                self.ground = ground.clone();
                self.sweep(order, id, ground);
            }
        }
        self.change_since(before)
    }

    /// Move the cursor and do `kind` to the selection on the way.
    pub fn key_move(
        &mut self,
        order: &[Id],
        step: SelectionStep,
        kind: SelectionMove,
    ) -> SelectionChange {
        let Some(id) = self.step_target(order, step) else {
            return SelectionChange::default();
        };
        match kind {
            SelectionMove::Replace => self.click(order, id, SelectionGesture::Replace),
            SelectionMove::Extend => self.click(order, id, SelectionGesture::Extend),
            SelectionMove::Cursor => {
                let before = self.snapshot();
                self.cursor = Some(id);
                self.change_since(before)
            }
        }
    }

    /// Flip the thing the cursor is standing on — the space bar, and the
    /// only way a cursor-only walk ends in a decision.
    pub fn toggle_cursor(&mut self, order: &[Id]) -> SelectionChange {
        let Some(id) = self.cursor else {
            return SelectionChange::default();
        };
        self.click(order, id, SelectionGesture::Toggle)
    }

    /// Take everything on show. A one-of list ignores it rather than
    /// settling on one of them arbitrarily.
    pub fn select_all(&mut self, order: &[Id]) -> SelectionChange {
        if self.mode == SelectionMode::One || order.is_empty() {
            return SelectionChange::default();
        }
        let before = self.snapshot();
        self.chosen = order.iter().copied().collect();
        // No ground: a wholesale set is an answer, not something a
        // reader was building on, so the next sweep replaces it. With
        // the ground left full instead, a shift-arrow after this could
        // never shrink the selection again.
        self.ground.clear();
        self.anchor = Some(order[0]);
        if self.cursor.and_then(|id| index_of(order, id)).is_none() {
            self.cursor = Some(order[0]);
        }
        self.change_since(before)
    }

    /// Let go of everything. The cursor stays where it was so the arrows
    /// carry on from where the reader was looking; the anchor does not,
    /// because an empty selection has no range to have been measuring.
    pub fn clear(&mut self) -> SelectionChange {
        let before = self.snapshot();
        self.chosen.clear();
        self.ground.clear();
        self.anchor = None;
        self.change_since(before)
    }

    /// Put a selection in from outside — a restore, or a host that owns
    /// the truth. Anything not in `order` is dropped, and the anchor
    /// lands on the last of them so a shift-press straight afterwards
    /// has an end to measure from instead of behaving like a plain one.
    pub fn set_chosen(&mut self, order: &[Id], ids: &[Id]) -> SelectionChange {
        let before = self.snapshot();
        let live: HashSet<Id> = order.iter().copied().collect();
        self.chosen = ids.iter().copied().filter(|id| live.contains(id)).collect();
        if self.mode == SelectionMode::One && self.chosen.len() > 1 {
            let keep = order.iter().copied().find(|id| self.chosen.contains(id));
            self.chosen.clear();
            self.chosen.extend(keep);
        }
        self.ground.clear();
        let last = order.iter().copied().rev().find(|id| self.chosen.contains(id));
        self.anchor = last;
        self.cursor = last;
        self.change_since(before)
    }

    /// Forget things that have left the list for good.
    ///
    /// Not to be called on a filter or a fold: those hide things, and a
    /// hidden thing is still chosen. This is for a delete, where the
    /// alternative is a selection that quietly grows a set of ids
    /// nothing can ever show or clear again.
    pub fn retain(&mut self, order: &[Id]) -> SelectionChange {
        let before = self.snapshot();
        let live: HashSet<Id> = order.iter().copied().collect();
        self.chosen.retain(|id| live.contains(id));
        self.ground.retain(|id| live.contains(id));
        self.anchor = self.anchor.filter(|id| live.contains(id));
        self.cursor = self.cursor.filter(|id| live.contains(id));
        self.change_since(before)
    }

    /// Lay the range from the anchor to `id` over `ground`.
    fn sweep(&mut self, order: &[Id], id: Id, ground: HashSet<Id>) {
        // The order arrives fresh each call, so the anchor may name
        // something that has since been filtered away.
        let from = self.anchor.and_then(|anchor| index_of(order, anchor));
        let (Some(from), Some(to)) = (from, index_of(order, id)) else {
            // Nowhere to measure from. Put the anchor under the finger
            // and take that one thing, which reads as a plain press on
            // an empty selection and as an addition to one that already
            // held things — either way a press whose only fault is that
            // the list moved underneath it loses nothing.
            self.chosen = ground;
            self.chosen.insert(id);
            self.anchor = Some(id);
            self.ground = self.chosen.clone();
            self.cursor = Some(id);
            return;
        };
        let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
        self.chosen = ground;
        for id in &order[lo..=hi] {
            self.chosen.insert(*id);
        }
        // The anchor stays: it is the end that does not move, which is
        // what makes a sweep repeatable from the same place.
        self.cursor = Some(id);
    }

    /// Put the anchor here and remember what is chosen at this moment.
    fn drop_anchor(&mut self, id: Id) {
        self.anchor = Some(id);
        self.cursor = Some(id);
        self.ground = self.chosen.clone();
    }

    fn step_target(&self, order: &[Id], step: SelectionStep) -> Option<Id> {
        if order.is_empty() {
            return None;
        }
        let last = order.len() - 1;
        let at = self.cursor.and_then(|id| index_of(order, id));
        let to = match step {
            SelectionStep::First => 0,
            SelectionStep::Last => last,
            SelectionStep::Prev => walk(at, -1, last),
            SelectionStep::Next => walk(at, 1, last),
            SelectionStep::By(by) => walk(at, by, last),
        };
        Some(order[to])
    }

    fn snapshot(&self) -> (HashSet<Id>, Option<Id>) {
        (self.chosen.clone(), self.cursor)
    }

    fn change_since(&self, before: (HashSet<Id>, Option<Id>)) -> SelectionChange {
        SelectionChange {
            chosen: self.chosen != before.0,
            cursor: self.cursor != before.1,
        }
    }
}

/// Where `id` sits in the order handed in, or nothing if it is not on show.
fn index_of<Id: Copy + Eq>(order: &[Id], id: Id) -> Option<usize> {
    order.iter().position(|other| *other == id)
}

/// Clamp at the ends rather than wrapping round: one held shift-arrow
/// that came back over the top would sweep the whole list, and there is
/// no press that undoes that except starting again.
fn walk(at: Option<usize>, by: isize, last: usize) -> usize {
    match at {
        Some(at) => (at as isize + by).clamp(0, last as isize) as usize,
        // Nothing under the cursor yet: start from the end the walk is
        // coming from, so one press of the down key reaches the first
        // thing rather than the second.
        None => {
            if by < 0 {
                last
            } else {
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six things in display order. The values are deliberately not
    /// their own indices: a model that quietly took an id for a position
    /// would pass every test written over `0..6`.
    const ROWS: [u32; 6] = [11, 22, 33, 44, 55, 66];

    fn many() -> ItemSelection<u32> {
        ItemSelection::new(SelectionMode::Many)
    }

    fn no_keys() -> KeyModifiers {
        KeyModifiers::default()
    }

    fn shift() -> KeyModifiers {
        KeyModifiers { shift: true, ..no_keys() }
    }

    /// Both keys down, because the primary modifier is the command key
    /// on one family of machines and the control key on the rest: a test
    /// that names one of them is a test that only runs on half of them.
    fn primary() -> KeyModifiers {
        KeyModifiers { control: true, logo: true, ..no_keys() }
    }

    fn primary_and_shift() -> KeyModifiers {
        KeyModifiers { shift: true, ..primary() }
    }

    #[test]
    fn a_plain_press_takes_one_thing_and_lets_go_of_the_rest() {
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.click(&ROWS, 55, SelectionGesture::Replace);
        assert_eq!(sel.chosen(&ROWS), vec![55]);
        assert_eq!(sel.anchor(), Some(55), "and the anchor came with it");
    }

    #[test]
    fn a_primary_press_flips_one_thing_and_leaves_the_others_alone() {
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.click(&ROWS, 55, SelectionGesture::Toggle);
        assert_eq!(sel.chosen(&ROWS), vec![22, 55]);
        sel.click(&ROWS, 22, SelectionGesture::Toggle);
        assert_eq!(sel.chosen(&ROWS), vec![55], "the first one went out again");
    }

    #[test]
    fn a_primary_press_that_lets_go_still_moves_the_anchor_to_that_thing() {
        // The thing the finger last touched is where a range measures
        // from, whether the touch took it or dropped it.
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.click(&ROWS, 44, SelectionGesture::Toggle);
        sel.click(&ROWS, 44, SelectionGesture::Toggle);
        assert!(!sel.is_selected(44));
        assert_eq!(sel.anchor(), Some(44));
        sel.click(&ROWS, 66, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![22, 44, 55, 66]);
    }

    #[test]
    fn a_shift_press_sweeps_from_the_anchor_over_what_was_chosen_before_it() {
        let mut sel = many();
        sel.click(&ROWS, 11, SelectionGesture::Replace);
        sel.click(&ROWS, 33, SelectionGesture::Toggle);
        sel.click(&ROWS, 55, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![11, 33, 44, 55], "the earlier pick survived the sweep");
    }

    #[test]
    fn a_second_shift_press_nearer_the_anchor_shortens_the_range_rather_than_adding_to_it() {
        // This is the disagreement the model exists to settle. A sweep
        // laid over the live set can only ever grow, so the gesture
        // people use to correct an over-long drag would do nothing.
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.click(&ROWS, 66, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![22, 33, 44, 55, 66]);
        sel.click(&ROWS, 44, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![22, 33, 44]);
    }

    #[test]
    fn a_shift_press_leaves_the_anchor_where_it_was_so_the_range_has_a_fixed_end() {
        let mut sel = many();
        sel.click(&ROWS, 33, SelectionGesture::Replace);
        sel.click(&ROWS, 55, SelectionGesture::Extend);
        assert_eq!(sel.anchor(), Some(33), "the end that does not move");
        assert_eq!(sel.cursor(), Some(55), "and the end that follows the finger");
    }

    #[test]
    fn a_range_reads_the_same_whichever_end_it_was_drawn_from() {
        let mut down = many();
        down.click(&ROWS, 22, SelectionGesture::Replace);
        down.click(&ROWS, 55, SelectionGesture::Extend);
        let mut up = many();
        up.click(&ROWS, 55, SelectionGesture::Replace);
        up.click(&ROWS, 22, SelectionGesture::Extend);
        assert_eq!(down.chosen(&ROWS), vec![22, 33, 44, 55]);
        assert_eq!(up.chosen(&ROWS), down.chosen(&ROWS));
    }

    #[test]
    fn a_shift_press_with_no_anchor_puts_one_down_rather_than_guessing() {
        let mut sel = many();
        sel.click(&ROWS, 44, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![44], "it read as a plain press");
        assert_eq!(sel.anchor(), Some(44));
    }

    #[test]
    fn a_shift_press_whose_anchor_has_left_the_order_keeps_what_was_chosen() {
        // The order arrives fresh on every call, so a list that filtered
        // its rows between two presses gets here with an anchor naming a
        // row that is gone. Emptying the selection over that would throw
        // away work nobody asked to lose.
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.click(&ROWS, 33, SelectionGesture::Toggle);
        let filtered = [11u32, 22, 55, 66];
        sel.click(&filtered, 66, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&filtered), vec![22, 66]);
        assert_eq!(sel.anchor(), Some(66), "the anchor moved to the thing under the finger");
    }

    #[test]
    fn a_primary_shift_press_keeps_the_range_already_swept_where_a_plain_one_replaces_it() {
        let mut plain = many();
        plain.click(&ROWS, 44, SelectionGesture::Replace);
        plain.click(&ROWS, 22, SelectionGesture::Extend);
        assert_eq!(plain.chosen(&ROWS), vec![22, 33, 44]);
        let mut added = plain.clone();

        plain.click(&ROWS, 66, SelectionGesture::Extend);
        assert_eq!(plain.chosen(&ROWS), vec![44, 55, 66], "the sweep turned round and let the old side go");

        added.click(&ROWS, 66, SelectionGesture::ToggleExtend);
        assert_eq!(added.chosen(&ROWS), vec![22, 33, 44, 55, 66], "and this one kept it");
    }

    #[test]
    fn a_range_added_by_a_primary_shift_press_can_still_be_swept_shorter() {
        let mut sel = many();
        sel.click(&ROWS, 11, SelectionGesture::Replace);
        sel.click(&ROWS, 44, SelectionGesture::Toggle);
        sel.click(&ROWS, 66, SelectionGesture::ToggleExtend);
        assert_eq!(sel.chosen(&ROWS), vec![11, 44, 55, 66]);
        sel.click(&ROWS, 55, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![11, 44, 55], "and the range under it stayed");
    }

    #[test]
    fn a_plain_arrow_moves_the_cursor_and_takes_the_thing_it_lands_on() {
        let mut sel = many();
        sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Replace);
        assert_eq!(sel.chosen(&ROWS), vec![11]);
        sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Replace);
        assert_eq!(sel.chosen(&ROWS), vec![22], "and let go of the one before");
        assert_eq!(sel.anchor(), Some(22));
    }

    #[test]
    fn a_shift_arrow_sweeps_from_the_anchor_and_can_walk_back_over_itself() {
        let mut sel = many();
        sel.click(&ROWS, 33, SelectionGesture::Replace);
        sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Extend);
        sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![33, 44, 55]);
        sel.key_move(&ROWS, SelectionStep::Prev, SelectionMove::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![33, 44], "walking back shortened the range");
        assert_eq!(sel.anchor(), Some(33), "and the anchor never moved");
    }

    #[test]
    fn a_primary_arrow_moves_the_cursor_and_changes_nothing_that_is_chosen() {
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        let change = sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Cursor);
        assert_eq!(sel.chosen(&ROWS), vec![22]);
        assert_eq!(sel.cursor(), Some(33));
        assert_eq!(sel.anchor(), Some(22), "the anchor stayed with the choice");
        assert_eq!(change, SelectionChange { chosen: false, cursor: true });
    }

    #[test]
    fn space_after_a_primary_arrow_walk_takes_the_thing_the_cursor_is_on() {
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Cursor);
        sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Cursor);
        sel.toggle_cursor(&ROWS);
        assert_eq!(sel.chosen(&ROWS), vec![22, 44]);
        assert_eq!(sel.anchor(), Some(44), "and a sweep now measures from there");
    }

    #[test]
    fn the_arrows_stop_at_the_ends_rather_than_wrapping_round() {
        let mut sel = many();
        sel.key_move(&ROWS, SelectionStep::Last, SelectionMove::Replace);
        sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Replace);
        assert_eq!(sel.chosen(&ROWS), vec![66], "the last thing is the last thing");
        sel.key_move(&ROWS, SelectionStep::First, SelectionMove::Replace);
        sel.key_move(&ROWS, SelectionStep::Prev, SelectionMove::Replace);
        assert_eq!(sel.chosen(&ROWS), vec![11]);
    }

    #[test]
    fn an_arrow_with_no_cursor_starts_from_the_end_it_is_walking_from() {
        let mut down = many();
        down.key_move(&ROWS, SelectionStep::Next, SelectionMove::Replace);
        assert_eq!(down.chosen(&ROWS), vec![11], "one press of the down key reached the first");
        let mut up = many();
        up.key_move(&ROWS, SelectionStep::Prev, SelectionMove::Replace);
        assert_eq!(up.chosen(&ROWS), vec![66]);
    }

    #[test]
    fn a_page_step_clamps_at_the_end_like_a_single_one() {
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.key_move(&ROWS, SelectionStep::By(3), SelectionMove::Replace);
        assert_eq!(sel.chosen(&ROWS), vec![55]);
        sel.key_move(&ROWS, SelectionStep::By(10), SelectionMove::Replace);
        assert_eq!(sel.chosen(&ROWS), vec![66]);
    }

    #[test]
    fn select_all_takes_everything_in_the_order_handed_in_and_nothing_else() {
        let mut sel = many();
        let filtered = [22u32, 44, 66];
        sel.select_all(&filtered);
        assert_eq!(sel.chosen(&ROWS), vec![22, 44, 66], "a filtered list takes what it shows");
        assert_eq!(sel.anchor(), Some(22));
    }

    #[test]
    fn select_all_leaves_the_cursor_where_the_reader_had_it() {
        let mut sel = many();
        sel.click(&ROWS, 44, SelectionGesture::Replace);
        sel.select_all(&ROWS);
        assert_eq!(sel.cursor(), Some(44));
        // The anchor went to the top and no ground was left behind, so
        // one shift-arrow sweeps a range rather than being unable to
        // shrink what select-all put in.
        sel.key_move(&ROWS, SelectionStep::Prev, SelectionMove::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![11, 22, 33]);
    }

    #[test]
    fn clearing_lets_go_of_the_anchor_but_keeps_the_cursor_so_the_arrows_resume() {
        let mut sel = many();
        sel.click(&ROWS, 44, SelectionGesture::Replace);
        sel.clear();
        assert!(sel.is_empty());
        assert_eq!(sel.anchor(), None, "there is nothing left to measure a range from");
        assert_eq!(sel.cursor(), Some(44), "but the keys carry on from where they were");
        sel.key_move(&ROWS, SelectionStep::Next, SelectionMove::Replace);
        assert_eq!(sel.chosen(&ROWS), vec![55]);
    }

    #[test]
    fn a_one_of_list_never_holds_two_things_however_it_is_pressed() {
        let mut sel = ItemSelection::new(SelectionMode::One);
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.click(&ROWS, 55, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![55], "a range came out as the thing under the finger");
        sel.click(&ROWS, 33, SelectionGesture::ToggleExtend);
        assert_eq!(sel.chosen(&ROWS), vec![33]);
        sel.click(&ROWS, 33, SelectionGesture::Toggle);
        assert!(sel.is_empty(), "and the primary key still lets go of the one thing");
    }

    #[test]
    fn a_one_of_list_ignores_select_all_rather_than_settling_on_one_of_them() {
        let mut sel = ItemSelection::new(SelectionMode::One);
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        let change = sel.select_all(&ROWS);
        assert!(!change.any(), "nothing moved");
        assert_eq!(sel.chosen(&ROWS), vec![22]);
    }

    #[test]
    fn retaining_drops_things_that_have_left_the_order_and_the_anchor_with_them() {
        let mut sel = many();
        sel.click(&ROWS, 22, SelectionGesture::Replace);
        sel.click(&ROWS, 55, SelectionGesture::Toggle);
        let shorter = [11u32, 22, 33];
        sel.retain(&shorter);
        assert_eq!(sel.chosen(&shorter), vec![22]);
        assert_eq!(sel.anchor(), None, "the thing it named is gone");
        assert_eq!(sel.cursor(), None);
    }

    #[test]
    fn the_chosen_come_back_in_display_order_however_they_were_pressed() {
        let mut sel = many();
        for id in [66u32, 11, 44] {
            sel.click(&ROWS, id, SelectionGesture::Toggle);
        }
        assert_eq!(sel.chosen(&ROWS), vec![11, 44, 66]);
        // In the order actually on show, which is not the same thing: a
        // list that sorts the other way reads its own selection back the
        // other way round.
        let reversed = [66u32, 55, 44, 33, 22, 11];
        assert_eq!(sel.chosen(&reversed), vec![66, 44, 11]);
    }

    #[test]
    fn a_press_on_something_not_in_the_order_changes_nothing_at_all() {
        let mut sel = many();
        sel.click(&ROWS, 33, SelectionGesture::Replace);
        let change = sel.click(&ROWS, 99, SelectionGesture::Replace);
        assert!(!change.any());
        assert_eq!(sel.chosen(&ROWS), vec![33]);
        assert_eq!(sel.anchor(), Some(33));
    }

    #[test]
    fn an_empty_order_moves_nothing_instead_of_reaching_past_its_end() {
        let mut sel = many();
        let empty: [u32; 0] = [];
        assert!(!sel.key_move(&empty, SelectionStep::Next, SelectionMove::Replace).any());
        assert!(!sel.key_move(&empty, SelectionStep::Last, SelectionMove::Extend).any());
        assert!(!sel.select_all(&empty).any());
        assert!(sel.is_empty());
    }

    #[test]
    fn a_restored_selection_puts_the_anchor_where_a_shift_press_can_use_it() {
        let mut sel = many();
        sel.set_chosen(&ROWS, &[22, 44, 99]);
        assert_eq!(sel.chosen(&ROWS), vec![22, 44], "the thing that is not on show was dropped");
        assert_eq!(sel.anchor(), Some(44), "the last of them in display order");
        sel.click(&ROWS, 66, SelectionGesture::Extend);
        assert_eq!(sel.chosen(&ROWS), vec![44, 55, 66], "a sweep replaces a restore rather than growing it");
    }

    #[test]
    fn a_restore_into_a_one_of_list_keeps_the_first_of_them_in_display_order() {
        let mut sel = ItemSelection::new(SelectionMode::One);
        sel.set_chosen(&ROWS, &[55, 22]);
        assert_eq!(sel.chosen(&ROWS), vec![22]);
    }

    #[test]
    fn the_change_report_tells_a_new_choice_apart_from_a_move_of_the_cursor() {
        let mut sel = many();
        let first = sel.click(&ROWS, 33, SelectionGesture::Replace);
        assert_eq!(first, SelectionChange { chosen: true, cursor: true });
        let again = sel.click(&ROWS, 33, SelectionGesture::Replace);
        assert!(!again.any(), "pressing the same thing twice is not news");
    }

    #[test]
    fn the_modifiers_read_the_same_whichever_key_the_platform_calls_primary() {
        assert_eq!(SelectionGesture::from_modifiers(no_keys()), SelectionGesture::Replace);
        assert_eq!(SelectionGesture::from_modifiers(shift()), SelectionGesture::Extend);
        assert_eq!(SelectionGesture::from_modifiers(primary()), SelectionGesture::Toggle);
        assert_eq!(
            SelectionGesture::from_modifiers(primary_and_shift()),
            SelectionGesture::ToggleExtend
        );
        assert_eq!(SelectionMove::from_modifiers(no_keys()), SelectionMove::Replace);
        assert_eq!(SelectionMove::from_modifiers(shift()), SelectionMove::Extend);
        assert_eq!(SelectionMove::from_modifiers(primary()), SelectionMove::Cursor);
        assert_eq!(SelectionMove::from_modifiers(primary_and_shift()), SelectionMove::Extend);
    }
}

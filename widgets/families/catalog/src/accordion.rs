//! Accordion — the panel that decides which of its sections have room.
//!
//! The policy in [`FoldPolicy`] is lifted from a working console panel
//! rather than invented, and the reasoning it carries is the reason to have
//! it in the library at all. Three rules, each of which was learned by
//! getting it wrong first:
//!
//! **The host decides HOW MANY may be open; the person decides WHICH.** A
//! panel narrows because the window did, not because anyone asked, so the
//! room is the host's to set and the choice of what fills it is not.
//!
//! **Folding a section leaves its room EMPTY.** Handing it to a section
//! nobody asked for sounds tidier and is worse: fold one block and the
//! panel answers by opening something else, which then sits there for good
//! while the sections the person actually uses trade places underneath it.
//! Folding is someone saying "not this one"; opening a different one is the
//! panel arguing back.
//!
//! **A section that is wanted but crowded out comes back on its own.**
//! Wanting is remembered separately from showing, so growing the window
//! restores what the room could not previously afford, in the order it was
//! asked for. That is also why the last open section cannot be folded: a
//! column of headings over dead space is not a state worth being able to
//! reach.
//!
//! The widget wraps the policy around a column of [`crate::fold_header::FoldHeader`]
//! children. It never draws a section itself; it only says which are open,
//! so any header the library has, or an app's own, can sit in one.
//!
//! `open_on_hover` lets a section open when the pointer RESTS on its
//! header, for a panel someone is browsing rather than working. It is off
//! by default and it waits: a panel that rearranges itself as the pointer
//! crosses it on the way somewhere else is hostile, so the dwell has to be
//! long enough that passing over a header is not an instruction.
//!
//! `hover_secs: 0.0` drops the wait, which is right for exactly one shape:
//! the panel is a list of things to look at, a pane beside it shows the one
//! being pointed at, and there is nothing else on that side to reach past.
//! Then the sweep IS the browsing and a delay only makes it feel stuck. Any
//! panel with something to press below it should keep the wait.

use crate::{
    animator::Animate,
    fold_button::FoldButtonAction,
    fold_header::FoldHeaderWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
};

/// Which sections of a panel are open, and which are only wanted.
///
/// `wanted` is what the person asked for; `showing` is what the room
/// allows. Keeping them apart is what lets a crowded-out section come back
/// by itself when the window grows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoldPolicy {
    /// What has been asked for, by section index.
    wanted: Vec<bool>,
    /// Most recently asked for first: decides which of the wanted sections
    /// the room can still afford.
    order: Vec<usize>,
    /// How many may be open at once. `None` means all of them, and then
    /// the folding marks have nothing to do.
    room: Option<usize>,
}

impl FoldPolicy {
    /// A panel of `count` sections with everything open and no limit.
    pub fn new(count: usize) -> Self {
        FoldPolicy {
            wanted: vec![true; count],
            order: (0..count).collect(),
            room: None,
        }
    }

    /// The number of sections.
    pub fn len(&self) -> usize {
        self.wanted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.wanted.is_empty()
    }

    /// Grow or shrink the panel to `count` sections, keeping what is
    /// already known about the ones that remain.
    pub fn resize(&mut self, count: usize) {
        if count == self.wanted.len() {
            return;
        }
        self.wanted.resize(count, true);
        self.order.retain(|i| *i < count);
        for i in 0..count {
            if !self.order.contains(&i) {
                self.order.push(i);
            }
        }
    }

    /// How many sections may be open. `None` lifts the limit.
    pub fn set_room(&mut self, room: Option<usize>) -> bool {
        let room = room.map(|r| r.max(1));
        let changed = self.room != room;
        self.room = room;
        changed
    }

    pub fn room(&self) -> Option<usize> {
        self.room
    }

    /// Whether the panel is short enough for the folding marks to mean
    /// anything.
    pub fn is_folding(&self) -> bool {
        self.room.is_some_and(|room| room < self.wanted.len())
    }

    /// Ask for a section to start closed, before anything is shown. Used
    /// for the section a short panel can most afford to lose.
    pub fn set_wanted(&mut self, index: usize, wanted: bool) {
        if let Some(slot) = self.wanted.get_mut(index) {
            *slot = wanted;
        }
    }

    fn rank(&self, index: usize) -> usize {
        self.order.iter().position(|i| *i == index).unwrap_or(0)
    }

    /// Whether a section shows its contents.
    ///
    /// With no limit everything does: the marks are not meaningful at that
    /// size, so there is no state behind them to honour.
    pub fn shows(&self, index: usize) -> bool {
        let Some(room) = self.room else {
            return true;
        };
        if !self.wanted.get(index).copied().unwrap_or(false) {
            return false;
        }
        // Wanted sections fill the room most-recently-asked-for first; the
        // rest wait for the panel to grow.
        let ahead = self
            .order
            .iter()
            .take(self.rank(index))
            .filter(|other| self.wanted[**other])
            .count();
        ahead < room
    }

    /// A mark was pressed: that section opens if it was folded and folds if
    /// it was open. Answers whether anything changed.
    pub fn press(&mut self, index: usize) -> bool {
        if index >= self.wanted.len() || self.room.is_none() {
            return false;
        }
        let before = self.showing();
        if self.shows(index) {
            // Never the last one: a column of headings over dead space is
            // not a state worth reaching.
            if before.len() > 1 {
                self.wanted[index] = false;
                self.demote(index);
            }
        } else {
            self.wanted[index] = true;
            self.promote(index);
        }
        self.showing() != before
    }

    fn promote(&mut self, index: usize) {
        let at = self.rank(index);
        self.order[..=at].rotate_right(1);
    }

    fn demote(&mut self, index: usize) {
        let at = self.rank(index);
        self.order[at..].rotate_left(1);
    }

    /// The sections on screen, in the panel's own order.
    pub fn showing(&self) -> Vec<usize> {
        (0..self.wanted.len()).filter(|i| self.shows(*i)).collect()
    }

    /// The section asked for most recently that is actually on screen.
    ///
    /// This is the one a pane beside the panel should be showing: when a
    /// person opens a section they are asking to look at it, and a preview
    /// that stayed on whichever section happens to sit highest would be
    /// answering a question nobody asked.
    pub fn newest(&self) -> Option<usize> {
        self.order.iter().copied().find(|i| self.shows(*i))
    }
}

/// What an accordion reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum AccordionAction {
    /// The set of open sections changed, by a press, by the pointer or by
    /// the room changing. Carries the section asked for most recently that
    /// is on screen, which is the one a pane beside the panel should show.
    Changed(usize),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AccordionBase = #(Accordion::register_widget(vm))
    /** A column of fold headers that agree on how many may be open. */
    mod.widgets.Accordion = set_type_default() do mod.widgets.AccordionBase{
        width: Fill
        height: Fit
        flow: Down
        /** how many sections may be open at once; 0 lifts the limit 0..8 step 1 */
        room: 0
        /** sections that start closed when the panel is short, by index */
        closed_first: []
        /** a section opens when the pointer rests on its header */
        open_on_hover: false
        /** how long the pointer must rest before it counts; 0 is at once 0..2 step 0.05 */
        hover_secs: 0.45
    }

    /** The panel that shows one section at a time. */
    mod.widgets.AccordionSingle = mod.widgets.Accordion{
        room: 1
    }

    /** The panel that opens whichever section the pointer rests on. */
    mod.widgets.AccordionHover = mod.widgets.AccordionSingle{
        open_on_hover: true
    }

    /** The panel that follows the pointer without a wait, for a list of
     * things to look at with a pane beside it showing the one pointed at. */
    mod.widgets.AccordionSweep = mod.widgets.AccordionHover{
        hover_secs: 0.0
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Accordion {
    #[deref]
    view: View,
    /// How many sections may be open at once; zero lifts the limit.
    #[live(0usize)]
    pub room: usize,
    /// Sections that start closed when the panel is short, by index.
    #[live]
    pub closed_first: Vec<ScriptValue>,
    /// A section opens when the pointer rests on its header.
    #[live]
    pub open_on_hover: bool,
    /// How long the pointer must rest before that counts; zero opens the
    /// section the moment the pointer arrives.
    #[live(0.45)]
    pub hover_secs: f64,
    #[rust]
    policy: FoldPolicy,
    /// Whether the policy has been matched to the children yet.
    #[rust]
    started: bool,
    /// The section the pointer is resting on, and the clock counting it
    /// down. Cleared the moment the pointer moves to another header, so a
    /// pass across the panel opens nothing.
    #[rust]
    dwelling: Option<usize>,
    #[rust]
    dwell_timer: Timer,
}

impl Accordion {
    /// The fold headers under this accordion, in layout order. Only direct
    /// children count.
    fn sections(&self) -> Vec<WidgetRef> {
        let mut found = Vec::new();
        self.view.children(&mut |_id, child| {
            if child.as_fold_header().borrow().is_some() {
                found.push(child);
            }
        });
        found
    }

    /// Match the policy to the children and the room, once the children
    /// exist. Answers whether anything about the panel changed.
    fn settle(&mut self, count: usize) -> bool {
        let mut changed = false;
        if self.policy.len() != count {
            self.policy.resize(count);
            changed = true;
        }
        if !self.started {
            self.started = true;
            // The sections a short panel can most afford to lose start
            // closed, so folding never has to summon one of them later.
            for value in self.closed_first.clone() {
                if let Some(index) = value.as_number() {
                    self.policy.set_wanted(index as usize, false);
                }
            }
            changed = true;
        }
        let room = if self.room == 0 { None } else { Some(self.room) };
        changed |= self.policy.set_room(room);
        changed
    }

    /// Tell every section whether it is open.
    fn apply(&mut self, cx: &mut Cx, animate: Animate) {
        for (index, section) in self.sections().iter().enumerate() {
            section
                .as_fold_header()
                .set_is_open(cx, self.policy.shows(index), animate);
        }
        // A fold header clamps its body on the draw AFTER it learns its own
        // height, so a state cut with nothing else going on would leave the
        // panel looking unfolded until something else asked for a frame.
        self.view.redraw(cx);
    }

    /// The pointer has settled on a section: open it.
    ///
    /// Only ever OPENS. A pointer resting on an open section's header must
    /// not fold it, or crossing the panel would close what it just opened.
    fn open_dwelt(&mut self, cx: &mut Cx, index: usize) {
        if !self.policy.shows(index) && self.policy.press(index) {
            self.apply(cx, Animate::Yes);
            self.report(cx);
        }
    }

    /// Say that the open set changed, and which section a pane beside the
    /// panel should now be showing.
    fn report(&self, cx: &mut Cx) {
        let newest = self.policy.newest().unwrap_or(0);
        cx.widget_action(self.widget_uid(), AccordionAction::Changed(newest));
    }

    /// The sections on screen, by index.
    pub fn showing(&self) -> Vec<usize> {
        self.policy.showing()
    }

    /// The section asked for most recently that is on screen.
    pub fn newest(&self) -> Option<usize> {
        self.policy.newest()
    }

    /// The policy, for a host that wants to read or drive it directly.
    pub fn policy(&self) -> &FoldPolicy {
        &self.policy
    }
}

impl Widget for Accordion {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let count = self.sections().len();
        if self.settle(count) {
            // Without animation on the way into a draw: this is the panel
            // arriving at its state, not moving between two of them.
            self.apply(cx.cx.cx, Animate::No);
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if self.open_on_hover {
            if self.dwell_timer.is_event(event).is_some() {
                self.dwell_timer = Timer::empty();
                if let Some(index) = self.dwelling {
                    self.open_dwelt(cx, index);
                }
            }
            if let Event::MouseMove(me) = event {
                // The whole HEADER STRIP, not the fold mark inside it. The
                // mark is a small square in a padded row, so a band its
                // height leaves most of the heading dead: the pointer has
                // to come to rest inside those few pixels for the dwell to
                // ever finish, which reads as hover not working at all.
                // Asking for the strip rather than the fold's own rect
                // matters just as much, since the fold's rect grows to
                // include an open body.
                let over = self.sections().iter().position(|section| {
                    let head = section.as_fold_header().header_rect(cx);
                    head.size.y > 0.0
                        && me.abs.y >= head.pos.y
                        && me.abs.y <= head.pos.y + head.size.y
                        && me.abs.x >= head.pos.x
                        && me.abs.x <= head.pos.x + head.size.x
                });
                if over != self.dwelling {
                    cx.stop_timer(self.dwell_timer);
                    self.dwell_timer = Timer::empty();
                    self.dwelling = over;
                    if let Some(index) = over {
                        if self.hover_secs <= 0.0 {
                            // No wait asked for: the arrival IS the
                            // instruction. `dwelling` still remembers the
                            // section, so the layout shifting under a
                            // pointer that has not moved cannot ask again.
                            self.open_dwelt(cx, index);
                        } else {
                            self.dwell_timer = cx.start_timeout(self.hover_secs);
                        }
                    }
                }
            }
        }

        let Event::Actions(actions) = event else {
            return;
        };
        // A fold header does not report its own toggle: it watches the
        // mark inside it and animates itself. The accordion watches the
        // same mark, so the panel and the header agree about what happened.
        //
        // ONLY Opening and Closing are a press. A mark also reports
        // `Animating` on every frame of every animation it runs — including
        // the ones this panel starts itself in `apply`, and the ones a mere
        // hover starts. Counting those as presses makes the panel argue with
        // itself: it folds a section, the fold animates, the animation reads
        // as another press, and the panel flips between two states for as
        // long as it is looked at.
        let sections = self.sections();
        let mut pressed = None;
        for (index, section) in sections.iter().enumerate() {
            let button = section.widget(cx, ids!(fold_button));
            if button.is_empty() {
                continue;
            }
            let Some(action) = actions.find_widget_action(button.widget_uid()) else {
                continue;
            };
            if matches!(
                action.cast::<FoldButtonAction>(),
                FoldButtonAction::Opening | FoldButtonAction::Closing
            ) {
                pressed = Some(index);
            }
        }
        let Some(index) = pressed else {
            return;
        };
        // The header has already toggled itself; the policy decides what
        // the panel does about it, and then every section is told.
        if self.policy.press(index) {
            self.apply(cx, Animate::Yes);
            self.report(cx);
        } else {
            // The press changed nothing, so put the header back where the
            // policy says it should be: the last open section stays open.
            self.apply(cx, Animate::Yes);
        }
    }

    /// The sections on screen, as their indices, so a test can read the
    /// panel's state in one line.
    fn text(&self) -> String {
        self.showing()
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl AccordionRef {
    /// The sections on screen, by index.
    pub fn showing(&self) -> Vec<usize> {
        self.borrow().map(|inner| inner.showing()).unwrap_or_default()
    }

    /// The section a pane beside the panel should show, if the open set
    /// changed this pass.
    pub fn changed(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<AccordionAction>() {
            AccordionAction::Changed(index) => Some(index),
            _ => None,
        }
    }

    /// The section asked for most recently that is on screen.
    pub fn newest(&self) -> Option<usize> {
        self.borrow().and_then(|inner| inner.newest())
    }

    /// How many sections may be open at once; zero lifts the limit.
    pub fn set_room(&self, cx: &mut Cx, room: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.room = room;
            let count = inner.sections().len();
            if inner.settle(count) {
                inner.apply(cx, Animate::Yes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no limit every section shows, and the folding marks have
    /// nothing to do: a stray press must not quietly rearrange a panel
    /// whose marks are not even meaningful at that size.
    #[test]
    fn an_unlimited_panel_shows_everything_and_ignores_its_marks() {
        let mut p = FoldPolicy::new(3);
        assert!(!p.is_folding());
        assert_eq!(p.showing(), vec![0, 1, 2]);
        assert!(!p.press(2));
        assert_eq!(p.showing(), vec![0, 1, 2]);
    }

    /// The rule this policy exists for: folding a section leaves its room
    /// empty rather than handing it to one nobody asked for.
    #[test]
    fn folding_never_summons_the_section_nobody_asked_for() {
        let mut p = FoldPolicy::new(3);
        p.set_wanted(2, false);
        p.set_room(Some(2));
        assert_eq!(p.showing(), vec![0, 1]);

        p.press(0);
        assert_eq!(p.showing(), vec![1], "the room stays empty, not filled");

        // And it stays gone however long the other two are worked.
        p.press(0);
        p.press(1);
        p.press(0);
        assert!(
            !p.showing().contains(&2),
            "a section opens when it is asked for, never as a side effect"
        );
    }

    /// A mark toggles its own section, both ways, and a section that was
    /// only crowded out comes back by itself.
    #[test]
    fn a_mark_toggles_its_own_section() {
        let mut p = FoldPolicy::new(3);
        p.set_wanted(2, false);
        p.set_room(Some(2));
        p.press(2);
        assert_eq!(p.showing(), vec![0, 2], "asking for it costs the older one");
        p.press(2);
        assert_eq!(p.showing(), vec![0, 1], "and the crowded-out one returns");
    }

    /// The last open section cannot be folded: a column of headings over
    /// dead space is not a state worth reaching.
    #[test]
    fn the_last_open_section_stays_open() {
        let mut p = FoldPolicy::new(3);
        p.set_room(Some(1));
        assert_eq!(p.showing().len(), 1);
        let only = p.showing()[0];
        p.press(only);
        assert_eq!(p.showing(), vec![only], "the panel still shows something");
    }

    /// Wanting is remembered apart from showing, so growing the panel
    /// restores what the room could not afford, in the order it was asked
    /// for.
    #[test]
    fn growing_the_panel_brings_back_what_was_wanted() {
        let mut p = FoldPolicy::new(3);
        p.set_room(Some(1));
        p.press(2);
        assert_eq!(p.showing(), vec![2]);
        p.set_room(Some(3));
        assert_eq!(p.showing(), vec![0, 1, 2], "everything wanted is back");
        p.set_room(Some(2));
        // Asking for section 2 put it at the head of the order, so a room
        // for two takes it and the next most recently asked for, which is
        // section 0 — not the one that merely sits next to it.
        assert_eq!(p.showing(), vec![0, 2], "the room takes the newest first");
    }

    /// The newest showing section is what a pane beside the panel follows,
    /// so it must be the one most recently ASKED for, not the one that
    /// happens to sit highest.
    #[test]
    fn the_newest_section_is_the_one_last_asked_for() {
        let mut p = FoldPolicy::new(3);
        p.set_room(Some(2));
        assert_eq!(p.newest(), Some(0), "nothing asked for yet: the first one");
        p.press(2);
        assert_eq!(p.newest(), Some(2), "asking for a section makes it the newest");
        p.press(1);
        assert_eq!(p.newest(), Some(1));
        // Folding the newest hands the pane to whatever is still showing,
        // never to a section the room does not afford.
        p.press(1);
        assert_eq!(p.newest(), Some(2));
        assert!(p.showing().contains(&2));
    }

    /// A panel that gains or loses sections keeps what it knew about the
    /// ones that remain.
    #[test]
    fn resizing_keeps_what_it_knew() {
        let mut p = FoldPolicy::new(3);
        p.set_room(Some(2));
        p.press(2);
        let before = p.showing();
        p.resize(4);
        assert_eq!(p.len(), 4);
        assert_eq!(p.showing(), before, "the new section is wanted but has no room");
        p.resize(2);
        assert_eq!(p.len(), 2);
        assert!(p.showing().iter().all(|i| *i < 2));
    }
}

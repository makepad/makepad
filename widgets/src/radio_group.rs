//! RadioGroup — one question, a handful of RadioButton children, and exactly
//! one answer shared between them.
//!
//! A radio button on its own knows whether it is lit and nothing else. Making
//! a set of them exclusive is left to whoever puts them on a page, and every
//! one of them does it the same way by hand: collect the buttons by id, ask
//! each whether it was clicked, and put the others out. That code is written
//! out five times on one catalogue page alone, and every copy has the same
//! two holes — the set is five tab stops instead of one, and the arrow keys
//! do nothing at all, so somebody working by keyboard has to Tab through
//! every answer and can still never change the one that is taken.
//!
//! This group closes both. It finds its RadioButton children at draw time,
//! wherever they sit in its subtree, and from then on there is one answer:
//! one row lit, one tab stop for the whole set, and arrows that move the
//! ANSWER rather than only the focus. That last part is the point of the
//! widget. Moving the focus alone and waiting for a second key press to take
//! the answer is how a list of rows behaves; a group of answers chooses as
//! the arrows go, and somebody who has to press twice will read the first
//! press as nothing having happened.
//!
//! # Answers that cannot be taken
//!
//! A row may be disabled on its own, and the arrows step straight past it: a
//! stop on an answer that cannot be taken is a dead end with nothing on
//! screen to explain it. Home and End go to the first and last answer that
//! CAN be taken, for the same reason. When every answer is disabled the group
//! holds no answer at all rather than lighting one that no press can reach.
//!
//! # What it deliberately does not do
//!
//! It does not own its answers as data. A control that holds a list of words
//! and draws them itself is a segmented control and it lives elsewhere; the
//! rows here are real widgets, so any radio preset in the library — and any
//! preset a caller invents, with its own icon, label walk and shader — can be
//! an answer.
//!
//! It does not build its rows from a template either. The answers are written
//! in the markup, where a reader can see what the question offers.
//!
//! And it never lets go: pressing the answer that is already taken changes
//! nothing and reports nothing. A set of answers where the current one can be
//! un-taken by pressing it again is a set of checkboxes.
use crate::{
    animator::Animate,
    makepad_derive_widget::*,
    makepad_draw::*,
    radio_button::RadioButtonWidgetRefExt,
    view::View,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawRadioRingBase = #(DrawRadioRing::script_component(vm))
    set_type_default() do #(DrawRadioRing::script_shader(vm)){
        ..mod.draw.DrawQuad

        /** ring thickness in pixels 0..8 step 0.5 */
        border_size: theme.size_focus_ring
        /** corner rounding radius 0..24 step 0.5 */
        border_radius: theme.corner_radius
        border_color: theme.color_val_focus

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            // Inset by half the stroke. A stroke sits centred on the edge it
            // is given, so a box drawn on the quad's own bounds spills half
            // its width outside them and comes back with that half cut off.
            let w = self.border_size
            sdf.box(w * 0.5, w * 0.5, self.rect_size.x - w, self.rect_size.y - w, self.border_radius)
            sdf.stroke(self.border_color, w)
            return sdf.result
        }
    }

    mod.widgets.RadioGroupBase = #(RadioGroup::register_widget(vm))

    /** One question, a set of RadioButton answers, and exactly one of them
     * taken. The whole group is a single tab stop, and inside it the arrows
     * move the answer. */
    mod.widgets.RadioGroup = set_type_default() do mod.widgets.RadioGroupBase{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_1
        padding: theme.mspace_1

        /** which answer is taken, as an index among the radio children 0..16 step 1 */
        selected: 0
        /** the arrows carry on round from either end instead of stopping */
        wrap: true
        /** room kept above the answers for the shared label 0..48 step 1 */
        label_height: 18.
        /** the whole question is unavailable */
        disabled: false
        /** the label's ink while the question is unavailable 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity

        draw_text +: {
            color: theme.color_label_outer
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    /** The answers laid across the page instead of down it. */
    mod.widgets.RadioGroupRow = mod.widgets.RadioGroup{
        flow: Right
        spacing: theme.space_2
    }
}

/// The ring drawn round the group while it holds the keyboard.
///
/// It is drawn round the GROUP and not round the answer the arrows are on,
/// because the group is what has the focus: one tab stop, one ring. The lit
/// row says which answer is taken; the ring says the keys will move it.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRadioRing {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    border_size: f32,
    #[live]
    border_radius: f32,
    #[live]
    border_color: Vec4f,
}

/// The first answer that can be taken.
///
/// `enabled` is one flag per answer in layout order: true where the answer
/// can be taken. These three functions are the whole of the group's index
/// arithmetic and they are kept out here, away from `Cx` and the script heap,
/// because stepping past a disabled answer, running off an end and having
/// nothing left to take are rules that are easy to get subtly wrong and
/// impossible to see once they are buried in a draw pass.
pub fn first_choice(enabled: &[bool]) -> Option<usize> {
    enabled.iter().position(|on| *on)
}

/// The last answer that can be taken.
pub fn last_choice(enabled: &[bool]) -> Option<usize> {
    enabled.iter().rposition(|on| *on)
}

/// Where an arrow key lands, moving `by` one place from `from`.
///
/// Answers `None` when there is nowhere else to go: an end reached without
/// wrapping, or nothing else in the set that can be taken. It never answers
/// `from` itself, so a caller can treat `Some` as "the answer moved".
pub fn step_choice(enabled: &[bool], from: usize, by: isize, wrap: bool) -> Option<usize> {
    let len = enabled.len();
    if len == 0 || by == 0 {
        return None;
    }
    // An index past the end is what the group holds for one pass after a row
    // is taken away. The keys still have to work, so the step starts from the
    // near end instead of from nowhere.
    if from >= len {
        return if by > 0 { first_choice(enabled) } else { last_choice(enabled) };
    }
    let dir = by.signum();
    let mut at = from as isize;
    // At most one lap. A set with nothing else takeable in it must answer
    // None rather than spin looking for one.
    for _ in 0..len {
        at += dir;
        if at < 0 || at >= len as isize {
            if !wrap {
                return None;
            }
            at = at.rem_euclid(len as isize);
        }
        let index = at as usize;
        if index == from {
            return None;
        }
        if enabled[index] {
            return Some(index);
        }
    }
    None
}

/// The answer a group holding `want` should actually be holding.
///
/// `want` is what the markup or the host asked for; it may be past the end,
/// or it may name a row that has since been disabled. Searching forward first
/// keeps a group whose leading answers are disabled on the first one a person
/// can really take, and the backward pass is what saves a group whose LAST
/// answers were the disabled ones. `None` when nothing at all can be taken.
pub fn settle_choice(enabled: &[bool], want: usize) -> Option<usize> {
    if enabled.get(want).copied().unwrap_or(false) {
        return Some(want);
    }
    let cut = want.min(enabled.len());
    (cut..enabled.len())
        .find(|i| enabled[*i])
        .or_else(|| (0..cut).rev().find(|i| enabled[*i]))
}

/// What a group reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum RadioGroupAction {
    /// The answer moved, carrying its index among the radio children.
    Selected(usize),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct RadioGroup {
    #[deref]
    view: View,

    #[live]
    draw_focus: DrawRadioRing,
    #[live]
    draw_text: DrawText,

    /// The question the answers are answering, drawn above them. Empty for a
    /// group whose host has already asked it in its own words.
    #[live]
    pub text: String,
    /// The room that label is given.
    #[live(18.0)]
    pub label_height: f64,

    /// Which answer the markup asks for, as an index among the radio
    /// children.
    #[live]
    pub selected: usize,

    /// The arrows carry on round from either end. On, because a set of
    /// answers is a ring rather than a line: there is no "past the last
    /// answer" for the keys to fall off.
    #[live(true)]
    pub wrap: bool,

    /// The whole question is unavailable.
    #[live]
    pub disabled: bool,
    #[live(0.38)]
    pub disabled_opacity: f32,

    /// The radio children, found afresh every draw.
    #[rust]
    items: Vec<WidgetRef>,
    /// The answer the group is holding. Kept apart from `selected` so the
    /// group may settle onto an answer that can actually be taken without
    /// overwriting what the markup asked for.
    #[rust]
    chosen: usize,
    /// Whether any answer at all can be taken, so a group of nothing but
    /// disabled rows lights none of them.
    #[rust]
    has_answer: bool,
    /// The last `selected` the group acted on. Without it there is no way to
    /// tell a host WRITING the property from the group's own press having
    /// written it a moment ago, and one of the two would keep undoing the
    /// other.
    #[rust]
    applied: Option<usize>,

    /// The top padding as the layout owns it, and the value this widget last
    /// wrote there. See the note in `draw_walk`.
    #[rust]
    base_top: f64,
    #[rust]
    wrote: Option<f64>,

    /// Whether the group's own `disabled` has been pushed into the rows, and
    /// what each row said for itself before it was.
    #[rust]
    pushed_disabled: bool,
    #[rust]
    own_disabled: Vec<bool>,
}

impl RadioGroup {
    /// Every RadioButton under the group, in layout order.
    ///
    /// Walked fresh each draw rather than cached: children can be replaced by
    /// a live edit, swapped by a view that chooses its own contents, or
    /// rebuilt by a host, and a cached list would hand the keyboard indices
    /// into rows that are no longer on the page.
    fn collect(&mut self) {
        let mut level = Vec::new();
        self.view.children(&mut |_id, child| level.push(child));
        let mut items = Vec::new();
        gather(level, &mut items, 4);
        self.items = items;
    }

    /// One flag per answer: true where it can be taken.
    fn enabled_flags(&self, cx: &Cx) -> Vec<bool> {
        if self.disabled {
            // While the group is holding every row down, what each row said
            // for ITSELF is what decides the answer. Reading the rows here
            // would find them all disabled and leave a disabled group showing
            // no answer at all, rather than showing a greyed one.
            return (0..self.items.len())
                .map(|i| !self.own_disabled.get(i).copied().unwrap_or(false))
                .collect();
        }
        self.items.iter().map(|item| !item.disabled(cx)).collect()
    }

    /// The group's own `disabled` reaches every answer, and lets go again
    /// without dragging the answers that were disabled on their own back up
    /// with it — hence the snapshot. Done at draw rather than in
    /// `set_disabled`, because a `disabled: true` written in the markup sets
    /// the field directly and never calls it, and because the rows do not
    /// exist to be reached until the first draw has found them.
    fn push_disabled(&mut self, cx: &mut Cx) {
        if self.pushed_disabled == self.disabled {
            return;
        }
        self.pushed_disabled = self.disabled;
        if self.disabled {
            self.own_disabled = self.items.iter().map(|item| item.disabled(cx)).collect();
            for item in &self.items {
                item.set_disabled(cx, true);
            }
        } else {
            for (i, item) in self.items.iter().enumerate() {
                item.set_disabled(cx, self.own_disabled.get(i).copied().unwrap_or(false));
            }
            self.own_disabled.clear();
        }
    }

    /// Take the answer at `index`, and say so.
    fn choose(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.items.len() || self.chosen == index {
            return;
        }
        self.chosen = index;
        // The property and the answer are kept in step both ways, so a host
        // that reads `selected` after a press sees the answer, and one that
        // writes it moves the group.
        self.selected = index;
        self.applied = Some(index);
        cx.widget_action(self.widget_uid(), RadioGroupAction::Selected(index));
        self.redraw(cx);
    }

    /// The answer the group is holding, or nothing when no answer can be
    /// taken.
    pub fn answer(&self) -> Option<usize> {
        self.has_answer.then_some(self.chosen)
    }

    /// The label of the answer being held.
    pub fn answer_text(&self) -> String {
        self.answer()
            .and_then(|index| self.items.get(index))
            .map(|item| item.text())
            .unwrap_or_default()
    }

    /// Move the answer without reporting it: a host that sets the answer
    /// already knows what it set.
    pub fn set_selected(&mut self, cx: &mut Cx, index: usize) {
        self.selected = index;
        self.chosen = index;
        self.applied = Some(index);
        self.redraw(cx);
    }
}

/// Collect the radio buttons out of a level of the tree, descending through
/// anything that is not one.
///
/// The descent is what lets a group hold its answers inside rows, cards or
/// wrappers instead of demanding they be its immediate children. It stops at
/// a nested group, because that group keeps its own answer and reaching into
/// it would let the outer one put its rows out.
fn gather(level: Vec<WidgetRef>, out: &mut Vec<WidgetRef>, depth: usize) {
    for child in level {
        if child.as_radio_button().borrow().is_some() {
            out.push(child);
            continue;
        }
        if depth == 0 || child.as_radio_group().borrow().is_some() {
            continue;
        }
        let mut next = Vec::new();
        child.children(&mut |_id, grand| next.push(grand));
        gather(next, out, depth - 1);
    }
}

impl Widget for RadioGroup {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.collect();
        self.push_disabled(cx.cx.cx);

        let enabled = self.enabled_flags(cx.cx.cx);
        // A host writing the property moves the answer; the group's own press
        // has already written both, so it does not come back through here.
        if self.applied != Some(self.selected) {
            self.applied = Some(self.selected);
            self.chosen = self.selected;
        }
        if let Some(settled) = settle_choice(&enabled, self.chosen) {
            self.chosen = settled;
        }
        self.has_answer = enabled.iter().any(|on| *on);

        let chosen = self.chosen;
        let has_answer = self.has_answer;
        for (i, item) in self.items.iter().enumerate() {
            let radio = item.as_radio_button();
            // The rows are taken out of the tab order here rather than in a
            // template: a caller writing its own answers cannot be expected
            // to remember a flag, and one that forgets puts a stop back on
            // every row.
            radio.set_nav_stop(false);
            // Exactly one is lit, said once here. set_active and not select:
            // `select` raises Clicked whenever it turns a row on, and this
            // runs every draw, so the first draw would manufacture a press
            // nobody made. Animate::No for the same reason — this is a
            // statement of what is true, repeated every frame, not a change.
            radio.set_active(cx.cx.cx, has_answer && i == chosen, Animate::No);
        }

        // The shared label needs room that the view's own layout knows
        // nothing about, so it is added to the top padding. The base is
        // re-read whenever something else has written that padding — a
        // re-applied markup value, an edit from the tweaker — and otherwise
        // kept, so the room is never added on top of itself draw after draw.
        let room = if self.text.is_empty() { 0.0 } else { self.label_height };
        if self.wrote != Some(self.view.layout.padding.top) {
            self.base_top = self.view.layout.padding.top;
        }
        let top = self.base_top + room;
        self.view.layout.padding.top = top;
        self.wrote = Some(top);

        let step = self.view.draw_walk(cx, scope, walk);
        if !step.is_done() {
            return step;
        }

        let area = self.view.area();
        let rect = area.rect(cx);
        // Only drawn while the group holds the keyboard, so an unfocused
        // group costs no draw call at all rather than a transparent one.
        if cx.has_key_focus(area) {
            self.draw_focus.draw_abs(cx, rect);
        }

        if !self.text.is_empty() {
            let text = self.text.clone();
            let size = self.draw_text.text_style.font_size as f64;
            let rest = self.draw_text.color;
            if self.disabled {
                self.draw_text.color = Vec4f { w: rest.w * self.disabled_opacity, ..rest };
            }
            // draw_abs takes the top of the LINE box and not of the ink. A
            // glyph's ink starts about a third of the font size below it, so
            // centring the line box in the band leaves the label riding high.
            let y = rect.pos.y + (self.label_height - size) * 0.5 - size * 0.30;
            let x = rect.pos.x + self.view.layout.padding.left;
            self.draw_text.draw_abs(cx, dvec2(x, y), &text);
            self.draw_text.color = rest;
        }

        // ONE stop for the whole group: Tab reaches it, the arrows move the
        // answer inside it, and Tab again leaves it — rather than walking
        // every answer on the way past.
        if !self.disabled {
            cx.add_nav_stop(area, NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::LiveEdit = event {
            // The children may have been replaced outright, so nothing
            // remembered about the old ones is worth keeping.
            self.items.clear();
            self.own_disabled.clear();
            self.pushed_disabled = false;
        }
        self.view.handle_event(cx, event, scope);

        let area = self.view.area();
        // A press anywhere in the group leaves the keyboard with the GROUP
        // and not with the row that was pressed. Taken after the rows have
        // had the press, because a row claims the focus itself on the way
        // down; and taken whether or not the answer moved, because pressing
        // the answer that is already taken raises nothing at all and the
        // focus would be stranded on a row the arrows do not reach.
        if let Event::MouseDown(me) = event {
            if area.rect(cx).contains(me.abs) {
                cx.set_key_focus(area);
            }
        }
        if self.disabled {
            return;
        }

        if let Event::Actions(actions) = event {
            let items = self.items.clone();
            for (index, item) in items.iter().enumerate() {
                if item.as_radio_button().clicked(actions) {
                    self.choose(cx, index);
                }
            }
        }

        match event.hits(cx, area) {
            // The MouseDown arm above covers a pointer; this covers a finger,
            // which sends no mouse press at all.
            Hit::FingerDown(_) => {
                cx.set_key_focus(area);
            }
            // The ring is drawn from the focus, so the focus changing is a
            // repaint.
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let enabled = self.enabled_flags(cx);
                let to = match ke.key_code {
                    // Both arrow pairs work whichever way the group runs. A
                    // group laid across the page is still a column to
                    // somebody who reaches for Down first, and a key that
                    // does nothing reads as a broken control.
                    KeyCode::ArrowRight | KeyCode::ArrowDown => {
                        step_choice(&enabled, self.chosen, 1, self.wrap)
                    }
                    KeyCode::ArrowLeft | KeyCode::ArrowUp => {
                        step_choice(&enabled, self.chosen, -1, self.wrap)
                    }
                    // The ends that can be TAKEN, not the ends of the set.
                    KeyCode::Home => first_choice(&enabled),
                    KeyCode::End => last_choice(&enabled),
                    _ => None,
                };
                if let Some(index) = to {
                    self.choose(cx, index);
                }
            }
            _ => {}
        }
    }

    /// The label of the answer, so a host or a test reads the group in one
    /// line.
    fn text(&self) -> String {
        self.answer_text()
    }

    /// A word takes the answer with that label; a number takes it by index.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some(index) = self.items.iter().position(|item| item.text() == v) {
            self.choose(cx, index);
        } else if let Ok(index) = v.trim().parse::<usize>() {
            self.choose(cx, index);
        }
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    /// The index of the answer, so a test can wait on the number rather than
    /// on the words.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.chosen.to_string())
    }

    fn snapshot_selected(&self, _cx: &Cx) -> Option<String> {
        Some(self.answer_text())
    }
}

impl RadioGroupRef {
    /// The answer chosen this pass, if it moved.
    pub fn selected(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<RadioGroupAction>() {
            RadioGroupAction::Selected(index) => Some(index),
            _ => None,
        }
    }

    /// The answer the group is holding, or nothing when no answer can be
    /// taken.
    pub fn answer(&self) -> Option<usize> {
        self.borrow().and_then(|inner| inner.answer())
    }

    /// The label of the answer being held.
    pub fn answer_text(&self) -> String {
        self.borrow().map(|inner| inner.answer_text()).unwrap_or_default()
    }

    pub fn set_selected(&self, cx: &mut Cx, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, index);
        }
    }

    /// How many answers the group found. Zero until it has drawn once.
    pub fn count(&self) -> usize {
        self.borrow().map(|inner| inner.items.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arrow_steps_past_an_answer_that_cannot_be_taken() {
        let enabled = [true, false, false, true];
        assert_eq!(step_choice(&enabled, 0, 1, false), Some(3), "over both disabled rows");
        assert_eq!(step_choice(&enabled, 3, -1, false), Some(0), "and back over them");
    }

    #[test]
    fn a_group_that_wraps_carries_on_round_and_one_that_does_not_stops() {
        let enabled = [true, true, true];
        assert_eq!(step_choice(&enabled, 2, 1, true), Some(0));
        assert_eq!(step_choice(&enabled, 0, -1, true), Some(2));
        assert_eq!(step_choice(&enabled, 2, 1, false), None, "the end is the end");
        assert_eq!(step_choice(&enabled, 0, -1, false), None);
    }

    #[test]
    fn wrapping_still_steps_past_what_cannot_be_taken() {
        let enabled = [true, false, true];
        assert_eq!(step_choice(&enabled, 2, 1, true), Some(0), "round the end");
        assert_eq!(step_choice(&enabled, 0, 1, true), Some(2), "past the gap");
    }

    #[test]
    fn a_group_with_nothing_to_take_has_no_answer_and_the_keys_do_nothing() {
        let enabled = [false, false, false];
        assert_eq!(first_choice(&enabled), None);
        assert_eq!(last_choice(&enabled), None);
        assert_eq!(step_choice(&enabled, 0, 1, true), None);
        assert_eq!(step_choice(&enabled, 0, -1, true), None);
        assert_eq!(step_choice(&enabled, 0, 1, false), None);
        assert_eq!(settle_choice(&enabled, 0), None, "and nothing to settle onto");
    }

    #[test]
    fn an_empty_group_answers_nothing_rather_than_reaching_past_its_ends() {
        let enabled: [bool; 0] = [];
        assert_eq!(step_choice(&enabled, 0, 1, true), None);
        assert_eq!(first_choice(&enabled), None);
        assert_eq!(last_choice(&enabled), None);
        assert_eq!(settle_choice(&enabled, 3), None);
    }

    #[test]
    fn home_and_end_reach_the_ends_that_can_be_taken() {
        // Not the ends of the set: an end that cannot be pressed is a place
        // the keyboard must not be able to leave the answer.
        let enabled = [false, true, true, false];
        assert_eq!(first_choice(&enabled), Some(1));
        assert_eq!(last_choice(&enabled), Some(2));
    }

    #[test]
    fn one_answer_alone_has_nowhere_to_step() {
        let enabled = [true];
        assert_eq!(step_choice(&enabled, 0, 1, true), None);
        assert_eq!(step_choice(&enabled, 0, -1, true), None);
        // And a set where only one of several can be taken is the same set.
        let enabled = [false, true, false];
        assert_eq!(step_choice(&enabled, 1, 1, true), None);
        assert_eq!(step_choice(&enabled, 1, -1, true), None);
    }

    #[test]
    fn a_step_never_answers_where_it_started() {
        // The caller reads Some as "the answer moved", so a lap that comes
        // back round to the same row has to answer None.
        let enabled = [true, true];
        assert_eq!(step_choice(&enabled, 0, 1, true), Some(1));
        assert_eq!(step_choice(&enabled, 0, 0, true), None, "no direction, no move");
    }

    #[test]
    fn an_answer_that_cannot_be_taken_settles_onto_one_that_can() {
        let enabled = [false, false, true, true];
        assert_eq!(settle_choice(&enabled, 2), Some(2), "already good, left alone");
        assert_eq!(settle_choice(&enabled, 0), Some(2), "forward to the first that can");

        // Nothing forward: it falls back, rather than leaving the group
        // lighting a row that no press can reach.
        let enabled = [true, false, false];
        assert_eq!(settle_choice(&enabled, 2), Some(0));
        // An index past the end is what `selected: 9` in the markup gives.
        assert_eq!(settle_choice(&enabled, 9), Some(0));
    }

    #[test]
    fn a_stale_answer_does_not_stop_the_arrows_working() {
        // An index past the end is what the group holds for one pass after a
        // row is taken away. The arrows still have to reach an answer.
        let enabled = [true, false, true];
        assert_eq!(step_choice(&enabled, 9, 1, false), Some(0));
        assert_eq!(step_choice(&enabled, 9, -1, false), Some(2));
    }
}

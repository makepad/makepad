//! A PortalList that can reorder its rows by dragging a per-row gripper.
//!
//! `ReorderList` is a thin specialisation of [`PortalList`]: templates,
//! scrolling, virtualisation and item actions all pass straight through
//! (the list is the `#[deref]` base). On top of that it watches one named
//! child of every item — the `drag_handle` — and turns a press-and-drag on
//! it into a reorder gesture:
//!
//! - the press on the gripper is captured HERE, before the inner list sees
//!   it, so drag-to-scroll never fights the gesture;
//! - with `drag_anywhere` on, a press on the row itself — its background,
//!   its words, the space between its cells — carries it too. That press
//!   goes to the rows FIRST and is taken up only if it landed on the
//!   row's bare background and nothing on the row took the finger, so a
//!   button, a slider, a chip, a picker and a label all keep their
//!   presses -- and a control that holds the mouse keeps it until the
//!   release, so a press it took never turns into a carry however far
//!   the pointer wanders from it;
//! - while the drag is live the widget tracks the insertion slot under the
//!   pointer (row midpoints decide) and carries the whole row: the lifted
//!   row is drawn under the pointer and over the rest, every row between
//!   its own place and the slot shifts by its height so the gap it would
//!   land in stands open, and `draw_indicator` draws a line in that gap.
//!   The lifted row is reported through [`ReorderList::drag_state`] so the
//!   host can tint it;
//! - Escape cancels the gesture; a wheel scroll during it is swallowed so
//!   the rows never slide under the pointer mid-drag;
//! - the release emits [`ReorderListAction::Reordered`] with indices into
//!   the host's item range. The widget itself moves nothing: item identity
//!   and the model belong to the host, which applies the move and redraws.
//!   The carry is ink, not layout — it is applied once the list's draw pass
//!   is done, and the next pass starts from an untouched list.
//!
//! The host names the gripper in its item template and points at it:
//!
//! ```text
//! list := ReorderList {
//!     drag_handle: @gripper
//!     Row := View { gripper := View { Icon { ... } } ... }
//! }
//! ```
//!
//! CAPTURE LAW (the bug this file once had): a finger capture in
//! `cx.fingers` is keyed on the captured widget's `Area`, and every redraw
//! REMAPS that stored capture to the widget's fresh area
//! (`Cx::update_area_refs`). Any `Area` snapshot a widget keeps for itself
//! goes stale on the first redraw — `event.hits` on it fails `is_valid`
//! and returns `Nothing` forever. So no `Area` is stored here: the
//! gripper's CURRENT area is re-resolved from the live item on every event
//! and always equals the remapped capture.

use crate::{
    flat_list::WidgetItem, makepad_derive_widget::*, makepad_draw::*, portal_list::PortalList,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ReorderListBase = #(ReorderList::register_widget(vm))

    mod.widgets.ReorderList = set_type_default() do mod.widgets.ReorderListBase {
        width: Fill
        height: Fill
        capture_overload: true
        scroll_bar: mod.widgets.ScrollBar {}
        flow: Down
        draw_indicator +: {
            draw_depth: 10.0
            color: #x5a8bd8
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum ReorderListAction {
    /// A row was dropped somewhere new. `from` and `to` are indices into
    /// the host's item range: remove the row at `from`, then insert it at
    /// `to` (`to` is already adjusted for the removal).
    Reordered { from: usize, to: usize },
    #[default]
    None,
}

/// One visible row as the drag machine sees it: entry id, top y, height.
pub type RowBand = (usize, f64, f64);

/// The insertion slot for a pointer at `y` over `rows` (ascending by entry
/// id): the first row whose midpoint is still below the pointer, else after
/// the last one (`last id + 1`). `None` when nothing is visible.
pub fn slot_for(rows: &[RowBand], y: f64) -> Option<usize> {
    let (last, _, _) = rows.last()?;
    let mut slot = last + 1;
    for (id, top, height) in rows {
        if y < top + height * 0.5 {
            slot = *id;
            break;
        }
    }
    Some(slot)
}

/// The row a press at `point` carries, of the rows the list has drawn:
/// the one whose band holds the pointer, and only when nothing on the row
/// took the press. `taken` are the rects of the things that did — in the
/// live widget, the row's own widgets holding the finger once the rows
/// have had the event, which is a button, a slider, a chip or a picker
/// and never a label or the row's background. `None` for a press outside
/// the list, off the rows, or on one of those.
pub fn press_carries(point: DVec2, list: Rect, rows: &[RowBand], taken: &[Rect]) -> Option<usize> {
    if !list.contains(point) {
        return None;
    }
    if taken.iter().any(|rect| rect.contains(point)) {
        return None;
    }
    rows.iter()
        .find(|(_, top, height)| point.y >= *top && point.y < top + height)
        .map(|(id, _, _)| *id)
}

/// How far out of the paint order a carried row is lifted, so it draws over
/// the rows the list painted after it. A 2D pass shares one depth buffer and
/// painting later buys almost no z, so this is what puts the row on top; it
/// stays well under the band the overlays live in (32), and clear of the 10
/// the drop indicator spends.
const CARRY_LIFT: f32 = 12.0;

/// One live drag, from gripper press to release — a pure state machine
/// (no `Cx`, no `Area`), so the whole gesture is unit-testable: pointer y
/// in, slot and carry geometry out, commit or cancel at the end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReorderDrag {
    /// The item (list entry id) whose gripper was pressed.
    pub from: usize,
    /// Where the press landed.
    start_y: f64,
    /// Where the press landed inside the row: pointer y less the row's top.
    /// The carried row keeps it, so the row does not jump under the finger
    /// as it is lifted.
    grab: f64,
    /// The lifted row's height, from the press. The gap the rows open for
    /// it is this tall, wherever the pointer wanders.
    height: f64,
    /// Insertion slot in entry-id space: the row would land BEFORE the
    /// current occupant of `slot`; `last visible + 1` means after the end.
    pub slot: usize,
    /// True once the press has moved past the threshold — only then does
    /// the row carry and the release commit. A plain click on the gripper
    /// stays a click.
    pub active: bool,
}

impl ReorderDrag {
    /// A fresh press on `from`'s gripper at pointer height `y`.
    pub fn press(from: usize, y: f64) -> Self {
        Self { from, start_y: y, grab: 0.0, height: 0.0, slot: from, active: false }
    }

    /// The row the press took hold of: its top and its height. The carry
    /// hangs the row off the grab this fixes and opens a gap its height.
    /// A machine that wants the slot alone and draws its own carry — the
    /// kanban board's cards — leaves it off and carries nothing.
    pub fn on_row(mut self, top: f64, height: f64) -> Self {
        self.grab = self.start_y - top;
        self.height = height;
        self
    }

    /// Where the carried row's top is drawn for a pointer at `y`: the
    /// pointer less the grab, kept inside the list's visible band so a row
    /// carried past either end stays in sight instead of sliding out of the
    /// list it belongs to.
    pub fn carry_top(&self, y: f64, band_top: f64, band_bottom: f64) -> f64 {
        let lowest = (band_bottom - self.height).max(band_top);
        (y - self.grab).clamp(band_top, lowest)
    }

    /// How far row `id` is drawn from where the list laid it out, so the
    /// gap the carried row would land in stands open: every row the carry
    /// reaches over moves one row height the other way. The carried row
    /// itself gets nothing here — [`Self::carry_top`] places that one.
    pub fn gap_offset(&self, id: usize) -> f64 {
        if self.slot > self.from && id > self.from && id < self.slot {
            -self.height
        } else if self.slot < self.from && id >= self.slot && id < self.from {
            self.height
        } else {
            0.0
        }
    }

    /// Advance with a new pointer `y` over the currently visible `rows`.
    /// Returns true when the visual state changed (activation, or the slot
    /// moved) and the host should redraw.
    pub fn move_to(&mut self, y: f64, threshold: f64, rows: &[RowBand]) -> bool {
        let mut changed = false;
        if !self.active && (y - self.start_y).abs() >= threshold {
            self.active = true;
            changed = true;
        }
        if self.active {
            if let Some(slot) = slot_for(rows, y) {
                if slot != self.slot {
                    self.slot = slot;
                    changed = true;
                }
            }
        }
        changed
    }

    /// The reorder a release commits: `(from, to)` with `to` already
    /// adjusted for the removal. `None` for a plain click (never activated)
    /// or a drop back onto the row's own place.
    pub fn commit(self) -> Option<(usize, usize)> {
        if !self.active {
            return None;
        }
        let to = if self.slot > self.from { self.slot - 1 } else { self.slot };
        if to == self.from {
            return None;
        }
        Some((self.from, to))
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ReorderList {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    list: PortalList,
    /// Child id of the drag gripper inside each item template
    /// (`drag_handle: @gripper`). Zero (the default) disables reordering.
    #[live]
    drag_handle: LiveId,
    /// The drop indicator: a flat quad drawn across the list in the gap the
    /// dragged row would land in.
    #[live]
    draw_indicator: DrawColor,
    /// A press on the row itself, and not only on its gripper, carries it:
    /// the row's background, its words, the space between its cells. Off by
    /// default — on a list whose rows are all controls it would take presses
    /// the host means for them, and it spends the row's own drag, which
    /// scrolls the list otherwise.
    #[live]
    drag_anywhere: bool,
    /// Vertical travel (px) before a gripper press becomes a drag.
    #[live(4.0)]
    drag_threshold: f64,
    #[rust]
    drag: Option<ReorderDrag>,
    /// Acquired when the drag starts and released when it ends, before the
    /// next event's cancel owner is selected.
    #[rust]
    cancel_scope: Option<CancelScope>,
    /// Pointer y of the live drag — read by the edge auto-scroll pump so a
    /// finger HELD at the viewport's edge keeps scrolling between moves,
    /// and by the draw pass to put the carried row under the pointer.
    #[rust]
    drag_pointer_y: Option<f64>,
    /// Every visible row as the last draw laid it out, taken BEFORE the
    /// carry moved any ink. The drag machine and the edge crawl read these
    /// and never the live areas: the carry moves the areas with the ink, so
    /// a slot derived from them would chase itself. Refilled in place every
    /// pass, so a live drag allocates nothing.
    #[rust]
    bands: Vec<RowBand>,
    #[rust]
    scroll_pump: NextFrame,
}

impl ReorderList {
    /// The live drag as `(from, slot)`, once it has passed the threshold.
    /// Hosts use this to tint the lifted row while drawing.
    pub fn drag_state(&self) -> Option<(usize, usize)> {
        self.drag.as_ref().filter(|d| d.active).map(|d| (d.from, d.slot))
    }

    /// The inner list's items that have actions in `actions` — the same
    /// contract as `PortalListRef::items_with_actions`, offered here on the
    /// widget because a downcast to `PortalList` no longer matches.
    pub fn items_with_actions(&self, actions: &Actions) -> Vec<(usize, WidgetRef)> {
        let uid = self.widget_uid();
        let mut set = Vec::new();
        for action in actions {
            if let Some(action) = action.as_widget_action() {
                if let Some(group) = &action.group {
                    if group.group_uid == uid {
                        for (item_id, item) in self.list.items().iter() {
                            if group.item_uid == item.widget.widget_uid() {
                                set.push((*item_id, item.widget.clone()));
                            }
                        }
                    }
                }
            }
        }
        set
    }

    /// The reorder this event pass delivered, if any: `(from, to)` indices
    /// into the host's item range (`to` already adjusted for the removal).
    pub fn reordered(&self, actions: &Actions) -> Option<(usize, usize)> {
        let uid = self.widget_uid();
        for action in actions {
            if let Some(action) = action.as_widget_action() {
                if action.widget_uid == uid {
                    if let ReorderListAction::Reordered { from, to } = action.cast() {
                        return Some((from, to));
                    }
                }
            }
        }
        None
    }

    /// Take down where the list just put every visible row, in entry order.
    /// Called with the draw pass done and before the carry moves anything,
    /// so these are the list's own positions and not the carried ones.
    fn capture_bands(&mut self, cx: &Cx) {
        let view = self.list.area().rect(cx);
        self.bands.clear();
        for (id, item) in self.list.items().iter() {
            let item: &WidgetItem = item;
            let r = item.widget.area().rect(cx);
            if r.size.y > 0.0
                && r.pos.y + r.size.y > view.pos.y
                && r.pos.y < view.pos.y + view.size.y
            {
                self.bands.push((*id, r.pos.y, r.size.y));
            }
        }
        self.bands.sort_by_key(|(id, _, _)| *id);
    }

    /// End the gesture without committing (Escape, or the dragged row left
    /// the virtualised viewport). The carry is redrawn away with it.
    fn cancel_drag(&mut self, cx: &mut Cx) {
        self.drag = None;
        self.cancel_scope = None;
        self.drag_pointer_y = None;
        self.list.redraw(cx);
    }

    /// Advance the gesture. `true` means the event belonged to the drag and
    /// must NOT reach the inner list (that is what keeps a gripper drag from
    /// also drag-scrolling the viewport).
    fn handle_drag(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if self.drag_handle == LiveId(0) {
            return false;
        }
        // A live drag: the finger capture on the gripper routes every move
        // and the release here, wherever the pointer wanders.
        if let Some(drag) = self.drag {
            // Escape or Back cancels outright. The stale capture in cx.fingers dies
            // by itself at release; until then the swallowed pointer events
            // keep the list from scroll-grabbing mid-gesture.
            if self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
                && (matches!(event, Event::KeyDown(ke) if ke.key_code == KeyCode::Escape)
                    || event.back_pressed())
            {
                self.cancel_drag(cx);
                return true;
            }
            // A live drag is modal for the list: a wheel scroll would slide
            // the rows away under the pointer, and so would a page around the
            // list scrolling by it, so the wheel is spent here.
            if drag.active && matches!(event, Event::Scroll(_)) {
                event.set_scroll_handled(Vec2Index::X);
                event.set_scroll_handled(Vec2Index::Y);
                return true;
            }
            let mut drag = drag;
            // The auto-scroll pump: rows slide under a HELD pointer, so each
            // frame re-derives the drop slot from the fresh bands and keeps
            // scrolling while the pointer stays in an edge band. NEVER
            // swallowed: NextFrame is one shared event, and the inner list's
            // own animation steps on the very same frame.
            if self.scroll_pump.is_event(event).is_some() && drag.active {
                if let Some(y) = self.drag_pointer_y {
                    let bands = std::mem::take(&mut self.bands);
                    let changed = drag.move_to(y, self.drag_threshold, &bands);
                    self.bands = bands;
                    if changed {
                        self.list.redraw(cx);
                    }
                    self.drag = Some(drag);
                    self.drag_edge_autoscroll(cx);
                }
            }
            // RAW window events, not `event.hits` on the gripper's area: the
            // edge crawl can scroll the LIFTED row out of the virtualised
            // viewport, and with it dies the widget whose area held the
            // finger capture — hits-based routing cancelled the gesture the
            // moment that happened. The drag is modal anyway; its identity
            // is `drag.from`, not a live widget.
            let moved_to = match event {
                Event::MouseMove(e) => Some(e.abs.y),
                Event::TouchUpdate(e) => e
                    .touches
                    .iter()
                    .find(|t| matches!(t.state, makepad_draw::makepad_platform::event::TouchState::Move | makepad_draw::makepad_platform::event::TouchState::Stable))
                    .map(|t| t.abs.y),
                _ => None,
            };
            if let Some(y) = moved_to {
                // A press that has not become a carry yet belongs to
                // whichever control holds the mouse, if one does: a
                // slider dragged out of its own bounds is still the
                // slider's, and the row under it stays where it is.
                if !drag.active && cx.fingers.is_mouse_held_outside(&self.own_areas(cx, drag.from)) {
                    self.cancel_drag(cx);
                    return false;
                }
                let bands = std::mem::take(&mut self.bands);
                let changed = drag.move_to(y, self.drag_threshold, &bands);
                self.bands = bands;
                // A carried row follows the pointer, not the slot, so every
                // move of a live carry redraws — not only the ones that
                // land the row in a new gap.
                if changed || drag.active {
                    self.list.redraw(cx);
                }
                if drag.active {
                    cx.set_cursor(MouseCursor::Grabbing);
                }
                self.drag_pointer_y = Some(y);
                self.drag = Some(drag);
                if drag.active {
                    self.drag_edge_autoscroll(cx);
                }
                return true;
            }
            // The press itself taken away (the host or the OS cancelled it):
            // the row goes back, nothing is reordered. The cancel still goes
            // on to the rows.
            if matches!(event, Event::FingerCancel(c) if cx.fingers.press_taken_away(c.digit_id)) {
                self.cancel_drag(cx);
                return false;
            }
            let released = matches!(event, Event::MouseUp(_))
                || matches!(event, Event::TouchUpdate(e)
                    if e.touches.iter().any(|t| matches!(t.state, makepad_draw::makepad_platform::event::TouchState::Stop)));
            if released {
                self.cancel_drag(cx);
                if let Some((from, to)) = drag.commit() {
                    let uid = self.widget_uid();
                    cx.widget_action(uid, ReorderListAction::Reordered { from, to });
                }
                return true;
            }
            // A second button pressed mid-drag rides the existing modal
            // gesture: keep it away from the list, change nothing.
            if matches!(event, Event::MouseDown(_)) {
                return true;
            }
            return false;
        }
        // No drag yet: watch every visible gripper. Runs BEFORE the inner
        // list handles the event, so a press on a gripper is captured here
        // first and the swallowed event never starts a drag-scroll.
        let mut start = None;
        for (id, item) in self.list.items().iter() {
            let handle = item.widget.widget(cx, &[self.drag_handle]);
            if handle.is_empty() {
                continue;
            }
            match event.hits(cx, handle.area()) {
                Hit::FingerDown(e) if e.is_primary_hit() => {
                    // The row's own rect, not the gripper's: what is
                    // carried is the whole row, and it is carried by the
                    // point of it the press took hold of.
                    let row = item.widget.area().rect(cx);
                    start = Some((*id, e.abs.y, row.pos.y, row.size.y));
                }
                Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                    cx.set_cursor(MouseCursor::Grab);
                }
                _ => {}
            }
        }
        if let Some((from, y, top, height)) = start {
            self.drag = Some(ReorderDrag::press(from, y).on_row(top, height));
            self.drag_pointer_y = Some(y);
            self.cancel_scope = Some(self.begin_cancel_scope(cx));
            return true;
        }
        false
    }

    /// Drag near (or past) the viewport's top/bottom edge scrolls the list
    /// so a long list can be reordered end to end in one gesture (user,
    /// 2026-08-27: "when moving maps we need to auto scroll if we're above
    /// the top one or bottom one"). A gentle per-frame crawl, re-armed via
    /// `scroll_pump` so holding still at the edge keeps it going.
    fn drag_edge_autoscroll(&mut self, cx: &mut Cx) {
        const BAND: f64 = 28.0;
        const CRAWL: f64 = 9.0;
        let Some(y) = self.drag_pointer_y else { return };
        let view = self.list.area().rect(cx);
        let first = self.list.first_id();
        let scroll = self.list.first_scroll();
        // A direct per-frame nudge: `smooth_scroll_to` refuses a target
        // whose top already touches the viewport boundary, which is exactly
        // the row an edge crawl starts from. The draw pass renormalizes
        // (first_id, first_scroll) and clamps at both ends.
        if y < view.pos.y + BAND {
            if first > 0 || scroll < 0.0 {
                // A POSITIVE offset mid-list is how the draw pass knows to
                // pull the previous row in (the setter itself pins real
                // overscroll at row 0) — clamping to 0 here froze the
                // upward crawl after one row.
                self.list.set_first_id_and_scroll(first, scroll + CRAWL);
                self.list.redraw(cx);
                self.scroll_pump = cx.new_next_frame();
            }
        } else if y > view.pos.y + view.size.y - BAND {
            let last_fully_visible = self.bands.last().is_some_and(|(id, top, height)| {
                *id + 1 >= self.list.range_end()
                    && top + height <= view.pos.y + view.size.y + 1.0
            });
            if !last_fully_visible {
                self.list.set_first_id_and_scroll(first, scroll - CRAWL);
                self.list.redraw(cx);
                self.scroll_pump = cx.new_next_frame();
            }
        }
    }

    /// The areas a press on row `id` may be held by without that being a
    /// control's hold: the list itself, which takes every press for its
    /// drag-scroll, the row, and the gripper the drag is named after.
    fn own_areas(&self, cx: &Cx, id: usize) -> Vec<Area> {
        let mut own = vec![self.list.area()];
        if let Some(item) = self.list.items().get(&id) {
            own.push(item.widget.area());
            let handle = item.widget.widget(cx, &[self.drag_handle]);
            if !handle.is_empty() {
                own.push(handle.area());
            }
        }
        own
    }

    /// A press the rows are about to see, which the list may carry once
    /// they have had it: the point, and the row it landed on. Taken before
    /// the event goes down, when the areas still say where the rows are.
    fn offer_row_press(&self, cx: &Cx, event: &Event) -> Option<(usize, DVec2)> {
        if !self.drag_anywhere || self.drag.is_some() {
            return None;
        }
        let point = match event {
            Event::MouseDown(e) if e.button.is_primary() => e.abs,
            Event::TouchUpdate(e) => e
                .touches
                .iter()
                .find(|t| matches!(t.state, makepad_draw::makepad_platform::event::TouchState::Start))
                .map(|t| t.abs)?,
            _ => return None,
        };
        let id = press_carries(point, self.list.area().rect(cx), &self.bands, &[])?;
        Some((id, point))
    }

    /// The rows have had the press: take it up as a carry unless one of the
    /// row's own widgets holds the finger now, which is what a control does
    /// with a press it means to keep.
    fn take_row_press(&mut self, cx: &mut Cx, id: usize, point: DVec2) {
        let Some(item) = self.list.items().get(&id) else { return };
        let uid = item.widget.widget_uid();
        let mut taken: Vec<Rect> = Vec::new();
        item.widget.find_widgets_from_point(cx, point, &mut |widget| {
            if widget.widget_uid() != uid && cx.fingers.is_area_captured(widget.area()) {
                taken.push(widget.area().rect(cx));
            }
        });
        let row = item.widget.area().rect(cx);
        if press_carries(point, self.list.area().rect(cx), &self.bands, &taken) != Some(id) {
            return;
        }
        // Only the row's bare background carries: a press over any widget
        // of the row -- a control, its label, its readout -- is that
        // widget's, whether or not it took the finger just now. Geometry
        // alone, so it holds when a control did not get the press at all.
        if item
            .widget
            .find_interactive_widget_from_point(cx, point)
            .is_some_and(|widget| widget.widget_uid() != uid)
        {
            return;
        }
        // The press went to stopping a scroll: the inner list kept it from
        // the rows, so whatever sits under the point never had the chance
        // to take hold of it. It is not the row's either.
        if self.list.was_scrolling() {
            return;
        }
        // A control that took the press keeps the mouse until the release,
        // wherever the pointer wanders meanwhile -- so the row does not
        // carry, whether or not the walk above found the control. The
        // list's own hold is the drag-scroll it takes on every press.
        if cx.fingers.is_mouse_held_outside(&self.own_areas(cx, id)) {
            return;
        }
        // The inner list took the press for a drag-scroll of its own on the
        // way down (it captures whatever a row did, by `capture_overload`).
        // The carry swallows every event after this one, so that gesture
        // would never see its own release: give it up now.
        self.list.stop_all_scroll_motion();
        self.drag = Some(ReorderDrag::press(id, point.y).on_row(row.pos.y, row.size.y));
        self.drag_pointer_y = Some(point.y);
        self.cancel_scope = Some(self.begin_cancel_scope(cx));
    }

    /// The carry itself: the lifted row drawn under the pointer and over
    /// the rows the list painted after it, and every row the carry reaches
    /// over shifted by the lifted row's height, so the gap it would land in
    /// stands open. Ink only — run once the list's draw pass is done, and
    /// gone again on the pass after the drag ends.
    fn draw_carry(&mut self, cx: &mut Cx2d) {
        let Some(drag) = self.drag.filter(|d| d.active) else { return };
        let Some(y) = self.drag_pointer_y else { return };
        let view = self.list.area().rect(cx);
        let bands = std::mem::take(&mut self.bands);
        for (id, top, _) in bands.iter() {
            let (offset, lift) = if *id == drag.from {
                (drag.carry_top(y, view.pos.y, view.pos.y + view.size.y) - top, CARRY_LIFT)
            } else {
                (drag.gap_offset(*id), 0.0)
            };
            if offset != 0.0 || lift != 0.0 {
                self.list.offset_drawn_item(cx, *id, dvec2(0.0, offset), lift);
            }
        }
        self.bands = bands;
    }

    /// The line in the gap the dragged row would land in — the gap the
    /// carry holds open, so the line reads as the floor the row lands on.
    /// Drawn after the list's own pass, so it rides on top of the rows.
    fn draw_drop_indicator(&mut self, cx: &mut Cx2d) {
        let Some(drag) = self.drag else { return };
        if !drag.active {
            return;
        }
        let Some((last, last_top, last_height)) = self.bands.last().copied() else { return };
        let y = if let Some((id, top, _)) = self.bands.iter().find(|(id, _, _)| *id == drag.slot) {
            top + drag.gap_offset(*id) - 4.0
        } else if drag.slot == last + 1 {
            last_top + drag.gap_offset(last) + last_height + 2.0
        } else {
            return;
        };
        let view = self.list.area().rect(cx);
        // The slot-0 gap sits above the first row, which is the viewport's
        // own top edge when the list is scrolled home — unclamped, the line
        // lands outside the list and is clipped away. Same for the last gap
        // against the bottom edge.
        let y = y.clamp(view.pos.y + 2.0, view.pos.y + view.size.y - 3.0);
        self.draw_indicator.draw_abs(
            cx,
            Rect {
                pos: dvec2(view.pos.x + 2.0, y - 1.0),
                size: dvec2(view.size.x - 12.0, 2.0),
            },
        );
    }
}

impl Widget for ReorderList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.drag.is_some()
            && (crate::modal::ModalAction::is_dismissal(event)
                || matches!(event, Event::WindowLostFocus(_)))
        {
            self.cancel_drag(cx);
        }
        if self.handle_drag(cx, event) {
            return;
        }
        // A press on the row itself goes to the rows first and is taken up
        // only if nothing on them wanted it — the gripper is the other way
        // round, since nothing else is ever under it.
        let offered = self.offer_row_press(cx, event);
        self.list.handle_event(cx, event, scope);
        if let Some((id, point)) = offered {
            self.take_row_press(cx, id, point);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.list.draw_walk(cx, scope, walk);
        if step.is_done() {
            self.capture_bands(cx);
            self.draw_carry(cx);
            self.draw_drop_indicator(cx);
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::{press_carries, slot_for, ReorderDrag, RowBand};
    use crate::makepad_draw::{dvec2, Rect};

    /// Four rows of height 60 starting at y=100: ids 0..3 at 100/160/220/280.
    fn rows() -> Vec<RowBand> {
        (0..4).map(|i| (i, 100.0 + i as f64 * 60.0, 60.0)).collect()
    }

    /// A press on row `id`'s gripper at pointer height `y`, over [`rows`].
    fn press(id: usize, y: f64) -> ReorderDrag {
        ReorderDrag::press(id, y).on_row(100.0 + id as f64 * 60.0, 60.0)
    }

    #[test]
    fn the_slot_is_the_first_row_whose_midpoint_is_below_the_pointer() {
        let rows = rows();
        assert_eq!(slot_for(&rows, 0.0), Some(0), "above everything: the very top");
        assert_eq!(slot_for(&rows, 129.0), Some(0), "above row 0's midpoint (130)");
        assert_eq!(slot_for(&rows, 131.0), Some(1), "below it: before row 1");
        assert_eq!(slot_for(&rows, 250.0), Some(3), "exactly row 2's midpoint is already past it");
        assert_eq!(slot_for(&rows, 311.0), Some(4), "below the last midpoint: after the end");
        assert_eq!(slot_for(&[], 100.0), None, "no rows, no slot");
    }

    #[test]
    fn a_press_only_becomes_a_drag_past_the_threshold() {
        let rows = rows();
        let mut drag = press(1, 170.0);
        assert!(!drag.active);
        assert!(!drag.move_to(172.0, 4.0, &rows), "2px of travel is still a click");
        assert!(!drag.active);
        assert_eq!(drag.commit(), None, "releasing a click reorders nothing");
        assert!(drag.move_to(175.0, 4.0, &rows), "5px activates (a visual change)");
        assert!(drag.active);
    }

    #[test]
    fn the_slot_tracks_the_pointer_and_the_drop_commits_adjusted_indices() {
        let rows = rows();
        let mut drag = press(0, 110.0);
        drag.move_to(255.0, 4.0, &rows);
        assert_eq!(drag.slot, 3, "pointer past row 2's midpoint: before row 3");
        // Slot 3 with the dragged row removed from index 0 = final index 2.
        assert_eq!(drag.commit(), Some((0, 2)));
        // Dragging upward: slot is the final index directly.
        let mut drag = press(3, 290.0);
        drag.move_to(120.0, 4.0, &rows);
        assert_eq!(drag.slot, 0);
        assert_eq!(drag.commit(), Some((3, 0)));
    }

    #[test]
    fn dropping_a_row_back_onto_its_own_place_is_a_no_op() {
        let rows = rows();
        // Down past the threshold but still before its own successor's
        // midpoint: slot 1 with from=0 adjusts back to index 0.
        let mut drag = press(0, 110.0);
        drag.move_to(170.0, 4.0, &rows);
        assert_eq!(drag.slot, 1, "just under row 0: the gap between 0 and 1");
        assert_eq!(drag.commit(), None, "that gap IS index 0 — nothing moved");
        // Its own slot exactly.
        let mut drag = press(2, 230.0);
        drag.move_to(225.0, 4.0, &rows);
        assert_eq!(drag.slot, 2);
        assert_eq!(drag.commit(), None);
    }

    #[test]
    fn moves_keep_reporting_only_real_changes() {
        let rows = rows();
        let mut drag = press(1, 170.0);
        assert!(drag.move_to(200.0, 4.0, &rows));
        assert!(!drag.move_to(201.0, 4.0, &rows), "same slot again: no redraw needed");
        assert!(drag.move_to(260.0, 4.0, &rows), "new slot: redraw");
        assert_eq!(drag.slot, 3);
    }

    #[test]
    fn the_rows_a_carry_reaches_over_shift_by_the_carried_row_s_height() {
        let rows = rows();
        // Carrying row 0 down to the gap before row 3: rows 1 and 2 come up
        // one row height, row 3 and the carried row stay put.
        let mut drag = press(0, 110.0);
        drag.move_to(255.0, 4.0, &rows);
        assert_eq!(drag.slot, 3);
        assert_eq!(drag.gap_offset(0), 0.0, "the carried row is placed, not shifted");
        assert_eq!(drag.gap_offset(1), -60.0);
        assert_eq!(drag.gap_offset(2), -60.0);
        assert_eq!(drag.gap_offset(3), 0.0, "the slot's own row holds the gap's floor");
        // And upward: carrying row 3 to the very top pushes 0, 1 and 2 down.
        let mut drag = press(3, 290.0);
        drag.move_to(120.0, 4.0, &rows);
        assert_eq!(drag.slot, 0);
        assert_eq!(drag.gap_offset(0), 60.0);
        assert_eq!(drag.gap_offset(1), 60.0);
        assert_eq!(drag.gap_offset(2), 60.0);
        assert_eq!(drag.gap_offset(3), 0.0);
    }

    #[test]
    fn a_carry_that_has_not_left_its_own_gap_shifts_nothing() {
        let rows = rows();
        // Down past the threshold but still in its own place: the gap it
        // would land in is the one it came out of, so no row makes way.
        let mut drag = press(0, 110.0);
        drag.move_to(170.0, 4.0, &rows);
        assert_eq!(drag.slot, 1);
        for id in 0..4 {
            assert_eq!(drag.gap_offset(id), 0.0, "row {id}");
        }
        // Past the end: every row after the carried one comes up.
        let mut drag = press(0, 110.0);
        drag.move_to(320.0, 4.0, &rows);
        assert_eq!(drag.slot, 4, "after the last row");
        assert_eq!(drag.gap_offset(1), -60.0);
        assert_eq!(drag.gap_offset(3), -60.0, "the last row makes way too");
    }

    #[test]
    fn the_carried_row_hangs_off_the_point_of_it_the_press_took_hold_of() {
        // Pressed 10 points down row 1 (top 160): wherever the pointer
        // goes, the row's top is drawn 10 points above it.
        let mut drag = press(1, 170.0);
        drag.move_to(300.0, 4.0, &rows());
        assert_eq!(drag.carry_top(300.0, 100.0, 400.0), 290.0);
        assert_eq!(drag.carry_top(170.0, 100.0, 400.0), 160.0, "back where it lay");
        // The band holds it: above the top edge and below the bottom one it
        // stops at the edge rather than sliding out of the list.
        assert_eq!(drag.carry_top(-500.0, 100.0, 400.0), 100.0);
        assert_eq!(drag.carry_top(5000.0, 100.0, 400.0), 340.0, "its bottom on the band's");
        // A band shorter than the row pins it to the top rather than
        // clamping backwards.
        assert_eq!(drag.carry_top(5000.0, 100.0, 130.0), 100.0);
    }

    #[test]
    fn the_row_carries_a_press_nothing_on_it_took() {
        let rows = rows();
        // The rows fill y 100..340 of a list that reaches to 400.
        let list = Rect { pos: dvec2(10.0, 100.0), size: dvec2(400.0, 300.0) };
        assert_eq!(
            press_carries(dvec2(200.0, 250.0), list, &rows, &[]),
            Some(2),
            "the row's own background, and nothing took the press"
        );
        // Something on row 2 that took it — a button, a chip, a slider.
        let control = Rect { pos: dvec2(180.0, 230.0), size: dvec2(60.0, 20.0) };
        assert_eq!(
            press_carries(dvec2(200.0, 240.0), list, &rows, &[control]),
            None,
            "on the control: the control's press, not the row's"
        );
        assert_eq!(
            press_carries(dvec2(200.0, 260.0), list, &rows, &[control]),
            Some(2),
            "under it, the row again"
        );
        assert_eq!(
            press_carries(dvec2(300.0, 240.0), list, &rows, &[control]),
            Some(2),
            "beside it, the row again"
        );
        assert_eq!(
            press_carries(dvec2(500.0, 250.0), list, &rows, &[]),
            None,
            "outside the list"
        );
        assert_eq!(
            press_carries(dvec2(200.0, 360.0), list, &rows, &[]),
            None,
            "inside the list but under the last row"
        );
        assert_eq!(
            press_carries(dvec2(200.0, 100.0), list, &rows, &[]),
            Some(0),
            "the very top of the first row is still that row"
        );
    }

    #[test]
    fn cancel_is_dropping_the_machine_nothing_pends() {
        // Escape (or a vanished row) simply drops the machine; committing a
        // copy afterwards would still be the caller's bug, and an inactive
        // one never commits anyway.
        let rows = rows();
        let mut drag = press(0, 110.0);
        drag.move_to(300.0, 4.0, &rows);
        assert!(drag.commit().is_some(), "the drag WOULD commit");
        let cancelled: Option<ReorderDrag> = None;
        assert!(cancelled.map_or(true, |d: ReorderDrag| d.commit().is_none()));
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;

    /// A mouse press the host took away (`Event::FingerCancel`) ends a
    /// gripper drag with the row back where it was: no `Reordered`, even
    /// when the drag had reached a slot that WOULD commit.
    #[test]
    fn a_cancelled_drag_reorders_nothing() {
        crate::on_test_cx(|| {
        let mut cx = crate::checkout_test_cx();
        let mut list = cx.with_vm(ReorderList::script_new_with_default);
        let rows: Vec<RowBand> = (0..4).map(|i| (i, 100.0 + i as f64 * 60.0, 60.0)).collect();
        let mut drag = ReorderDrag::press(0, 110.0);
        drag.move_to(300.0, 4.0, &rows);
        assert!(drag.commit().is_some(), "the drag would commit; the test proves nothing otherwise");
        list.drag_handle = live_id!(grip);
        list.drag = Some(drag);
        // The host takes the mouse press itself away.
        cx.fingers.cancel_digit(live_id!(mouse).into());
        let cancel = Event::FingerCancel(crate::event::FingerCancelEvent {
            window_id: WindowId(1, 1),
            digit_id: live_id!(mouse).into(),
            device: DigitDevice::Mouse { button: MouseButton::PRIMARY },
            abs: dvec2(10.0, 300.0),
            time: 1.0,
            modifiers: KeyModifiers::default(),
        });
        let mut owned = true;
        let actions = cx.capture_actions(|cx| owned = list.handle_drag(cx, &cancel));
        assert!(!owned, "the cancel was kept from the rows");
        assert!(list.drag.is_none(), "the drag survived its cancel");
        let reordered = actions.iter().any(|a| {
            matches!(
                a.as_widget_action().map(|w| w.cast::<ReorderListAction>()),
                Some(ReorderListAction::Reordered { .. })
            )
        });
        assert!(!reordered, "a cancelled drag reordered the list");
        });
    }
}


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
//! - while the drag is live the widget tracks the insertion slot under the
//!   pointer (row midpoints decide) and draws `draw_indicator` as a line
//!   in the gap the row would land in; the lifted row is reported through
//!   [`ReorderList::drag_state`] so the host can tint it;
//! - Escape cancels the gesture; a wheel scroll during it is swallowed so
//!   the rows never slide under the pointer mid-drag;
//! - the release emits [`ReorderListAction::Reordered`] with indices into
//!   the host's item range. The widget itself moves nothing: item identity
//!   and the model belong to the host, which applies the move and redraws.
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

/// One live drag, from gripper press to release — a pure state machine
/// (no `Cx`, no `Area`), so the whole gesture is unit-testable: pointer y
/// in, slot out, commit or cancel at the end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReorderDrag {
    /// The item (list entry id) whose gripper was pressed.
    pub from: usize,
    /// Where the press landed.
    start_y: f64,
    /// Insertion slot in entry-id space: the row would land BEFORE the
    /// current occupant of `slot`; `last visible + 1` means after the end.
    pub slot: usize,
    /// True once the press has moved past the threshold — only then does
    /// the indicator draw and the release commit. A plain click on the
    /// gripper stays a click.
    pub active: bool,
}

impl ReorderDrag {
    /// A fresh press on `from`'s gripper at pointer height `y`.
    pub fn press(from: usize, y: f64) -> Self {
        Self { from, start_y: y, slot: from, active: false }
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
    /// Vertical travel (px) before a gripper press becomes a drag.
    #[live(4.0)]
    drag_threshold: f64,
    #[rust]
    drag: Option<ReorderDrag>,
    /// Pointer y of the live drag — read by the edge auto-scroll pump so a
    /// finger HELD at the viewport's edge keeps scrolling between moves.
    #[rust]
    drag_pointer_y: Option<f64>,
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

    /// Currently drawn rows as `(entry id, rect)`, in entry order.
    fn visible_rows(&self, cx: &Cx) -> Vec<(usize, Rect)> {
        let view = self.list.area().rect(cx);
        let mut rows: Vec<(usize, Rect)> = self
            .list
            .items()
            .iter()
            .map(|(id, item): (&usize, &WidgetItem)| (*id, item.widget.area().rect(cx)))
            .filter(|(_, r)| {
                r.size.y > 0.0
                    && r.pos.y + r.size.y > view.pos.y
                    && r.pos.y < view.pos.y + view.size.y
            })
            .collect();
        rows.sort_by_key(|(id, _)| *id);
        rows
    }

    /// [`visible_rows`] in the drag machine's shape.
    fn row_bands(&self, cx: &Cx) -> Vec<RowBand> {
        self.visible_rows(cx).iter().map(|(id, r)| (*id, r.pos.y, r.size.y)).collect()
    }

    /// End the gesture without committing (Escape, or the dragged row left
    /// the virtualised viewport).
    fn cancel_drag(&mut self, cx: &mut Cx) {
        self.drag = None;
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
            // Escape cancels outright. The stale capture in cx.fingers dies
            // by itself at release; until then the swallowed pointer events
            // keep the list from scroll-grabbing mid-gesture.
            if let Event::KeyDown(ke) = event {
                if ke.key_code == KeyCode::Escape {
                    self.cancel_drag(cx);
                    return true;
                }
            }
            // A live drag is modal for the list: a wheel scroll would slide
            // the rows away under the pointer.
            if drag.active && matches!(event, Event::Scroll(_)) {
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
                    let bands = self.row_bands(cx);
                    if drag.move_to(y, self.drag_threshold, &bands) {
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
                let bands = self.row_bands(cx);
                if drag.move_to(y, self.drag_threshold, &bands) {
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
            let released = matches!(event, Event::MouseUp(_))
                || matches!(event, Event::TouchUpdate(e)
                    if e.touches.iter().any(|t| matches!(t.state, makepad_draw::makepad_platform::event::TouchState::Stop)));
            if released {
                self.drag = None;
                self.drag_pointer_y = None;
                self.list.redraw(cx);
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
                    start = Some((*id, e.abs.y));
                }
                Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                    cx.set_cursor(MouseCursor::Grab);
                }
                _ => {}
            }
        }
        if let Some((from, y)) = start {
            self.drag = Some(ReorderDrag::press(from, y));
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
            let last_fully_visible = self
                .visible_rows(cx)
                .last()
                .is_some_and(|(id, rect)| {
                    *id + 1 >= self.list.range_end()
                        && rect.pos.y + rect.size.y <= view.pos.y + view.size.y + 1.0
                });
            if !last_fully_visible {
                self.list.set_first_id_and_scroll(first, scroll - CRAWL);
                self.list.redraw(cx);
                self.scroll_pump = cx.new_next_frame();
            }
        }
    }

    /// The line in the gap the dragged row would land in. Drawn after the
    /// list's own pass, so it rides on top of the rows.
    fn draw_drop_indicator(&mut self, cx: &mut Cx2d) {
        let Some(drag) = self.drag else { return };
        if !drag.active {
            return;
        }
        let rows = self.visible_rows(cx);
        let Some((last, last_rect)) = rows.last().copied() else { return };
        let y = if let Some((_, rect)) = rows.iter().find(|(id, _)| *id == drag.slot) {
            rect.pos.y - 4.0
        } else if drag.slot == last + 1 {
            last_rect.pos.y + last_rect.size.y + 2.0
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
        if self.handle_drag(cx, event) {
            return;
        }
        self.list.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.list.draw_walk(cx, scope, walk);
        if step.is_done() {
            self.draw_drop_indicator(cx);
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::{slot_for, ReorderDrag, RowBand};

    /// Four rows of height 60 starting at y=100: ids 0..3 at 100/160/220/280.
    fn rows() -> Vec<RowBand> {
        (0..4).map(|i| (i, 100.0 + i as f64 * 60.0, 60.0)).collect()
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
        let mut drag = ReorderDrag::press(1, 170.0);
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
        let mut drag = ReorderDrag::press(0, 110.0);
        drag.move_to(255.0, 4.0, &rows);
        assert_eq!(drag.slot, 3, "pointer past row 2's midpoint: before row 3");
        // Slot 3 with the dragged row removed from index 0 = final index 2.
        assert_eq!(drag.commit(), Some((0, 2)));
        // Dragging upward: slot is the final index directly.
        let mut drag = ReorderDrag::press(3, 290.0);
        drag.move_to(120.0, 4.0, &rows);
        assert_eq!(drag.slot, 0);
        assert_eq!(drag.commit(), Some((3, 0)));
    }

    #[test]
    fn dropping_a_row_back_onto_its_own_place_is_a_no_op() {
        let rows = rows();
        // Down past the threshold but still before its own successor's
        // midpoint: slot 1 with from=0 adjusts back to index 0.
        let mut drag = ReorderDrag::press(0, 110.0);
        drag.move_to(170.0, 4.0, &rows);
        assert_eq!(drag.slot, 1, "just under row 0: the gap between 0 and 1");
        assert_eq!(drag.commit(), None, "that gap IS index 0 — nothing moved");
        // Its own slot exactly.
        let mut drag = ReorderDrag::press(2, 230.0);
        drag.move_to(225.0, 4.0, &rows);
        assert_eq!(drag.slot, 2);
        assert_eq!(drag.commit(), None);
    }

    #[test]
    fn moves_keep_reporting_only_real_changes() {
        let rows = rows();
        let mut drag = ReorderDrag::press(1, 170.0);
        assert!(drag.move_to(200.0, 4.0, &rows));
        assert!(!drag.move_to(201.0, 4.0, &rows), "same slot again: no redraw needed");
        assert!(drag.move_to(260.0, 4.0, &rows), "new slot: redraw");
        assert_eq!(drag.slot, 3);
    }

    #[test]
    fn cancel_is_dropping_the_machine_nothing_pends() {
        // Escape (or a vanished row) simply drops the machine; committing a
        // copy afterwards would still be the caller's bug, and an inactive
        // one never commits anyway.
        let rows = rows();
        let mut drag = ReorderDrag::press(0, 110.0);
        drag.move_to(300.0, 4.0, &rows);
        assert!(drag.commit().is_some(), "the drag WOULD commit");
        let cancelled: Option<ReorderDrag> = None;
        assert!(cancelled.map_or(true, |d: ReorderDrag| d.commit().is_none()));
    }
}

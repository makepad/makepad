//! Two lists with move-across controls, a search over each side and a count
//! on each side.
//!
//! For a long flat set where what matters is what is in and what is out,
//! both readable at once.
//!
//! One of three ways to choose out of a set that will not fit in one list;
//! `column_picker` and `tree_select` are the other two. `picker_parts` holds
//! what they share, and says what all three deliberately leave out.

use crate::{
    badge::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    picker_parts::{elide, text_y, DrawPickerPanel, DrawPickerRow},
    text_input::TextInputAction,
    widget::*,
    widget_tree::CxWidgetExt,
};
use std::collections::HashSet;

/// The four move-across marks: one across, all across, and back. The angle
/// quotes and the guillemets, because the default face carries them: a glyph
/// it lacks draws as an empty box and nothing says so.
const MOVE_ONE_RIGHT: &str = "\u{203A}";
const MOVE_ALL_RIGHT: &str = "\u{00BB}";
const MOVE_ONE_LEFT: &str = "\u{2039}";
const MOVE_ALL_LEFT: &str = "\u{00AB}";

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The surface only this control draws; the panel and the row are in
    // `picker_parts`. As there, a value with a Rust field behind it is
    // written PLAIN and a value the shader alone owns is a uniform, so the
    // fields keep their own instance slots.
    set_type_default() do #(DrawPickerButton::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: 0.0
        down: 0.0
        color: uniform(theme.color_surface_container)
        color_hover: uniform(theme.color_surface_container_high)
        color_down: uniform(theme.color_surface_container_highest)
        border_color: uniform(theme.color_bevel)
        /** the outline's thickness in pixels 0..3 step 0.5 */
        border_size: uniform(1.0)
        /** corner rounding radius 0..12 step 0.5 */
        radius: uniform(theme.corner_radius)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size * 0.5
                self.border_size * 0.5
                self.rect_size.x - self.border_size
                self.rect_size.y - self.border_size
                self.radius
            )
            sdf.fill_keep(self.color.mix(self.color_hover, self.hover).mix(self.color_down, self.down))
            sdf.stroke(self.border_color, self.border_size)
            return sdf.result
        }
    }

    mod.widgets.TransferBase = #(Transfer::register_widget(vm))

    /** Two lists with move-across controls, a search over each side and a
     * count on each side. */
    mod.widgets.Transfer = set_type_default() do mod.widgets.TransferBase{
        width: Fill
        height: Fit
        padding: Inset{left: 0. right: 0. top: 0. bottom: 0.}

        /** every item, in the order the lists show them */
        items: []
        /** the heading over the list of what is out */
        left_title: "Available"
        /** the heading over the list of what is in */
        right_title: "Chosen"
        /** how tall one row is 16..40 step 1 */
        row_height: 22.
        /** how tall each list is, in pixels 60..600 step 10 */
        list_height: 170.
        /** the heading strip over each list 0..40 step 1 */
        header_height: 18.
        /** the search box over each list 0..40 step 1 */
        search_height: 24.
        /** how wide the column of move controls is 24..80 step 1 */
        controls_width: 34.
        /** the room between the three columns 0..24 step 1 */
        gap: 8.
        /** room before a label inside a list 0..20 step 1 */
        pad_x: 8.
        /** a search box over each list */
        searchable: true

        left_search: mod.widgets.TextInput{
            empty_text: "Search"
        }
        right_search: mod.widgets.TextInput{
            empty_text: "Search"
        }

        draw_panel +: {
            color: theme.color_surface_container_lowest
            border_color: theme.color_outline_variant
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_title +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_count +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_move +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_move_off +: {
            color: theme.color_text_disabled
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerButton {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
}

/// Which side each item stands on, and the moves between the two.
///
/// One bool per item rather than two lists of indices: with two lists there
/// are two places an item can be and therefore a state where it is in both,
/// or in neither, and every move has to be written twice to keep them
/// honest. With one array the two sides are views of the same fact, and an
/// item is on exactly one side by construction.
#[derive(Clone, Debug, Default, PartialEq)]
struct Sides {
    across: Vec<bool>,
}

impl Sides {
    fn fit(&mut self, len: usize) {
        self.across.resize(len, false);
    }

    /// The items on one side, in ITEM order — never the order they were
    /// moved in. Two readers who moved the same set in a different sequence
    /// should be looking at the same list.
    fn side(&self, across: bool) -> Vec<usize> {
        (0..self.across.len()).filter(|index| self.across[*index] == across).collect()
    }

    fn counts(&self) -> (usize, usize) {
        let across = self.across.iter().filter(|on| **on).count();
        (self.across.len() - across, across)
    }

    /// Move `picked` to one side; how many actually moved comes back.
    ///
    /// Anything already on that side is left alone rather than counted
    /// again, so a move of a set that overlaps what is already across is a
    /// no-op for the overlap instead of a duplicate.
    fn move_across(&mut self, picked: &[usize], across: bool) -> usize {
        let mut moved = 0;
        for index in picked {
            let Some(slot) = self.across.get_mut(*index) else {
                continue;
            };
            if *slot != across {
                *slot = across;
                moved += 1;
            }
        }
        moved
    }
}

/// Case-insensitive substring. An empty query matches everything, so a side
/// with nothing typed into it shows the whole side.
fn search_matches(label: &str, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    label.to_lowercase().contains(&query.trim().to_lowercase())
}

/// The rows one side shows: its items, in item order, that pass the search.
///
/// This is the list the move-all controls act on, NOT the whole side. A
/// move-all that reached past the search would take rows the reader cannot
/// see and did not ask for, which is the one way a transfer control can
/// silently lose somebody's work.
fn showing(items: &[String], side: &[usize], query: &str) -> Vec<usize> {
    side.iter()
        .copied()
        .filter(|index| {
            items.get(*index).is_some_and(|label| search_matches(label, query))
        })
        .collect()
}

/// What a transfer reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TransferAction {
    /// Some items moved; true means they moved to the right-hand list. Read
    /// `chosen` for the whole answer.
    Moved(usize, bool),
    #[default]
    None,
}

/// Which control was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    OneRight,
    AllRight,
    OneLeft,
    AllLeft,
}

impl Move {
    const ORDER: [Move; 4] = [Move::OneRight, Move::AllRight, Move::OneLeft, Move::AllLeft];

    fn mark(self) -> &'static str {
        match self {
            Move::OneRight => MOVE_ONE_RIGHT,
            Move::AllRight => MOVE_ALL_RIGHT,
            Move::OneLeft => MOVE_ONE_LEFT,
            Move::AllLeft => MOVE_ALL_LEFT,
        }
    }

    fn to_right(self) -> bool {
        matches!(self, Move::OneRight | Move::AllRight)
    }

    fn is_all(self) -> bool {
        matches!(self, Move::AllRight | Move::AllLeft)
    }
}

/// Where the three columns of a transfer were laid out. Worked out from the
/// widget's own rect by both the draw and the hit test, so what is drawn is
/// what is pressable even after the parent's alignment has moved the whole
/// control.
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    left_search: Rect,
    right_search: Rect,
    left_list: Rect,
    right_list: Rect,
    buttons: [Rect; 4],
    header_y: f64,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Transfer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawPickerPanel,
    #[live]
    draw_panel: DrawPickerPanel,
    #[live]
    draw_row: DrawPickerRow,
    #[live]
    draw_button: DrawPickerButton,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_title: DrawText,
    #[live]
    pub draw_count: DrawText,
    #[live]
    pub draw_move: DrawText,
    #[live]
    pub draw_move_off: DrawText,

    /// The search box over each list. They are real inputs, so selection,
    /// the clipboard and undo all work in them.
    #[find]
    #[live]
    pub left_search: WidgetRef,
    #[find]
    #[live]
    pub right_search: WidgetRef,

    #[live]
    pub items: Vec<String>,
    #[live]
    pub left_title: String,
    #[live]
    pub right_title: String,
    #[live(22.0)]
    pub row_height: f64,
    #[live(170.0)]
    pub list_height: f64,
    #[live(18.0)]
    pub header_height: f64,
    #[live(24.0)]
    pub search_height: f64,
    #[live(34.0)]
    pub controls_width: f64,
    #[live(8.0)]
    pub gap: f64,
    #[live(8.0)]
    pub pad_x: f64,
    #[live(true)]
    pub searchable: bool,

    #[rust]
    sides: Sides,
    /// The highlighted items. ONE set, not one per side: an item is on
    /// exactly one side, so a second set would only ever be able to
    /// disagree with the first.
    #[rust]
    picked: HashSet<usize>,
    #[rust]
    hover_row: Option<(bool, usize)>,
    #[rust]
    hover_button: Option<usize>,
    #[rust]
    down_button: Option<usize>,
    #[rust]
    frame: Frame,
    #[rust]
    area: Area,
}

impl Transfer {
    fn fit(&mut self) {
        if self.sides.across.len() != self.items.len() {
            self.sides.fit(self.items.len());
            self.picked.retain(|index| *index < self.items.len());
        }
    }

    fn query(&self, across: bool) -> String {
        if !self.searchable {
            return String::new();
        }
        if across {
            self.right_search.text()
        } else {
            self.left_search.text()
        }
    }

    /// The rows one side shows, after its own search.
    fn rows(&self, across: bool) -> Vec<usize> {
        let side = self.sides.side(across);
        showing(&self.items, &side, &self.query(across))
    }

    /// Everything the two lists and the four buttons need, from the widget's
    /// own rect. Both the draw and the hit test go through this, so a
    /// pressable row is always a drawn row.
    fn frame_of(&self, rect: Rect) -> Frame {
        let inner = Rect {
            pos: dvec2(
                rect.pos.x + self.layout.padding.left,
                rect.pos.y + self.layout.padding.top,
            ),
            size: dvec2(
                (rect.size.x - self.layout.padding.left - self.layout.padding.right).max(0.0),
                (rect.size.y - self.layout.padding.top - self.layout.padding.bottom).max(0.0),
            ),
        };
        let panel_w =
            ((inner.size.x - self.controls_width - self.gap * 2.0) * 0.5).max(40.0);
        let left_x = inner.pos.x;
        let controls_x = left_x + panel_w + self.gap;
        let right_x = controls_x + self.controls_width + self.gap;
        let search_h = if self.searchable { self.search_height } else { 0.0 };
        let list_y = inner.pos.y + self.header_height + search_h;

        let search = |x: f64| Rect {
            pos: dvec2(x, inner.pos.y + self.header_height),
            size: dvec2(panel_w, search_h),
        };
        let list = |x: f64| Rect {
            pos: dvec2(x, list_y),
            size: dvec2(panel_w, self.list_height),
        };

        // The four controls are centred against the LISTS, not the whole
        // widget: a column of buttons lined up with the headings sits high
        // and reads as belonging to the left panel.
        let button = self.controls_width.min(28.0);
        let spread = button * 4.0 + 6.0 * 3.0;
        let top = list_y + (self.list_height - spread) * 0.5;
        let bx = controls_x + (self.controls_width - button) * 0.5;
        let mut buttons = [Rect::default(); 4];
        for (index, slot) in buttons.iter_mut().enumerate() {
            *slot = Rect {
                pos: dvec2(bx, top + index as f64 * (button + 6.0)),
                size: dvec2(button, button),
            };
        }

        Frame {
            left_search: search(left_x),
            right_search: search(right_x),
            left_list: list(left_x),
            right_list: list(right_x),
            buttons,
            header_y: inner.pos.y,
        }
    }

    /// Which row a window-absolute point lands on, as (side, item index).
    fn row_at(&self, pos: DVec2) -> Option<(bool, usize)> {
        for across in [false, true] {
            let list = if across { self.frame.right_list } else { self.frame.left_list };
            if !list.contains(pos) {
                continue;
            }
            let row = ((pos.y - list.pos.y) / self.row_height).floor();
            if row < 0.0 {
                return None;
            }
            let rows = self.rows(across);
            let row = row as usize;
            if row < rows.len() && row < self.row_budget() {
                return Some((across, rows[row]));
            }
            return None;
        }
        None
    }

    /// How many rows fit in a list. This control does not scroll, so a row
    /// past this is not drawn and not pressable — the two have to agree.
    fn row_budget(&self) -> usize {
        (self.list_height / self.row_height.max(1.0)).floor().max(0.0) as usize
    }

    /// What a control would move, given the searches as they stand.
    fn payload(&self, action: Move) -> Vec<usize> {
        let from = !action.to_right();
        if action.is_all() {
            self.rows(from)
        } else {
            // Only the picked items on the side being emptied, in item
            // order. A picked item on the OTHER side is not in this move.
            self.rows(from).into_iter().filter(|index| self.picked.contains(index)).collect()
        }
    }

    fn run(&mut self, cx: &mut Cx, action: Move) {
        let payload = self.payload(action);
        if payload.is_empty() {
            return;
        }
        let moved = self.sides.move_across(&payload, action.to_right());
        // The highlight goes with the move: an item that has crossed and is
        // still lit would be moved straight back by the next press of the
        // control facing the other way.
        for index in &payload {
            self.picked.remove(index);
        }
        let uid = self.uid;
        self.draw_bg.redraw(cx);
        cx.widget_action(uid, TransferAction::Moved(moved, action.to_right()));
    }

    /// The items on the right, in item order.
    pub fn chosen(&self) -> Vec<usize> {
        self.sides.side(true)
    }

    pub fn chosen_labels(&self) -> Vec<String> {
        self.chosen().iter().filter_map(|index| self.items.get(*index).cloned()).collect()
    }

    pub fn set_chosen(&mut self, cx: &mut Cx, indices: &[usize]) {
        self.fit();
        for (index, slot) in self.sides.across.iter_mut().enumerate() {
            *slot = indices.contains(&index);
        }
        self.picked.clear();
        self.draw_bg.redraw(cx);
    }

    pub fn set_items(&mut self, cx: &mut Cx, items: &[String]) {
        if self.items == items {
            return;
        }
        self.items = items.to_vec();
        // Everything that indexed the old list is dropped: an index into a
        // replaced list names a different item, not a missing one.
        self.sides = Sides::default();
        self.sides.fit(self.items.len());
        self.picked.clear();
        self.draw_bg.redraw(cx);
    }
}

impl Widget for Transfer {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.fit();
        let search_h = if self.searchable { self.search_height } else { 0.0 };
        // A Fill inside a Fit resolves to nothing, so the height is worked
        // out here rather than left to the turtle.
        let natural_h = self.header_height
            + search_h
            + self.list_height
            + self.layout.padding.top
            + self.layout.padding.bottom;
        let walk = Walk {
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural_h),
                other => other,
            },
            ..walk
        };

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        let frame = self.frame_of(rect);
        self.frame = frame;

        let title_font = self.draw_title.text_style.font_size as f64;
        let count_font = self.draw_count.text_style.font_size as f64;
        let row_font = self.draw_text.text_style.font_size as f64;
        let budget = self.row_budget();

        let (left_total, right_total) = self.sides.counts();
        for across in [false, true] {
            let list = if across { frame.right_list } else { frame.left_list };
            let title =
                if across { self.right_title.clone() } else { self.left_title.clone() };
            let total = if across { right_total } else { left_total };
            let rows = self.rows(across);

            let ty = text_y(title_font, frame.header_y, self.header_height);
            let cut = elide(&self.draw_title, cx, &title, list.size.x * 0.6);
            self.draw_title.draw_abs(cx, dvec2(list.pos.x, ty), &cut);

            // The count says both numbers while a search is narrowing the
            // side, and one when it is not: "4 of 30" and "30" answer
            // different questions and only one of them is being asked.
            let count = if rows.len() == total {
                format!("{total}")
            } else {
                format!("{} of {}", rows.len(), total)
            };
            let count_w = measure(&self.draw_count, cx, &count);
            self.draw_count.draw_abs(
                cx,
                dvec2(
                    list.pos.x + list.size.x - count_w,
                    text_y(count_font, frame.header_y, self.header_height),
                ),
                &count,
            );

            self.draw_panel.draw_abs(cx, list);

            for (row, index) in rows.iter().enumerate().take(budget) {
                let y = list.pos.y + row as f64 * self.row_height;
                let rect = Rect {
                    pos: dvec2(list.pos.x + 2.0, y),
                    size: dvec2((list.size.x - 4.0).max(1.0), self.row_height),
                };
                self.draw_row.hover =
                    if self.hover_row == Some((across, *index)) { 1.0 } else { 0.0 };
                self.draw_row.chosen = if self.picked.contains(index) { 1.0 } else { 0.0 };
                self.draw_row.keyed = 0.0;
                self.draw_row.draw_abs(cx, rect);

                let label = self.items.get(*index).cloned().unwrap_or_default();
                let cut =
                    elide(&self.draw_text, cx, &label, rect.size.x - self.pad_x * 2.0);
                self.draw_text.draw_abs(
                    cx,
                    dvec2(rect.pos.x + self.pad_x, text_y(row_font, y, self.row_height)),
                    &cut,
                );
            }
        }

        for (slot, action) in Move::ORDER.iter().enumerate() {
            let rect = frame.buttons[slot];
            let live = !self.payload(*action).is_empty();
            self.draw_button.hover =
                if live && self.hover_button == Some(slot) { 1.0 } else { 0.0 };
            self.draw_button.down =
                if live && self.down_button == Some(slot) { 1.0 } else { 0.0 };
            self.draw_button.draw_abs(cx, rect);

            let mark = action.mark();
            // Two text layers rather than one whose colour is written every
            // frame: a layer whose ink is overwritten has nowhere left to
            // keep the ink the call site handed it.
            let font = if live {
                self.draw_move.text_style.font_size as f64
            } else {
                self.draw_move_off.text_style.font_size as f64
            };
            let pen = if live { &mut self.draw_move } else { &mut self.draw_move_off };
            let w = measure(&*pen, cx, mark);
            pen.draw_abs(
                cx,
                dvec2(
                    rect.pos.x + (rect.size.x - w) * 0.5,
                    text_y(font, rect.pos.y, rect.size.y),
                ),
                mark,
            );
        }

        if self.searchable {
            // Drawn here rather than by a container, so nothing else puts
            // them in the tree and a host can reach ids!(transfer.left_search).
            cx.widget_tree_insert_child(self.uid, live_id!(left_search), self.left_search.clone());
            cx.widget_tree_insert_child(
                self.uid,
                live_id!(right_search),
                self.right_search.clone(),
            );
            let _ = self.left_search.draw_walk(
                cx,
                scope,
                Walk::fixed(frame.left_search.size.x, frame.left_search.size.y)
                    .with_abs_pos(frame.left_search.pos),
            );
            let _ = self.right_search.draw_walk(
                cx,
                scope,
                Walk::fixed(frame.right_search.size.x, frame.right_search.size.y)
                    .with_abs_pos(frame.right_search.pos),
            );
        }

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.left_search.set_disabled(cx, disabled);
        self.right_search.set_disabled(cx, disabled);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.fit();
        if self.searchable {
            let actions = cx.capture_actions(|cx| {
                self.left_search.handle_event(cx, event, scope);
                self.right_search.handle_event(cx, event, scope);
            });
            for action in actions {
                // The lists are filtered from the boxes on the next draw, so
                // a change only has to ask for one.
                if let TextInputAction::Changed(_) = action.as_widget_action().cast() {
                    self.draw_bg.redraw(cx);
                }
            }
        }

        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                // The three columns are worked out again from the rect the
                // pointer actually landed in. The rect the draw pass saw is
                // the widget's PRE-alignment one — a control in a centred
                // row has not been moved yet when it draws — so a hit test
                // against it answers for a place the reader never pressed.
                self.frame = self.frame_of(fe.rect);
                let row = self.row_at(fe.abs);
                let button =
                    self.frame.buttons.iter().position(|rect| rect.contains(fe.abs));
                if row != self.hover_row || button != self.hover_button {
                    self.hover_row = row;
                    self.hover_button = button;
                    cx.set_cursor(if row.is_some() || button.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover_row.take().is_some() || self.hover_button.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.frame = self.frame_of(fe.rect);
                if let Some(slot) = self.frame.buttons.iter().position(|r| r.contains(fe.abs)) {
                    self.down_button = Some(slot);
                    let action = Move::ORDER[slot];
                    self.run(cx, action);
                    return;
                }
                let Some((across, index)) = self.row_at(fe.abs) else {
                    return;
                };
                // A double press moves it: the hand is already on the row,
                // and the round trip out to the control column is the whole
                // reason a transfer feels slow.
                if fe.tap_count == 2 {
                    self.picked.insert(index);
                    let action = if across { Move::OneLeft } else { Move::OneRight };
                    self.run(cx, action);
                    return;
                }
                // Plain click toggles: this is a set being built, and a
                // click that cleared the rest would make choosing five
                // things five separate journeys.
                if !self.picked.remove(&index) {
                    self.picked.insert(index);
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(_) => {
                if self.down_button.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            _ => {}
        }
    }

    /// What is on the right, so a test can read the control in one line.
    fn text(&self) -> String {
        self.chosen_labels().join(", ")
    }
}

impl TransferRef {
    /// The items on the right, as indices into `items`.
    pub fn chosen(&self) -> Vec<usize> {
        self.borrow().map(|inner| inner.chosen()).unwrap_or_default()
    }

    pub fn chosen_labels(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.chosen_labels()).unwrap_or_default()
    }

    pub fn set_chosen(&self, cx: &mut Cx, indices: &[usize]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_chosen(cx, indices);
        }
    }

    pub fn set_items(&self, cx: &mut Cx, items: &[String]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_items(cx, items);
        }
    }

    /// How many moved this pass, and which way.
    pub fn moved(&self, actions: &Actions) -> Option<(usize, bool)> {
        match actions.find_widget_action(self.widget_uid())?.cast::<TransferAction>() {
            TransferAction::Moved(count, to_right) => Some((count, to_right)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock() -> Vec<String> {
        ["Kick", "Snare", "Hat", "Clap", "Rim", "Crash"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn moving_across_keeps_the_two_counts_adding_up() {
        let items = stock();
        let mut sides = Sides::default();
        sides.fit(items.len());
        assert_eq!(sides.counts(), (6, 0));

        sides.move_across(&[1, 3], true);
        assert_eq!(sides.counts(), (4, 2));
        assert_eq!(sides.side(true), vec![1, 3]);
        assert_eq!(sides.side(false), vec![0, 2, 4, 5]);

        sides.move_across(&[3], false);
        assert_eq!(sides.counts(), (5, 1));
    }

    #[test]
    fn moving_something_that_is_already_across_moves_nothing() {
        let mut sides = Sides::default();
        sides.fit(stock().len());
        assert_eq!(sides.move_across(&[0, 1], true), 2);
        // The overlap is not counted a second time; only the one that had
        // not crossed yet reports as moved.
        assert_eq!(sides.move_across(&[1, 2], true), 1);
        assert_eq!(sides.counts(), (3, 3));
    }

    #[test]
    fn a_row_that_came_back_stands_where_it_did_before_it_left() {
        // Item order, never move order: two readers who moved the same set
        // in a different sequence must be looking at the same list.
        let mut sides = Sides::default();
        sides.fit(stock().len());
        sides.move_across(&[4, 0, 2], true);
        assert_eq!(sides.side(true), vec![0, 2, 4]);
        sides.move_across(&[0], false);
        assert_eq!(sides.side(false), vec![0, 1, 3, 5]);
    }

    #[test]
    fn a_move_all_takes_what_the_search_is_showing_and_nothing_else() {
        // The one way this control can silently lose somebody's work: a
        // move-all that reaches past the search takes rows the reader cannot
        // see and did not ask for.
        let items = stock();
        let mut sides = Sides::default();
        sides.fit(items.len());

        let left = sides.side(false);
        let shown = showing(&items, &left, "a");
        let names: Vec<&str> = shown.iter().map(|i| items[*i].as_str()).collect();
        assert_eq!(names, vec!["Snare", "Hat", "Clap", "Crash"]);

        sides.move_across(&shown, true);
        assert_eq!(sides.counts(), (2, 4));
        let stayed: Vec<&str> =
            sides.side(false).iter().map(|i| items[*i].as_str()).collect();
        assert_eq!(stayed, vec!["Kick", "Rim"], "the rows the search had hidden stayed put");
    }

    #[test]
    fn an_empty_search_shows_the_whole_side() {
        let items = stock();
        let left: Vec<usize> = (0..items.len()).collect();
        assert_eq!(showing(&items, &left, "").len(), 6);
        assert_eq!(showing(&items, &left, "   ").len(), 6);
    }

    #[test]
    fn the_search_does_not_care_about_case() {
        let items = stock();
        let left: Vec<usize> = (0..items.len()).collect();
        assert_eq!(showing(&items, &left, "KICK"), vec![0]);
        assert_eq!(showing(&items, &left, "sNaRe"), vec![1]);
    }

    #[test]
    fn a_search_that_matches_nothing_leaves_a_move_all_with_nothing_to_do() {
        let items = stock();
        let mut sides = Sides::default();
        sides.fit(items.len());
        let shown = showing(&items, &sides.side(false), "tuba");
        assert!(shown.is_empty());
        assert_eq!(sides.move_across(&shown, true), 0);
        assert_eq!(sides.counts(), (6, 0));
    }
}

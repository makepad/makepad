//! A tree in a popover with boxes that cascade, the chosen leaves standing
//! as chips on the face.
//!
//! For picking a SET out of a hierarchy, where the shape of the hierarchy is
//! the reason the set makes sense.
//!
//! One of three ways to choose out of a set that will not fit in one list;
//! `column_picker` and `transfer` are the other two. `picker_parts` holds
//! what they share, and says what all three deliberately leave out.

use crate::{
    badge::measure,
    event::TouchState,
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{place_overlay, PlaceRequest, Placement, Side},
    picker_parts::{elide, text_y, DrawPickerPanel, DrawPickerRow, Forest},
    widget::*,
};
use std::collections::HashSet;

/// The mark on a face that opens a panel. Checked against the default face
/// first, as every glyph the pickers name was: most of the pictorial ranges
/// render as an empty box here, and nothing says when one has.
const CARET_MARK: &str = "\u{25BC}";

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The surfaces only this control draws; the panel and the row are in
    // `picker_parts`. As there, a value with a Rust field behind it is
    // written PLAIN and a value the shader alone owns is a uniform, so the
    // fields keep their own instance slots.
    set_type_default() do #(DrawPickerFace::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: 0.0
        open: 0.0
        color: uniform(theme.color_inset)
        color_hover: uniform(theme.color_inset_hover)
        color_open: uniform(theme.color_inset_focus)
        border_color: uniform(theme.color_bevel)
        border_color_open: uniform(theme.color_primary)
        /** the outline's thickness in pixels 0..4 step 0.5 */
        border_size: uniform(1.0)
        /** corner rounding radius 0..16 step 0.5 */
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
            sdf.fill_keep(self.color.mix(self.color_hover, self.hover).mix(self.color_open, self.open))
            sdf.stroke(self.border_color.mix(self.border_color_open, self.open), self.border_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawPickerTick::script_shader(vm)){
        ..mod.draw.DrawQuad
        on: 0.0
        mixed: 0.0
        color: uniform(#00000000)
        color_set: uniform(theme.color_primary)
        color_mark: uniform(theme.color_on_primary)
        border_color: uniform(theme.color_outline)
        /** the empty box's stroke 0..3 step 0.5 */
        border_size: uniform(1.0)
        /** box corner radius 0..8 step 0.5 */
        radius: uniform(3.0)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let s = min(self.rect_size.x, self.rect_size.y)
            let x0 = (self.rect_size.x - s) * 0.5
            let y0 = (self.rect_size.y - s) * 0.5
            let lit = max(self.on, self.mixed)
            sdf.box(
                x0 + self.border_size
                y0 + self.border_size
                s - self.border_size * 2.0
                s - self.border_size * 2.0
                self.radius
            )
            sdf.fill_keep(self.color.mix(self.color_set, lit))
            sdf.stroke(self.border_color.mix(self.color_set, lit), self.border_size)
            // On is a dot and mixed is a dash: both are shapes this renderer
            // paints every time, where a tick drawn as a path is the mark
            // that goes missing.
            if self.on > 0.5 {
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, s * 0.17)
                sdf.fill(self.color_mark)
            }
            if self.mixed > 0.5 {
                sdf.rect(x0 + s * 0.27, self.rect_size.y * 0.5 - s * 0.07, s * 0.46, s * 0.14)
                sdf.fill(self.color_mark)
            }
            return sdf.result
        }
    }

    set_type_default() do #(DrawPickerChip::script_shader(vm)){
        ..mod.draw.DrawQuad
        hot: 0.0
        color: uniform(theme.color_surface_container_high)
        color_hot: uniform(theme.color_error_container)
        border_color: uniform(theme.color_outline_variant)
        /** the outline's thickness in pixels 0..3 step 0.5 */
        border_size: uniform(1.0)
        /** corner rounding radius 0..16 step 0.5 */
        radius: uniform(6.0)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size * 0.5
                self.border_size * 0.5
                self.rect_size.x - self.border_size
                self.rect_size.y - self.border_size
                self.radius
            )
            sdf.fill_keep(self.color.mix(self.color_hot, self.hot))
            sdf.stroke(self.border_color, self.border_size)
            return sdf.result
        }
    }

    mod.widgets.TreeSelectBase = #(TreeSelect::register_widget(vm))

    /** A tree in a popover with boxes that cascade to the leaves, the chosen
     * leaves standing as chips on the face. */
    mod.widgets.TreeSelect = set_type_default() do mod.widgets.TreeSelectBase{
        width: 260.
        // A stated height, not Fit: the face lays its chips out from Rust
        // against the box it was given, and a Fit face has no box yet.
        height: 30.
        padding: Inset{left: 6. right: 6. top: 4. bottom: 4.}

        /** the tree as indented lines: a tab, or every two spaces, is one level */
        outline: []
        /** what the face says while nothing is ticked */
        placeholder: "Nothing chosen"
        /** how many chips the face shows before it counts the rest 1..8 step 1 */
        chip_limit: 3
        /** how wide the panel is, in pixels 140..420 step 1 */
        panel_width: 240.
        /** how many rows the panel shows; the rest are not drawn 3..40 step 1 */
        row_budget: 12
        /** how tall one panel row is 16..40 step 1 */
        row_height: 22.
        /** how far one level is set in from its parent 8..32 step 1 */
        indent: 14.
        /** the tick box 10..24 step 1 */
        tick_size: 14.
        /** room between two things in a row, and inside a chip 2..16 step 1 */
        gap: 6.

        draw_panel +: {
            color: theme.color_surface_container_low
            border_color: theme.color_outline_variant
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_summary +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_chip_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_caret +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: 8., line_spacing: 1.0}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerFace {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    open: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerTick {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    on: f32,
    #[live]
    mixed: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerChip {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hot: f32,
}

/// What one box in the tree says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tick {
    Off,
    /// Some of the leaves under this branch are ticked and some are not.
    Mixed,
    On,
}

/// What a node's box shows.
///
/// A branch is **derived, never stored**: its state is read back out of its
/// leaves every time it is asked for. That is the whole of why a parent can
/// never sit there disagreeing with its children, and why [`cascade`] writes
/// to the leaves rather than to the branch it was handed.
fn tick_of(forest: &Forest, ticked: &HashSet<LiveId>, node: usize) -> Tick {
    let Some(entry) = forest.node(node) else {
        return Tick::Off;
    };
    if entry.children.is_empty() {
        return if ticked.contains(&entry.id) { Tick::On } else { Tick::Off };
    }
    let mut any_on = false;
    let mut any_off = false;
    for child in &entry.children {
        match tick_of(forest, ticked, *child) {
            Tick::On => any_on = true,
            Tick::Off => any_off = true,
            Tick::Mixed => {
                any_on = true;
                any_off = true;
            }
        }
    }
    match (any_on, any_off) {
        (true, false) => Tick::On,
        (false, true) => Tick::Off,
        _ => Tick::Mixed,
    }
}

/// Tick or untick a node and everything under it.
///
/// Only leaves are written. Storing the branch as well would give the set
/// two answers to the same question the moment one child changed, and the
/// stale one is the one a naive reader finds first.
fn cascade(forest: &Forest, ticked: &mut HashSet<LiveId>, node: usize, on: bool) {
    let Some(entry) = forest.node(node) else {
        return;
    };
    if entry.children.is_empty() {
        if on {
            ticked.insert(entry.id);
        } else {
            ticked.remove(&entry.id);
        }
        return;
    }
    let children = entry.children.clone();
    for child in children {
        cascade(forest, ticked, child, on);
    }
}

/// The ticked leaves, in document order. Branches are derived and so are
/// never in this list.
fn ticked_leaves(forest: &Forest, ticked: &HashSet<LiveId>) -> Vec<usize> {
    (0..forest.nodes.len())
        .filter(|index| {
            forest.is_leaf(*index)
                && forest.id(*index).is_some_and(|id| ticked.contains(&id))
        })
        .collect()
}

/// What a tree select reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TreeSelectAction {
    /// A box was set; the cascade has already run, so read `chosen` for the
    /// whole answer.
    Toggled(LiveId, bool),
    /// A chip was pressed off the face.
    Removed(LiveId),
    #[default]
    None,
}

/// Popover geometry, in layout points.
const PANEL_PAD: f64 = 6.0;
const PANEL_GAP: f64 = 4.0;
/// Window edge kept free when the panel has to be pushed back on.
const PANEL_EDGE: f64 = 6.0;

#[derive(Script, Widget)]
pub struct TreeSelect {
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
    draw_bg: DrawPickerFace,
    #[live]
    draw_chip: DrawPickerChip,
    #[live]
    draw_panel: DrawPickerPanel,
    #[live]
    draw_row: DrawPickerRow,
    #[live]
    draw_tick: DrawPickerTick,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_summary: DrawText,
    #[live]
    pub draw_chip_text: DrawText,
    #[live]
    pub draw_caret: DrawText,

    #[live]
    pub outline: Vec<String>,
    #[live]
    pub placeholder: String,
    #[live(3usize)]
    pub chip_limit: usize,
    #[live(240.0)]
    pub panel_width: f64,
    #[live(12usize)]
    pub row_budget: usize,
    #[live(22.0)]
    pub row_height: f64,
    #[live(14.0)]
    pub indent: f64,
    #[live(14.0)]
    pub tick_size: f64,
    #[live(6.0)]
    pub gap: f64,

    #[rust]
    forest: Forest,
    #[rust]
    seeded_from: Vec<String>,
    /// The ticked LEAVES. Branches are worked out from these every draw.
    #[rust]
    ticked: HashSet<LiveId>,
    #[rust]
    open: bool,
    #[rust]
    hover_row: Option<usize>,
    /// The chip under the pointer, so it can say it is a remove target
    /// before it is pressed.
    #[rust]
    hover_chip: Option<usize>,
    /// The row the arrow keys are standing on while the panel is open, and
    /// whether a key has actually put it anywhere. Without the second half
    /// a panel opened with the mouse would draw a ring on its first row,
    /// claiming a keyboard position nobody asked for.
    #[rust]
    key_row: usize,
    #[rust]
    key_used: bool,
    /// Where each chip was drawn and which leaf it stands for.
    #[rust]
    chips: Vec<(Rect, LiveId)>,
    /// The face's FINAL rect and the panel's, both captured on the event
    /// side. Mid-draw the face only knows its pre-alignment position, and
    /// the flip decision needs the place the reader actually pressed.
    #[rust]
    face_rect: Rect,
    #[rust]
    panel_rect: Rect,
    /// The window this drew into last, for the edge flip. The event side has
    /// no `Cx2d` to ask, so the draw side leaves it here.
    #[rust]
    pass_size: DVec2,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for TreeSelect {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
}

impl TreeSelect {
    fn seed(&mut self) {
        if self.seeded_from == self.outline {
            return;
        }
        self.seeded_from = self.outline.clone();
        let forest = Forest::from_outline(&self.outline);
        // Ticks survive a live edit only where the same line is still there:
        // the ids are line numbers, so a rewritten outline would otherwise
        // hand its ticks to whatever moved into those lines.
        self.ticked.retain(|id| forest.index_of(*id).is_some());
        self.forest = forest;
        self.hover_row = None;
        self.key_row = 0;
        self.key_used = false;
    }

    /// How many rows the panel draws: the whole tree, capped by the budget,
    /// because the panel does not scroll.
    fn rows_drawn(&self) -> usize {
        self.forest.nodes.len().min(self.row_budget)
    }

    fn panel_size(&self) -> DVec2 {
        let rows = self.rows_drawn().max(1) as f64;
        dvec2(self.panel_width.max(60.0), rows * self.row_height + PANEL_PAD * 2.0)
    }

    /// Offset from the face's top-left to the panel's: below with the left
    /// edges aligned, flipped above when there is room up there and none
    /// down, and pulled back inboard of the window edge.
    fn panel_offset(&self, face: Rect) -> DVec2 {
        let size = self.panel_size();
        let placed = place_overlay(&PlaceRequest {
            anchor: face,
            size,
            bounds: Rect {
                pos: dvec2(PANEL_EDGE, PANEL_EDGE),
                size: self.pass_size - dvec2(PANEL_EDGE * 2.0, PANEL_EDGE * 2.0),
            },
            gap: PANEL_GAP,
            placement: Placement::BOTTOM_START,
            match_anchor_width: true,
        });
        // Only the SIDE is read off the placement, not its rect: the helper
        // shortens a popup to the room it has, and a panel shortened from
        // the bottom would drop the last rows with nothing to say so. A
        // panel that overruns the window edge is the lesser fault.
        let y = match placed.side {
            Side::Top => -(size.y + PANEL_GAP),
            _ => face.size.y + PANEL_GAP,
        };
        dvec2(placed.rect.pos.x - face.pos.x, y)
    }

    /// Which panel row a window-absolute point lands on. `None` for the
    /// panel's own padding above the first row and below the last.
    fn row_at(&self, abs: DVec2) -> Option<usize> {
        if !self.panel_rect.contains(abs) {
            return None;
        }
        let row = ((abs.y - self.panel_rect.pos.y - PANEL_PAD) / self.row_height).floor();
        if row < 0.0 {
            return None;
        }
        let row = row as usize;
        if row < self.rows_drawn() {
            Some(row)
        } else {
            None
        }
    }

    fn chip_at(&self, abs: DVec2) -> Option<usize> {
        self.chips.iter().position(|(rect, _)| rect.contains(abs))
    }

    /// The ticked leaves, in document order.
    pub fn chosen(&self) -> Vec<LiveId> {
        ticked_leaves(&self.forest, &self.ticked)
            .into_iter()
            .filter_map(|node| self.forest.id(node))
            .collect()
    }

    pub fn chosen_labels(&self) -> Vec<String> {
        ticked_leaves(&self.forest, &self.ticked)
            .into_iter()
            .map(|node| self.forest.label(node).to_string())
            .collect()
    }

    pub fn set_chosen(&mut self, cx: &mut Cx, ids: &[LiveId]) {
        self.seed();
        // Only leaves: a branch id in the incoming set would be stored
        // alongside its own leaves and then contradict them.
        let ticked: HashSet<LiveId> = ids
            .iter()
            .copied()
            .filter(|id| self.forest.index_of(*id).is_some_and(|at| self.forest.is_leaf(at)))
            .collect();
        self.ticked = ticked;
        self.redraw_all(cx);
    }

    fn redraw_all(&mut self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
    }

    /// Opening takes the pointer for the whole widget tree and closing hands
    /// it back. Without it the panel is a picture: the widgets it floats
    /// over were walked first and have acted on the press already.
    ///
    /// An empty tree refuses to open. A panel that answers nothing is a hole
    /// in the screen, and one holding the pointer lock is a hole that
    /// swallows the rest of the window with it.
    fn set_open(&mut self, cx: &mut Cx, open: bool) {
        let open = open && !self.forest.is_empty();
        if self.open == open {
            return;
        }
        self.open = open;
        self.hover_row = None;
        self.key_used = false;
        if open {
            cx.sweep_lock(self.draw_bg.area());
        } else {
            cx.sweep_unlock(self.draw_bg.area());
        }
        self.draw_bg.open = if open { 1.0 } else { 0.0 };
        self.redraw_all(cx);
    }

    fn toggle_row(&mut self, cx: &mut Cx, row: usize) {
        let Some(id) = self.forest.id(row) else {
            return;
        };
        // Mixed resolves to on, the way a select-all box does: the useful
        // next step from "some of them" is "all of them", not "none".
        let on = tick_of(&self.forest, &self.ticked, row) != Tick::On;
        cascade(&self.forest, &mut self.ticked, row, on);
        self.key_row = row;
        let uid = self.uid;
        self.redraw_all(cx);
        cx.widget_action(uid, TreeSelectAction::Toggled(id, on));
    }

    /// A press while the panel is open. True when it landed inside the
    /// panel, meaning the caller must mark the event handled: the sweep lock
    /// turned away everything walked BEFORE this widget, and this stops
    /// anything walked after it.
    fn press_at(&mut self, cx: &mut Cx, abs: DVec2, primary: bool) -> bool {
        if self.panel_rect.contains(abs) {
            if primary {
                if let Some(row) = self.row_at(abs) {
                    self.toggle_row(cx, row);
                }
            }
            return true;
        }
        if !self.face_rect.contains(abs) {
            self.set_open(cx, false);
        }
        false
    }

    fn hover_at(&mut self, cx: &mut Cx, abs: DVec2) {
        let row = self.row_at(abs);
        if row != self.hover_row {
            self.hover_row = row;
            self.redraw_all(cx);
        }
    }
}

impl Widget for TreeSelect {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.seed();

        self.draw_bg.begin(cx, walk, self.layout);
        let inner = cx.turtle().inner_rect();
        self.chips.clear();

        let chip_font = self.draw_chip_text.text_style.font_size as f64;
        let caret_font = self.draw_caret.text_style.font_size as f64;
        let caret_w = measure(&self.draw_caret, cx, CARET_MARK);
        let chosen = ticked_leaves(&self.forest, &self.ticked);
        let room_x = inner.pos.x + inner.size.x - caret_w - self.gap;

        if chosen.is_empty() {
            let placeholder = self.placeholder.clone();
            let text = elide(&self.draw_summary, cx, &placeholder, room_x - inner.pos.x);
            let font = self.draw_summary.text_style.font_size as f64;
            let y = text_y(font, inner.pos.y, inner.size.y);
            self.draw_summary.draw_abs(cx, dvec2(inner.pos.x, y), &text);
        } else {
            let chip_h = (inner.size.y - 4.0).max(12.0);
            let chip_y = inner.pos.y + (inner.size.y - chip_h) * 0.5;
            let mut x = inner.pos.x;
            let mut shown = 0;
            for node in chosen.iter().take(self.chip_limit) {
                let label = self.forest.label(*node).to_string();
                let label_w = measure(&self.draw_chip_text, cx, &label);
                let width = label_w + self.gap * 2.0;
                // A chip that would run under the caret is not drawn at all:
                // half a chip reads as a whole one with a short name.
                if x + width > room_x {
                    break;
                }
                let rect = Rect { pos: dvec2(x, chip_y), size: dvec2(width, chip_h) };
                let at = self.chips.len();
                self.draw_chip.hot = if self.hover_chip == Some(at) { 1.0 } else { 0.0 };
                self.draw_chip.draw_abs(cx, rect);
                self.draw_chip_text.draw_abs(
                    cx,
                    dvec2(x + self.gap, text_y(chip_font, chip_y, chip_h)),
                    &label,
                );
                if let Some(id) = self.forest.id(*node) {
                    self.chips.push((rect, id));
                }
                x += width + 4.0;
                shown += 1;
            }
            let rest = chosen.len() - shown;
            if rest > 0 {
                let text = format!("+{rest}");
                let font = self.draw_summary.text_style.font_size as f64;
                // Pulled back off the caret rather than allowed to run under
                // it: the count is the only thing saying the face is not the
                // whole answer, so it is the one run that must stay legible.
                let w = measure(&self.draw_summary, cx, &text);
                let at = x.min(room_x - w).max(inner.pos.x);
                self.draw_summary.draw_abs(
                    cx,
                    dvec2(at, text_y(font, inner.pos.y, inner.size.y)),
                    &text,
                );
            }
        }

        self.draw_caret.draw_abs(
            cx,
            dvec2(room_x + self.gap, text_y(caret_font, inner.pos.y, inner.size.y)),
            CARET_MARK,
        );
        self.draw_bg.end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::DropDown, Inset::default());

        if self.open {
            // Everything the overlay pass needs is worked out here, before
            // the draw list is borrowed out of `self`.
            self.pass_size = cx.current_pass_size();
            let drawn = self.draw_bg.area().rect(cx);
            let anchor = Rect {
                pos: if self.face_rect.size.y > 0.0 { self.face_rect.pos } else { drawn.pos },
                size: drawn.size,
            };
            let panel_size = self.panel_size();
            let offset = self.panel_offset(anchor);
            let rows = self.rows_drawn();
            let font = self.draw_text.text_style.font_size as f64;
            let mut ticks = Vec::with_capacity(rows);
            let mut labels = Vec::with_capacity(rows);
            for row in 0..rows {
                ticks.push(tick_of(&self.forest, &self.ticked, row));
                let depth = self.forest.node(row).map(|node| node.depth).unwrap_or(0);
                labels.push((depth, self.forest.label(row).to_string()));
            }
            let hover_row = self.hover_row;
            let key_row = if self.key_used { Some(self.key_row) } else { None };
            let row_h = self.row_height;
            let tick_size = self.tick_size;
            let indent = self.indent;
            let gap = self.gap;
            if let Some(draw_list) = self.draw_list.as_mut() {
                // The proven popup idiom: draw the panel as turtle content
                // at the overlay root, then SHIFT the whole list to hang off
                // the face. A `draw_abs` into a bare overlay list renders
                // nothing at all.
                draw_list.begin_overlay_reuse(cx);
                let size = cx.current_pass_size();
                cx.begin_root_turtle(size, Layout::flow_down());
                self.draw_panel.begin(
                    cx,
                    Walk::fixed(panel_size.x, panel_size.y),
                    Layout::default(),
                );
                let panel = cx.turtle().rect();
                for row in 0..rows {
                    let y = panel.pos.y + PANEL_PAD + row as f64 * row_h;
                    let rect = Rect {
                        pos: dvec2(panel.pos.x + PANEL_PAD, y),
                        size: dvec2((panel.size.x - PANEL_PAD * 2.0).max(1.0), row_h),
                    };
                    // A panel row is never "chosen": what is chosen here is
                    // said by the box, not by the row's fill. The ring is
                    // only where the arrow keys are standing.
                    self.draw_row.hover = if hover_row == Some(row) { 1.0 } else { 0.0 };
                    self.draw_row.chosen = 0.0;
                    self.draw_row.keyed = if key_row == Some(row) { 1.0 } else { 0.0 };
                    self.draw_row.draw_abs(cx, rect);

                    let (depth, label) = &labels[row];
                    let x = rect.pos.x + gap + *depth as f64 * indent;
                    self.draw_tick.on = if ticks[row] == Tick::On { 1.0 } else { 0.0 };
                    self.draw_tick.mixed = if ticks[row] == Tick::Mixed { 1.0 } else { 0.0 };
                    self.draw_tick.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y + (row_h - tick_size) * 0.5),
                            size: dvec2(tick_size, tick_size),
                        },
                    );
                    let text_x = x + tick_size + gap;
                    let room = rect.pos.x + rect.size.x - text_x - gap;
                    let cut = elide(&self.draw_text, cx, label, room);
                    self.draw_text.draw_abs(cx, dvec2(text_x, text_y(font, y, row_h)), &cut);
                }
                self.draw_panel.end(cx);
                cx.end_pass_sized_turtle_with_shift(self.draw_bg.area(), offset);
                draw_list.end(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.open {
            // Raw events rather than `hits`: the panel has no area of its
            // own, and the sweep lock taken in `set_open` is what keeps
            // every other widget out of these same presses.
            let face = self.draw_bg.area().rect(cx);
            self.face_rect = face;
            self.panel_rect =
                Rect { pos: face.pos + self.panel_offset(face), size: self.panel_size() };
            match event {
                Event::MouseDown(me) => {
                    if self.press_at(cx, me.abs, me.button.is_primary()) {
                        me.handled.set(self.draw_bg.area());
                    }
                }
                // Touch never becomes a MouseDown: `hits` synthesises a
                // FingerDown for the face, but the panel is not an area, so
                // without this the panel opens on a phone and then answers
                // nothing at all.
                Event::TouchUpdate(te) => {
                    if let Some(touch) = te.touches.first() {
                        match touch.state {
                            TouchState::Start => {
                                if self.press_at(cx, touch.abs, true) {
                                    touch.handled.set(self.draw_bg.area());
                                }
                            }
                            TouchState::Move => self.hover_at(cx, touch.abs),
                            _ => {}
                        }
                    }
                }
                Event::MouseMove(me) => self.hover_at(cx, me.abs),
                Event::KeyDown(ke) => {
                    let rows = self.rows_drawn();
                    match ke.key_code {
                        KeyCode::Escape | KeyCode::ReturnKey => self.set_open(cx, false),
                        // Without these the boxes are reachable by pointer
                        // only, which makes the whole control unusable from
                        // the keyboard.
                        KeyCode::ArrowUp => {
                            self.key_row = if self.key_used {
                                self.key_row.saturating_sub(1)
                            } else {
                                rows.saturating_sub(1)
                            };
                            self.key_used = true;
                            self.redraw_all(cx);
                        }
                        KeyCode::ArrowDown => {
                            if rows > 0 {
                                self.key_row = if self.key_used {
                                    (self.key_row + 1).min(rows - 1)
                                } else {
                                    0
                                };
                                self.key_used = true;
                                self.redraw_all(cx);
                            }
                        }
                        KeyCode::Space => {
                            if self.key_used && self.key_row < rows {
                                let row = self.key_row;
                                self.toggle_row(cx, row);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // Named as its own sweep area, or the lock this widget took would
        // turn the face's own hits away along with everyone else's.
        match event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.draw_bg.hover = 1.0;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.hover = 0.0;
                if self.hover_chip.take().is_some() {
                    self.redraw_all(cx);
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOver(fe) => {
                let chip = self.chip_at(fe.abs);
                if chip != self.hover_chip {
                    self.hover_chip = chip;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.face_rect = self.draw_bg.area().rect(cx);
                // A chip is a remove target, so a press on one takes that
                // leaf off rather than opening the panel. Without this the
                // chips would be decoration and every removal would be a
                // round trip through the tree.
                if let Some(at) = self.chip_at(fe.abs) {
                    let id = self.chips[at].1;
                    if let Some(node) = self.forest.index_of(id) {
                        cascade(&self.forest, &mut self.ticked, node, false);
                        let uid = self.uid;
                        self.hover_chip = None;
                        self.redraw_all(cx);
                        cx.widget_action(uid, TreeSelectAction::Removed(id));
                    }
                    return;
                }
                cx.set_key_focus(self.draw_bg.area());
                let open = self.open;
                self.set_open(cx, !open);
            }
            _ => {}
        }
    }

    /// What the face says, so a test can read the control in one line.
    fn text(&self) -> String {
        let labels = self.chosen_labels();
        if labels.is_empty() {
            self.placeholder.clone()
        } else {
            labels.join(", ")
        }
    }
}

impl TreeSelectRef {
    /// The ticked leaves, in document order.
    pub fn chosen(&self) -> Vec<LiveId> {
        self.borrow().map(|inner| inner.chosen()).unwrap_or_default()
    }

    pub fn chosen_labels(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.chosen_labels()).unwrap_or_default()
    }

    pub fn set_chosen(&self, cx: &mut Cx, ids: &[LiveId]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_chosen(cx, ids);
        }
    }

    /// The box set this pass, and what it was set to.
    pub fn toggled(&self, actions: &Actions) -> Option<(LiveId, bool)> {
        match actions.find_widget_action(self.widget_uid())?.cast::<TreeSelectAction>() {
            TreeSelectAction::Toggled(id, on) => Some((id, on)),
            _ => None,
        }
    }

    /// The chip taken off the face this pass.
    pub fn removed(&self, actions: &Actions) -> Option<LiveId> {
        match actions.find_widget_action(self.widget_uid())?.cast::<TreeSelectAction>() {
            TreeSelectAction::Removed(id) => Some(id),
            _ => None,
        }
    }

    /// True when the set changed this pass, however it changed.
    pub fn changed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast::<TreeSelectAction>()),
            Some(TreeSelectAction::Toggled(..)) | Some(TreeSelectAction::Removed(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker_parts::fixtures::places;

    #[test]
    fn ticking_a_branch_reaches_every_leaf_under_it() {
        let forest = places();
        let mut ticked = HashSet::new();
        let france = forest.nodes.iter().position(|node| node.label == "France").unwrap();
        cascade(&forest, &mut ticked, france, true);

        let names: Vec<&str> =
            ticked_leaves(&forest, &ticked).iter().map(|node| forest.label(*node)).collect();
        assert_eq!(names, vec!["Rennes", "Brest", "Colmar"]);
        // Only leaves are stored, so the branch cannot end up disagreeing
        // with its own children.
        assert_eq!(ticked.len(), 3);
        assert_eq!(tick_of(&forest, &ticked, france), Tick::On);
    }

    #[test]
    fn a_branch_whose_leaves_disagree_is_mixed_all_the_way_up() {
        let forest = places();
        let mut ticked = HashSet::new();
        let rennes = forest.nodes.iter().position(|node| node.label == "Rennes").unwrap();
        cascade(&forest, &mut ticked, rennes, true);

        let brittany = forest.nodes.iter().position(|node| node.label == "Brittany").unwrap();
        let france = forest.nodes.iter().position(|node| node.label == "France").unwrap();
        let europe = forest.nodes.iter().position(|node| node.label == "Europe").unwrap();
        let asia = forest.nodes.iter().position(|node| node.label == "Asia").unwrap();
        assert_eq!(tick_of(&forest, &ticked, brittany), Tick::Mixed);
        assert_eq!(tick_of(&forest, &ticked, france), Tick::Mixed);
        assert_eq!(tick_of(&forest, &ticked, europe), Tick::Mixed);
        assert_eq!(tick_of(&forest, &ticked, asia), Tick::Off, "a branch nobody touched");
    }

    #[test]
    fn unticking_a_branch_reaches_the_same_leaves_the_tick_did() {
        let forest = places();
        let mut ticked = HashSet::new();
        let europe = forest.nodes.iter().position(|node| node.label == "Europe").unwrap();
        cascade(&forest, &mut ticked, europe, true);
        assert_eq!(ticked.len(), 4);

        let france = forest.nodes.iter().position(|node| node.label == "France").unwrap();
        cascade(&forest, &mut ticked, france, false);
        let names: Vec<&str> =
            ticked_leaves(&forest, &ticked).iter().map(|node| forest.label(*node)).collect();
        assert_eq!(names, vec!["Vigo"], "Spain's leaf was not in the branch that was cleared");
    }
}

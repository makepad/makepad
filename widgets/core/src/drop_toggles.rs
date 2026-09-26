//! DropToggles — a compact chip that opens a POPOVER of independent toggles.
//!
//! A row of four filter chips eats a whole bar and still only says four
//! things. This says the same four in the width of one word: the chip
//! carries the name, the popover carries the switches, and the count sitting
//! on the chip ("FILTER 2") says how hard the list is being narrowed without
//! anything being opened. With nothing on, the chip is at rest; the moment
//! one item is on it lights in the accent, because a filter that is silently
//! in force is a filter that gets blamed on the data.
//!
//! It is a MULTI-select, so a click on a row toggles that row and nothing
//! else and the popover stays open — closing on every pick would turn "three
//! of four" into a four-click ritual. A click anywhere outside, or Escape,
//! closes it and changes nothing.
//!
//! The popover draws on its own overlay draw list (the `TipLayer` idiom)
//! from the chip's FINAL rect, so it floats over every panel and never
//! disturbs layout. It hangs under the chip with the left edges aligned, and
//! flips above / slides inboard when the window edge is nearer than the
//! panel is tall or wide. Every pick emits
//! [`DropTogglesAction::Toggled`]; hosts push their own state back in with
//! `set_active_mask`, which is why the on/off set is carried as a bit per
//! item rather than a Vec<bool> nobody wants to diff.
//!
//! While it is open the popover GRABS THE POINTER (`cx.sweep_lock`, the same
//! pairing every other popup in this crate uses). It has to: the panel floats
//! over widgets that were already walked this dispatch, so marking the press
//! handled by the time it reaches us is too late — the row under the panel
//! has fired already. The lock refuses every hit test that does not name the
//! chip's area, and is handed back the moment the popover closes.
//!
//! The on/off set is stored RAW and masked against the current row count on
//! every read. That is what makes the two setters agree: `set_active_mask`
//! before `set_labels` keeps the whole mask instead of dropping it on the
//! floor, and a bit with no row behind it is never counted, never lights the
//! chip, and comes back if the labels that own it come back.

use crate::{
    event::TouchState,
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{place_overlay, PlaceRequest, Placement, Side},
    widget::*,
};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum DropTogglesAction {
    /// One item was toggled: (index, now_on). The popover STAYS OPEN.
    Toggled(usize, bool),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawTogglesChipBase = #(DrawTogglesChip::script_component(vm))
    set_type_default() do #(DrawTogglesChip::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawTogglesRowBase = #(DrawTogglesRow::script_component(vm))
    set_type_default() do #(DrawTogglesRow::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawTogglesMarkBase = #(DrawTogglesMark::script_component(vm))
    set_type_default() do #(DrawTogglesMark::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawTogglesLabelBase = #(DrawTogglesLabel::script_component(vm))
    set_type_default() do #(DrawTogglesLabel::script_shader(vm)){
        ..mod.draw.DrawText
    }

    mod.widgets.DropTogglesBase = #(DropToggles::register_widget(vm))
    mod.widgets.DropToggles = set_type_default() do mod.widgets.DropTogglesBase{
        width: Fit
        height: 22
        padding: Inset{left: 7.0 right: 8.0 top: 0.0 bottom: 0.0}
        align: Align{x: 0.0, y: 0.5}

        // `lit` is the ONE instance here: it rides in the widget struct, so
        // every other prop has to be a uniform or the instance slots stop
        // lining up with the struct and `lit` reads a colour channel.
        draw_bg +: {
            lit: 0.0
            hover: uniform(0.0)
            open: uniform(0.0)
            color: uniform(#x272e38)
            color_hover: uniform(#x2f3842)
            border_color: uniform(#xffffff26)
            border_color_lit: uniform(#xff5c39)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, theme.corner_radius)
                // fill_KEEP: a plain fill wipes the shape and the stroke that
                // follows lands on nothing, which is a silent way to lose the
                // one mark that says a filter is in force.
                sdf.fill_keep(self.color.mix(self.color_hover, max(self.hover, self.open)))
                sdf.stroke(self.border_color.mix(self.border_color_lit, self.lit), 1.0)
                return sdf.result
            }
        }
        draw_icon +: {
            color: #xd6dee6
        }
        icon_walk: Walk{width: 10 height: Fit margin: Inset{right: 5.0}}
        draw_text +: {
            color: #xf4f7fa
            text_style: theme.font_bold{font_size: 9}
        }
        // The icon's lit ink. The rest ink is whatever the call site put in
        // `draw_icon.color`, captured on the first draw.
        icon_color_lit: #xff5c39
        // The popover panel + its rows.
        draw_panel +: {
            color: #x181c23
            border_color: #xffffff2e
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, theme.corner_radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_row +: {
            hover: 0.0
            color: uniform(theme.color_u_hidden)
            color_hover: uniform(#xffffff14)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 3.0)
                sdf.fill(self.color.mix(self.color_hover, self.hover))
                return sdf.result
            }
        }
        draw_mark +: {
            hover: 0.0
            active: 0.0
            color: uniform(theme.color_u_hidden)
            color_active: uniform(#xff5c39)
            border_color: uniform(#xffffff26)
            border_color_hover: uniform(#xffffff70)
            border_color_active: uniform(#xff5c39)
            // The tick ink is the panel fill, so a lit box reads as a hole
            // punched in the accent rather than a sticker laid on it.
            tick_color: uniform(#x181c23)
            tick_color_off: uniform(theme.color_u_hidden)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let sz = self.rect_size.x
                // Deliberately a rounded SQUARE. `sdf.box` doubles the radius,
                // so 1.5 here is 3px of corner — a circle would say "pick
                // one" to an operator, and this is a multi-select.
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 1.5)
                sdf.fill_keep(self.color.mix(self.color_active, self.active))
                sdf.stroke(
                    self.border_color
                        .mix(self.border_color_hover, self.hover)
                        .mix(self.border_color_active, self.active),
                    1.0
                )
                sdf.move_to(sz * 0.26, sz * 0.52)
                sdf.line_to(sz * 0.44, sz * 0.72)
                sdf.line_to(sz * 0.76, sz * 0.30)
                sdf.stroke(self.tick_color_off.mix(self.tick_color, self.active), 1.5)
                return sdf.result
            }
        }
        draw_label +: {
            hover: 0.0
            active: 0.0
            color: #x8e9aa7
            color_hover: uniform(#xd6dee6)
            color_active: uniform(#xf4f7fa)
            text_style: theme.font_bold{font_size: 9}
            get_color: fn() {
                return self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_active, self.active)
            }
        }
    }
}

/// Popover geometry (layout points).
const PANEL_MIN_W: f64 = 118.0;
const PANEL_MAX_W: f64 = 280.0;
const PANEL_PAD_X: f64 = 8.0;
const PANEL_PAD_Y: f64 = 6.0;
const PANEL_GAP: f64 = 4.0;
const ROW_H: f64 = 22.0;
/// Rows sit inboard of the panel edge; the mark sits inboard of the row.
const ROW_INSET: f64 = 4.0;
const MARK: f64 = 13.0;
const MARK_INSET: f64 = 4.0;
const MARK_GAP: f64 = 8.0;
/// Keep this much window edge free when the panel has to be pushed back on.
const EDGE: f64 = 6.0;
/// A u32 carries the on/off set, so 32 items is the ceiling by construction.
/// Everything past it is DROPPED — not drawn, not hit-tested, not toggled.
/// `1u32 << 32` panics in debug and silently wraps to `1 << 0` in release, so
/// a 33rd row would toggle the first one; the ceiling has to be enforced, not
/// merely written down.
const MAX_ITEMS: usize = 32;

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTogglesChip {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    lit: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTogglesRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTogglesMark {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    active: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTogglesLabel {
    #[deref]
    draw_super: DrawText,
    #[live]
    hover: f32,
    #[live]
    active: f32,
}

#[derive(Script, Widget)]
pub struct DropToggles {
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
    draw_bg: DrawTogglesChip,
    #[live]
    draw_icon: DrawSvg,
    #[live]
    icon_walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_panel: DrawQuad,
    #[live]
    draw_row: DrawTogglesRow,
    #[live]
    draw_mark: DrawTogglesMark,
    #[live]
    draw_label: DrawTogglesLabel,

    /// The chip's word. May be empty for an icon-only chip.
    #[live]
    pub text: String,
    /// One row per label, in order. Index in the action is an index here.
    /// Only the first [`MAX_ITEMS`] are ever drawn or hit-tested.
    #[live]
    pub labels: Vec<String>,
    /// Icon ink while anything is on.
    #[live]
    pub icon_color_lit: Vec4f,

    /// Bit i = item i is on, held RAW: bits past the current row count are
    /// kept but dormant, so a host may push its saved mask in before it has
    /// pushed the labels and lose nothing. Every read goes through
    /// `visible_mask`, which is what stops a dormant bit from being counted
    /// or from lighting the chip.
    #[rust]
    active: u32,
    #[rust]
    open: bool,
    #[rust]
    hover_row: Option<usize>,
    /// The window this drew into last, for the edge flip/clamp. The event
    /// side has no `Cx2d` to ask, so the draw side leaves it here.
    #[rust]
    pass_size: DVec2,
    /// The widest label as actually LAID OUT, left here by the draw pass.
    /// Zero until an open panel has been drawn once, and then the size is
    /// measured rather than guessed. Here for the same reason `pass_size`
    /// is: text cannot be measured without a `Cx2d`, and the panel's offset
    /// has to be worked out on the event side.
    #[rust]
    label_w: f64,
    /// The chip's FINAL rect, captured on the event side. Mid-draw the chip
    /// only knows its pre-alignment position — a chip in a right-aligned row
    /// has not been moved yet — and the flip/clamp decision needs the place
    /// the operator actually clicked. Sizes are honest at draw time; only
    /// positions lie.
    #[rust]
    chip_rect: Rect,
    /// The popover rect of the last draw (event-side hit tests use it).
    #[rust]
    panel_rect: Rect,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for DropToggles {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    /// A live re-apply keeps the instance and every `#[rust]` field, so state
    /// that indexes into `labels` outlives the labels themselves. A shorter
    /// list would leave the hover on a row that is no longer there. The
    /// active mask needs no repair here — it is masked on read against the
    /// current row count, so shrinking the list makes the high bits dormant
    /// rather than stale, and growing it back brings them home.
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.hover_row.is_some_and(|row| row >= self.item_count()) {
            self.hover_row = None;
        }
    }
}

fn bit(mask: u32, index: usize) -> bool {
    index < MAX_ITEMS && mask & (1u32 << index) != 0
}

fn mask_for_len(len: usize) -> u32 {
    if len >= MAX_ITEMS {
        u32::MAX
    } else {
        (1u32 << len) - 1
    }
}

/// What to do with a raw press while the popover is open.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PanelPress {
    /// In the panel, and nothing else holds the mouse: work the row under
    /// it, and the press is spent.
    Row,
    /// In the panel, but a control that is dragged continuously holds the
    /// mouse. No row is worked — that control keeps the pointer until the
    /// release — and the press is spent all the same, since the panel is
    /// drawn over whatever is under it.
    Swallow,
    /// On the chip: the chip's own hit deals with it, this must not.
    Chip,
    /// Outside both: the popover closes.
    Dismiss,
}

/// Which of those a press is.
///
/// `mouse_held_outside` is [`CxFingers::is_mouse_held_outside`] asked with
/// this widget's chip area. It is the app-wide rule: a control that is
/// dragged continuously locks the pointer on its press, and every other
/// gesture that would start from the same press stands down until the
/// release. The chip's own press — the one that opened the popover, still
/// held — is `mine`, so press-the-chip-drag-onto-a-row still works.
/// Dismissing is not such a gesture and is left alone by it: a press that
/// lands nowhere near the popover closes the popover whatever else is going
/// on.
fn panel_press(inside_panel: bool, inside_chip: bool, mouse_held_outside: bool) -> PanelPress {
    if inside_panel {
        if mouse_held_outside {
            PanelPress::Swallow
        } else {
            PanelPress::Row
        }
    } else if inside_chip {
        PanelPress::Chip
    } else {
        PanelPress::Dismiss
    }
}

fn row_layout() -> Layout {
    Layout {
        padding: Inset {
            left: MARK_INSET + MARK + MARK_GAP,
            right: ROW_INSET,
            top: 0.0,
            bottom: 0.0,
        },
        align: Align { x: 0.0, y: 0.5 },
        ..Layout::flow_right()
    }
}

impl DropToggles {
    /// Rows that actually exist. `labels` is a live field a script can set to
    /// anything, so the ceiling is applied HERE rather than trusted upstream:
    /// every loop, hit test and bit shift in this file counts to this.
    fn item_count(&self) -> usize {
        self.labels.len().min(MAX_ITEMS)
    }

    /// The stored mask with the dormant bits — the ones no row stands behind —
    /// cut away. Everything public reads through this.
    fn visible_mask(&self) -> u32 {
        self.active & mask_for_len(self.item_count())
    }

    /// Replace the item labels. Clears nothing: bits belonging to rows that
    /// are gone go dormant rather than being destroyed, so a host may push a
    /// mask and its labels in EITHER order and end up in the same place.
    /// Anything past [`MAX_ITEMS`] is dropped on the way in.
    pub fn set_labels(&mut self, cx: &mut Cx, labels: &[String]) {
        self.labels = labels.iter().take(MAX_ITEMS).cloned().collect();
        if self.hover_row.is_some_and(|row| row >= self.labels.len()) {
            self.hover_row = None;
        }
        if self.labels.is_empty() {
            self.set_open(cx, false);
        }
        self.redraw_all(cx);
    }

    /// Set one item. Indices from [`MAX_ITEMS`] up are refused — there is no
    /// bit for them. An index that is merely past the CURRENT last label is
    /// accepted and parked: see `active`.
    pub fn set_active(&mut self, cx: &mut Cx, index: usize, on: bool) {
        if index >= MAX_ITEMS || bit(self.active, index) == on {
            return;
        }
        self.toggle_bit(index, on);
        self.redraw_all(cx);
    }

    pub fn is_active(&self, index: usize) -> bool {
        index < self.item_count() && bit(self.active, index)
    }

    /// Bit i = item i. Handy for hosts that keep their own [bool; N]. Only
    /// bits with a row behind them are reported.
    pub fn active_mask(&self) -> u32 {
        self.visible_mask()
    }

    /// Takes the mask WHOLE, whatever the labels currently are. Masking it
    /// against the row count here is what used to make an early restore —
    /// mask first, labels second — evaporate against `mask_for_len(0)`.
    pub fn set_active_mask(&mut self, cx: &mut Cx, mask: u32) {
        if self.active != mask {
            self.active = mask;
            self.redraw_all(cx);
        }
    }

    /// How many are on, for the chip's badge.
    pub fn active_count(&self) -> usize {
        self.visible_mask().count_ones() as usize
    }

    fn toggle_bit(&mut self, index: usize, on: bool) {
        if on {
            self.active |= 1u32 << index;
        } else {
            self.active &= !(1u32 << index);
        }
    }

    /// The chip's word plus the count of what is in force. The count alone
    /// when there is no word, nothing extra when nothing is on.
    fn chip_text(&self, count: usize) -> String {
        if count == 0 {
            self.text.clone()
        } else if self.text.is_empty() {
            count.to_string()
        } else {
            format!("{} {}", self.text, count)
        }
    }

    /// Tall enough for every row, and an ESTIMATE of wide enough for the
    /// longest label: character count times font size times a fudge, with
    /// PANEL_MAX_W as a hard ceiling. It is not a measured run — only
    /// `font_size` is read, no advance widths — so a wide-glyph label, or one
    /// past roughly 35 characters, will still run into the ceiling and elide.
    /// It stays an estimate because the offset has to be known on the EVENT
    /// side, where there is no `Cx2d` to lay text out in.
    fn panel_size(&self) -> DVec2 {
        let rows = self.item_count().max(1) as f64;
        let font = (self.draw_label.text_style.font_size as f64).max(6.0);
        // Measured if a draw has happened, guessed only before the first
        // one. The guess is a per-character average with no slack in it, so
        // capitals and digits, whose advances run wider than the average,
        // were sliced off at the panel's edge with no ellipsis to say so.
        let chars = self
            .labels
            .iter()
            .take(MAX_ITEMS)
            .map(|label| label.chars().count())
            .max()
            .unwrap_or(1) as f64;
        let text_w = if self.label_w > 0.0 { self.label_w } else { chars * font * 0.62 };
        let w = (text_w + PANEL_PAD_X * 2.0 + MARK + MARK_GAP)
            .clamp(PANEL_MIN_W, PANEL_MAX_W);
        dvec2(w, rows * ROW_H + PANEL_PAD_Y * 2.0)
    }

    /// Offset from the chip's top-left to the panel's: straight below with
    /// the left edges aligned, then pulled back inboard — flipping above the
    /// chip when there is room up there and no room down — so the panel is
    /// never half off the window.
    fn panel_offset(&self, chip: Rect) -> DVec2 {
        let size = self.panel_size();
        let pass = self.pass_size;
        // The shared placement rule decides the side (below, or above when
        // there is room up there and none down) and the x (pulled back
        // inboard of an EDGE inset, left edge winning). A pass of zero — the
        // first event after opening, before a draw has measured it — makes
        // both axes unbounded, so the panel simply hangs below until the
        // next draw, as it always did.
        let placed = place_overlay(&PlaceRequest {
            anchor: chip,
            size,
            bounds: Rect {
                pos: dvec2(EDGE, EDGE),
                size: pass - dvec2(EDGE * 2.0, EDGE * 2.0),
            },
            gap: PANEL_GAP,
            placement: Placement::BOTTOM_START,
            match_anchor_width: false,
        });
        // The only two vertical places the panel is ever allowed to be. It is
        // never pinned to the WINDOW instead: a panel pulled back over the
        // chip would take a press as a row toggle and then hand the same
        // press to the chip, closing the popover on a pick — the one thing
        // this widget promises cannot happen. Overrunning the window edge is
        // the lesser fault, so in the degenerate case (taller than the window
        // has room for on either side) the panel keeps its edge against the
        // chip and lets the roomier side show what it can. That is why only
        // the SIDE is read off the placement here, not its rect: the helper
        // shortens a popup to the room, and this panel keeps its full height.
        let y = match placed.side {
            Side::Top => -(size.y + PANEL_GAP),
            _ => chip.size.y + PANEL_GAP,
        };
        dvec2(placed.rect.pos.x - chip.pos.x, y)
    }

    /// Which row a WINDOW-absolute point lands on. `None` for anything off
    /// the panel, for the panel's own padding above the first row and below
    /// the last, and for the ROW_INSET gutters the rows are drawn inboard of
    /// — a press in the gutter is a press beside a row, not on it.
    fn row_at(&self, abs: DVec2) -> Option<usize> {
        if !self.panel_rect.contains(abs) {
            return None;
        }
        if abs.x < self.panel_rect.pos.x + ROW_INSET
            || abs.x > self.panel_rect.pos.x + self.panel_rect.size.x - ROW_INSET
        {
            return None;
        }
        let row = ((abs.y - self.panel_rect.pos.y - PANEL_PAD_Y) / ROW_H).floor();
        if row < 0.0 {
            return None;
        }
        let row = row as usize;
        if row < self.item_count() {
            Some(row)
        } else {
            None
        }
    }

    fn redraw_all(&mut self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_bg.redraw(cx);
    }

    /// Opening takes the pointer for the whole widget tree and closing hands
    /// it back — the pairing every popup in this crate uses. Without it the
    /// panel is a picture: the widgets it floats over are walked first and
    /// have already acted on the press by the time this widget sees it.
    ///
    /// A chip with no rows refuses to open at all. An empty 118x34 box that
    /// answers nothing is a hole in the screen, and one that has taken the
    /// pointer lock is a hole that swallows the rest of the window with it.
    fn set_open(&mut self, cx: &mut Cx, open: bool) {
        let open = open && self.item_count() > 0;
        if self.open != open {
            self.open = open;
            self.hover_row = None;
            if open {
                cx.sweep_lock(self.draw_bg.area());
            } else {
                cx.sweep_unlock(self.draw_bg.area());
            }
            self.draw_bg.set_uniform(cx, id!(open), &[if open { 1.0 } else { 0.0 }]);
            self.redraw_all(cx);
        }
    }

    /// A press while the popover is open. Returns true when it landed inside
    /// the panel, meaning the caller must mark the event handled: the sweep
    /// lock already turned away everything walked BEFORE this widget, and
    /// this stops anything walked after it.
    ///
    /// `primary` is false for the secondary and middle buttons: they may
    /// dismiss the popover from outside, but they never work a row and never
    /// reach through it to whatever is underneath.
    ///
    /// `mouse_held_outside` says another control has the pointer; see
    /// [`panel_press`] for what that changes and what it deliberately does
    /// not. A TOUCH press hands in false: a finger on a row still works it
    /// while a mouse elsewhere is held, which is the same asymmetry
    /// `is_mouse_held_outside` is built on.
    fn press_at(
        &mut self,
        cx: &mut Cx,
        abs: DVec2,
        primary: bool,
        mouse_held_outside: bool,
    ) -> bool {
        match panel_press(
            self.panel_rect.contains(abs),
            self.chip_rect.contains(abs),
            mouse_held_outside,
        ) {
            PanelPress::Row => {
                if primary {
                    if let Some(row) = self.row_at(abs) {
                        let on = !bit(self.active, row);
                        self.toggle_bit(row, on);
                        let uid = self.widget_uid();
                        cx.widget_action(uid, DropTogglesAction::Toggled(row, on));
                        self.redraw_all(cx);
                    }
                }
                true
            }
            PanelPress::Swallow => true,
            PanelPress::Chip => false,
            PanelPress::Dismiss => {
                self.set_open(cx, false);
                false
            }
        }
    }

    fn hover_at(&mut self, cx: &mut Cx, abs: DVec2) {
        let row = self.row_at(abs);
        if row != self.hover_row {
            self.hover_row = row;
            self.redraw_all(cx);
        }
    }
}

impl Widget for DropToggles {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let count = self.active_count();
        let lit = count > 0;
        // Set, draw, RESTORE. `draw_icon.color` is the call site's rest ink
        // and the only place that ink lives, so a widget that overwrites it
        // for good has nothing left to read back: a live re-apply would hand
        // in a fresh colour and then find it already clobbered by the last
        // frame, and a capture taken while lit would enshrine the accent as
        // the rest state. Straight at the field, too — a script apply naming
        // `draw_icon` re-applies the whole object and drops the loaded
        // document with it.
        let icon_rest = self.draw_icon.color;
        if lit {
            self.draw_icon.color = self.icon_color_lit;
        }
        self.draw_bg.lit = if lit { 1.0 } else { 0.0 };
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_icon.color = icon_rest;
        let chip_text = self.chip_text(count);
        if !chip_text.is_empty() {
            self.draw_text
                .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &chip_text);
        }
        self.draw_bg.end(cx);

        if self.open {
            // Everything the overlay pass needs is worked out here, before
            // the draw list is borrowed out of `self`.
            self.pass_size = cx.current_pass_size();
            let drawn = self.draw_bg.area().rect(cx);
            let anchor = Rect {
                pos: if self.chip_rect.size.y > 0.0 {
                    self.chip_rect.pos
                } else {
                    drawn.pos
                },
                size: drawn.size,
            };
            // Measure the real advances now that there is a Cx2d to do it
            // in, every open frame, so a label swap cannot leave a stale
            // width behind.
            let mut widest: f64 = 0.0;
            for label in self.labels.iter().take(MAX_ITEMS) {
                if let Some(run) = self.draw_label.prepare_single_line_run(cx, label) {
                    widest = widest.max(run.width_in_lpxs as f64);
                }
            }
            self.label_w = widest.ceil();
            let panel_size = self.panel_size();
            let offset = self.panel_offset(anchor);
            let mask = self.active;
            let hover_row = self.hover_row;
            if let Some(draw_list) = self.draw_list.as_mut() {
                // The PROVEN popup idiom (PopupMenu): draw the panel as
                // turtle content at the overlay root, then SHIFT the whole
                // list to hang under the chip. (draw_abs into a bare
                // overlay list renders nothing — learned the hard way.)
                draw_list.begin_overlay_reuse(cx);
                let size = cx.current_pass_size();
                cx.begin_root_turtle(size, Layout::flow_down());
                self.draw_panel.begin(
                    cx,
                    Walk::fixed(panel_size.x, panel_size.y),
                    Layout::default(),
                );
                let panel = cx.turtle().rect();
                let row_x = panel.pos.x + ROW_INSET;
                let row_w = (panel.size.x - ROW_INSET * 2.0).max(1.0);
                for (index, label) in self.labels.iter().take(MAX_ITEMS).enumerate() {
                    let y = panel.pos.y + PANEL_PAD_Y + index as f64 * ROW_H;
                    let on = bit(mask, index);
                    self.draw_row.hover = if hover_row == Some(index) { 1.0 } else { 0.0 };
                    self.draw_row.begin(
                        cx,
                        Walk::fixed(row_w, ROW_H).with_abs_pos(dvec2(row_x, y)),
                        row_layout(),
                    );
                    self.draw_mark.hover = self.draw_row.hover;
                    self.draw_mark.active = if on { 1.0 } else { 0.0 };
                    self.draw_mark.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(row_x + MARK_INSET, y + (ROW_H - MARK) * 0.5),
                            size: dvec2(MARK, MARK),
                        },
                    );
                    self.draw_label.hover = self.draw_row.hover;
                    self.draw_label.active = self.draw_mark.active;
                    self.draw_label
                        .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, label);
                    self.draw_row.end(cx);
                }
                self.draw_panel.end(cx);
                cx.end_pass_sized_turtle_with_shift(self.draw_bg.area(), offset);
                draw_list.end(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // The popover owns the pointer while open: a press on a row toggles
        // that row and NOTHING else, a press outside chip AND panel closes.
        // The CHIP toggle itself lives in the hits arm below — one press,
        // one state change. Raw events rather than `hits` because the panel
        // has no area of its own; the sweep lock taken in `set_open` is what
        // keeps every other widget out of these same presses.
        if self.open {
            // The panel hangs off the chip deterministically.
            let chip = self.draw_bg.area().rect(cx);
            self.chip_rect = chip;
            self.panel_rect = Rect {
                pos: chip.pos + self.panel_offset(chip),
                size: self.panel_size(),
            };
            match event {
                Event::MouseDown(me) => {
                    let held = cx.fingers.is_mouse_held_outside(&[self.draw_bg.area()]);
                    if self.press_at(cx, me.abs, me.button.is_primary(), held) {
                        me.handled.set(self.draw_bg.area());
                    }
                }
                // Touch never becomes a MouseDown: `hits` synthesises a
                // FingerDown from it for the chip, but the panel is not an
                // area, so without this arm the popover opens on a phone and
                // then answers nothing at all — not a row, not a dismiss.
                Event::TouchUpdate(te) => {
                    if let Some(touch) = te.touches.first() {
                        match touch.state {
                            TouchState::Start => {
                                if self.press_at(cx, touch.abs, true, false) {
                                    touch.handled.set(self.draw_bg.area());
                                }
                            }
                            TouchState::Move => self.hover_at(cx, touch.abs),
                            _ => {}
                        }
                    }
                }
                // A hover is a press-like state, and while another control
                // holds the mouse nothing else may take one from that
                // pointer. Reached raw, so `hits`' half of the rule never
                // runs and the question has to be asked here. The chip's own
                // capture — the press that opened the popover, still held —
                // is `mine`, so press-drag-over-the-rows still lights them;
                // and a TOUCH capture answers false there by design, so a
                // finger driving the popover is untouched.
                Event::MouseMove(me) => {
                    if !cx.fingers.is_mouse_held_outside(&[self.draw_bg.area()]) {
                        self.hover_at(cx, me.abs);
                    }
                }
                Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                    self.set_open(cx, false);
                }
                _ => {}
            }
        }
        // Named as its own sweep area, or the lock this widget took would
        // turn the chip's own hits away along with everyone else's.
        match event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.draw_bg.set_uniform(cx, id!(hover), &[1.0]);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.set_uniform(cx, id!(hover), &[0.0]);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.chip_rect = self.draw_bg.area().rect(cx);
                self.set_open(cx, !self.open);
            }
            _ => {}
        }
    }
}

impl DropTogglesRef {
    pub fn toggled(&self, actions: &Actions) -> Option<(usize, bool)> {
        if let DropTogglesAction::Toggled(index, on) =
            actions.find_widget_action(self.widget_uid())?.cast()
        {
            return Some((index, on));
        }
        None
    }

    pub fn set_labels(&self, cx: &mut Cx, labels: &[String]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_labels(cx, labels);
        }
    }

    /// Drop the chip word, or hand it back. Emptying it is how a chip that
    /// has run out of room goes icon-only: chip_text falls through to the
    /// count alone when the word is gone, so a filter that is in force is
    /// still readable on a chip too narrow to spell FILTER.
    pub fn set_text(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.text != text {
                inner.text = text.to_string();
                inner.redraw_all(cx);
            }
        }
    }

    pub fn set_active(&self, cx: &mut Cx, index: usize, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active(cx, index, on);
        }
    }

    pub fn is_active(&self, index: usize) -> bool {
        self.borrow().map(|i| i.is_active(index)).unwrap_or(false)
    }

    pub fn active_mask(&self) -> u32 {
        self.borrow().map(|inner| inner.active_mask()).unwrap_or(0)
    }

    pub fn set_active_mask(&self, cx: &mut Cx, mask: u32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active_mask(cx, mask);
        }
    }

    pub fn active_count(&self) -> usize {
        self.borrow().map(|i| i.active_count()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_on_a_row_works_it_and_is_spent() {
        crate::on_test_cx(|| {
        assert_eq!(panel_press(true, false, false), PanelPress::Row);
        });
    }

    #[test]
    fn a_press_in_the_panel_while_another_control_holds_the_mouse_works_no_row() {
        crate::on_test_cx(|| {
        // The control that took the pointer keeps it until the release. The
        // press is still the popover's, so it is swallowed rather than
        // pressed through the panel into whatever is underneath.
        assert_eq!(panel_press(true, false, true), PanelPress::Swallow);
        });
    }

    #[test]
    fn a_press_on_the_chip_is_left_to_the_chips_own_hit() {
        crate::on_test_cx(|| {
        assert_eq!(panel_press(false, true, false), PanelPress::Chip);
        assert_eq!(panel_press(false, true, true), PanelPress::Chip);
        });
    }

    #[test]
    fn a_press_outside_both_closes_the_popover_whatever_holds_the_mouse() {
        crate::on_test_cx(|| {
        // Dismissal is not a drag and does not stand down: a popover left up
        // while a control elsewhere is being dragged must still be closable.
        assert_eq!(panel_press(false, false, false), PanelPress::Dismiss);
        assert_eq!(panel_press(false, false, true), PanelPress::Dismiss);
        });
    }
}

/// The rule driven through the real event path, with a button holding the
/// pointer the way any control that is dragged continuously holds it.
#[cfg(test)]
mod pointer_tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: DVec2 = DVec2 { x: 800.0, y: 600.0 };

    /// A pass and a list to draw a page into, the way a window holds one.
    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    /// A button to hold the pointer down on, and a chip with three switches
    /// in its popover.
    fn page(cx: &mut Cx) -> WidgetRef {
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    grab := Button{text: "Grab"}
                    chip := DropToggles{text: "FILTER"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let labels = ["One".to_string(), "Two".to_string(), "Three".to_string()];
        root.widget(cx, ids!(chip)).as_drop_toggles().set_labels(cx, &labels);
        root
    }

    fn press(abs: DVec2) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn drag(abs: DVec2) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: Cell::new(Area::Empty),
        })
    }

    /// The popover open, drawn, and its panel rect worked out the way an
    /// event does: the middle of its second row, and the row the widget
    /// itself says is there.
    fn row_point(cx: &mut Cx, root: &WidgetRef, target: &mut Target) -> DVec2 {
        target.draw(cx, root);
        // The first move is what puts the panel rect on the widget; where it
        // points does not matter, only that it is off the panel.
        root.handle_event(cx, &drag(dvec2(700.0, 560.0)), &mut Scope::empty());
        let chip = root.widget(cx, ids!(chip));
        let inner = chip.borrow::<DropToggles>().expect("a chip");
        let panel = inner.panel_rect;
        let at = dvec2(panel.pos.x + panel.size.x * 0.5, panel.pos.y + PANEL_PAD_Y + ROW_H * 1.5);
        assert_eq!(inner.row_at(at), Some(1), "the point is on a row");
        at
    }

    fn hovered(cx: &Cx, root: &WidgetRef) -> Option<usize> {
        let chip = root.widget(cx, ids!(chip));
        let inner = chip.borrow::<DropToggles>().expect("a chip");
        inner.hover_row
    }

    /// A control that is dragged continuously holds the pointer until the
    /// release, and an open popover lights no row under it.
    #[test]
    fn no_row_lights_under_a_pointer_another_control_holds() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);

        let grab = root.widget(&cx, ids!(grab)).area().rect(&cx);
        root.handle_event(&mut cx, &press(grab.center()), &mut Scope::empty());
        assert!(cx.fingers.is_mouse_held_outside(&[]), "the button took the pointer");

        let chip = root.widget(&cx, ids!(chip)).area().rect(&cx);
        root.handle_event(&mut cx, &press(chip.center()), &mut Scope::empty());
        let at = row_point(&mut cx, &root, &mut target);
        root.handle_event(&mut cx, &drag(at), &mut Scope::empty());
        assert_eq!(hovered(&cx, &root), None, "the row under the drag stays unlit");
        });
    }

    /// And the other half: the chip's own press — the one that opened the
    /// popover, still held — is the popover's own pointer, so dragging from
    /// the chip onto a row lights it.
    #[test]
    fn the_chips_own_press_still_lights_the_row_it_is_dragged_onto() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);

        let chip = root.widget(&cx, ids!(chip)).area().rect(&cx);
        root.handle_event(&mut cx, &press(chip.center()), &mut Scope::empty());
        assert!(cx.fingers.is_mouse_held_outside(&[]), "the chip took the pointer");

        let at = row_point(&mut cx, &root, &mut target);
        root.handle_event(&mut cx, &drag(at), &mut Scope::empty());
        assert_eq!(hovered(&cx, &root), Some(1), "its own press lights the row it reaches");
        });
    }
}

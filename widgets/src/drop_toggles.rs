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

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

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
                sdf.fill(self.color.mix(self.color_hover, max(self.hover, self.open)))
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
                sdf.fill(self.color)
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
            border_color: uniform(#xffffff3d)
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
            color: #x9aa8b6
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
    #[live]
    pub labels: Vec<String>,
    /// Icon ink while anything is on.
    #[live]
    pub icon_color_lit: Vec4f,

    /// Bit i = item i is on.
    #[rust]
    active: u32,
    #[rust]
    open: bool,
    #[rust]
    hover_row: Option<usize>,
    /// The icon's rest ink, taken from the call site on the first draw so
    /// the lit state can hand it back untouched.
    #[rust]
    icon_color_rest: Vec4f,
    #[rust]
    icon_color_init: bool,
    /// The window this drew into last, for the edge flip/clamp. The event
    /// side has no `Cx2d` to ask, so the draw side leaves it here.
    #[rust]
    pass_size: DVec2,
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
    /// Replace the item labels. Clears nothing — the active mask is kept
    /// and truncated to the new length.
    pub fn set_labels(&mut self, cx: &mut Cx, labels: &[String]) {
        self.labels = labels.to_vec();
        self.active &= mask_for_len(self.labels.len());
        if let Some(row) = self.hover_row {
            if row >= self.labels.len() {
                self.hover_row = None;
            }
        }
        self.redraw_all(cx);
    }

    pub fn set_active(&mut self, cx: &mut Cx, index: usize, on: bool) {
        if index >= MAX_ITEMS || bit(self.active, index) == on {
            return;
        }
        self.toggle_bit(index, on);
        self.redraw_all(cx);
    }

    pub fn is_active(&self, index: usize) -> bool {
        bit(self.active, index)
    }

    /// Bit i = item i. Handy for hosts that keep their own [bool; N].
    pub fn active_mask(&self) -> u32 {
        self.active
    }

    pub fn set_active_mask(&mut self, cx: &mut Cx, mask: u32) {
        let mask = mask & mask_for_len(self.labels.len());
        if self.active != mask {
            self.active = mask;
            self.redraw_all(cx);
        }
    }

    /// How many are on, for the chip's badge.
    pub fn active_count(&self) -> usize {
        self.active.count_ones() as usize
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
            format!("{}", count)
        } else {
            format!("{} {}", self.text, count)
        }
    }

    /// Wide enough for the longest label without eliding, tall enough for
    /// every row. Measured off the label font so a call site restyling the
    /// rows does not saw the text in half.
    fn panel_size(&self) -> DVec2 {
        let rows = self.labels.len().max(1) as f64;
        let font = (self.draw_label.text_style.font_size as f64).max(6.0);
        let chars = self
            .labels
            .iter()
            .map(|label| label.chars().count())
            .max()
            .unwrap_or(1) as f64;
        let w = (chars * font * 0.62 + PANEL_PAD_X * 2.0 + MARK + MARK_GAP)
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
        let mut offset = dvec2(0.0, chip.size.y + PANEL_GAP);
        if pass.x > 0.0 {
            let right = pass.x - EDGE - size.x;
            if chip.pos.x > right {
                offset.x = (right - chip.pos.x).min(0.0);
            }
            if chip.pos.x + offset.x < EDGE {
                offset.x = EDGE - chip.pos.x;
            }
        }
        if pass.y > 0.0 && chip.pos.y + offset.y + size.y > pass.y - EDGE {
            let above = -(size.y + PANEL_GAP);
            offset.y = if chip.pos.y + above >= EDGE {
                above
            } else {
                (pass.y - EDGE - size.y - chip.pos.y).max(EDGE - chip.pos.y)
            };
        }
        offset
    }

    /// Which row an absolute point lands on, panel-relative.
    fn row_at(&self, abs: DVec2) -> Option<usize> {
        if !self.panel_rect.contains(abs) {
            return None;
        }
        let row = ((abs.y - self.panel_rect.pos.y - PANEL_PAD_Y) / ROW_H).floor();
        if row < 0.0 {
            return None;
        }
        let row = row as usize;
        if row < self.labels.len() {
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

    fn set_open(&mut self, cx: &mut Cx, open: bool) {
        if self.open != open {
            self.open = open;
            self.hover_row = None;
            self.draw_bg.set_uniform(cx, id!(open), &[if open { 1.0 } else { 0.0 }]);
            self.redraw_all(cx);
        }
    }
}

impl Widget for DropToggles {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let count = self.active_count();
        let lit = count > 0;
        if !self.icon_color_init {
            self.icon_color_init = true;
            self.icon_color_rest = self.draw_icon.color;
        }
        // Straight at the field: a script apply naming `draw_icon` re-applies
        // the whole object and drops the loaded document with it.
        self.draw_icon.color = if lit {
            self.icon_color_lit
        } else {
            self.icon_color_rest
        };
        self.draw_bg.lit = if lit { 1.0 } else { 0.0 };
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
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
            let chip = self.draw_bg.area().rect(cx);
            let panel_size = self.panel_size();
            let offset = self.panel_offset(chip);
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
                for (index, label) in self.labels.iter().enumerate() {
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
        let uid = self.widget_uid();
        // The popover owns the pointer while open: a press on a row toggles
        // that row and NOTHING else, a press outside chip AND panel closes.
        // The CHIP toggle itself lives in the hits arm below — one press,
        // one state change.
        if self.open {
            // The panel hangs off the chip deterministically.
            let chip = self.draw_bg.area().rect(cx);
            self.panel_rect = Rect {
                pos: chip.pos + self.panel_offset(chip),
                size: self.panel_size(),
            };
            match event {
                Event::MouseDown(me) => {
                    if self.panel_rect.contains(me.abs) {
                        if let Some(row) = self.row_at(me.abs) {
                            let on = !bit(self.active, row);
                            self.toggle_bit(row, on);
                            cx.widget_action(uid, DropTogglesAction::Toggled(row, on));
                            self.redraw_all(cx);
                        }
                    } else if !chip.contains(me.abs) {
                        self.set_open(cx, false);
                    }
                }
                Event::MouseMove(me) => {
                    let row = self.row_at(me.abs);
                    if row != self.hover_row {
                        self.hover_row = row;
                        self.redraw_all(cx);
                    }
                }
                Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                    self.set_open(cx, false);
                }
                _ => {}
            }
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.draw_bg.set_uniform(cx, id!(hover), &[1.0]);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.set_uniform(cx, id!(hover), &[0.0]);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(_) => {
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

    pub fn set_active(&self, cx: &mut Cx, index: usize, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active(cx, index, on);
        }
    }

    pub fn is_active(&self, index: usize) -> bool {
        self.borrow()
            .map(|inner| inner.is_active(index))
            .unwrap_or(false)
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
        self.borrow()
            .map(|inner| inner.active_count())
            .unwrap_or(0)
    }
}

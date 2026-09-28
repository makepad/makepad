//! HotkeyEditor — the keymap as a list the person can change.
//!
//! Each row is one command from the app's [`Hotkeys`] registry: its label,
//! the chord it answers to now (drawn as key caps), the chord it shipped
//! with, and a Reset when the two differ. A mark in front of the label says
//! the chord clashes with another command in the same scope; resting the
//! pointer on the mark says which.
//!
//! # Binding a chord
//!
//! Press a chord cell and the row starts listening. The next key pressed
//! together with the modifiers held becomes the binding; the cell shows the
//! modifiers as they go down so the person can see what they are about to
//! bind. Escape cancels, Backspace or Delete clears the binding, and moving
//! the key focus elsewhere cancels. While a row listens the registry resolves
//! nothing, so the chord being bound does not also run its old command.
//!
//! A clash is allowed: the new binding is kept and both rows get the mark.
//! Refusing it would make swapping two chords impossible without a detour.
//!
//! # Where the list comes from
//!
//! The rows are the registry's, read fresh on every draw, so a rebinding made
//! anywhere — this editor, a keyboard map, the host — shows up here. Defaults
//! can be declared on the editor in the menu bar's shape:
//!
//! ```text
//! HotkeyEditor{
//!     hotkeys: [
//!         {id: @save label: "Save" chord: "Mod+S"}
//!         {id: @find label: "Find" chord: "Mod+F" focus_scope: @editor}
//!     ]
//! }
//! ```
//!
//! Every change is reported as [`HotkeyEditorAction::Changed`] after it has
//! been made in the registry; the host saves it (see [`Hotkeys::save`]).
use crate::{
    badge::{measure, sized},
    button::{Button, ButtonAction},
    hotkeys::{is_modifier_key, HotkeyScope, Hotkeys, KeyChord, KeyPlatform},
    kbd::KbdGroup,
    makepad_derive_widget::*,
    makepad_draw::*,
    text_input::{TextInput, TextInputAction},
    tip::TipAction,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // Owns a TextInput, two Buttons and a KbdGroup, so this registers after
    // all of them: a block's `use` only sees what already exists.

    set_type_default() do #(DrawHotkeyCell::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.HotkeyEditorBase = #(HotkeyEditor::register_widget(vm))

    /** The app's hotkeys as rows of command, chord, default and reset. Press
     * a chord to bind the next key combination; Escape cancels, Backspace
     * clears. */
    mod.widgets.HotkeyEditor = set_type_default() do mod.widgets.HotkeyEditorBase{
        width: Fill
        height: Fit
        /** default bindings: {id: @save label: "Save" chord: "Mod+S" focus_scope: @editor} */
        hotkeys: []
        /** the room inside the panel edge 0..40 step 1 */
        pad: 10.
        /** the height of the filter field and the buttons 16..48 step 1 */
        toolbar_height: 26.
        /** the width of each button 40..160 step 1 */
        button_width: 84.
        /** the height of the column headings; 0 leaves them out 0..40 step 1 */
        header_height: 22.
        /** the height of one row 20..56 step 1 */
        row_height: 30.
        /** the width of the chord column 80..320 step 1 */
        chord_width: 190.
        /** the width of the default column; 0 leaves it out 0..240 step 1 */
        default_width: 130.
        /** the width of the reset column 24..100 step 1 */
        reset_width: 52.
        /** the room in front of a label for the clash mark 0..40 step 1 */
        mark_width: 18.
        /** the height of the caps in a chord cell 12..40 step 1 */
        cap_height: 20.
        /** shown in a cell while it listens and no modifier is down */
        capture_text: "Press a key\u{2026}"
        /** shown in a cell with no binding */
        unbound_text: "\u{2014}"
        /** shown in place of the rows when the filter matches nothing */
        empty_text: "No command matches"
        clip_x: true

        search: mod.widgets.TextInput{
            empty_text: "Filter commands"
        }
        undo: mod.widgets.ButtonFlatter{text: "Undo"}
        reset_all: mod.widgets.ButtonFlatter{text: "Reset all"}
        chord: mod.widgets.KbdGroup{}

        draw_bg +: {
            /** corner rounding 0..24 step 0.5 */
            radius: uniform(theme.radius_m)
            border_size: uniform(1.0)
            color: uniform(theme.color_surface_container)
            border_color: uniform(theme.color_outline_variant)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }
        draw_row +: {
            radius: uniform(theme.radius_s)
            color: uniform(theme.color_opaque_u_1)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_cell +: {
            radius: uniform(theme.radius_s)
            border_size: uniform(1.0)
            color: uniform(theme.color_surface_container_low)
            color_hover: uniform(theme.color_surface_container_high)
            color_capture: uniform(theme.color_primary_container)
            border_color: uniform(theme.color_outline_variant)
            border_capture: uniform(theme.color_primary)
            border_conflict: uniform(theme.color_error)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                let face = mix(self.color, self.color_hover, self.hover)
                let face2 = mix(face, self.color_capture, self.capture)
                sdf.fill_keep(face2)
                let edge = mix(self.border_color, self.border_conflict, self.conflict)
                let edge2 = mix(edge, self.border_capture, self.capture)
                sdf.stroke(edge2, self.border_size + self.capture)
                return sdf.result
            }
        }
        draw_label +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_meta +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_heading +: {
            color: theme.color_text_meta
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_mark +: {
            color: theme.color_error
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_action +: {
            color: theme.color_primary
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
    }
}

/// Where a line of text starts so it sits in the middle of `rect`: the y
/// `draw_abs` takes is the top of the line box, not of the ink.
fn text_y(rect: Rect, font_size: f64) -> f64 {
    rect.pos.y + (rect.size.y - font_size) * 0.5 - font_size * 0.3
}

/// A chord cell: its face, whether the pointer is on it, whether it is
/// listening for a chord, and whether its chord clashes with another.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawHotkeyCell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    capture: f32,
    #[live]
    conflict: f32,
}

#[derive(Clone, Debug, Default)]
pub enum HotkeyEditorAction {
    /// A command's chord changed in the registry: bound, cleared, reset or
    /// undone. `None` is unbound.
    Changed(LiveId, Option<KeyChord>),
    /// A row started listening for a chord.
    CaptureStarted(LiveId),
    /// A row stopped listening without a change.
    CaptureCancelled(LiveId),
    /// The pointer moved onto a row, or off every row.
    Hovered(Option<LiveId>),
    #[default]
    None,
}

/// Where one drawn row landed.
#[derive(Clone, Copy, Debug)]
struct RowHit {
    id: LiveId,
    row: Rect,
    cell: Rect,
    reset: Option<Rect>,
    mark: Option<Rect>,
}

/// What the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    Row(LiveId),
    Cell(LiveId),
    Reset(LiveId),
    Mark(LiveId),
}

impl Spot {
    fn id(self) -> LiveId {
        match self {
            Spot::Row(id) | Spot::Cell(id) | Spot::Reset(id) | Spot::Mark(id) => id,
        }
    }
}

#[derive(Script, Widget)]
pub struct HotkeyEditor {
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
    draw_bg: DrawQuad,
    #[live]
    draw_row: DrawQuad,
    #[live]
    draw_cell: DrawHotkeyCell,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_meta: DrawText,
    #[live]
    draw_heading: DrawText,
    #[live]
    draw_mark: DrawText,
    #[live]
    draw_action: DrawText,
    /// One chord, drawn once per bound row.
    #[live]
    chord: KbdGroup,
    #[live]
    search: TextInput,
    #[live]
    undo: Button,
    #[live]
    reset_all: Button,

    /// Default bindings written in the DSL, registered on apply.
    #[live]
    hotkeys: ScriptValue,

    #[live(10.0)]
    pub pad: f64,
    #[live(26.0)]
    pub toolbar_height: f64,
    #[live(84.0)]
    pub button_width: f64,
    #[live(22.0)]
    pub header_height: f64,
    #[live(30.0)]
    pub row_height: f64,
    #[live(190.0)]
    pub chord_width: f64,
    #[live(130.0)]
    pub default_width: f64,
    #[live(52.0)]
    pub reset_width: f64,
    #[live(18.0)]
    pub mark_width: f64,
    #[live(20.0)]
    pub cap_height: f64,
    #[live]
    pub capture_text: String,
    #[live]
    pub unbound_text: String,
    #[live]
    pub empty_text: String,
    #[live(true)]
    #[visible]
    visible: bool,

    #[rust]
    hotkeys_source: ScriptValue,
    #[rust]
    query: String,
    #[rust]
    rows: Vec<RowHit>,
    /// The rows' visible window, for the scroll wheel.
    #[rust]
    list_rect: Rect,
    #[rust]
    scroll: f64,
    #[rust]
    scroll_max: f64,
    #[rust]
    spot: Option<Spot>,
    /// The row whose clash tip is up.
    #[rust]
    tip_for: Option<LiveId>,
    #[rust]
    capturing: Option<LiveId>,
    /// The modifiers held while listening, for the cell's preview.
    #[rust]
    capture_mods: KeyModifiers,
    #[rust]
    drawn_revision: u64,
}

impl ScriptHook for HotkeyEditor {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.hotkeys.raw() != self.hotkeys_source.raw() {
            self.hotkeys_source = self.hotkeys;
            Hotkeys::register_script(vm, self.hotkeys);
        }
        let cx = vm.cx_mut();
        self.draw_bg.redraw(cx);
    }
}

/// Does a command match the filter? On its label, its chord as this
/// platform writes it, or its id.
fn matches_query(label: &str, chord: Option<KeyChord>, id: LiveId, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if label.to_lowercase().contains(query) {
        return true;
    }
    if chord.is_some_and(|c| c.format().to_lowercase().contains(query)) {
        return true;
    }
    id.to_string().to_lowercase().contains(query)
}

/// The modifiers held so far, written the way a chord is, ending in an
/// ellipsis for the key still to come.
fn modifiers_preview(m: KeyModifiers) -> String {
    let apple = KeyPlatform::current() == KeyPlatform::Apple;
    let mut parts: Vec<&str> = Vec::new();
    if apple && m.logo {
        parts.push("Cmd");
    }
    if m.control {
        parts.push("Ctrl");
    }
    if !apple && m.logo {
        parts.push("Super");
    }
    if m.alt {
        parts.push(if apple { "Opt" } else { "Alt" });
    }
    if m.shift {
        parts.push("Shift");
    }
    parts.push("\u{2026}");
    parts.join("+")
}

impl HotkeyEditor {
    /// Filter the rows. The field is set along with the model, so a query
    /// pushed by the host and one typed in leave the widget the same.
    pub fn set_query(&mut self, cx: &mut Cx, query: &str) {
        if self.query != query {
            self.query = query.to_string();
            self.scroll = 0.0;
            self.spot = None;
            if self.search.text() != query {
                self.search.set_text(cx, query);
            }
            self.redraw(cx);
        }
    }

    /// The row listening for a chord, if one is.
    pub fn capturing(&self) -> Option<LiveId> {
        self.capturing
    }

    /// Make a row listen for the next chord, as a press on its cell does.
    pub fn start_capture(&mut self, cx: &mut Cx, id: LiveId) {
        if cx.global::<Hotkeys>().get(id).is_none() {
            return;
        }
        if let Some(previous) = self.capturing.take() {
            if previous != id {
                cx.widget_action(self.uid, HotkeyEditorAction::CaptureCancelled(previous));
            }
        }
        self.capturing = Some(id);
        self.capture_mods = KeyModifiers::default();
        cx.global::<Hotkeys>().set_capturing(true);
        cx.set_key_focus(self.draw_bg.area());
        cx.widget_action(self.uid, HotkeyEditorAction::CaptureStarted(id));
        self.redraw(cx);
    }

    /// Stop listening without changing anything.
    pub fn cancel_capture(&mut self, cx: &mut Cx) {
        if let Some(id) = self.capturing.take() {
            cx.global::<Hotkeys>().set_capturing(false);
            cx.widget_action(self.uid, HotkeyEditorAction::CaptureCancelled(id));
            self.redraw(cx);
        }
    }

    /// End listening by binding `chord` (or clearing with `None`); `ke` is
    /// the key press that ended it.
    fn finish_capture(&mut self, cx: &mut Cx, ke: &KeyEvent, chord: Option<KeyChord>) {
        let Some(id) = self.capturing.take() else {
            return;
        };
        let hotkeys = cx.global::<Hotkeys>();
        hotkeys.end_capture_with(ke);
        if hotkeys.rebind(id, chord) {
            cx.widget_action(self.uid, HotkeyEditorAction::Changed(id, chord));
        } else {
            cx.widget_action(self.uid, HotkeyEditorAction::CaptureCancelled(id));
        }
        self.redraw(cx);
    }

    fn reset_one(&mut self, cx: &mut Cx, id: LiveId) {
        let hotkeys = cx.global::<Hotkeys>();
        if hotkeys.reset(id) {
            let chord = hotkeys.bindings_for(id);
            cx.widget_action(self.uid, HotkeyEditorAction::Changed(id, chord));
            self.redraw(cx);
        }
    }

    fn reset_every(&mut self, cx: &mut Cx) {
        let hotkeys = cx.global::<Hotkeys>();
        let changed: Vec<(LiveId, Option<KeyChord>)> = hotkeys
            .reset_all()
            .into_iter()
            .map(|id| (id, hotkeys.bindings_for(id)))
            .collect();
        for (id, chord) in changed {
            cx.widget_action(self.uid, HotkeyEditorAction::Changed(id, chord));
        }
        self.redraw(cx);
    }

    fn undo_last(&mut self, cx: &mut Cx) {
        let restored = cx.global::<Hotkeys>().undo();
        for (id, chord) in restored {
            cx.widget_action(self.uid, HotkeyEditorAction::Changed(id, chord));
        }
        self.redraw(cx);
    }

    fn spot_at(&self, abs: DVec2) -> Option<Spot> {
        if !self.list_rect.contains(abs) {
            return None;
        }
        let hit = self.rows.iter().find(|hit| hit.row.contains(abs))?;
        if hit.cell.contains(abs) {
            return Some(Spot::Cell(hit.id));
        }
        if hit.reset.is_some_and(|r| r.contains(abs)) {
            return Some(Spot::Reset(hit.id));
        }
        if hit.mark.is_some_and(|r| r.contains(abs)) {
            return Some(Spot::Mark(hit.id));
        }
        Some(Spot::Row(hit.id))
    }

    /// Move the pointer's spot, raising or lowering the clash tip and the
    /// hover report as it changes.
    fn set_spot(&mut self, cx: &mut Cx, spot: Option<Spot>) {
        if spot == self.spot {
            return;
        }
        let row_before = self.spot.map(Spot::id);
        self.spot = spot;
        let row_now = spot.map(Spot::id);
        if row_before != row_now {
            cx.widget_action(self.uid, HotkeyEditorAction::Hovered(row_now));
        }
        cx.set_cursor(match spot {
            Some(Spot::Cell(_)) | Some(Spot::Reset(_)) => MouseCursor::Hand,
            _ => MouseCursor::Default,
        });
        let tip_now = match spot {
            Some(Spot::Mark(id)) => Some(id),
            _ => None,
        };
        if tip_now != self.tip_for {
            self.tip_for = tip_now;
            match tip_now {
                Some(id) => {
                    let text = self.clash_text(cx, id);
                    let anchor = self
                        .rows
                        .iter()
                        .find(|hit| hit.id == id)
                        .and_then(|hit| hit.mark)
                        .unwrap_or_default();
                    cx.widget_action(self.uid, TipAction::HoverIn(text, anchor));
                }
                None => cx.widget_action(self.uid, TipAction::HoverOut),
            }
        }
        self.redraw(cx);
    }

    /// What the clash mark on a row says.
    fn clash_text(&self, cx: &mut Cx, id: LiveId) -> String {
        let hotkeys = cx.global::<Hotkeys>();
        let Some(chord) = hotkeys.bindings_for(id) else {
            return String::new();
        };
        let others: Vec<String> = hotkeys
            .conflicts_with(id, chord)
            .into_iter()
            .filter_map(|other| hotkeys.get(other).map(|h| h.label.clone()))
            .collect();
        format!("{} is also bound to {}", chord.format(), others.join(", "))
    }
}

impl Widget for HotkeyEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let (list, conflicted, revision) = {
            let hotkeys = cx.global::<Hotkeys>();
            let list: Vec<_> = hotkeys.iter().cloned().collect();
            let mut conflicted: Vec<LiveId> = Vec::new();
            for (a, b) in hotkeys.conflicts() {
                conflicted.push(a);
                conflicted.push(b);
            }
            (list, conflicted, hotkeys.revision())
        };
        self.drawn_revision = revision;
        let query = self.query.trim().to_lowercase();
        let shown: Vec<_> = list
            .iter()
            .filter(|h| matches_query(&h.label, h.chord, h.id, &query))
            .collect();

        let above_list = self.pad + self.toolbar_height + self.pad * 0.5 + self.header_height;
        let content = (shown.len().max(1)) as f64 * self.row_height;
        let fit_height = above_list + content + self.pad;
        let min_width =
            self.pad * 2.0 + self.mark_width + 120.0 + self.chord_width + self.default_width + self.reset_width;

        self.draw_bg
            .begin(cx, sized(walk, min_width, fit_height), self.layout);
        let panel = cx.turtle().rect();
        let left = panel.pos.x + self.pad;
        let width = (panel.size.x - self.pad * 2.0).max(0.0);
        let mut y = panel.pos.y + self.pad;

        // The toolbar: the filter, then the two buttons at the right end.
        let buttons = (self.button_width + self.pad * 0.5) * 2.0;
        let search_walk = Walk {
            abs_pos: Some(dvec2(left, y)),
            width: Size::Fixed((width - buttons).max(40.0)),
            height: Size::Fixed(self.toolbar_height),
            ..Walk::default()
        };
        let _ = self.search.draw_walk(cx, scope, search_walk);
        let mut bx = left + width - self.button_width * 2.0 - self.pad * 0.5;
        for button in [&mut self.undo, &mut self.reset_all] {
            let walk = Walk {
                abs_pos: Some(dvec2(bx, y)),
                width: Size::Fixed(self.button_width),
                height: Size::Fixed(self.toolbar_height),
                ..Walk::default()
            };
            let _ = button.draw_walk(cx, scope, walk);
            bx += self.button_width + self.pad * 0.5;
        }
        y += self.toolbar_height + self.pad * 0.5;

        // Column edges, right to left from the panel edge.
        let reset_x = left + width - self.reset_width;
        let default_x = reset_x - self.default_width;
        let chord_x = default_x - self.chord_width;
        let label_x = left + self.mark_width;

        if self.header_height > 0.0 {
            let rect = Rect {
                pos: dvec2(left, y),
                size: dvec2(width, self.header_height),
            };
            let fs = self.draw_heading.text_style.font_size as f64;
            let ty = text_y(rect, fs);
            self.draw_heading.draw_abs(cx, dvec2(label_x, ty), "Command");
            self.draw_heading.draw_abs(cx, dvec2(chord_x + 6.0, ty), "Shortcut");
            if self.default_width > 0.0 {
                self.draw_heading.draw_abs(cx, dvec2(default_x + 6.0, ty), "Default");
            }
            y += self.header_height;
        }

        // The rows scroll inside a clipped window of their own, so the
        // filter and the headings stay put.
        let list_height = (panel.pos.y + panel.size.y - self.pad - y).max(0.0);
        self.list_rect = Rect {
            pos: dvec2(left, y),
            size: dvec2(width, list_height),
        };
        self.scroll_max = (content - list_height).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.scroll_max);
        cx.begin_turtle(
            Walk {
                abs_pos: Some(self.list_rect.pos),
                width: Size::Fixed(width),
                height: Size::Fixed(list_height),
                ..Walk::default()
            },
            Layout {
                clip_x: true,
                clip_y: true,
                ..Layout::default()
            },
        );

        self.rows.clear();
        if shown.is_empty() {
            let rect = Rect {
                pos: dvec2(left, y),
                size: dvec2(width, self.row_height),
            };
            let fs = self.draw_meta.text_style.font_size as f64;
            let text = self.empty_text.clone();
            self.draw_meta
                .draw_abs(cx, dvec2(label_x, text_y(rect, fs)), &text);
        }
        let mut row_y = y - self.scroll;
        for hotkey in shown {
            let row = Rect {
                pos: dvec2(left, row_y),
                size: dvec2(width, self.row_height),
            };
            row_y += self.row_height;
            if row.pos.y + row.size.y < y || row.pos.y > y + list_height {
                continue;
            }
            let id = hotkey.id;
            let hovered = self.spot.map(Spot::id) == Some(id);
            let capturing = self.capturing == Some(id);
            let clashes = hotkey.chord.is_some() && conflicted.contains(&id);
            if hovered || capturing {
                self.draw_row.draw_abs(cx, row);
            }

            let fs = self.draw_label.text_style.font_size as f64;
            let ty = text_y(row, fs);
            let mut mark = None;
            if clashes {
                let rect = Rect {
                    pos: dvec2(left, row.pos.y),
                    size: dvec2(self.mark_width, row.size.y),
                };
                self.draw_mark.draw_abs(cx, dvec2(left + 4.0, ty), "!");
                mark = Some(rect);
            }
            self.draw_label.draw_abs(cx, dvec2(label_x, ty), &hotkey.label);
            if let HotkeyScope::Focused(scope_id) = hotkey.scope {
                let label_w = measure(&self.draw_label, cx, &hotkey.label);
                let note = format!("in {}", scope_id);
                self.draw_meta
                    .draw_abs(cx, dvec2(label_x + label_w + 6.0, ty), &note);
            }

            // The chord cell.
            let cell = Rect {
                pos: dvec2(chord_x, row.pos.y + 3.0),
                size: dvec2(self.chord_width - 8.0, row.size.y - 6.0),
            };
            self.draw_cell.hover = if self.spot == Some(Spot::Cell(id)) { 1.0 } else { 0.0 };
            self.draw_cell.capture = if capturing { 1.0 } else { 0.0 };
            self.draw_cell.conflict = if clashes { 1.0 } else { 0.0 };
            self.draw_cell.draw_abs(cx, cell);
            if capturing {
                let text = if self.capture_mods.any() {
                    modifiers_preview(self.capture_mods)
                } else {
                    self.capture_text.clone()
                };
                self.draw_label.draw_abs(cx, dvec2(cell.pos.x + 8.0, ty), &text);
            } else if let Some(chord) = hotkey.chord {
                self.chord.shortcut = chord.to_kbd_shortcut();
                let caps_h = self.cap_height.min(cell.size.y - 2.0).max(8.0);
                let size = self.chord.chord_size(cx, caps_h);
                let caps = Rect {
                    pos: dvec2(cell.pos.x + 4.0, cell.pos.y + (cell.size.y - size.y) * 0.5),
                    size,
                };
                self.chord.draw_chord(cx, caps);
            } else {
                let text = self.unbound_text.clone();
                self.draw_meta.draw_abs(cx, dvec2(cell.pos.x + 8.0, ty), &text);
            }

            if self.default_width > 0.0 {
                let text = match hotkey.default {
                    Some(chord) => chord.format(),
                    None => self.unbound_text.clone(),
                };
                self.draw_meta.draw_abs(cx, dvec2(default_x + 6.0, ty), &text);
            }

            let mut reset = None;
            if hotkey.is_customized() {
                let rect = Rect {
                    pos: dvec2(reset_x, row.pos.y),
                    size: dvec2(self.reset_width, row.size.y),
                };
                self.draw_action.draw_abs(cx, dvec2(reset_x + 4.0, ty), "Reset");
                reset = Some(rect);
            }
            self.rows.push(RowHit {
                id,
                row,
                cell,
                reset,
                mark,
            });
        }
        cx.end_turtle();

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // A change made elsewhere — a keyboard map, the host — redraws the
        // rows it touched.
        if cx.global::<Hotkeys>().revision() != self.drawn_revision {
            self.drawn_revision = cx.global::<Hotkeys>().revision();
            self.redraw(cx);
        }

        for action in cx.capture_actions(|cx| self.search.handle_event(cx, event, scope)) {
            if let TextInputAction::Changed(text) = action.as_widget_action().cast() {
                self.set_query(cx, &text);
            }
        }
        let mut undo_clicked = false;
        for action in cx.capture_actions(|cx| self.undo.handle_event(cx, event, scope)) {
            if let ButtonAction::Clicked(_) = action.as_widget_action().cast() {
                undo_clicked = true;
            }
        }
        let mut reset_clicked = false;
        for action in cx.capture_actions(|cx| self.reset_all.handle_event(cx, event, scope)) {
            if let ButtonAction::Clicked(_) = action.as_widget_action().cast() {
                reset_clicked = true;
            }
        }
        if undo_clicked {
            self.cancel_capture(cx);
            self.undo_last(cx);
        }
        if reset_clicked {
            self.cancel_capture(cx);
            self.reset_every(cx);
        }

        // A press anywhere outside the panel ends a capture: the person has
        // moved on, whether or not what they pressed takes the key focus.
        if let Event::MouseDown(me) = event {
            if self.capturing.is_some() && !self.draw_bg.area().rect(cx).contains(me.abs) {
                self.cancel_capture(cx);
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyDown(ke) if self.capturing.is_some() => {
                let bare = !ke.modifiers.any();
                match ke.key_code {
                    KeyCode::Escape if bare => {
                        self.cancel_capture(cx);
                        cx.global::<Hotkeys>().end_capture_with(&ke);
                    }
                    KeyCode::Backspace | KeyCode::Delete if bare => {
                        self.finish_capture(cx, &ke, None)
                    }
                    key if is_modifier_key(key) => {
                        // Whether a modifier's own press already counts it
                        // held differs between platforms; count it here.
                        let mut mods = ke.modifiers;
                        match key {
                            KeyCode::Control => mods.control = true,
                            KeyCode::Alt => mods.alt = true,
                            KeyCode::Shift => mods.shift = true,
                            _ => mods.logo = true,
                        }
                        self.capture_mods = mods;
                        self.redraw(cx);
                    }
                    _ => {
                        if let Some(chord) = KeyChord::from_key_event(&ke) {
                            self.finish_capture(cx, &ke, Some(chord));
                        }
                    }
                }
            }
            Hit::KeyUp(ke) if self.capturing.is_some() => {
                let mut mods = ke.modifiers;
                match ke.key_code {
                    KeyCode::Control => mods.control = false,
                    KeyCode::Alt => mods.alt = false,
                    KeyCode::Shift => mods.shift = false,
                    KeyCode::Logo => mods.logo = false,
                    _ => {}
                }
                self.capture_mods = mods;
                self.redraw(cx);
            }
            Hit::KeyFocusLost(_) => self.cancel_capture(cx),
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let spot = self.spot_at(fe.abs);
                self.set_spot(cx, spot);
            }
            Hit::FingerHoverOut(_) => self.set_spot(cx, None),
            Hit::FingerScroll(fe) if self.list_rect.contains(fe.abs) && self.scroll_max > 0.0 => {
                let scroll = (self.scroll + fe.scroll.y).clamp(0.0, self.scroll_max);
                if scroll != self.scroll {
                    self.scroll = scroll;
                    self.spot = None;
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                if self.tip_for.take().is_some() {
                    cx.widget_action(self.uid, TipAction::HoverOut);
                }
                match self.spot_at(fe.abs) {
                    Some(Spot::Cell(id)) => {
                        if self.capturing == Some(id) {
                            self.cancel_capture(cx);
                        } else {
                            self.start_capture(cx, id);
                        }
                    }
                    Some(Spot::Reset(id)) => {
                        self.cancel_capture(cx);
                        self.reset_one(cx, id);
                    }
                    _ => self.cancel_capture(cx),
                }
            }
            _ => {}
        }
    }

    /// What is in the filter field.
    fn text(&self) -> String {
        self.query.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_query(cx, v);
    }

    /// The row listening for a chord, or the number of rows drawn.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(match self.capturing {
            Some(id) => format!("capturing {}", id),
            None => format!("{} rows", self.rows.len()),
        })
    }
}

impl HotkeyEditorRef {
    pub fn set_query(&self, cx: &mut Cx, query: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_query(cx, query);
        }
    }

    pub fn start_capture(&self, cx: &mut Cx, id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.start_capture(cx, id);
        }
    }

    pub fn cancel_capture(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.cancel_capture(cx);
        }
    }

    pub fn capturing(&self) -> Option<LiveId> {
        self.borrow().and_then(|inner| inner.capturing())
    }

    /// Every change this pass, in the order it was made.
    pub fn changes(&self, actions: &Actions) -> Vec<(LiveId, Option<KeyChord>)> {
        actions
            .filter_widget_actions_cast::<HotkeyEditorAction>(self.widget_uid())
            .filter_map(|action| match action {
                HotkeyEditorAction::Changed(id, chord) => Some((id, chord)),
                _ => None,
            })
            .collect()
    }

    /// The first change this pass.
    pub fn changed(&self, actions: &Actions) -> Option<(LiveId, Option<KeyChord>)> {
        self.changes(actions).into_iter().next()
    }

    /// The row the pointer moved to this pass: `Some(None)` when it left
    /// every row.
    pub fn hovered(&self, actions: &Actions) -> Option<Option<LiveId>> {
        actions
            .filter_widget_actions_cast::<HotkeyEditorAction>(self.widget_uid())
            .filter_map(|action| match action {
                HotkeyEditorAction::Hovered(id) => Some(id),
                _ => None,
            })
            .last()
    }
}

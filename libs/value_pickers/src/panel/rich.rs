//! The panel's larger controls, each bound to a path like the value
//! controls: a colour (a row that opens the HDR picker), words (one line or
//! several), a function slot (the functions that could stand there), and a
//! group that folds.

use super::control::{draw_key, fit_text, DrawPanelKey, DrawPanelShape, LABEL, R_CONTROL, R_GROUP};
use super::host::{self, KeyButton, PanelValue};
use crate::color_picker::{format_code_color, parse_code_color, ColorPickAction, HdrColorPicker};
use makepad_widgets::makepad_script::syntax::{self as tree, Kind};
use makepad_widgets::badge::advance;
use makepad_widgets::makepad_script::ScriptFnRef;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    /** What a colour or font row opens: a rounded inset under the row. */
    let PickerInset = RoundedView{width: Fill height: Fit flow: Down padding: Inset{left: 8 right: 8 top: 8 bottom: 8}
        draw_bg +: {color: stage.well border_color: stage.line_faint border_size: 1.0 border_radius: 5.0}}

    mod.widgets.PanelGroupBase = #(PanelGroup::register_widget(vm))
    /** Controls under a title that folds; groups stack. */
    mod.widgets.PanelGroup = set_type_default() do mod.widgets.PanelGroupBase{
        width: Fill height: Fit flow: Down spacing: 6 padding: Inset{left: 8 right: 8 top: 0 bottom: 8}
        draw_title +: {text_style: theme.font_bold{font_size: 7.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelColorBase = #(PanelColor::register_widget(vm))
    /** A colour at a path: its row, and the HDR picker it opens. */
    mod.widgets.PanelColor = set_type_default() do mod.widgets.PanelColorBase{
        width: Fill height: Fit flow: Down spacing: 4
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        // Open, the row's code is this field (the picker does not show it again).
        hex := TextInput{width: 132 height: 20 margin: 0 padding: Inset{left: 7 right: 7 top: 2 bottom: 2} empty_text: "#rrggbb"
            draw_bg +: {color: stage.row color_hover: stage.row color_focus: stage.row border_color: stage.line_faint border_size: 1.0 border_radius: 4.0}
            draw_text +: {text_style: theme.font_code{font_size: 8}}}
        inset := PickerInset{picker := mod.widgets.HdrColorPicker{}}
    }

    mod.widgets.PanelTextBase = #(PanelText::register_widget(vm))
    /** Words at a path, typed live. */
    mod.widgets.PanelText = set_type_default() do mod.widgets.PanelTextBase{
        width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.0}
        name := Label{width: 104 padding: Inset{top: 5} max_lines: 1 text_overflow: Ellipsis text: ""
            draw_text +: {color: stage.label text_style: theme.font_regular{font_size: 8.5}}}
        input := TextInput{width: Fill height: Fit margin: 0 padding: Inset{left: 8 right: 8 top: 4 bottom: 4} empty_text: ""
            draw_bg +: {color: stage.well color_hover: stage.well color_focus: stage.well border_color: stage.line_faint border_size: 1.0 border_radius: 4.0}
            draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}
    }
    /** Words over several lines (a lyric, a caption). */
    mod.widgets.PanelWords = mod.widgets.PanelText{
        input +: {is_multiline: true}
    }

    mod.widgets.PanelFnBase = #(PanelFn::register_widget(vm))
    /** A function slot: the functions with the same parameters, to swap in. */
    mod.widgets.PanelFn = set_type_default() do mod.widgets.PanelFnBase{
        width: Fill height: Fit
        draw_bg +: {color: #x00000000}
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

pub const ROW: f64 = 24.0;

/// A row a rich control draws above what it opens: the label (cut to the
/// label column), a swatch or nothing, the value as written, and a chevron
/// (open or folded); washed under the pointer and while open.
pub fn draw_row(cx: &mut Cx2d, head: &mut DrawPanelShape, marks: &mut DrawPanelShape, text: &mut DrawText, rect: Rect, label_in: f64, label: &str, fill: Option<Vec4f>, value: &str, open: bool, hot: bool) {
    let p = makepad_look::palette::get(cx);
    let wash = if hot { color(p.control_hover, 0.6) } else if open { color(p.raised, 1.0) } else { color(0, 0.0) };
    head.fill_round(cx, Rect { pos: dvec2(rect.pos.x - 4.0, rect.pos.y), size: dvec2(rect.size.x + 8.0, rect.size.y) }, wash, R_CONTROL);
    // Narrow (several side by side in a row): the swatch, then the label,
    // nothing else; the value is in the picker it opens.
    if rect.size.x < LABEL * 2.0 + 40.0 {
        let mut x = rect.pos.x + label_in;
        if let Some(fill) = fill {
            marks.edged_round(cx, Rect { pos: dvec2(x, rect.pos.y + 4.0), size: dvec2(18.0, ROW - 8.0) }, fill, color(p.border, 1.0), 1.0, 3.0);
            x += 24.0;
        }
        text.color = color(if open { p.text_hi } else { p.label }, 1.0);
        let (label, _) = fit_text(text, cx, label, (rect.pos.x + rect.size.x - x - 2.0).max(10.0));
        text.draw_abs(cx, dvec2(x, rect.pos.y + 6.0), &label);
        return;
    }
    text.color = color(p.label, 1.0);
    let (label, _) = fit_text(text, cx, label, LABEL - label_in - 4.0);
    text.draw_abs(cx, dvec2(rect.pos.x + label_in, rect.pos.y + 6.0), &label);
    let mut x = rect.pos.x + LABEL + 8.0;
    if let Some(fill) = fill {
        marks.edged_round(cx, Rect { pos: dvec2(x, rect.pos.y + 4.0), size: dvec2(22.0, ROW - 8.0) }, fill, color(p.border, 1.0), 1.0, 3.0);
        x += 30.0;
    }
    text.color = color(p.text_bright, 1.0);
    let (value, _) = fit_text(text, cx, value, (rect.pos.x + rect.size.x - 22.0 - x).max(10.0));
    text.draw_abs(cx, dvec2(x, rect.pos.y + 6.0), &value);
    marks.chevron(cx, Rect { pos: dvec2(rect.pos.x + rect.size.x - 14.0, rect.pos.y + 6.0), size: dvec2(12.0, 12.0) }, color(p.muted, 1.0), open);
}

/// What a value written as a name stands for (`BONE` → `#eee9df`, `SANS`
/// → `Font{…}`): the name followed to what it holds (in the module `path`
/// is in, else the document), through names of names; a value that is not
/// a name is itself. A `/**tag*/` before a value is not part of it.
pub fn named_value(path: &str, code: &str) -> String {
    let mut now = code.trim().to_string();
    for _ in 0..4 {
        if !is_name(&now) {
            break;
        }
        let module = path.split_once('.').map(|(m, _)| m).filter(|m| !now.contains('.') && *m != now);
        let found = host::read(&now).or_else(|| module.and_then(|m| host::read(&format!("{m}.{now}"))));
        let Some(value) = found else { break };
        now = value.code().trim().to_string();
        while let Some(rest) = now.strip_prefix("/**").and_then(|r| r.split_once("*/")).map(|(_, r)| r.trim().to_string()) {
            now = rest;
        }
    }
    now
}

/// Whether a value is written as a name (of a `let`, a module's member).
fn is_name(code: &str) -> bool {
    let t = code.trim();
    !t.is_empty() && t.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') && !t.starts_with(|c: char| c.is_ascii_digit()) && !matches!(t, "true" | "false")
}

// ---------------------------------------------------------------- group

#[derive(Script, ScriptHook, Widget)]
pub struct PanelGroup {
    /// When in the film it acts: the host reads it from the panel's code.
    #[live]
    pub when: String,
    #[deref]
    view: View,
    #[live]
    pub title: String,
    /// The group's inset: a rounded box under its heading and controls.
    #[live]
    draw_box: DrawPanelShape,
    /// The heading (what the pointer hits).
    #[live]
    draw_head: DrawPanelShape,
    /// The heading's chevron.
    #[live]
    draw_mark: DrawPanelShape,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_small: DrawText,
    /// Folded until clicked or revealed (and again each time its module
    /// opens: [`PanelGroup::fold`]).
    #[rust]
    folded: Option<bool>,
    /// Opened by hand: brought into view once it has drawn open.
    #[rust]
    reveal_open: bool,
    /// Its fold's key ([`host::fold_key`]), as last drawn.
    #[rust]
    key: String,
    #[rust]
    hot: bool,
    /// The picture's selection it last acted on.
    #[rust]
    reveal_seen: Option<u64>,
}

impl PanelGroup {
    fn folded(&self) -> bool {
        self.folded.unwrap_or(true) && host::search().is_empty()
    }

    /// Folded again (its module was opened), unless the person opened it.
    pub fn fold(&mut self) {
        self.folded = None;
    }

    /// Folded or opened by hand: remembered, and brought into view.
    fn toggle(&mut self, cx: &mut Cx) {
        let open = self.folded();
        self.folded = Some(!open);
        host::set_fold(&self.key, open);
        self.reveal_open = open;
        self.view.redraw(cx);
    }
}

impl Widget for PanelGroup {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event.hits(cx, self.draw_head.area()) {
            Hit::FingerHoverIn(_) => {
                self.hot = true;
                cx.set_cursor(MouseCursor::Hand);
                self.draw_head.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hot = false;
                self.draw_head.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() => self.toggle(cx),
            Hit::KeyDown(ke) if matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space) => self.toggle(cx),
            _ => {}
        }
        if !self.folded() {
            self.view.handle_event(cx, event, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let kids: Vec<WidgetRef> = self.view.children.iter().map(|(_, w)| w.clone()).collect();
        let kid_paths: Vec<Vec<String>> = kids.iter().map(|k| super::tree::paths_of(k).0).collect();
        let paths: Vec<String> = kid_paths.iter().flatten().cloned().collect();
        // The search finds none of it: not drawn.
        if !host::found(&paths) {
            return DrawStep::done();
        }
        self.key = host::fold_key(&self.title, &paths);
        if self.folded.is_none() {
            self.folded = host::folded_by_hand(&self.key).map(|open| !open);
        }
        // The picture's selection changed to something it holds: opened.
        let reveal = host::reveal_rev();
        if self.reveal_seen.is_some_and(|r| r != reveal) && host::picked(&paths) {
            self.folded = Some(false);
        }
        self.reveal_seen = Some(reveal);
        let folded = self.folded();
        // A rounded inset holds the heading and, open, the controls: what
        // belongs to the group reads as one block inside its module.
        self.draw_box.shape = 0.0;
        self.draw_box.radius = R_GROUP;
        self.draw_box.stroke = 1.0;
        self.draw_box.fill = color(p.well, if folded { 0.35 } else { 0.55 });
        self.draw_box.edge = color(if self.hot { p.border } else { p.line_soft }, 1.0);
        self.draw_box.begin(cx, walk, Layout::flow_down());
        let head = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(26.0)));
        self.draw_head.fill_round(cx, head, color(0, 0.0), R_GROUP);
        let cy = head.pos.y + 13.0;
        self.draw_mark.chevron(cx, Rect { pos: dvec2(head.pos.x + 6.0, cy - 6.0), size: dvec2(12.0, 12.0) }, color(if self.hot { p.text } else { p.muted }, 1.0), !folded);
        self.draw_title.color = color(if self.hot { p.text_hi } else { p.secondary }, 1.0);
        let title = self.title.to_uppercase();
        self.draw_title.draw_abs(cx, dvec2(head.pos.x + 22.0, cy - 5.0), &title);
        let n: usize = if folded { kids.iter().map(|k| super::tree::paths_of(k).1).sum() } else { 0 };
        // (A group of read-only rows has no controls to count.)
        if folded && n > 0 {
            let said = if n == 1 { "1 control".to_string() } else { format!("{n} controls") };
            let w = advance(&self.draw_small, cx, &said);
            self.draw_small.color = color(p.dim_hi, 1.0);
            self.draw_small.draw_abs(cx, dvec2(head.pos.x + head.size.x - 10.0 - w, cy - 5.0), &said);
        }
        if !folded {
            cx.begin_turtle(Walk::new(Size::fill(), Size::fit()), self.view.layout.clone());
            for (k, ps) in kids.iter().zip(&kid_paths) {
                if host::found(ps) {
                    k.draw_all(cx, scope);
                }
            }
            cx.end_turtle();
        }
        self.draw_box.end(cx);
        if std::mem::take(&mut self.reveal_open) && !folded {
            host::opened(self.draw_box.area().rect(cx));
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- colour

#[derive(Script, ScriptHook, Widget)]
pub struct PanelColor {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[deref]
    view: View,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    /// The colour shown without a path.
    #[live]
    pub value: String,
    #[live]
    on_change: ScriptFnRef,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_head: DrawPanelShape,
    #[live]
    draw_swatch: DrawPanelShape,
    #[live]
    draw_text: DrawText,
    /// The key button.
    #[live]
    draw_key: DrawPanelKey,
    #[rust]
    key: KeyButton,
    #[rust]
    read_at: f64,
    #[rust]
    code: String,
    /// What the code stands for (a name's colour: `BONE` → `#eee9df`).
    #[rust]
    resolved: String,
    /// The palette the picker lists, and the name each is written as when
    /// picked (a colour a `let` names).
    #[rust]
    palette_refs: Vec<([f64; 4], Option<String>)>,
    /// A palette pick written as its name: the picker's commit of the same
    /// colour that follows is that pick.
    #[rust]
    picked_ref: Option<[f64; 4]>,
    #[rust]
    seen: u64,
    /// Opened by hand: brought into view once drawn open.
    #[rust]
    reveal_open: bool,
    #[rust]
    open: bool,
    #[rust]
    hot: bool,
    /// A gesture in the picker holds its own value.
    #[rust]
    moving: bool,
    /// The colour when it was opened (the swatch's old half).
    #[rust]
    old: Option<Vec4f>,
    /// The code the row's field shows.
    #[rust]
    hex_shown: String,
}

impl PanelColor {
    fn read(&mut self, cx: &mut Cx) {
        let rev = host::revision();
        // A keyed colour is its colour at the playhead.
        let head = if self.key.keyed() { host::playhead().map(|p| p.0) } else { None };
        let moved = head.is_some_and(|t| t != self.read_at);
        if let Some(t) = head {
            self.read_at = t;
        }
        if self.seen == rev && !self.code.is_empty() && !moved {
            return;
        }
        self.seen = rev;
        // The palette the open picker lists, as it is now (a colour of it
        // moved: this control's drag, another's, an undo).
        if self.open {
            self.give_palette(cx);
        }
        let found = host::read(&self.path).map(|v| v.code()).or_else(|| (!self.value.is_empty()).then(|| self.value.clone()));
        let Some(code) = found else { return };
        let resolved = named_value(&self.path, &code);
        if self.moving || (code == self.code && resolved == self.resolved) {
            return;
        }
        self.code = code;
        self.resolved = resolved;
        if let (Some(rgba), Some(mut picker)) = (parse_code_color(&self.resolved), self.view.widget(cx, ids!(picker)).borrow_mut::<HdrColorPicker>()) {
            picker.set_value(cx, rgba);
            picker.set_hdr_allowed(cx, self.resolved.trim_start().starts_with("color("));
        }
    }

    /// The scene's palette given to the picker (on opening it): each
    /// colour, and the name a pick writes where a `let` names it.
    fn give_palette(&mut self, cx: &mut Cx) {
        let swatches = host::palette("");
        self.palette_refs = swatches.iter().map(|w| (w.rgba, w.refer.clone())).collect();
        let entries = swatches.into_iter().map(|w| crate::color_picker::PaletteEntry { rgba: w.rgba, name: w.refer.clone().unwrap_or(w.name), uses: w.uses }).collect();
        if let Some(mut picker) = self.view.widget(cx, ids!(picker)).borrow_mut::<HdrColorPicker>() {
            picker.set_palette(cx, entries);
        }
    }

    /// A name written as the value (a palette colour picked).
    fn write_name(&mut self, cx: &mut Cx, name: &str) {
        self.code = name.to_string();
        self.resolved = named_value(&self.path, name);
        if host::hosted() && !self.path.is_empty() {
            host::set(&self.path, PanelValue::Code(self.code.clone()));
        }
        if host::bound(&self.on_change) {
            let arg = cx.with_vm(|vm| vm.bx.heap_mut().new_string_from_str(&self.code));
            cx.widget_to_script_call(self.widget_uid(), NIL, self.source.clone(), self.on_change.clone(), &[arg]);
        }
        self.draw_head.redraw(cx);
    }

    fn write(&mut self, cx: &mut Cx, rgba: [f64; 4]) {
        self.code = format_code_color(rgba);
        self.resolved = self.code.clone();
        if host::hosted() && !self.path.is_empty() {
            host::set(&self.path, PanelValue::Code(self.code.clone()));
        }
        if host::bound(&self.on_change) {
            let arg = cx.with_vm(|vm| vm.bx.heap_mut().new_string_from_str(&self.code));
            cx.widget_to_script_call(self.widget_uid(), NIL, self.source.clone(), self.on_change.clone(), &[arg]);
        }
        self.draw_head.redraw(cx);
    }
}

impl Widget for PanelColor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event.hits(cx, self.draw_head.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let on_key = self.key.contains(fe.abs);
                if !self.hot || on_key != self.key.hot {
                    self.hot = true;
                    self.key.hot = on_key;
                    self.draw_head.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerHoverOut(_) => {
                self.hot = false;
                self.key.hot = false;
                self.draw_head.redraw(cx);
            }
            Hit::FingerDown(fe) if !fe.is_primary_hit() => {
                if self.key.context() {
                    self.draw_head.redraw(cx);
                }
            }
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() && self.key.click(&self.path, fe.abs) => {
                self.draw_head.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() => {
                self.key.offering = false;
                self.open = !self.open;
                self.reveal_open = self.open;
                if self.open {
                    self.give_palette(cx);
                    self.old = parse_code_color(&self.resolved).map(|c| {
                        let m = c[0].max(c[1]).max(c[2]).max(1.0);
                        vec4((c[0] / m) as f32, (c[1] / m) as f32, (c[2] / m) as f32, c[3] as f32)
                    });
                    self.hex_shown.clear();
                }
                self.view.redraw(cx);
            }
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            // The code typed in the row: a colour, or a palette name.
            let hex = self.view.widget(cx, ids!(hex)).as_text_input();
            if let Some((text, _)) = hex.returned(actions) {
                let named = self.palette_refs.iter().find(|(_, n)| n.as_deref() == Some(text.trim())).cloned();
                if let Some((rgba, Some(name))) = named {
                    if let Some(mut picker) = self.view.widget(cx, ids!(picker)).borrow_mut::<HdrColorPicker>() {
                        picker.set_value(cx, rgba);
                    }
                    self.moving = false;
                    self.write_name(cx, &name);
                } else if let Some(rgba) = parse_code_color(&text) {
                    if let Some(mut picker) = self.view.widget(cx, ids!(picker)).borrow_mut::<HdrColorPicker>() {
                        picker.set_value(cx, rgba);
                    }
                    self.moving = false;
                    self.write(cx, rgba);
                }
                self.hex_shown.clear();
            }
            let picker = self.view.widget(cx, ids!(picker));
            let uid = picker.widget_uid();
            for action in actions.filter_widget_actions_cast::<ColorPickAction>(uid) {
                match action {
                    // A palette colour a `let` names: written as that name
                    // (the control then follows the palette).
                    ColorPickAction::Palette(i) => {
                        if let Some((rgba, Some(name))) = self.palette_refs.get(i).cloned() {
                            self.picked_ref = Some(rgba);
                            self.moving = false;
                            self.write_name(cx, &name);
                        }
                    }
                    ColorPickAction::Preview(rgba) => {
                        self.picked_ref = None;
                        self.moving = true;
                        self.write(cx, rgba);
                    }
                    ColorPickAction::Commit(rgba) if self.picked_ref.take() == Some(rgba) => {}
                    ColorPickAction::Commit(rgba) => {
                        self.moving = false;
                        self.write(cx, rgba);
                    }
                    _ => {}
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.read(cx);
        host::register(host::Bound { path: self.path.clone(), kind: host::BoundKind::Colour });
        cx.begin_turtle(walk, Layout::flow_down());
        let row = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(ROW)));
        let label = if self.label.is_empty() { host::label_of(&self.path) } else { self.label.clone() };
        let fill = parse_code_color(&self.resolved).map(|c| {
            let m = c[0].max(c[1]).max(c[2]).max(1.0);
            vec4((c[0] / m) as f32, (c[1] / m) as f32, (c[2] / m) as f32, c[3] as f32)
        });
        self.key.refresh(&self.path);
        let label_in = if self.key.state.is_some() { 14.0 } else { 0.0 };
        // Wide enough, the open row's code is its field and its swatch shows
        // old and new: the picker under it shows neither again.
        let inline = self.open && row.size.x >= LABEL * 2.0 + 40.0;
        draw_row(cx, &mut self.draw_head, &mut self.draw_swatch, &mut self.draw_text, row, label_in, &label, fill, if inline { "" } else { &self.code }, self.open, self.hot);
        if inline {
            let sw = Rect { pos: dvec2(row.pos.x + LABEL + 8.0, row.pos.y + 4.0), size: dvec2(22.0, ROW - 8.0) };
            if let (Some(old), Some(new)) = (self.old, fill) {
                if old != new {
                    let edge = color(p_border(cx), 1.0);
                    self.draw_swatch.edged_round(cx, Rect { pos: sw.pos, size: dvec2(11.0, sw.size.y) }, old, edge, 1.0, 3.0);
                }
            }
            let shown = self.code.clone();
            if shown != self.hex_shown {
                self.hex_shown = shown.clone();
                self.view.widget(cx, ids!(hex)).set_text(cx, &shown);
            }
            // The field fits between the swatch and the chevron.
            let field_w = (row.pos.x + row.size.x - 22.0 - (sw.pos.x + 30.0)).clamp(48.0, 132.0);
            let _ = self.view.widget(cx, ids!(hex)).draw_walk(cx, scope, Walk::new(Size::Fixed(field_w), Size::Fixed(20.0)).with_abs_pos(dvec2(sw.pos.x + 30.0, row.pos.y + 2.0)));
        }
        // The offer stays in the label's column (the swatch is drawn over what
        // reaches it).
        let column = Rect { pos: row.pos, size: dvec2(LABEL - 4.0, row.size.y) };
        draw_key(cx, &mut self.key, dvec2(row.pos.x, row.pos.y + 7.0), column, &mut self.draw_key, &mut self.draw_text);
        if self.key.keyed() && host::playhead().is_some_and(|p| p.2) {
            cx.redraw_area_in_draw(self.draw_head.area());
        }
        if self.open {
            if let Some(mut picker) = self.view.widget(cx, ids!(picker)).borrow_mut::<HdrColorPicker>() {
                if picker.shows_value() == inline {
                    picker.set_show_value(cx, !inline);
                }
            }
            let _ = self.view.widget(cx, ids!(inset)).draw_walk(cx, scope, Walk::new(Size::fill(), Size::fit()));
        }
        let placed_rect = cx.end_turtle();
        if std::mem::take(&mut self.reveal_open) && self.open {
            host::opened(placed_rect);
        }
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, placed_rect, paths);
        DrawStep::done()
    }
}

fn p_border(cx: &mut Cx2d) -> u32 {
    makepad_look::palette::get(cx).border
}

// ---------------------------------------------------------------- words

#[derive(Script, ScriptHook, Widget)]
pub struct PanelText {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[deref]
    view: View,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    #[live]
    pub value: String,
    #[live]
    on_change: ScriptFnRef,
    #[source]
    source: ScriptObjectRef,
    #[rust]
    seen: u64,
    #[rust]
    shown: String,
}

/// A string literal's words (`"see"` → see), else the text as written.
fn words_of(code: &str) -> String {
    match tree::parse_value(code.trim()).map(|n| n.kind) {
        Ok(Kind::Str(s)) => s,
        _ => code.to_string(),
    }
}

impl Widget for PanelText {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if let Some(text) = self.view.widget(cx, ids!(input)).as_text_input().changed(actions) {
                self.shown = text.clone();
                let code = makepad_widgets::makepad_script::syntax::write::quote(&text);
                if host::hosted() && !self.path.is_empty() {
                    host::set(&self.path, PanelValue::Code(code));
                }
                if host::bound(&self.on_change) {
                    let arg = cx.with_vm(|vm| vm.bx.heap_mut().new_string_from_str(&text));
                    cx.widget_to_script_call(self.widget_uid(), NIL, self.source.clone(), self.on_change.clone(), &[arg]);
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev {
            self.seen = rev;
            let found = host::read(&self.path).map(|v| words_of(&v.code())).or_else(|| (!self.value.is_empty()).then(|| self.value.clone()));
            if let Some(words) = found {
                if words != self.shown {
                    self.shown = words.clone();
                    self.view.widget(cx, ids!(input)).set_text(cx, &words);
                }
            }
            let label = if self.label.is_empty() { host::label_of(&self.path) } else { self.label.clone() };
            self.view.widget(cx, ids!(name)).set_text(cx, &label);
        }
        cx.begin_turtle(walk, Layout::flow_down());
        let _ = self.view.draw_walk(cx, scope, Walk::new(Size::fill(), Size::fit()));
        let placed_rect = cx.end_turtle();
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, placed_rect, paths);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- function slot

#[derive(Script, ScriptHook, Widget)]
pub struct PanelFn {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    pub path: String,
    #[live]
    pub label: String,
    /// The names offered without a host (separated by spaces or commas).
    #[live]
    pub choices: String,
    #[live]
    on_change: ScriptFnRef,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_chip: DrawPanelShape,
    #[live]
    draw_text: DrawText,
    #[rust]
    current: String,
    #[rust]
    names: Vec<String>,
    #[rust]
    seen: u64,
    #[rust]
    spots: Vec<(Rect, usize)>,
    #[rust]
    hot: Option<usize>,
}

impl Widget for PanelFn {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let at = |s: &Self, abs: DVec2| s.spots.iter().find(|(r, _)| r.contains(abs)).map(|(_, i)| *i);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = at(self, fe.abs);
                cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() => {
                let Some(i) = at(self, fe.abs) else { return };
                let Some(name) = self.names.get(i).cloned() else { return };
                self.current = name.clone();
                if host::hosted() && !self.path.is_empty() {
                    host::swap(&self.path, &name);
                }
                if host::bound(&self.on_change) {
                    let arg = cx.with_vm(|vm| vm.bx.heap_mut().new_string_from_str(&name));
                    cx.widget_to_script_call(self.uid, NIL, self.source.clone(), self.on_change.clone(), &[arg]);
                }
                self.draw_bg.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rev = host::revision();
        if self.seen != rev {
            self.seen = rev;
            self.current = host::read(&self.path).map(|v| v.code().trim().to_string()).unwrap_or_default();
            let mut names = host::fns_like(&self.path);
            names.extend(self.choices.split([' ', ',']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string));
            if !self.current.is_empty() && !names.contains(&self.current) {
                names.insert(0, self.current.clone());
            }
            names.dedup();
            self.names = names;
        }
        let p = makepad_look::palette::get(cx);
        // The chips' rows, measured before the walk.
        let width = (cx.turtle().inner_width() - LABEL - 8.0).max(40.0);
        let mut layout = Vec::new();
        let (mut x, mut y) = (0.0, 0.0);
        for name in &self.names {
            let w = advance(&self.draw_text, cx, name) + 14.0;
            if x + w > width && x > 0.0 {
                x = 0.0;
                y += ROW;
            }
            layout.push((x, y, w));
            x += w + 4.0;
        }
        let rect = cx.walk_turtle(Walk { height: Size::Fixed(y + ROW), ..walk });
        let (uid, paths) = (self.widget_uid().0, vec![self.path.clone()]);
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.draw_abs(cx, rect);
        self.spots.clear();
        let label = if self.label.is_empty() { host::label_of(&self.path) } else { self.label.clone() };
        self.draw_text.color = color(p.label, 1.0);
        let (label, _) = fit_text(&self.draw_text, cx, &label, LABEL - 4.0);
        self.draw_text.draw_abs(cx, dvec2(rect.pos.x, rect.pos.y + 6.0), &label);
        for (i, (x, y, w)) in layout.into_iter().enumerate() {
            let r = Rect { pos: dvec2(rect.pos.x + LABEL + 8.0 + x, rect.pos.y + y + 2.0), size: dvec2(w, ROW - 4.0) };
            let on = self.names[i] == self.current;
            let look = p.pick(on, self.hot == Some(i), p.raised);
            self.draw_chip.edged_round(cx, r, color(look.fill, 1.0), color(look.edge, 1.0), 1.0, R_CONTROL);
            self.draw_text.color = color(look.ink, 1.0);
            self.draw_text.draw_abs(cx, dvec2(r.pos.x + 7.0, r.pos.y + 4.0), &self.names[i]);
            self.spots.push((r, i));
        }
        DrawStep::done()
    }
}

//! Compact property numbers: press arms, horizontal travel scrubs, click types.
//! Shift refines, Ctrl snaps, modifiers and limits re-anchor; Escape restores.
//! Changed follows live edits; Ended marks each completed gesture. Track mode
//! retains absolute pointer mapping. Tab opens the next field in draw order.

use crate::{
    badge::advance, focus_order, makepad_derive_widget::*, makepad_draw::{event::TouchState, *},
    text_input::*, widget::*,
};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ValueInputAction {
    Changed(f64),
    Ended(f64),
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ValueInputFormat {
    #[pick]
    Number = 0,
    HoursMinutes = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ValueInputJoin {
    None = 0,
    Top = 1,
    Bottom = 2,
    #[pick]
    Both = 3,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.ValueInputJoin = #(ValueInputJoin::script_api(vm))
    mod.widgets.ValueInputFormat = #(ValueInputFormat::script_api(vm))
    use mod.widgets.*

    set_type_default() do #(DrawValueInput::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawValueInputArrows::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.ValueInputBase = #(ValueInput::register_widget(vm))
    mod.widgets.ValueInput = set_type_default() do mod.widgets.ValueInputBase{
        width: 76
        height: theme.prop_row_height
        arrow_zone: theme.prop_arrow_zone
        pad: theme.prop_pad
        arrow_color: theme.prop_ink_dim
        draw_bg +: {
            color: uniform(theme.prop_field)
            color_hover: uniform(theme.prop_field_hover)
            color_drag: uniform(theme.prop_field_active)
            color_focus: uniform(theme.color_focus)
            color_marked: uniform(theme.color_focus)
            fill_color: uniform(theme.color_u_3)
            radius: uniform(theme.prop_radius)
            vertex: fn() {
                let dpi = self.draw_pass.dpi_factor
                let shift = self.draw_list.view_shift
                let start = floor((self.rect_pos + shift) * dpi + vec2(0.5)) / dpi - shift
                let end = floor((self.rect_pos + self.rect_size + shift) * dpi + vec2(0.5)) / dpi - shift
                self.vertex_pos = self.clip_and_transform_vertex(start, end - start)
            }
            pixel: fn() {
                let sdf = Sdf2d.viewport_px(self.pos * self.rect_size, self.draw_pass.dpi_factor)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let hairline = 1.0 / self.draw_pass.dpi_factor
                let inset = hairline * 0.5
                let top = self.radius * self.round_top
                let bottom = self.radius * self.round_bottom
                let base = self.color.mix(self.color_hover, self.hover).mix(self.color_drag, self.drag)
                sdf.box_y(inset, inset, w - hairline, h - hairline, top, bottom)
                let reveal = mix(1.0, max(self.hover, max(self.drag, self.focus)), self.flat)
                sdf.fill_keep(vec4(base.rgb, base.a * reveal))
                if self.fill >= 0.0 {
                    let fill_x = 1.0 + (w - 2.0) * self.fill
                    let wash = vec4(self.fill_color.xyz, self.fill_color.w * step(self.pos.x * w, fill_x))
                    sdf.fill_keep(wash)
                }
                sdf.fill_keep(vec4(self.color_marked.rgb, self.color_marked.a * self.marked * 0.18))
                if self.focus > 0.5 {
                    // SDF strokes take a half-width.
                    sdf.stroke(self.color_focus, hairline * 0.5)
                }
                return sdf.result
            }
        }
        draw_arrows +: {
            color_hover: uniform(theme.color_text)
            chevron: uniform(theme.prop_chevron)
            vertex: fn() {
                let dpi = self.draw_pass.dpi_factor
                let shift = self.draw_list.view_shift
                let start = floor((self.rect_pos + shift) * dpi + vec2(0.5)) / dpi - shift
                let end = floor((self.rect_pos + self.rect_size + shift) * dpi + vec2(0.5)) / dpi - shift
                self.vertex_pos = self.clip_and_transform_vertex(start, end - start)
            }
            pixel: fn() {
                let sdf = Sdf2d.viewport_px(self.pos * self.rect_size, self.draw_pass.dpi_factor)
                let w = self.rect_size.x
                let cy = self.rect_size.y * 0.5
                let half = self.chevron * 0.5
                sdf.move_to(5.0 + half, cy - half)
                sdf.line_to(5.0, cy)
                sdf.line_to(5.0 + half, cy + half)
                sdf.stroke(self.color.mix(self.color_hover, self.left), 1.25 * 0.5)
                sdf.move_to(w - 5.0 - half, cy - half)
                sdf.line_to(w - 5.0, cy)
                sdf.line_to(w - 5.0 - half, cy + half)
                sdf.stroke(self.color.mix(self.color_hover, self.right), 1.25 * 0.5)
                return sdf.result
            }
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
        draw_label +: {
            color: theme.prop_ink_dim
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
        text_input: TextInput{
            width: Fill height: Fill min_height: 0
            margin: Inset{}
            padding: Inset{}
            flow: Right
            label_align: Align{x: 0.5 y: 0.0}
            empty_text: ""
            keep_focus_on_submit: true
            draw_bg +: {
                color: #0000 color_hover: #0000 color_focus: #0000
                color_down: #0000 color_empty: #0000 color_disabled: #0000
                color_2: #0000 color_2_hover: #0000 color_2_focus: #0000
                color_2_down: #0000 color_2_empty: #0000 color_2_disabled: #0000
                border_color: #0000 border_color_hover: #0000 border_color_focus: #0000
                border_color_down: #0000 border_color_empty: #0000 border_color_disabled: #0000
                border_color_2: #0000 border_color_2_hover: #0000 border_color_2_focus: #0000
                border_color_2_down: #0000 border_color_2_empty: #0000 border_color_2_disabled: #0000
                border_size: 0.0 border_radius: 0.0
                pixel: fn() { return vec4(0.0) }
            }
            draw_text +: {
                ink_centered: false
                color: theme.color_text
                color_hover: theme.color_text color_focus: theme.color_text
                color_down: theme.color_text color_disabled: theme.color_text
                text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
                get_color: fn() { return self.color }
            }
            draw_selection +: {
                border_radius: 0.0
                pixel: fn() {
                    return vec4(theme.color_selection_focus.rgb * theme.color_selection_focus.a,
                        theme.color_selection_focus.a)
                }
            }
            draw_cursor +: {
                border_radius: 0.0
                pixel: fn() {
                    let dpi = self.draw_pass.dpi_factor
                    let shift = self.draw_list.view_shift.x
                    let origin = self.rect_pos.x + shift
                    let left = max(self.draw_clip.x + shift, self.draw_list.view_clip.x)
                    let right = min(self.draw_clip.z + shift, self.draw_list.view_clip.z)
                    let column = floor((origin + self.rect_size.x * 0.5) * dpi) / dpi
                    let x = clamp(column, ceil(left * dpi) / dpi, floor(right * dpi) / dpi - 1.0 / dpi) - origin
                    let sdf = Sdf2d.viewport_px(self.pos * self.rect_size, dpi)
                    sdf.rect(x, 0.0, 1.0 / dpi, self.rect_size.y)
                    sdf.fill(vec4(self.color.rgb, self.color.a * (1.0 - self.blink) * self.focus))
                    return sdf.result
                }
            }
        }
    }
}

const DRAG_THRESHOLD: f64 = 3.0;
const TRACK_INSET: f64 = 1.0;

pub fn track_span(width: f64) -> (f64, f64) {
    (TRACK_INSET, (width - TRACK_INSET).max(TRACK_INSET + 1.0))
}

pub fn track_value_at(x: f64, lo: f64, hi: f64, min: f64, max: f64) -> f64 {
    min + ((x - lo) / (hi - lo).max(1.0)).clamp(0.0, 1.0) * (max - min)
}

pub fn track_press_means_typing(tap_count: u32) -> bool {
    tap_count >= 2
}

#[derive(Clone, Copy)]
struct DragParams {
    min: f64,
    max: f64,
    step: f64,
    wrap: bool,
    bounded: bool,
    snap_override: f64,
}

#[derive(Clone, Copy)]
struct DragAnchor {
    x: f64,
    value: f64,
}

// The relative drag mapping, shared by bounded and unbounded fields.
fn drag_rate(p: DragParams, width: f64, shift: bool) -> f64 {
    if p.bounded && p.max > p.min {
        (p.max - p.min) / width.max(1.0) * if shift { 0.05 } else { 1.0 }
    } else {
        p.step * if shift { 0.1 } else { 1.0 }
    }
}

fn snap_increment(p: DragParams, fine: bool) -> f64 {
    let range = if p.max > p.min { p.max - p.min } else { 21.0 };
    let base = if p.snap_override > 0.0 { p.snap_override }
        else if range < 2.1 { 0.1 } else if range < 21.0 { 1.0 } else { 10.0 };
    base * if fine { 0.1 } else { 1.0 }
}

fn drag_map(p: DragParams, mut anchor: DragAnchor, x: f64, width: f64, shift: bool, ctrl: bool)
    -> (f64, DragAnchor)
{
    let raw = anchor.value + (x - anchor.x) * drag_rate(p, width, shift);
    let ranged = if p.max <= p.min { raw } else if p.wrap {
        p.min + (raw - p.min).rem_euclid(p.max - p.min)
    } else { raw.clamp(p.min, p.max) };
    if (ranged - raw).abs() > f64::EPSILON {
        anchor = DragAnchor { x, value: ranged };
    }
    let mut publish = ranged;
    if ctrl {
        let inc = snap_increment(p, shift);
        publish = (ranged / inc).round() * inc;
        if p.max > p.min && !p.wrap {
            publish = publish.clamp(p.min, p.max);
            if ranged <= p.min { publish = p.min; }
            else if ranged >= p.max { publish = p.max; }
        }
    }
    (publish, anchor)
}

#[derive(Clone, Copy)]
struct DragState {
    press_x: f64,
    press_value: f64,
    width: f64,
    engaged: bool,
    anchor: DragAnchor,
    last_x: f64,
    shift: bool,
    ctrl: bool,
    relative_track: bool,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawValueInput {
    #[deref] draw_super: DrawQuad,
    #[live] fill: f32,
    #[live] hover: f32,
    #[live] drag: f32,
    #[live] focus: f32,
    #[live] marked: f32,
    #[live] round_top: f32,
    #[live] round_bottom: f32,
    #[live] flat: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawValueInputArrows {
    #[deref] draw_super: DrawQuad,
    #[live] color: Vec4f,
    #[live] left: f32,
    #[live] right: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ValueInput {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[live(true)] #[visible] visible: bool,
    #[layout] layout: Layout,
    #[redraw] #[live] draw_bg: DrawValueInput,
    #[live] draw_text: DrawText,
    #[live] draw_arrows: DrawValueInputArrows,
    #[live] pub arrow_color: Vec4f,
    #[live] draw_label: DrawText,
    #[live] text_input: TextInput,
    #[live(0.0)] pub min: f64,
    #[live(1.0)] pub max: f64,
    #[live(0.1)] pub step: f64,
    #[live(1.0)] pub precision: f64,
    #[live] pub label: String,
    #[live] pub suffix: String,
    #[live] pub track: bool,
    #[live] pub show_fill: bool,
    #[live] pub wrap: bool,
    #[live] pub snap: f64,
    #[live] pub read_only: bool,
    #[live] pub mixed: bool,
    #[live] pub marked: bool,
    #[live] pub inherited: bool,
    #[live] pub placeholder: String,
    #[live] pub placeholder_value: f64,
    #[live] pub format: ValueInputFormat,
    #[live] pub join: ValueInputJoin,
    #[live] arrow_zone: f64,
    #[live] pad: f64,
    #[live] value: f64,
    #[rust] editing: bool,
    #[rust] edit_focus: bool,
    #[rust] hovered: bool,
    #[rust] hover_zone: i32,
    #[rust] drag: Option<DragState>,
    #[rust] track_press_value: f64,
    #[rust] editor_layout: Option<(f64, f64, f64, f64)>,
}

impl ValueInput {
    fn params(&self) -> DragParams {
        DragParams { min: self.min, max: self.max, step: self.step, wrap: self.wrap,
            bounded: self.show_fill || self.track, snap_override: self.snap }
    }

    fn normalize(&self, value: f64) -> f64 {
        if self.max <= self.min { value }
        else if self.wrap { self.min + (value - self.min).rem_euclid(self.max - self.min) }
        else { value.clamp(self.min, self.max) }
    }

    fn format(&self) -> String {
        if self.mixed { return "—".to_string(); }
        if self.value == self.placeholder_value && !self.placeholder.is_empty() { return self.placeholder.clone(); }
        if self.format == ValueInputFormat::HoursMinutes {
            let minute = (self.value * 60.0).round().rem_euclid(1440.0) as u32;
            return format!("{:02}:{:02}", minute / 60, minute % 60);
        }
        format!("{:.*}{}", self.precision.max(0.0) as usize, self.value, self.suffix)
    }

    fn parse(&self, text: &str) -> Option<f64> {
        let text = text.trim();
        if self.format == ValueInputFormat::HoursMinutes {
            if let Some((hours, minutes)) = text.split_once(':') {
                let hours = hours.trim().parse::<u32>().ok()?;
                let minutes = minutes.trim().parse::<u32>().ok()?;
                return (minutes < 60).then_some(hours as f64 + minutes as f64 / 60.0);
            }
        }
        text.strip_suffix(self.suffix.trim()).unwrap_or(text).trim().parse().ok()
    }

    pub fn value(&self) -> f64 { self.value }
    pub fn is_editing(&self) -> bool { self.editing }
    pub fn is_interacting(&self) -> bool { self.editing || self.drag.is_some() }

    pub fn set_read_only(&mut self, cx: &mut Cx, read_only: bool) {
        if self.read_only != read_only {
            self.read_only = read_only;
            self.sync_read_only(cx);
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        if self.is_interacting() || !value.is_finite() { return; }
        let value = self.normalize(value);
        if value != self.value {
            self.value = value;
            self.draw_bg.redraw(cx);
        }
    }

    fn commit(&mut self, cx: &mut Cx, uid: WidgetUid, value: f64) {
        if !value.is_finite() { return; }
        self.value = self.normalize(value);
        self.mixed = false;
        cx.widget_action(uid, ValueInputAction::Changed(self.value));
        self.draw_bg.redraw(cx);
    }

    fn ended(&self, cx: &mut Cx, uid: WidgetUid) {
        cx.widget_action(uid, ValueInputAction::Ended(self.value));
    }

    fn step_once(&mut self, cx: &mut Cx, direction: f64, shift: bool) {
        let step = self.step * if shift { 0.1 } else { 1.0 };
        self.commit(cx, self.uid, self.value + direction * step);
        self.ended(cx, self.uid);
    }

    fn track_value(&self, cx: &Cx, abs_x: f64) -> f64 {
        let face = self.draw_bg.area().rect(cx);
        let (lo, hi) = track_span(face.size.x);
        let raw = track_value_at(abs_x - face.pos.x, lo, hi, self.min, self.max);
        self.normalize(if self.step > 0.0 { (raw / self.step).round() * self.step } else { raw })
    }

    fn type_here(&mut self, cx: &mut Cx, uid: WidgetUid) {
        self.drag = None;
        self.commit(cx, uid, self.track_press_value);
        self.ended(cx, uid);
        self.enter_edit(cx);
    }

    pub fn begin_edit(&mut self, cx: &mut Cx) {
        if !self.read_only { self.enter_edit(cx); }
    }

    fn enter_edit(&mut self, cx: &mut Cx) {
        self.drag = None;
        self.editing = true;
        self.edit_focus = true;
        self.editor_layout = None;
        let text = if self.format == ValueInputFormat::HoursMinutes { self.format() }
            else { self.value.to_string() };
        self.text_input.set_text(cx, &text);
        self.text_input.take_key_focus(cx);
        self.text_input.select_all(cx);
        self.draw_bg.redraw(cx);
    }

    fn exit_edit(&mut self, cx: &mut Cx) {
        self.editing = false;
        self.edit_focus = false;
        self.draw_bg.redraw(cx);
    }

    fn commit_edit(&mut self, cx: &mut Cx) {
        if let Some(value) = self.parse(&self.text_input.text()) {
            if value.is_finite() {
                self.commit(cx, self.uid, value);
                self.ended(cx, self.uid);
            }
        }
        self.exit_edit(cx);
    }

    fn cancel_drag(&mut self, cx: &mut Cx) {
        if let Some(drag) = self.drag.take() {
            if drag.engaged {
                self.commit(cx, self.uid, drag.press_value);
                self.ended(cx, self.uid);
            }
            self.draw_bg.redraw(cx);
        }
    }

    fn zone(&self, x: f64, width: f64) -> i32 {
        let band = self.arrow_zone.min(width / 3.0);
        if x < band { -1 } else if x > width - band { 1 } else { 0 }
    }

    fn hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        let rect = self.draw_bg.area().rect(cx);
        let zone = self.zone(abs.x - rect.pos.x, rect.size.x);
        if !self.hovered || self.hover_zone != zone {
            self.hovered = true;
            self.hover_zone = zone;
            self.draw_bg.redraw(cx);
        }
        cx.set_cursor(MouseCursor::Hand);
    }

    fn sync_read_only(&mut self, cx: &mut Cx) {
        if self.read_only {
            if self.key_focus(cx) { cx.set_key_focus(Area::Empty); }
            self.cancel_drag(cx);
            if self.editing { self.exit_edit(cx); }
            self.hovered = false;
        }
    }

    fn reanchor_modifiers(&mut self, modifiers: KeyModifiers) {
        if let Some(drag) = &mut self.drag {
            let ctrl = modifiers.control || modifiers.logo;
            if modifiers.shift != drag.shift || ctrl != drag.ctrl {
                drag.shift = modifiers.shift;
                drag.ctrl = ctrl;
                drag.anchor = DragAnchor { x: drag.last_x, value: self.value };
                drag.relative_track = true;
            }
        }
    }
}

impl Widget for ValueInput {
    fn text(&self) -> String { self.format() }

    fn key_focus(&self, cx: &Cx) -> bool {
        (!self.draw_bg.area().is_empty() && cx.has_key_focus(self.draw_bg.area()))
            || (self.editing && !self.text_input.area().is_empty() && cx.has_key_focus(self.text_input.area()))
    }

    fn handle_action(&mut self, cx: &mut Cx, request: &a11y_tree::ActionRequest,
        _scope: &mut Scope) -> a11y_tree::ActionResult
    {
        use a11y_tree::{ActionArg, ActionResult, Actions};
        if self.read_only || self.disabled(cx) { return ActionResult::Disabled; }
        let value = match (request.action, &request.arg) {
            (Actions::SET_VALUE, ActionArg::Number(value)) => *value,
            (Actions::INCREMENT, _) => self.value + self.step,
            (Actions::DECREMENT, _) => self.value - self.step,
            _ => return ActionResult::Unhandled,
        };
        if !value.is_finite() { return ActionResult::Unsupported; }
        self.commit(cx, self.uid, value);
        self.ended(cx, self.uid);
        ActionResult::Handled
    }

    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        node.role = a11y_tree::Role::SpinButton;
        makepad_platform::a11y::set_string(&mut node.name, &self.label);
        node.value = self.format();
        node.states = a11y_tree::States::FOCUSABLE;
        node.states.set(a11y_tree::States::DISABLED, self.disabled(cx));
        node.states.set(a11y_tree::States::READ_ONLY, self.read_only);
        if !self.read_only {
            node.actions = a11y_tree::Actions::FOCUS | a11y_tree::Actions::SET_VALUE
                | a11y_tree::Actions::INCREMENT | a11y_tree::Actions::DECREMENT;
        }
        node.range = Some(a11y_tree::RangeValue { min: self.min, max: self.max,
            now: self.value, step: self.step });
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible { return DrawStep::done(); }
        self.sync_read_only(cx);
        if !self.read_only && focus_order::take_request(cx, self.draw_bg.area()) {
            self.enter_edit(cx);
        }
        cx.begin_turtle(walk, self.layout);
        let mut face = cx.turtle().rect();
        let dpi = cx.current_dpi_factor();
        let end = (face.pos + face.size) * dpi;
        face.pos = dvec2((face.pos.x * dpi).round(), (face.pos.y * dpi).round()) / dpi;
        face.size = dvec2(end.x.round(), end.y.round()) / dpi - face.pos;
        self.draw_bg.fill = if (self.track || self.show_fill) && self.max > self.min {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0) as f32
        } else { -1.0 };
        self.draw_bg.hover = (self.hovered && !self.read_only) as u8 as f32;
        self.draw_bg.drag = self.drag.is_some_and(|drag| drag.engaged) as u8 as f32;
        self.draw_bg.focus = self.editing as u8 as f32;
        self.draw_bg.marked = self.marked as u8 as f32;
        (self.draw_bg.round_top, self.draw_bg.round_bottom) = match self.join {
            ValueInputJoin::None => (0.0, 0.0), ValueInputJoin::Top => (1.0, 0.0),
            ValueInputJoin::Bottom => (0.0, 1.0), ValueInputJoin::Both => (1.0, 1.0),
        };
        self.draw_bg.draw_abs(cx, face);
        let size = self.draw_text.text_style.font_size as f64;
        // Share DrawText's cap-height centering with labels and choice faces.
        let line = self.draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), "0");
        let scale = self.draw_text.font_scale as f64;
        let shift = line.ink_center_offset_in_lpxs() as f64 * scale;
        let y = face.pos.y + (face.size.y - line.size_in_lpxs.height as f64 * scale) * 0.5 + shift;
        let edge = if self.track { self.pad.min(face.size.x * 0.5) }
            else { self.arrow_zone.min(face.size.x / 3.0) };
        let text = self.format();
        let width = advance(&self.draw_text, cx, &text);
        let labelled = !self.label.is_empty() && !self.mixed;
        let left = if labelled {
            let label_width = (face.size.x - edge * 2.0 - width - self.pad).max(0.0);
            let line = self.draw_label.layout(cx, 0.0, 0.0, None, false, Align::default(), &self.label);
            let scale = self.draw_label.font_scale as f64;
            let label_y = face.pos.y + (face.size.y - line.size_in_lpxs.height as f64 * scale) * 0.5
                + line.ink_center_offset_in_lpxs() as f64 * scale;
            cx.push_clip_rect(rect(face.pos.x + edge, face.pos.y, label_width, face.size.y));
            self.draw_label.draw_abs(cx, dvec2(face.pos.x + edge, label_y), &self.label);
            cx.pop_clip_rect();
            edge + label_width + self.pad
        } else { edge };
        cx.push_clip_rect(rect(face.pos.x + left, face.pos.y,
            (face.size.x - left - edge).max(0.0), face.size.y));
        if self.editing {
            // Match the display's measured line origin, including selection/caret geometry.
            // Single-line inputs lay out at intrinsic width; padding carries the alignment.
            let edit_width = advance(&self.draw_text, cx, &self.text_input.text());
            let spare = (face.size.x - left - edge - edit_width).max(0.0);
            let align = if labelled { 1.0 } else { 0.5 };
            let edit_left = left + spare * align;
            let edit_right = edge + spare * (1.0 - align);
            let geometry = (edit_left, edit_right, shift, size);
            if self.editor_layout != Some(geometry) {
                self.editor_layout = Some(geometry);
                let line_spacing = self.draw_text.text_style.line_spacing;
                let top_drop = self.draw_text.text_style.top_drop;
                let color = self.draw_text.color;
                script_apply_eval!(cx, self.text_input, {
                    padding: mod.turtle.Inset{left: #(edit_left) right: #(edit_right) top: #(shift) bottom: #(-shift)}
                    label_align: mod.turtle.Align{x: 0.0 y: 0.0}
                    draw_text +: {
                        color: #(color)
                        text_style +: {font_size: #(size) line_spacing: #(line_spacing) top_drop: #(top_drop)}
                    }
                });
            }
            let _ = self.text_input.draw_walk(cx, scope, Walk {
                abs_pos: Some(face.pos), width: Size::Fixed(face.size.x),
                height: Size::Fixed(face.size.y), ..Walk::default()
            });
            if self.edit_focus && !self.text_input.area().is_empty() {
                self.edit_focus = false;
                self.text_input.take_key_focus(cx);
            }
        } else {
            let color = self.draw_text.color;
            if self.mixed || self.inherited || self.read_only || (self.value == self.placeholder_value && !self.placeholder.is_empty()) { self.draw_text.color = self.draw_label.color; }
            let x = if labelled { face.pos.x + face.size.x - edge - width }
                else { face.pos.x + (face.size.x - width) * 0.5 };
            self.draw_text.draw_abs(cx, dvec2(x, y), &text);
            self.draw_text.color = color;
        }
        cx.pop_clip_rect();
        if self.hovered && !self.editing && !self.read_only && !self.track {
            self.draw_arrows.color = self.arrow_color;
            self.draw_arrows.left = (self.hover_zone < 0) as u8 as f32;
            self.draw_arrows.right = (self.hover_zone > 0) as u8 as f32;
            self.draw_arrows.draw_abs(cx, face);
        }
        cx.end_turtle();
        if !self.read_only {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::Slider, Inset::default());
            focus_order::register(cx, self.draw_bg.area(), id!(property_number));
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible { return; }
        self.sync_read_only(cx);
        if self.read_only { return; }
        if matches!(event, Event::KeyFocus(_)) && focus_order::take_request(cx, self.draw_bg.area()) {
            self.enter_edit(cx);
            return;
        }
        if self.drag.is_some() {
            match event {
                Event::KeyDown(key) if key.key_code == KeyCode::Escape => {
                    self.cancel_drag(cx); return;
                }
                Event::MouseDown(mouse) if mouse.button.is_secondary() => {
                    self.cancel_drag(cx); return;
                }
                Event::KeyDown(key) | Event::KeyUp(key) => {
                    self.reanchor_modifiers(key.modifiers);
                    return;
                }
                _ => {}
            }
        }
        if let Event::Scroll(e) = event {
            if self.key_focus(cx) && self.drag.is_none() && !e.handled_y.get()
                && e.scroll.y != 0.0 && self.draw_bg.area().clipped_rect(cx).contains(e.abs)
            {
                if self.editing {
                    if let Some(value) = self.parse(&self.text_input.text()) {
                        if value.is_finite() { self.value = self.normalize(value); }
                    }
                    self.exit_edit(cx);
                    cx.set_key_focus(self.draw_bg.area());
                }
                self.step_once(cx, -e.scroll.y.signum(), e.modifiers.shift);
                e.handled_y.set(true);
            }
            return;
        }
        if self.editing {
            if let Event::KeyDown(key) = event {
                if self.key_focus(cx) && key.key_code == KeyCode::Tab {
                    self.commit_edit(cx);
                    focus_order::advance(cx, self.draw_bg.area(), id!(property_number), key.modifiers.shift);
                    return;
                }
            }
            for action in cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope)) {
                match action.as_widget_action().cast() {
                    TextInputAction::Returned(_, _) => {
                        self.commit_edit(cx); cx.set_key_focus(self.draw_bg.area());
                    }
                    TextInputAction::Escaped => {
                        self.exit_edit(cx); cx.set_key_focus(self.draw_bg.area());
                    }
                    TextInputAction::KeyFocusLost if self.editing => self.commit_edit(cx),
                    _ => {}
                }
            }
            let press = match event {
                Event::MouseDown(_) => true,
                Event::TouchUpdate(touches) => touches.touches.iter().any(|touch| touch.state == TouchState::Start),
                _ => false,
            };
            if self.editing && press {
                if let Hit::FingerDown(finger) = event.hits(cx, self.text_input.area()) {
                    if finger.device.is_primary_hit() && finger.tap_count >= 2 {
                        self.text_input.select_all(cx);
                    }
                }
            }
            return;
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => self.hover(cx, fe.abs),
            Hit::FingerHoverOut(_) => {
                self.hovered = false; self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                if fe.tap_count >= 2 {
                    if self.track { self.type_here(cx, self.uid); }
                    else { self.enter_edit(cx); }
                    return;
                }
                let face = self.draw_bg.area().rect(cx);
                self.track_press_value = self.value;
                self.drag = Some(DragState {
                    press_x: fe.abs.x, press_value: self.value, width: face.size.x,
                    engaged: self.track, anchor: DragAnchor { x: fe.abs.x, value: self.value },
                    last_x: fe.abs.x, shift: fe.modifiers.shift,
                    ctrl: fe.modifiers.control || fe.modifiers.logo,
                    relative_track: fe.modifiers.shift || fe.modifiers.control || fe.modifiers.logo,
                });
                cx.claim_finger_drag(fe.digit_id);
                if self.track {
                    let value = self.track_value(cx, fe.abs.x);
                    self.commit(cx, self.uid, value);
                    if let Some(drag) = &mut self.drag { drag.anchor.value = self.value; }
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let Some(mut drag) = self.drag else { return };
                cx.set_cursor(MouseCursor::EwResize);
                let hovered = fe.is_over && fe.device.has_hovers();
                let face = self.draw_bg.area().rect(cx);
                let zone = self.zone(fe.abs.x - face.pos.x, face.size.x);
                if self.hovered != hovered || self.hover_zone != zone {
                    self.hovered = hovered;
                    self.hover_zone = zone;
                    self.draw_bg.redraw(cx);
                }
                if !drag.engaged {
                    if (fe.abs.x - drag.press_x).abs() < DRAG_THRESHOLD { return; }
                    drag.engaged = true;
                    drag.anchor = DragAnchor { x: fe.abs.x, value: self.value };
                    drag.last_x = fe.abs.x;
                    self.drag = Some(drag);
                    self.draw_bg.redraw(cx);
                    return;
                }
                let ctrl = fe.modifiers.control || fe.modifiers.logo;
                if drag.shift != fe.modifiers.shift || drag.ctrl != ctrl {
                    drag.shift = fe.modifiers.shift;
                    drag.ctrl = ctrl;
                    drag.anchor = DragAnchor { x: fe.abs.x, value: self.value };
                    drag.last_x = fe.abs.x;
                    drag.relative_track = true;
                    self.drag = Some(drag);
                    return;
                }
                if fe.abs.x == drag.last_x { return; }
                let value = if self.track && !drag.relative_track {
                    self.track_value(cx, fe.abs.x)
                } else {
                    let (value, anchor) = drag_map(self.params(), drag.anchor, fe.abs.x,
                        drag.width, drag.shift, drag.ctrl);
                    drag.anchor = anchor;
                    value
                };
                drag.last_x = fe.abs.x;
                self.drag = Some(drag);
                if value != self.value { self.commit(cx, self.uid, value); }
            }
            Hit::FingerUp(fe) => {
                if fe.cancelled { self.cancel_drag(cx); return; }
                let Some(drag) = self.drag.take() else { return };
                if drag.engaged { self.ended(cx, self.uid); }
                else if fe.is_over {
                    let face = self.draw_bg.area().rect(cx);
                    let zone = self.zone(fe.abs.x - face.pos.x, face.size.x);
                    if zone == 0 { self.enter_edit(cx); }
                    else { self.step_once(cx, zone as f64, fe.modifiers.shift); }
                }
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(key) => match key.key_code {
                KeyCode::ArrowUp => self.step_once(cx, 1.0, key.modifiers.shift),
                KeyCode::ArrowDown => self.step_once(cx, -1.0, key.modifiers.shift),
                KeyCode::ArrowRight if self.track => self.step_once(cx, 1.0, key.modifiers.shift),
                KeyCode::ArrowLeft if self.track => self.step_once(cx, -1.0, key.modifiers.shift),
                KeyCode::ReturnKey => self.enter_edit(cx),
                KeyCode::Tab => focus_order::advance(cx, self.draw_bg.area(), id!(property_number), key.modifiers.shift),
                _ => {}
            },
            _ => {}
        }
    }
}

impl ValueInputRef {
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        actions.filter_widget_actions_cast::<ValueInputAction>(self.widget_uid())
            .filter_map(|action| match action { ValueInputAction::Changed(value) => Some(value), _ => None }).last()
    }

    pub fn ended(&self, actions: &Actions) -> Option<f64> {
        actions.filter_widget_actions_cast::<ValueInputAction>(self.widget_uid())
            .filter_map(|action| match action { ValueInputAction::Ended(value) => Some(value), _ => None }).last()
    }

    pub fn value(&self) -> f64 { self.borrow().map_or(0.0, |inner| inner.value()) }
    pub fn is_editing(&self) -> bool { self.borrow().is_some_and(|inner| inner.is_editing()) }
    pub fn is_interacting(&self) -> bool { self.borrow().is_some_and(|inner| inner.is_interacting()) }
    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_value(cx, value); }
    }
    pub fn set_read_only(&self, cx: &mut Cx, read_only: bool) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_read_only(cx, read_only); }
    }
    pub fn begin_edit(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.begin_edit(cx); }
    }
}

#[cfg(test)]
mod track {
    use super::*;
    use crate::event::{ScrollEvent, ScrollPhase};
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);
    const WIDE: f64 = 228.0;

    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx) }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: Vec2d, time: f64) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time,
        })
    }

    fn moved(abs: Vec2d, time: f64) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time,
        })
    }

    fn release(abs: Vec2d, time: f64) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time,
        })
    }

    fn wheel(abs: Vec2d, notches: f64) -> Event {
        Event::Scroll(ScrollEvent {
            window_id: WINDOW,
            scroll: dvec2(0.0, -120.0 * notches),
            abs,
            modifiers: KeyModifiers::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: true,
            time: 0.0,
            phase: ScrollPhase::Changed,
        })
    }

    fn key(key_code: KeyCode) -> Event {
        Event::KeyDown(KeyEvent {
            key_code,
            is_repeat: false,
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    /// Key focus is a request until the loop turns over, and the loop only
    /// turns over when there is something in it.
    fn settle_focus(cx: &mut Cx) {
        cx.action(());
        cx.handle_actions();
    }

    fn send(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()))
    }

    /// With no event loop here to end a capture on the release, the area a
    /// press captured is let go by hand, or it takes every later press.
    fn claimed(event: &Event) -> Area {
        match event {
            Event::MouseDown(e) => e.handled.get(),
            Event::MouseMove(e) => e.handled.get(),
            _ => Area::Empty,
        }
    }

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    band := ValueInput{
                        width: 228.
                        min: 0.
                        max: 255.
                        step: 1.
                        precision: 0.
                        track: true
                    }
                    scrub := ValueInput{
                        width: 228.
                        min: 0.
                        max: 255.
                        step: 1.
                        precision: 0.
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let band = root.widget(cx, ids!(band));
        let scrub = root.widget(cx, ids!(scrub));
        assert!(!band.is_empty() && !scrub.is_empty(), "the scene has both fields");
        (root, band, scrub)
    }

    /// A window point `t` (0..1) along the field's fill — the span the field
    /// paints across and reads a press against.
    fn along(cx: &Cx, field: &WidgetRef, t: f64) -> Vec2d {
        let face = field.area().rect(cx);
        assert_eq!(face.size.x, WIDE, "the field drew at the width asked for");
        let (lo, hi) = track_span(face.size.x);
        dvec2(face.pos.x + lo + t * (hi - lo), face.pos.y + face.size.y * 0.5)
    }

    fn value(field: &WidgetRef) -> f64 {
        field.borrow::<ValueInput>().unwrap().value()
    }

    fn changed(actions: &ActionsBuf, field: &WidgetRef) -> Option<f64> {
        actions
            .filter_widget_actions_cast::<ValueInputAction>(field.widget_uid())
            .find_map(|a| match a {
                ValueInputAction::Changed(v) => Some(v),
                _ => None,
            })
    }

    fn ended(actions: &ActionsBuf, field: &WidgetRef) -> Option<f64> {
        actions
            .filter_widget_actions_cast::<ValueInputAction>(field.widget_uid())
            .find_map(|a| match a {
                ValueInputAction::Ended(v) => Some(v),
                _ => None,
            })
    }

    /// The mapping, with no window anywhere near it: a point along the fill
    /// names the value that point of the fill stands for.
    #[test]
    fn a_point_on_the_track_names_the_value_that_point_means() {
        assert_eq!(track_value_at(16.0, 16.0, 192.0, 0.0, 255.0), 0.0);
        assert_eq!(track_value_at(192.0, 16.0, 192.0, 0.0, 255.0), 255.0);
        assert!((track_value_at(104.0, 16.0, 192.0, 0.0, 255.0) - 127.5).abs() < 1e-9);
        // Past either end is that end: a hand that overshoots meant the limit.
        assert_eq!(track_value_at(-40.0, 16.0, 192.0, 0.0, 255.0), 0.0);
        assert_eq!(track_value_at(900.0, 16.0, 192.0, 0.0, 255.0), 255.0);
        // And the fill's two ends are the field's two edges, less the border
        // the shader keeps clear.
        assert_eq!(track_span(228.0), (1.0, 227.0));
    }

    /// The press lands the value under the pointer at once, the drag keeps it
    /// there, and the release says so once. This is the one the scrub fails.
    #[test]
    fn a_press_on_the_track_lands_the_value_and_the_drag_keeps_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.5);
        let down = press(at, 0.0);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(
            changed(&actions, &band),
            Some(128.0),
            "the press did not land the value it was pointing at"
        );
        assert_eq!(ended(&actions, &band), None, "the press ended a gesture it had just begun");
        let to = along(&cx, &band, 0.75);
        let actions = send(&mut cx, &root, &moved(to, 0.1));
        assert_eq!(changed(&actions, &band), Some(191.0), "the drag did not follow the pointer");
        let actions = send(&mut cx, &root, &release(to, 0.2));
        assert_eq!(ended(&actions, &band), Some(191.0), "the release did not commit");
        // And once only.
        let actions = send(&mut cx, &root, &release(to, 0.3));
        assert_eq!(ended(&actions, &band), None, "a second release ended the gesture again");
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }

    /// The two ends of the fill are the two ends of the range, which is the
    /// whole reason a hand can reach zero and full at all.
    #[test]
    fn the_ends_of_the_fill_are_the_ends_of_the_range() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        for (t, want) in [(1.0, 255.0), (0.0, 0.0)] {
            let at = along(&cx, &band, t);
            let down = press(at, 0.0);
            send(&mut cx, &root, &down);
            send(&mut cx, &root, &release(at, 0.1));
            down.unhandle(&mut cx, &claimed(&down));
            assert_eq!(value(&band), want, "the end of the fill named {want}");
        }
        cx.fingers.first_mouse_button = None;
    }

    /// And the field that did NOT ask for a track is the field it always was:
    /// the press only arms, travel scrubs it at a step per point, a plain
    /// click in the middle opens typing and a click on an end steps it.
    #[test]
    fn a_scrub_reanchors_at_engagement_and_clicks_still_step_or_type() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _band, scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &scrub, 0.5);
        let down = press(at, 0.0);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(changed(&actions, &scrub), None, "the press moved a scrub field");
        assert_eq!(value(&scrub), 0.0);
        // The threshold arms without moving; subsequent travel scrubs.
        send(&mut cx, &root, &moved(dvec2(at.x + 3.0, at.y), 0.05));
        let to = dvec2(at.x + 13.0, at.y);
        let actions = send(&mut cx, &root, &moved(to, 0.1));
        assert_eq!(changed(&actions, &scrub), Some(10.0), "the scrub did not follow the travel");
        assert_eq!(ended(&actions, &scrub), None, "a scrub ended before release");
        let actions = send(&mut cx, &root, &release(to, 0.2));
        assert_eq!(ended(&actions, &scrub), Some(10.0));
        down.unhandle(&mut cx, &claimed(&down));

        // A click that did not wander: the middle types, an end steps.
        let face = scrub.area().rect(&cx);
        let middle = face.pos + face.size * 0.5;
        let down = press(middle, 1.0);
        send(&mut cx, &root, &down);
        send(&mut cx, &root, &release(middle, 1.1));
        down.unhandle(&mut cx, &claimed(&down));
        assert!(
            scrub.borrow::<ValueInput>().unwrap().editing,
            "a plain click in the middle no longer opens typing"
        );
        scrub.borrow_mut::<ValueInput>().unwrap().exit_edit(&mut cx);
        let stood = value(&scrub);
        let end = dvec2(face.pos.x + 4.0, middle.y);
        let down = press(end, 2.0);
        send(&mut cx, &root, &down);
        let actions = send(&mut cx, &root, &release(end, 2.1));
        down.unhandle(&mut cx, &claimed(&down));
        assert_eq!(
            changed(&actions, &scrub),
            Some(stood - 1.0),
            "a click on the low end no longer steps the field"
        );
        cx.fingers.first_mouse_button = None;
    }

    /// The wheel over a track still steps it, and the arrows step it once a
    /// press has left the keyboard here — which is where the edge arrows a
    /// track gives up went.
    #[test]
    fn the_wheel_and_the_arrows_still_step_a_track() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        let over = along(&cx, &band, 0.5);
        cx.set_key_focus(band.area());
        settle_focus(&mut cx);
        let actions = send(&mut cx, &root, &wheel(over, 1.0));
        assert_eq!(changed(&actions, &band), Some(1.0), "the wheel did not step the field");
        assert_eq!(ended(&actions, &band), Some(1.0), "a step is its own commit");
        let actions = send(&mut cx, &root, &wheel(over, -1.0));
        assert_eq!(changed(&actions, &band), Some(0.0), "and back again");

        // The keyboard is only reached by a press, and `set_key_focus` records
        // the request rather than granting it, so the actions are dispatched.
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.5);
        let down = press(at, 0.0);
        root.handle_event(&mut cx, &down, &mut Scope::empty());
        cx.handle_actions();
        root.handle_event(&mut cx, &release(at, 0.1), &mut Scope::empty());
        cx.handle_actions();
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert!(cx.has_key_focus(band.area()), "the press left the keyboard elsewhere");
        let stood = value(&band);
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight));
        assert_eq!(changed(&actions, &band), Some(stood + 1.0), "the arrow did not step");
        assert_eq!(ended(&actions, &band), Some(stood + 1.0));
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowLeft));
        assert_eq!(changed(&actions, &band), Some(stood), "and back again");
    }

    /// Typing is still the way in to an exact number, and Return still
    /// commits it. On a track the way to the editor is the keyboard's own
    /// Return, or a second press on the heels of the first.
    #[test]
    fn typing_an_exact_number_still_commits_on_a_track() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.25);
        let down = press(at, 0.0);
        root.handle_event(&mut cx, &down, &mut Scope::empty());
        cx.handle_actions();
        root.handle_event(&mut cx, &release(at, 0.1), &mut Scope::empty());
        cx.handle_actions();
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert_eq!(value(&band), 64.0, "the press did not land a quarter of the way along");

        // Dispatched, not captured: the editor asks for the keyboard with an
        // action, and a captured action never reaches the one it asks.
        root.handle_event(&mut cx, &key(KeyCode::ReturnKey), &mut Scope::empty());
        cx.handle_actions();
        assert!(
            band.borrow::<ValueInput>().unwrap().editing,
            "Return on a focused track did not open the editor"
        );
        // The frame the editor's redraw asks for, which is where it takes the
        // keyboard: a widget that has never been drawn has no area to take it
        // with.
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        settle_focus(&mut cx);
        let editor = band.borrow::<ValueInput>().unwrap().text_input.area();
        assert!(cx.has_key_focus(editor), "the editor opened without the keyboard");
        band.borrow_mut::<ValueInput>().unwrap().text_input.set_text(&mut cx, "200");
        let actions = send(&mut cx, &root, &key(KeyCode::ReturnKey));
        assert_eq!(changed(&actions, &band), Some(200.0), "the typed number never committed");
        assert_eq!(ended(&actions, &band), Some(200.0), "and a commit is the end of a gesture");
        assert!(!band.borrow::<ValueInput>().unwrap().editing, "the editor stayed open");
    }

    /// The second press of a double means "type here", and it puts the field
    /// back where the pair found it: reaching for the keyboard leaves no
    /// number behind that nobody asked for.
    ///
    /// The count of taps is the platform's own, kept where no test outside it
    /// can reach, so the rule and what it leads to are taken at the two seams
    /// the press runs through rather than at the press.
    #[test]
    fn a_double_press_opens_typing_and_puts_back_what_it_found() {
        assert!(!track_press_means_typing(1), "a single press means set it here");
        assert!(track_press_means_typing(2), "a second press on its heels means type here");
        assert!(track_press_means_typing(3), "and a third is still the keyboard");

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        band.borrow_mut::<ValueInput>().unwrap().set_value(&mut cx, 100.0);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.75);
        let first = press(at, 0.0);
        send(&mut cx, &root, &first);
        send(&mut cx, &root, &release(at, 0.05));
        first.unhandle(&mut cx, &claimed(&first));
        cx.fingers.first_mouse_button = None;
        assert_eq!(value(&band), 191.0, "the first press of the pair still lands the value");

        let uid = band.widget_uid();
        let actions = cx.capture_actions(|cx| {
            band.borrow_mut::<ValueInput>().unwrap().type_here(cx, uid);
        });
        assert_eq!(
            changed(&actions, &band),
            Some(100.0),
            "the second press did not put back the number the pair found"
        );
        assert_eq!(ended(&actions, &band), Some(100.0), "and it ends the gesture it undid");
        assert!(
            band.borrow::<ValueInput>().unwrap().editing,
            "the second press did not open the editor"
        );
    }
}

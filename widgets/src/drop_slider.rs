//! DropSlider — a compact value chip that opens a POPOVER slider.
//!
//! For bars and other tight chrome where an inline slider would fight
//! window-dragging or crowd the line: the widget itself is a small click
//! target (icon + live readout); a CLICK opens a horizontal slider in an
//! overlay panel just below the chip, dragging there sets the value live,
//! and releasing or clicking anywhere else closes it. No drag interaction
//! exists on the chip itself, by construction.
//!
//! The popover draws on its own overlay draw list (the `TipLayer` idiom)
//! from the chip's FINAL rect, so it floats over every panel and never
//! disturbs layout. Value semantics mirror `ValueInput`: `min`/`max`,
//! `display_scale`/`precision`/`suffix` shape the readout (`0.9` shown as
//! `90%` with scale 100, suffix "%"). Every change emits
//! [`DropSliderAction::Changed`]; hosts push external updates back with
//! `set_value`.

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum DropSliderAction {
    /// The value changed (live while the popover slider is dragged).
    Changed(f64),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DropSliderBase = #(DropSlider::register_widget(vm))
    mod.widgets.DropSlider = set_type_default() do mod.widgets.DropSliderBase{
        width: Fit
        height: 22
        padding: Inset{left: 7.0 right: 8.0 top: 0.0 bottom: 0.0}
        align: Align{x: 0.0, y: 0.5}

        draw_bg +: {
            hover: uniform(0.0)
            open: uniform(0.0)
            color: #x272e38
            color_hover: #x2f3842
            border_color: #xffffff26
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 5.0)
                sdf.fill(self.color.mix(self.color_hover, max(self.hover, self.open)))
                sdf.stroke(self.border_color, 1.0)
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
        // The popover panel + its slider parts.
        draw_panel +: {
            color: #x181c23
            border_color: #xffffff2e
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 6.0)
                sdf.fill(self.color)
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_track +: { color: #x2b343f }
        draw_fill +: { color: #xff5c39 }
        draw_knob +: { color: #xe8eef4 }
    }
}

/// Popover geometry (layout points).
const PANEL_W: f64 = 150.0;
const PANEL_H: f64 = 30.0;
const PANEL_PAD: f64 = 12.0;
const PANEL_GAP: f64 = 4.0;
const TRACK_H: f64 = 6.0;
const KNOB_W: f64 = 10.0;

#[derive(Script, Widget)]
pub struct DropSlider {
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
    draw_icon: DrawSvg,
    #[live]
    icon_walk: Walk,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_panel: DrawQuad,
    #[live]
    draw_track: DrawColor,
    #[live]
    draw_fill: DrawColor,
    #[live]
    draw_knob: DrawColor,

    #[live(0.0)]
    pub min: f64,
    #[live(1.0)]
    pub max: f64,
    #[live(0.0)]
    pub default: f64,
    /// Readout = value × display_scale, at `precision` decimals + suffix.
    #[live(1.0)]
    pub display_scale: f64,
    #[live(0.0)]
    pub precision: f64,
    #[live]
    pub suffix: String,

    #[rust]
    value_init: bool,
    #[rust]
    value: f64,
    #[rust]
    open: bool,
    #[rust]
    dragging: bool,
    /// The popover rect of the last draw (event-side hit tests use it).
    #[rust]
    panel_rect: Rect,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for DropSlider {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
}

impl DropSlider {
    fn readout(&self) -> String {
        format!(
            "{:.*}{}",
            self.precision.max(0.0) as usize,
            self.value * self.display_scale,
            self.suffix
        )
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        if self.dragging {
            return;
        }
        let value = value.clamp(self.min, self.max);
        if (value - self.value).abs() > f64::EPSILON {
            self.value = value;
            self.value_init = true;
            self.redraw_all(cx);
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
            self.dragging = false;
            self.draw_bg.set_uniform(cx, id!(open), &[if open { 1.0 } else { 0.0 }]);
            self.redraw_all(cx);
        }
    }

    fn drag_to(&mut self, cx: &mut Cx, uid: WidgetUid, x: f64) {
        let x0 = self.panel_rect.pos.x + PANEL_PAD;
        let w = (self.panel_rect.size.x - PANEL_PAD * 2.0).max(1.0);
        let fraction = ((x - x0) / w).clamp(0.0, 1.0);
        let value = self.min + fraction * (self.max - self.min);
        if (value - self.value).abs() > f64::EPSILON {
            self.value = value;
            cx.widget_action(uid, DropSliderAction::Changed(value));
            self.redraw_all(cx);
        }
    }
}

impl Widget for DropSlider {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.value_init {
            self.value_init = true;
            self.value = self.default.clamp(self.min, self.max);
        }
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text
            .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &self.readout());
        self.draw_bg.end(cx);

        if self.open {
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
                    Walk::fixed(PANEL_W, PANEL_H),
                    Layout::default(),
                );
                let panel = cx.turtle().rect();
                let x0 = panel.pos.x + PANEL_PAD;
                let w = panel.size.x - PANEL_PAD * 2.0;
                let y = panel.pos.y + (panel.size.y - TRACK_H) * 0.5;
                self.draw_track.draw_abs(
                    cx,
                    Rect { pos: dvec2(x0, y), size: dvec2(w, TRACK_H) },
                );
                let fraction = if self.max > self.min {
                    ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                self.draw_fill.draw_abs(
                    cx,
                    Rect { pos: dvec2(x0, y), size: dvec2(w * fraction, TRACK_H) },
                );
                self.draw_knob.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(
                            x0 + (w - KNOB_W) * fraction,
                            y - (14.0 - TRACK_H) * 0.5,
                        ),
                        size: dvec2(KNOB_W, 14.0),
                    },
                );
                self.draw_panel.end(cx);
                let chip = self.draw_bg.area().rect(cx);
                cx.end_pass_sized_turtle_with_shift(
                    self.draw_bg.area(),
                    dvec2((chip.size.x - PANEL_W) * 0.5, chip.size.y + PANEL_GAP),
                );
                draw_list.end(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        // The popover owns the pointer while open: a press inside scrubs,
        // a press outside chip AND panel closes. The CHIP toggle itself
        // lives in the hits arm below — one press, one state change.
        if self.open {
            // The panel hangs under the chip deterministically.
            let chip = self.draw_bg.area().rect(cx);
            self.panel_rect = Rect {
                pos: dvec2(
                    chip.pos.x + (chip.size.x - PANEL_W) * 0.5,
                    chip.pos.y + chip.size.y + PANEL_GAP,
                ),
                size: dvec2(PANEL_W, PANEL_H),
            };
            match event {
                Event::MouseDown(me) => {
                    if self.panel_rect.contains(me.abs) {
                        self.dragging = true;
                        self.drag_to(cx, uid, me.abs.x);
                    } else {
                        let chip = self.draw_bg.area().rect(cx);
                        if !chip.contains(me.abs) {
                            self.set_open(cx, false);
                        }
                    }
                }
                Event::MouseMove(me) => {
                    if self.dragging {
                        self.drag_to(cx, uid, me.abs.x);
                    }
                }
                Event::MouseUp(_) => {
                    self.dragging = false;
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

impl DropSliderRef {
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        if let DropSliderAction::Changed(v) =
            actions.find_widget_action(self.widget_uid())?.cast()
        {
            return Some(v);
        }
        None
    }

    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value()).unwrap_or(0.0)
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }
}

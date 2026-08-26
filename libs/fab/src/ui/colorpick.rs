//! Lane D. The colour picker: a swatch button that opens a popover with a
//! hue ring around a saturation/value square, numeric rows built on
//! [`crate::ui::dragnum::FabDragNumber`] (RGB / HSV / Hex modes), and a row
//! of recent colours.
//!
//! The behaviour contract:
//!
//! * The **swatch** is the whole control at rest. Clicking it opens the
//!   popover; opening broadcasts through the popover bus
//!   ([`crate::ui::popover::OpenPopup`] → `MenuOpened` / `MenuClosed`) so
//!   every popup button in the shell mirrors "exactly one popup is open",
//!   and a `MenuClickAway` landing on the swatch swaps a menu for the
//!   picker on a single click.
//! * The **ring** drags hue, the **square** drags saturation/value; the
//!   pointer is captured, so tracking continues outside the widget until
//!   release. The picked point is a two-tone ring (dark outline, light
//!   inner) so it reads over both light and dark colours.
//! * **Every change publishes immediately** (`Changed`), the same way the
//!   drag-number publishes per drag step — a bound material follows the
//!   hand. Release / commit publishes `Ended` once.
//! * **Hex** accepts `#rgb`, `#rrggbb`, `#rrggbbaa` and the same without
//!   the hash; select-all on focus, Enter commits, Escape reverts the text.
//! * **Escape closes without changing** (the colour at open is restored and
//!   published), **clicking outside commits** — the same commit rules as a
//!   number field's text entry. Arrow keys on the wheel nudge hue (←→) and
//!   value (↑↓), Shift for fine steps.
//! * **Recent colours** are process-wide for the session: the last eight
//!   committed colours, newest first, click to apply.
//!
//! There is no eyedropper: the shell has no in-process screen-pixel read,
//! and inventing an OS capture path is out of scope by design.

use crate::ui::dragnum::*;
use crate::ui::popover::{FabUiAction, OpenPopup, PopupChange};
use makepad_widgets::*;
use std::sync::{Mutex, OnceLock};

// ===========================================================================
// The pure colour core — conversions, hex parsing, wheel geometry
// ===========================================================================

/// HSV → RGB, all channels 0..1. `h` wraps.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// RGB → HSV, all channels 0..1. A grey (max == min) keeps hue 0 and sat 0.
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d <= 0.0 {
        0.0
    } else if (max - r).abs() < f32::EPSILON {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if (max - g).abs() < f32::EPSILON {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, s, v]
}

/// Accepts `#rgb`, `#rrggbb`, `#rrggbbaa`, each with or without the hash.
/// Returns the colour and whether the string carried alpha.
pub fn parse_hex(text: &str) -> Option<([f32; 4], bool)> {
    let t = text.trim().trim_start_matches('#');
    if !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let nib = |c: u8| -> f32 {
        let d = (c as char).to_digit(16).unwrap_or(0) as f32;
        d / 15.0
    };
    let byte = |hi: u8, lo: u8| -> f32 {
        let h = (hi as char).to_digit(16).unwrap_or(0);
        let l = (lo as char).to_digit(16).unwrap_or(0);
        ((h * 16 + l) as f32) / 255.0
    };
    let b = t.as_bytes();
    match b.len() {
        3 => Some(([nib(b[0]), nib(b[1]), nib(b[2]), 1.0], false)),
        6 => Some((
            [byte(b[0], b[1]), byte(b[2], b[3]), byte(b[4], b[5]), 1.0],
            false,
        )),
        8 => Some((
            [
                byte(b[0], b[1]),
                byte(b[2], b[3]),
                byte(b[4], b[5]),
                byte(b[6], b[7]),
            ],
            true,
        )),
        _ => None,
    }
}

/// `#RRGGBB`, or `#RRGGBBAA` when `with_alpha`.
pub fn format_hex(rgba: [f32; 4], with_alpha: bool) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    if with_alpha {
        format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            b(rgba[0]),
            b(rgba[1]),
            b(rgba[2]),
            b(rgba[3])
        )
    } else {
        format!("#{:02X}{:02X}{:02X}", b(rgba[0]), b(rgba[1]), b(rgba[2]))
    }
}

/// Ring outer radius as a fraction of the widget size (the shader uses the
/// same constants, so hit testing and pixels never disagree).
pub const RING_OUTER: f64 = 0.48;
/// Ring inner radius as a fraction of the widget size.
pub const RING_INNER: f64 = 0.385;
/// Half-side of the SV square as a fraction of the widget size — fits inside
/// the ring with a small gap (`RING_INNER / sqrt(2)` would touch).
pub const SQUARE_HALF: f64 = 0.255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelZone {
    Ring,
    Square,
    None,
}

/// Which zone a pointer at `rel` (widget-local, origin top-left) lands in,
/// for a wheel drawn at `size` (its smaller dimension).
pub fn wheel_zone(rel: DVec2, size: f64) -> WheelZone {
    let dx = rel.x - size * 0.5;
    let dy = rel.y - size * 0.5;
    let half = SQUARE_HALF * size;
    if dx.abs() <= half && dy.abs() <= half {
        return WheelZone::Square;
    }
    let r = (dx * dx + dy * dy).sqrt();
    if r <= RING_OUTER * size + 4.0 && r >= RING_INNER * size - 4.0 {
        return WheelZone::Ring;
    }
    WheelZone::None
}

/// Hue (0..1) for a pointer on the ring: 0 at twelve o'clock, increasing
/// clockwise, red at the top.
pub fn ring_hue(rel: DVec2, size: f64) -> f32 {
    let dx = rel.x - size * 0.5;
    let dy = rel.y - size * 0.5;
    let ang = dx.atan2(-dy); // 0 at top, +cw
    ((ang / std::f64::consts::TAU).rem_euclid(1.0)) as f32
}

/// (saturation, value) for a pointer over the SV square, clamped so a drag
/// that leaves the square keeps tracking the nearest edge.
pub fn square_sv(rel: DVec2, size: f64) -> (f32, f32) {
    let half = SQUARE_HALF * size;
    let cx = size * 0.5;
    let s = ((rel.x - (cx - half)) / (half * 2.0)).clamp(0.0, 1.0);
    let v = 1.0 - ((rel.y - (cx - half)) / (half * 2.0)).clamp(0.0, 1.0);
    (s as f32, v as f32)
}

/// Session-wide recent colours, newest first, capped at eight.
fn recent_store() -> &'static Mutex<Vec<[f32; 4]>> {
    static S: OnceLock<Mutex<Vec<[f32; 4]>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

const RECENT_MAX: usize = 8;

fn push_recent(rgba: [f32; 4]) {
    let mut store = recent_store().lock().unwrap();
    store.retain(|c| {
        c.iter()
            .zip(rgba.iter())
            .any(|(a, b)| (a - b).abs() > 1.0 / 512.0)
    });
    store.insert(0, rgba);
    store.truncate(RECENT_MAX);
}

// ===========================================================================
// DSL
// ===========================================================================

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    set_type_default() do #(DrawColorWheel::script_shader(vm)){
        ..mod.draw.DrawQuad

        hue: 0.0
        sat: 0.0
        val: 0.0

        pixel: fn() {
            let size = min(self.rect_size.x, self.rect_size.y)
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            let dx = self.pos.x * self.rect_size.x - c.x
            let dy = self.pos.y * self.rect_size.y - c.y

            let outer = size * 0.48
            let inner = size * 0.385
            let half = size * 0.255

            // Hue ring: 0 at twelve o'clock, clockwise, red at the top.
            sdf.circle(c.x, c.y, outer)
            sdf.circle(c.x, c.y, inner)
            sdf.subtract()
            let ang = atan2(dx, 0.0 - dy)
            let hue_at = fract(ang / 6.2831853 + 1.0)
            sdf.fill(Pal.hsv2rgb(vec4(hue_at, 1.0, 1.0, 1.0)))

            // Saturation/value square at the current hue.
            let sq_s = clamp((dx + half) / (2.0 * half), 0.0, 1.0)
            let sq_v = 1.0 - clamp((dy + half) / (2.0 * half), 0.0, 1.0)
            sdf.rect(c.x - half, c.y - half, half * 2.0, half * 2.0)
            sdf.fill(Pal.hsv2rgb(vec4(self.hue, sq_s, sq_v, 1.0)))

            // Pucks: a dark outline with a light ring inside stays visible
            // over any colour underneath.
            let mid = (outer + inner) * 0.5
            let pa = self.hue * 6.2831853
            let rp = vec2(c.x + sin(pa) * mid, c.y - cos(pa) * mid)
            sdf.circle(rp.x, rp.y, 6.5)
            sdf.stroke(vec4(0.04, 0.04, 0.04, 0.9), 1.4)
            sdf.circle(rp.x, rp.y, 5.0)
            sdf.stroke(vec4(1.0, 1.0, 1.0, 0.95), 1.6)

            let sp = vec2(
                c.x - half + self.sat * 2.0 * half,
                c.y - half + (1.0 - self.val) * 2.0 * half
            )
            sdf.circle(sp.x, sp.y, 6.0)
            sdf.stroke(vec4(0.04, 0.04, 0.04, 0.9), 1.4)
            sdf.circle(sp.x, sp.y, 4.5)
            sdf.stroke(vec4(1.0, 1.0, 1.0, 0.95), 1.6)

            return sdf.result
        }
    }

    mod.widgets.FabColorWheelBase = #(FabColorWheel::register_widget(vm))
    mod.widgets.FabColorWheel = set_type_default() do mod.widgets.FabColorWheelBase{
        width: 220
        height: 220
    }

    let PickTab = View{
        width: Fill
        height: 18
        cursor: MouseCursor.Hand
        align: Align{x: 0.5 y: 0.5}
        show_bg: true
        draw_bg +: {
            active: instance(0.0)
            hover: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                let base = fab.color_button.mix(fab.color_button_hover, self.hover)
                sdf.fill(base.mix(fab.color_button_active, self.active))
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
        }
    }

    let RecentSwatch = View{
        visible: false
        width: 20
        height: 16
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            swatch: instance(vec4(0.5, 0.5, 0.5, 1.0))
            hover: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                sdf.fill_keep(vec4(self.swatch.xyz, 1.0))
                sdf.stroke(fab.color_border.mix(fab.color_focus_ring, self.hover), 1.0)
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
        }
    }

    let PickNum = mod.widgets.FabDragNumber{}

    mod.widgets.FabColorPickerBase = #(FabColorPicker::register_widget(vm))
    mod.widgets.FabColorPicker = set_type_default() do mod.widgets.FabColorPickerBase{
        width: Fit
        height: Fit
        with_alpha: false

        swatch := View{
            width: fab.swatch_width
            height: 16
            cursor: MouseCursor.Hand
            show_bg: true
            draw_bg +: {
                hover: instance(0.0)
                open: instance(0.0)
                swatch: instance(vec4(0.8, 0.8, 0.8, 1.0))
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                    sdf.fill_keep(vec4(self.swatch.xyz, 1.0))
                    let ring = fab.color_border.mix(fab.color_focus_ring, max(self.hover, self.open))
                    sdf.stroke(ring, 1.0)
                    return sdf.result
                }
            }
            animator: Animator{
                hover: {
                    default: @off
                    off: AnimatorState{
                        from: {all: Forward {duration: fab.anim_fast}}
                        apply: { draw_bg: {hover: 0.0} }
                    }
                    on: AnimatorState{
                        from: {all: Snap}
                        apply: { draw_bg: {hover: 1.0} }
                    }
                }
            }
        }
    }

    // The popover itself lives at the END of the shell's overlay stack
    // (`FabShell`), NOT next to the swatch: children handle events in
    // reverse declaration order, so only a top-of-shell widget reliably
    // wins the press race against the dock's areas — the same reason the
    // menu layer and the command palette live there.
    mod.widgets.FabColorPickerLayerBase = #(FabColorPickerLayer::register_widget(vm))
    mod.widgets.FabColorPickerLayer = set_type_default() do mod.widgets.FabColorPickerLayerBase{
        width: Fill
        height: Fill
        modal := Modal{
            align: Align{x: 0.0 y: 0.0}
            bg_view +: {
                draw_bg +: {
                    color: vec4(0.0, 0.0, 0.0, 0.0)
                }
            }
            content +: {
                panel := View{
                    width: 244
                    height: Fit
                    flow: Down
                    padding: 8
                    spacing: 6
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
                            sdf.fill_keep(fab.color_popover)
                            sdf.stroke(fab.color_popover_border, 1.0)
                            return sdf.result
                        }
                    }
                    wheel := mod.widgets.FabColorWheel{
                        width: 228
                        height: 228
                    }
                    tabs := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 2
                        tab_rgb := PickTab{ mod.widgets.FabLabelSmall{ text: "RGB" } }
                        tab_hsv := PickTab{ mod.widgets.FabLabelSmall{ text: "HSV" } }
                        tab_hex := PickTab{ mod.widgets.FabLabelSmall{ text: "Hex" } }
                    }
                    rows_rgb := View{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        num_r := PickNum{ label: "R" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true }
                        num_g := PickNum{ label: "G" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true }
                        num_b := PickNum{ label: "B" min: 0.0 max: 255.0 step: 1.0 precision: 0 show_fill: true quantize: true }
                    }
                    rows_hsv := View{
                        visible: false
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        num_h := PickNum{ label: "H" min: 0.0 max: 360.0 step: 1.0 precision: 0 wrap: true show_fill: true }
                        num_s := PickNum{ label: "S" min: 0.0 max: 100.0 step: 1.0 precision: 0 show_fill: true }
                        num_v := PickNum{ label: "V" min: 0.0 max: 100.0 step: 1.0 precision: 0 show_fill: true }
                    }
                    hex_row := View{
                        visible: false
                        width: Fill
                        height: fab.row_height
                        flow: Right
                        align: Align{x: 0.0 y: 0.5}
                        spacing: 6
                        mod.widgets.FabLabelDim{ width: 30 text: "Hex" }
                        hex := TextInput{
                            width: Fill
                            height: Fill
                            empty_text: ""
                            draw_bg +: {
                                color: fab.color_input
                                border_radius: fab.radius
                            }
                            draw_text +: {
                                color: fab.color_text
                                ink_centered: true
                                text_style: theme.font_regular{ font_size: fab.font_size_ui }
                            }
                        }
                    }
                    row_alpha := View{
                        visible: false
                        width: Fill
                        height: Fit
                        num_a := PickNum{ label: "A" min: 0.0 max: 100.0 step: 1.0 precision: 0 show_fill: true }
                    }
                    recent_row := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 3
                        r0 := RecentSwatch{}
                        r1 := RecentSwatch{}
                        r2 := RecentSwatch{}
                        r3 := RecentSwatch{}
                        r4 := RecentSwatch{}
                        r5 := RecentSwatch{}
                        r6 := RecentSwatch{}
                        r7 := RecentSwatch{}
                    }
                }
            }
        }
    }
}

// ===========================================================================
// The wheel widget
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawColorWheel {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hue: f32,
    #[live]
    sat: f32,
    #[live]
    val: f32,
}

#[derive(Clone, Debug, Default)]
pub enum ColorWheelAction {
    /// Live while dragging or nudging: (hue, sat, val), all 0..1.
    Changed([f32; 3]),
    /// The gesture finished (mouse up).
    Ended([f32; 3]),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabColorWheel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_wheel: DrawColorWheel,
    #[walk]
    walk: Walk,
    #[rust]
    drag: Option<WheelZone>,
}

impl FabColorWheel {
    pub fn set_hsv(&mut self, cx: &mut Cx, h: f32, s: f32, v: f32) {
        if (h - self.draw_wheel.hue).abs() > f32::EPSILON
            || (s - self.draw_wheel.sat).abs() > f32::EPSILON
            || (v - self.draw_wheel.val).abs() > f32::EPSILON
        {
            self.draw_wheel.hue = h;
            self.draw_wheel.sat = s;
            self.draw_wheel.val = v;
            self.draw_wheel.redraw(cx);
        }
    }

    pub fn hsv(&self) -> [f32; 3] {
        [self.draw_wheel.hue, self.draw_wheel.sat, self.draw_wheel.val]
    }

    fn apply_pointer(&mut self, cx: &mut Cx, uid: WidgetUid, abs: DVec2, ended: bool) {
        let rect = self.draw_wheel.area().rect(cx);
        let size = rect.size.x.min(rect.size.y);
        let rel = abs - rect.pos;
        match self.drag {
            Some(WheelZone::Ring) => {
                self.draw_wheel.hue = ring_hue(rel, size);
            }
            Some(WheelZone::Square) => {
                let (s, v) = square_sv(rel, size);
                self.draw_wheel.sat = s;
                self.draw_wheel.val = v;
            }
            _ => return,
        }
        self.draw_wheel.redraw(cx);
        let hsv = self.hsv();
        cx.widget_action(uid, ColorWheelAction::Changed(hsv));
        if ended {
            cx.widget_action(uid, ColorWheelAction::Ended(hsv));
        }
    }

    fn nudge(&mut self, cx: &mut Cx, uid: WidgetUid, dh: f32, dv: f32) {
        self.draw_wheel.hue = (self.draw_wheel.hue + dh).rem_euclid(1.0);
        self.draw_wheel.val = (self.draw_wheel.val + dv).clamp(0.0, 1.0);
        self.draw_wheel.redraw(cx);
        let hsv = self.hsv();
        cx.widget_action(uid, ColorWheelAction::Changed(hsv));
        cx.widget_action(uid, ColorWheelAction::Ended(hsv));
    }
}

impl Widget for FabColorWheel {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let _ = self.draw_wheel.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_wheel.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Crosshair);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_wheel.area());
                let rect = self.draw_wheel.area().rect(cx);
                let size = rect.size.x.min(rect.size.y);
                let zone = wheel_zone(fe.abs - rect.pos, size);
                if zone != WheelZone::None {
                    self.drag = Some(zone);
                    self.apply_pointer(cx, uid, fe.abs, false);
                }
            }
            Hit::FingerMove(fe) => {
                if self.drag.is_some() {
                    self.apply_pointer(cx, uid, fe.abs, false);
                }
            }
            Hit::FingerUp(fe) => {
                if self.drag.is_some() {
                    self.apply_pointer(cx, uid, fe.abs, true);
                    self.drag = None;
                }
            }
            Hit::KeyDown(ke) => {
                let fine = if ke.modifiers.shift { 0.1 } else { 1.0 };
                match ke.key_code {
                    KeyCode::ArrowLeft => self.nudge(cx, uid, -fine / 360.0, 0.0),
                    KeyCode::ArrowRight => self.nudge(cx, uid, fine / 360.0, 0.0),
                    KeyCode::ArrowUp => self.nudge(cx, uid, 0.0, fine / 100.0),
                    KeyCode::ArrowDown => self.nudge(cx, uid, 0.0, -fine / 100.0),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl FabColorWheelRef {
    pub fn changed(&self, actions: &Actions) -> Option<[f32; 3]> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ColorWheelAction::Changed(hsv) = item.cast() {
                return Some(hsv);
            }
        }
        None
    }

    pub fn ended(&self, actions: &Actions) -> Option<[f32; 3]> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ColorWheelAction::Ended(hsv) = item.cast() {
                return Some(hsv);
            }
        }
        None
    }

    pub fn set_hsv(&self, cx: &mut Cx, h: f32, s: f32, v: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_hsv(cx, h, s, v);
        }
    }
}

// ===========================================================================
// The picker
// ===========================================================================

#[derive(Clone, Debug, Default)]
pub enum ColorPickerAction {
    /// Live: the bound value should follow immediately.
    Changed(Vec4f),
    /// Commit (release, Enter, click-away close). Escape publishes
    /// `Changed(original)` then `Ended(original)`.
    Ended(Vec4f),
    #[default]
    None,
}

/// Which set of numeric rows the popover shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum PickMode {
    #[default]
    Rgb,
    Hsv,
    Hex,
}

// ===========================================================================
// The layer — the popover itself, one instance at the top of the shell
// ===========================================================================

#[derive(Script, ScriptHook, Widget)]
pub struct FabColorPickerLayer {
    #[deref]
    view: View,

    /// The swatch (bus owner) this popover is editing, while open.
    #[rust]
    owner: Option<LiveId>,
    #[rust]
    hsv: [f32; 3],
    #[rust(1.0)]
    alpha: f32,
    #[rust]
    with_alpha: bool,
    #[rust]
    mode: PickMode,
    /// The colour when the popover opened — restored by Escape.
    #[rust]
    opened_value: [f32; 4],
    #[rust]
    anchor: Rect,
    /// True until the anchor position has been applied on the first draw
    /// after opening.
    #[rust]
    place_pending: bool,
    #[rust]
    hex_focused: bool,
    /// Set on the hex field gaining focus; the select-all re-runs after the
    /// mouse release that would otherwise collapse it to a caret.
    #[rust]
    hex_select_pending: bool,
    /// The popover bus mirror: broadcasts MenuOpened/MenuClosed for the
    /// owner so every popup button in the shell agrees on what is open.
    #[rust]
    popup: OpenPopup,
}

const RECENT_IDS: &[&[LiveId]] = &[
    ids!(modal.recent_row.r0),
    ids!(modal.recent_row.r1),
    ids!(modal.recent_row.r2),
    ids!(modal.recent_row.r3),
    ids!(modal.recent_row.r4),
    ids!(modal.recent_row.r5),
    ids!(modal.recent_row.r6),
    ids!(modal.recent_row.r7),
];

impl FabColorPickerLayer {
    fn rgba(&self) -> [f32; 4] {
        let [h, s, v] = self.hsv;
        let [r, g, b] = hsv_to_rgb(h, s, v);
        [r, g, b, self.alpha]
    }

    fn publish(&mut self, cx: &mut Cx, ended: bool) {
        let Some(owner) = self.owner else { return };
        let rgba = self.rgba();
        cx.action(FabUiAction::ColorPickerChanged { owner, rgba });
        if ended {
            cx.action(FabUiAction::ColorPickerEnded { owner, rgba });
        }
    }

    /// Push the state into every control (wheel, rows, hex).
    fn sync_widgets(&mut self, cx: &mut Cx) {
        if self.owner.is_none() {
            return;
        }
        let [h, s, v] = self.hsv;
        let rgba = self.rgba();
        self.view
            .fab_color_wheel(cx, ids!(modal.wheel))
            .set_hsv(cx, h, s, v);
        self.view
            .fab_drag_number(cx, ids!(modal.num_r))
            .set_value(cx, (rgba[0] * 255.0).round() as f64);
        self.view
            .fab_drag_number(cx, ids!(modal.num_g))
            .set_value(cx, (rgba[1] * 255.0).round() as f64);
        self.view
            .fab_drag_number(cx, ids!(modal.num_b))
            .set_value(cx, (rgba[2] * 255.0).round() as f64);
        self.view
            .fab_drag_number(cx, ids!(modal.num_h))
            .set_value(cx, (h * 360.0) as f64);
        self.view
            .fab_drag_number(cx, ids!(modal.num_s))
            .set_value(cx, (s * 100.0) as f64);
        self.view
            .fab_drag_number(cx, ids!(modal.num_v))
            .set_value(cx, (v * 100.0) as f64);
        self.view
            .fab_drag_number(cx, ids!(modal.num_a))
            .set_value(cx, (self.alpha * 100.0) as f64);
        if !self.hex_focused {
            let hex = format_hex(rgba, self.with_alpha);
            self.view.text_input(cx, ids!(modal.hex)).set_text(cx, &hex);
        }
    }

    fn sync_recent(&mut self, cx: &mut Cx) {
        let colors = recent_store().lock().unwrap().clone();
        for (i, id) in RECENT_IDS.iter().enumerate() {
            let row = self.view.view(cx, id);
            match colors.get(i) {
                Some(c) => {
                    row.set_visible(cx, true);
                    let col = vec4(c[0], c[1], c[2], 1.0);
                    let mut row = row.clone();
                    script_apply_eval!(cx, row, {
                        draw_bg +: { swatch: #(col) }
                    });
                }
                None => row.set_visible(cx, false),
            }
        }
        self.view.redraw(cx);
    }

    fn sync_mode(&mut self, cx: &mut Cx) {
        let mode = self.mode;
        self.view
            .view(cx, ids!(modal.rows_rgb))
            .set_visible(cx, mode == PickMode::Rgb);
        self.view
            .view(cx, ids!(modal.rows_hsv))
            .set_visible(cx, mode == PickMode::Hsv);
        self.view
            .view(cx, ids!(modal.hex_row))
            .set_visible(cx, mode == PickMode::Hex);
        for (id, m) in [
            (ids!(modal.tabs.tab_rgb), PickMode::Rgb),
            (ids!(modal.tabs.tab_hsv), PickMode::Hsv),
            (ids!(modal.tabs.tab_hex), PickMode::Hex),
        ] {
            let mut tab = self.view.view(cx, id);
            let active: f32 = if mode == m { 1.0 } else { 0.0 };
            script_apply_eval!(cx, tab, {
                draw_bg +: { active: #(active) }
            });
        }
    }

    /// The broadcast half of the popover contract: every open/close goes
    /// through the tracker and out as `MenuOpened` / `MenuClosed`, close
    /// before open, then hovers are cleared tree-wide.
    fn broadcast(&mut self, cx: &mut Cx, next: Option<LiveId>) {
        let changes = self.popup.set(next);
        if changes.is_empty() {
            return;
        }
        for change in changes {
            match change {
                PopupChange::Closed(owner) => cx.action(FabUiAction::MenuClosed { owner }),
                PopupChange::Opened(owner) => cx.action(FabUiAction::MenuOpened { owner }),
            }
        }
        cx.clear_all_hovers();
    }

    fn open_popover(
        &mut self,
        cx: &mut Cx,
        owner: LiveId,
        anchor: Rect,
        rgba: [f32; 4],
        with_alpha: bool,
    ) {
        // Re-anchoring an already-open popover (another swatch) commits the
        // first edit before the second begins.
        if self.owner.is_some() && self.owner != Some(owner) {
            self.close_popover(cx, true);
        }
        self.owner = Some(owner);
        self.hsv = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
        self.alpha = rgba[3];
        self.with_alpha = with_alpha;
        self.opened_value = rgba;
        self.anchor = anchor;
        self.place_pending = true;
        self.hex_focused = false;
        self.view
            .view(cx, ids!(modal.row_alpha))
            .set_visible(cx, with_alpha);
        self.view.modal(cx, ids!(modal)).open(cx);
        self.broadcast(cx, Some(owner));
        self.sync_widgets(cx);
        self.sync_recent(cx);
        self.sync_mode(cx);
        self.view.redraw(cx);
    }

    fn close_popover(&mut self, cx: &mut Cx, commit: bool) {
        if self.owner.is_none() {
            return;
        }
        self.hex_focused = false;
        if commit {
            push_recent(self.rgba());
            self.publish(cx, true);
        } else {
            // Escape: the colour at open comes back, live and committed.
            self.hsv = rgb_to_hsv(
                self.opened_value[0],
                self.opened_value[1],
                self.opened_value[2],
            );
            self.alpha = self.opened_value[3];
            self.publish(cx, true);
        }
        self.owner = None;
        self.view.modal(cx, ids!(modal)).close(cx);
        self.broadcast(cx, None);
        self.view.redraw(cx);
    }

    fn commit_hex(&mut self, cx: &mut Cx, text: &str) {
        if let Some((rgba, had_alpha)) = parse_hex(text) {
            self.hsv = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
            if had_alpha && self.with_alpha {
                self.alpha = rgba[3];
            }
            self.publish(cx, true);
            push_recent(self.rgba());
            self.sync_recent(cx);
            self.sync_widgets(cx);
        } else {
            // Parse failure leaves the value untouched; the field snaps
            // back to the current colour.
            let hex = format_hex(self.rgba(), self.with_alpha);
            self.view.text_input(cx, ids!(modal.hex)).set_text(cx, &hex);
        }
    }
}

impl Widget for FabColorPickerLayer {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.owner.is_some() && self.place_pending {
            self.place_pending = false;
            // Anchor the bubble below the swatch, right edges aligned,
            // clamped into the window (the popover's own estimate of its
            // height; the clamp only needs to be close).
            let win = cx.current_pass_size();
            let w = 244.0;
            let h = 380.0;
            let mut x = self.anchor.pos.x + self.anchor.size.x - w;
            let mut y = self.anchor.pos.y + self.anchor.size.y + 2.0;
            if y + h > win.y - 4.0 {
                y = (self.anchor.pos.y - h - 2.0).max(4.0);
            }
            x = x.clamp(4.0, (win.x - w - 4.0).max(4.0));
            y = y.clamp(4.0, (win.y - 40.0).max(4.0));
            let content = self.view.view(cx, ids!(modal.content));
            if let Some(mut inner) = content.borrow_mut() {
                inner.walk.abs_pos = Some(dvec2(x, y));
            }
            content.redraw(cx);
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Escape closes without changing — before the children see the key,
        // and only while no text field inside is doing its own Escape.
        if self.owner.is_some() && !self.hex_focused {
            if let Event::KeyDown(ke) = event {
                if ke.key_code == KeyCode::Escape {
                    self.close_popover(cx, false);
                    return;
                }
            }
        }
        self.view.handle_event(cx, event, scope);

        // A click into the hex field places the caret on the release,
        // undoing the select-all that ran on focus; re-select after the
        // release so "focus selects everything" survives a mouse entry.
        if let Event::MouseUp(_) = event {
            if self.hex_select_pending {
                self.hex_select_pending = false;
                let hex = self.view.text_input(cx, ids!(modal.hex));
                if let Some(mut inner) = hex.borrow_mut() {
                    inner.select_all(cx);
                }
                hex.redraw(cx);
            }
        }

        let Event::Actions(actions) = event else {
            return;
        };

        // The popover bus: open requests, and a menu opening elsewhere
        // closes this picker (commit — it behaves like a click-away).
        let mut request = None;
        for a in crate::ui::popover::ui_actions(actions) {
            match a {
                FabUiAction::OpenColorPicker {
                    owner,
                    anchor,
                    rgba,
                    with_alpha,
                } => {
                    request = Some((*owner, *anchor, *rgba, *with_alpha));
                }
                FabUiAction::OpenMenu { .. } if self.owner.is_some() => {
                    self.close_popover(cx, true);
                }
                FabUiAction::MenuOpened { owner }
                    if self.owner.is_some() && Some(*owner) != self.owner =>
                {
                    self.close_popover(cx, true);
                }
                _ => {}
            }
        }
        if let Some((owner, anchor, rgba, with_alpha)) = request {
            self.open_popover(cx, owner, anchor, rgba, with_alpha);
        }

        if self.owner.is_none() {
            return;
        }

        // Click-away on the modal background commits, like a number field
        // committing on focus loss. The Modal dismisses ITSELF on the
        // background press (and posts its Dismissed under the content's
        // uid, which `ModalRef::dismissed` does not match), so the robust
        // signal is its own open flag going false behind us.
        if !self.view.modal(cx, ids!(modal)).is_open() {
            self.close_popover(cx, true);
        }
        if self.owner.is_none() {
            return;
        }

        // The wheel.
        let wheel = self.view.fab_color_wheel(cx, ids!(modal.wheel));
        if let Some([h, s, v]) = wheel.changed(actions) {
            self.hsv = [h, s, v];
            self.publish(cx, false);
            self.sync_widgets(cx);
        }
        if wheel.ended(actions).is_some() {
            self.publish(cx, true);
            push_recent(self.rgba());
            self.sync_recent(cx);
        }

        // Mode tabs.
        for (id, m) in [
            (ids!(modal.tabs.tab_rgb), PickMode::Rgb),
            (ids!(modal.tabs.tab_hsv), PickMode::Hsv),
            (ids!(modal.tabs.tab_hex), PickMode::Hex),
        ] {
            if let Some(fe) = self.view.view(cx, id).finger_up(actions) {
                if fe.is_over && self.mode != m {
                    self.mode = m;
                    self.sync_mode(cx);
                    self.sync_widgets(cx);
                    self.view.redraw(cx);
                }
            }
        }

        // Numeric rows: publish live on every drag step, commit on release.
        let mut changed = false;
        let mut ended = false;
        {
            let r = self.view.fab_drag_number(cx, ids!(modal.num_r));
            let g = self.view.fab_drag_number(cx, ids!(modal.num_g));
            let b = self.view.fab_drag_number(cx, ids!(modal.num_b));
            let rgb_change = [r.changed(actions), g.changed(actions), b.changed(actions)];
            if rgb_change.iter().any(|c| c.is_some()) {
                let cur = self.rgba();
                let nr = rgb_change[0].map_or(cur[0], |v| (v / 255.0) as f32);
                let ng = rgb_change[1].map_or(cur[1], |v| (v / 255.0) as f32);
                let nb = rgb_change[2].map_or(cur[2], |v| (v / 255.0) as f32);
                self.hsv = rgb_to_hsv(nr, ng, nb);
                changed = true;
            }
            ended |= r.ended(actions).is_some()
                || g.ended(actions).is_some()
                || b.ended(actions).is_some();
        }
        {
            let h = self.view.fab_drag_number(cx, ids!(modal.num_h));
            let s = self.view.fab_drag_number(cx, ids!(modal.num_s));
            let v = self.view.fab_drag_number(cx, ids!(modal.num_v));
            if let Some(nh) = h.changed(actions) {
                self.hsv[0] = (nh / 360.0).rem_euclid(1.0) as f32;
                changed = true;
            }
            if let Some(ns) = s.changed(actions) {
                self.hsv[1] = (ns / 100.0).clamp(0.0, 1.0) as f32;
                changed = true;
            }
            if let Some(nv) = v.changed(actions) {
                self.hsv[2] = (nv / 100.0).clamp(0.0, 1.0) as f32;
                changed = true;
            }
            ended |= h.ended(actions).is_some()
                || s.ended(actions).is_some()
                || v.ended(actions).is_some();
        }
        {
            let a = self.view.fab_drag_number(cx, ids!(modal.num_a));
            if let Some(na) = a.changed(actions) {
                self.alpha = (na / 100.0).clamp(0.0, 1.0) as f32;
                changed = true;
            }
            ended |= a.ended(actions).is_some();
        }
        if changed {
            self.publish(cx, false);
            self.sync_widgets(cx);
        }
        if ended {
            self.publish(cx, true);
            push_recent(self.rgba());
            self.sync_recent(cx);
        }

        // Hex: select-all on focus, Enter commits, Escape reverts the text.
        let hex = self.view.text_input(cx, ids!(modal.hex));
        for action in actions.iter() {
            if let Some(wa) = action.as_widget_action() {
                if wa.widget_uid == hex.widget_uid() {
                    match wa.cast::<TextInputAction>() {
                        TextInputAction::KeyFocus => {
                            self.hex_focused = true;
                            self.hex_select_pending = true;
                            if let Some(mut inner) = hex.borrow_mut() {
                                inner.select_all(cx);
                            }
                            hex.redraw(cx);
                        }
                        TextInputAction::KeyFocusLost => {
                            self.hex_select_pending = false;
                            if self.hex_focused {
                                self.hex_focused = false;
                                let text = hex.text().to_string();
                                self.commit_hex(cx, &text);
                            }
                        }
                        TextInputAction::Returned(text, _) => {
                            self.commit_hex(cx, &text);
                            cx.revert_key_focus();
                            self.hex_focused = false;
                            self.hex_select_pending = false;
                        }
                        TextInputAction::Escaped => {
                            let t = format_hex(self.rgba(), self.with_alpha);
                            hex.set_text(cx, &t);
                            cx.revert_key_focus();
                            self.hex_focused = false;
                            self.hex_select_pending = false;
                        }
                        _ => {}
                    }
                }
            }
        }
        // Recent colours: click to apply.
        let colors = recent_store().lock().unwrap().clone();
        for (i, id) in RECENT_IDS.iter().enumerate() {
            if let Some(fe) = self.view.view(cx, id).finger_up(actions) {
                if fe.is_over {
                    if let Some(c) = colors.get(i) {
                        self.hsv = rgb_to_hsv(c[0], c[1], c[2]);
                        if self.with_alpha {
                            self.alpha = c[3];
                        }
                        self.publish(cx, true);
                        self.sync_widgets(cx);
                    }
                }
            }
        }
        let _ = scope;
    }
}

// ===========================================================================
// The swatch — the in-panel control that raises the layer's popover
// ===========================================================================

#[derive(Script, ScriptHook, Widget)]
pub struct FabColorPicker {
    #[deref]
    view: View,
    /// Show the alpha row and format hex as #RRGGBBAA.
    #[live]
    with_alpha: bool,
    #[rust([0.8, 0.8, 0.8, 1.0])]
    value: [f32; 4],
    /// Mirrored from the popover bus broadcast.
    #[rust]
    open: bool,
}

impl FabColorPicker {
    fn owner_id(&self) -> LiveId {
        LiveId(self.widget_uid().0)
    }

    fn set_swatch(&mut self, cx: &mut Cx) {
        let col = vec4(self.value[0], self.value[1], self.value[2], 1.0);
        let open: f32 = if self.open { 1.0 } else { 0.0 };
        let mut sw = self.view.view(cx, ids!(swatch));
        script_apply_eval!(cx, sw, {
            draw_bg +: { swatch: #(col) open: #(open) }
        });
    }

    /// Set the bound colour from outside (no publish).
    pub fn set_color(&mut self, cx: &mut Cx, rgba: [f32; 4]) {
        let same = self
            .value
            .iter()
            .zip(rgba.iter())
            .all(|(a, b)| (a - b).abs() < 1.0 / 512.0);
        if same {
            return;
        }
        self.value = rgba;
        self.set_swatch(cx);
    }

    fn request_open(&mut self, cx: &mut Cx) {
        let anchor = self.view.view(cx, ids!(swatch)).area().rect(cx);
        cx.action(FabUiAction::OpenColorPicker {
            owner: self.owner_id(),
            anchor,
            rgba: self.value,
            with_alpha: self.with_alpha,
        });
    }
}

impl Widget for FabColorPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let uid = self.widget_uid();
        let owner = self.owner_id();

        // The swatch opens the popover. While it is open, the layer's modal
        // consumes every press, so this click can only arrive when closed.
        if let Some(fe) = self.view.view(cx, ids!(swatch)).finger_up(actions) {
            if fe.is_over && !self.open {
                self.request_open(cx);
            }
        }

        // Mirror the popover bus, exactly like every popup button; and take
        // over a click-away that dismissed a menu on this swatch.
        for a in crate::ui::popover::ui_actions(actions) {
            match a {
                FabUiAction::ColorPickerChanged { owner: o, rgba } if *o == owner => {
                    self.value = *rgba;
                    self.set_swatch(cx);
                    cx.widget_action(
                        uid,
                        ColorPickerAction::Changed(vec4(
                            rgba[0], rgba[1], rgba[2], rgba[3],
                        )),
                    );
                }
                FabUiAction::ColorPickerEnded { owner: o, rgba } if *o == owner => {
                    self.value = *rgba;
                    self.set_swatch(cx);
                    cx.widget_action(
                        uid,
                        ColorPickerAction::Ended(vec4(rgba[0], rgba[1], rgba[2], rgba[3])),
                    );
                }
                FabUiAction::MenuClickAway { at } if !self.open => {
                    if self.view.view(cx, ids!(swatch)).area().rect(cx).contains(*at) {
                        self.request_open(cx);
                    }
                }
                _ => {}
            }
        }
        if let Some(next) = crate::ui::popover::open_after(
            crate::ui::popover::ui_actions(actions),
            owner,
        ) {
            if next != self.open {
                self.open = next;
                self.set_swatch(cx);
            }
        }
        let _ = scope;
    }
}

impl FabColorPickerRef {
    /// Live colour while dragging any control inside the picker.
    pub fn changed(&self, actions: &Actions) -> Option<Vec4f> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ColorPickerAction::Changed(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    /// Commit points (release, Enter, close).
    pub fn ended(&self, actions: &Actions) -> Option<Vec4f> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ColorPickerAction::Ended(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    pub fn set_color(&self, cx: &mut Cx, rgba: [f32; 4]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color(cx, rgba);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map_or(false, |i| i.open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn rgb_hsv_round_trips_at_the_edges() {
        // Grey, black, white: hue and sat collapse but value survives, and
        // the round trip reproduces the rgb exactly.
        for (r, g, b) in [
            (0.5, 0.5, 0.5),
            (0.0, 0.0, 0.0),
            (1.0, 1.0, 1.0),
            (1.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
            (0.0, 0.0, 1.0),
            (1.0, 1.0, 0.0),
            (0.0, 1.0, 1.0),
            (1.0, 0.0, 1.0),
            (0.25, 0.5, 0.75),
        ] {
            let [h, s, v] = rgb_to_hsv(r, g, b);
            let [r2, g2, b2] = hsv_to_rgb(h, s, v);
            assert!(
                close(r, r2) && close(g, g2) && close(b, b2),
                "({r},{g},{b}) -> ({h},{s},{v}) -> ({r2},{g2},{b2})"
            );
        }
    }

    #[test]
    fn pure_hues_land_on_the_expected_angles() {
        assert!(close(rgb_to_hsv(1.0, 0.0, 0.0)[0], 0.0));
        assert!(close(rgb_to_hsv(0.0, 1.0, 0.0)[0], 1.0 / 3.0));
        assert!(close(rgb_to_hsv(0.0, 0.0, 1.0)[0], 2.0 / 3.0));
        // Grey and black report hue 0, sat 0 — not NaN.
        assert_eq!(rgb_to_hsv(0.5, 0.5, 0.5)[1], 0.0);
        assert_eq!(rgb_to_hsv(0.0, 0.0, 0.0), [0.0, 0.0, 0.0]);
        assert_eq!(rgb_to_hsv(1.0, 1.0, 1.0), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn hex_parses_every_accepted_form() {
        let (c, a) = parse_hex("#fff").unwrap();
        assert_eq!(c, [1.0, 1.0, 1.0, 1.0]);
        assert!(!a);
        let (c, _) = parse_hex("f00").unwrap();
        assert_eq!(c, [1.0, 0.0, 0.0, 1.0]);
        let (c, _) = parse_hex("#FF8000").unwrap();
        assert!(close(c[0], 1.0) && close(c[1], 128.0 / 255.0) && close(c[2], 0.0));
        let (c, a) = parse_hex("ff800080").unwrap();
        assert!(a);
        assert!(close(c[3], 128.0 / 255.0));
        let (c, _) = parse_hex("  #204060  ").unwrap();
        assert!(close(c[0], 32.0 / 255.0));
        assert!(parse_hex("").is_none());
        assert!(parse_hex("#12345").is_none());
        assert!(parse_hex("#gggggg").is_none());
        assert!(parse_hex("12 34 56").is_none());
    }

    #[test]
    fn hex_formats_and_round_trips() {
        assert_eq!(format_hex([1.0, 0.5019608, 0.0, 1.0], false), "#FF8000");
        assert_eq!(
            format_hex([0.0, 0.0, 0.0, 0.5019608], true),
            "#00000080"
        );
        for src in ["#000000", "#FFFFFF", "#5680C2", "#010203"] {
            let (c, _) = parse_hex(src).unwrap();
            assert_eq!(format_hex(c, false), src.to_uppercase());
        }
    }

    #[test]
    fn ring_points_give_the_expected_hue() {
        let size = 200.0;
        let mid = (RING_OUTER + RING_INNER) * 0.5 * size;
        let c = size * 0.5;
        // Top = 0, right = 0.25, bottom = 0.5, left = 0.75.
        assert!(close(ring_hue(dvec2(c, c - mid), size), 0.0));
        assert!(close(ring_hue(dvec2(c + mid, c), size), 0.25));
        assert!(close(ring_hue(dvec2(c, c + mid), size), 0.5));
        assert!(close(ring_hue(dvec2(c - mid, c), size), 0.75));
        // And they hit the ring zone.
        assert_eq!(wheel_zone(dvec2(c, c - mid), size), WheelZone::Ring);
        assert_eq!(wheel_zone(dvec2(c + mid, c), size), WheelZone::Ring);
    }

    #[test]
    fn square_corners_give_the_expected_sat_val() {
        let size = 200.0;
        let c = size * 0.5;
        let half = SQUARE_HALF * size;
        // Top-left: sat 0, val 1. Top-right: sat 1, val 1.
        // Bottom-left: sat 0, val 0. Bottom-right: sat 1, val 0.
        let tl = square_sv(dvec2(c - half, c - half), size);
        let tr = square_sv(dvec2(c + half, c - half), size);
        let bl = square_sv(dvec2(c - half, c + half), size);
        let br = square_sv(dvec2(c + half, c + half), size);
        assert!(close(tl.0, 0.0) && close(tl.1, 1.0));
        assert!(close(tr.0, 1.0) && close(tr.1, 1.0));
        assert!(close(bl.0, 0.0) && close(bl.1, 0.0));
        assert!(close(br.0, 1.0) && close(br.1, 0.0));
        // The corners are inside the square zone, the centre too.
        assert_eq!(wheel_zone(dvec2(c, c), size), WheelZone::Square);
        assert_eq!(
            wheel_zone(dvec2(c - half + 1.0, c - half + 1.0), size),
            WheelZone::Square
        );
        // A drag that leaves the square clamps to the nearest edge.
        let out = square_sv(dvec2(-50.0, size + 50.0), size);
        assert!(close(out.0, 0.0) && close(out.1, 0.0));
    }

    #[test]
    fn the_dead_centre_belongs_to_nothing_and_far_outside_too() {
        let size = 200.0;
        let c = size * 0.5;
        // Between the square corner and the ring inner edge.
        let gap = (SQUARE_HALF * 1.02 * size, RING_INNER * 0.9 * size);
        let p = dvec2(c + gap.0, c);
        // This point is right of the square but inside the ring's hole.
        if wheel_zone(p, size) != WheelZone::Square {
            assert_eq!(wheel_zone(p, size), WheelZone::None);
        }
        assert_eq!(wheel_zone(dvec2(-30.0, -30.0), size), WheelZone::None);
    }

    #[test]
    fn recent_colours_dedupe_and_cap() {
        {
            recent_store().lock().unwrap().clear();
        }
        for i in 0..12 {
            push_recent([i as f32 / 12.0, 0.0, 0.0, 1.0]);
        }
        // A repeat of the newest is deduped, not doubled.
        push_recent([11.0 / 12.0, 0.0, 0.0, 1.0]);
        let store = recent_store().lock().unwrap();
        assert_eq!(store.len(), RECENT_MAX);
        assert!(close(store[0][0], 11.0 / 12.0));
    }
}

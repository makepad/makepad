//! Progress — bars, rings, arcs, activity rings, gauges and a navigation
//! line: every widget that answers "how far along is it".
//!
//! One module because they are ONE drawing idea in five shapes. A bar, a
//! ring and a gauge all have a track, a fill that covers `value` of it, and a
//! colour role that says whether the thing being measured is fine, slow,
//! worrying or failed. So the shaders share an instance layout — `start`,
//! `end`, `value`, `intent`, `track`, `opacity` — and a bar with stacked
//! segments, a ring cut into sections and three nested activity rings are all
//! the same quad drawn a few times with different fractions. Nothing here
//! takes input: a progress widget is a readout, and a control that can be
//! dragged lives in `slider.rs`.
//!
//! The VALUE that is shown is not the value that was set. A set value eases
//! into place over `theme.motion_medium_1` (a NextFrame chain, not animator
//! states, because the destination is a number the host chooses at run time
//! rather than one of two poses) so a download that reports in bursts still
//! reads as a bar that moves. The easing only ever runs FORWARD: a value set
//! below the one on screen snaps there at once, because a bar that slides
//! backwards looks like a bar that is lying, while a reset is a thing a host
//! does on purpose and should look like one.
//!
//! A value below zero means INDETERMINATE: the shader replaces the fill with
//! a segment that sweeps the track on the pass clock, the same way
//! `loading_spinner.rs` turns its arc, so nothing has to pump frames from
//! Rust while a bar waits for a first byte.
//!
//! The intent colours come straight from the theme's accent roles
//! (`color_primary`, `color_success`, `color_warning`, `color_error`,
//! `color_info`) so a bar that goes red goes the same red as every alert.
//! The NAVIGATION line is the one stateful member: `start` puts it on
//! screen and it trickles toward — never reaching — the ceiling until
//! `complete` sends it to the end and fades it, the pattern every browser
//! uses for a page that has not said how long it will take.

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

/// A progress widget's actions.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ProgressAction {
    /// The value was set to (or advanced onto) the end, or the navigation
    /// line arrived there after `complete`.
    Completed,
    #[default]
    None,
}

/// The colour role a fill speaks in. Every intent is one of the theme's
/// accent roles, so a warning here is the same colour as a warning anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Default, Script, ScriptHook)]
pub enum Intent {
    #[pick]
    #[default]
    Primary,
    Success,
    Warning,
    Error,
    Info,
}

impl Intent {
    /// The shader's colour selector; `5.0` past the end is the disabled ink.
    pub fn index(self) -> f32 {
        match self {
            Intent::Primary => 0.0,
            Intent::Success => 1.0,
            Intent::Warning => 2.0,
            Intent::Error => 3.0,
            Intent::Info => 4.0,
        }
    }

    /// The nth role, cycling, for stacked segments and nested rings.
    pub fn nth(index: usize) -> Intent {
        match index % 5 {
            0 => Intent::Primary,
            1 => Intent::Success,
            2 => Intent::Warning,
            3 => Intent::Error,
            _ => Intent::Info,
        }
    }
}

const DISABLED_INK: f32 = 5.0;
script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.Intent = set_type_default() do #(Intent::script_api(vm))
    mod.widgets.splat(mod.widgets.Intent)

    mod.widgets.DrawProgressBarBase = #(DrawProgressBar::script_component(vm))
    set_type_default() do #(DrawProgressBar::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.ProgressBarBase = #(ProgressBar::register_widget(vm))
    /** The flat progress bar: a track and a fill, a percentage or a label
     * beside it, an indeterminate sweep while the value is unknown. */
    mod.widgets.ProgressBarFlat = set_type_default() do mod.widgets.ProgressBarBase{
        width: Fill
        height: 8
        align: Align{x: 0.0, y: 0.5}
        /** where the fill stands; anything below zero is indeterminate -1..1 step 0.01 */
        value: 0.0
        /** the colour role of the fill */
        intent: mod.widgets.Intent.Primary
        /** show the value as a percentage beside the bar 0..1 step 1 */
        show_percent: false
        /** a fixed label beside the bar; wins over the percentage */
        text: ""
        /** space between the bar and its label 0..24 step 1 */
        label_gap: theme.space_2
        /** room reserved for the label 0..120 step 1 */
        label_width: 34.0
        /** stacked sections as fractions of the whole; drawn instead of `value` when given */
        segments: []
        /** seconds a value change takes to arrive 0..2 step 0.05 */
        ease_secs: theme.motion_medium_1
        /** greyed and dimmed 0..1 step 1 */
        disabled: false

        // `start`..`opacity` are the instances: they ride in the draw struct,
        // so every other prop here has to be a uniform or the instance
        // slots stop lining up with the struct (see drop_toggles.rs).
        draw_bg +: {
            start: 0.0
            end: 1.0
            value: 0.0
            intent: 0.0
            track: 1.0
            opacity: 1.0
            /** bar height inside the walk 1..32 step 0.5 */
            thickness: uniform(8.0)
            /** corner radius; the full radius makes a pill 0..16 step 0.5 */
            border_radius: uniform(theme.radius_full)
            /** clear space between the end of the fill and the track 0..8 step 0.5 */
            gap: uniform(0.0)
            /** a dot at the far end of the track 0..1 step 1 */
            stop_indicator: uniform(0.0)
            /** indeterminate sweep: the fraction of the track the segment covers 0.1..0.8 step 0.05 */
            sweep_width: uniform(0.35)
            /** indeterminate sweep: passes per second 0.2..3 step 0.1 */
            sweep_speed: uniform(0.7)
            /** shade amount of the gradient variants 0..1 step 0.05 */
            gradient: uniform(0.0)
            /** gradient axis: 1 runs left to right 0..1 step 1 */
            gradient_horizontal: uniform(0.0)
            /** how dark the far gradient stop is 0.3..1 step 0.05 */
            gradient_shade: uniform(0.65)
            /** the track ink */
            track_color: uniform(theme.color_surface_container_highest)
            color_primary: uniform(theme.color_primary)
            color_success: uniform(theme.color_success)
            color_warning: uniform(theme.color_warning)
            color_error: uniform(theme.color_error)
            color_info: uniform(theme.color_info)
            color_disabled: uniform(theme.color_val_disabled)
            /** bevel stroke on the track; a zero alpha draws none */
            border_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            /** second bevel stop, mixed in top to bottom */
            border_color_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))

            fill_color: fn() -> vec4 {
                let mut fill = self.color_primary
                if self.intent > 0.5 { fill = self.color_success }
                if self.intent > 1.5 { fill = self.color_warning }
                if self.intent > 2.5 { fill = self.color_error }
                if self.intent > 3.5 { fill = self.color_info }
                if self.intent > 4.5 { fill = self.color_disabled }
                let axis = mix(self.pos.y, self.pos.x, self.gradient_horizontal)
                return mix(fill, vec4(fill.xyz * self.gradient_shade, fill.w), self.gradient * axis)
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = min(self.thickness, self.rect_size.y)
                let y0 = (self.rect_size.y - h) * 0.5
                let r = min(self.border_radius, h * 0.5)
                let fill = self.fill_color()
                let x0 = w * self.start
                let x1 = w * self.end
                let xv = w * clamp(self.value, self.start, self.end)
                if self.track > 0.5 {
                    // With a gap the track starts a little past the fill,
                    // so both keep their rounded ends and a sliver of
                    // background shows between them.
                    let mut tx = x0
                    if self.gap > 0.0 {
                        if self.value > self.start {
                            tx = min(xv + self.gap, x1)
                        }
                    }
                    if x1 - tx > 0.5 {
                        sdf.box(tx, y0, x1 - tx, h, r)
                        // fill_KEEP only when a stroke follows to consume
                        // the shape: a kept shape unions with the next box
                        // and the fill would paint the whole track.
                        if self.border_color.w > 0.0 {
                            sdf.fill_keep(self.track_color)
                            sdf.stroke(mix(self.border_color, self.border_color_2, self.pos.y), 1.0)
                        } else {
                            sdf.fill(self.track_color)
                        }
                    }
                }
                if self.value >= 0.0 {
                    if xv > x0 {
                        // Never thinner than it is tall: a 1% fill is a
                        // dot, not a smear.
                        let fw = min(max(xv - x0, h), x1 - x0)
                        sdf.box(x0, y0, fw, h, r)
                        sdf.fill(fill)
                    }
                }
                if self.value < 0.0 {
                    let span = x1 - x0
                    let seg = span * self.sweep_width
                    let t = fract(self.draw_pass.time * self.sweep_speed)
                    let te = t * t * (3.0 - 2.0 * t)
                    let x = x0 - seg + (span + seg) * te
                    let a = max(x, x0)
                    let b = min(x + seg, x1)
                    if b - a > 0.5 {
                        sdf.box(a, y0, b - a, h, r)
                        sdf.fill(fill)
                    }
                }
                if self.stop_indicator > 0.5 {
                    if self.track > 0.5 {
                        sdf.circle(x1 - h * 0.5, y0 + h * 0.5, h * 0.5)
                        sdf.fill(fill)
                    }
                }
                return sdf.result * self.opacity
            }
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    /** The standard progress bar: the flat bar with the inset bevel on its track. */
    mod.widgets.ProgressBar = mod.widgets.ProgressBarFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_2: theme.color_bevel_inset_2
        }
    }

    /** The gradient bar: the standard bar with the fill shaded top to bottom. */
    mod.widgets.ProgressBarGradientX = mod.widgets.ProgressBar{
        draw_bg +: {
            gradient: 1.0
        }
    }

    /** The gradient bar turned sideways: the same shade run left to right. */
    mod.widgets.ProgressBarGradientY = mod.widgets.ProgressBarGradientX{
        draw_bg +: {
            gradient_horizontal: 1.0
        }
    }

}

/// The bar's shader. The instances are a piece of a bar: the fraction it
/// covers, where its fill stands, its colour role, whether it draws the
/// track under itself. A plain bar is one piece from 0 to 1.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawProgressBar {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    start: f32,
    #[live]
    end: f32,
    #[live]
    value: f32,
    #[live]
    intent: f32,
    #[live]
    track: f32,
    #[live]
    opacity: f32,
}

/// The eased journey from the value that was on screen to the one that was
/// set: a cubic ease-out over `ease_secs`, forward only.
#[derive(Default)]
struct Ease {
    from: f64,
    started: f64,
    running: bool,
}

impl Ease {
    /// Where the shown value stands at `now` on the way to `target`;
    /// `None` once it has arrived (and the ease switches itself off).
    fn step(&mut self, now: f64, secs: f64, target: f64) -> Option<f64> {
        if !self.running {
            return None;
        }
        let t = if secs <= 0.0 {
            1.0
        } else {
            ((now - self.started) / secs).clamp(0.0, 1.0)
        };
        if t >= 1.0 {
            self.running = false;
            return None;
        }
        let e = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
        Some(self.from + (target - self.from) * e)
    }
}

/// A DSL list of numbers (`[0.3, 0.2]`) as f64s; anything that is not a
/// number is skipped rather than read as zero.
fn numbers(list: &[ScriptValue]) -> Vec<f64> {
    list.iter().filter_map(|v| v.as_number()).collect()
}

fn script_numbers(values: &[f64]) -> Vec<ScriptValue> {
    values.iter().map(|v| ScriptValue::from(*v)).collect()
}

fn percent_label(value: f64) -> String {
    format!("{}%", (value.clamp(0.0, 1.0) * 100.0).round() as i64)
}

fn two_decimals(value: f64) -> String {
    format!("{value:.2}")
}

/// The size `text` takes in this style, in layout points.
fn text_size(cx: &mut Cx2d, draw_text: &DrawText, text: &str) -> DVec2 {
    if text.is_empty() {
        return dvec2(0.0, 0.0);
    }
    let laid = draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    let scale = draw_text.font_scale as f64;
    dvec2(laid.size_in_lpxs.width as f64 * scale, laid.size_in_lpxs.height as f64 * scale)
}

/// Draw `text` inside an absolute box, placed by `align`. Measured first
/// and then walked at an absolute position, which places it without moving
/// the enclosing turtle: a nested turtle would advance the row it sits in.
fn draw_text_in(
    cx: &mut Cx2d,
    draw_text: &mut DrawText,
    rect: Rect,
    align: Align,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let laid = draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    let scale = draw_text.font_scale as f64;
    let size = dvec2(laid.size_in_lpxs.width as f64 * scale, laid.size_in_lpxs.height as f64 * scale);
    let pos = rect.pos + dvec2((rect.size.x - size.x) * align.x, (rect.size.y - size.y) * align.y);
    draw_text.draw_walk_laidout(cx, Walk::fixed(size.x, size.y).with_abs_pos(pos), &laid);
}

/// A bar with a track and a fill. `value` is the target; what is drawn is
/// the eased value on its way there.
#[derive(Script, ScriptHook, Widget)]
pub struct ProgressBar {
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
    draw_bg: DrawProgressBar,
    #[live]
    draw_text: DrawText,
    /// A fixed label beside the bar; wins over the percentage.
    #[live]
    pub text: String,
    /// The target, 0..1; below zero is indeterminate.
    #[live]
    pub value: f64,
    #[live(Intent::Primary)]
    pub intent: Intent,
    #[live]
    pub show_percent: bool,
    #[live]
    pub label_gap: f64,
    #[live]
    pub label_width: f64,
    /// Stacked sections as fractions of the whole; when non-empty they are
    /// drawn instead of `value`. Script values because the DSL hands lists
    /// over that way; `numbers` reads them.
    #[live]
    pub segments: Vec<ScriptValue>,
    #[live]
    pub ease_secs: f64,
    #[live]
    pub disabled: bool,
    /// What is on screen. Trails `value` through the ease.
    #[rust]
    shown: f64,
    /// The `value` the shown value was last aimed at. A script apply that
    /// changes `value` is noticed by comparing the two at draw time.
    #[rust]
    target: f64,
    #[rust]
    ease: Ease,
    #[rust]
    drawn: bool,
    #[rust]
    next_frame: NextFrame,
}

impl ProgressBar {
    /// The label the bar shows: the fixed text, else the percentage, else
    /// nothing. The percentage reads the SHOWN value so it moves with the
    /// fill; an indeterminate bar has no percentage to show.
    pub fn label_text(&self) -> String {
        if !self.text.is_empty() {
            self.text.clone()
        } else if self.show_percent && self.shown >= 0.0 {
            percent_label(self.shown)
        } else {
            String::new()
        }
    }

    /// Set the target. A value at or above the shown one eases there; a
    /// lower one snaps, because the easing only ever runs forward. Any
    /// negative value is indeterminate and snaps too.
    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        let value = if value < 0.0 { -1.0 } else { value.min(1.0) };
        let was = self.value;
        self.value = value;
        self.aim(cx.seconds_since_app_start());
        if value >= 1.0 && was < 1.0 {
            let uid = self.widget_uid();
            cx.widget_action(uid, ProgressAction::Completed);
        }
        if self.ease.running {
            self.next_frame = cx.new_next_frame();
        }
        self.draw_bg.redraw(cx);
    }

    /// Move the target forward by `delta`, stopping at the end.
    pub fn advance(&mut self, cx: &mut Cx, delta: f64) {
        let base = if self.value < 0.0 { 0.0 } else { self.value };
        self.set_value(cx, (base + delta).min(1.0));
    }

    pub fn set_intent(&mut self, cx: &mut Cx, intent: Intent) {
        if self.intent != intent {
            self.intent = intent;
            self.draw_bg.redraw(cx);
        }
    }

    /// Point the shown value at `value`: forward by an ease from where it
    /// stands, backward (or into indeterminate) by a snap.
    fn aim(&mut self, now: f64) {
        self.target = self.value;
        if self.value < 0.0 || self.shown < 0.0 || self.value < self.shown || !self.drawn {
            self.shown = self.value;
            self.ease.running = false;
        } else if self.value > self.shown {
            self.ease = Ease {
                from: self.shown,
                started: now,
                running: true,
            };
        }
    }
}

impl Widget for ProgressBar {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.drawn || self.value != self.target {
            // The first draw, or a script apply changed `value` behind the
            // setter's back: aim from here so the ease starts on time.
            let now = cx.seconds_since_app_start();
            let first = !self.drawn;
            self.drawn = true;
            self.aim(now);
            if self.ease.running {
                self.next_frame = cx.new_next_frame();
            }
            if first {
                self.ease.running = false;
            }
        }
        let label = self.label_text();
        let label_size = text_size(cx, &self.draw_text, &label);
        // A bar is thinner than its label: grow the walk to the text so a
        // Fit row does not clip the label to the bar's height.
        let mut walk = walk;
        if let Size::Fixed(h) = walk.height {
            if !label.is_empty() && h < label_size.y {
                walk.height = Size::Fixed(label_size.y);
            }
        }
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        let reserve = if label.is_empty() {
            0.0
        } else {
            self.label_width + self.label_gap
        };
        let bar_w = if rect.size.x > 0.0 {
            (rect.size.x - reserve).max(1.0)
        } else {
            120.0
        };
        let bar = Rect {
            pos: rect.pos,
            size: dvec2(bar_w, rect.size.y.max(1.0)),
        };
        let intent = if self.disabled { DISABLED_INK } else { self.intent.index() };
        self.draw_bg.opacity = if self.disabled { 0.6 } else { 1.0 };
        if self.segments.is_empty() {
            self.draw_bg.start = 0.0;
            self.draw_bg.end = 1.0;
            self.draw_bg.value = self.shown as f32;
            self.draw_bg.intent = intent;
            self.draw_bg.track = 1.0;
            self.draw_bg.draw_abs(cx, bar);
        } else {
            // The track alone first, then one piece per segment over it.
            self.draw_bg.start = 0.0;
            self.draw_bg.end = 1.0;
            self.draw_bg.value = 0.0;
            self.draw_bg.intent = intent;
            self.draw_bg.track = 1.0;
            self.draw_bg.draw_abs(cx, bar);
            let mut at = 0.0f64;
            for (index, seg) in numbers(&self.segments).into_iter().enumerate() {
                let seg = seg.max(0.0);
                let end = (at + seg).min(1.0);
                if end > at {
                    self.draw_bg.start = at as f32;
                    self.draw_bg.end = end as f32;
                    self.draw_bg.value = end as f32;
                    self.draw_bg.intent = if self.disabled {
                        DISABLED_INK
                    } else {
                        Intent::nth(index).index()
                    };
                    self.draw_bg.track = 0.0;
                    self.draw_bg.draw_abs(cx, bar);
                }
                at = end;
            }
        }
        if !label.is_empty() {
            draw_text_in(
                cx,
                &mut self.draw_text,
                Rect {
                    pos: dvec2(bar.pos.x + bar_w + self.label_gap, rect.pos.y),
                    size: dvec2(self.label_width.max(1.0), rect.size.y.max(1.0)),
                },
                Align { x: 1.0, y: 0.5 },
                &label,
            );
        }
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(ne) = self.next_frame.is_event(event) {
            if let Some(shown) = self.ease.step(ne.time, self.ease_secs, self.target) {
                self.shown = shown;
                self.next_frame = cx.new_next_frame();
            } else {
                self.shown = self.target;
            }
            self.draw_bg.redraw(cx);
        }
    }

    fn text(&self) -> String {
        self.label_text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(two_decimals(self.value))
    }
}

impl ProgressBarRef {
    pub fn completed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast()),
            Some(ProgressAction::Completed)
        )
    }

    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value).unwrap_or(0.0)
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }

    pub fn advance(&self, cx: &mut Cx, delta: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.advance(cx, delta);
        }
    }

    pub fn set_intent(&self, cx: &mut Cx, intent: Intent) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_intent(cx, intent);
        }
    }

    pub fn set_segments(&self, cx: &mut Cx, segments: &[f64]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.segments = script_numbers(segments);
            inner.draw_bg.redraw(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_runs_forward_and_stops() {
        let mut ease = Ease { from: 0.2, started: 10.0, running: true };
        let mid = ease.step(10.125, 0.25, 0.6).unwrap();
        assert!(mid > 0.2 && mid < 0.6, "{mid}");
        assert!(ease.step(10.3, 0.25, 0.6).is_none());
        assert!(!ease.running);
        assert!(ease.step(10.4, 0.25, 0.6).is_none());
    }

    #[test]
    fn labels_round_and_format() {
        assert_eq!(percent_label(0.5), "50%");
        assert_eq!(percent_label(0.004), "0%");
        assert_eq!(percent_label(1.7), "100%");
        assert_eq!(two_decimals(0.1 * 5.0), "0.50");
        assert_eq!(two_decimals(-1.0), "-1.00");
    }

    #[test]
    fn intents_index_and_cycle() {
        assert_eq!(Intent::Primary.index(), 0.0);
        assert_eq!(Intent::Info.index(), 4.0);
        assert_eq!(Intent::nth(6), Intent::Success);
        assert!(DISABLED_INK > Intent::Info.index());
    }
}

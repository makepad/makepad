//! Spinner — the mark that says "working, no idea how long": a turning
//! arc, three pulsing dots or five rising bars, with a status variant that
//! ends in a tick or a cross, a saving indicator built from it, and an
//! overlay that dims and blocks a region while it loads.
//!
//! `loading_spinner.rs` stays as it is: it is a DSL-only View whose shader
//! parameters seven apps override by name, so its text cannot move. This
//! module is the Rust widget the library was missing — one that can be
//! ASKED things (is it showing, what does it say) and TOLD things (start,
//! finish, fail) instead of being toggled through `visible`.
//!
//! The three faces are one shader with a `face` selector rather than three
//! widgets, so the status spinner and the overlay get all of them for free
//! and a host can swap faces with a prop. The shader runs on the pass clock
//! (`draw_pass.time`), the same idiom as the loading spinner, so nothing
//! pumps frames from Rust while a spinner turns.
//!
//! DELAY: a spinner that appears for a 40 ms fetch is a flash, not
//! feedback. `delay_secs` keeps the mark at zero alpha until the delay has
//! elapsed since the spinner first drew, then fades it in; a host that
//! finishes inside the delay never shows anything. The delay is measured
//! from the first draw, not from construction, because a spinner in a
//! hidden tab must not be "already late" when the tab opens.
//!
//! The STATUS spinner morphs rather than swaps: the arc's gap closes into a
//! full ring while the tick or cross fades in over `motion_medium_1`, and
//! after `auto_reset_secs` it goes quiet again by itself, so a form that
//! saves ten times in a row does not have to remember to clear ten marks.
//!
//! The SAVING indicator is the status spinner plus a label and a retry
//! button, with the same debounce every editor uses: "Saving" only appears
//! if the save has taken longer than `debounce_secs`, so a fast autosave
//! reads as "Saved at 14:05" and nothing else.
//!
//! The LOADING overlay claims every pointer event over its content BEFORE
//! the content sees it (it is the parent, so it is asked first, and a hit it
//! takes is marked handled for everything asked after), which is the whole
//! reason it exists: a list that is being refreshed must not take a click.
//! Optionally it puts a blurred glass pane over the content instead of a
//! plain scrim; the pane's own spinner is a child of the pane because the
//! glass composites above anything its parent draws after it.

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

/// What a spinner draws inside its square.
#[derive(Clone, Copy, Debug, PartialEq, Default, Script, ScriptHook)]
pub enum SpinnerFace {
    /// A turning arc whose gap breathes.
    #[pick]
    #[default]
    Arc,
    /// Three dots pulsing in sequence.
    Dots,
    /// Five bars rising in a wave.
    Bars,
}

impl SpinnerFace {
    fn index(self) -> f32 {
        match self {
            SpinnerFace::Arc => 0.0,
            SpinnerFace::Dots => 1.0,
            SpinnerFace::Bars => 2.0,
        }
    }
}

const FACE_STATUS_SPINNING: f32 = 0.0;
const DEFAULT_SIZE: f64 = 24.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SpinnerFace = #(SpinnerFace::script_api(vm))

    mod.widgets.DrawSpinnerBase = #(DrawSpinner::script_component(vm))
    // The shader lives on the TYPE default, not on a widget, because three
    // widgets draw with it; each only re-tints. `face`..`contained` are the
    // instances (they ride in the draw struct), so everything else is a
    // uniform or the instance slots stop lining up with the struct.
    set_type_default() do #(DrawSpinner::script_shader(vm)){
        ..mod.draw.DrawQuad
        face: 0.0
        alpha: 1.0
        status: 0.0
        morph: 0.0
        contained: 0.0
        /** the ink while turning */
        color: uniform(theme.color_primary)
        /** the ink of the tick */
        color_success: uniform(theme.color_success)
        /** the ink of the cross */
        color_error: uniform(theme.color_error)
        /** stroke width; 0 scales it with the size 0..8 step 0.5 */
        stroke_width: uniform(0.0)
        /** turns, pulses or waves per second 0.2..3 step 0.1 */
        speed: uniform(1.0)
        /** how strongly the full circle shows under the arc 0..1 step 0.05 */
        track_alpha: uniform(0.15)
        /** the rounded container behind a contained spinner */
        container_color: uniform(theme.color_surface_container)
        /** corner radius of that container 0..24 step 0.5 */
        container_radius: uniform(theme.radius_m)

        bar_height: fn(i: float, w: float) -> float {
            return 0.35 + 0.65 * (0.5 + 0.5 * sin(w - i * 0.9))
        }

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let size = min(self.rect_size.x, self.rect_size.y)
            let center = self.rect_size * 0.5
            let mut ink = self.color
            if self.status > 0.5 { ink = self.color_success }
            if self.status > 1.5 { ink = self.color_error }
            let mut stroke = self.stroke_width
            if stroke <= 0.0 { stroke = max(1.5, size * 0.09) }
            if self.contained > 0.5 {
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.container_radius)
                sdf.fill(self.container_color)
            }
            let inset = self.contained * size * 0.2
            let radius = size * 0.5 - inset - stroke * 0.5
            // How much of the turning face is left; the mark takes the rest.
            let spin = 1.0 - self.morph
            let t = self.draw_pass.time * self.speed
            if self.face < 0.5 {
                // The loading spinner's motion: a turn with a breathing gap.
                let rotation = t * TAU
                let phase = fract(t * 0.5)
                let expand = clamp(phase / 0.55, 0.0, 1.0)
                let contract = clamp((phase - 0.55) / 0.45, 0.0, 1.0)
                let gap = mix(0.12, 0.92, expand * (1.0 - contract)) * TAU * spin
                if self.track_alpha > 0.0 {
                    sdf.circle(center.x, center.y, radius)
                    sdf.stroke(vec4(ink.xyz, ink.w * self.track_alpha * spin), stroke * 0.5)
                }
                sdf.arc_round_caps(center.x, center.y, radius, rotation, rotation + TAU - gap, stroke)
                sdf.fill(vec4(ink.xyz, ink.w * spin))
            }
            if self.face > 0.5 {
                if self.face < 1.5 {
                    // Three dots pulsing in sequence, the chat idiom.
                    let r = min(self.rect_size.x * 0.11, (self.rect_size.y - inset * 2.0) * 0.4)
                    let w = t * 5.2
                    let p1 = 0.25 + 0.75 * max(0.0, sin(w))
                    sdf.circle(self.rect_size.x * 0.2, center.y, r)
                    sdf.fill(vec4(ink.xyz, ink.w * p1 * spin))
                    let p2 = 0.25 + 0.75 * max(0.0, sin(w - 1.1))
                    sdf.circle(self.rect_size.x * 0.5, center.y, r)
                    sdf.fill(vec4(ink.xyz, ink.w * p2 * spin))
                    let p3 = 0.25 + 0.75 * max(0.0, sin(w - 2.2))
                    sdf.circle(self.rect_size.x * 0.8, center.y, r)
                    sdf.fill(vec4(ink.xyz, ink.w * p3 * spin))
                }
            }
            if self.face > 1.5 {
                // Five bars, equal to their gaps, rising in a wave.
                let bw = (self.rect_size.x - inset * 2.0) / 9.0
                let x0 = inset
                let h = self.rect_size.y - inset * 2.0
                let w = t * 6.0
                let bar = vec4(ink.xyz, ink.w * spin)
                let h0 = h * self.bar_height(0.0, w)
                sdf.box(x0, center.y - h0 * 0.5, bw, h0, bw * 0.5)
                sdf.fill(bar)
                let h1 = h * self.bar_height(1.0, w)
                sdf.box(x0 + bw * 2.0, center.y - h1 * 0.5, bw, h1, bw * 0.5)
                sdf.fill(bar)
                let h2 = h * self.bar_height(2.0, w)
                sdf.box(x0 + bw * 4.0, center.y - h2 * 0.5, bw, h2, bw * 0.5)
                sdf.fill(bar)
                let h3 = h * self.bar_height(3.0, w)
                sdf.box(x0 + bw * 6.0, center.y - h3 * 0.5, bw, h3, bw * 0.5)
                sdf.fill(bar)
                let h4 = h * self.bar_height(4.0, w)
                sdf.box(x0 + bw * 8.0, center.y - h4 * 0.5, bw, h4, bw * 0.5)
                sdf.fill(bar)
            }
            if self.morph > 0.001 {
                // The closed ring the mark sits in, then the mark itself.
                let m = self.morph
                sdf.circle(center.x, center.y, radius)
                sdf.stroke(vec4(ink.xyz, ink.w * m * 0.5), stroke * 0.5)
                let mark = vec4(ink.xyz, ink.w * m)
                if self.status > 1.5 {
                    let d = radius * 0.5
                    sdf.move_to(center.x - d, center.y - d)
                    sdf.line_to(center.x + d, center.y + d)
                    sdf.stroke(mark, stroke * 0.5)
                    sdf.move_to(center.x + d, center.y - d)
                    sdf.line_to(center.x - d, center.y + d)
                    sdf.stroke(mark, stroke * 0.5)
                } else {
                    sdf.move_to(center.x - radius * 0.55, center.y + radius * 0.05)
                    sdf.line_to(center.x - radius * 0.15, center.y + radius * 0.48)
                    sdf.line_to(center.x + radius * 0.6, center.y - radius * 0.42)
                    sdf.stroke(mark, stroke * 0.5)
                }
            }
            return sdf.result * self.alpha
        }
    }

    mod.widgets.SpinnerBase = #(Spinner::register_widget(vm))
    /** The flat spinner: a turning arc, sized by `size`, with an optional
     * word beside it and a delay before it shows. */
    mod.widgets.SpinnerFlat = set_type_default() do mod.widgets.SpinnerBase{
        width: Fit
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0.0, y: 0.5}
        /** the mark: a turning arc, three pulsing dots or five rising bars */
        face: mod.widgets.SpinnerFace.Arc
        /** side of the mark in points; 12, 16, 24, 32 and 48 are the xs..xl ladder 8..96 step 1 */
        size: 24.0
        /** seconds before anything shows, so a quick load never flashes 0..3 step 0.05 */
        delay_secs: 0.0
        /** seconds the fade-in takes once the delay has passed 0..1 step 0.05 */
        fade_secs: theme.motion_short_4
        /** a rounded container behind the mark 0..1 step 1 */
        contained: false
        /** a word beside the mark */
        text: ""
        /** dimmed 0..1 step 1 */
        disabled: false
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    /** Three dots pulsing in sequence; wider than tall by nature. */
    mod.widgets.SpinnerDots = mod.widgets.SpinnerFlat{
        face: mod.widgets.SpinnerFace.Dots
    }

    /** Five bars rising in a wave. */
    mod.widgets.SpinnerBars = mod.widgets.SpinnerFlat{
        face: mod.widgets.SpinnerFace.Bars
    }

    /** The arc on a rounded container, for sitting over content. */
    mod.widgets.SpinnerContained = mod.widgets.SpinnerFlat{
        contained: true
        size: 40.0
    }

}

/// The spinner's shader. `face` picks the mark, `alpha` is the delay fade,
/// `status`/`morph` carry the status spinner's tick or cross, `contained`
/// draws the container.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSpinner {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    face: f32,
    #[live]
    alpha: f32,
    #[live]
    status: f32,
    #[live]
    morph: f32,
    #[live]
    contained: f32,
}

/// The mark's alpha `now`, given when it first drew: nothing until the
/// delay has passed, then a fade over `fade_secs`.
fn delayed_alpha(now: f64, shown_since: f64, delay_secs: f64, fade_secs: f64) -> f64 {
    let t = now - shown_since - delay_secs.max(0.0);
    if t < 0.0 {
        0.0
    } else if fade_secs <= 0.0 {
        1.0
    } else {
        (t / fade_secs).clamp(0.0, 1.0)
    }
}

/// The turning mark, with a word beside it and a delay before it shows.
#[derive(Script, ScriptHook, Widget)]
pub struct Spinner {
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
    draw_bg: DrawSpinner,
    #[live]
    draw_text: DrawText,
    #[live]
    pub text: String,
    #[live(SpinnerFace::Arc)]
    pub face: SpinnerFace,
    /// Side of the mark in points; 0 falls back to 24.
    #[live]
    pub size: f64,
    #[live]
    pub delay_secs: f64,
    #[live]
    pub fade_secs: f64,
    #[live]
    pub contained: bool,
    #[live]
    pub disabled: bool,
    /// When it first drew, for the delay. `None` until then and again
    /// after `restart`.
    #[rust]
    shown_since: Option<f64>,
    /// The mark's alpha as last drawn, reported to the test tree.
    #[rust]
    alpha: f64,
    /// Armed while the alpha is still moving. The pass clock only repaints
    /// the GPU pass, it never re-runs draw code, so the delay and the fade
    /// need frames of their own until the mark is fully on screen.
    #[rust]
    next_frame: NextFrame,
}

impl Spinner {
    fn mark_size(&self) -> f64 {
        if self.size > 0.0 {
            self.size
        } else {
            DEFAULT_SIZE
        }
    }

    /// Arm the delay again: the next draw counts as the first.
    pub fn restart(&mut self, cx: &mut Cx) {
        self.shown_since = None;
        self.draw_bg.redraw(cx);
    }

    pub fn set_face(&mut self, cx: &mut Cx, face: SpinnerFace) {
        if self.face != face {
            self.face = face;
            self.draw_bg.redraw(cx);
        }
    }

    /// The mark's alpha this frame, and whether the delay and fade are
    /// over (after which the pass clock alone keeps the mark turning).
    fn alpha_now(&mut self, now: f64) -> (f64, bool) {
        let since = *self.shown_since.get_or_insert(now);
        let raw = delayed_alpha(now, since, self.delay_secs, self.fade_secs);
        let mut alpha = raw;
        if self.disabled {
            alpha *= 0.5;
        }
        (alpha, raw >= 1.0)
    }
}

impl Widget for Spinner {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let now = cx.seconds_since_app_start();
        let (alpha, settled) = self.alpha_now(now);
        self.alpha = alpha;
        if !settled {
            self.next_frame = cx.new_next_frame();
        }
        let s = self.mark_size();
        cx.begin_turtle(walk, self.layout);
        self.draw_bg.face = self.face.index();
        self.draw_bg.alpha = alpha as f32;
        self.draw_bg.status = FACE_STATUS_SPINNING;
        self.draw_bg.morph = 0.0;
        self.draw_bg.contained = if self.contained { 1.0 } else { 0.0 };
        // Dots read better wider than tall; the others are square.
        let mark = match self.face {
            SpinnerFace::Dots if !self.contained => Walk::fixed(s * 1.8, s * 0.6),
            _ => Walk::fixed(s, s),
        };
        self.draw_bg.draw_walk(cx, mark);
        if !self.text.is_empty() && alpha > 0.0 {
            self.draw_text
                .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &self.text);
        }
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            // The draw re-arms the chain while the alpha is still moving.
            self.draw_bg.redraw(cx);
        }
    }

    fn text(&self) -> String {
        self.text.clone()
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

    /// The mark's alpha with two decimals: "0.00" while the delay holds,
    /// "1.00" once it is fully on screen, so a test can wait for either.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!("{:.2}", self.alpha))
    }
}

impl SpinnerRef {
    pub fn restart(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.restart(cx);
        }
    }

    pub fn set_face(&self, cx: &mut Cx, face: SpinnerFace) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_face(cx, face);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_holds_then_fades() {
        assert_eq!(delayed_alpha(10.0, 10.0, 0.5, 0.2), 0.0);
        assert_eq!(delayed_alpha(10.4, 10.0, 0.5, 0.2), 0.0);
        let mid = delayed_alpha(10.6, 10.0, 0.5, 0.2);
        assert!(mid > 0.4 && mid < 0.6, "{mid}");
        assert_eq!(delayed_alpha(11.0, 10.0, 0.5, 0.2), 1.0);
        assert_eq!(delayed_alpha(10.0, 10.0, 0.0, 0.0), 1.0);
    }
}

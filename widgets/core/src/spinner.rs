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
//! The four faces are one shader with a `face` selector rather than four
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
//!
//! FAILED: the overlay can also say the load STOPPED. Without that a host
//! whose load fails has one move — tear the overlay down and put something
//! else in the hole — and in the meantime a scrim that stays up with an arc
//! turning on it is a lie about what the machine is doing. `fail` morphs
//! the arc into a cross through the status spinner that is already in this
//! file, because a stopped mark must never be mistaken for a slow one.

use crate::{
    button::*,
    gauss_view::{arm_gauss_capture, GaussRoundedView},
    label::*,
    makepad_derive_widget::*,
    makepad_draw::*, view::View, widget::*,
};
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// A bright head leading a tail that fades away round the ring. Wanted
    /// where the mark is the only thing moving on the screen — a splash, a
    /// full-window overlay — and the arc's breathing gap reads as a stutter.
    Comet,
}

impl SpinnerFace {
    fn index(self) -> f32 {
        match self {
            SpinnerFace::Arc => 0.0,
            SpinnerFace::Dots => 1.0,
            SpinnerFace::Bars => 2.0,
            SpinnerFace::Comet => 3.0,
        }
    }
}

/// Where a status spinner is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Default, Script, ScriptHook)]
pub enum SpinnerStatus {
    /// Nothing shows; the space is kept.
    #[pick]
    #[default]
    Inactive,
    /// Turning.
    Active,
    /// The arc became a tick.
    Finished,
    /// The arc became a cross.
    Error,
}

impl SpinnerStatus {
    fn name(self) -> &'static str {
        match self {
            SpinnerStatus::Inactive => "Inactive",
            SpinnerStatus::Active => "Active",
            SpinnerStatus::Finished => "Finished",
            SpinnerStatus::Error => "Error",
        }
    }
}

/// A status spinner's actions.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum StatusSpinnerAction {
    /// The tick is fully on screen.
    Finished,
    /// The cross is fully on screen.
    Failed,
    /// The mark went quiet by itself after `auto_reset_secs`.
    Reset,
    #[default]
    None,
}

/// What the saving indicator says.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SavingState {
    #[default]
    Idle,
    Saving,
    Saved,
    Failed,
}

/// A saving indicator's actions.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum SavingIndicatorAction {
    /// The retry button after a failure was pressed.
    Retry,
    #[default]
    None,
}

const FACE_STATUS_SPINNING: f32 = 0.0;
const FACE_STATUS_FINISHED: f32 = 1.0;
const FACE_STATUS_ERROR: f32 = 2.0;
const DEFAULT_SIZE: f64 = 24.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SpinnerFace = #(SpinnerFace::script_api(vm))
    mod.widgets.SpinnerStatus = #(SpinnerStatus::script_api(vm))

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
            if self.face > 1.5 && self.face < 2.5 {
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
            if self.face > 2.5 {
                // A comet: one bright head dragging a tail that fades out
                // behind it. The tail is worked out per pixel rather than
                // stamped as a row of dots, so it stays smooth at any size.
                let d = self.pos * self.rect_size - center
                // PHASE MINUS ANGLE, and not the other way round. The bright
                // end has to be the LEADING one; swap the two and the tail
                // sits in FRONT of the head, and the whole mark reads as a
                // spinner turning backwards.
                let sweep = fract(t - atan2(d.y, d.x) / TAU)
                let tail = pow(1.0 - sweep, 2.2)
                if self.track_alpha > 0.0 {
                    sdf.circle(center.x, center.y, radius)
                    sdf.stroke(vec4(ink.xyz, ink.w * self.track_alpha * spin), stroke * 0.5)
                }
                // Half the width, like the track above: `stroke` is the ink
                // the arc face lays down, and `sdf.stroke` spreads its
                // argument either side of the line.
                sdf.circle(center.x, center.y, radius)
                sdf.stroke(vec4(ink.xyz, ink.w * tail * spin), stroke * 0.5)
                // The head itself, so the leading end is a round cap rather
                // than the flat cut the falloff alone would leave there.
                let head = t * TAU
                sdf.circle(center.x + cos(head) * radius, center.y + sin(head) * radius, stroke * 0.5)
                sdf.fill(vec4(ink.xyz, ink.w * spin))
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

    mod.widgets.DrawLoadingScrimBase = #(DrawLoadingScrim::script_component(vm))
    set_type_default() do #(DrawLoadingScrim::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha: 0.0
        /** the scrim ink */
        color: uniform(theme.color_scrim)
        /** how dark the scrim gets 0..1 step 0.05 */
        dim: uniform(0.45)
        pixel: fn() {
            let a = self.dim * self.alpha
            return vec4(self.color.xyz * a, a)
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
        /** drawn at all; false takes no room either 0..1 step 1 */
        visible: true
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

    /** A bright head leading a tail that fades away round the ring. */
    mod.widgets.SpinnerComet = mod.widgets.SpinnerFlat{
        face: mod.widgets.SpinnerFace.Comet
    }

    /** The arc on a rounded container, for sitting over content. */
    mod.widgets.SpinnerContained = mod.widgets.SpinnerFlat{
        contained: true
        size: 40.0
    }

    mod.widgets.StatusSpinnerBase = #(StatusSpinner::register_widget(vm))
    /** A spinner that ends: Active turns, Finished becomes a tick, Error a
     * cross, and a mark goes quiet again by itself after `auto_reset_secs`. */
    mod.widgets.StatusSpinner = set_type_default() do mod.widgets.StatusSpinnerBase{
        width: Fit
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0.0, y: 0.5}
        /** where it is: inactive, active, finished or error */
        status: mod.widgets.SpinnerStatus.Inactive
        /** side of the mark in points 8..96 step 1 */
        size: 16.0
        /** seconds a tick or cross stays before the mark goes quiet; 0 keeps it 0..10 step 0.5 */
        auto_reset_secs: 2.0
        /** seconds the arc takes to become the mark 0..1 step 0.05 */
        morph_secs: theme.motion_medium_1
        /** the word beside the mark while active */
        text: ""
        /** the word once finished; empty keeps `text` */
        text_finished: ""
        /** the word once failed; empty keeps `text` */
        text_error: ""
        /** drawn at all; false takes no room either 0..1 step 1 */
        visible: true
        /** dimmed 0..1 step 1 */
        disabled: false
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    mod.widgets.SavingIndicatorBase = #(SavingIndicator::register_widget(vm))
    /** "Saving", then "Saved at 14:05", or "Could not save" with a retry
     * button: a status spinner and a label driven by `saving`/`saved`/
     * `failed`, debounced so a fast save never shows the spinner. */
    mod.widgets.SavingIndicator = set_type_default() do mod.widgets.SavingIndicatorBase{
        width: Fit
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0.0, y: 0.5}
        /** how long a save may take before "Saving" appears 0..2 step 0.05 */
        debounce_secs: 0.3
        /** minutes east of UTC for the clock in "Saved at" -840..840 step 15 */
        utc_offset_minutes: 0.0
        /** the word while a save is in flight */
        text_saving: "Saving"
        /** the words before the clock once saved */
        text_saved: "Saved at"
        /** the words after a failure */
        text_failed: "Could not save"
        status := mod.widgets.StatusSpinner{
            size: 14.0
            auto_reset_secs: 0.0
        }
        label := Label{
            text: ""
        }
        retry := View{
            width: Fit
            height: Fit
            visible: false
            retry_button := Button{
                text: "Retry"
            }
        }
    }

    mod.widgets.LoadingOverlayBase = #(LoadingOverlay::register_widget(vm))
    /** Wraps content; while `active` it dims the content, blocks the
     * pointer over it and centres a spinner on it, optionally through a
     * blurred glass pane. `failed` swaps the spinner for a cross so the
     * same overlay can report that the load stopped. */
    mod.widgets.LoadingOverlay = set_type_default() do mod.widgets.LoadingOverlayBase{
        width: Fill
        height: Fill
        /** on: dims and blocks the content 0..1 step 1 */
        active: false
        /** seconds before the overlay appears, so a quick refresh never flashes 0..3 step 0.05 */
        delay_secs: 0.0
        /** seconds the fade in and out take 0..1 step 0.05 */
        fade_secs: theme.motion_short_4
        /** blur the content through a glass pane instead of dimming it 0..1 step 1 */
        blur: false
        /** the word under the spinner */
        text: ""
        /** the load stopped: the arc becomes a cross and stays 0..1 step 1 */
        failed: false
        /** the word once it has failed; empty keeps `text` */
        text_failed: ""
        spinner: mod.widgets.SpinnerFlat{
            size: 28.0
        }
        // The failure face is a whole StatusSpinner rather than a second
        // shader: the arc-to-cross morph, its easing and its ink all already
        // live on that widget, and this is what it was written for.
        mark: mod.widgets.StatusSpinner{
            size: 28.0
            auto_reset_secs: 0.0
        }
        glass: GaussRoundedView{
            width: Fill
            height: Fill
            align: Align{x: 0.5, y: 0.5}
            draw_bg +: {
                corner_radius: 0.0
                border_alpha: 0.0
                shadow_radius: 0.0
                surface_alpha: 0.6
            }
            spinner := mod.widgets.SpinnerFlat{
                size: 28.0
            }
            // A child of the pane for the same reason the spinner is: the
            // glass composites above anything its parent draws after it, so
            // a cross drawn by the overlay would be painted straight out.
            mark := mod.widgets.StatusSpinner{
                visible: false
                size: 28.0
                auto_reset_secs: 0.0
            }
        }
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

/// The overlay's scrim: one instance, its fade.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLoadingScrim {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    alpha: f32,
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

/// How bright the comet face is at `angle_turns` when its head is at
/// `phase_turns`, both counted in whole turns: 1 at the head, falling away
/// round the ring behind it.
///
/// PHASE MINUS ANGLE, and never the other way round. The bright end has to
/// be the LEADING one; swap the two and the tail sits in front of the head
/// and the mark reads as a spinner turning backwards. That is the one
/// hard-won detail about drawing a comet, so it is written down here as
/// well as in the shader, in a form that can be checked without a window.
pub fn comet_tail(phase_turns: f64, angle_turns: f64) -> f64 {
    let sweep = (phase_turns - angle_turns).rem_euclid(1.0);
    (1.0 - sweep).powf(2.2)
}

/// Whether `set_active(active)` is a real flip and should restart the fade.
///
/// It has to say no while the overlay is already where it is being put, or a
/// host calling `set_active(cx, true)` on every draw would restart the delay
/// every frame and the overlay would never appear at all. The price is that
/// a RECYCLED row is told "no change" too: its previous occupant left the
/// overlay up, the new one is loading as well, and nothing in the flip says
/// the row changed hands. That is what [`LoadingOverlay::restart`] is for.
fn flip_restarts(applied: bool, alpha: f64, active: bool) -> bool {
    applied != active || (alpha > 0.0) != active
}

/// The loading overlay's alpha at `now`, and whether it is still moving.
/// `from` is the alpha it flipped at, so a fade interrupted half way runs
/// on from where it was rather than jumping.
///
/// A free function because the glass pane's announcement hangs on the exact
/// step this first goes above zero, and that is worth being able to check
/// without a window.
fn overlay_alpha(
    now: f64,
    since: f64,
    from: f64,
    active: bool,
    delay_secs: f64,
    fade_secs: f64,
) -> (f64, bool) {
    let fade = fade_secs.max(0.0);
    if active {
        let t = now - since - delay_secs.max(0.0);
        if t < 0.0 {
            return (0.0, true);
        }
        let a = if fade <= 0.0 { 1.0 } else { (t / fade).clamp(0.0, 1.0) };
        (a.max(from.min(1.0) * (1.0 - a)), a < 1.0)
    } else {
        let t = now - since;
        let a = if fade <= 0.0 { 0.0 } else { from * (1.0 - (t / fade).clamp(0.0, 1.0)) };
        (a, a > 0.0)
    }
}

/// Whether the blur pane arrives on this step. It is drawn only while the
/// alpha is above zero, so the step the alpha first crosses is the frame it
/// first paints in - and the only frame worth announcing it for.
fn glass_arrives(blur: bool, was: f64, now: f64) -> bool {
    blur && was <= 0.0 && now > 0.0
}

/// HH:MM of the wall clock, shifted east of UTC by `offset_minutes`. The
/// platform has no local-time query, so the offset is the host's to give.
fn clock_hhmm(offset_minutes: f64) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + (offset_minutes as i64) * 60;
    let day = secs.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, (day % 3600) / 60)
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
    /// Drawn at all. A hidden spinner takes no room either, so a host can
    /// swap it for another mark in the same slot without the layout moving.
    #[live(true)]
    #[visible]
    pub visible: bool,
    #[live]
    pub disabled: bool,
    /// When it first drew, for the delay. `None` until then and again
    /// after `restart`.
    #[rust]
    shown_since: Option<f64>,
    /// An extra multiplier a host (the overlay) fades with.
    #[rust(1.0)]
    opacity: f64,
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

    fn set_opacity(&mut self, opacity: f64) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// The mark's alpha this frame, and whether the delay and fade are
    /// over (after which the pass clock alone keeps the mark turning).
    fn alpha_now(&mut self, now: f64) -> (f64, bool) {
        let since = *self.shown_since.get_or_insert(now);
        let raw = delayed_alpha(now, since, self.delay_secs, self.fade_secs);
        let mut alpha = raw * self.opacity;
        if self.disabled {
            alpha *= 0.5;
        }
        (alpha, raw >= 1.0)
    }
}

impl Widget for Spinner {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
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

/// The eased journey of the status spinner's `morph` (0 turning, 1 mark).
#[derive(Default)]
struct Morph {
    from: f64,
    to: f64,
    started: f64,
    running: bool,
}

impl Morph {
    fn at(&mut self, now: f64, secs: f64) -> f64 {
        if !self.running {
            return self.to;
        }
        let t = if secs <= 0.0 {
            1.0
        } else {
            ((now - self.started) / secs).clamp(0.0, 1.0)
        };
        if t >= 1.0 {
            self.running = false;
            return self.to;
        }
        let e = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
        self.from + (self.to - self.from) * e
    }
}

/// A spinner with an ending: the arc becomes a tick or a cross and then
/// goes quiet by itself.
#[derive(Script, ScriptHook, Widget)]
pub struct StatusSpinner {
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
    #[live]
    pub text_finished: String,
    #[live]
    pub text_error: String,
    #[live(SpinnerStatus::Inactive)]
    pub status: SpinnerStatus,
    #[live]
    pub size: f64,
    #[live]
    pub auto_reset_secs: f64,
    #[live]
    pub morph_secs: f64,
    /// Drawn at all. A hidden mark takes no room either.
    #[live(true)]
    #[visible]
    pub visible: bool,
    #[live]
    pub disabled: bool,
    /// An extra multiplier a host (the loading overlay) fades with.
    #[rust(1.0)]
    opacity: f64,
    /// The status the transitions were last run for; a script apply that
    /// changes `status` behind the setter's back is caught at draw time.
    #[rust]
    applied: SpinnerStatus,
    #[rust]
    morph: Morph,
    #[rust]
    shown_morph: f64,
    /// Whether Finished/Failed was raised for the current mark.
    #[rust]
    announced: bool,
    #[rust]
    reset_timer: Timer,
    #[rust]
    next_frame: NextFrame,
}

impl StatusSpinner {
    fn mark_size(&self) -> f64 {
        if self.size > 0.0 {
            self.size
        } else {
            16.0
        }
    }

    /// The word for the current status: the finished or error word when
    /// one is given, else the active word.
    pub fn label_text(&self) -> String {
        match self.status {
            SpinnerStatus::Finished if !self.text_finished.is_empty() => self.text_finished.clone(),
            SpinnerStatus::Error if !self.text_error.is_empty() => self.text_error.clone(),
            SpinnerStatus::Inactive => String::new(),
            _ => self.text.clone(),
        }
    }

    pub fn status(&self) -> SpinnerStatus {
        self.status
    }

    fn set_opacity(&mut self, opacity: f64) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// Move to `status`. Finished and Error morph the arc into their mark
    /// and, with `auto_reset_secs` above zero, arm the timer that takes it
    /// away again.
    pub fn set_status(&mut self, cx: &mut Cx, status: SpinnerStatus) {
        self.status = status;
        self.applied = status;
        cx.stop_timer(self.reset_timer);
        self.reset_timer = Timer::empty();
        let now = cx.seconds_since_app_start();
        let target = match status {
            SpinnerStatus::Finished | SpinnerStatus::Error => 1.0,
            _ => 0.0,
        };
        if status == SpinnerStatus::Inactive {
            self.shown_morph = 0.0;
            self.morph = Morph::default();
        } else if (target - self.shown_morph).abs() > f64::EPSILON {
            self.morph = Morph {
                from: self.shown_morph,
                to: target,
                started: now,
                running: true,
            };
            self.next_frame = cx.new_next_frame();
        }
        self.announced = false;
        if target > 0.0 && self.auto_reset_secs > 0.0 {
            self.reset_timer = cx.start_timeout(self.auto_reset_secs);
        }
        self.draw_bg.redraw(cx);
    }

    /// Raise Finished/Failed once the mark is fully on screen.
    fn announce(&mut self, cx: &mut Cx) {
        if self.announced || self.shown_morph < 0.999 {
            return;
        }
        self.announced = true;
        let uid = self.widget_uid();
        match self.status {
            SpinnerStatus::Finished => cx.widget_action(uid, StatusSpinnerAction::Finished),
            SpinnerStatus::Error => cx.widget_action(uid, StatusSpinnerAction::Failed),
            _ => {}
        }
    }
}

impl Widget for StatusSpinner {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        if self.status != self.applied {
            let status = self.status;
            self.set_status(cx, status);
        }
        let s = self.mark_size();
        cx.begin_turtle(walk, self.layout);
        let (face_status, alpha) = match self.status {
            SpinnerStatus::Inactive => (FACE_STATUS_SPINNING, 0.0),
            SpinnerStatus::Active => (FACE_STATUS_SPINNING, 1.0),
            SpinnerStatus::Finished => (FACE_STATUS_FINISHED, 1.0),
            SpinnerStatus::Error => (FACE_STATUS_ERROR, 1.0),
        };
        let alpha = alpha * self.opacity as f32;
        self.draw_bg.face = SpinnerFace::Arc.index();
        self.draw_bg.alpha = if self.disabled { alpha * 0.5 } else { alpha };
        self.draw_bg.status = face_status;
        self.draw_bg.morph = self.shown_morph as f32;
        self.draw_bg.contained = 0.0;
        self.draw_bg.draw_walk(cx, Walk::fixed(s, s));
        let label = self.label_text();
        if !label.is_empty() && alpha > 0.0 {
            self.draw_text
                .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &label);
        }
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(ne) = self.next_frame.is_event(event) {
            self.shown_morph = self.morph.at(ne.time, self.morph_secs);
            if self.morph.running {
                self.next_frame = cx.new_next_frame();
            }
            self.announce(cx);
            self.draw_bg.redraw(cx);
        }
        if self.reset_timer.is_event(event).is_some() {
            self.reset_timer = Timer::empty();
            self.set_status(cx, SpinnerStatus::Inactive);
            let uid = self.widget_uid();
            cx.widget_action(uid, StatusSpinnerAction::Reset);
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
        Some(self.status.name().to_string())
    }
}

impl StatusSpinnerRef {
    pub fn finished(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast()),
            Some(StatusSpinnerAction::Finished)
        )
    }

    pub fn failed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast()),
            Some(StatusSpinnerAction::Failed)
        )
    }

    pub fn was_reset(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast()),
            Some(StatusSpinnerAction::Reset)
        )
    }

    pub fn status(&self) -> SpinnerStatus {
        self.borrow().map(|inner| inner.status()).unwrap_or_default()
    }

    pub fn set_status(&self, cx: &mut Cx, status: SpinnerStatus) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_status(cx, status);
        }
    }

    pub fn start(&self, cx: &mut Cx) {
        self.set_status(cx, SpinnerStatus::Active);
    }

    pub fn finish(&self, cx: &mut Cx) {
        self.set_status(cx, SpinnerStatus::Finished);
    }

    pub fn fail(&self, cx: &mut Cx) {
        self.set_status(cx, SpinnerStatus::Error);
    }

    pub fn reset(&self, cx: &mut Cx) {
        self.set_status(cx, SpinnerStatus::Inactive);
    }
}

/// "Saving" / "Saved at HH:MM" / "Could not save" + Retry, debounced.
#[derive(Script, ScriptHook, Widget)]
pub struct SavingIndicator {
    #[deref]
    view: View,
    #[live]
    pub debounce_secs: f64,
    #[live]
    pub utc_offset_minutes: f64,
    #[live]
    pub text_saving: String,
    #[live]
    pub text_saved: String,
    #[live]
    pub text_failed: String,
    #[rust]
    state: SavingState,
    #[rust]
    debounce: Timer,
    #[rust]
    label_text: String,
}

impl SavingIndicator {
    pub fn state(&self) -> SavingState {
        self.state
    }

    /// A save began. "Saving" appears only once `debounce_secs` have
    /// passed with the save still in flight.
    pub fn saving(&mut self, cx: &mut Cx) {
        if self.state == SavingState::Saving {
            return;
        }
        self.state = SavingState::Saving;
        self.stop_debounce(cx);
        self.show_retry(cx, false);
        if self.debounce_secs > 0.0 {
            self.debounce = cx.start_timeout(self.debounce_secs);
        } else {
            self.show_saving(cx);
        }
    }

    /// The save landed: "Saved at HH:MM", with a tick.
    pub fn saved(&mut self, cx: &mut Cx) {
        self.stop_debounce(cx);
        self.state = SavingState::Saved;
        let text = format!("{} {}", self.text_saved, clock_hhmm(self.utc_offset_minutes));
        self.set_label(cx, &text);
        self.status_spinner(cx, ids!(status)).set_status(cx, SpinnerStatus::Finished);
        self.show_retry(cx, false);
        self.redraw(cx);
    }

    /// The save failed: the failure words, a cross, and the retry button.
    pub fn failed(&mut self, cx: &mut Cx) {
        self.stop_debounce(cx);
        self.state = SavingState::Failed;
        let text = self.text_failed.clone();
        self.set_label(cx, &text);
        self.status_spinner(cx, ids!(status)).set_status(cx, SpinnerStatus::Error);
        self.show_retry(cx, true);
        self.redraw(cx);
    }

    /// Back to nothing.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.stop_debounce(cx);
        self.state = SavingState::Idle;
        self.set_label(cx, "");
        self.status_spinner(cx, ids!(status)).set_status(cx, SpinnerStatus::Inactive);
        self.show_retry(cx, false);
        self.redraw(cx);
    }

    fn show_saving(&mut self, cx: &mut Cx) {
        let text = self.text_saving.clone();
        self.set_label(cx, &text);
        self.status_spinner(cx, ids!(status)).set_status(cx, SpinnerStatus::Active);
        self.redraw(cx);
    }

    fn stop_debounce(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.debounce);
        self.debounce = Timer::empty();
    }

    fn set_label(&mut self, cx: &mut Cx, text: &str) {
        self.label_text = text.to_string();
        self.label(cx, ids!(label)).set_text(cx, text);
    }

    fn show_retry(&mut self, cx: &mut Cx, on: bool) {
        self.view.widget(cx, ids!(retry)).set_visible(cx, on);
    }
}

impl Widget for SavingIndicator {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if self.debounce.is_event(event).is_some() {
            self.debounce = Timer::empty();
            if self.state == SavingState::Saving {
                self.show_saving(cx);
            }
        }
        if let Event::Actions(actions) = event {
            if self.button(cx, ids!(retry_button)).clicked(actions) {
                let uid = self.widget_uid();
                cx.widget_action(uid, SavingIndicatorAction::Retry);
            }
        }
    }

    fn text(&self) -> String {
        self.label_text.clone()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(
            match self.state {
                SavingState::Idle => "Idle",
                SavingState::Saving => "Saving",
                SavingState::Saved => "Saved",
                SavingState::Failed => "Failed",
            }
            .to_string(),
        )
    }
}

impl SavingIndicatorRef {
    pub fn retry(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast()),
            Some(SavingIndicatorAction::Retry)
        )
    }

    pub fn state(&self) -> SavingState {
        self.borrow().map(|inner| inner.state()).unwrap_or_default()
    }

    pub fn saving(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.saving(cx);
        }
    }

    pub fn saved(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.saved(cx);
        }
    }

    pub fn failed(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.failed(cx);
        }
    }

    pub fn reset(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset(cx);
        }
    }
}

/// Dims and blocks its content while `active`, with a spinner on top.
#[derive(Script, ScriptHook, Widget)]
pub struct LoadingOverlay {
    #[deref]
    view: View,
    #[live]
    draw_scrim: DrawLoadingScrim,
    #[live]
    spinner: Spinner,
    /// The face it wears once the load has failed: a status spinner, whose
    /// arc-to-cross morph is the thing being borrowed.
    #[live]
    mark: StatusSpinner,
    #[live]
    glass: GaussRoundedView,
    #[live]
    pub text: String,
    #[live]
    pub text_failed: String,
    #[live]
    pub active: bool,
    #[live]
    pub failed: bool,
    #[live]
    pub delay_secs: f64,
    #[live]
    pub fade_secs: f64,
    #[live]
    pub blur: bool,
    /// The `active` the fade was last started for.
    #[rust]
    applied_active: bool,
    /// The `failed` the faces were last swapped for; a script apply that
    /// writes the field behind the setter's back is caught at draw time.
    #[rust]
    applied_failed: bool,
    /// The scrim's alpha on screen.
    #[rust]
    alpha: f64,
    /// When `active` last flipped, and the alpha it flipped from.
    #[rust]
    since: f64,
    #[rust]
    alpha_at_flip: f64,
    #[rust]
    next_frame: NextFrame,
}

impl LoadingOverlay {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn has_failed(&self) -> bool {
        self.failed
    }

    /// Turn the overlay on or off. On: the scrim waits out `delay_secs`
    /// and fades in. Off: it fades out from wherever it was, and a failure
    /// it was reporting goes with it — a failure belongs to the load that
    /// failed, and the next one starts clean.
    pub fn set_active(&mut self, cx: &mut Cx, active: bool) {
        self.active = active;
        if !active && self.failed {
            self.set_failed(cx, false);
        }
        if !flip_restarts(self.applied_active, self.alpha, active) {
            return;
        }
        self.applied_active = active;
        self.since = cx.seconds_since_app_start();
        self.alpha_at_flip = self.alpha;
        if active {
            self.spinner.restart(cx);
        }
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }

    /// The scrim's alpha at `now`; whether it is still moving.
    fn alpha_at(&self, now: f64) -> (f64, bool) {
        overlay_alpha(
            now,
            self.since,
            self.alpha_at_flip,
            self.active,
            self.delay_secs,
            self.fade_secs,
        )
    }

    /// Say the load STOPPED: the arc closes into a ring and a cross fades
    /// into it, and there it stays.
    ///
    /// This is the state a busy overlay is usually missing, and the reason
    /// to want it is that without one a host has to tear the overlay down
    /// and put something else in the hole, while a scrim that stays up with
    /// an arc still turning on it says the machine is working when it has
    /// stopped. A stopped mark must never be mistaken for a slow one.
    pub fn fail(&mut self, cx: &mut Cx) {
        self.set_failed(cx, true);
    }

    /// Whether the overlay is reporting a failure. Turning it on also turns
    /// the overlay on, because there is nothing to report it against
    /// otherwise.
    pub fn set_failed(&mut self, cx: &mut Cx, failed: bool) {
        if self.applied_failed == failed {
            self.failed = failed;
            return;
        }
        self.applied_failed = failed;
        self.failed = failed;
        if failed {
            self.set_active(cx, true);
        }
        let status = if failed {
            SpinnerStatus::Error
        } else {
            SpinnerStatus::Inactive
        };
        self.mark.set_status(cx, status);
        // The blurred pane keeps its own pair, because anything the overlay
        // draws after the glass is composited out by it.
        self.glass.widget(cx, ids!(spinner)).set_visible(cx, !failed);
        let glass_mark = self.glass.widget(cx, ids!(mark));
        glass_mark.set_visible(cx, failed);
        glass_mark.as_status_spinner().set_status(cx, status);
        self.push_text(cx);
        self.redraw(cx);
    }

    /// Arm the overlay again as if it had never been up: the fade starts
    /// from nothing, the delay runs in full and a failure is cleared.
    ///
    /// A recycled list row is what this is for. Such a row keeps its widgets
    /// and only swaps what they show, so the overlay the PREVIOUS row left
    /// up is the overlay the new row starts with — and `set_active(cx,
    /// true)` on a row that is already active is not a flip, so the new row
    /// silently inherits the old one's finished fade and its own delay never
    /// runs. Call this when a row is re-seated.
    pub fn restart(&mut self, cx: &mut Cx) {
        self.alpha = 0.0;
        self.alpha_at_flip = 0.0;
        self.applied_active = !self.active;
        self.set_failed(cx, false);
        self.spinner.restart(cx);
        let active = self.active;
        self.set_active(cx, active);
    }

    /// The word under the mark: the failure word once it has failed, and
    /// `text` whenever that is empty, so a host that only wants one word
    /// writes one.
    fn word(&self) -> String {
        if self.failed && !self.text_failed.is_empty() {
            self.text_failed.clone()
        } else {
            self.text.clone()
        }
    }

    fn push_text(&mut self, cx: &mut Cx) {
        let text = self.word();
        self.spinner.set_text(cx, &text);
        self.mark.set_text(cx, &text);
        self.glass.widget(cx, ids!(spinner)).set_text(cx, &text);
        self.glass.widget(cx, ids!(mark)).set_text(cx, &text);
    }
}

impl Widget for LoadingOverlay {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.active != self.applied_active {
            // A script apply flipped `active` behind the setter's back.
            let active = self.active;
            self.set_active(cx, active);
        }
        if self.failed != self.applied_failed {
            let failed = self.failed;
            self.set_failed(cx, failed);
        }
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_step() {
            return step;
        }
        if self.alpha <= 0.0 {
            return DrawStep::done();
        }
        let rect = self.view.area().rect(cx);
        self.push_text(cx);
        self.draw_scrim.alpha = if self.blur { 0.0 } else { self.alpha as f32 };
        self.draw_scrim.draw_abs(cx, rect);
        if self.blur {
            self.glass.draw_walk_all(
                cx,
                scope,
                Walk::fixed(rect.size.x, rect.size.y).with_abs_pos(rect.pos),
            );
        } else {
            self.spinner.set_opacity(self.alpha);
            self.mark.set_opacity(self.alpha);
            cx.begin_turtle(
                Walk::fixed(rect.size.x, rect.size.y).with_abs_pos(rect.pos),
                Layout {
                    align: Align { x: 0.5, y: 0.5 },
                    ..Layout::flow_down()
                },
            );
            if self.failed {
                let _ = self.mark.draw_walk(cx, scope, Walk::fit());
            } else {
                let _ = self.spinner.draw_walk(cx, scope, Walk::fit());
            }
            cx.end_turtle();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(ne) = self.next_frame.is_event(event) {
            let was = self.alpha;
            let (alpha, moving) = self.alpha_at(ne.time);
            self.alpha = alpha;
            if glass_arrives(self.blur, was, alpha) {
                // The pane is about to paint for the first time, and this is
                // the last moment at which saying so still counts: the
                // window decides whether to capture the scene behind the
                // glass before any widget draws, and next-frame events are
                // delivered before that decision is taken.
                //
                // Saying it in `set_active` does not work. The arm is taken
                // on the very next frame whether or not any glass drew, and
                // `draw_walk` returns before it reaches the pane while the
                // alpha is still zero - so with any delay set at all, the
                // arm is spent tens of frames before the pane appears and
                // the pop is exactly as it was.
                arm_gauss_capture(cx);
            }
            if moving {
                self.next_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }
        if self.active || self.alpha > 0.0 {
            // Claim every pointer event over the content first: this
            // widget is asked before its children, and a hit taken here is
            // marked handled for everyone asked after it.
            let _ = event.hits(cx, self.draw_scrim.area());
            if self.blur {
                self.glass.handle_event(cx, event, scope);
            }
        }
        // The spinner pumps its own fade frames and the mark its own morph
        // frames; both need their events.
        self.spinner.handle_event(cx, event, scope);
        self.mark.handle_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }

    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.push_text(cx);
            self.redraw(cx);
        }
    }

    /// "active", "failed" or "idle" plus the scrim's alpha, so a test can
    /// wait for the overlay to be up, to have given up, or to be gone,
    /// rather than for a frame count.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        let state = if self.failed {
            "failed"
        } else if self.active {
            "active"
        } else {
            "idle"
        };
        Some(format!("{} {:.2}", state, self.alpha))
    }
}

impl LoadingOverlayRef {
    pub fn is_active(&self) -> bool {
        self.borrow().map(|inner| inner.is_active()).unwrap_or(false)
    }

    pub fn set_active(&self, cx: &mut Cx, active: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active(cx, active);
        }
    }

    pub fn has_failed(&self) -> bool {
        self.borrow().map(|inner| inner.has_failed()).unwrap_or(false)
    }

    /// The load stopped; say so without taking the overlay down.
    pub fn fail(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.fail(cx);
        }
    }

    pub fn set_failed(&self, cx: &mut Cx, failed: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_failed(cx, failed);
        }
    }

    /// Call when a recycled row is re-seated; see
    /// [`LoadingOverlay::restart`].
    pub fn restart(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.restart(cx);
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

    #[test]
    fn morph_eases_and_settles() {
        let mut m = Morph { from: 0.0, to: 1.0, started: 5.0, running: true };
        let mid = m.at(5.1, 0.25);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert_eq!(m.at(5.3, 0.25), 1.0);
        assert!(!m.running);
    }

    /// Turned on at ten seconds with half a second of delay and a fifth of
    /// a second of fade. The blur pane paints nothing until 10.5, so the
    /// frame to announce it on is the first one past that - and there is
    /// exactly one such frame, not one per frame of the fade.
    #[test]
    fn the_blur_pane_is_announced_on_the_frame_it_first_paints_in() {
        let mut was = 0.0;
        let mut announced = Vec::new();
        for step in 0..90 {
            let t = 10.0 + step as f64 / 60.0;
            let (now, _) = overlay_alpha(t, 10.0, 0.0, true, 0.5, 0.2);
            if glass_arrives(true, was, now) {
                announced.push(t);
            }
            was = now;
        }
        assert_eq!(announced.len(), 1, "announced once, not on every frame of the fade");
        assert!(
            announced[0] >= 10.5,
            "and not at {:.3}, while the delay was still being waited out",
            announced[0]
        );
        assert!(!glass_arrives(false, 0.0, 1.0), "a plain scrim has no glass to announce");
    }

    /// The `script_mod!` block is invisible to the Rust compiler: a mistake
    /// in it shows up only in a running app's log, and a shader that fails
    /// to compile is not an error anywhere — the draw is simply skipped and
    /// the widget paints nothing. Building one of each out of its type
    /// default and reading the shader-error slot back is what turns either
    /// into a failed build. The comet is a new arm of the spinner shader
    /// and the failure face a new slot on the overlay, so both are here.
    #[test]
    fn the_spinners_and_the_overlay_come_out_of_the_dsl_and_compile() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (spinner, overlay) = cx.with_vm(|vm| {
            crate::script_mod(vm);
            // Registering type defaults compiles nothing; making an instance
            // out of one does. Clearing here keeps any other module's
            // complaint out of this test's answer.
            let _ = crate::makepad_draw::makepad_platform::shader_error::take();
            (
                Spinner::script_new_with_default(vm),
                LoadingOverlay::script_new_with_default(vm),
            )
        });
        assert_eq!(
            crate::makepad_draw::makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        // Values only the DSL sets, so the block was really evaluated.
        assert_eq!(spinner.size, 24.0);
        assert_eq!(spinner.face, SpinnerFace::Arc);
        // Everything new defaults to what these did before they had it.
        assert!(spinner.visible, "a spinner still draws unless it is told not to");
        assert!(!overlay.active);
        assert!(!overlay.failed, "an overlay does not start out having given up");
        assert!(overlay.text_failed.is_empty(), "and falls back to `text`");
        // The failure face came through its slot, and is quiet.
        assert_eq!(overlay.mark.size, 28.0);
        assert_eq!(overlay.mark.auto_reset_secs, 0.0, "a failure does not time out");
        assert_eq!(overlay.mark.status, SpinnerStatus::Inactive);
    }

    /// The head is the BRIGHT end and it leads. A point just behind it is
    /// nearly as bright; the same distance in front of it is nearly dark.
    /// Get these two the wrong way round and the mark turns backwards.
    #[test]
    fn the_comets_head_leads_its_tail() {
        let head = 0.25;
        assert!((comet_tail(head, head) - 1.0).abs() < 1e-9, "brightest at the head");
        let behind = comet_tail(head, head - 0.05);
        let ahead = comet_tail(head, head + 0.05);
        assert!(behind > ahead, "behind {behind}, ahead {ahead}");
        assert!(behind > 0.8, "just behind the head is still bright: {behind}");
        assert!(ahead < 0.05, "just in front of it is dark: {ahead}");
        // All the way round is the head again, with no seam.
        assert!((comet_tail(head, head - 1.0) - 1.0).abs() < 1e-9);
        // And it never leaves 0..1, whatever turn the phase is on.
        for step in 0..64 {
            let a = step as f64 / 64.0;
            let v = comet_tail(7.5, a);
            assert!((0.0..=1.0).contains(&v), "{a} -> {v}");
        }
    }

    /// The trap `LoadingOverlay::restart` exists for: a recycled row whose
    /// previous occupant left the overlay up is told, correctly by this
    /// rule and wrongly for the row, that nothing changed.
    #[test]
    fn a_repeated_set_active_does_not_restart_the_fade() {
        assert!(flip_restarts(false, 0.0, true), "off to on is a flip");
        assert!(flip_restarts(true, 1.0, false), "on to off is a flip");
        assert!(!flip_restarts(true, 1.0, true), "on while already on is not");
        assert!(!flip_restarts(false, 0.0, false), "off while already off is not");
        // Mid fade-out, and asked for on again: the alpha has not reached
        // zero, so the flip is taken and the fade runs on from where it is.
        assert!(flip_restarts(false, 0.4, true));
    }

    #[test]
    fn clock_is_hh_mm_and_wraps_the_day() {
        let s = clock_hhmm(0.0);
        assert_eq!(s.len(), 5);
        assert_eq!(&s[2..3], ":");
        let h: u32 = s[..2].parse().unwrap();
        let m: u32 = s[3..].parse().unwrap();
        assert!(h < 24 && m < 60);
        assert_eq!(clock_hhmm(-840.0).len(), 5);
    }
}

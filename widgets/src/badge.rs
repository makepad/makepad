//! Badge, StatusDot and LabelValue — the small marks that say ONE fact
//! about the thing they sit next to: how many, what state, which value.
//!
//! A count in a corner, a coloured dot beside a name, a `cpu: 84%` pill in
//! a toolbar — every app draws these, and every app draws them slightly
//! differently, so they read as three different things when they are one.
//! This module gives them one vocabulary: an INTENT (what the mark means:
//! error, warning, success, info, or the brand's primary), an APPEARANCE
//! (filled, ghost, outline, tint), a SIZE ladder and a SHAPE, all resolved
//! from the theme's role tokens so a badge on a light theme and a badge on a
//! dark one are the same badge.
//!
//! A `Badge` with nothing to say collapses to a DOT: the smallest honest
//! mark. With a count it shows the number, capped at `max` as "99+", because
//! a badge that reads "1204" has stopped being a badge and become a
//! statistic. With a `text` it shows the word.
//!
//! A `StatusDot` pairs every status with a SHAPE as well as a colour —
//! success is a circle, warning a triangle, error a square, info a circle
//! with an i, in-progress a ring that breathes, unknown a dashed ring,
//! pending a hollow one — so an operator who cannot tell the red from the
//! green still tells the states apart. The breathing ring is the only mark
//! that reads `draw_pass.time`, and it lives in its own shader so a screen
//! full of static dots never pins the window at display rate.
//!
//! A `LabelValue` is a two-tone pill, the label in the quiet half and the
//! value in a half whose colour follows the value's MAGNITUDE between `min`
//! and `max`, so a row of them can be scanned for the one that is running
//! hot without reading a single digit.
//!
//! A `BadgeAnchor` wraps any widget and draws its badge over a corner of it
//! on the OVERLAY draw list (the `TipLayer` idiom), shifted from the host's
//! final rect, so the host keeps exactly the size it had and the badge
//! floats over whatever neighbours crowd the corner. It hides while the
//! badge has nothing to say, because an "unread" mark that is always there
//! is a mark nobody reads.
//!
//! A `Marker` is the same anchor placed by RELATIVE coordinates, 0..1 across
//! and down its content, numbered, answering a click with
//! [`MarkerAction::Clicked`]; laid over a picture with `flow: Overlay` it
//! turns the picture into a legend. The pin is hit-tested before the content
//! under it is handed the event, so a pin over a button is a pin, not a
//! button.

use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

/// What a mark means. The colours come from the theme's role tokens, so
/// the meaning survives a theme change.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum BadgeIntent {
    #[pick]
    Neutral = 0,
    Primary = 1,
    Secondary = 2,
    Tertiary = 3,
    Error = 4,
    Warning = 5,
    Success = 6,
    Info = 7,
}

/// How loudly a mark is drawn: a solid pill, bare ink, a stroked outline, or
/// a tinted container with the intent's darker ink on it.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum BadgeAppearance {
    #[pick]
    Filled = 0,
    Ghost = 1,
    Outline = 2,
    Tint = 3,
}

/// The size ladder: pill height, dot diameter, padding and font all step
/// together, so a badge is never a big pill with tiny digits.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum BadgeSize {
    Tiny = 0,
    Small = 1,
    #[pick]
    Medium = 2,
    Large = 3,
    Xl = 4,
}

/// Round is a pill (a circle for one digit), Rounded a soft rectangle,
/// Square a hard one.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum BadgeShape {
    #[pick]
    Round = 0,
    Rounded = 1,
    Square = 2,
}

/// A status, each with its own SHAPE so the state is never colour-only.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum StatusKind {
    #[pick]
    Unknown = 0,
    Success = 1,
    Warning = 2,
    Error = 3,
    Info = 4,
    InProgress = 5,
    Pending = 6,
}

impl StatusKind {
    /// The name a test waits on, and the name `set_text` accepts.
    pub fn name(self) -> &'static str {
        match self {
            StatusKind::Unknown => "unknown",
            StatusKind::Success => "success",
            StatusKind::Warning => "warning",
            StatusKind::Error => "error",
            StatusKind::Info => "info",
            StatusKind::InProgress => "in-progress",
            StatusKind::Pending => "pending",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        [
            StatusKind::Unknown,
            StatusKind::Success,
            StatusKind::Warning,
            StatusKind::Error,
            StatusKind::Info,
            StatusKind::InProgress,
            StatusKind::Pending,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

/// Which corner of the wrapped widget an anchored badge sits on.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum BadgeCorner {
    #[pick]
    TopRight = 0,
    TopLeft = 1,
    BottomRight = 2,
    BottomLeft = 3,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum MarkerAction {
    /// The pin was pressed and released.
    Clicked,
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // The enums a call site writes bare (`intent: Error`, `size: Small`) are
    // splatted into mod.widgets BEFORE the `use` below, because a block's
    // `use` only sees what exists when it runs.
    mod.widgets.BadgeIntent = set_type_default() do #(BadgeIntent::script_api(vm))
    mod.widgets.splat(mod.widgets.BadgeIntent)
    mod.widgets.BadgeAppearance = set_type_default() do #(BadgeAppearance::script_api(vm))
    mod.widgets.splat(mod.widgets.BadgeAppearance)
    mod.widgets.BadgeSize = set_type_default() do #(BadgeSize::script_api(vm))
    mod.widgets.splat(mod.widgets.BadgeSize)
    mod.widgets.BadgeShape = set_type_default() do #(BadgeShape::script_api(vm))
    mod.widgets.splat(mod.widgets.BadgeShape)
    // StatusKind shares variant names with BadgeIntent, so it stays
    // qualified: `status: StatusKind.Warning`.
    let StatusKind = set_type_default() do #(StatusKind::script_api(vm))
    mod.widgets.StatusKind = StatusKind
    let BadgeCorner = set_type_default() do #(BadgeCorner::script_api(vm))
    mod.widgets.BadgeCorner = BadgeCorner
    /** The colour of every role: base, ink on the base, container, ink on
     * the container. Every small mark that speaks a role reads these, so a
     * chip and a badge that both mean "error" are the same red. */
    mod.widgets.BadgePalette = set_type_default() do #(BadgePalette::script_api(vm)){
        neutral: theme.color_opaque_u_4
        on_neutral: theme.color_opaque_d_5
        neutral_container: theme.color_opaque_u_1
        on_neutral_container: theme.color_text
        primary: theme.color_primary
        on_primary: theme.color_on_primary
        primary_container: theme.color_primary_container
        on_primary_container: theme.color_on_primary_container
        secondary: theme.color_secondary
        on_secondary: theme.color_on_secondary
        secondary_container: theme.color_secondary_container
        on_secondary_container: theme.color_on_secondary_container
        tertiary: theme.color_tertiary
        on_tertiary: theme.color_on_tertiary
        tertiary_container: theme.color_tertiary_container
        on_tertiary_container: theme.color_on_tertiary_container
        error: theme.color_error
        on_error: theme.color_on_error
        error_container: theme.color_error_container
        on_error_container: theme.color_on_error_container
        warning: theme.color_warning
        on_warning: theme.color_on_warning
        warning_container: theme.color_warning_container
        on_warning_container: theme.color_on_warning_container
        success: theme.color_success
        on_success: theme.color_on_success
        success_container: theme.color_success_container
        on_success_container: theme.color_on_success_container
        info: theme.color_info
        on_info: theme.color_on_info
        info_container: theme.color_info_container
        on_info_container: theme.color_on_info_container
    }

    use mod.widgets.*

    mod.widgets.DrawBadgeBase = #(DrawBadge::script_component(vm))
    set_type_default() do #(DrawBadge::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawStatusDotBase = #(DrawStatusDot::script_component(vm))
    set_type_default() do #(DrawStatusDot::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawStatusPulseBase = #(DrawStatusPulse::script_component(vm))
    set_type_default() do #(DrawStatusPulse::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawLabelValueBase = #(DrawLabelValue::script_component(vm))
    set_type_default() do #(DrawLabelValue::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.BadgeBase = #(Badge::register_widget(vm))
    /** A count, a word or a dot in a small pill, coloured by intent. */
    mod.widgets.BadgeFlat = set_type_default() do mod.widgets.BadgeBase{
        width: Fit
        height: Fit
        /** a word shown instead of the count */
        text: ""
        /** the number shown; 0 collapses to a dot unless show_zero 0..999 step 1 */
        count: 0
        /** counts above this render as "max+" 1..999 step 1 */
        max: 99
        /** draw a 0 count as "0" instead of a dot */
        show_zero: false
        /** the dot form whatever the count and text say */
        dot: false
        /** what the mark means: Neutral Primary Secondary Tertiary Error Warning Success Info */
        intent: Neutral
        /** Filled Ghost Outline Tint */
        appearance: Filled
        /** Tiny Small Medium Large Xl */
        size: Medium
        /** Round Rounded Square */
        shape: Round
        /** corner radius of the Rounded shape 0..12 step 0.5 */
        radius: theme.radius_s
        /** ink and fill alpha while disabled 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        /** The colour of every intent: base, ink on the base, container,
         * ink on the container. Filled uses the first pair, Tint the second,
         * Ghost and Outline draw the base as ink. */
        /** The colour of every intent, inherited from the shared palette
         * so a badge and a chip that mean the same thing look the same. */
        palette: mod.widgets.BadgePalette{}
        // The fill, the stroke, its width and the corner radius are the
        // INSTANCES, set by the widget every draw from intent, appearance
        // and shape; they ride in the struct, so every other prop here is a
        // uniform or the instance slots stop lining up.
        draw_bg +: {
            /** bevel strength: 0 flat, 1 the full two-stop stroke 0..1 step 0.05 */
            bevel: uniform(0.0)
            /** bevel stroke, top stop */
            color_bevel_1: uniform(theme.color_bevel_outset_1)
            /** bevel stroke, bottom stop */
            color_bevel_2: uniform(theme.color_bevel_outset_2)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // `sdf.box` draws twice the radius it is given, so the
                // visual radius the widget worked out is halved here.
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius * 0.5)
                sdf.fill_keep(self.color)
                if self.border_size > 0.0 {
                    sdf.stroke_keep(self.border_color, self.border_size)
                }
                // The bevel follows the fill's alpha: a ghost or an outline
                // badge has no face to bevel.
                if self.bevel > 0.0 {
                    sdf.stroke_keep(mix(self.color_bevel_1, self.color_bevel_2, self.pos.y) * self.bevel * self.color.a, 1.0)
                }
                return sdf.result
            }
        }
        draw_text +: {
            text_style: theme.font_bold{font_size: 9}
            color: theme.color_text
        }
    }

    /** The flat badge with the theme's bevel stroke. */
    mod.widgets.Badge = mod.widgets.BadgeFlat{
        draw_bg +: {
            bevel: 1.0
        }
    }

    mod.widgets.BadgeAnchorBase = #(BadgeAnchor::register_widget(vm))
    /** Wraps one widget and draws a badge over its corner on the overlay,
     * so the wrapped widget keeps its size. Hidden while the badge has
     * nothing to say: no count, no word, no dot asked for. */
    mod.widgets.BadgeAnchor = set_type_default() do mod.widgets.BadgeAnchorBase{
        width: Fit
        height: Fit
        /** BadgeCorner.TopRight TopLeft BottomRight BottomLeft */
        corner: BadgeCorner.TopRight
        /** nudge along x, positive moves right -20..20 step 0.5 */
        offset_x: 0.0
        /** nudge along y, positive moves down -20..20 step 0.5 */
        offset_y: 0.0
        /** how much of the badge hangs outside the corner 0..1 step 0.05 */
        overhang: 0.5
        /** the badge itself; `badge +: {count: 3 intent: Error}` at a call site */
        badge: mod.widgets.Badge{intent: Primary size: Small}
    }

    mod.widgets.MarkerBase = #(Marker::register_widget(vm))
    /** A numbered pin at a relative position over the content it wraps, or
     * over whatever it is laid over with `flow: Overlay`; a click on the pin
     * raises Clicked. */
    mod.widgets.Marker = set_type_default() do mod.widgets.MarkerBase{
        width: Fit
        height: Fit
        /** pin centre across the content 0..1 step 0.01 */
        x: 0.5
        /** pin centre down the content 0..1 step 0.01 */
        y: 0.5
        /** the number on the pin; 0 is a dot 0..999 step 1 */
        number: 0
        /** the pin itself; `badge +: {intent: Error}` at a call site */
        badge: mod.widgets.Badge{intent: Primary}
    }

    mod.widgets.StatusDotBase = #(StatusDot::register_widget(vm))
    /** A status as a shape AND a colour: circle, triangle, square, i-circle,
     * breathing ring, dashed ring, hollow ring. */
    mod.widgets.StatusDot = set_type_default() do mod.widgets.StatusDotBase{
        width: 12
        height: 12
        /** StatusKind.Unknown Success Warning Error Info InProgress Pending */
        status: StatusKind.Unknown
        /** alpha while disabled 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        draw_bg +: {
            color_success: uniform(theme.color_success)
            color_warning: uniform(theme.color_warning)
            color_error: uniform(theme.color_error)
            color_info: uniform(theme.color_info)
            /** ink of the i on the info circle */
            color_on_info: uniform(theme.color_on_info)
            color_in_progress: uniform(theme.color_primary)
            color_unknown: uniform(theme.color_outline)
            color_pending: uniform(theme.color_on_surface_variant)
            /** ring stroke width in pixels 0.5..4 step 0.25 */
            stroke: uniform(1.5)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let s = min(self.rect_size.x, self.rect_size.y)
                let cx = self.rect_size.x * 0.5
                let cy = self.rect_size.y * 0.5
                // The glyph sits inside 0.7 of the box: the ring that
                // breathes around the in-progress mark needs the margin.
                let r = s * 0.35
                let color = match self.status {
                    StatusKind.Success => self.color_success
                    StatusKind.Warning => self.color_warning
                    StatusKind.Error => self.color_error
                    StatusKind.Info => self.color_info
                    StatusKind.InProgress => self.color_in_progress
                    StatusKind.Pending => self.color_pending
                    _ => self.color_unknown
                }
                match self.status {
                    StatusKind.Success => {
                        sdf.circle(cx, cy, r)
                        sdf.fill(color * self.opacity)
                    }
                    StatusKind.Warning => {
                        sdf.move_to(cx, cy - r * 1.1)
                        sdf.line_to(cx + r * 1.15, cy + r * 0.9)
                        sdf.line_to(cx - r * 1.15, cy + r * 0.9)
                        sdf.close_path()
                        sdf.fill(color * self.opacity)
                    }
                    StatusKind.Error => {
                        sdf.box(cx - r * 0.95, cy - r * 0.95, r * 1.9, r * 1.9, 0.5)
                        sdf.fill(color * self.opacity)
                    }
                    StatusKind.Info => {
                        sdf.circle(cx, cy, r)
                        sdf.fill(color * self.opacity)
                        sdf.circle(cx, cy - r * 0.5, r * 0.16)
                        sdf.fill(self.color_on_info * self.opacity)
                        // A rect, not a box with no radius: sdf.box's interior
                        // distance is twice the radius, so at zero the fill gets
                        // no coverage and the stem of the "i" never appeared.
                        sdf.rect(cx - r * 0.14, cy - r * 0.2, r * 0.28, r * 0.85)
                        sdf.fill(self.color_on_info * self.opacity)
                    }
                    StatusKind.InProgress => {
                        sdf.circle(cx, cy, r * 0.85)
                        sdf.stroke(color * self.opacity, self.stroke)
                    }
                    StatusKind.Pending => {
                        sdf.circle(cx, cy, r * 0.85)
                        sdf.stroke(color * self.opacity, self.stroke * 0.75)
                    }
                    _ => {
                        // Four arcs with four gaps: a ring that says
                        // "nothing is known" rather than "nothing is wrong".
                        let ra = r * 0.85
                        sdf.arc_flat_caps(cx, cy, ra, 0.1, 1.2, self.stroke)
                        sdf.arc_flat_caps(cx, cy, ra, 1.67, 2.77, self.stroke)
                        sdf.arc_flat_caps(cx, cy, ra, 3.24, 4.34, self.stroke)
                        sdf.arc_flat_caps(cx, cy, ra, 4.81, 5.91, self.stroke)
                        sdf.fill(color * self.opacity)
                    }
                }
                return sdf.result
            }
        }
        // The one time-driven mark. Its own shader, so only a screen with
        // an in-progress dot on it repaints at display rate.
        draw_pulse +: {
            /** seconds per breath 0.5..4 step 0.1 */
            period: uniform(1.6)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let s = min(self.rect_size.x, self.rect_size.y)
                let t = fract(self.draw_pass.time / self.period)
                let r = s * mix(0.3, 0.48, t)
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, r)
                sdf.stroke(self.color * (1.0 - t) * self.opacity, 1.0)
                return sdf.result
            }
        }
    }

    mod.widgets.LabelValueBase = #(LabelValue::register_widget(vm))
    /** A `label: value` pill whose value half is coloured by where the value
     * sits between min and max. */
    mod.widgets.LabelValue = set_type_default() do mod.widgets.LabelValueBase{
        width: Fit
        height: 18
        /** the quiet half */
        label: "label"
        /** the number in the coloured half */
        value: 0.0
        /** decimals shown 0..4 step 1 */
        precision: 0
        /** appended to the value, "%" or " ms" */
        unit: ""
        /** the value that reads as color_low */
        min: 0.0
        /** the value that reads as color_high */
        max: 1.0
        /** value-half fill at min */
        color_low: theme.color_success
        /** value-half fill half way */
        color_mid: theme.color_warning
        /** value-half fill at max */
        color_high: theme.color_error
        /** value ink at min */
        ink_low: theme.color_on_success
        /** value ink half way */
        ink_mid: theme.color_on_warning
        /** value ink at max */
        ink_high: theme.color_on_error
        /** horizontal padding inside each half 2..12 step 0.5 */
        pad: 6.0
        /** alpha while disabled 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        // `split`, `color` and `color_value` are the instances (see the
        // badge above); everything else is a uniform.
        draw_bg +: {
            /** label-half fill */
            color: theme.color_opaque_u_2
            /** outline stroke */
            border_color: uniform(theme.color_outline_variant)
            /** outline width in pixels 0..2 step 0.5 */
            border_size: uniform(1.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.rect_size.y * 0.5)
                sdf.fill_keep(mix(self.color, self.color_value, step(self.split, self.pos.x)))
                if self.border_size > 0.0 {
                    sdf.stroke(self.border_color, self.border_size)
                }
                return sdf.result
            }
        }
        draw_label +: {
            text_style: theme.font_bold{font_size: 8.5}
            color: theme.color_text
        }
        draw_value +: {
            text_style: theme.font_bold{font_size: 8.5}
            color: theme.color_text
        }
    }
}

/// Fill, stroke, stroke width and corner radius: set by the widget every
/// draw, so a badge changes colour without a shader recompile.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBadge {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_size: f32,
    #[live]
    radius: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawStatusDot {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    status: StatusKind,
    #[live(1.0)]
    opacity: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawStatusPulse {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live(1.0)]
    opacity: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLabelValue {
    #[deref]
    draw_super: DrawQuad,
    /// Where the label half ends, as a fraction of the width.
    #[live]
    split: f32,
    #[live]
    color: Vec4f,
    #[live]
    color_value: Vec4f,
}

/// The colour of every intent, four roles each.
#[derive(Script, ScriptHook)]
pub struct BadgePalette {
    #[live]
    pub neutral: Vec4f,
    #[live]
    pub on_neutral: Vec4f,
    #[live]
    pub neutral_container: Vec4f,
    #[live]
    pub on_neutral_container: Vec4f,
    #[live]
    pub primary: Vec4f,
    #[live]
    pub on_primary: Vec4f,
    #[live]
    pub primary_container: Vec4f,
    #[live]
    pub on_primary_container: Vec4f,
    #[live]
    pub secondary: Vec4f,
    #[live]
    pub on_secondary: Vec4f,
    #[live]
    pub secondary_container: Vec4f,
    #[live]
    pub on_secondary_container: Vec4f,
    #[live]
    pub tertiary: Vec4f,
    #[live]
    pub on_tertiary: Vec4f,
    #[live]
    pub tertiary_container: Vec4f,
    #[live]
    pub on_tertiary_container: Vec4f,
    #[live]
    pub error: Vec4f,
    #[live]
    pub on_error: Vec4f,
    #[live]
    pub error_container: Vec4f,
    #[live]
    pub on_error_container: Vec4f,
    #[live]
    pub warning: Vec4f,
    #[live]
    pub on_warning: Vec4f,
    #[live]
    pub warning_container: Vec4f,
    #[live]
    pub on_warning_container: Vec4f,
    #[live]
    pub success: Vec4f,
    #[live]
    pub on_success: Vec4f,
    #[live]
    pub success_container: Vec4f,
    #[live]
    pub on_success_container: Vec4f,
    #[live]
    pub info: Vec4f,
    #[live]
    pub on_info: Vec4f,
    #[live]
    pub info_container: Vec4f,
    #[live]
    pub on_info_container: Vec4f,
}

/// The four roles of one intent: base, ink on base, container, ink on
/// container.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntentColors {
    pub base: Vec4f,
    pub on_base: Vec4f,
    pub container: Vec4f,
    pub on_container: Vec4f,
}

impl BadgePalette {
    pub fn family(&self, intent: BadgeIntent) -> IntentColors {
        let (base, on_base, container, on_container) = match intent {
            BadgeIntent::Neutral => (
                self.neutral,
                self.on_neutral,
                self.neutral_container,
                self.on_neutral_container,
            ),
            BadgeIntent::Primary => (
                self.primary,
                self.on_primary,
                self.primary_container,
                self.on_primary_container,
            ),
            BadgeIntent::Secondary => (
                self.secondary,
                self.on_secondary,
                self.secondary_container,
                self.on_secondary_container,
            ),
            BadgeIntent::Tertiary => (
                self.tertiary,
                self.on_tertiary,
                self.tertiary_container,
                self.on_tertiary_container,
            ),
            BadgeIntent::Error => (
                self.error,
                self.on_error,
                self.error_container,
                self.on_error_container,
            ),
            BadgeIntent::Warning => (
                self.warning,
                self.on_warning,
                self.warning_container,
                self.on_warning_container,
            ),
            BadgeIntent::Success => (
                self.success,
                self.on_success,
                self.success_container,
                self.on_success_container,
            ),
            BadgeIntent::Info => (
                self.info,
                self.on_info,
                self.info_container,
                self.on_info_container,
            ),
        };
        IntentColors {
            base,
            on_base,
            container,
            on_container,
        }
    }
}

/// What the appearance makes of an intent: the fill, the ink, the stroke and
/// its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgeColors {
    pub fill: Vec4f,
    pub ink: Vec4f,
    pub border: Vec4f,
    pub border_size: f32,
}

pub fn badge_colors(family: IntentColors, appearance: BadgeAppearance) -> BadgeColors {
    let clear = Vec4f::default();
    match appearance {
        BadgeAppearance::Filled => BadgeColors {
            fill: family.base,
            ink: family.on_base,
            border: clear,
            border_size: 0.0,
        },
        BadgeAppearance::Ghost => BadgeColors {
            fill: clear,
            ink: family.base,
            border: clear,
            border_size: 0.0,
        },
        BadgeAppearance::Outline => BadgeColors {
            fill: clear,
            ink: family.base,
            border: family.base,
            border_size: 1.0,
        },
        BadgeAppearance::Tint => BadgeColors {
            fill: family.container,
            ink: family.on_container,
            border: clear,
            border_size: 0.0,
        },
    }
}

/// One rung of the size ladder, in layout points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgeMetrics {
    /// Pill height.
    pub height: f64,
    /// Dot diameter.
    pub dot: f64,
    /// Horizontal padding either side of the text.
    pub pad_x: f64,
    /// Multiplier on the declared font size.
    pub font_scale: f32,
}

impl BadgeSize {
    pub fn metrics(self) -> BadgeMetrics {
        match self {
            BadgeSize::Tiny => BadgeMetrics { height: 12.0, dot: 6.0, pad_x: 3.0, font_scale: 0.75 },
            BadgeSize::Small => BadgeMetrics { height: 15.0, dot: 8.0, pad_x: 4.0, font_scale: 0.875 },
            BadgeSize::Medium => BadgeMetrics { height: 18.0, dot: 10.0, pad_x: 5.0, font_scale: 1.0 },
            BadgeSize::Large => BadgeMetrics { height: 22.0, dot: 12.0, pad_x: 6.0, font_scale: 1.125 },
            BadgeSize::Xl => BadgeMetrics { height: 28.0, dot: 14.0, pad_x: 8.0, font_scale: 1.35 },
        }
    }
}

/// The text a badge shows for a count: the number, or "max+" past the cap.
pub fn count_text(count: usize, max: usize) -> String {
    if max > 0 && count > max {
        format!("{max}+")
    } else {
        count.to_string()
    }
}

/// A `Fit` walk becomes the size the widget worked out; a fixed one is the
/// call site's business and is left alone.
pub(crate) fn sized(mut walk: Walk, w: f64, h: f64) -> Walk {
    if matches!(walk.width, Size::Fit { .. }) {
        walk.width = Size::Fixed(w);
    }
    if matches!(walk.height, Size::Fit { .. }) {
        walk.height = Size::Fixed(h);
    }
    walk
}

fn dimmed(color: Vec4f, opacity: f32) -> Vec4f {
    Vec4f { w: color.w * opacity, ..color }
}

/// What a drawn run overhangs its measured advance by, in layout points:
/// the last glyph's side bearing and the anti-alias pad. Without it a word
/// pill clips its final letter.
const TEXT_SLACK: f64 = 2.0;

/// The width of one line of `text` in this text style, measured, plus the
/// slack a drawn run needs; a per-character estimate only for text the
/// layout engine returns no row for.
pub(crate) fn measure(draw_text: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    advance(draw_text, cx, text) + TEXT_SLACK
}

/// How far a drawn run advances the pen, without the slack a box drawn AROUND
/// it needs.
///
/// This is what a widget laying its own label out takes: the turtle walks the
/// advance, so a width measured with the slack on is two points wider than the
/// box that gets drawn. Use [`measure`] to draw a box around a run, and this to
/// predict the width of a widget that already contains one.
pub(crate) fn advance(draw_text: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    draw_text
        .prepare_single_line_run(cx, text)
        .map(|run| run.width_in_lpxs as f64)
        .unwrap_or_else(|| text.chars().count() as f64 * draw_text.text_style.font_size as f64 * 0.62)
}

#[derive(Script, ScriptHook, Widget)]
pub struct Badge {
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
    draw_bg: DrawBadge,
    #[live]
    draw_text: DrawText,

    /// A word shown instead of the count.
    #[live]
    pub text: String,
    /// The number shown; 0 collapses to a dot unless `show_zero`.
    #[live]
    pub count: usize,
    /// Counts above this render as "max+".
    #[live(99usize)]
    pub max: usize,
    #[live]
    pub show_zero: bool,
    /// The dot form whatever the count and text say.
    #[live]
    pub dot: bool,
    #[live]
    pub intent: BadgeIntent,
    #[live]
    pub appearance: BadgeAppearance,
    #[live]
    pub size: BadgeSize,
    #[live]
    pub shape: BadgeShape,
    /// Corner radius of the Rounded shape.
    #[live(4.0)]
    pub radius: f64,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live]
    pub palette: BadgePalette,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust]
    disabled: bool,
}

impl Badge {
    /// What the badge shows: the word, else the count, else nothing (a dot).
    pub fn display_text(&self) -> String {
        if self.dot {
            return String::new();
        }
        if !self.text.is_empty() {
            return self.text.clone();
        }
        if self.count > 0 || self.show_zero {
            return count_text(self.count, self.max);
        }
        String::new()
    }

    /// True when there is nothing to show but a dot, and the dot was not
    /// asked for: what an anchor hides.
    pub fn is_empty(&self) -> bool {
        !self.dot && self.text.is_empty() && self.count == 0 && !self.show_zero
    }

    pub fn set_count(&mut self, cx: &mut Cx, count: usize) {
        if self.count != count {
            self.count = count;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_intent(&mut self, cx: &mut Cx, intent: BadgeIntent) {
        if self.intent != intent {
            self.intent = intent;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_appearance(&mut self, cx: &mut Cx, appearance: BadgeAppearance) {
        if self.appearance != appearance {
            self.appearance = appearance;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_dot(&mut self, cx: &mut Cx, dot: bool) {
        if self.dot != dot {
            self.dot = dot;
            self.draw_bg.redraw(cx);
        }
    }

    fn colors(&self) -> BadgeColors {
        let mut colors = badge_colors(self.palette.family(self.intent), self.appearance);
        if self.disabled {
            colors.fill = dimmed(colors.fill, self.disabled_opacity);
            colors.ink = dimmed(colors.ink, self.disabled_opacity);
            colors.border = dimmed(colors.border, self.disabled_opacity);
        }
        colors
    }

    fn visual_radius(&self, height: f64) -> f32 {
        match self.shape {
            BadgeShape::Round => (height * 0.5) as f32,
            BadgeShape::Rounded => self.radius as f32,
            BadgeShape::Square => 0.0,
        }
    }

    /// The size the badge takes for `text` at its size rung: a dot's
    /// diameter, or a pill at least as wide as it is tall so a single digit
    /// sits in a circle.
    fn extent(&mut self, cx: &mut Cx2d, text: &str) -> (f64, f64) {
        let metrics = self.size.metrics();
        if text.is_empty() {
            return (metrics.dot, metrics.dot);
        }
        let text_w = measure(&self.draw_text, cx, text);
        let h = metrics.height;
        ((text_w + metrics.pad_x * 2.0).max(h), h)
    }

    /// Draw at `walk`, returning the rect drawn. Set, draw, RESTORE: the
    /// text's colour and size are the call site's rest values, re-applied
    /// on every live edit, so the intent ink and the size rung are laid
    /// over them per draw rather than written into them.
    pub fn draw_badge(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        let text = self.display_text();
        let colors = self.colors();
        let metrics = self.size.metrics();
        let (w, h) = {
            let font_rest = self.draw_text.text_style.font_size;
            self.draw_text.text_style.font_size = font_rest * metrics.font_scale;
            let extent = self.extent(cx, &text);
            self.draw_text.text_style.font_size = font_rest;
            extent
        };
        self.draw_bg.color = colors.fill;
        self.draw_bg.border_color = colors.border;
        self.draw_bg.border_size = colors.border_size;
        self.draw_bg.radius = self.visual_radius(h);
        let walk = sized(walk, w, h);
        if text.is_empty() {
            return self.draw_bg.draw_walk(cx, walk);
        }
        let ink_rest = self.draw_text.color;
        let font_rest = self.draw_text.text_style.font_size;
        self.draw_text.color = colors.ink;
        self.draw_text.text_style.font_size = font_rest * metrics.font_scale;
        // The CONTAINER centres the text. The align handed to the text
        // itself stays left: the layouter would otherwise centre the row
        // inside the container's width as well, and the two shifts add up
        // to a word pushed against the pill's right edge.
        self.draw_bg.begin(
            cx,
            walk,
            Layout {
                align: Align { x: 0.5, y: 0.5 },
                ..Layout::flow_right()
            },
        );
        self.draw_text
            .draw_walk(cx, Walk::fit(), Align::default(), &text);
        self.draw_bg.end(cx);
        self.draw_text.color = ink_rest;
        self.draw_text.text_style.font_size = font_rest;
        self.draw_bg.area().rect(cx)
    }
}

impl Widget for Badge {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.draw_badge(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    /// The text on screen: the word, or the capped count.
    fn text(&self) -> String {
        self.display_text()
    }

    /// A number sets the count; anything else is the word.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        match v.trim().parse::<usize>() {
            Ok(count) => {
                self.text.clear();
                self.count = count;
            }
            Err(_) => self.text = v.to_string(),
        }
        self.draw_bg.redraw(cx);
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

    /// The raw count, uncapped, so a test can wait on the number the host
    /// pushed rather than on "99+".
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.count.to_string())
    }
}

impl BadgeRef {
    pub fn set_count(&self, cx: &mut Cx, count: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_count(cx, count);
        }
    }

    pub fn count(&self) -> usize {
        self.borrow().map(|inner| inner.count).unwrap_or(0)
    }

    pub fn set_intent(&self, cx: &mut Cx, intent: BadgeIntent) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_intent(cx, intent);
        }
    }

    pub fn set_appearance(&self, cx: &mut Cx, appearance: BadgeAppearance) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_appearance(cx, appearance);
        }
    }

    pub fn set_dot(&self, cx: &mut Cx, dot: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_dot(cx, dot);
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct StatusDot {
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
    draw_bg: DrawStatusDot,
    #[live]
    draw_pulse: DrawStatusPulse,
    #[live]
    pub status: StatusKind,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust]
    disabled: bool,
}

impl StatusDot {
    pub fn set_status(&mut self, cx: &mut Cx, status: StatusKind) {
        if self.status != status {
            self.status = status;
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for StatusDot {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let opacity = if self.disabled { self.disabled_opacity } else { 1.0 };
        self.draw_bg.status = self.status;
        self.draw_bg.opacity = opacity;
        self.draw_bg.begin(cx, walk, Layout::default());
        if self.status == StatusKind::InProgress {
            // The breathing ring fills the same box as the mark; its colour
            // is the mark's, read back from the shader's uniform table
            // would cost a lookup per draw, so it is the primary role here.
            self.draw_pulse.opacity = opacity;
            self.draw_pulse.draw_walk(cx, Walk::fill());
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    /// The status by name: "success", "in-progress", ...
    fn text(&self) -> String {
        self.status.name().to_string()
    }

    /// Accepts a status name; anything else is ignored.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some(status) = StatusKind::from_name(v) {
            self.set_status(cx, status);
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

impl StatusDotRef {
    pub fn set_status(&self, cx: &mut Cx, status: StatusKind) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_status(cx, status);
        }
    }

    pub fn status(&self) -> Option<StatusKind> {
        self.borrow().map(|inner| inner.status)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct LabelValue {
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
    draw_bg: DrawLabelValue,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_value: DrawText,
    #[live]
    pub label: String,
    #[live]
    pub value: f64,
    /// Decimals shown.
    #[live]
    pub precision: usize,
    /// Appended to the value.
    #[live]
    pub unit: String,
    /// The value that reads as `color_low`.
    #[live]
    pub min: f64,
    /// The value that reads as `color_high`.
    #[live(1.0)]
    pub max: f64,
    #[live]
    pub color_low: Vec4f,
    #[live]
    pub color_mid: Vec4f,
    #[live]
    pub color_high: Vec4f,
    #[live]
    pub ink_low: Vec4f,
    #[live]
    pub ink_mid: Vec4f,
    #[live]
    pub ink_high: Vec4f,
    /// Horizontal padding inside each half.
    #[live(6.0)]
    pub pad: f64,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust]
    disabled: bool,
}

/// Where `value` sits between `min` and `max`, 0..1; 0 when the range is
/// empty or reversed.
pub fn magnitude(value: f64, min: f64, max: f64) -> f64 {
    if max <= min {
        return 0.0;
    }
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

/// Three-stop scale: low to mid over the first half, mid to high over the
/// second.
pub fn scale_color(low: Vec4f, mid: Vec4f, high: Vec4f, t: f64) -> Vec4f {
    let lerp = |a: Vec4f, b: Vec4f, t: f32| Vec4f {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        z: a.z + (b.z - a.z) * t,
        w: a.w + (b.w - a.w) * t,
    };
    if t < 0.5 {
        lerp(low, mid, (t * 2.0) as f32)
    } else {
        lerp(mid, high, ((t - 0.5) * 2.0) as f32)
    }
}

impl LabelValue {
    pub fn value_text(&self) -> String {
        format!("{:.*}{}", self.precision, self.value, self.unit)
    }

    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        if self.value != value {
            self.value = value;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_label(&mut self, cx: &mut Cx, label: &str) {
        if self.label != label {
            self.label = label.to_string();
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for LabelValue {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let value_text = self.value_text();
        let t = magnitude(self.value, self.min, self.max);
        let opacity = if self.disabled { self.disabled_opacity } else { 1.0 };
        let fill = dimmed(scale_color(self.color_low, self.color_mid, self.color_high, t), opacity);
        let ink = dimmed(scale_color(self.ink_low, self.ink_mid, self.ink_high, t), opacity);
        let label_w = measure(&self.draw_label, cx, &self.label);
        let value_w = measure(&self.draw_value, cx, &value_text);
        let label_half = self.pad * 2.0 + label_w;
        let w = label_half + value_w + self.pad * 2.0;
        let h = match walk.height {
            Size::Fixed(h) => h,
            _ => 18.0,
        };
        self.draw_bg.split = (label_half / w) as f32;
        self.draw_bg.color_value = fill;
        // Set, draw, restore: the value ink follows the magnitude, the
        // label ink is the call site's.
        let ink_rest = self.draw_value.color;
        let label_rest = self.draw_label.color;
        self.draw_value.color = ink;
        self.draw_label.color = dimmed(label_rest, opacity);
        self.draw_bg.begin(
            cx,
            sized(walk, w, h),
            Layout {
                padding: Inset {
                    left: self.pad,
                    right: self.pad,
                    top: 0.0,
                    bottom: 0.0,
                },
                spacing: self.pad * 2.0,
                align: Align { x: 0.0, y: 0.5 },
                ..Layout::flow_right()
            },
        );
        self.draw_label
            .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &self.label);
        self.draw_value
            .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, &value_text);
        self.draw_bg.end(cx);
        self.draw_value.color = ink_rest;
        self.draw_label.color = label_rest;
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    /// "label: value", as read.
    fn text(&self) -> String {
        format!("{}: {}", self.label, self.value_text())
    }

    /// A number sets the value; anything else is the label.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        match v.trim().parse::<f64>() {
            Ok(value) => self.set_value(cx, value),
            Err(_) => self.set_label(cx, v),
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
        Some(self.value_text())
    }
}

impl LabelValueRef {
    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }

    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value).unwrap_or(0.0)
    }

    pub fn set_label(&self, cx: &mut Cx, label: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_label(cx, label);
        }
    }
}

/// Where the badge's top-left goes, relative to the host's top-left, for
/// a badge of `badge` size on a host of `host` size: pulled into the corner
/// and hung `overhang` of its own size outside it.
pub fn corner_offset(corner: BadgeCorner, host: DVec2, badge: DVec2, overhang: f64) -> DVec2 {
    let out = badge * overhang;
    match corner {
        BadgeCorner::TopRight => dvec2(host.x - badge.x + out.x, -out.y),
        BadgeCorner::TopLeft => dvec2(-out.x, -out.y),
        BadgeCorner::BottomRight => dvec2(host.x - badge.x + out.x, host.y - badge.y + out.y),
        BadgeCorner::BottomLeft => dvec2(-out.x, host.y - badge.y + out.y),
    }
}

/// Where a pin's top-left goes so that its centre lands at (`x`, `y`) of
/// the content, both 0..1.
pub fn pin_offset(x: f64, y: f64, host: DVec2, badge: DVec2) -> DVec2 {
    dvec2(
        x.clamp(0.0, 1.0) * host.x - badge.x * 0.5,
        y.clamp(0.0, 1.0) * host.y - badge.y * 0.5,
    )
}

#[derive(Script, Widget)]
pub struct BadgeAnchor {
    #[deref]
    view: View,
    /// The badge drawn over the corner.
    #[live]
    pub badge: Badge,
    #[live]
    pub corner: BadgeCorner,
    /// Nudge along x, positive moves right.
    #[live]
    pub offset_x: f64,
    /// Nudge along y, positive moves down.
    #[live]
    pub offset_y: f64,
    /// How much of the badge hangs outside the corner, 0..1.
    #[live(0.5)]
    pub overhang: f64,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for BadgeAnchor {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
}

/// The overlay list draws inside the host's draw pass, so the host's own
/// area has to be dirtied along with the list, or a count that changed in
/// a resting window would wait for the next unrelated redraw.
fn redraw_overlay(cx: &mut Cx, view: &mut View, draw_list: &Option<DrawList2d>) {
    if let Some(draw_list) = draw_list {
        draw_list.redraw(cx);
    }
    view.redraw(cx);
}

/// Draw `badge` on `draw_list` at `offset` from the top-left of `anchor`'s
/// final rect. The list is begun and ended even when there is nothing to
/// draw, so a badge that has just gone quiet is cleared rather than left
/// on screen from the previous frame.
fn draw_badge_overlay(
    cx: &mut Cx2d,
    draw_list: &mut DrawList2d,
    anchor: Area,
    badge: &mut Badge,
    show: bool,
    offset: impl FnOnce(DVec2) -> DVec2,
) {
    // The PROVEN popup idiom (PopupMenu, TipLayer): the badge as turtle
    // content at the overlay root, then the whole list SHIFTED to the host's
    // final rect. A draw_abs into a bare overlay list renders nothing.
    draw_list.begin_overlay_reuse(cx);
    let pass = cx.current_pass_size();
    cx.begin_root_turtle(pass, Layout::flow_down());
    let shift = if show {
        let rect = badge.draw_badge(cx, Walk::fit());
        offset(rect.size)
    } else {
        DVec2::default()
    };
    cx.end_pass_sized_turtle_with_shift(anchor, shift);
    draw_list.end(cx);
}

impl BadgeAnchor {
    pub fn set_count(&mut self, cx: &mut Cx, count: usize) {
        if self.badge.count != count {
            self.badge.count = count;
            redraw_overlay(cx, &mut self.view, &self.draw_list);
        }
    }

    pub fn set_dot(&mut self, cx: &mut Cx, dot: bool) {
        if self.badge.dot != dot {
            self.badge.dot = dot;
            redraw_overlay(cx, &mut self.view, &self.draw_list);
        }
    }

    pub fn set_intent(&mut self, cx: &mut Cx, intent: BadgeIntent) {
        if self.badge.intent != intent {
            self.badge.intent = intent;
            redraw_overlay(cx, &mut self.view, &self.draw_list);
        }
    }
}

impl Widget for BadgeAnchor {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        if !step.is_done() {
            return step;
        }
        let Some(draw_list) = self.draw_list.as_mut() else {
            return DrawStep::done();
        };
        // Sizes are honest at draw time, positions are not: the offset is
        // worked out from the host's SIZE and applied to its FINAL rect by
        // the shift.
        let host = self.view.area().rect(cx).size;
        let show = !self.badge.is_empty();
        let (corner, overhang, nudge) =
            (self.corner, self.overhang, dvec2(self.offset_x, self.offset_y));
        draw_badge_overlay(cx, draw_list, self.view.area(), &mut self.badge, show, |size| {
            corner_offset(corner, host, size, overhang) + nudge
        });
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    /// The badge's text: the word or the capped count.
    fn text(&self) -> String {
        self.badge.display_text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.badge.set_text(cx, v);
        redraw_overlay(cx, &mut self.view, &self.draw_list);
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.badge.set_disabled(cx, disabled);
        self.view.set_disabled(cx, disabled);
        redraw_overlay(cx, &mut self.view, &self.draw_list);
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.badge.disabled(cx)
    }

    fn snapshot_value(&self, cx: &Cx) -> Option<String> {
        self.badge.snapshot_value(cx)
    }
}

impl BadgeAnchorRef {
    pub fn set_count(&self, cx: &mut Cx, count: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_count(cx, count);
        }
    }

    pub fn count(&self) -> usize {
        self.borrow().map(|inner| inner.badge.count).unwrap_or(0)
    }

    pub fn set_dot(&self, cx: &mut Cx, dot: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_dot(cx, dot);
        }
    }

    pub fn set_intent(&self, cx: &mut Cx, intent: BadgeIntent) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_intent(cx, intent);
        }
    }
}

#[derive(Script, Widget)]
pub struct Marker {
    #[deref]
    view: View,
    /// The pin.
    #[live]
    pub badge: Badge,
    /// Pin centre across the content, 0..1.
    #[live(0.5)]
    pub x: f64,
    /// Pin centre down the content, 0..1.
    #[live(0.5)]
    pub y: f64,
    /// The number on the pin; 0 is a dot.
    #[live]
    pub number: usize,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for Marker {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
}

impl Marker {
    pub fn set_number(&mut self, cx: &mut Cx, number: usize) {
        if self.number != number {
            self.number = number;
            redraw_overlay(cx, &mut self.view, &self.draw_list);
        }
    }

    pub fn set_position(&mut self, cx: &mut Cx, x: f64, y: f64) {
        if self.x != x || self.y != y {
            self.x = x;
            self.y = y;
            redraw_overlay(cx, &mut self.view, &self.draw_list);
        }
    }
}

impl Widget for Marker {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        if !step.is_done() {
            return step;
        }
        let Some(draw_list) = self.draw_list.as_mut() else {
            return DrawStep::done();
        };
        // The number is the pin's count; a dot pin is a badge with nothing
        // to say, asked for explicitly so the anchor's hide rule stays out
        // of it.
        self.badge.count = self.number;
        self.badge.dot = self.number == 0;
        let host = self.view.area().rect(cx).size;
        let (x, y) = (self.x, self.y);
        draw_badge_overlay(cx, draw_list, self.view.area(), &mut self.badge, true, |size| {
            pin_offset(x, y, host, size)
        });
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The pin first: its area is the badge's quad on the overlay list,
        // and a press it takes is marked handled before the content under
        // it is walked.
        match event.hits(cx, self.badge.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerHoverOut(_) => cx.set_cursor(MouseCursor::Default),
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() && fe.was_tap() => {
                cx.widget_action(self.widget_uid(), MarkerAction::Clicked);
            }
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
    }

    /// The pin's number, "" for a dot.
    fn text(&self) -> String {
        self.badge.display_text()
    }

    /// A number sets the pin's number.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Ok(number) = v.trim().parse::<usize>() {
            self.set_number(cx, number);
        }
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.badge.set_disabled(cx, disabled);
        redraw_overlay(cx, &mut self.view, &self.draw_list);
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.badge.disabled(cx)
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.number.to_string())
    }
}

impl MarkerRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<MarkerAction>(), MarkerAction::Clicked);
        }
        false
    }

    pub fn number(&self) -> usize {
        self.borrow().map(|inner| inner.number).unwrap_or(0)
    }

    pub fn set_number(&self, cx: &mut Cx, number: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_number(cx, number);
        }
    }

    pub fn set_position(&self, cx: &mut Cx, x: f64, y: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_position(cx, x, y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_wired() {
        let lib = include_str!("lib.rs");
        let badge = include_str!("badge.rs");
        assert!(lib.contains("pub mod badge;"));
        assert!(lib.contains("badge::*"));
        assert!(lib.contains("crate::badge::script_mod(vm);"));
        assert!(badge.contains("mod.widgets.BadgeBase = #(Badge::register_widget(vm))"));
        assert!(badge.contains("mod.widgets.BadgeFlat = set_type_default()"));
        assert!(badge.contains("mod.widgets.StatusDotBase = #(StatusDot::register_widget(vm))"));
        assert!(badge.contains("mod.widgets.LabelValueBase = #(LabelValue::register_widget(vm))"));
        assert!(badge.contains("mod.widgets.BadgeAnchorBase = #(BadgeAnchor::register_widget(vm))"));
        assert!(badge.contains("mod.widgets.MarkerBase = #(Marker::register_widget(vm))"));
        // The anchor and the marker deref View, which registers first.
        let view = lib.find("crate::view::script_mod(vm);").unwrap();
        assert!(view < lib.find("crate::badge::script_mod(vm);").unwrap());
        // Badge registers after the label module its text draws with.
        let label = lib.find("crate::label::script_mod(vm);").unwrap();
        let badge_at = lib.find("crate::badge::script_mod(vm);").unwrap();
        assert!(label < badge_at);
    }

    #[test]
    fn counts_cap_at_max() {
        assert_eq!(count_text(0, 99), "0");
        assert_eq!(count_text(99, 99), "99");
        assert_eq!(count_text(100, 99), "99+");
        assert_eq!(count_text(1204, 999), "999+");
        assert_eq!(count_text(1204, 0), "1204");
    }

    #[test]
    fn status_names_round_trip() {
        for kind in [
            StatusKind::Unknown,
            StatusKind::Success,
            StatusKind::Warning,
            StatusKind::Error,
            StatusKind::Info,
            StatusKind::InProgress,
            StatusKind::Pending,
        ] {
            assert_eq!(StatusKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(StatusKind::from_name("In Progress"), Some(StatusKind::InProgress));
        assert_eq!(StatusKind::from_name("nope"), None);
    }

    #[test]
    fn magnitude_clamps_and_survives_a_bad_range() {
        assert_eq!(magnitude(0.5, 0.0, 1.0), 0.5);
        assert_eq!(magnitude(-1.0, 0.0, 1.0), 0.0);
        assert_eq!(magnitude(5.0, 0.0, 1.0), 1.0);
        assert_eq!(magnitude(0.5, 1.0, 0.0), 0.0);
        assert_eq!(magnitude(75.0, 50.0, 100.0), 0.5);
    }

    #[test]
    fn corners_hang_the_badge_outside() {
        let host = dvec2(100.0, 40.0);
        let badge = dvec2(20.0, 16.0);
        assert_eq!(corner_offset(BadgeCorner::TopRight, host, badge, 0.5), dvec2(90.0, -8.0));
        assert_eq!(corner_offset(BadgeCorner::TopLeft, host, badge, 0.5), dvec2(-10.0, -8.0));
        assert_eq!(corner_offset(BadgeCorner::BottomRight, host, badge, 0.5), dvec2(90.0, 32.0));
        assert_eq!(corner_offset(BadgeCorner::BottomLeft, host, badge, 0.5), dvec2(-10.0, 32.0));
        // No overhang: flush inside the corner.
        assert_eq!(corner_offset(BadgeCorner::TopRight, host, badge, 0.0), dvec2(80.0, 0.0));
    }

    #[test]
    fn pins_centre_on_their_point() {
        let host = dvec2(200.0, 100.0);
        let badge = dvec2(20.0, 20.0);
        assert_eq!(pin_offset(0.5, 0.5, host, badge), dvec2(90.0, 40.0));
        assert_eq!(pin_offset(0.0, 0.0, host, badge), dvec2(-10.0, -10.0));
        assert_eq!(pin_offset(2.0, -1.0, host, badge), dvec2(190.0, -10.0));
    }

    #[test]
    fn scale_passes_through_its_stops() {
        let low = Vec4f { x: 0.0, y: 1.0, z: 0.0, w: 1.0 };
        let mid = Vec4f { x: 1.0, y: 1.0, z: 0.0, w: 1.0 };
        let high = Vec4f { x: 1.0, y: 0.0, z: 0.0, w: 1.0 };
        assert_eq!(scale_color(low, mid, high, 0.0), low);
        assert_eq!(scale_color(low, mid, high, 0.5), mid);
        assert_eq!(scale_color(low, mid, high, 1.0), high);
    }
}

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

/// What a `ProgressRing` draws.
#[derive(Clone, Copy, Debug, PartialEq, Default, Script, ScriptHook)]
pub enum ProgressShape {
    /// A closed ring, filled clockwise from the top.
    #[pick]
    #[default]
    Ring,
    /// An open arc — half a ring by default — with the label inside it.
    Arc,
    /// Nested rings, one per entry of `values`, outermost first.
    Rings,
}

const DISABLED_INK: f32 = 5.0;
/// The classic three: outer, middle, inner.
const RING_INTENTS: [Intent; 3] = [Intent::Error, Intent::Success, Intent::Info];

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.Intent = set_type_default() do #(Intent::script_api(vm))
    mod.widgets.splat(mod.widgets.Intent)
    mod.widgets.ProgressShape = #(ProgressShape::script_api(vm))

    mod.widgets.DrawProgressBarBase = #(DrawProgressBar::script_component(vm))
    set_type_default() do #(DrawProgressBar::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawProgressRingBase = #(DrawProgressRing::script_component(vm))
    set_type_default() do #(DrawProgressRing::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawGaugeBase = #(DrawGauge::script_component(vm))
    set_type_default() do #(DrawGauge::script_shader(vm)){
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

    mod.widgets.ProgressRingBase = #(ProgressRing::register_widget(vm))
    /** The flat progress ring: an arc that fills clockwise from the top,
     * with the percentage or a label in the middle. */
    mod.widgets.ProgressRingFlat = set_type_default() do mod.widgets.ProgressRingBase{
        width: 48
        height: 48
        /** where the fill stands; anything below zero is indeterminate -1..1 step 0.01 */
        value: 0.0
        /** one value per nested ring, outermost first, for the Rings shape */
        values: []
        /** the colour role of the fill */
        intent: mod.widgets.Intent.Primary
        /** ring, open arc or nested rings */
        shape: mod.widgets.ProgressShape.Ring
        /** cut the ring into this many sections; 0 or 1 is a whole ring 0..12 step 1 */
        sections: 0
        /** the gap between sections, in degrees 0..30 step 1 */
        section_gap_deg: 8.0
        /** space between nested rings 0..12 step 0.5 */
        ring_gap: 3.0
        /** show the value as a percentage in the middle 0..1 step 1 */
        show_percent: false
        /** a fixed label in the middle; wins over the percentage */
        text: ""
        /** seconds a value change takes to arrive 0..2 step 0.05 */
        ease_secs: theme.motion_medium_1
        /** greyed and dimmed 0..1 step 1 */
        disabled: false

        draw_bg +: {
            start: 0.0
            end: 1.0
            value: 0.0
            intent: 0.0
            track: 1.0
            opacity: 1.0
            inset: 0.0
            /** stroke width of the ring 1..24 step 0.5 */
            thickness: 5.0
            /** the angle the whole shape covers, in radians 0.5..6.2832 step 0.01 */
            sweep: 6.2831853
            /** the shader angle of the start; -pi is the top 0..6.2832 step 0.01 */
            base_angle: uniform(-3.14159265)
            /** 1 fills clockwise, -1 the other way -1..1 step 2 */
            direction: uniform(1.0)
            /** the centre's height as a fraction of the walk; 1 puts it on the bottom edge 0..1 step 0.05 */
            center_y: uniform(0.5)
            /** round the ends of the arc 0..1 step 1 */
            rounded_caps: uniform(1.0)
            /** indeterminate turn: revolutions per second 0.2..3 step 0.1 */
            sweep_speed: uniform(0.8)
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

            fill_color: fn() -> vec4 {
                let mut fill = self.color_primary
                if self.intent > 0.5 { fill = self.color_success }
                if self.intent > 1.5 { fill = self.color_warning }
                if self.intent > 2.5 { fill = self.color_error }
                if self.intent > 3.5 { fill = self.color_info }
                if self.intent > 4.5 { fill = self.color_disabled }
                return fill
            }

            // The shader angle of a fraction of the sweep. The sweep always
            // covers the same span from `base_angle`; a negative direction
            // MIRRORS the fraction so the fill runs from the other end,
            // rather than negating the angle, which would put the track on
            // the other half of the circle.
            angle_of: fn(x: float) -> float {
                let flip = step(self.direction, 0.0)
                return self.base_angle + mix(x, 1.0 - x, flip) * self.sweep
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let th = self.thickness
                let center = vec2(self.rect_size.x * 0.5, self.rect_size.y * self.center_y)
                let radius = min(self.rect_size.x * 0.5, self.rect_size.y * self.center_y) - th * 0.5 - self.inset
                let fill = self.fill_color()
                let a_start = self.angle_of(self.start)
                let a_end = self.angle_of(self.end)
                let lo = min(a_start, a_end)
                let hi = max(a_start, a_end)
                // The arc is cut three times (track, fill, sweep) from the
                // same centre; the cap style is a branch each time because
                // the shader language cannot hand the sdf to a helper.
                if self.track > 0.5 {
                    if hi - lo > 0.001 {
                        if self.rounded_caps > 0.5 {
                            sdf.arc_round_caps(center.x, center.y, radius, lo, hi, th)
                        } else {
                            sdf.arc_flat_caps(center.x, center.y, radius, lo, hi, th)
                        }
                        // fill_KEEP only when a stroke follows to consume
                        // the shape; a kept shape unions with the fill arc.
                        if self.border_color.w > 0.0 {
                            sdf.fill_keep(self.track_color)
                            sdf.stroke(self.border_color, 1.0)
                        } else {
                            sdf.fill(self.track_color)
                        }
                    }
                }
                if self.value >= 0.0 {
                    let v = clamp(self.value, self.start, self.end)
                    if v - self.start > 0.0005 {
                        let a_v = self.angle_of(v)
                        let vlo = min(a_start, a_v)
                        let vhi = max(a_start, a_v)
                        if self.rounded_caps > 0.5 {
                            sdf.arc_round_caps(center.x, center.y, radius, vlo, vhi, th)
                        } else {
                            sdf.arc_flat_caps(center.x, center.y, radius, vlo, vhi, th)
                        }
                        sdf.fill(fill)
                    }
                }
                if self.value < 0.0 {
                    let span = self.sweep * (self.end - self.start)
                    let len = span * 0.25
                    let mut a0 = lo
                    if self.sweep > 6.0 {
                        a0 = lo + fract(self.draw_pass.time * self.sweep_speed) * TAU
                    } else {
                        let tri = abs(fract(self.draw_pass.time * self.sweep_speed * 0.5) * 2.0 - 1.0)
                        a0 = lo + tri * tri * (3.0 - 2.0 * tri) * (span - len)
                    }
                    if self.rounded_caps > 0.5 {
                        sdf.arc_round_caps(center.x, center.y, radius, a0, a0 + len, th)
                    } else {
                        sdf.arc_flat_caps(center.x, center.y, radius, a0, a0 + len, th)
                    }
                    sdf.fill(fill)
                }
                return sdf.result * self.opacity
            }
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    /** The standard progress ring: the flat ring with the inset bevel on its track. */
    mod.widgets.ProgressRing = mod.widgets.ProgressRingFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
        }
    }

    /** Half a ring, open at the bottom, filled left to right, label inside. */
    mod.widgets.ProgressArc = mod.widgets.ProgressRingFlat{
        width: 96
        height: 52
        shape: mod.widgets.ProgressShape.Arc
        draw_bg +: {
            sweep: 3.14159265
            base_angle: 1.5707963
            center_y: 1.0
            thickness: 7.0
        }
    }

    /** Three nested rings, one value each, outermost first. */
    mod.widgets.ActivityRings = mod.widgets.ProgressRingFlat{
        width: 72
        height: 72
        shape: mod.widgets.ProgressShape.Rings
        values: [0.0, 0.0, 0.0]
        draw_bg +: {
            thickness: 7.0
            track_color: theme.color_surface_container_high
        }
    }

    mod.widgets.GaugeBase = #(Gauge::register_widget(vm))
    /** A read-only dial: a three-quarter arc coloured by zone (safe, warning,
     * critical), a needle at the value, the value under the hub and the range
     * at the ends. */
    mod.widgets.Gauge = set_type_default() do mod.widgets.GaugeBase{
        width: 120
        height: 110
        /** the reading, in the units of `min`..`max` */
        value: 0.0
        /** the reading at the empty end */
        min: 0.0
        /** the reading at the full end */
        max: 100.0
        /** decimals in the value label 0..4 step 1 */
        precision: 0
        /** a suffix on the value label */
        unit: ""
        /** a fixed centre label; wins over the value */
        text: ""
        /** print the value under the hub 0..1 step 1 */
        show_value: true
        /** print min and max at the ends 0..1 step 1 */
        show_range: true
        /** the label at the empty end; the number when empty */
        min_label: ""
        /** the label at the full end; the number when empty */
        max_label: ""
        /** lay the zones out along a bar instead of an arc 0..1 step 1 */
        linear: false
        /** seconds a value change takes to arrive 0..2 step 0.05 */
        ease_secs: theme.motion_medium_1
        /** greyed and dimmed 0..1 step 1 */
        disabled: false

        draw_bg +: {
            value: 0.0
            opacity: 1.0
            /** stroke width of the arc or bar 1..24 step 0.5 */
            thickness: 8.0
            /** the angle the arc covers, in radians; the opening is centred at the bottom 0.5..6.2 step 0.01 */
            sweep: 4.712389
            /** the bar's corner radius, linear only 0..16 step 0.5 */
            border_radius: uniform(theme.radius_full)
            /** where the warning zone begins 0..1 step 0.01 */
            zone_warn: uniform(0.6)
            /** where the critical zone begins 0..1 step 0.01 */
            zone_critical: uniform(0.85)
            /** how wide the blend between zones is 0..0.2 step 0.01 */
            zone_soft: uniform(0.04)
            /** how much zone colour the unreached part of the arc shows 0..1 step 0.05 */
            band: uniform(0.3)
            /** draw the needle 0..1 step 1 */
            needle: uniform(1.0)
            /** lay the zones out along a bar 0..1 step 1 */
            linear: uniform(0.0)
            track_color: uniform(theme.color_surface_container_highest)
            color_safe: uniform(theme.color_success)
            color_warn: uniform(theme.color_warning)
            color_critical: uniform(theme.color_error)
            needle_color: uniform(theme.color_text)

            zone_color: fn(f: float) -> vec4 {
                let c = mix(self.color_safe, self.color_warn, smoothstep(self.zone_warn - self.zone_soft, self.zone_warn + self.zone_soft, f))
                return mix(c, self.color_critical, smoothstep(self.zone_critical - self.zone_soft, self.zone_critical + self.zone_soft, f))
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let v = clamp(self.value, 0.0, 1.0)
                let th = self.thickness
                if self.linear > 0.5 {
                    let w = self.rect_size.x
                    let h = min(th, self.rect_size.y)
                    let y0 = (self.rect_size.y - h) * 0.5
                    let r = min(self.border_radius, h * 0.5)
                    let zone = self.zone_color(self.pos.x)
                    sdf.box(0.0, y0, w, h, r)
                    sdf.fill(mix(self.track_color, zone, self.band))
                    if v > 0.001 {
                        sdf.box(0.0, y0, min(max(w * v, h), w), h, r)
                        sdf.fill(zone)
                    }
                    if self.needle > 0.5 {
                        let nx = clamp(w * v, 1.0, w - 1.0)
                        sdf.box(nx - 1.0, 0.0, 2.0, self.rect_size.y, 1.0)
                        sdf.fill(self.needle_color)
                    }
                } else {
                    let center = self.rect_size * 0.5
                    let radius = min(self.rect_size.x, self.rect_size.y) * 0.5 - th * 0.5
                    // Screen angle of the start: the opening sits centred
                    // on the bottom, so the arc begins that far past it.
                    let phi0 = 1.5 * PI - self.sweep * 0.5
                    let p = self.pos * self.rect_size - center
                    let rel = fract((atan2(p.y, p.x) - phi0) / TAU) * TAU
                    let zone = self.zone_color(clamp(rel / self.sweep, 0.0, 1.0))
                    // The arc functions take angles a quarter turn behind
                    // the screen's.
                    let t0 = phi0 - PI * 0.5
                    sdf.arc_round_caps(center.x, center.y, radius, t0, t0 + self.sweep, th)
                    sdf.fill(mix(self.track_color, zone, self.band))
                    if v > 0.001 {
                        sdf.arc_round_caps(center.x, center.y, radius, t0, t0 + v * self.sweep, th)
                        sdf.fill(zone)
                    }
                    if self.needle > 0.5 {
                        let phi = phi0 + v * self.sweep
                        let dir = vec2(cos(phi), sin(phi))
                        let tip = center + dir * (radius - th * 0.5 - 2.0)
                        let tail = center - dir * 6.0
                        sdf.move_to(tail.x, tail.y)
                        sdf.line_to(tip.x, tip.y)
                        sdf.stroke(self.needle_color, 1.0)
                        sdf.circle(center.x, center.y, 3.0)
                        sdf.fill(self.needle_color)
                    }
                }
                return sdf.result * self.opacity
            }
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_range +: {
            color: theme.color_on_surface_variant
            text_style: theme.font_regular{font_size: theme.type_label_s_size}
        }
    }

    /** The dial laid flat: the same zones along a bar with a tick at the
     * value and the range printed under the ends. */
    mod.widgets.GaugeLinear = mod.widgets.Gauge{
        width: Fill
        height: 44
        linear: true
        draw_bg +: {
            linear: 1.0
            thickness: 10.0
        }
    }

    mod.widgets.NavigationProgressBase = #(NavigationProgress::register_widget(vm))
    /** A hairline along the top of a page that starts on demand, trickles
     * toward the end while a load is in flight and fades once it is done. */
    mod.widgets.NavigationProgress = set_type_default() do mod.widgets.NavigationProgressBase{
        width: Fill
        height: 3
        /** the colour role of the line */
        intent: mod.widgets.Intent.Primary
        /** how fast the trickle closes on the ceiling, per second 0.05..3 step 0.05 */
        trickle_rate: 0.3
        /** the trickle never passes this 0.8..0.999 step 0.001 */
        trickle_cap: 0.994
        /** seconds the full line stays before it fades 0..2 step 0.1 */
        fade_delay_secs: 0.3
        /** seconds a step and the fade take 0..2 step 0.05 */
        ease_secs: theme.motion_medium_1
        draw_bg +: {
            start: 0.0
            end: 1.0
            value: 0.0
            intent: 0.0
            track: 0.0
            opacity: 0.0
            thickness: uniform(3.0)
            border_radius: uniform(0.0)
            gap: uniform(0.0)
            stop_indicator: uniform(0.0)
            sweep_width: uniform(0.35)
            sweep_speed: uniform(0.7)
            gradient: uniform(0.0)
            gradient_horizontal: uniform(0.0)
            gradient_shade: uniform(0.65)
            track_color: uniform(theme.color_u_hidden)
            color_primary: uniform(theme.color_primary)
            color_success: uniform(theme.color_success)
            color_warning: uniform(theme.color_warning)
            color_error: uniform(theme.color_error)
            color_info: uniform(theme.color_info)
            color_disabled: uniform(theme.color_val_disabled)
            border_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            border_color_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            fill_color: fn() -> vec4 {
                let mut fill = self.color_primary
                if self.intent > 0.5 { fill = self.color_success }
                if self.intent > 1.5 { fill = self.color_warning }
                if self.intent > 2.5 { fill = self.color_error }
                if self.intent > 3.5 { fill = self.color_info }
                if self.intent > 4.5 { fill = self.color_disabled }
                return fill
            }
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let xv = w * clamp(self.value, 0.0, 1.0)
                if xv > 0.5 {
                    sdf.box(0.0, 0.0, xv, h, self.border_radius)
                    sdf.fill(self.fill_color())
                }
                return sdf.result * self.opacity
            }
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

/// The ring's shader: the bar's instances plus `inset`, how far inside the
/// walk this ring's outer edge sits, which is what nests the activity rings.
/// `thickness` and `sweep` are instances rather than uniforms so the Rust
/// side can read the geometry it lays sections and labels out against.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawProgressRing {
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
    #[live]
    inset: f32,
    #[live(5.0)]
    thickness: f32,
    #[live(6.2831853)]
    sweep: f32,
}

/// The gauge's shader: one piece, always; the zones are uniforms. The
/// geometry the labels hang off (`thickness`, `sweep`) rides as instances.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGauge {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    value: f32,
    #[live]
    opacity: f32,
    #[live(8.0)]
    thickness: f32,
    #[live(4.712389)]
    sweep: f32,
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

/// A ring, an open arc or nested rings. Shares the bar's value contract:
/// `value` is the target, the drawn value eases toward it, forward only.
#[derive(Script, ScriptHook, Widget)]
pub struct ProgressRing {
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
    draw_bg: DrawProgressRing,
    #[live]
    draw_text: DrawText,
    #[live]
    pub text: String,
    #[live]
    pub value: f64,
    /// One value per nested ring, outermost first (the Rings shape).
    #[live]
    pub values: Vec<ScriptValue>,
    #[live(Intent::Primary)]
    pub intent: Intent,
    #[live(ProgressShape::Ring)]
    pub shape: ProgressShape,
    #[live]
    pub sections: usize,
    #[live]
    pub section_gap_deg: f64,
    #[live]
    pub ring_gap: f64,
    #[live]
    pub show_percent: bool,
    #[live]
    pub ease_secs: f64,
    #[live]
    pub disabled: bool,
    #[rust]
    shown: f64,
    #[rust]
    target: f64,
    #[rust]
    ease: Ease,
    #[rust]
    drawn: bool,
    #[rust]
    next_frame: NextFrame,
}

impl ProgressRing {
    pub fn label_text(&self) -> String {
        if !self.text.is_empty() {
            self.text.clone()
        } else if self.show_percent && self.shown >= 0.0 && self.shape != ProgressShape::Rings {
            percent_label(self.shown)
        } else {
            String::new()
        }
    }

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

    pub fn advance(&mut self, cx: &mut Cx, delta: f64) {
        let base = if self.value < 0.0 { 0.0 } else { self.value };
        self.set_value(cx, (base + delta).min(1.0));
    }

    /// The nested rings' values, outermost first. Rings snap: three values
    /// arriving together should land together.
    pub fn set_values(&mut self, cx: &mut Cx, values: &[f64]) {
        let clamped: Vec<f64> = values.iter().map(|v| v.clamp(0.0, 1.0)).collect();
        self.values = script_numbers(&clamped);
        self.draw_bg.redraw(cx);
    }

    pub fn set_intent(&mut self, cx: &mut Cx, intent: Intent) {
        if self.intent != intent {
            self.intent = intent;
            self.draw_bg.redraw(cx);
        }
    }

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

    /// One piece of ring: the fraction it covers and where its fill stands.
    fn piece(&mut self, cx: &mut Cx2d, rect: Rect, start: f64, end: f64, value: f64, intent: f32, inset: f64) {
        self.draw_bg.start = start as f32;
        self.draw_bg.end = end as f32;
        self.draw_bg.value = value as f32;
        self.draw_bg.intent = intent;
        self.draw_bg.track = 1.0;
        self.draw_bg.inset = inset as f32;
        self.draw_bg.draw_abs(cx, rect);
    }
}

impl Widget for ProgressRing {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.drawn || self.value != self.target {
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
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        let intent = if self.disabled { DISABLED_INK } else { self.intent.index() };
        self.draw_bg.opacity = if self.disabled { 0.6 } else { 1.0 };
        match self.shape {
            ProgressShape::Rings => {
                let step = self.draw_bg.thickness as f64 + self.ring_gap;
                let values = numbers(&self.values);
                for (index, value) in values.iter().enumerate() {
                    let ink = if self.disabled {
                        DISABLED_INK
                    } else {
                        RING_INTENTS[index % RING_INTENTS.len()].index()
                    };
                    self.piece(cx, rect, 0.0, 1.0, value.clamp(0.0, 1.0), ink, index as f64 * step);
                }
            }
            _ => {
                let shown = self.shown;
                let sweep = self.draw_bg.sweep as f64;
                if self.sections > 1 && sweep > 0.0 && shown >= 0.0 {
                    let n = self.sections.min(64);
                    let gap = (self.section_gap_deg.max(0.0).to_radians() / sweep).min(0.5 / n as f64);
                    for i in 0..n {
                        let mut s = i as f64 / n as f64;
                        let mut e = (i + 1) as f64 / n as f64;
                        if i > 0 {
                            s += gap * 0.5;
                        }
                        if i + 1 < n {
                            e -= gap * 0.5;
                        }
                        let v = shown.clamp(s, e);
                        self.piece(cx, rect, s, e, v, intent, 0.0);
                    }
                } else {
                    self.piece(cx, rect, 0.0, 1.0, shown, intent, 0.0);
                }
            }
        }
        let label = self.label_text();
        if !label.is_empty() {
            let align = match self.shape {
                ProgressShape::Arc => Align { x: 0.5, y: 1.0 },
                _ => Align { x: 0.5, y: 0.5 },
            };
            draw_text_in(cx, &mut self.draw_text, rect, align, &label);
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
        Some(two_decimals(match self.shape {
            ProgressShape::Rings => numbers(&self.values).first().copied().unwrap_or(0.0),
            _ => self.value,
        }))
    }
}

impl ProgressRingRef {
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

    pub fn set_values(&self, cx: &mut Cx, values: &[f64]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_values(cx, values);
        }
    }

    pub fn set_intent(&self, cx: &mut Cx, intent: Intent) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_intent(cx, intent);
        }
    }
}

/// A read-only dial (or bar) with safe, warning and critical zones, a
/// needle at the value and the range printed at the ends.
#[derive(Script, ScriptHook, Widget)]
pub struct Gauge {
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
    draw_bg: DrawGauge,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_range: DrawText,
    /// A fixed centre label; wins over the value.
    #[live]
    pub text: String,
    /// The reading, in the units of `min`..`max`.
    #[live]
    pub value: f64,
    #[live]
    pub min: f64,
    #[live(100.0)]
    pub max: f64,
    #[live]
    pub precision: usize,
    #[live]
    pub unit: String,
    #[live(true)]
    pub show_value: bool,
    #[live(true)]
    pub show_range: bool,
    #[live]
    pub min_label: String,
    #[live]
    pub max_label: String,
    #[live]
    pub linear: bool,
    #[live]
    pub ease_secs: f64,
    #[live]
    pub disabled: bool,
    #[rust]
    shown: f64,
    #[rust]
    target: f64,
    #[rust]
    ease: Ease,
    #[rust]
    drawn: bool,
    #[rust]
    next_frame: NextFrame,
}

impl Gauge {
    fn format(&self, value: f64) -> String {
        format!("{:.*}{}", self.precision, value, self.unit)
    }

    /// The reading as the fraction of the range it covers.
    fn fraction(&self, value: f64) -> f64 {
        let span = self.max - self.min;
        if span.abs() < f64::EPSILON {
            0.0
        } else {
            ((value - self.min) / span).clamp(0.0, 1.0)
        }
    }

    pub fn label_text(&self) -> String {
        if !self.text.is_empty() {
            self.text.clone()
        } else if self.show_value {
            self.format(self.shown)
        } else {
            String::new()
        }
    }

    /// Set the reading. Unlike a bar, a gauge is allowed to fall: a reading
    /// eases in BOTH directions, because a needle that jumps down reads as
    /// a fault, not a reset.
    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        self.value = value;
        self.aim(cx.seconds_since_app_start());
        if self.ease.running {
            self.next_frame = cx.new_next_frame();
        }
        self.draw_bg.redraw(cx);
    }

    fn aim(&mut self, now: f64) {
        self.target = self.value;
        if !self.drawn || (self.value - self.shown).abs() < f64::EPSILON {
            self.shown = self.value;
            self.ease.running = false;
        } else {
            self.ease = Ease {
                from: self.shown,
                started: now,
                running: true,
            };
        }
    }

    fn range_labels(&self) -> (String, String) {
        let lo = if self.min_label.is_empty() {
            self.format(self.min)
        } else {
            self.min_label.clone()
        };
        let hi = if self.max_label.is_empty() {
            self.format(self.max)
        } else {
            self.max_label.clone()
        };
        (lo, hi)
    }
}

impl Widget for Gauge {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.drawn || self.value != self.target {
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
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.value = self.fraction(self.shown) as f32;
        self.draw_bg.opacity = if self.disabled { 0.6 } else { 1.0 };
        self.draw_bg.draw_abs(cx, rect);
        let th = self.draw_bg.thickness as f64;
        let label = self.label_text();
        let (lo, hi) = self.range_labels();
        let range_h = 14.0;
        if self.linear {
            // The value rides above the bar, the range hangs under its ends.
            let bar_top = rect.pos.y + (rect.size.y - th) * 0.5;
            if !label.is_empty() {
                draw_text_in(
                    cx,
                    &mut self.draw_text,
                    Rect { pos: rect.pos, size: dvec2(rect.size.x, (bar_top - rect.pos.y).max(1.0)) },
                    Align { x: 0.5, y: 1.0 },
                    &label,
                );
            }
            if self.show_range {
                let y = bar_top + th + 1.0;
                let h = (rect.pos.y + rect.size.y - y).max(1.0);
                draw_text_in(
                    cx,
                    &mut self.draw_range,
                    Rect { pos: dvec2(rect.pos.x, y), size: dvec2(rect.size.x * 0.5, h) },
                    Align { x: 0.0, y: 0.0 },
                    &lo,
                );
                draw_text_in(
                    cx,
                    &mut self.draw_range,
                    Rect { pos: dvec2(rect.pos.x + rect.size.x * 0.5, y), size: dvec2(rect.size.x * 0.5, h) },
                    Align { x: 1.0, y: 0.0 },
                    &hi,
                );
            }
        } else {
            let center = rect.pos + rect.size * 0.5;
            let radius = rect.size.x.min(rect.size.y) * 0.5 - th * 0.5;
            let sweep = self.draw_bg.sweep as f64;
            if !label.is_empty() {
                // Under the hub, inside the opening.
                let top = center.y + 8.0;
                draw_text_in(
                    cx,
                    &mut self.draw_text,
                    Rect { pos: dvec2(rect.pos.x, top), size: dvec2(rect.size.x, (rect.pos.y + rect.size.y - top).max(1.0)) },
                    Align { x: 0.5, y: 0.0 },
                    &label,
                );
            }
            if self.show_range {
                let phi0 = 1.5 * std::f64::consts::PI - sweep * 0.5;
                let phi1 = phi0 + sweep;
                let w = 48.0;
                for (phi, text) in [(phi0, lo), (phi1, hi)] {
                    let cap = center + dvec2(phi.cos(), phi.sin()) * radius;
                    draw_text_in(
                        cx,
                        &mut self.draw_range,
                        Rect { pos: dvec2(cap.x - w * 0.5, cap.y + th * 0.5 + 2.0), size: dvec2(w, range_h) },
                        Align { x: 0.5, y: 0.0 },
                        &text,
                    );
                }
            }
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

impl GaugeRef {
    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value).unwrap_or(0.0)
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }

    pub fn set_range(&self, cx: &mut Cx, min: f64, max: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.min = min;
            inner.max = max;
            inner.draw_bg.redraw(cx);
        }
    }
}

/// Where the navigation line is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum NavPhase {
    /// Off screen. Nothing draws (the quad is there at zero opacity so the
    /// area stays valid for the redraw that `start` asks for).
    #[default]
    Idle,
    /// On screen, creeping toward the ceiling.
    Trickling,
    /// `complete` was called: easing to the end.
    Completing,
    /// At the end, waiting out `fade_delay_secs`.
    Holding,
    /// Fading out over `ease_secs`, then Idle.
    Fading,
}

/// A hairline along the top of a page that starts on demand, trickles
/// toward the end while a load is in flight and fades once it is done.
#[derive(Script, ScriptHook, Widget)]
pub struct NavigationProgress {
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
    #[live(Intent::Primary)]
    pub intent: Intent,
    #[live]
    pub trickle_rate: f64,
    #[live]
    pub trickle_cap: f64,
    #[live]
    pub fade_delay_secs: f64,
    #[live]
    pub ease_secs: f64,
    #[rust]
    phase: NavPhase,
    /// Where the line is being pulled toward.
    #[rust]
    target: f64,
    /// Where the line is drawn.
    #[rust]
    shown: f64,
    #[rust]
    opacity: f64,
    #[rust]
    phase_since: f64,
    #[rust]
    last_tick: f64,
    #[rust]
    next_frame: NextFrame,
}

impl NavigationProgress {
    /// Put the line on screen at a first small step and let it trickle. A
    /// line already running keeps its place.
    pub fn start(&mut self, cx: &mut Cx) {
        let now = cx.seconds_since_app_start();
        match self.phase {
            NavPhase::Idle | NavPhase::Holding | NavPhase::Fading => {
                self.target = 0.08;
                self.shown = 0.0;
                self.opacity = 1.0;
            }
            NavPhase::Completing => {
                self.target = self.shown.min(self.trickle_cap);
            }
            NavPhase::Trickling => return,
        }
        self.phase = NavPhase::Trickling;
        self.phase_since = now;
        self.last_tick = now;
        self.next_frame = cx.new_next_frame();
        self.draw_bg.redraw(cx);
    }

    /// Nudge the target forward, never past the ceiling; starts the line if
    /// it was idle.
    pub fn increment(&mut self, cx: &mut Cx, amount: f64) {
        if self.phase != NavPhase::Trickling {
            self.start(cx);
        }
        self.target = (self.target + amount.max(0.0)).min(self.trickle_cap);
        self.draw_bg.redraw(cx);
    }

    /// Pin the target, never past the ceiling; starts the line if idle.
    pub fn set_progress(&mut self, cx: &mut Cx, value: f64) {
        if self.phase != NavPhase::Trickling {
            self.start(cx);
        }
        self.target = value.clamp(self.target, self.trickle_cap);
        self.draw_bg.redraw(cx);
    }

    /// Send the line to the end; it holds, then fades. A line that was
    /// never started completes invisibly (no flash for a load that was
    /// instant).
    pub fn complete(&mut self, cx: &mut Cx) {
        if self.phase == NavPhase::Idle {
            return;
        }
        let now = cx.seconds_since_app_start();
        self.target = 1.0;
        self.phase = NavPhase::Completing;
        self.phase_since = now;
        self.last_tick = now;
        self.next_frame = cx.new_next_frame();
        self.draw_bg.redraw(cx);
    }

    /// Take the line off screen at once.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.phase = NavPhase::Idle;
        self.target = 0.0;
        self.shown = 0.0;
        self.opacity = 0.0;
        self.draw_bg.redraw(cx);
    }

    pub fn is_active(&self) -> bool {
        self.phase != NavPhase::Idle
    }

    /// The shown fraction, 0..1, or `None` while the line is off screen.
    pub fn progress(&self) -> Option<f64> {
        (self.phase != NavPhase::Idle).then_some(self.shown)
    }

    /// One frame of the life cycle. Returns whether another is wanted.
    fn tick(&mut self, cx: &mut Cx, now: f64) -> bool {
        let dt = (now - self.last_tick).clamp(0.0, 0.25);
        self.last_tick = now;
        let ease = self.ease_secs.max(0.001);
        // The shown value chases the target exponentially, so the same
        // constant serves a step and the trickle alike.
        let chase = 1.0 - (-dt / (ease * 0.5)).exp();
        match self.phase {
            NavPhase::Idle => return false,
            NavPhase::Trickling => {
                let room = self.trickle_cap - self.target;
                self.target += room * (1.0 - (-self.trickle_rate * dt).exp());
                self.shown += (self.target - self.shown) * chase;
            }
            NavPhase::Completing => {
                self.shown += (1.0 - self.shown) * chase;
                if self.shown > 0.998 {
                    self.shown = 1.0;
                    self.phase = NavPhase::Holding;
                    self.phase_since = now;
                    let uid = self.widget_uid();
                    cx.widget_action(uid, ProgressAction::Completed);
                }
            }
            NavPhase::Holding => {
                if now - self.phase_since >= self.fade_delay_secs {
                    self.phase = NavPhase::Fading;
                    self.phase_since = now;
                }
            }
            NavPhase::Fading => {
                self.opacity = 1.0 - ((now - self.phase_since) / ease).clamp(0.0, 1.0);
                if self.opacity <= 0.0 {
                    self.reset(cx);
                    return false;
                }
            }
        }
        true
    }
}

impl Widget for NavigationProgress {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.start = 0.0;
        self.draw_bg.end = 1.0;
        self.draw_bg.track = 0.0;
        self.draw_bg.intent = self.intent.index();
        self.draw_bg.value = if self.phase == NavPhase::Idle { 0.0 } else { self.shown as f32 };
        self.draw_bg.opacity = if self.phase == NavPhase::Idle { 0.0 } else { self.opacity as f32 };
        self.draw_bg.draw_abs(cx, rect);
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(ne) = self.next_frame.is_event(event) {
            if self.tick(cx, ne.time) {
                self.next_frame = cx.new_next_frame();
            }
            self.draw_bg.redraw(cx);
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(two_decimals(self.progress().unwrap_or(0.0)))
    }
}

impl NavigationProgressRef {
    pub fn completed(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast()),
            Some(ProgressAction::Completed)
        )
    }

    pub fn start(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.start(cx);
        }
    }

    pub fn increment(&self, cx: &mut Cx, amount: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.increment(cx, amount);
        }
    }

    pub fn set_progress(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_progress(cx, value);
        }
    }

    pub fn complete(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.complete(cx);
        }
    }

    pub fn reset(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset(cx);
        }
    }

    pub fn is_active(&self) -> bool {
        self.borrow().map(|inner| inner.is_active()).unwrap_or(false)
    }

    pub fn progress(&self) -> Option<f64> {
        self.borrow().and_then(|inner| inner.progress())
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

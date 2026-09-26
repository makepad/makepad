//! The colour picker family: one colour, and the several ways a person
//! reaches for it.
//!
//! A swatch that shows a colour and can be pressed; a hue ring; a
//! saturation/value square; an alpha strip; a strip of palette cells; a text
//! field that takes a colour written down and repairs itself when you leave
//! it; and a picker panel that puts them together, either standing inline on
//! a page or hanging off a swatch that opens it.
//!
//! The geometry is not new. The ring's radii, the square's mapping and the
//! two-tone pucks are carried over from the node editor's picker, constants
//! and all, so hit testing and pixels cannot disagree. What changed is the
//! colours the controls are DRAWN in: every one of them reads `theme.*`. The
//! table they used to read was a single fixed dark grade with no light and no
//! skeleton variant, so those controls stayed dark on a light page.
//!
//! **A number row is a track.** The six channel rows under the wheel are
//! filled bars, and a filled bar the width of a panel column reads as a
//! slider: a press lands the channel where it fell and the drag keeps it
//! there. The hex line below them is the one row that is still a text field,
//! because a colour written down is typed, not aimed at.
//!
//! **Hue is the state, not red-green-blue.** Every control here keeps an
//! [`Hsva`] and derives the RGBA from it. Round-tripping through RGBA loses
//! the hue of a grey and the hue of black, and a picker that forgets which
//! way the ring was pointing the moment the value reaches zero is the classic
//! way this widget goes wrong. `color` is the property a caller reads and
//! writes; the HSVA behind it survives the trip.
//!
//! **What it is not.** There is no eyedropper: nothing here can read a screen
//! pixel, and an OS capture path is a host's business, not a widget's. There
//! is no colour space beyond sRGB and HSV — no Lab, no OKLCH, no gamut
//! mapping. There is no gradient or ramp editor. And a palette strip binds to
//! nothing: it reports the cell that was picked and the colour in it, and
//! what that cell MEANS — a theme token, a layer's fill, a tag — is known
//! only to the host that filled the strip.

use crate::{
    makepad_derive_widget::*, makepad_draw::*, text_input::*, value_input::*, widget::*,
    CxWidgetExt,
};
use std::sync::{Mutex, OnceLock};

// ===========================================================================
// The pure core: colour conversion, the notations, the wheel geometry
// ===========================================================================

/// A colour as hue, saturation, value and alpha, every channel 0..1 except
/// `h`, which wraps.
///
/// This, and not RGBA, is what the controls hold: RGBA cannot say which way
/// the hue ring points once the colour is grey or black, so a picker that
/// stored RGBA would swing its ring back to red the moment the value or the
/// saturation reached zero.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Hsva {
    pub h: f32,
    pub s: f32,
    pub v: f32,
    pub a: f32,
}

impl Hsva {
    pub const fn new(h: f32, s: f32, v: f32, a: f32) -> Self {
        Self { h, s, v, a }
    }

    /// Opaque white — the colour a control starts on before anything has set
    /// one, chosen because it shows the ring and the square at full strength.
    pub const fn white() -> Self {
        Self::new(0.0, 0.0, 1.0, 1.0)
    }

    pub fn from_rgba(rgba: [f32; 4]) -> Self {
        let (r, g, b) = (rgba[0], rgba[1], rgba[2]);
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
        Self { h, s, v, a: rgba[3] }
    }

    pub fn to_rgba(self) -> [f32; 4] {
        let h = self.h.rem_euclid(1.0) * 6.0;
        let i = h.floor();
        let f = h - i;
        let s = self.s.clamp(0.0, 1.0);
        let v = self.v.clamp(0.0, 1.0);
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        let [r, g, b] = match i as i32 % 6 {
            0 => [v, t, p],
            1 => [q, v, p],
            2 => [p, v, t],
            3 => [p, q, v],
            4 => [t, p, v],
            _ => [v, p, q],
        };
        [r, g, b, self.a]
    }

    pub fn from_vec4(v: Vec4f) -> Self {
        Self::from_rgba([v.x, v.y, v.z, v.w])
    }

    pub fn to_vec4(self) -> Vec4f {
        let c = self.to_rgba();
        vec4(c[0], c[1], c[2], c[3])
    }

    /// The colour at full alpha — what the alpha strip and the checkerboard
    /// need to draw the gradient the alpha is being chosen along.
    pub fn opaque_vec4(self) -> Vec4f {
        let c = Self { a: 1.0, ..self }.to_rgba();
        vec4(c[0], c[1], c[2], 1.0)
    }
}

/// How a colour is written down in a text field.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ColorNotation {
    /// `#rrggbb`, or `#rrggbbaa` when alpha is shown.
    #[pick]
    Hex = 0,
    /// `rgb(255, 128, 0)`, or `rgba(255, 128, 0, 0.50)`.
    Rgb = 1,
}

/// Parse `#rgb`, `#rrggbb` or `#rrggbbaa`, with or without the hash. The
/// second value says whether the text carried an alpha of its own, so a
/// caller can leave the alpha it already has alone when it did not.
pub fn parse_hex_color(text: &str) -> Option<([f32; 4], bool)> {
    let t = text.trim().trim_start_matches('#');
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let nib = |c: u8| ((c as char).to_digit(16).unwrap_or(0) as f32) / 15.0;
    let byte = |hi: u8, lo: u8| {
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

/// Parse `rgb(255, 128, 0)` / `rgba(255, 128, 0, 0.5)`, or the bare numbers
/// with the wrapper left off. The three colour channels are 0..255; the
/// alpha is a FRACTION 0..1 rather than a byte, so there is no guessing over
/// whether `1` means opaque or one part in 255.
pub fn parse_rgb_color(text: &str) -> Option<([f32; 4], bool)> {
    let t = text.trim().trim_start_matches("rgba").trim_start_matches("rgb");
    let t = t.trim().trim_start_matches('(').trim_end_matches(')');
    let mut parts = Vec::new();
    for piece in t.split(|c: char| c == ',' || c.is_whitespace()) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        parts.push(piece.parse::<f64>().ok()?);
    }
    let channel = |v: f64| (v / 255.0).clamp(0.0, 1.0) as f32;
    match parts.len() {
        3 => Some((
            [channel(parts[0]), channel(parts[1]), channel(parts[2]), 1.0],
            false,
        )),
        4 => Some((
            [
                channel(parts[0]),
                channel(parts[1]),
                channel(parts[2]),
                parts[3].clamp(0.0, 1.0) as f32,
            ],
            true,
        )),
        _ => None,
    }
}

/// A colour written either way. A hash, or nothing but hex digits, means
/// hex; a comma or an `rgb` prefix means the numeric form. Anything else is
/// refused, and refusing is the whole point: the field that calls this puts
/// back the colour it already had rather than inventing one.
pub fn parse_color(text: &str) -> Option<([f32; 4], bool)> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("rgb") || t.contains(',') {
        return parse_rgb_color(t);
    }
    parse_hex_color(t)
}

fn byte_of(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u32
}

/// `#rrggbb`, or `#rrggbbaa` when `with_alpha`. Lower case: it is the form
/// the rest of this repository writes, and a field that reformats what you
/// typed should at least be consistent about it.
pub fn format_color_hex(rgba: [f32; 4], with_alpha: bool) -> String {
    if with_alpha {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            byte_of(rgba[0]),
            byte_of(rgba[1]),
            byte_of(rgba[2]),
            byte_of(rgba[3])
        )
    } else {
        format!(
            "#{:02x}{:02x}{:02x}",
            byte_of(rgba[0]),
            byte_of(rgba[1]),
            byte_of(rgba[2])
        )
    }
}

/// `rgb(255, 128, 0)`, or `rgba(255, 128, 0, 0.50)` when `with_alpha`.
pub fn format_color_rgb(rgba: [f32; 4], with_alpha: bool) -> String {
    if with_alpha {
        format!(
            "rgba({}, {}, {}, {:.2})",
            byte_of(rgba[0]),
            byte_of(rgba[1]),
            byte_of(rgba[2]),
            rgba[3].clamp(0.0, 1.0)
        )
    } else {
        format!(
            "rgb({}, {}, {})",
            byte_of(rgba[0]),
            byte_of(rgba[1]),
            byte_of(rgba[2])
        )
    }
}

/// A colour in the notation asked for.
pub fn format_color(rgba: [f32; 4], notation: ColorNotation, with_alpha: bool) -> String {
    match notation {
        ColorNotation::Hex => format_color_hex(rgba, with_alpha),
        ColorNotation::Rgb => format_color_rgb(rgba, with_alpha),
    }
}

/// A palette written as one string: hex colours separated by whitespace or
/// commas. Anything that does not parse is dropped rather than refused, so
/// one typo costs one cell instead of the whole strip.
///
/// Hex only, deliberately: the numeric notation uses commas itself, and a
/// list format that could not be split on a comma would be worse than a
/// list format that only takes one spelling.
pub fn parse_color_list(text: &str) -> Vec<[f32; 4]> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|piece| !piece.trim().is_empty())
        .filter_map(|piece| parse_hex_color(piece).map(|(rgba, _)| rgba))
        .collect()
}

/// Outer radius of the hue ring, as a fraction of the control's smaller
/// side. The shader uses the same number, so a pointer lands where the
/// pixels say it will.
pub const RING_OUTER_FRACTION: f64 = 0.48;
/// Inner radius of the hue ring, same units.
pub const RING_INNER_FRACTION: f64 = 0.385;
/// Half the side of the saturation/value square that sits in the ring's
/// hole, same units. `RING_INNER_FRACTION / sqrt(2)` would touch the ring;
/// this leaves a little air.
pub const SQUARE_HALF_FRACTION: f64 = 0.255;
/// How far outside the drawn band a press still counts as the ring. A ring
/// eight points wide is hard to hit exactly, and a press that misses by a
/// point should move the hue rather than do nothing.
pub const RING_SLOP: f64 = 4.0;

/// Whether a pointer at `rel` (control-local, origin top left) is on the hue
/// ring of a control whose smaller side is `size`.
pub fn ring_contains(rel: DVec2, size: f64) -> bool {
    let dx = rel.x - size * 0.5;
    let dy = rel.y - size * 0.5;
    let r = (dx * dx + dy * dy).sqrt();
    r <= RING_OUTER_FRACTION * size + RING_SLOP && r >= RING_INNER_FRACTION * size - RING_SLOP
}

/// The hue (0..1) a pointer on the ring is asking for: zero at twelve
/// o'clock, rising clockwise, so red is at the top.
pub fn ring_hue_at(rel: DVec2, size: f64) -> f32 {
    let dx = rel.x - size * 0.5;
    let dy = rel.y - size * 0.5;
    let ang = dx.atan2(-dy);
    ((ang / std::f64::consts::TAU).rem_euclid(1.0)) as f32
}

/// The (saturation, value) a pointer over a square of `size` is asking for.
/// Clamped, so a drag that leaves the square keeps tracking the nearest
/// edge instead of stopping dead.
pub fn area_sv_at(rel: DVec2, size: DVec2) -> (f32, f32) {
    let s = if size.x > 0.0 {
        (rel.x / size.x).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let v = if size.y > 0.0 {
        1.0 - (rel.y / size.y).clamp(0.0, 1.0)
    } else {
        1.0
    };
    (s as f32, v as f32)
}

/// The alpha a pointer along a strip `width` wide is asking for.
pub fn alpha_at(rel_x: f64, width: f64) -> f32 {
    if width <= 0.0 {
        return 1.0;
    }
    (rel_x / width).clamp(0.0, 1.0) as f32
}

/// Where the saturation/value square goes inside a ring's rect. The picker
/// places the square itself rather than nesting it, so the two remain
/// separate widgets that a host can use one without the other.
pub fn square_in_ring(rect: Rect) -> Rect {
    let size = rect.size.x.min(rect.size.y);
    let half = SQUARE_HALF_FRACTION * size;
    let cx = rect.pos.x + rect.size.x * 0.5;
    let cy = rect.pos.y + rect.size.y * 0.5;
    Rect {
        pos: dvec2(cx - half, cy - half),
        size: dvec2(half * 2.0, half * 2.0),
    }
}

/// The narrowest a panel or a row is allowed to get. Below this the fixed
/// widths computed from it go negative and the children are laid out and
/// never painted.
pub const MIN_INNER_WIDTH: f64 = 40.0;

/// The room inside a panel: what the parent offered the walk, less the
/// padding. A `Fit` parent measures NaN, and then `fallback` is all there is
/// to go on.
pub fn inner_width(outer: f64, padding: f64, fallback: f64) -> f64 {
    let room = if outer.is_finite() && outer > 0.0 {
        outer - padding
    } else {
        fallback - padding
    };
    room.max(MIN_INNER_WIDTH)
}

/// What is left of a row for the text field once the swatch and the gap
/// beside it have taken their share.
pub fn field_input_width(inner: f64, swatch: f64, spacing: f64) -> f64 {
    (inner - swatch.max(0.0) - spacing).max(MIN_INNER_WIDTH)
}

/// How many recent colours are kept. Eight is two rows of four in a picker
/// this width, and more than that is a palette, which is a different thing
/// with a different widget.
pub const RECENT_MAX: usize = 8;

/// Put a colour at the head of a recent list, moving it rather than
/// duplicating it if it is already there. Two colours count as the same when
/// every channel agrees to within half a step of 255, because a colour that
/// came back through a hex field is not bit-identical to the one that went
/// in and a list of visually identical swatches helps nobody.
pub fn push_recent_into(list: &mut Vec<[f32; 4]>, rgba: [f32; 4]) {
    list.retain(|c| {
        c.iter()
            .zip(rgba.iter())
            .any(|(a, b)| (a - b).abs() > 1.0 / 512.0)
    });
    list.insert(0, rgba);
    list.truncate(RECENT_MAX);
}

fn recent_store() -> &'static Mutex<Vec<[f32; 4]>> {
    static STORE: OnceLock<Mutex<Vec<[f32; 4]>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Remember a committed colour for the rest of the session. Process-wide on
/// purpose: a person who mixes a colour in one panel expects to find it in
/// the next one, and threading a store through every host to get that would
/// be a lot of plumbing for a list of eight.
pub fn remember_color(rgba: [f32; 4]) {
    if let Ok(mut list) = recent_store().lock() {
        push_recent_into(&mut list, rgba);
    }
}

/// The recent colours, newest first.
pub fn recent_colors() -> Vec<[f32; 4]> {
    recent_store().lock().map(|l| l.clone()).unwrap_or_default()
}

// ===========================================================================
// One action for the whole family
// ===========================================================================

/// What every control here reports. One enum rather than six: they all speak
/// about the same thing, and a host that swaps a wheel for a field should
/// not have to rewrite its match arm.
#[derive(Clone, Debug, Default)]
pub enum ColorAction {
    /// The colour is moving under the hand. Follow this; do not write it
    /// down.
    Changed(Vec4f),
    /// The gesture finished, or a typed value was committed. This is the one
    /// to record.
    Ended(Vec4f),
    /// A swatch was pressed.
    Pressed(Vec4f),
    /// A palette cell was chosen: which one, and the colour in it.
    Picked(usize, Vec4f),
    /// The pointer rests on a palette cell, or has left the strip.
    Hovered(Option<usize>),
    /// A picker's panel opened.
    Opened,
    /// A picker's panel closed.
    Closed,
    #[default]
    None,
}

fn report(cx: &mut Cx, uid: WidgetUid, color: Vec4f, ended: bool) {
    cx.widget_action(uid, ColorAction::Changed(color));
    if ended {
        cx.widget_action(uid, ColorAction::Ended(color));
    }
}

/// The `Changed` colour in a batch of actions, if this widget raised one.
fn changed_in(actions: &Actions, uid: WidgetUid) -> Option<Vec4f> {
    for action in actions.filter_widget_actions_cast::<ColorAction>(uid) {
        if let ColorAction::Changed(v) = action {
            return Some(v);
        }
    }
    None
}

/// The `Ended` colour in a batch of actions, if this widget raised one.
fn ended_in(actions: &Actions, uid: WidgetUid) -> Option<Vec4f> {
    for action in actions.filter_widget_actions_cast::<ColorAction>(uid) {
        if let ColorAction::Ended(v) = action {
            return Some(v);
        }
    }
    None
}

// ===========================================================================
// DSL
// ===========================================================================

script_mod! {
    use mod.prelude.widgets_internal.*

    // Declared before the `use` below, because a block's `use` only sees
    // what already exists when it runs.
    let ColorNotation = set_type_default() do #(ColorNotation::script_api(vm))
    mod.widgets.ColorNotation = ColorNotation

    use mod.widgets.*

    // The two-tone puck (`puck_dark` over `puck_light`, below) is the one
    // thing here that does NOT read the theme, and cannot: it sits on the
    // colour being chosen, not on a surface, so it has to stay legible over
    // red, over white and over black. A dark outline with a light ring
    // inside it is the shape that manages that.

    // ---- the swatch -------------------------------------------------------

    mod.widgets.DrawColorSwatchBase = #(DrawColorSwatch::script_component(vm))
    set_type_default() do #(DrawColorSwatch::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ColorSwatchBase = #(ColorSwatch::register_widget(vm))

    /** A colour as a small block, pressable.
     *
     * The checker under the fill is not decoration: without it a colour at
     * half alpha and the same colour at full alpha look identical on a flat
     * background, and the whole reason to show alpha is that they are not. */
    mod.widgets.ColorSwatch = set_type_default() do mod.widgets.ColorSwatchBase{
        width: 24
        height: 24
        /** the colour the block shows */
        color: #x808080FF
        /** drawn with the chosen ring */
        selected: false
        // Every value below is plain, and every one has a Rust field behind
        // it. instance() here hands an f32 an object: the property never
        // binds and the widget draws with whatever was in the slot.
        draw_bg +: {
            hover: 0.0
            down: 0.0
            chosen: 0.0
            /** border thickness in pixels 0..4 step 0.5 */
            border_size: theme.size_border
            /** corner rounding 0..24 step 0.5 */
            border_radius: theme.radius_xs
            /** the side of one square of the transparency checker 2..16 step 1 */
            checker_size: 5.0
            border_color: theme.color_outline_variant
            border_color_hover: theme.color_outline
            border_color_chosen: theme.color_primary
            checker_light: theme.color_surface_container_highest
            checker_dark: theme.color_surface_container_lowest
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let b = self.border_size
                sdf.box(b * 0.5, b * 0.5, self.rect_size.x - b, self.rect_size.y - b, self.border_radius)
                let gx = floor(self.pos.x * self.rect_size.x / self.checker_size)
                let gy = floor(self.pos.y * self.rect_size.y / self.checker_size)
                let odd = modf(gx + gy, 2.0)
                let back = self.checker_light.xyz.mix(self.checker_dark.xyz, odd)
                let lift = self.hover * 0.05 - self.down * 0.05
                let rgb = back.mix(self.swatch.xyz, self.swatch.w) + vec3(lift, lift, lift)
                sdf.fill_keep(vec4(rgb, 1.0))
                let ring = self.border_color.mix(self.border_color_hover, self.hover).mix(self.border_color_chosen, self.chosen)
                sdf.stroke(ring, self.border_size)
                return sdf.result
            }
        }
    }

    /** The swatch a picker hangs off: wider than tall, the way a colour row
     * in a property panel is drawn. */
    mod.widgets.ColorSwatchWide = mod.widgets.ColorSwatch{
        width: 46
        height: 18
    }

    // ---- the hue ring -----------------------------------------------------

    mod.widgets.DrawHueRingBase = #(DrawHueRing::script_component(vm))
    set_type_default() do #(DrawHueRing::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ColorWheelBase = #(ColorWheel::register_widget(vm))

    /** The hue ring: a full turn of hue, zero at twelve o'clock and rising
     * clockwise. The hole in the middle is left empty; a picker puts the
     * saturation/value square in it, and a host using the ring on its own
     * gets an annulus with nothing inside, which is what a hue control is. */
    mod.widgets.ColorWheel = set_type_default() do mod.widgets.ColorWheelBase{
        width: 200
        height: 200
        /** the colour the ring points at */
        color: #xFF0000FF
        draw_bg +: {
            hue: 0.0
            puck_dark: #x0A0A0AE6
            puck_light: #xFFFFFFF2
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let size = min(self.rect_size.x, self.rect_size.y)
                let c = self.rect_size * 0.5
                let dx = self.pos.x * self.rect_size.x - c.x
                let dy = self.pos.y * self.rect_size.y - c.y
                let outer = size * 0.48
                let inner = size * 0.385
                sdf.circle(c.x, c.y, outer)
                sdf.circle(c.x, c.y, inner)
                sdf.subtract()
                let ang = atan2(dx, 0.0 - dy)
                let hue_at = fract(ang / 6.2831853 + 1.0)
                sdf.fill(Pal.hsv2rgb(vec4(hue_at, 1.0, 1.0, 1.0)))
                let mid = (outer + inner) * 0.5
                let pa = self.hue * 6.2831853
                let rp = vec2(c.x + sin(pa) * mid, c.y - cos(pa) * mid)
                sdf.circle(rp.x, rp.y, 6.5)
                sdf.stroke(self.puck_dark, 1.4)
                sdf.circle(rp.x, rp.y, 5.0)
                sdf.stroke(self.puck_light, 1.6)
                return sdf.result
            }
        }
    }

    // ---- the saturation / value square ------------------------------------

    mod.widgets.DrawColorAreaBase = #(DrawColorArea::script_component(vm))
    set_type_default() do #(DrawColorArea::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ColorAreaBase = #(ColorArea::register_widget(vm))

    /** The saturation/value square at one hue: saturation runs left to
     * right, value bottom to top, so the top right corner is the pure hue
     * and the whole left edge is grey. */
    mod.widgets.ColorArea = set_type_default() do mod.widgets.ColorAreaBase{
        width: 120
        height: 120
        /** the colour the puck sits on */
        color: #xFF0000FF
        draw_bg +: {
            hue: 0.0
            sat: 1.0
            val: 1.0
            /** corner rounding 0..24 step 0.5 */
            border_radius: theme.radius_xs
            puck_dark: #x0A0A0AE6
            puck_light: #xFFFFFFF2
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, self.border_radius)
                let s = clamp(self.pos.x, 0.0, 1.0)
                let v = 1.0 - clamp(self.pos.y, 0.0, 1.0)
                sdf.fill(Pal.hsv2rgb(vec4(self.hue, s, v, 1.0)))
                let sp = vec2(self.sat * w, (1.0 - self.val) * h)
                sdf.circle(sp.x, sp.y, 6.0)
                sdf.stroke(self.puck_dark, 1.4)
                sdf.circle(sp.x, sp.y, 4.5)
                sdf.stroke(self.puck_light, 1.6)
                return sdf.result
            }
        }
    }

    // ---- the alpha strip --------------------------------------------------

    mod.widgets.DrawAlphaStripBase = #(DrawAlphaStrip::script_component(vm))
    set_type_default() do #(DrawAlphaStrip::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ColorAlphaBase = #(ColorAlpha::register_widget(vm))

    /** The alpha strip: the current colour laid over a checker, transparent
     * at the left and opaque at the right. The gradient is the control —
     * there is nothing to read off a number that this does not show. */
    mod.widgets.ColorAlpha = set_type_default() do mod.widgets.ColorAlphaBase{
        width: Fill
        height: 16
        /** the colour whose alpha is being chosen */
        color: #xFF0000FF
        draw_bg +: {
            alpha: 1.0
            /** border thickness in pixels 0..4 step 0.5 */
            border_size: theme.size_border
            /** corner rounding 0..24 step 0.5 */
            border_radius: theme.radius_xs
            /** the side of one square of the transparency checker 2..16 step 1 */
            checker_size: 5.0
            border_color: theme.color_outline_variant
            checker_light: theme.color_surface_container_highest
            checker_dark: theme.color_surface_container_lowest
            puck_dark: #x0A0A0AE6
            puck_light: #xFFFFFFF2
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, self.border_radius)
                let gx = floor(self.pos.x * w / self.checker_size)
                let gy = floor(self.pos.y * h / self.checker_size)
                let odd = modf(gx + gy, 2.0)
                let back = self.checker_light.xyz.mix(self.checker_dark.xyz, odd)
                let rgb = back.mix(self.swatch.xyz, clamp(self.pos.x, 0.0, 1.0))
                sdf.fill_keep(vec4(rgb, 1.0))
                sdf.stroke(self.border_color, self.border_size)
                // The handle is a bar rather than a circle: the strip is
                // shorter than a puck is wide, and a circle would spill out
                // of it at both edges.
                let hx = self.alpha * w
                sdf.box(hx - 2.5, -1.0, 5.0, h + 2.0, 2.0)
                sdf.stroke(self.puck_dark, 1.4)
                sdf.box(hx - 1.5, 0.5, 3.0, h - 1.0, 1.5)
                sdf.stroke(self.puck_light, 1.6)
                return sdf.result
            }
        }
    }

    // ---- the palette strip ------------------------------------------------

    mod.widgets.DrawPaletteCellBase = #(DrawPaletteCell::script_component(vm))
    set_type_default() do #(DrawPaletteCell::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.PaletteStripBase = #(PaletteStrip::register_widget(vm))

    /** A wrapped grid of small colour cells in one draw call, hit-tested by
     * rect arithmetic. The host fills it — a theme palette, the colours
     * already used in a document, the last few mixed by hand — and gets back
     * the index of the cell under the pointer and the index of the one that
     * was clicked. */
    mod.widgets.PaletteStrip = set_type_default() do mod.widgets.PaletteStripBase{
        width: Fill
        height: Fit
        /** the cells, as hex colours separated by spaces or commas */
        colors: ""
        /** the cell equal to this hex colour is ringed; empty rings none */
        selected: ""
        /** the side of one cell in points 6..48 step 1 */
        cell_size: 14.0
        /** the gap between cells 0..12 step 1 */
        gap: 3.0
        draw_bg +: {
            cell: vec4(0.0, 0.0, 0.0, 1.0)
            hot: 0.0
            chosen: 0.0
            /** border thickness in pixels 0..4 step 0.5 */
            border_size: theme.size_border
            /** corner rounding 0..24 step 0.5 */
            border_radius: theme.radius_xs
            /** the side of one square of the transparency checker 2..16 step 1 */
            checker_size: 4.0
            border_color: theme.color_outline_variant
            border_color_hover: theme.color_outline
            border_color_chosen: theme.color_primary
            checker_light: theme.color_surface_container_highest
            checker_dark: theme.color_surface_container_lowest
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let b = self.border_size
                sdf.box(b * 0.5, b * 0.5, self.rect_size.x - b, self.rect_size.y - b, self.border_radius)
                let gx = floor(self.pos.x * self.rect_size.x / self.checker_size)
                let gy = floor(self.pos.y * self.rect_size.y / self.checker_size)
                let odd = modf(gx + gy, 2.0)
                let back = self.checker_light.xyz.mix(self.checker_dark.xyz, odd)
                let rgb = back.mix(self.cell.xyz, self.cell.w)
                sdf.fill_keep(vec4(rgb, 1.0))
                let ring = self.border_color.mix(self.border_color_hover, self.hot).mix(self.border_color_chosen, self.chosen)
                sdf.stroke(ring, self.border_size)
                return sdf.result
            }
        }
    }

    // ---- the shapes the picker's rows are made of -------------------------

    let ChannelLabel = Label{
        width: 12
        height: Fit
        padding: 0.
        draw_text +: {
            color: theme.color_on_surface_variant
            text_style: theme.font_regular{font_size: theme.type_label_s_size}
        }
    }

    let ChannelRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_1
        align: Align{y: 0.5}
    }

    // A channel row is a filled bar the width of a third of the panel, and a
    // bar that wide reads as a slider whatever it is made of. `track` makes
    // it behave the way it reads: the press lands the channel where it fell,
    // and the drag keeps it under the pointer. As a scrub these rows looked
    // movable and were not — crossing 0..255 at a step per pixel is most of
    // a screen of travel, and a press on its own did nothing at all.
    let ChannelField = mod.widgets.ValueInput{
        width: Fill
        height: 20
        precision: 0.
        track: true
    }

    // ---- the picker -------------------------------------------------------

    mod.widgets.ColorPickerBase = #(ColorPicker::register_widget(vm))

    /** Wheel, square, alpha strip, numbers and the last few colours you
     * mixed, in one panel on the page.
     *
     * The square is placed inside the ring's hole by the picker rather than
     * nested inside the ring widget, so both stay usable on their own. */
    mod.widgets.ColorPicker = set_type_default() do mod.widgets.ColorPickerBase{
        width: 244
        height: Fit
        padding: theme.mspace_2
        spacing: theme.space_2
        /** the colour being chosen */
        color: #x3B82F6FF
        /** open under a swatch instead of standing on the page */
        popover: false
        /** show the alpha strip, the A row, and alpha in the hex */
        with_alpha: true
        /** show the strip of recently committed colours */
        with_recent: true
        /** how wide the panel is when it opens under a swatch 160..480 step 4 */
        panel_width: 244.0
        /** drawn at all */
        visible: true

        // uniform(), not plain: this is a bare quad, so nothing here has a
        // Rust field behind it and every value belongs to the shader alone.
        draw_bg +: {
            /** corner rounding 0..24 step 0.5 */
            border_radius: uniform(theme.radius_l)
            /** border thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.size_border)
            color: uniform(theme.color_surface_container_high)
            border_color: uniform(theme.color_outline_variant)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let b = self.border_size
                sdf.box(b * 0.5, b * 0.5, self.rect_size.x - b, self.rect_size.y - b, self.border_radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }

        // Slots, not named children: a slot takes a VALUE, and `wheel :=`
        // here would make a child called `wheel` and leave the slot empty —
        // a panel with a hole where the ring should be.
        swatch: mod.widgets.ColorSwatchWide{}
        wheel: mod.widgets.ColorWheel{}
        square: mod.widgets.ColorArea{}
        alpha: mod.widgets.ColorAlpha{}
        recent: mod.widgets.PaletteStrip{cell_size: 14.0}
        rows: View{
            width: Fill
            height: Fit
            flow: Down
            spacing: theme.space_1
            row_rgb := ChannelRow{
                ChannelLabel{text: "R"}
                num_r := ChannelField{min: 0. max: 255. step: 1.}
                ChannelLabel{text: "G"}
                num_g := ChannelField{min: 0. max: 255. step: 1.}
                ChannelLabel{text: "B"}
                num_b := ChannelField{min: 0. max: 255. step: 1.}
            }
            row_hsv := ChannelRow{
                ChannelLabel{text: "H"}
                num_h := ChannelField{min: 0. max: 360. step: 1.}
                ChannelLabel{text: "S"}
                num_s := ChannelField{min: 0. max: 100. step: 1.}
                ChannelLabel{text: "V"}
                num_v := ChannelField{min: 0. max: 100. step: 1.}
            }
            row_hex := ChannelRow{
                ChannelLabel{width: 26 text: "Hex"}
                hex := TextInput{
                    width: Fill
                    height: 20
                    empty_text: ""
                    draw_text +: {
                        text_style: theme.font_regular{font_size: theme.type_label_s_size}
                    }
                }
            }
        }
    }

    /** The picker as a swatch that opens it. The swatch is the whole control
     * at rest; a press opens the panel over the page, a press outside it
     * commits, and Escape puts back the colour that was there when it
     * opened. */
    mod.widgets.ColorPickerButton = mod.widgets.ColorPicker{
        // The walk is the SWATCH's here, not the panel's; the padding stays
        // as it is, because it is the open panel's padding and the swatch is
        // drawn in a turtle of its own with none.
        width: Fit
        height: Fit
        popover: true
    }

    // ---- the text field ---------------------------------------------------

    mod.widgets.ColorFieldBase = #(ColorField::register_widget(vm))

    /** A colour written down, next to the colour itself.
     *
     * It accepts either notation whichever one it shows — `#f80`, `#ff8000`,
     * `#ff8000cc`, `rgb(255, 128, 0)` — and when the keyboard leaves it, it
     * rewrites what is in it: the colour it understood in the notation it
     * was asked for, or the colour it already had if it understood nothing.
     * A field that silently keeps unparseable text is a field that lies. */
    mod.widgets.ColorField = set_type_default() do mod.widgets.ColorFieldBase{
        width: Fill
        height: Fit
        spacing: theme.space_2
        align: Align{y: 0.5}
        /** the colour */
        color: #x3B82F6FF
        /** how the colour is written: ColorNotation.Hex Rgb */
        notation: ColorNotation.Hex
        /** carry alpha in the text as well as the colour */
        with_alpha: false
        /** drawn at all */
        visible: true

        swatch: mod.widgets.ColorSwatch{width: 22 height: 22}
        input: TextInput{
            width: Fill
            height: Fit
            empty_text: "#000000"
        }
    }
}

// ===========================================================================
// The swatch
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawColorSwatch {
    #[deref]
    draw_super: DrawQuad,
    /// The colour, pushed from Rust every draw.
    #[live]
    pub swatch: Vec4f,
    #[live]
    pub hover: f32,
    #[live]
    pub down: f32,
    #[live]
    pub chosen: f32,
    #[live]
    pub border_size: f32,
    #[live]
    pub border_radius: f32,
    #[live]
    pub checker_size: f32,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub border_color_hover: Vec4f,
    #[live]
    pub border_color_chosen: Vec4f,
    #[live]
    pub checker_light: Vec4f,
    #[live]
    pub checker_dark: Vec4f,
}

/// A colour as a small block, pressable.
#[derive(Script, ScriptHook, Widget)]
pub struct ColorSwatch {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColorSwatch,
    #[live]
    pub color: Vec4f,
    #[live]
    pub selected: bool,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust]
    pressed: bool,
}

impl ColorSwatch {
    pub fn set_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.color != color {
            self.color = color;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_selected(&mut self, cx: &mut Cx, selected: bool) {
        if self.selected != selected {
            self.selected = selected;
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for ColorSwatch {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.draw_bg.swatch = self.color;
        self.draw_bg.chosen = if self.selected { 1.0 } else { 0.0 };
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // A hidden control still ends a press it holds (a cancellation is
        // terminal cleanup, not input).
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.draw_bg.hover = 1.0;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.hover = 0.0;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                self.pressed = true;
                self.draw_bg.down = 1.0;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                self.draw_bg.down = 0.0;
                self.draw_bg.redraw(cx);
                // The press only counts if it ends on the block it started
                // on, the way every other pressable thing behaves.
                if self.pressed && fe.is_over {
                    cx.widget_action(uid, ColorAction::Pressed(self.color));
                }
                self.pressed = false;
            }
            _ => {}
        }
    }

    fn text(&self) -> String {
        format_color_hex([self.color.x, self.color.y, self.color.z, self.color.w], true)
    }

    /// Accepts a colour in either notation; anything else leaves the block
    /// showing what it already showed.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some((rgba, _)) = parse_color(v) {
            self.set_color(cx, vec4(rgba[0], rgba[1], rgba[2], rgba[3]));
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format_color_hex(
            [self.color.x, self.color.y, self.color.z, self.color.w],
            true,
        ))
    }
}

impl ColorSwatchRef {
    pub fn set_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color(cx, color);
        }
    }

    pub fn color(&self) -> Vec4f {
        self.borrow().map(|inner| inner.color).unwrap_or_default()
    }

    /// The colour of a press, if this swatch was pressed.
    pub fn pressed(&self, actions: &Actions) -> Option<Vec4f> {
        for action in actions.filter_widget_actions_cast::<ColorAction>(self.widget_uid()) {
            if let ColorAction::Pressed(v) = action {
                return Some(v);
            }
        }
        None
    }
}

// ===========================================================================
// The hue ring
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawHueRing {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub hue: f32,
    #[live]
    pub puck_dark: Vec4f,
    #[live]
    pub puck_light: Vec4f,
}

/// A full turn of hue as a ring, with the rest of the colour carried along
/// so a host can read a whole colour off it.
#[derive(Script, ScriptHook, Widget)]
pub struct ColorWheel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawHueRing,
    #[live]
    pub color: Vec4f,
    #[live(true)]
    #[visible]
    visible: bool,
    /// The working state. The `color` property is derived from this and not
    /// the other way round, so the ring keeps pointing where it pointed when
    /// the colour goes grey or black.
    #[rust(Hsva::white())]
    hsva: Hsva,
    /// The `color` the HSVA was last built from, so an outside write to the
    /// property is noticed and one made from inside is not.
    #[rust]
    adopted: Vec4f,
    #[rust]
    dragging: bool,
}

impl ColorWheel {
    /// Take up a `color` written from outside — the DSL, a host, a control
    /// panel — without disturbing a hue that is only implied.
    fn adopt(&mut self) {
        if self.color != self.adopted {
            self.hsva = Hsva::from_vec4(self.color);
            self.adopted = self.color;
        }
    }

    fn store(&mut self, cx: &mut Cx) {
        self.color = self.hsva.to_vec4();
        self.adopted = self.color;
        self.draw_bg.redraw(cx);
    }

    pub fn hsva(&self) -> Hsva {
        self.hsva
    }

    pub fn set_hsva(&mut self, cx: &mut Cx, hsva: Hsva) {
        if self.hsva != hsva {
            self.hsva = hsva;
            self.store(cx);
        }
    }

    pub fn set_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.color != color {
            self.color = color;
            self.hsva = Hsva::from_vec4(color);
            self.adopted = color;
            self.draw_bg.redraw(cx);
        }
    }

    fn track(&mut self, cx: &mut Cx, uid: WidgetUid, abs: DVec2, ended: bool) {
        let rect = self.draw_bg.area().rect(cx);
        let size = rect.size.x.min(rect.size.y);
        self.hsva.h = ring_hue_at(abs - rect.pos, size);
        self.store(cx);
        report(cx, uid, self.color, ended);
    }
}

impl Widget for ColorWheel {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.adopt();
        self.draw_bg.hue = self.hsva.h;
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }
        let uid = self.widget_uid();
        // The RING is the control, not the square of screen it is drawn in.
        //
        // The hole is a hole: in a picker the saturation/value square is
        // drawn through it, and the corners belong to whatever is behind.
        // The hole test used to sit in the `FingerDown` arm below, which is
        // one step too late -- `hits` has captured the pointer and marked
        // the press handled by the time it answers, so the ring swallowed
        // every press in its bounding box and then declined to do anything
        // with it. A capture is a claim on the pointer: under the
        // pointer-capture rule everything around it stands its own gesture
        // down for the life of that press, so a press in the hole froze the
        // list the wheel sits in for as long as the button was held.
        //
        // Given to `hits` as the hit test, the ring only ever takes the
        // presses it is going to use -- and then keeps them, because a
        // capture tracks the pointer wherever it goes and the moves below
        // arrive whether or not they are still over the ring.
        match event.hits_with_test(cx, self.draw_bg.area(), |abs, rect, _| {
            let size = rect.size.x.min(rect.size.y);
            ring_contains(abs - rect.pos, size)
        }) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Crosshair);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.dragging = true;
                self.track(cx, uid, fe.abs, false);
            }
            Hit::FingerMove(fe) => {
                if self.dragging {
                    self.track(cx, uid, fe.abs, false);
                }
            }
            Hit::FingerUp(fe) => {
                if self.dragging {
                    self.dragging = false;
                    if fe.cancelled {
                        // Taken away: the colour stays the last one the drag
                        // set, and the edit ends there.
                        report(cx, uid, self.color, true);
                    } else {
                        self.track(cx, uid, fe.abs, true);
                    }
                }
            }
            Hit::KeyDown(ke) => {
                // A degree a press, a tenth of one with Shift: the ring is a
                // full turn across two hundred points, so one pixel is
                // nearly two degrees and the keyboard is the only way to
                // land on a hue exactly.
                let step = (if ke.modifiers.shift { 0.1 } else { 1.0 }) / 360.0;
                let delta = match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => -step,
                    KeyCode::ArrowRight | KeyCode::ArrowUp => step,
                    _ => return,
                };
                self.hsva.h = (self.hsva.h + delta).rem_euclid(1.0);
                self.store(cx);
                report(cx, uid, self.color, true);
            }
            _ => {}
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!("hue {:.0}", self.hsva.h * 360.0))
    }
}

impl ColorWheelRef {
    pub fn set_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color(cx, color);
        }
    }

    pub fn changed(&self, actions: &Actions) -> Option<Vec4f> {
        changed_in(actions, self.widget_uid())
    }

    pub fn ended(&self, actions: &Actions) -> Option<Vec4f> {
        ended_in(actions, self.widget_uid())
    }
}

// ===========================================================================
// The saturation / value square
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawColorArea {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub hue: f32,
    #[live]
    pub sat: f32,
    #[live]
    pub val: f32,
    #[live]
    pub border_radius: f32,
    #[live]
    pub puck_dark: Vec4f,
    #[live]
    pub puck_light: Vec4f,
}

/// Saturation across, value up, at one hue.
#[derive(Script, ScriptHook, Widget)]
pub struct ColorArea {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColorArea,
    #[live]
    pub color: Vec4f,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust(Hsva::white())]
    hsva: Hsva,
    #[rust]
    adopted: Vec4f,
    #[rust]
    dragging: bool,
}

impl ColorArea {
    fn adopt(&mut self) {
        if self.color != self.adopted {
            self.hsva = Hsva::from_vec4(self.color);
            self.adopted = self.color;
        }
    }

    fn store(&mut self, cx: &mut Cx) {
        self.color = self.hsva.to_vec4();
        self.adopted = self.color;
        self.draw_bg.redraw(cx);
    }

    pub fn hsva(&self) -> Hsva {
        self.hsva
    }

    pub fn set_hsva(&mut self, cx: &mut Cx, hsva: Hsva) {
        if self.hsva != hsva {
            self.hsva = hsva;
            self.store(cx);
        }
    }

    pub fn set_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.color != color {
            self.color = color;
            self.hsva = Hsva::from_vec4(color);
            self.adopted = color;
            self.draw_bg.redraw(cx);
        }
    }

    fn track(&mut self, cx: &mut Cx, uid: WidgetUid, abs: DVec2, ended: bool) {
        let rect = self.draw_bg.area().rect(cx);
        let (s, v) = area_sv_at(abs - rect.pos, rect.size);
        self.hsva.s = s;
        self.hsva.v = v;
        self.store(cx);
        report(cx, uid, self.color, ended);
    }
}

impl Widget for ColorArea {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.adopt();
        self.draw_bg.hue = self.hsva.h;
        self.draw_bg.sat = self.hsva.s;
        self.draw_bg.val = self.hsva.v;
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Crosshair);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.dragging = true;
                self.track(cx, uid, fe.abs, false);
            }
            Hit::FingerMove(fe) => {
                if self.dragging {
                    self.track(cx, uid, fe.abs, false);
                }
            }
            Hit::FingerUp(fe) => {
                if self.dragging {
                    self.dragging = false;
                    if fe.cancelled {
                        // Taken away: the colour stays the last one the drag
                        // set, and the edit ends there.
                        report(cx, uid, self.color, true);
                    } else {
                        self.track(cx, uid, fe.abs, true);
                    }
                }
            }
            Hit::KeyDown(ke) => {
                let step = if ke.modifiers.shift { 0.002 } else { 0.02 };
                let (ds, dv) = match ke.key_code {
                    KeyCode::ArrowLeft => (-step, 0.0),
                    KeyCode::ArrowRight => (step, 0.0),
                    KeyCode::ArrowUp => (0.0, step),
                    KeyCode::ArrowDown => (0.0, -step),
                    _ => return,
                };
                self.hsva.s = (self.hsva.s + ds).clamp(0.0, 1.0);
                self.hsva.v = (self.hsva.v + dv).clamp(0.0, 1.0);
                self.store(cx);
                report(cx, uid, self.color, true);
            }
            _ => {}
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!(
            "s {:.0} v {:.0}",
            self.hsva.s * 100.0,
            self.hsva.v * 100.0
        ))
    }
}

impl ColorAreaRef {
    pub fn set_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color(cx, color);
        }
    }

    pub fn changed(&self, actions: &Actions) -> Option<Vec4f> {
        changed_in(actions, self.widget_uid())
    }

    pub fn ended(&self, actions: &Actions) -> Option<Vec4f> {
        ended_in(actions, self.widget_uid())
    }
}

// ===========================================================================
// The alpha strip
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAlphaStrip {
    #[deref]
    draw_super: DrawQuad,
    /// The colour at full alpha — the right-hand end of the gradient.
    #[live]
    pub swatch: Vec4f,
    #[live]
    pub alpha: f32,
    #[live]
    pub border_size: f32,
    #[live]
    pub border_radius: f32,
    #[live]
    pub checker_size: f32,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub checker_light: Vec4f,
    #[live]
    pub checker_dark: Vec4f,
    #[live]
    pub puck_dark: Vec4f,
    #[live]
    pub puck_light: Vec4f,
}

/// The alpha of a colour, chosen along a strip.
#[derive(Script, ScriptHook, Widget)]
pub struct ColorAlpha {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawAlphaStrip,
    #[live]
    pub color: Vec4f,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust(Hsva::white())]
    hsva: Hsva,
    #[rust]
    adopted: Vec4f,
    #[rust]
    dragging: bool,
}

impl ColorAlpha {
    fn adopt(&mut self) {
        if self.color != self.adopted {
            self.hsva = Hsva::from_vec4(self.color);
            self.adopted = self.color;
        }
    }

    fn store(&mut self, cx: &mut Cx) {
        self.color = self.hsva.to_vec4();
        self.adopted = self.color;
        self.draw_bg.redraw(cx);
    }

    pub fn hsva(&self) -> Hsva {
        self.hsva
    }

    pub fn set_hsva(&mut self, cx: &mut Cx, hsva: Hsva) {
        if self.hsva != hsva {
            self.hsva = hsva;
            self.store(cx);
        }
    }

    pub fn set_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.color != color {
            self.color = color;
            self.hsva = Hsva::from_vec4(color);
            self.adopted = color;
            self.draw_bg.redraw(cx);
        }
    }

    fn track(&mut self, cx: &mut Cx, uid: WidgetUid, abs: DVec2, ended: bool) {
        let rect = self.draw_bg.area().rect(cx);
        self.hsva.a = alpha_at(abs.x - rect.pos.x, rect.size.x);
        self.store(cx);
        report(cx, uid, self.color, ended);
    }
}

impl Widget for ColorAlpha {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.adopt();
        self.draw_bg.swatch = self.hsva.opaque_vec4();
        self.draw_bg.alpha = self.hsva.a;
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.dragging = true;
                self.track(cx, uid, fe.abs, false);
            }
            Hit::FingerMove(fe) => {
                if self.dragging {
                    self.track(cx, uid, fe.abs, false);
                }
            }
            Hit::FingerUp(fe) => {
                if self.dragging {
                    self.dragging = false;
                    if fe.cancelled {
                        // Taken away: the colour stays the last one the drag
                        // set, and the edit ends there.
                        report(cx, uid, self.color, true);
                    } else {
                        self.track(cx, uid, fe.abs, true);
                    }
                }
            }
            Hit::KeyDown(ke) => {
                let step = (if ke.modifiers.shift { 1.0 } else { 5.0 }) / 255.0;
                let delta = match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => -step,
                    KeyCode::ArrowRight | KeyCode::ArrowUp => step,
                    _ => return,
                };
                self.hsva.a = (self.hsva.a + delta).clamp(0.0, 1.0);
                self.store(cx);
                report(cx, uid, self.color, true);
            }
            _ => {}
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!("alpha {:.2}", self.hsva.a))
    }
}

impl ColorAlphaRef {
    pub fn set_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color(cx, color);
        }
    }

    pub fn changed(&self, actions: &Actions) -> Option<Vec4f> {
        changed_in(actions, self.widget_uid())
    }
}

// ===========================================================================
// The palette strip
// ===========================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPaletteCell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub cell: Vec4f,
    #[live]
    pub hot: f32,
    #[live]
    pub chosen: f32,
    #[live]
    pub border_size: f32,
    #[live]
    pub border_radius: f32,
    #[live]
    pub checker_size: f32,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub border_color_hover: Vec4f,
    #[live]
    pub border_color_chosen: Vec4f,
    #[live]
    pub checker_light: Vec4f,
    #[live]
    pub checker_dark: Vec4f,
}

/// A wrapped grid of colour cells, drawn in one call and hit-tested by rect
/// arithmetic.
#[derive(Script, ScriptHook, Widget)]
pub struct PaletteStrip {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[area]
    area: Area,
    #[live]
    draw_bg: DrawPaletteCell,
    /// The cells, as hex colours separated by spaces or commas. A string
    /// rather than a list: the script layer's lists carry strings and ids,
    /// not numbers or colours, so a list of colours in the DSL has to be
    /// written down and parsed.
    #[live]
    pub colors: String,
    /// The cell equal to this colour is ringed. A colour rather than an
    /// index, so a host that knows what it holds does not have to work out
    /// where in the strip it landed.
    #[live]
    pub selected: String,
    #[live(14.0)]
    pub cell_size: f64,
    #[live(3.0)]
    pub gap: f64,
    #[live(true)]
    #[visible]
    visible: bool,

    #[rust]
    cells: Vec<[f32; 4]>,
    /// The `colors` string the cells were last built from. `None` until the
    /// first parse; set by `set_cells` too, so a host that fills the strip
    /// from Rust is not overwritten on the next draw.
    #[rust]
    parsed: Option<String>,
    #[rust]
    chosen: Option<usize>,
    #[rust]
    hot: Option<usize>,
    #[rust]
    cols: usize,
}

impl PaletteStrip {
    /// Fill the strip from Rust. Takes precedence over the `colors` string
    /// until that string is changed.
    pub fn set_cells(&mut self, cx: &mut Cx, cells: Vec<[f32; 4]>) {
        if self.cells != cells {
            self.cells = cells;
            self.hot = None;
            self.area.redraw(cx);
        }
        self.parsed = Some(self.colors.clone());
    }

    pub fn cells(&self) -> &[[f32; 4]] {
        &self.cells
    }

    fn adopt(&mut self) {
        if self.parsed.as_deref() != Some(self.colors.as_str()) {
            self.cells = parse_color_list(&self.colors);
            self.parsed = Some(self.colors.clone());
            self.hot = None;
        }
    }

    /// Ring the cell holding this colour, or none.
    pub fn set_selected_color(&mut self, cx: &mut Cx, color: Option<[f32; 4]>) {
        let chosen = color.and_then(|c| self.index_of(c));
        if self.chosen != chosen {
            self.chosen = chosen;
            self.area.redraw(cx);
        }
    }

    /// The cell holding a colour, comparing at the precision the strip can
    /// actually show.
    pub fn index_of(&self, color: [f32; 4]) -> Option<usize> {
        self.cells
            .iter()
            .position(|c| (0..4).all(|k| byte_of(c[k]) == byte_of(color[k])))
    }

    fn pitch(&self) -> f64 {
        self.cell_size + self.gap
    }

    fn cols_for(&self, width: f64) -> usize {
        if !width.is_finite() || width <= 0.0 {
            return 1;
        }
        (((width + self.gap) / self.pitch()).floor() as usize).max(1)
    }

    /// How tall the strip is at a width. A host that sizes a panel around it
    /// needs this before the strip is drawn.
    pub fn height_for(&self, width: f64) -> f64 {
        if self.cells.is_empty() {
            return 0.0;
        }
        let rows = self.cells.len().div_ceil(self.cols_for(width));
        rows as f64 * self.pitch() - self.gap
    }

    fn cell_at(&self, rect: Rect, abs: DVec2) -> Option<usize> {
        if !rect.contains(abs) || self.cols == 0 {
            return None;
        }
        let rel = abs - rect.pos;
        let col = (rel.x / self.pitch()).floor() as usize;
        let row = (rel.y / self.pitch()).floor() as usize;
        if col >= self.cols {
            return None;
        }
        // The gap between two cells belongs to neither of them.
        if rel.x - col as f64 * self.pitch() > self.cell_size
            || rel.y - row as f64 * self.pitch() > self.cell_size
        {
            return None;
        }
        let index = row * self.cols + col;
        (index < self.cells.len()).then_some(index)
    }
}

impl Widget for PaletteStrip {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.adopt();
        if !self.selected.is_empty() {
            if let Some((rgba, _)) = parse_color(&self.selected) {
                self.chosen = self.index_of(rgba);
            }
        }
        cx.begin_turtle(walk, Layout::flow_down());
        let width = cx.turtle().rect().size.x;
        self.cols = self.cols_for(width);
        let height = self.height_for(width);
        // Claim the grid's room first, then paint the cells over it: one
        // walk, so the strip cannot half-fit a row.
        let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(height)));
        let (pitch, cell_size) = (self.pitch(), self.cell_size);
        for (i, c) in self.cells.iter().enumerate() {
            let col = (i % self.cols) as f64;
            let row = (i / self.cols) as f64;
            self.draw_bg.cell = vec4(c[0], c[1], c[2], c[3]);
            self.draw_bg.hot = if self.hot == Some(i) { 1.0 } else { 0.0 };
            self.draw_bg.chosen = if self.chosen == Some(i) { 1.0 } else { 0.0 };
            self.draw_bg.draw_abs(
                cx,
                Rect {
                    pos: dvec2(rect.pos.x + col * pitch, rect.pos.y + row * pitch),
                    size: dvec2(cell_size, cell_size),
                },
            );
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }
        let uid = self.widget_uid();
        let rect = self.area.rect(cx);
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.cell_at(rect, fe.abs);
                cx.set_cursor(if hot.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                if hot != self.hot {
                    self.hot = hot;
                    self.area.redraw(cx);
                    cx.widget_action(uid, ColorAction::Hovered(hot));
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hot.is_some() {
                    self.hot = None;
                    self.area.redraw(cx);
                    cx.widget_action(uid, ColorAction::Hovered(None));
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                if let Some(i) = self.cell_at(rect, fe.abs) {
                    let c = self.cells[i];
                    cx.widget_action(uid, ColorAction::Picked(i, vec4(c[0], c[1], c[2], c[3])));
                }
            }
            _ => {}
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!("{} colours", self.cells.len()))
    }
}

impl PaletteStripRef {
    pub fn set_cells(&self, cx: &mut Cx, cells: Vec<[f32; 4]>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_cells(cx, cells);
        }
    }

    /// Which cell was clicked, and the colour in it.
    pub fn picked(&self, actions: &Actions) -> Option<(usize, Vec4f)> {
        for action in actions.filter_widget_actions_cast::<ColorAction>(self.widget_uid()) {
            if let ColorAction::Picked(i, c) = action {
                return Some((i, c));
            }
        }
        None
    }

    /// Which cell the pointer rests on, or `None` when it has left.
    pub fn hovered(&self, actions: &Actions) -> Option<Option<usize>> {
        for action in actions.filter_widget_actions_cast::<ColorAction>(self.widget_uid()) {
            if let ColorAction::Hovered(i) = action {
                return Some(i);
            }
        }
        None
    }
}

// ===========================================================================
// The picker
// ===========================================================================

/// How tall the panel is guessed to be before it has ever been drawn, as a
/// multiple of its width. The panel is a square wheel plus four short rows,
/// so this is close; it only decides whether the first frame of a popover
/// opens downwards or upwards, and the second frame has the real height.
const PANEL_ASPECT_GUESS: f64 = 1.7;

/// Wheel, square, alpha strip, numbers and recent colours, inline or under a
/// swatch.
#[derive(Script, WidgetRegister, WidgetRef, WidgetSet)]
pub struct ColorPicker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawQuad,

    /// The swatch that opens the panel. Only drawn in `popover` mode.
    #[live]
    pub swatch: WidgetRef,
    #[live]
    pub wheel: WidgetRef,
    #[live]
    pub square: WidgetRef,
    #[live]
    pub alpha: WidgetRef,
    /// The numeric and hex rows, as one view so a host can restyle or
    /// replace the lot.
    #[live]
    pub rows: WidgetRef,
    #[live]
    pub recent: WidgetRef,

    #[live]
    pub color: Vec4f,
    #[live]
    pub popover: bool,
    #[live(true)]
    pub with_alpha: bool,
    #[live(true)]
    pub with_recent: bool,
    #[live(244.0)]
    pub panel_width: f64,
    #[live(true)]
    visible: bool,

    #[rust(Hsva::white())]
    hsva: Hsva,
    #[rust]
    adopted: Vec4f,
    #[rust]
    open: bool,
    /// The colour when the panel opened, put back by Escape.
    #[rust]
    opened_with: Hsva,
    /// The panel's own draw list, so an open popover paints over everything
    /// its host is clipped by.
    #[rust]
    overlay: Option<DrawList2d>,
    /// The open panel's window-local rect. Unclipped on purpose: a clipped
    /// rect inside a scroll list comes back empty, and then every press
    /// anywhere reads as a press outside the panel and closes it.
    #[rust]
    panel_rect: Rect,
    /// The whole control at rest: the panel inline, the swatch in popover
    /// mode. What a snapshot and a pick see.
    #[rust]
    whole: Area,
    /// The recent list the strip was last filled from, so the strip is only
    /// refilled when the list actually changed.
    #[rust]
    recent_shown: Vec<[f32; 4]>,
    /// The colour the controls were last given. Without this the panel would
    /// rewrite the hex field on every draw, and rewriting a field asks for a
    /// redraw, which is a repaint that never settles.
    #[rust]
    pushed: Option<Hsva>,
}

impl ScriptHook for ColorPicker {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay = Some(DrawList2d::script_new(vm));
    }
}

impl ColorPicker {
    fn adopt(&mut self) {
        if self.color != self.adopted {
            self.hsva = Hsva::from_vec4(self.color);
            self.adopted = self.color;
        }
    }

    pub fn hsva(&self) -> Hsva {
        self.hsva
    }

    pub fn rgba(&self) -> [f32; 4] {
        self.hsva.to_rgba()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.color != color {
            self.color = color;
            self.hsva = Hsva::from_vec4(color);
            self.adopted = color;
            self.push(cx, false);
            self.redraw(cx);
        }
    }

    fn store(&mut self) {
        self.color = self.hsva.to_vec4();
        self.adopted = self.color;
    }

    /// Push the state into every control at once. Each of them holds its own
    /// HSVA, and each is given the WHOLE colour rather than the one channel
    /// it owns, so the square knows the hue to draw and the strip knows the
    /// colour to fade.
    fn push(&mut self, cx: &mut Cx, force: bool) {
        if !force && self.pushed == Some(self.hsva) {
            return;
        }
        self.pushed = Some(self.hsva);
        let hsva = self.hsva;
        if let Some(mut w) = self.wheel.borrow_mut::<ColorWheel>() {
            w.set_hsva(cx, hsva);
        }
        if let Some(mut w) = self.square.borrow_mut::<ColorArea>() {
            w.set_hsva(cx, hsva);
        }
        if let Some(mut w) = self.alpha.borrow_mut::<ColorAlpha>() {
            w.set_hsva(cx, hsva);
        }
        if let Some(mut w) = self.swatch.borrow_mut::<ColorSwatch>() {
            w.set_color(cx, hsva.to_vec4());
        }
        let rgba = hsva.to_rgba();
        for (row, id, value) in [
            (live_id!(row_rgb), live_id!(num_r), rgba[0] * 255.0),
            (live_id!(row_rgb), live_id!(num_g), rgba[1] * 255.0),
            (live_id!(row_rgb), live_id!(num_b), rgba[2] * 255.0),
            (live_id!(row_hsv), live_id!(num_h), hsva.h * 360.0),
            (live_id!(row_hsv), live_id!(num_s), hsva.s * 100.0),
            (live_id!(row_hsv), live_id!(num_v), hsva.v * 100.0),
        ] {
            if let Some(mut field) = self.field(row, id).borrow_mut::<ValueInput>() {
                field.set_value(cx, value.round() as f64);
            }
        }
        let hex = self.field(live_id!(row_hex), live_id!(hex));
        if !hex.is_empty() {
            // Never while the person is typing in it: rewriting the text
            // under a caret moves the caret.
            if hex.area() == Area::Empty || !cx.has_key_focus(hex.area()) {
                hex.set_text(cx, &format_color_hex(rgba, self.with_alpha));
            }
        }
        if let Some(mut strip) = self.recent.borrow_mut::<PaletteStrip>() {
            strip.set_selected_color(cx, Some(rgba));
        }
    }

    fn field(&self, row: LiveId, id: LiveId) -> WidgetRef {
        self.rows.child(row).child(id)
    }

    fn publish(&mut self, cx: &mut Cx, ended: bool) {
        self.store();
        let uid = self.widget_uid();
        report(cx, uid, self.color, ended);
        if ended {
            remember_color(self.hsva.to_rgba());
        }
        self.redraw(cx);
    }

    /// Refill the recent strip when the session's list has moved on.
    fn refresh_recent(&mut self, cx: &mut Cx) {
        if !self.with_recent {
            return;
        }
        let list = recent_colors();
        if list != self.recent_shown {
            self.recent_shown = list.clone();
            if let Some(mut strip) = self.recent.borrow_mut::<PaletteStrip>() {
                strip.set_cells(cx, list);
                strip.set_selected_color(cx, Some(self.hsva.to_rgba()));
            }
        }
    }

    pub fn open_panel(&mut self, cx: &mut Cx) {
        if self.open {
            return;
        }
        self.open = true;
        self.opened_with = self.hsva;
        let uid = self.widget_uid();
        cx.widget_action(uid, ColorAction::Opened);
        if let Some(list) = &self.overlay {
            list.redraw(cx);
        }
        self.redraw(cx);
    }

    /// Close the panel. `revert` puts back the colour it opened with, which
    /// is what Escape asks for; anything else commits what is there.
    pub fn close_panel(&mut self, cx: &mut Cx, revert: bool) {
        if !self.open {
            return;
        }
        if revert {
            self.hsva = self.opened_with;
            self.push(cx, false);
        }
        self.publish(cx, true);
        self.open = false;
        let uid = self.widget_uid();
        cx.widget_action(uid, ColorAction::Closed);
        if let Some(list) = &self.overlay {
            list.redraw(cx);
        }
        // The panel was painted in an overlay above every clip; nothing else
        // knows the room it occupied is free again.
        cx.redraw_all();
    }

    /// The open panel's rect, or an empty one. A host that gives an open
    /// popup priority over its own scrolling needs to know where it is.
    pub fn panel_rect(&self) -> Rect {
        if self.open {
            self.panel_rect
        } else {
            Rect::default()
        }
    }

    /// The panel: background, ring, square, strip, rows, recent colours.
    fn draw_panel(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) {
        let mut layout = self.layout;
        // The shape is the whole widget. A caller who wants these five
        // pieces in some other arrangement wants five widgets.
        layout.flow = Flow::Down;
        // Measured before the turtle opens: once begun, a turtle answers
        // about its inside.
        let outer = cx.turtle().next_walk_width(walk.width, walk.margin);
        let padding = layout.padding.left + layout.padding.right;
        let inner = inner_width(outer, padding, self.panel_width);

        self.draw_bg.begin(cx, walk, layout);
        for (name, slot) in [
            (live_id!(wheel), &self.wheel),
            (live_id!(square), &self.square),
            (live_id!(alpha), &self.alpha),
            (live_id!(rows), &self.rows),
            (live_id!(recent), &self.recent),
        ] {
            // Inserted here rather than by a container: nothing else draws
            // these, so without this a host could not reach ids!(picker.rows)
            // at all.
            cx.widget_tree_insert_child(self.uid, name, slot.clone());
        }
        // The ring takes the panel's full width, and the square goes in its
        // hole afterwards at an absolute rect, which does not disturb the
        // column the rest of the panel is flowing down.
        let _ = self.wheel.draw_walk(cx, scope, Walk::fixed(inner, inner));
        let ring = self.wheel.area().rect(cx);
        if ring.size.x > 0.0 {
            let _ = self
                .square
                .draw_walk(cx, scope, Walk::abs_rect(square_in_ring(ring)));
        }
        if self.with_alpha {
            let alpha_walk = Walk {
                width: Size::Fixed(inner),
                height: self.alpha.walk(cx.cx.cx).height,
                ..Walk::default()
            };
            let _ = self.alpha.draw_walk(cx, scope, alpha_walk);
        }
        // A FIXED width, never Fit: the fields inside ask for Fill, and a
        // Fill inside a Fit is laid out and never painted.
        let rows_walk = Walk {
            width: Size::Fixed(inner),
            height: Size::fit(),
            ..Walk::default()
        };
        let _ = self.rows.draw_walk(cx, scope, rows_walk);
        if self.with_recent && !self.recent_shown.is_empty() {
            let _ = self.recent.draw_walk(cx, scope, rows_walk);
        }
        self.draw_bg.end(cx);
    }
}

impl WidgetNode for ColorPicker {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.whole
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.whole.redraw(cx);
        if let Some(list) = &self.overlay {
            list.redraw(cx);
        }
    }

    /// The slots under their own names, so `ids!(picker.wheel)` reaches the
    /// ring and `ids!(picker.rows.row_hex.hex)` reaches the hex field. In
    /// popover mode the panel's pieces only exist while it is open.
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if self.popover {
            visit(live_id!(swatch), self.swatch.clone());
            if !self.open {
                return;
            }
        }
        for (id, slot) in [
            (live_id!(wheel), &self.wheel),
            (live_id!(square), &self.square),
            (live_id!(alpha), &self.alpha),
            (live_id!(rows), &self.rows),
            (live_id!(recent), &self.recent),
        ] {
            if !slot.is_empty() {
                visit(id, slot.clone());
            }
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for slot in [
            &self.swatch,
            &self.wheel,
            &self.square,
            &self.alpha,
            &self.rows,
            &self.recent,
        ] {
            slot.find_widgets_from_point(cx, point, found);
        }
    }

    fn layer_areas(&self) -> Vec<(&'static str, Area)> {
        vec![("draw_bg", self.draw_bg.area())]
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        // A widget that has never been drawn has no area to redraw through,
        // and its parent has to lay it out afresh before it exists at all.
        if matches!(self.whole, Area::Empty) {
            cx.redraw_all();
        } else {
            self.whole.redraw(cx);
        }
    }
}

impl Widget for ColorPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.adopt();
        self.refresh_recent(cx.cx.cx);
        if !self.popover {
            self.push(cx.cx.cx, false);
            self.draw_panel(cx, scope, walk);
            self.whole = self.draw_bg.area();
            return DrawStep::done();
        }

        if let Some(mut s) = self.swatch.borrow_mut::<ColorSwatch>() {
            s.set_color(cx.cx.cx, self.hsva.to_vec4());
        }
        // A bare layout, not the widget's: that padding and spacing belong to
        // the panel, and spending them around the swatch would leave a hole
        // the size of a popover in the row the swatch sits in.
        cx.begin_turtle(walk, Layout::flow_right());
        cx.widget_tree_insert_child(self.uid, live_id!(swatch), self.swatch.clone());
        let swatch_walk = self.swatch.walk(cx.cx.cx);
        let _ = self.swatch.draw_walk(cx, scope, swatch_walk);
        cx.end_turtle_with_area(&mut self.whole);

        if self.open {
            let anchor = self.whole.rect(cx);
            let width = self.panel_width;
            // The height guess only decides which way the first frame opens;
            // from the second frame on the measured rect is used.
            let height = if self.panel_rect.size.y > 0.0 {
                self.panel_rect.size.y
            } else {
                width * PANEL_ASPECT_GUESS
            };
            let overlay = self.overlay.as_mut().unwrap();
            overlay.begin_overlay_reuse(cx);
            let pass = cx.current_pass_size();
            cx.begin_root_turtle(pass, Layout::flow_down());
            // Under the swatch, right edges aligned; flipped above it when
            // the bottom would run off the pass.
            let mut pos = dvec2(
                anchor.pos.x + anchor.size.x - width,
                anchor.pos.y + anchor.size.y + 2.0,
            );
            if pos.y + height > pass.y {
                pos.y = (anchor.pos.y - height - 2.0).max(0.0);
            }
            pos.x = pos.x.clamp(0.0, (pass.x - width).max(0.0));
            let panel_walk = Walk {
                abs_pos: Some(pos),
                width: Size::Fixed(width),
                height: Size::fit(),
                ..Walk::default()
            };
            // Pushed BEFORE the controls draw: a redraw asked for during a
            // draw is dropped, so a sync afterwards would only show on the
            // next unrelated repaint.
            self.push(cx.cx.cx, false);
            self.draw_panel(cx, scope, panel_walk);
            self.panel_rect = self.draw_bg.area().rect(cx);
            cx.end_pass_sized_turtle();
            self.overlay.as_mut().unwrap().end(cx);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }

        if self.popover && self.open {
            if let Event::KeyDown(ke) = event {
                if ke.key_code == KeyCode::Escape {
                    self.close_panel(cx, true);
                    return;
                }
            }
            if let Event::MouseDown(me) = event {
                let anchor = self.whole.rect(cx);
                if !self.panel_rect.contains(me.abs) && !anchor.contains(me.abs) {
                    self.close_panel(cx, false);
                    // Not a return: the press still belongs to whatever is
                    // under it.
                }
            }
        }

        if !self.popover || self.open {
            let mut changed = false;
            let mut ended = false;
            let wheel_uid = self.wheel.widget_uid();
            let square_uid = self.square.widget_uid();
            let alpha_uid = self.alpha.widget_uid();
            let recent_uid = self.recent.widget_uid();
            let hex = self.field(live_id!(row_hex), live_id!(hex));
            let hex_uid = hex.widget_uid();
            // Read before the event is dispatched: by the time a blur is
            // reported the field has already let the keyboard go, and the
            // action that says so carries nothing.
            let hex_text = hex.text();
            let numbers = [
                (self.field(live_id!(row_rgb), live_id!(num_r)).widget_uid(), 0),
                (self.field(live_id!(row_rgb), live_id!(num_g)).widget_uid(), 1),
                (self.field(live_id!(row_rgb), live_id!(num_b)).widget_uid(), 2),
                (self.field(live_id!(row_hsv), live_id!(num_h)).widget_uid(), 3),
                (self.field(live_id!(row_hsv), live_id!(num_s)).widget_uid(), 4),
                (self.field(live_id!(row_hsv), live_id!(num_v)).widget_uid(), 5),
            ];
            // The square is offered the event before the ring: it is drawn
            // inside the ring's rect, and the ring's own hit test lets a
            // press in the hole through.
            let actions = cx.capture_actions(|cx| {
                self.square.handle_event(cx, event, scope);
                self.wheel.handle_event(cx, event, scope);
                self.alpha.handle_event(cx, event, scope);
                self.rows.handle_event(cx, event, scope);
                self.recent.handle_event(cx, event, scope);
            });
            for action in actions {
                let Some(wa) = action.as_widget_action() else {
                    continue;
                };
                if wa.widget_uid == wheel_uid
                    || wa.widget_uid == square_uid
                    || wa.widget_uid == alpha_uid
                {
                    match wa.cast::<ColorAction>() {
                        ColorAction::Changed(v) => {
                            self.hsva = component_hsva(self.hsva, v, wa.widget_uid, wheel_uid, square_uid);
                            changed = true;
                        }
                        ColorAction::Ended(v) => {
                            self.hsva = component_hsva(self.hsva, v, wa.widget_uid, wheel_uid, square_uid);
                            changed = true;
                            ended = true;
                        }
                        _ => {}
                    }
                    continue;
                }
                if wa.widget_uid == recent_uid {
                    if let ColorAction::Picked(_, c) = wa.cast::<ColorAction>() {
                        self.hsva = Hsva::from_vec4(c);
                        changed = true;
                        ended = true;
                    }
                    continue;
                }
                if wa.widget_uid == hex_uid {
                    // Enter and losing the keyboard mean the same thing: the
                    // person is done with the text. Only `Returned` carries
                    // it, so on a blur the field is asked what it holds.
                    let typed = match wa.cast::<TextInputAction>() {
                        TextInputAction::Returned(text, _) => Some(text),
                        TextInputAction::KeyFocusLost => Some(hex_text.clone()),
                        _ => None,
                    };
                    if let Some(text) = typed {
                        if let Some((rgba, had_alpha)) = parse_color(&text) {
                            let mut next = Hsva::from_rgba(rgba);
                            if !had_alpha {
                                next.a = self.hsva.a;
                            }
                            self.hsva = next;
                            changed = true;
                            ended = true;
                        }
                        // Right or wrong, the field is rewritten from the
                        // colour that is actually held.
                        self.push(cx, true);
                    }
                    continue;
                }
                for (num_uid, which) in numbers {
                    if wa.widget_uid != num_uid {
                        continue;
                    }
                    // A row is dragged now, so the two reports are two
                    // different moments: the move is what the wheel, the
                    // square and the hex follow, and the release is the one
                    // a host writes down and the recent strip remembers. A
                    // panel that ended on every move would file a hundred
                    // commits across one drag of the R row.
                    if let ValueInputAction::Changed(v) = wa.cast::<ValueInputAction>() {
                        self.hsva = channel_hsva(self.hsva, which, v);
                        changed = true;
                    }
                    if let ValueInputTrackAction::Ended(v) = wa.cast::<ValueInputTrackAction>() {
                        self.hsva = channel_hsva(self.hsva, which, v);
                        changed = true;
                        ended = true;
                    }
                }
            }
            if changed {
                self.publish(cx, ended);
                self.push(cx, false);
            }
        }

        if self.popover {
            let swatch_uid = self.swatch.widget_uid();
            let actions = cx.capture_actions(|cx| self.swatch.handle_event(cx, event, scope));
            for action in actions {
                let Some(wa) = action.as_widget_action() else {
                    continue;
                };
                if wa.widget_uid == swatch_uid {
                    if let ColorAction::Pressed(_) = wa.cast::<ColorAction>() {
                        if self.open {
                            self.close_panel(cx, false);
                        } else {
                            self.open_panel(cx);
                        }
                    }
                }
            }
        }
    }

    fn text(&self) -> String {
        format_color_hex(self.hsva.to_rgba(), self.with_alpha)
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some((rgba, _)) = parse_color(v) {
            self.set_color(cx, vec4(rgba[0], rgba[1], rgba[2], rgba[3]));
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format_color_hex(self.hsva.to_rgba(), true))
    }
}

/// Fold one component control's report back into the picker's colour: the
/// ring owns the hue, the square owns saturation and value, the strip owns
/// alpha. Taking the whole colour from any of them would let the ring's
/// implied saturation overwrite the square's real one.
fn component_hsva(
    current: Hsva,
    reported: Vec4f,
    from: WidgetUid,
    wheel: WidgetUid,
    square: WidgetUid,
) -> Hsva {
    let next = Hsva::from_vec4(reported);
    if from == wheel {
        Hsva { h: next.h, ..current }
    } else if from == square {
        Hsva {
            s: next.s,
            v: next.v,
            ..current
        }
    } else {
        Hsva { a: next.a, ..current }
    }
}

/// Fold one numeric row's value back into the colour. `which` is the row's
/// position in the picker's list: 0..2 are red, green and blue, 3..5 hue,
/// saturation and value.
fn channel_hsva(current: Hsva, which: usize, value: f64) -> Hsva {
    match which {
        0..=2 => {
            let mut rgba = current.to_rgba();
            rgba[which] = (value / 255.0).clamp(0.0, 1.0) as f32;
            let mut next = Hsva::from_rgba(rgba);
            next.a = current.a;
            // A colour driven to black or grey by its numbers keeps the hue
            // the ring is pointing at, or the ring would jump to red. The
            // threshold is a byte rather than zero: a hue that has been
            // round-tripped through RGB comes back a ten-millionth off, and
            // an exact test would let that through as a real colour.
            if next.s * 255.0 < 0.5 || next.v * 255.0 < 0.5 {
                next.h = current.h;
            }
            next
        }
        3 => Hsva {
            h: (value / 360.0).rem_euclid(1.0) as f32,
            ..current
        },
        4 => Hsva {
            s: (value / 100.0).clamp(0.0, 1.0) as f32,
            ..current
        },
        _ => Hsva {
            v: (value / 100.0).clamp(0.0, 1.0) as f32,
            ..current
        },
    }
}

impl ColorPickerRef {
    pub fn set_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color(cx, color);
        }
    }

    pub fn color(&self) -> Vec4f {
        self.borrow().map(|inner| inner.color).unwrap_or_default()
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map_or(false, |inner| inner.is_open())
    }

    /// Follow the colour under the hand.
    pub fn changed(&self, actions: &Actions) -> Option<Vec4f> {
        changed_in(actions, self.widget_uid())
    }

    /// Record the colour. One per gesture, not one per pixel.
    pub fn ended(&self, actions: &Actions) -> Option<Vec4f> {
        ended_in(actions, self.widget_uid())
    }
}

// ===========================================================================
// The text field
// ===========================================================================

/// A colour written down, next to the colour itself.
#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct ColorField {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live]
    pub swatch: WidgetRef,
    #[live]
    pub input: WidgetRef,

    #[live]
    pub color: Vec4f,
    #[live]
    pub notation: ColorNotation,
    #[live]
    pub with_alpha: bool,
    #[live(true)]
    visible: bool,

    #[rust]
    adopted: Vec4f,
    /// Whether the text in the field has been written since the colour last
    /// changed. Set on the first draw so an empty field is filled once.
    #[rust]
    written: bool,
    #[rust]
    whole: Area,
}

impl ColorField {
    fn adopt(&mut self) {
        if self.color != self.adopted {
            self.adopted = self.color;
            self.written = false;
        }
    }

    pub fn rgba(&self) -> [f32; 4] {
        [self.color.x, self.color.y, self.color.z, self.color.w]
    }

    /// The colour as the field spells it.
    pub fn formatted(&self) -> String {
        format_color(self.rgba(), self.notation, self.with_alpha)
    }

    pub fn set_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.color != color {
            self.color = color;
            self.adopted = color;
            self.written = false;
            self.redraw(cx);
        }
    }

    /// Read what is in the field and take it, or put back what is held. This
    /// is what "fixes itself" means: after this call the text and the colour
    /// always agree, whatever was typed.
    fn commit(&mut self, cx: &mut Cx, text: &str) {
        let uid = self.widget_uid();
        if let Some((rgba, had_alpha)) = parse_color(text) {
            let alpha = if had_alpha && self.with_alpha {
                rgba[3]
            } else {
                self.color.w
            };
            let next = vec4(rgba[0], rgba[1], rgba[2], alpha);
            if next != self.color {
                self.color = next;
                self.adopted = next;
                if let Some(mut s) = self.swatch.borrow_mut::<ColorSwatch>() {
                    s.set_color(cx, next);
                }
                report(cx, uid, next, true);
                remember_color([next.x, next.y, next.z, next.w]);
            }
        }
        let text = self.formatted();
        self.input.set_text(cx, &text);
        self.written = true;
        self.redraw(cx);
    }
}

impl WidgetNode for ColorField {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.whole
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.whole.redraw(cx);
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, slot) in [
            (live_id!(swatch), &self.swatch),
            (live_id!(input), &self.input),
        ] {
            if !slot.is_empty() {
                visit(id, slot.clone());
            }
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for slot in [&self.swatch, &self.input] {
            slot.find_widgets_from_point(cx, point, found);
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if matches!(self.whole, Area::Empty) {
            cx.redraw_all();
        } else {
            self.whole.redraw(cx);
        }
    }
}

impl Widget for ColorField {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.adopt();
        if let Some(mut s) = self.swatch.borrow_mut::<ColorSwatch>() {
            s.set_color(cx.cx.cx, self.color);
        }
        // Never while the person is typing in it: rewriting the text under a
        // caret moves the caret.
        if !self.written && !cx.cx.cx.has_key_focus(self.input.area()) {
            let text = self.formatted();
            self.input.set_text(cx.cx.cx, &text);
            self.written = true;
        }

        let mut layout = self.layout;
        layout.flow = Flow::right();
        let outer = cx.turtle().next_walk_width(walk.width, walk.margin);
        let padding = layout.padding.left + layout.padding.right;
        let inner = inner_width(outer, padding, 200.0);
        let spacing = layout.spacing;

        cx.begin_turtle(walk, layout);
        for (name, slot) in [
            (live_id!(swatch), &self.swatch),
            (live_id!(input), &self.input),
        ] {
            cx.widget_tree_insert_child(self.uid, name, slot.clone());
        }
        let swatch_walk = self.swatch.walk(cx.cx.cx);
        let _ = self.swatch.draw_walk(cx, scope, swatch_walk);
        // A FIXED width for the input: it asks for Fill, and a Fill in a row
        // that may be measured as Fit is laid out and never painted. The
        // swatch has already been drawn, so its width is known.
        let taken = self.swatch.area().rect(cx).size.x.max(0.0);
        let input_walk = Walk {
            width: Size::Fixed(field_input_width(inner, taken, spacing)),
            height: Size::fit(),
            ..Walk::default()
        };
        let _ = self.input.draw_walk(cx, scope, input_walk);
        cx.end_turtle_with_area(&mut self.whole);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }
        let input_uid = self.input.widget_uid();
        let swatch_uid = self.swatch.widget_uid();
        // Read before the event is dispatched: a blur is reported after the
        // field has let the keyboard go, and the action that says so carries
        // no text of its own.
        let typed = self.input.text();
        let actions = cx.capture_actions(|cx| {
            self.swatch.handle_event(cx, event, scope);
            self.input.handle_event(cx, event, scope);
        });
        let mut commit = None;
        for action in actions {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            if wa.widget_uid == input_uid {
                match wa.cast::<TextInputAction>() {
                    // Enter, and losing the keyboard, mean the same thing:
                    // the person is done with the text. A field that only
                    // repaired itself on Enter would sit there holding
                    // nonsense for as long as you looked away.
                    TextInputAction::Returned(text, _) => {
                        commit = Some(text);
                    }
                    TextInputAction::KeyFocusLost => {
                        commit = Some(typed.clone());
                    }
                    TextInputAction::Escaped => {
                        commit = Some(self.formatted());
                    }
                    _ => {}
                }
            } else if wa.widget_uid == swatch_uid {
                if let ColorAction::Pressed(_) = wa.cast::<ColorAction>() {
                    // The swatch beside the text is a shortcut into the
                    // field, not a second control: pressing it puts the
                    // keyboard where the colour is edited.
                    self.input.set_key_focus(cx);
                }
            }
        }
        if let Some(text) = commit {
            self.commit(cx, &text);
        }
    }

    fn text(&self) -> String {
        self.formatted()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.commit(cx, v);
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.formatted())
    }
}

impl ColorFieldRef {
    pub fn set_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color(cx, color);
        }
    }

    pub fn color(&self) -> Vec4f {
        self.borrow().map(|inner| inner.color).unwrap_or_default()
    }

    pub fn ended(&self, actions: &Actions) -> Option<Vec4f> {
        ended_in(actions, self.widget_uid())
    }
}

// ===========================================================================
// Tests — the pure core, which is where every decision in this file lives
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn rgb_and_hsv_are_the_same_colour_written_two_ways() {
        for rgb in [
            [1.0f32, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.2, 0.7, 0.4],
            [0.5, 0.5, 0.5],
            [0.0, 0.0, 0.0],
        ] {
            let rgba = [rgb[0], rgb[1], rgb[2], 0.75];
            let back = Hsva::from_rgba(rgba).to_rgba();
            for i in 0..4 {
                assert!(close(back[i], rgba[i]), "{rgba:?} came back {back:?}");
            }
        }
    }

    #[test]
    fn a_grey_keeps_the_hue_it_was_given() {
        // The whole reason the controls hold HSVA: a grey has no hue to read
        // back out of its RGB, so one that is only stored as RGB loses it.
        let held = Hsva::new(0.5, 0.0, 0.5, 1.0);
        let round_tripped = Hsva::from_rgba(held.to_rgba());
        assert_eq!(round_tripped.h, 0.0);
        assert_eq!(held.h, 0.5);
    }

    #[test]
    fn hex_is_read_in_all_three_lengths() {
        let (rgba, had_alpha) = parse_hex_color("#f80").unwrap();
        assert!(!had_alpha);
        assert!(close(rgba[0], 1.0));
        assert!(close(rgba[1], 8.0 / 15.0));
        assert!(close(rgba[2], 0.0));

        let (rgba, had_alpha) = parse_hex_color("#ff8000").unwrap();
        assert!(!had_alpha);
        assert!(close(rgba[1], 128.0 / 255.0));
        assert_eq!(format_color_hex(rgba, false), "#ff8000");

        // The hash is optional, and the case is not the caller's problem.
        let (rgba, had_alpha) = parse_hex_color("40E0D080").unwrap();
        assert!(had_alpha);
        assert_eq!(format_color_hex(rgba, true), "#40e0d080");
    }

    #[test]
    fn a_hex_that_is_not_a_colour_is_refused() {
        // Refusing is the point: the field puts back the colour it has
        // rather than inventing one out of half a number.
        assert!(parse_hex_color("#12345").is_none());
        assert!(parse_hex_color("#").is_none());
        assert!(parse_hex_color("nope").is_none());
        assert!(parse_hex_color("#ff80zz").is_none());
        assert!(parse_color("").is_none());
        assert!(parse_color("rgb(1, 2)").is_none());
    }

    #[test]
    fn the_numeric_notation_round_trips() {
        let (rgba, had_alpha) = parse_color("rgb(255, 128, 0)").unwrap();
        assert!(!had_alpha);
        assert_eq!(format_color_rgb(rgba, false), "rgb(255, 128, 0)");
        let (rgba, had_alpha) = parse_color("rgba(255, 128, 0, 0.5)").unwrap();
        assert!(had_alpha);
        assert_eq!(format_color_rgb(rgba, true), "rgba(255, 128, 0, 0.50)");
        // Bare numbers, because a person pasting three values means them.
        assert!(parse_color("255, 128, 0").is_some());
    }

    #[test]
    fn a_palette_string_drops_only_what_it_cannot_read() {
        let cells = parse_color_list("#f00 #00ff00, nonsense  #0000ffff");
        assert_eq!(cells.len(), 3);
        assert!(close(cells[0][0], 1.0));
        assert!(close(cells[1][1], 1.0));
        assert!(close(cells[2][2], 1.0));
        assert!(parse_color_list("   ").is_empty());
    }

    #[test]
    fn the_ring_puts_red_at_the_top_and_runs_clockwise() {
        let size = 200.0;
        let c = size * 0.5;
        assert!(close(ring_hue_at(dvec2(c, 10.0), size), 0.0));
        assert!(close(ring_hue_at(dvec2(size - 10.0, c), size), 0.25));
        assert!(close(ring_hue_at(dvec2(c, size - 10.0), size), 0.5));
        assert!(close(ring_hue_at(dvec2(10.0, c), size), 0.75));
    }

    #[test]
    fn the_ring_takes_presses_on_the_band_and_lets_the_hole_through() {
        let size = 200.0;
        let c = size * 0.5;
        // On the band: the mid radius is 43 percent of the side.
        assert!(ring_contains(dvec2(c + size * 0.43, c), size));
        // In the hole, where the square is drawn.
        assert!(!ring_contains(dvec2(c, c), size));
        // Outside the disc altogether.
        assert!(!ring_contains(dvec2(size, size), size));
    }

    #[test]
    fn the_square_reads_saturation_across_and_value_up() {
        let size = dvec2(100.0, 100.0);
        assert_eq!(area_sv_at(dvec2(0.0, 0.0), size), (0.0, 1.0));
        assert_eq!(area_sv_at(dvec2(100.0, 0.0), size), (1.0, 1.0));
        assert_eq!(area_sv_at(dvec2(100.0, 100.0), size), (1.0, 0.0));
        // A drag that leaves the square tracks the nearest edge.
        assert_eq!(area_sv_at(dvec2(400.0, -80.0), size), (1.0, 1.0));
    }

    #[test]
    fn the_square_sits_in_the_rings_hole() {
        let ring = Rect {
            pos: dvec2(10.0, 20.0),
            size: dvec2(200.0, 200.0),
        };
        let square = square_in_ring(ring);
        assert_eq!(square.size.x, 200.0 * SQUARE_HALF_FRACTION * 2.0);
        // Concentric with the ring.
        assert_eq!(square.pos.x + square.size.x * 0.5, 110.0);
        assert_eq!(square.pos.y + square.size.y * 0.5, 120.0);
        // And clear of the band, which is what the fractions are chosen for.
        let half_diagonal = square.size.x * 0.5 * std::f64::consts::SQRT_2;
        assert!(half_diagonal < RING_INNER_FRACTION * 200.0);
    }

    #[test]
    fn the_alpha_strip_clamps_at_both_ends() {
        assert_eq!(alpha_at(-20.0, 100.0), 0.0);
        assert_eq!(alpha_at(50.0, 100.0), 0.5);
        assert_eq!(alpha_at(300.0, 100.0), 1.0);
        // A strip that has never been drawn is fully opaque rather than
        // fully transparent, so a first frame never blanks a colour.
        assert_eq!(alpha_at(0.0, 0.0), 1.0);
    }

    #[test]
    fn a_panel_measured_in_a_fit_parent_falls_back_to_its_own_width() {
        assert_eq!(inner_width(244.0, 12.0, 244.0), 232.0);
        assert_eq!(inner_width(f64::NAN, 12.0, 244.0), 232.0);
        // Narrower than the floor and the fixed widths taken from it would
        // go negative.
        assert_eq!(inner_width(20.0, 12.0, 244.0), MIN_INNER_WIDTH);
    }

    #[test]
    fn the_field_gives_the_text_what_the_swatch_leaves() {
        assert_eq!(field_input_width(200.0, 22.0, 6.0), 172.0);
        // A swatch that has never been drawn measures zero, and the row
        // still has to paint something.
        assert_eq!(field_input_width(200.0, 0.0, 6.0), 194.0);
        assert_eq!(field_input_width(30.0, 22.0, 6.0), MIN_INNER_WIDTH);
    }

    #[test]
    fn recent_colours_move_to_the_front_rather_than_pile_up() {
        let mut list = Vec::new();
        let red = [1.0, 0.0, 0.0, 1.0];
        let green = [0.0, 1.0, 0.0, 1.0];
        push_recent_into(&mut list, red);
        push_recent_into(&mut list, green);
        push_recent_into(&mut list, red);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], red);
        assert_eq!(list[1], green);
        // A colour that came back through a hex field is not bit-identical
        // and must still count as the same one.
        push_recent_into(&mut list, [1.0, 0.0005, 0.0, 1.0]);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn the_recent_list_stops_at_its_cap() {
        let mut list = Vec::new();
        for i in 0..20 {
            push_recent_into(&mut list, [i as f32 / 20.0, 0.0, 0.0, 1.0]);
        }
        assert_eq!(list.len(), RECENT_MAX);
        // Newest first.
        assert!(close(list[0][0], 19.0 / 20.0));
    }

    #[test]
    fn a_number_row_only_moves_the_channel_it_owns() {
        let held = Hsva::new(0.25, 0.8, 0.6, 0.5);
        // Hue row, in degrees.
        assert!(close(channel_hsva(held, 3, 180.0).h, 0.5));
        assert!(close(channel_hsva(held, 3, 180.0).s, 0.8));
        // Saturation and value rows, in percent.
        assert!(close(channel_hsva(held, 4, 50.0).s, 0.5));
        assert!(close(channel_hsva(held, 5, 10.0).v, 0.1));
        // Alpha is nobody's row here; it survives every one of them.
        assert!(close(channel_hsva(held, 0, 128.0).a, 0.5));
    }

    #[test]
    fn a_colour_driven_to_black_by_its_numbers_keeps_the_hue_it_is_left_with() {
        // Cyan is green plus blue. Zeroing the green makes it blue, which is
        // a real hue change and must be kept. Zeroing the blue as well leaves
        // black, which has no hue of its own to read back, so the one being
        // held has to survive, or the ring would jump to red the moment the
        // numbers reach zero.
        let cyan = Hsva::new(0.5, 1.0, 1.0, 1.0);
        let blue = channel_hsva(cyan, 1, 0.0);
        assert!(close(blue.h, 4.0 / 6.0), "{}", blue.h);
        let black = channel_hsva(blue, 2, 0.0);
        assert!(black.v < 1.0 / 255.0, "{}", black.v);
        assert!(close(black.h, blue.h), "{} {}", black.h, blue.h);
    }
}

/// THE POINTER-CAPTURE RULE, as it applies to the hue ring.
///
/// The ring is a continuously dragged control, so it takes the pointer on its
/// press and keeps it until the release. The HOLE is not the ring: the
/// saturation square is drawn through it and the corners belong to whatever
/// is behind. A press there must leave the pointer alone, or every host
/// around the wheel stands its own gesture down for a press the wheel was
/// never going to use.
#[cfg(test)]
mod pointer_capture_tests {
    #![allow(dead_code)]
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);

    pub(super) struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        pub(super) fn new(cx: &mut Cx) -> Self {
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx) }
        }

        pub(super) fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
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

    pub(super) fn press(abs: Vec2d) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    pub(super) fn moved(abs: Vec2d) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.1,
        })
    }

    pub(super) fn release(abs: Vec2d) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 0.2,
        })
    }

    /// With no event loop here to end a capture on the release, the area a
    /// press captured is let go by hand, or it takes every later press.
    pub(super) fn claimed(event: &Event) -> Area {
        match event {
            Event::MouseDown(e) => e.handled.get(),
            Event::MouseMove(e) => e.handled.get(),
            _ => Area::Empty,
        }
    }

    pub(super) fn send(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()))
    }

    fn middle(cx: &Cx, widget: &WidgetRef) -> Vec2d {
        let rect = widget.area().rect(cx);
        assert!(rect.size.x > 0.0 && rect.size.y > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    const WHEEL: f64 = 200.0;

    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    hue := ColorWheel{
                        width: 200.
                        height: 200.
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let hue = root.widget(cx, ids!(hue));
        (root, hue)
    }

    /// Straight up from the middle, half way across the band.
    fn on_the_ring(cx: &Cx, hue: &WidgetRef) -> Vec2d {
        let rect = hue.area().rect(cx);
        assert_eq!(rect.size.x, WHEEL, "the wheel drew at the size asked for");
        let radius = (RING_INNER_FRACTION + RING_OUTER_FRACTION) * 0.5 * WHEEL;
        assert!(
            ring_contains(dvec2(WHEEL * 0.5, WHEEL * 0.5 - radius), WHEEL),
            "the point this test presses really is on the band"
        );
        rect.pos + dvec2(WHEEL * 0.5, WHEEL * 0.5 - radius)
    }

    fn changed(actions: &ActionsBuf, hue: &WidgetRef) -> bool {
        actions.iter().filter_map(|a| a.as_widget_action()).any(|a| {
            a.widget_uid == hue.widget_uid()
                && matches!(a.cast::<ColorAction>(), ColorAction::Changed(_))
        })
    }

    /// The control: a press on the band is the ring's, and it takes the
    /// pointer for the drag that follows.
    #[test]
    fn a_press_on_the_band_is_taken_and_held() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, hue) = start(&mut cx);
        let at = on_the_ring(&cx, &hue);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let actions = send(&mut cx, &root, &press(at));
        assert!(changed(&actions, &hue), "the hue moved to the press");
        assert!(
            cx.fingers.is_area_captured(hue.area()),
            "and the ring holds the pointer for the drag"
        );
        cx.fingers.first_mouse_button = None;
    }

    /// The bug: the hole test used to sit in the FingerDown arm, one step
    /// after `hits` had already captured the pointer and marked the press
    /// handled. The wheel swallowed every press in its bounding box and then
    /// did nothing with it.
    #[test]
    fn a_press_in_the_hole_is_not_the_rings_to_hold() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, hue) = start(&mut cx);
        let at = middle(&cx, &hue);
        assert!(
            !ring_contains(dvec2(WHEEL * 0.5, WHEEL * 0.5), WHEEL),
            "the middle of the wheel is the hole"
        );
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let event = press(at);
        let actions = send(&mut cx, &root, &event);
        assert!(!changed(&actions, &hue), "nothing was chosen");
        assert!(
            !cx.fingers.any_areas_captured(),
            "and nothing was held: the press is still there for the square"
        );
        let Event::MouseDown(e) = &event else { unreachable!() };
        assert!(e.handled.get().is_empty(), "nor was it marked as spoken for");
        cx.fingers.first_mouse_button = None;
    }
}

/// One colour, four views of it, and a hand on the number rows.
///
/// The rows are tracks now, so a press on one is a place to put the channel
/// rather than the start of a scrub that never got going. What the panel owes
/// the rest of itself is unchanged and easy to break while changing how a row
/// is driven: every other view of the colour follows on the same frame, the
/// hue survives a trip through grey and through black, and the panel says
/// what it holds while the row moves and commits once when it stops.
#[cfg(test)]
mod channel_rows {
    use super::pointer_capture_tests::{claimed, moved, press, release, send, Target};
    use super::*;

    const WINDOW: WindowId = WindowId(1, 1);

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef) {
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
                    pick := ColorPicker{color: #xFF0000FF}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let pick = root.widget(cx, ids!(pick));
        assert!(!pick.is_empty(), "the scene has a picker in it");
        (root, pick)
    }

    fn field(pick: &WidgetRef, row: LiveId, id: LiveId) -> WidgetRef {
        pick.borrow::<ColorPicker>().unwrap().field(row, id)
    }

    fn num(pick: &WidgetRef, id: LiveId) -> WidgetRef {
        let row = if id == live_id!(num_r) || id == live_id!(num_g) || id == live_id!(num_b) {
            live_id!(row_rgb)
        } else {
            live_id!(row_hsv)
        };
        field(pick, row, id)
    }

    fn row_value(pick: &WidgetRef, id: LiveId) -> f64 {
        num(pick, id)
            .borrow::<ValueInput>()
            .expect("the panel has that row")
            .value()
    }

    /// A window point `t` (0..1) along a row's fill — the span the row paints
    /// across and reads a press against.
    fn along(cx: &Cx, pick: &WidgetRef, id: LiveId, t: f64) -> Vec2d {
        let f = num(pick, id).area().rect(cx);
        assert!(f.size.x > 10.0, "the row was drawn wide enough to press");
        let (lo, hi) = crate::value_input::track_span(f.size.x);
        dvec2(f.pos.x + lo + t * (hi - lo), f.pos.y + f.size.y * 0.5)
    }

    /// One press on a row and its release, with the capture let go by hand.
    fn drag_row(cx: &mut Cx, root: &WidgetRef, pick: &WidgetRef, id: LiveId, t: f64) -> ActionsBuf {
        let at = along(cx, pick, id, t);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = press(at);
        let mut actions = send(cx, root, &down);
        actions.extend(send(cx, root, &release(at)));
        down.unhandle(cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        actions
    }

    fn wheel_hue(pick: &WidgetRef) -> f32 {
        pick.borrow::<ColorPicker>()
            .unwrap()
            .wheel
            .borrow::<ColorWheel>()
            .expect("the panel has a wheel")
            .hsva()
            .h
    }

    fn square_sv(pick: &WidgetRef) -> (f32, f32) {
        let inner = pick.borrow::<ColorPicker>().unwrap();
        let area = inner.square.borrow::<ColorArea>().expect("the panel has a square");
        (area.hsva().s, area.hsva().v)
    }

    fn hex_text(pick: &WidgetRef) -> String {
        field(pick, live_id!(row_hex), live_id!(hex)).text()
    }

    fn said(actions: &ActionsBuf, pick: &WidgetRef) -> Vec<ColorAction> {
        actions
            .filter_widget_actions_cast::<ColorAction>(pick.widget_uid())
            .collect()
    }

    /// The six number rows are tracks, and the hex line is not: a colour
    /// written down is typed, not aimed at.
    #[test]
    fn the_number_rows_are_tracks_and_the_hex_row_is_a_text_field() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (_root, pick) = start(&mut cx);
        for id in [
            live_id!(num_r),
            live_id!(num_g),
            live_id!(num_b),
            live_id!(num_h),
            live_id!(num_s),
            live_id!(num_v),
        ] {
            assert!(
                num(&pick, id).borrow::<ValueInput>().expect("the panel has that row").track,
                "a number row of the panel is not a track"
            );
        }
        let hex = field(&pick, live_id!(row_hex), live_id!(hex));
        assert!(hex.borrow::<TextInput>().is_some(), "the hex row stopped being a text field");
    }

    /// A move on the H row reaches the bytes, the hex, the wheel and the
    /// square, and does it on the same frame.
    #[test]
    fn moving_the_hue_row_carries_the_bytes_the_hex_and_the_wheel_with_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, pick) = start(&mut cx);
        assert_eq!(row_value(&pick, live_id!(num_h)), 0.0, "the panel opened on red");
        // A third of the way along 0..360 is green.
        drag_row(&mut cx, &root, &pick, live_id!(num_h), 1.0 / 3.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 120.0);
        assert_eq!(row_value(&pick, live_id!(num_r)), 0.0, "R never heard the hue move");
        assert_eq!(row_value(&pick, live_id!(num_g)), 255.0);
        assert_eq!(row_value(&pick, live_id!(num_b)), 0.0);
        assert_eq!(hex_text(&pick), "#00ff00ff", "the hex field never heard it");
        assert!((wheel_hue(&pick) - 1.0 / 3.0).abs() < 1e-4, "the wheel never heard it");
    }

    /// And the other way: a byte reaches H S V, the hex, the wheel and the
    /// square.
    #[test]
    fn moving_a_byte_row_carries_the_hsv_rows_the_hex_and_the_wheel_with_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, pick) = start(&mut cx);
        // Half way along G: 128 of 255, which is orange at 30 degrees.
        drag_row(&mut cx, &root, &pick, live_id!(num_g), 0.5);
        assert_eq!(row_value(&pick, live_id!(num_g)), 128.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 30.0, "H never heard the byte move");
        assert_eq!(row_value(&pick, live_id!(num_s)), 100.0);
        assert_eq!(row_value(&pick, live_id!(num_v)), 100.0);
        assert_eq!(hex_text(&pick), "#ff8000ff", "the hex field never heard it");
        let hue = wheel_hue(&pick);
        assert!((hue - 30.0 / 360.0).abs() < 1e-3, "the wheel never heard it: {hue}");
        assert_eq!(square_sv(&pick), (1.0, 1.0), "the square never heard it");
    }

    /// Saturation down to nothing and back up again keeps the hue.
    ///
    /// The panel holds HSVA and derives the rest; a grey has no hue to read
    /// back out of RGB, so a state kept as bytes would answer red the moment
    /// S touched the floor, and the colour would come back red.
    #[test]
    fn saturation_to_zero_and_back_keeps_the_hue() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, pick) = start(&mut cx);
        drag_row(&mut cx, &root, &pick, live_id!(num_h), 200.0 / 360.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 200.0);
        drag_row(&mut cx, &root, &pick, live_id!(num_s), 0.0);
        assert_eq!(row_value(&pick, live_id!(num_s)), 0.0);
        assert_eq!(
            row_value(&pick, live_id!(num_h)),
            200.0,
            "the hue jumped when the colour went grey"
        );
        drag_row(&mut cx, &root, &pick, live_id!(num_s), 1.0);
        assert_eq!(row_value(&pick, live_id!(num_s)), 100.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 200.0, "the hue did not come back");
    }

    /// And value down to black and back up again keeps both.
    #[test]
    fn value_to_zero_and_back_keeps_the_hue_and_the_saturation() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, pick) = start(&mut cx);
        drag_row(&mut cx, &root, &pick, live_id!(num_h), 200.0 / 360.0);
        drag_row(&mut cx, &root, &pick, live_id!(num_v), 0.0);
        assert_eq!(row_value(&pick, live_id!(num_v)), 0.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 200.0, "black lost the hue");
        assert_eq!(row_value(&pick, live_id!(num_s)), 100.0, "black lost the saturation");
        drag_row(&mut cx, &root, &pick, live_id!(num_v), 1.0);
        assert_eq!(row_value(&pick, live_id!(num_v)), 100.0);
        assert_eq!(row_value(&pick, live_id!(num_h)), 200.0, "the hue did not come back");
        assert_eq!(row_value(&pick, live_id!(num_s)), 100.0, "the saturation did not come back");
    }

    /// A colour that went in as a hex comes back out of the hex field
    /// unchanged, however far round the HSVA state it travelled.
    #[test]
    fn a_hex_comes_back_out_of_the_field_unchanged() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (_root, pick) = start(&mut cx);
        for text in ["#3a7bd5ff", "#ff8000ff", "#808080ff", "#000000ff", "#ffffffff", "#01020380"] {
            let (rgba, _) = parse_hex_color(text).expect("a hex this test wrote");
            pick.borrow_mut::<ColorPicker>()
                .unwrap()
                .set_color(&mut cx, vec4(rgba[0], rgba[1], rgba[2], rgba[3]));
            assert_eq!(hex_text(&pick), text, "the colour did not survive the round trip");
        }
    }

    /// The panel follows the row while it moves and commits once when it
    /// stops. A panel that ended on every move would write a hundred commits
    /// across one drag and fill the recent strip with the way there.
    #[test]
    fn the_panel_follows_a_row_and_commits_once_at_the_end() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, pick) = start(&mut cx);
        let changes = |actions: &ActionsBuf| {
            said(actions, &pick).iter().filter(|a| matches!(a, ColorAction::Changed(_))).count()
        };
        let ends = |actions: &ActionsBuf| {
            said(actions, &pick).iter().filter(|a| matches!(a, ColorAction::Ended(_))).count()
        };

        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &pick, live_id!(num_h), 0.25);
        let down = press(at);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(changes(&actions), 1, "the press said its colour more than once");
        assert_eq!(ends(&actions), 0, "the press committed a gesture it had just begun");

        let to = along(&cx, &pick, live_id!(num_h), 0.5);
        let actions = send(&mut cx, &root, &moved(to));
        assert_eq!(changes(&actions), 1, "the move said its colour more than once");
        assert_eq!(ends(&actions), 0, "the move committed mid-drag");
        assert_eq!(row_value(&pick, live_id!(num_h)), 180.0, "the drag did not follow");

        let actions = send(&mut cx, &root, &release(to));
        assert_eq!(ends(&actions), 1, "the release did not commit, or committed twice");
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }
}

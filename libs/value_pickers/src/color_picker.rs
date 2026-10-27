//! A colour written in code, edited by eye.
//!
//! The picker is a set of fixed-size designs, chosen by the width it is
//! given, never stretched: a hue RING of one size with the saturation/value
//! square in its hole (square corners, so pure white, black and the full
//! hue are each a corner), and an alpha strip; extra width is margin.
//! - compact (a narrow column): the palette as a row of swatches, the ring
//!   centred, the alpha strip under it;
//! - medium: the ring with the alpha strip standing beside it; to the right
//!   the colour as numbers (a switch steps HSL › RGB › OKLCH), the glow and
//!   the palette as a grid;
//! - wide: the same with the spaces as three chips, a row per number, and
//!   the palette with names.
//!
//! A colour in code may be light over 1 (`color(3.0, 2.0, 1.2)`: it glows
//! through the film's bloom), which no square can show: the picker keeps
//! such a colour as its hue, saturation and value at most 1 times an
//! INTENSITY, with its own slider ("glow ×2.4" above 1). The colour written
//! down takes `#rrggbb[aa]` or `color(r, g, b[, a])`.
//!
//! Nothing is shown twice: a host whose row already shows the colour and
//! its code (the editor panel's colour row) turns the picker's own value
//! line off ([`HdrColorPicker::set_show_value`]) and edits the code in its
//! row; on its own (an inspector) the picker shows a small old/new swatch
//! and the code field at its top.
//!
//! The picker writes nothing. A drag says [`ColorPickAction::Preview`] at
//! every step and [`ColorPickAction::Commit`] when it is let go; a typed
//! colour or a palette pick commits at once.

use crate::panel::control::{DrawPanelShape, R_CONTROL};
use crate::panel::palette_gen::{from_oklch, to_oklch};
use makepad_widgets::badge::advance;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    mod.widgets.DrawColorFieldBase = #(DrawColorField::script_component(vm))
    /** The picker's colour fields: the saturation/value square at a hue
     * (`mode` 0), an alpha strip over a checker (2), an old/new swatch (3),
     * the hue ring (4, its band `radius` wide); `vertical` turns the strip
     * upright. */
    set_type_default() do #(DrawColorField::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            if self.mode > 3.5 {
                // The hue ring: a full turn, zero at twelve o'clock and
                // rising clockwise; its band `radius` wide inside the quad.
                let c = vec2(w * 0.5, h * 0.5)
                let outer = min(w, h) * 0.5 - 0.5
                let p = self.pos * self.rect_size - c
                sdf.circle(c.x, c.y, outer)
                sdf.circle(c.x, c.y, outer - self.radius)
                sdf.subtract()
                let hue_at = fract(atan2(p.x, 0.0 - p.y) / 6.2831853 + 1.0)
                sdf.fill(Pal.hsv2rgb(vec4(hue_at, 1.0, 1.0, 1.0)))
                return sdf.result
            }
            sdf.box(0.5, 0.5, w - 1.0, h - 1.0, min(self.radius, min(w, h) * 0.5))
            let t = mix(self.pos.x, 1.0 - self.pos.y, self.vertical)
            let gx = floor(self.pos.x * w / 5.0)
            let gy = floor(self.pos.y * h / 5.0)
            let checker = mix(vec3(0.24, 0.26, 0.29), vec3(0.15, 0.16, 0.18), modf(gx + gy, 2.0))
            if self.mode < 0.5 {
                let s = clamp(self.pos.x, 0.0, 1.0)
                let v = 1.0 - clamp(self.pos.y, 0.0, 1.0)
                sdf.fill_keep(Pal.hsv2rgb(vec4(self.hue, s, v, 1.0)))
            } else if self.mode < 2.5 {
                let a = clamp(t, 0.0, 1.0)
                sdf.fill_keep(vec4(mix(checker, self.color.xyz, a), 1.0))
            } else {
                let c = mix(self.color2, self.color, step(0.5, self.pos.x))
                sdf.fill_keep(vec4(mix(checker, c.xyz, c.w), 1.0))
            }
            sdf.stroke(self.edge, 1.0)
            return sdf.result
        }
    }

    mod.widgets.HdrColorPickerBase = #(HdrColorPicker::register_widget(vm))

    /** A colour written in code, edited by eye: a fixed-size square with
     * hue and alpha strips, the palette, the glow and (wide) the colour as
     * numbers; compact, medium or wide by the width it gets. */
    mod.widgets.HdrColorPicker = set_type_default() do mod.widgets.HdrColorPickerBase{
        width: Fill height: Fit flow: Down spacing: 4
        draw_bg +: {color: #x00000000}
        draw_text +: {text_style: theme.font_regular{font_size: 8}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
        hex := TextInput{width: 132 height: 22 margin: 0 padding: Inset{left: 7 right: 7 top: 3 bottom: 3} empty_text: "#rrggbb"
            draw_bg +: {color: stage.row color_hover: stage.row color_focus: stage.row border_color: stage.line_faint border_size: 1.0 border_radius: 4.0}
            draw_text +: {text_style: theme.font_code{font_size: 8}}}
        everywhere := CheckBox{width: Fill visible: false text: "Everywhere this colour is used"
            draw_text +: {color: stage.text text_style: theme.font_regular{font_size: 8}}}
    }
}

/// A colour of a palette (or of the thing being edited), as the picker
/// lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteEntry {
    /// Its channels (over 1 for light) and alpha.
    pub rgba: [f64; 4],
    /// What it is called (its tag, or the `let` it is), else empty.
    pub name: String,
    /// How many places write or read it.
    pub uses: usize,
}

#[derive(Clone, Debug, Default)]
pub enum ColorPickAction {
    #[default]
    None,
    /// The colour while it moves: show it, do not write it.
    Preview([f64; 4]),
    /// The colour to write (a drag let go, a colour typed or picked).
    Commit([f64; 4]),
    /// Palette colour `i` was picked (its Commit follows): a host that
    /// writes palette colours by name writes that name.
    Palette(usize),
    /// The edit's reach: every place the colour (or its tag) is written.
    Everywhere(bool),
    /// Which of the thing's colours to edit ([`HdrColorPicker::set_sources`]).
    Source(usize),
}

/// The intensity slider's top: `2^MAX_STOPS` times the colour.
const MAX_STOPS: f64 = 4.0;

/// A colour as its displayable part (channels 0..1) and the intensity it is
/// multiplied by (1 unless a channel is over 1).
pub fn split_hdr(rgba: [f64; 4]) -> ([f64; 4], f64) {
    let m = rgba[0].max(rgba[1]).max(rgba[2]);
    if m > 1.0 {
        ([rgba[0] / m, rgba[1] / m, rgba[2] / m, rgba[3]], m)
    } else {
        (rgba, 1.0)
    }
}

/// A number as code writes it: at most three decimals, no trailing zeros.
fn num(v: f64) -> String {
    let mut s = format!("{:.3}", (v * 1000.0).round() / 1000.0);
    while s.contains('.') && (s.ends_with('0') || s.ends_with('.')) {
        s.pop();
    }
    if s == "-0" { "0".into() } else { s }
}

/// A colour as code writes it: `#rrggbb[aa]` when it fits a byte per
/// channel, else `color(r, g, b[, a])`.
pub fn format_code_color(rgba: [f64; 4]) -> String {
    if rgba[..3].iter().any(|c| *c > 1.0 + 1e-9) {
        if rgba[3] < 1.0 - 1e-9 {
            format!("color({}, {}, {}, {})", num(rgba[0]), num(rgba[1]), num(rgba[2]), num(rgba[3]))
        } else {
            format!("color({}, {}, {})", num(rgba[0]), num(rgba[1]), num(rgba[2]))
        }
    } else {
        let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        if b(rgba[3]) < 255 {
            format!("#{:02x}{:02x}{:02x}{:02x}", b(rgba[0]), b(rgba[1]), b(rgba[2]), b(rgba[3]))
        } else {
            format!("#{:02x}{:02x}{:02x}", b(rgba[0]), b(rgba[1]), b(rgba[2]))
        }
    }
}

/// A colour typed: `#rgb`, `#rrggbb`, `#rrggbbaa` (the hash optional), or
/// `color(r, g, b[, a])` with channels that may pass 1.
pub fn parse_code_color(text: &str) -> Option<[f64; 4]> {
    let t = text.trim();
    if let Some(args) = t.strip_prefix("color(").and_then(|a| a.strip_suffix(')')) {
        let v: Vec<f64> = args.split(',').map(|a| a.trim().parse::<f64>()).collect::<Result<_, _>>().ok()?;
        if !(3..=4).contains(&v.len()) || v.iter().any(|c| !c.is_finite() || *c < 0.0) {
            return None;
        }
        return Some([v[0], v[1], v[2], v.get(3).copied().unwrap_or(1.0).min(1.0)]);
    }
    let (rgba, _) = parse_hex_color(t)?;
    Some(rgba.map(|c| c as f64))
}

fn vec4_of(rgba: [f64; 4]) -> Vec4f {
    vec4(rgba[0] as f32, rgba[1] as f32, rgba[2] as f32, rgba[3] as f32)
}

/// What a palette colour is, said on hover.
fn entry_note(e: &PaletteEntry) -> String {
    let (_, intensity) = split_hdr(e.rgba);
    let name = if e.name.is_empty() { format_code_color(e.rgba) } else { format!("{} · {}", e.name, format_code_color(e.rgba)) };
    let glow = if intensity > 1.0 { format!(" · glow ×{}", num(intensity)) } else { String::new() };
    let uses = match e.uses {
        0 => String::new(),
        1 => " · used once".to_string(),
        n => format!(" · used by {n}"),
    };
    format!("{name}{glow}{uses}")
}

// ---------------------------------------------------------------- colour spaces

fn hsl_of(rgb: [f64; 3]) -> [f64; 3] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d <= 1e-9 {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-9);
    let h = if max == rgb[0] {
        ((rgb[1] - rgb[2]) / d).rem_euclid(6.0)
    } else if max == rgb[1] {
        (rgb[2] - rgb[0]) / d + 2.0
    } else {
        (rgb[0] - rgb[1]) / d + 4.0
    };
    [h * 60.0, s, l]
}

fn rgb_of_hsl(hsl: [f64; 3]) -> [f64; 3] {
    let (h, s, l) = (hsl[0].rem_euclid(360.0) / 60.0, hsl[1].clamp(0.0, 1.0), hsl[2].clamp(0.0, 1.0));
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    [r + m, g + m, b + m]
}

/// The numbers the wide picker shows the colour as.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Space {
    Hsl,
    Rgb,
    Oklch,
}

impl Space {
    const ALL: [Space; 3] = [Space::Hsl, Space::Rgb, Space::Oklch];

    fn name(self) -> &'static str {
        match self {
            Space::Hsl => "HSL",
            Space::Rgb => "RGB",
            Space::Oklch => "OKLCH",
        }
    }

    /// The colour's three numbers in this space.
    fn of(self, rgb: [f64; 3]) -> [f64; 3] {
        match self {
            Space::Hsl => hsl_of(rgb),
            Space::Rgb => rgb,
            Space::Oklch => to_oklch(rgb),
        }
    }

    fn rgb(self, v: [f64; 3]) -> [f64; 3] {
        match self {
            Space::Hsl => rgb_of_hsl(v),
            Space::Rgb => v.map(|c| c.clamp(0.0, 1.0)),
            Space::Oklch => from_oklch([v[0].clamp(0.0, 1.0), v[1].max(0.0), v[2].rem_euclid(360.0)]),
        }
    }

    /// Each number's name, how it is shown, and how much a pixel of drag
    /// moves it.
    fn fields(self) -> [(&'static str, fn(f64) -> String, f64); 3] {
        fn deg(v: f64) -> String {
            format!("{:.0}°", v)
        }
        fn pct(v: f64) -> String {
            format!("{:.0}%", v * 100.0)
        }
        fn byte(v: f64) -> String {
            format!("{:.0}", v * 255.0)
        }
        fn chroma(v: f64) -> String {
            format!("{:.3}", v)
        }
        match self {
            Space::Hsl => [("H", deg, 1.0), ("S", pct, 0.005), ("L", pct, 0.005)],
            Space::Rgb => [("R", byte, 1.0 / 255.0), ("G", byte, 1.0 / 255.0), ("B", byte, 1.0 / 255.0)],
            Space::Oklch => [("L", pct, 0.005), ("C", chroma, 0.002), ("H", deg, 1.0)],
        }
    }
}

// ---------------------------------------------------------------- layout

/// The square's design size, the strips' thickness and the gap.
/// The hue ring's diameter and band; the saturation/value square sits in
/// its hole (square corners: every corner colour is reachable).
const RING: f64 = 196.0;
const BAND: f64 = 13.0;
const SQ: f64 = RING;
const STRIP: f64 = 12.0;
const GAP: f64 = 8.0;
const CELL: f64 = 16.0;
/// Around everything: a puck or a ring at an edge stays inside.
const PAD: f64 = 7.0;
/// A strip's thumb travel is inset by half a thumb at each end.
const THUMB: f64 = 6.0;
/// The column right of the square (medium and wide): its least width in
/// each (where its content fills it) and its widest (more is margin).
const COLUMN_MEDIUM: f64 = 136.0;
const COLUMN_WIDE: f64 = 224.0;
const COLUMN: f64 = 240.0;
/// The square with its two upright strips.
const LEFT: f64 = RING + GAP + STRIP;
/// Each design from the width where its content fills it: below `MEDIUM`
/// compact, below `WIDE` medium.
const MEDIUM: f64 = LEFT + 16.0 + COLUMN_MEDIUM;
const WIDE: f64 = LEFT + 16.0 + COLUMN_WIDE;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Design {
    Compact,
    Medium,
    Wide,
}

/// Where the pointer is in the picker.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Square,
    Hue,
    Alpha,
    Glow,
    Palette(usize),
    Source(usize),
    Space(usize),
    /// Number `k` of the space's three (3: alpha).
    Number(usize),
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawColorField {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub mode: f32,
    #[live]
    pub vertical: f32,
    #[live]
    pub hue: f32,
    #[live]
    pub radius: f32,
    #[live]
    pub color: Vec4f,
    #[live]
    pub color2: Vec4f,
    #[live]
    pub edge: Vec4f,
}

#[derive(Script, ScriptHook, Widget)]
pub struct HdrColorPicker {
    #[deref]
    view: View,
    /// Under the drawn part (what the pointer hits).
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_field: DrawColorField,
    #[live]
    draw_shape: DrawPanelShape,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_small: DrawText,
    /// The colour as hue, saturation, value and alpha (kept through greys).
    #[rust(Hsva::white())]
    hsva: Hsva,
    #[rust(1.0)]
    intensity: f64,
    /// The colour it opened on (the swatch's old half).
    #[rust]
    old: Option<[f64; 4]>,
    #[rust]
    palette: Vec<PaletteEntry>,
    #[rust]
    sources: Vec<PaletteEntry>,
    #[rust]
    source: usize,
    /// Light over 1 can be written here ([`HdrColorPicker::set_hdr_allowed`]).
    #[rust(true)]
    hdr_allowed: bool,
    /// The value line (old/new swatch and the code field) is shown.
    #[rust(true)]
    show_value: bool,
    #[rust(Space::Hsl)]
    space: Space,
    #[rust]
    spots: Vec<(Rect, Part)>,
    #[rust]
    hot: Option<Part>,
    /// A drag: the part, and (for a number) where it began and its value.
    #[rust]
    press: Option<(Part, DVec2, [f64; 4])>,
    /// Where the code field goes (drawn after the hand-drawn part).
    #[rust]
    hex_at: Option<DVec2>,
}

impl HdrColorPicker {
    /// The colour shown (a host's value; nothing is reported).
    pub fn set_value(&mut self, cx: &mut Cx, rgba: [f64; 4]) {
        let (base, intensity) = split_hdr(rgba);
        let mut hsva = Hsva::from_rgba(base.map(|c| c as f32));
        // A grey keeps the hue the square was at.
        if hsva.s <= 0.0 || hsva.v <= 0.0 {
            hsva.h = self.hsva.h;
        }
        self.hsva = hsva;
        self.intensity = intensity;
        if self.press.is_none() {
            self.old = Some(rgba);
        }
        self.show(cx);
    }

    /// The colour as it stands (channels over 1 for light).
    pub fn value(&self) -> [f64; 4] {
        let c = self.hsva.to_rgba();
        let k = self.intensity;
        [c[0] as f64 * k, c[1] as f64 * k, c[2] as f64 * k, c[3] as f64]
    }

    /// Whether the place edited can take light over 1: a host whose code
    /// reads the colour as a plain colour (channels 0..1, `#rrggbb`) turns
    /// the intensity off.
    pub fn set_hdr_allowed(&mut self, cx: &mut Cx, allowed: bool) {
        self.hdr_allowed = allowed;
        self.show(cx);
    }

    /// The value line (an old/new swatch and the code field) on or off: off
    /// where the host's own row shows and edits the code.
    pub fn set_show_value(&mut self, cx: &mut Cx, show: bool) {
        self.show_value = show;
        self.view.widget(cx, ids!(hex)).set_visible(cx, show);
        self.view.redraw(cx);
    }

    pub fn shows_value(&self) -> bool {
        self.show_value
    }

    /// The file's palette.
    pub fn set_palette(&mut self, cx: &mut Cx, palette: Vec<PaletteEntry>) {
        if self.palette != palette {
            self.palette = palette;
            self.view.redraw(cx);
        }
    }

    /// The colours of the thing being edited (shown when it has more than
    /// one), and which of them the picker edits.
    pub fn set_sources(&mut self, cx: &mut Cx, sources: Vec<PaletteEntry>, selected: usize) {
        self.sources = sources;
        self.source = selected;
        self.view.redraw(cx);
    }

    /// The edit's reach, and how many places "everywhere" is.
    pub fn set_everywhere(&mut self, cx: &mut Cx, everywhere: bool, places: usize, tag: Option<&str>) {
        let what = match tag {
            Some(tag) => format!("Everywhere “{tag}” is used ({places} places)"),
            None => format!("Everywhere this colour is used ({places} places)"),
        };
        let check = self.view.widget(cx, ids!(everywhere)).as_check_box();
        check.set_text(&what);
        check.set_active(cx, everywhere, Animate::No);
        self.view.widget(cx, ids!(everywhere)).set_visible(cx, places > 1);
    }

    /// The code field written again and the picker redrawn.
    fn show(&mut self, cx: &mut Cx) {
        let value = self.value();
        self.view.widget(cx, ids!(hex)).set_text(cx, &format_code_color(value));
        self.view.redraw(cx);
    }

    fn rgb(&self) -> [f64; 3] {
        let c = self.hsva.to_rgba();
        [c[0] as f64, c[1] as f64, c[2] as f64]
    }

    fn set_rgb(&mut self, rgb: [f64; 3]) {
        let mut hsva = Hsva::from_rgba([rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, self.hsva.a]);
        if hsva.s <= 0.001 || hsva.v <= 0.001 {
            hsva.h = self.hsva.h;
        }
        self.hsva = hsva;
    }

    fn design(width: f64) -> Design {
        if width < MEDIUM {
            Design::Compact
        } else if width < WIDE {
            Design::Medium
        } else {
            Design::Wide
        }
    }

    /// The value at the pointer while `part` is dragged; whether anything
    /// moved.
    fn drag(&mut self, part: &Part, abs: DVec2, origin: DVec2, start: [f64; 4]) -> bool {
        let Some(rect) = self.spots.iter().find(|(_, p)| p == part).map(|(r, _)| *r) else { return false };
        // A strip's thumb travels its length less half a thumb at each end.
        let inset = if matches!(part, Part::Alpha) { THUMB } else { 0.0 };
        let tx = ((abs.x - rect.pos.x - inset) / (rect.size.x - 2.0 * inset).max(1.0)).clamp(0.0, 1.0) as f32;
        let ty = ((abs.y - rect.pos.y - inset) / (rect.size.y - 2.0 * inset).max(1.0)).clamp(0.0, 1.0) as f32;
        let upright = rect.size.y > rect.size.x;
        match part {
            Part::Square => {
                self.hsva.s = tx;
                self.hsva.v = 1.0 - ty;
            }
            Part::Hue => {
                let c = rect.pos + rect.size * 0.5;
                let d = abs - c;
                self.hsva.h = ((d.x.atan2(-d.y) / std::f64::consts::TAU).rem_euclid(1.0) as f32).min(0.9999);
            }
            Part::Alpha => self.hsva.a = if upright { 1.0 - ty } else { tx },
            Part::Glow => self.intensity = 2f64.powf(tx as f64 * MAX_STOPS),
            Part::Number(k) => {
                let d = abs.x - origin.x;
                if *k == 3 {
                    self.hsva.a = (start[3] + d * 0.005).clamp(0.0, 1.0) as f32;
                } else {
                    let space = self.space;
                    let mut v = space.of([start[0], start[1], start[2]]);
                    v[*k] += d * space.fields()[*k].2;
                    if space != Space::Rgb && ((space == Space::Hsl && *k == 0) || (space == Space::Oklch && *k == 2)) {
                        v[*k] = v[*k].rem_euclid(360.0);
                    }
                    self.set_rgb(space.rgb(v));
                }
            }
            _ => return false,
        }
        true
    }
}

impl Widget for HdrColorPicker {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        let at = |s: &Self, abs: DVec2| s.spots.iter().rev().find(|(r, _)| r.contains(abs)).map(|(_, p)| p.clone());
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = at(self, fe.abs);
                cx.set_cursor(match &hot {
                    Some(Part::Number(_)) => MouseCursor::EwResize,
                    Some(Part::Square) => MouseCursor::Crosshair,
                    Some(_) => MouseCursor::Hand,
                    None => MouseCursor::Default,
                });
                if hot != self.hot {
                    self.hot = hot;
                    self.view.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hot = None;
                self.view.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let Some(part) = at(self, fe.abs) else { return };
                let c = self.hsva.to_rgba();
                let start = [c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64];
                self.press = Some((part.clone(), fe.abs, start));
                if self.drag(&part, fe.abs, fe.abs, start) {
                    self.show(cx);
                    cx.widget_action(uid, ColorPickAction::Preview(self.value()));
                }
            }
            Hit::FingerMove(fe) => {
                if let Some((part, origin, start)) = self.press.clone() {
                    if self.drag(&part, fe.abs, origin, start) {
                        self.show(cx);
                        cx.widget_action(uid, ColorPickAction::Preview(self.value()));
                    }
                }
            }
            Hit::FingerUp(fe) => {
                let Some((part, origin, start)) = self.press.take() else { return };
                match part {
                    Part::Palette(i) if fe.is_over => {
                        if let Some(e) = self.palette.get(i).cloned() {
                            self.set_value(cx, e.rgba);
                            cx.widget_action(uid, ColorPickAction::Palette(i));
                            cx.widget_action(uid, ColorPickAction::Commit(e.rgba));
                        }
                    }
                    Part::Source(i) if fe.is_over && i != self.source => cx.widget_action(uid, ColorPickAction::Source(i)),
                    Part::Space(k) if fe.is_over => {
                        self.space = Space::ALL[k];
                        self.view.redraw(cx);
                    }
                    Part::Square | Part::Hue | Part::Alpha | Part::Glow | Part::Number(_) => {
                        self.drag(&part, fe.abs, origin, start);
                        self.show(cx);
                        cx.widget_action(uid, ColorPickAction::Commit(self.value()));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let hex = self.view.widget(cx, ids!(hex)).as_text_input();
            let typed = hex.returned(actions).map(|(t, _)| t).or_else(|| hex.key_focus_lost(actions).then(|| hex.text()));
            if let Some(text) = typed {
                match parse_code_color(&text) {
                    Some(rgba) if rgba != self.value() => {
                        self.set_value(cx, rgba);
                        cx.widget_action(uid, ColorPickAction::Commit(self.value()));
                    }
                    // Not a colour (or the same): the colour it is, written again.
                    _ => hex.set_text(cx, &format_code_color(self.value())),
                }
            }
            if let Some(on) = self.view.widget(cx, ids!(everywhere)).as_check_box().changed(actions) {
                cx.widget_action(uid, ColorPickAction::Everywhere(on));
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.view.visible {
            return DrawStep::done();
        }
        let p = makepad_look::palette::get(cx);
        let width = (cx.turtle().inner_width() - 2.0 * PAD).max(SQ);
        let design = Self::design(width);
        let value = self.value();
        let base = self.hsva.to_rgba();
        // Laid out first: the value line, the sources and palette rows,
        // the square and its strips, the right column.
        let line = if self.show_value { 26.0 } else { 0.0 };
        let rows_in = |n: usize, w: f64| -> f64 {
            if n == 0 {
                return 0.0;
            }
            let per = ((w + 4.0) / (CELL + 4.0)).floor().max(1.0) as usize;
            n.div_ceil(per) as f64 * (CELL + 4.0) + 2.0
        };
        let sources = if self.sources.len() > 1 { self.sources.len() } else { 0 };
        let glow_h = if self.hdr_allowed { 22.0 } else { 0.0 };
        let sq = if design == Design::Compact { SQ.min(width) } else { SQ };
        let column_x = LEFT + 16.0;
        let column_w = (width - column_x).clamp(0.0, COLUMN);
        let named = design == Design::Wide;
        // The numbers: four rows (wide), or two rows of two (medium).
        let numbers_h = match design {
            Design::Compact => 0.0,
            Design::Medium => 24.0 + 2.0 * 22.0 + 4.0,
            Design::Wide => 24.0 + 4.0 * 22.0 + 4.0,
        };
        let palette_h = if named {
            // Names beside swatches, in two columns.
            self.palette.len().div_ceil(2) as f64 * 18.0 + 2.0
        } else {
            rows_in(self.palette.len(), if design == Design::Compact { sq } else { column_w })
        };
        let inner_h = match design {
            Design::Compact => line + rows_in(sources, sq) + palette_h + sq + STRIP + 6.0 + glow_h,
            _ => (line + rows_in(sources, column_w) + numbers_h + glow_h + palette_h).max(sq),
        };
        cx.begin_turtle(walk, Layout::flow_down());
        let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(inner_h + 2.0 * PAD)));
        self.draw_bg.color = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.spots.clear();
        let o = rect.pos + dvec2(PAD, PAD);
        // The left part: centred in a compact design.
        let lx = if design == Design::Compact { o.x + ((width - sq) * 0.5).max(0.0) } else { o.x };
        let (mut ly, mut cy) = (o.y, o.y);
        let cx0 = o.x + column_x;
        // The value line: old | new and the code field.
        let value_line = |s: &mut Self, cx: &mut Cx2d, x: f64, y: f64| {
            let sw = Rect { pos: dvec2(x, y + 1.0), size: dvec2(36.0, 20.0) };
            s.draw_field.mode = 3.0;
            s.draw_field.radius = R_CONTROL;
            s.draw_field.color = vec4_of(split_hdr(value).0);
            s.draw_field.color2 = vec4_of(split_hdr(s.old.unwrap_or(value)).0);
            s.draw_field.edge = Vec4f::from_u32(p.border);
            s.draw_field.draw_abs(cx, sw);
            s.hex_at = Some(dvec2(x + 44.0, y));
        };
        self.hex_at = None;
        // Swatches in a row (wrapped in `w`), the current one ringed, the
        // one under the pointer named.
        let swatch_row = |s: &mut Self, cx: &mut Cx2d, x: f64, y: f64, w: f64, entries: &[PaletteEntry], current: Option<usize>, part: fn(usize) -> Part| -> f64 {
            let per = ((w + 4.0) / (CELL + 4.0)).floor().max(1.0) as usize;
            for (i, e) in entries.iter().enumerate() {
                let r = Rect { pos: dvec2(x + (i % per) as f64 * (CELL + 4.0), y + (i / per) as f64 * (CELL + 4.0)), size: dvec2(CELL, CELL) };
                let fill = vec4_of(split_hdr(e.rgba).0);
                if current == Some(i) || s.hot == Some(part(i)) {
                    let ring = if current == Some(i) { Vec4f::from_u32(p.text_hi) } else { Vec4f::from_u32(p.muted) };
                    s.draw_shape.edged_round(cx, Rect { pos: r.pos - dvec2(2.0, 2.0), size: r.size + dvec2(4.0, 4.0) }, vec4(0.0, 0.0, 0.0, 0.0), ring, 1.5, 6.0);
                }
                s.draw_shape.edged_round(cx, r, fill, Vec4f::from_u32(p.border), 1.0, 4.0);
                s.spots.push((r, part(i)));
            }
            if entries.is_empty() { 0.0 } else { entries.len().div_ceil(per) as f64 * (CELL + 4.0) + 2.0 }
        };
        let near = |a: [f64; 4], b: [f64; 4]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.6 / 255.0);
        let current = self.palette.iter().position(|e| near(e.rgba, value));
        let palette = self.palette.clone();
        let sources_list = self.sources.clone();
        let (tx, tw) = if design == Design::Compact { (lx, sq) } else { (cx0, column_w) };
        // The top of the text column (compact: over the square).
        let mut ty = if design == Design::Compact { ly } else { cy };
        if self.show_value {
            value_line(self, cx, tx, ty);
            ty += line;
        }
        if sources > 0 {
            ty += swatch_row(self, cx, tx, ty, tw, &sources_list, Some(self.source), Part::Source);
        }
        if design == Design::Compact {
            ty += swatch_row(self, cx, tx, ty, tw, &palette, current, Part::Palette);
            ly = ty;
        } else {
            cy = ty;
        }
        // The hue ring, the saturation/value square in its hole (square
        // corners: every corner colour can be reached), and the alpha strip
        // (beside it, or under it in a compact design).
        let ring = Rect { pos: dvec2(lx, ly), size: dvec2(sq, sq) };
        self.draw_field.mode = 4.0;
        self.draw_field.radius = BAND as f32;
        self.draw_field.draw_abs(cx, ring);
        self.spots.push((ring, Part::Hue));
        let side = 2.0 * ((sq * 0.5 - BAND) / std::f64::consts::SQRT_2 - 3.0);
        let square = Rect { pos: ring.pos + dvec2((sq - side) * 0.5, (sq - side) * 0.5), size: dvec2(side, side) };
        self.draw_field.mode = 0.0;
        self.draw_field.vertical = 0.0;
        self.draw_field.radius = 0.0;
        self.draw_field.hue = self.hsva.h;
        self.draw_field.edge = Vec4f::from_u32(p.line_faint);
        self.draw_field.draw_abs(cx, square);
        self.spots.push((square, Part::Square));
        let upright = design != Design::Compact;
        let alpha = if upright {
            Rect { pos: dvec2(lx + sq + GAP, ly), size: dvec2(STRIP, sq) }
        } else {
            Rect { pos: dvec2(lx, ly + sq + 6.0), size: dvec2(sq, STRIP) }
        };
        self.draw_field.mode = 2.0;
        self.draw_field.vertical = if upright { 1.0 } else { 0.0 };
        self.draw_field.radius = (STRIP * 0.5) as f32;
        self.draw_field.color = vec4(base[0], base[1], base[2], 1.0);
        self.draw_field.draw_abs(cx, alpha);
        self.spots.push((alpha, Part::Alpha));
        // The pucks: a light ring in a dark one, legible on any colour.
        let puck = |s: &mut Self, cx: &mut Cx2d, c: DVec2, r: f64| {
            s.draw_shape.edged_round(cx, Rect { pos: c - dvec2(r, r), size: dvec2(2.0 * r, 2.0 * r) }, vec4(0.0, 0.0, 0.0, 0.0), vec4(0.04, 0.04, 0.04, 0.9), 1.4, r as f32);
            s.draw_shape.edged_round(cx, Rect { pos: c - dvec2(r - 1.5, r - 1.5), size: dvec2(2.0 * r - 3.0, 2.0 * r - 3.0) }, vec4(0.0, 0.0, 0.0, 0.0), vec4(0.97, 0.97, 0.97, 1.0), 1.6, (r - 1.5) as f32);
        };
        puck(self, cx, dvec2(square.pos.x + side * self.hsva.s as f64, square.pos.y + side * (1.0 - self.hsva.v as f64)), 6.5);
        let h = self.hsva.h as f64;
        let a = self.hsva.a as f64;
        let mid = sq * 0.5 - BAND * 0.5 - 0.5;
        let angle = h * std::f64::consts::TAU;
        puck(self, cx, ring.pos + dvec2(sq * 0.5 + angle.sin() * mid, sq * 0.5 - angle.cos() * mid), THUMB + 0.5);
        // The alpha thumb, inset by half a thumb at each end.
        let travel = sq - 2.0 * THUMB;
        if upright {
            puck(self, cx, dvec2(alpha.pos.x + STRIP * 0.5, alpha.pos.y + THUMB + travel * (1.0 - a)), THUMB);
        } else {
            puck(self, cx, dvec2(alpha.pos.x + THUMB + travel * a, alpha.pos.y + STRIP * 0.5), THUMB);
        }
        // The glow (light over 1): a slim track with its value.
        let glow = |s: &mut Self, cx: &mut Cx2d, x: f64, y: f64, w: f64| {
            s.draw_small.color = Vec4f::from_u32(p.label);
            s.draw_small.draw_abs(cx, dvec2(x, y + 4.0), "Glow");
            let track = Rect { pos: dvec2(x + 34.0, y + 8.0), size: dvec2((w - 34.0 - 44.0).max(20.0), 3.0) };
            let t = (s.intensity.max(1.0).log2() / MAX_STOPS).clamp(0.0, 1.0);
            s.draw_shape.fill_round(cx, track, Vec4f::from_u32(p.track), 1.5);
            s.draw_shape.fill_round(cx, Rect { pos: track.pos, size: dvec2(track.size.x * t, 3.0) }, Vec4f::from_u32(p.accent), 1.5);
            s.draw_shape.edged_round(cx, Rect { pos: dvec2(track.pos.x + track.size.x * t - 5.0, track.pos.y - 3.5), size: dvec2(10.0, 10.0) }, Vec4f::from_u32(p.text_bright), Vec4f::from_u32(p.row), 1.0, 5.0);
            s.draw_small.color = Vec4f::from_u32(p.text_bright);
            s.draw_small.draw_abs(cx, dvec2(track.pos.x + track.size.x + 8.0, y + 4.0), &format!("×{}", num(s.intensity)));
            s.spots.push((Rect { pos: dvec2(track.pos.x - 4.0, y), size: dvec2(track.size.x + 8.0, 20.0) }, Part::Glow));
        };
        match design {
            Design::Compact => {
                if self.hdr_allowed {
                    glow(self, cx, lx, ly + sq + STRIP + 8.0, sq);
                }
            }
            Design::Medium | Design::Wide => {
                // The space: three chips (wide), or one that steps to the
                // next (medium); then its three numbers and the alpha.
                let mut x = cx0;
                for (k, space) in Space::ALL.iter().enumerate() {
                    let on = *space == self.space;
                    if !named && !on {
                        continue;
                    }
                    let label = if named { space.name().to_string() } else { format!("{} ›", space.name()) };
                    let w = advance(&self.draw_small, cx, &label) + 14.0;
                    let r = Rect { pos: dvec2(x, cy), size: dvec2(w, 18.0) };
                    let target = if named { k } else { (k + 1) % Space::ALL.len() };
                    let look = p.pick(on && named, self.hot == Some(Part::Space(target)), p.raised);
                    self.draw_shape.edged_round(cx, r, Vec4f::from_u32(look.fill), Vec4f::from_u32(look.edge), 1.0, R_CONTROL);
                    self.draw_small.color = Vec4f::from_u32(if on { p.text_hi } else { look.ink });
                    self.draw_small.draw_abs(cx, dvec2(x + 7.0, cy + 4.0), &label);
                    self.spots.push((r, Part::Space(target)));
                    x += w + 4.0;
                }
                cy += 24.0;
                let rgb = self.rgb();
                let values = self.space.of(rgb);
                let fields = self.space.fields();
                let field_w = if named { 64.0 } else { 46.0 };
                for k in 0..4 {
                    let (name, shown) = if k < 3 { (fields[k].0, (fields[k].1)(values[k])) } else { ("A", format!("{:.0}%", a * 100.0)) };
                    // Wide: a row each; medium: two to a row.
                    let (fx, fy) = if named { (cx0, cy + k as f64 * 22.0) } else { (cx0 + (k % 2) as f64 * (16.0 + field_w + 8.0), cy + (k / 2) as f64 * 22.0) };
                    self.draw_small.color = Vec4f::from_u32(p.muted);
                    self.draw_small.draw_abs(cx, dvec2(fx, fy + 4.0), name);
                    let r = Rect { pos: dvec2(fx + 14.0, fy), size: dvec2(field_w, 18.0) };
                    let hot = self.hot == Some(Part::Number(k)) || self.press.as_ref().is_some_and(|(q, _, _)| *q == Part::Number(k));
                    self.draw_shape.edged_round(cx, r, Vec4f::from_u32(p.row), Vec4f::from_u32(if hot { p.accent } else { p.line_faint }), 1.0, R_CONTROL);
                    self.draw_small.color = Vec4f::from_u32(p.text_bright);
                    self.draw_small.draw_abs(cx, dvec2(r.pos.x + 6.0, fy + 4.0), &shown);
                    self.spots.push((r, Part::Number(k)));
                }
                cy += numbers_h - 24.0;
                if self.hdr_allowed {
                    glow(self, cx, cx0, cy, column_w);
                    cy += glow_h;
                }
                if named {
                    // The palette with names, in two columns.
                    let col = (column_w * 0.5).floor();
                    for (i, e) in palette.iter().enumerate() {
                        let r = Rect { pos: dvec2(cx0 + (i % 2) as f64 * col, cy + (i / 2) as f64 * 18.0), size: dvec2(col - 4.0, 16.0) };
                        let hot = self.hot == Some(Part::Palette(i));
                        if hot || current == Some(i) {
                            self.draw_shape.fill_round(cx, r, Vec4f::from_u32(if current == Some(i) { p.accent_wash } else { p.control_hover }), R_CONTROL);
                        }
                        self.draw_shape.edged_round(cx, Rect { pos: r.pos + dvec2(2.0, 2.0), size: dvec2(12.0, 12.0) }, vec4_of(split_hdr(e.rgba).0), Vec4f::from_u32(p.border), 1.0, 3.0);
                        let name = if e.name.is_empty() { format_code_color(e.rgba) } else { e.name.clone() };
                        let (name, _) = crate::panel::control::fit_text(&self.draw_small, cx, &name, r.size.x - 22.0);
                        self.draw_small.color = Vec4f::from_u32(if current == Some(i) { p.text_hi } else { p.label });
                        self.draw_small.draw_abs(cx, dvec2(r.pos.x + 20.0, r.pos.y + 3.0), &name);
                        self.spots.push((r, Part::Palette(i)));
                    }
                } else {
                    // The palette as a grid of swatches (names on hover).
                    swatch_row(self, cx, cx0 + 2.0, cy + 2.0, column_w - 4.0, &palette, current, Part::Palette);
                }
            }
        }
        // A palette or source swatch under the pointer: what it is.
        let note = match &self.hot {
            Some(Part::Palette(i)) if !named => palette.get(*i).map(entry_note),
            Some(Part::Source(i)) => sources_list.get(*i).map(entry_note),
            _ => None,
        };
        if let (Some(note), Some((r, _))) = (note, self.hot.as_ref().and_then(|h| self.spots.iter().find(|(_, p)| p == h)).cloned()) {
            let w = advance(&self.draw_small, cx, &note) + 12.0;
            let x = r.pos.x.min(rect.pos.x + rect.size.x - w).max(rect.pos.x);
            let tip = Rect { pos: dvec2(x, r.pos.y + r.size.y + 3.0), size: dvec2(w, 18.0) };
            self.draw_text.new_draw_call(cx);
            self.draw_shape.new_draw_call(cx);
            self.draw_shape.edged_round(cx, tip, Vec4f::from_u32(p.menu), Vec4f::from_u32(p.border), 1.0, R_CONTROL);
            self.draw_small.new_draw_call(cx);
            self.draw_small.color = Vec4f::from_u32(p.text);
            self.draw_small.draw_abs(cx, dvec2(tip.pos.x + 6.0, tip.pos.y + 4.0), &note);
        }
        // The code field, where the value line put it; the reach under all.
        if let Some(at) = self.hex_at {
            let _ = self.view.widget(cx, ids!(hex)).draw_walk(cx, scope, Walk::new(Size::Fixed(132.0), Size::Fixed(22.0)).with_abs_pos(at));
        }
        let _ = self.view.widget(cx, ids!(everywhere)).draw_walk(cx, scope, Walk::new(Size::fill(), Size::fit()));
        cx.end_turtle();
        DrawStep::done()
    }
}

impl HdrColorPickerRef {
    /// What the picker said in this batch of actions.
    pub fn picked(&self, actions: &Actions) -> Vec<ColorPickAction> {
        actions.filter_widget_actions_cast::<ColorPickAction>(self.widget_uid()).filter(|a| !matches!(a, ColorPickAction::None)).collect()
    }

    pub fn set_value(&self, cx: &mut Cx, rgba: [f64; 4]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, rgba);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_round_trip() {
        for rgb in [[0.8, 0.3, 0.1], [0.1, 0.5, 0.9], [0.5, 0.5, 0.5], [1.0, 0.0, 1.0]] {
            for space in Space::ALL {
                let back = space.rgb(space.of(rgb));
                assert!(back.iter().zip(&rgb).all(|(a, b)| (a - b).abs() < 2e-3), "{space:?} {rgb:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn designs_by_width() {
        assert_eq!(HdrColorPicker::design(260.0), Design::Compact);
        assert_eq!(HdrColorPicker::design(MEDIUM - 1.0), Design::Compact);
        assert_eq!(HdrColorPicker::design(MEDIUM), Design::Medium);
        assert_eq!(HdrColorPicker::design(WIDE - 1.0), Design::Medium);
        assert_eq!(HdrColorPicker::design(WIDE), Design::Wide);
    }
}

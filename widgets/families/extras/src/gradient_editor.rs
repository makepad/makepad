//! GradientEditor — a colour ramp, edited on the ramp itself.
//!
//! A horizontal bar showing the gradient, with its COLOUR marks hanging under
//! it and its ALPHA marks standing over it. The two kinds are kept apart on
//! purpose: a colour and a transparency are almost never chosen at the same
//! places along a ramp, and a single mark that carries both forces a second
//! mark wherever only one of them changes.
//!
//! # The gestures
//!
//! * **Press the bar or a mark row** where there is no mark: a mark is added
//!   there, carrying the colour (or alpha) the ramp already has at that
//!   point, so adding one changes nothing until it is moved or edited. The
//!   row over the bar and the checkered top half of the bar add alpha marks;
//!   the row under it and the solid bottom half add colour marks. The drag
//!   carries on with the new mark, so placing one is a single gesture.
//! * **Drag a mark** along the bar to move it. It may pass the others; the
//!   stops are re-sorted and the mark keeps the finger.
//! * **Drag a mark well away from the bar** to remove it. It is taken out of
//!   the ramp as soon as it is far enough away, so the bar shows what letting
//!   go would leave, and it comes back if it is brought back. The last mark
//!   of either kind cannot be removed.
//! * **Double press a colour mark** to open the colour picker on it.
//!
//! The keyboard has the selected mark: Left and Right nudge it by `nudge`
//! (ten times that with Shift), Delete or Backspace removes it, and Enter
//! opens the picker on a colour mark.
//!
//! # The fields
//!
//! Under the bar sits a row for the selected mark: the colour picker under a
//! swatch (the library's own `ColorPickerButton`, not a second picker), the
//! mark's location in percent, and its intensity (a colour mark) or its
//! alpha (an alpha mark). The row is an ordinary `View` slot, so a host can
//! restyle it or turn it off with `show_fields: false`.
//!
//! # Intensity
//!
//! Every colour mark carries an HDR intensity of at least 1: the colour a
//! sample reports is the interpolated colour times the interpolated
//! intensity, so a ramp can run brighter than white for a bloom or an
//! emission map. The bar can only show what a display can, and draws the
//! product clipped to 1.
//!
//! # Interpolation
//!
//! `space` chooses what "halfway between two colours" means. `Srgb` blends
//! the stored sRGB values directly, which is what most tools do and what
//! designers expect to see; `Linear` blends light, which keeps a red-green
//! ramp from sagging to a muddy dark middle; `Oklab` blends in a perceptual
//! space, which also keeps the lightness even. The bar is drawn with one
//! quad per stretch between two stops, and the shader blends the two ends of
//! that stretch in the same space `sample` uses, so what is drawn is what a
//! host reads back.
use crate::{
    color::{format_color_hex, inner_width, ColorAction, ColorPicker},
    makepad_derive_widget::*,
    makepad_draw::*,
    value_input::{ValueInput, ValueInputAction},
    widget::*,
    CxWidgetExt,
};

// ===========================================================================
// The data model
// ===========================================================================

/// What "halfway between two colours" means.
#[derive(Copy, Clone, Debug, Default, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum GradientSpace {
    /// Blend the stored sRGB values directly.
    #[pick]
    #[default]
    Srgb = 0,
    /// Blend in linear light.
    Linear = 1,
    /// Blend in the Oklab perceptual space.
    Oklab = 2,
}

/// A few ready-made ramps. A `GradientEditor` whose `stops` (or
/// `alpha_stops`) are left empty takes them from its `preset`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum GradientPreset {
    /// Black to white.
    #[pick]
    #[default]
    Grayscale = 0,
    /// Black through deep red, orange and yellow to white.
    Fire = 1,
    /// Once round the hue circle, red back to red.
    Spectrum = 2,
    /// Deep water up to foam.
    Ocean = 3,
    /// White, fading from opaque to clear.
    Fade = 4,
    /// Black to an orange that runs four times brighter than white at the
    /// hot end: a ramp for emission, and the one that shows intensity.
    Ember = 5,
}

impl GradientPreset {
    pub const ALL: [GradientPreset; 6] = [
        GradientPreset::Grayscale,
        GradientPreset::Fire,
        GradientPreset::Spectrum,
        GradientPreset::Ocean,
        GradientPreset::Fade,
        GradientPreset::Ember,
    ];

    pub fn name(self) -> &'static str {
        match self {
            GradientPreset::Grayscale => "Grayscale",
            GradientPreset::Fire => "Fire",
            GradientPreset::Spectrum => "Spectrum",
            GradientPreset::Ocean => "Ocean",
            GradientPreset::Fade => "Fade",
            GradientPreset::Ember => "Ember",
        }
    }
}

/// One colour mark: where it sits along the ramp, its colour, and how many
/// times brighter than that colour it is.
///
/// In the DSL: `ColorStop{t: 0.5 color: #ff8000 intensity: 2.0}`.
#[derive(Clone, Copy, Debug, PartialEq, Script)]
pub struct ColorStop {
    /// Position along the ramp, 0..1.
    #[live]
    pub t: f64,
    /// The colour, sRGB. Its alpha is ignored: transparency belongs to the
    /// alpha stops.
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub color: Vec4f,
    /// HDR multiplier, 1 or more.
    #[live(1.0)]
    pub intensity: f32,
}

impl ColorStop {
    pub fn new(t: f64, color: Vec4f) -> Self {
        Self {
            t,
            color,
            intensity: 1.0,
        }
    }

    pub fn with_intensity(t: f64, color: Vec4f, intensity: f32) -> Self {
        Self {
            t,
            color,
            intensity,
        }
    }
}

// A stop may also be written as a bare object, `{t: 0.5 color: #f80}`, in a
// `stops: [...]` list: the list is the only place one is ever read, so what
// it holds can only mean a stop.
impl ScriptHook for ColorStop {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.as_object().is_some()
    }
}

/// One alpha mark: where it sits along the ramp and the alpha there.
///
/// In the DSL: `AlphaStop{t: 1.0 alpha: 0.0}`.
#[derive(Clone, Copy, Debug, PartialEq, Script)]
pub struct AlphaStop {
    /// Position along the ramp, 0..1.
    #[live]
    pub t: f64,
    /// Opacity, 0..1.
    #[live(1.0)]
    pub alpha: f32,
}

impl AlphaStop {
    pub fn new(t: f64, alpha: f32) -> Self {
        Self { t, alpha }
    }
}

impl ScriptHook for AlphaStop {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.as_object().is_some()
    }
}

/// Anything that sits at a position along the ramp.
trait Keyed {
    fn key(&self) -> f64;
    fn set_key(&mut self, t: f64);
}

impl Keyed for ColorStop {
    fn key(&self) -> f64 {
        self.t
    }
    fn set_key(&mut self, t: f64) {
        self.t = t;
    }
}

impl Keyed for AlphaStop {
    fn key(&self) -> f64 {
        self.t
    }
    fn set_key(&mut self, t: f64) {
        self.t = t;
    }
}

/// A position, made safe: finite and inside 0..1.
fn unit(t: f64) -> f64 {
    if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn unit_f32(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The pair of stops a position falls between: the last one at or before
/// it and the first one after it. Before the first stop and after the last
/// the pair is that stop twice, which holds its value out to the end.
fn locate<T: Keyed>(list: &[T], t: f64) -> (usize, usize) {
    let n = list.len();
    if n <= 1 {
        return (0, 0);
    }
    match list.iter().position(|s| s.key() > t) {
        None => (n - 1, n - 1),
        Some(0) => (0, 0),
        Some(j) => (j - 1, j),
    }
}

/// How far `t` is from `a` to `b`, 0..1.
fn frac(a: f64, b: f64, t: f64) -> f32 {
    let span = b - a;
    if span <= 0.0 {
        1.0
    } else {
        ((t - a) / span).clamp(0.0, 1.0) as f32
    }
}

/// Put `item` in its sorted place: after every stop at the same position,
/// so a new or moved mark lands on top of the ones it meets.
fn insert_sorted<T: Keyed>(list: &mut Vec<T>, item: T) -> usize {
    let t = item.key();
    let at = list.iter().position(|s| s.key() > t).unwrap_or(list.len());
    list.insert(at, item);
    at
}

fn sort_keyed<T: Keyed>(list: &mut [T]) {
    list.sort_by(|a, b| a.key().total_cmp(&b.key()));
}

// ---- colour arithmetic -----------------------------------------------------

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.max(0.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn to_linear3(c: Vec4f) -> [f32; 3] {
    [
        srgb_to_linear(c.x),
        srgb_to_linear(c.y),
        srgb_to_linear(c.z),
    ]
}

fn from_linear3(c: [f32; 3]) -> Vec4f {
    vec4(
        linear_to_srgb(c[0]),
        linear_to_srgb(c[1]),
        linear_to_srgb(c[2]),
        1.0,
    )
}

/// sRGB to Oklab, by way of linear light.
fn to_oklab(c: Vec4f) -> [f32; 3] {
    let [r, g, b] = to_linear3(c);
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.max(0.0).cbrt(), m.max(0.0).cbrt(), s.max(0.0).cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Oklab back to sRGB.
fn from_oklab(lab: [f32; 3]) -> Vec4f {
    let [ll, a, b] = lab;
    let l = ll + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = ll - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = ll - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    from_linear3([
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ])
}

fn lerp3(a: [f32; 3], b: [f32; 3], f: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
}

/// Two sRGB colours, blended `f` of the way in `space`. The result is sRGB
/// with an alpha of 1.
pub fn mix_in_space(a: Vec4f, b: Vec4f, f: f32, space: GradientSpace) -> Vec4f {
    match space {
        GradientSpace::Srgb => {
            let c = lerp3([a.x, a.y, a.z], [b.x, b.y, b.z], f);
            vec4(c[0], c[1], c[2], 1.0)
        }
        GradientSpace::Linear => from_linear3(lerp3(to_linear3(a), to_linear3(b), f)),
        GradientSpace::Oklab => from_oklab(lerp3(to_oklab(a), to_oklab(b), f)),
    }
}

fn rgb(hex: u32) -> Vec4f {
    vec4(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        1.0,
    )
}

/// Which list a mark belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Color,
    Alpha,
}

/// One mark: which list, and where in it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GradientMark {
    Color(usize),
    Alpha(usize),
}

impl GradientMark {
    pub fn new(kind: MarkKind, index: usize) -> Self {
        match kind {
            MarkKind::Color => GradientMark::Color(index),
            MarkKind::Alpha => GradientMark::Alpha(index),
        }
    }

    pub fn kind(self) -> MarkKind {
        match self {
            GradientMark::Color(_) => MarkKind::Color,
            GradientMark::Alpha(_) => MarkKind::Alpha,
        }
    }

    pub fn index(self) -> usize {
        match self {
            GradientMark::Color(i) | GradientMark::Alpha(i) => i,
        }
    }
}

/// A stop taken out of the ramp while it is dragged away from the bar, kept
/// so it can be put back if the drag comes back.
#[derive(Copy, Clone, Debug, PartialEq)]
enum HeldStop {
    Color(ColorStop),
    Alpha(AlphaStop),
}

/// One stretch of the bar between two consecutive positions where any stop
/// sits: the colour, intensity and alpha at both ends. Within a stretch every
/// channel runs straight in the gradient's own space, so the shader can draw
/// it from the two ends alone.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GradientPiece {
    pub t0: f64,
    pub t1: f64,
    /// sRGB colour at `t0`, alpha in `w`.
    pub c0: Vec4f,
    pub c1: Vec4f,
    pub i0: f32,
    pub i1: f32,
}

/// A colour ramp: colour stops with an HDR intensity each, a separate set of
/// alpha stops, and the space the colours are blended in.
///
/// Both lists are kept sorted by position, every position is inside 0..1,
/// and neither list is ever empty: every method that edits the ramp holds
/// those three, so a reader never has to check them.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub stops: Vec<ColorStop>,
    pub alpha_stops: Vec<AlphaStop>,
    pub space: GradientSpace,
}

impl Default for Gradient {
    /// Black to white, opaque.
    fn default() -> Self {
        Gradient::preset(GradientPreset::Grayscale)
    }
}

impl Gradient {
    /// A ramp from the given stops, sorted and clamped. An empty list gets
    /// one stop: opaque white for colour, fully opaque for alpha.
    pub fn new(stops: Vec<ColorStop>, alpha_stops: Vec<AlphaStop>, space: GradientSpace) -> Self {
        let mut g = Self {
            stops,
            alpha_stops,
            space,
        };
        g.normalize();
        g
    }

    /// Restore the invariants after the lists were edited by hand.
    pub fn normalize(&mut self) {
        for s in &mut self.stops {
            s.t = unit(s.t);
            s.color = vec4(
                unit_f32(s.color.x),
                unit_f32(s.color.y),
                unit_f32(s.color.z),
                1.0,
            );
            s.intensity = if s.intensity.is_finite() {
                s.intensity.max(1.0)
            } else {
                1.0
            };
        }
        for s in &mut self.alpha_stops {
            s.t = unit(s.t);
            s.alpha = unit_f32(s.alpha);
        }
        sort_keyed(&mut self.stops);
        sort_keyed(&mut self.alpha_stops);
        if self.stops.is_empty() {
            self.stops
                .push(ColorStop::new(0.0, vec4(1.0, 1.0, 1.0, 1.0)));
        }
        if self.alpha_stops.is_empty() {
            self.alpha_stops.push(AlphaStop::new(0.0, 1.0));
        }
    }

    pub fn preset(preset: GradientPreset) -> Self {
        let opaque = vec![AlphaStop::new(0.0, 1.0)];
        let c = ColorStop::new;
        let (stops, alpha) = match preset {
            GradientPreset::Grayscale => {
                (vec![c(0.0, rgb(0x000000)), c(1.0, rgb(0xffffff))], opaque)
            }
            GradientPreset::Fire => (
                vec![
                    c(0.0, rgb(0x000000)),
                    c(0.35, rgb(0xa8180c)),
                    c(0.62, rgb(0xf0801c)),
                    c(0.85, rgb(0xffdc50)),
                    c(1.0, rgb(0xffffff)),
                ],
                opaque,
            ),
            GradientPreset::Spectrum => (
                vec![
                    c(0.0, rgb(0xff0000)),
                    c(1.0 / 6.0, rgb(0xffff00)),
                    c(2.0 / 6.0, rgb(0x00ff00)),
                    c(3.0 / 6.0, rgb(0x00ffff)),
                    c(4.0 / 6.0, rgb(0x0000ff)),
                    c(5.0 / 6.0, rgb(0xff00ff)),
                    c(1.0, rgb(0xff0000)),
                ],
                opaque,
            ),
            GradientPreset::Ocean => (
                vec![
                    c(0.0, rgb(0x04101c)),
                    c(0.4, rgb(0x0b4a68)),
                    c(0.75, rgb(0x1fa2c2)),
                    c(1.0, rgb(0xc4f2ff)),
                ],
                opaque,
            ),
            GradientPreset::Fade => (
                vec![c(0.0, rgb(0xffffff))],
                vec![AlphaStop::new(0.0, 1.0), AlphaStop::new(1.0, 0.0)],
            ),
            GradientPreset::Ember => (
                vec![
                    c(0.0, rgb(0x000000)),
                    ColorStop::with_intensity(0.55, rgb(0xff5a14), 1.0),
                    ColorStop::with_intensity(1.0, rgb(0xffb040), 4.0),
                ],
                opaque,
            ),
        };
        Self::new(stops, alpha, GradientSpace::Srgb)
    }

    /// The colour at `t`, before intensity, and the intensity there.
    pub fn color_at(&self, t: f64) -> (Vec4f, f32) {
        let (a, b) = locate(&self.stops, t);
        let (sa, sb) = (&self.stops[a], &self.stops[b]);
        let f = frac(sa.t, sb.t, t);
        if a == b {
            return (sa.color, sa.intensity);
        }
        (
            mix_in_space(sa.color, sb.color, f, self.space),
            sa.intensity + (sb.intensity - sa.intensity) * f,
        )
    }

    pub fn alpha_at(&self, t: f64) -> f32 {
        let (a, b) = locate(&self.alpha_stops, t);
        let (sa, sb) = (&self.alpha_stops[a], &self.alpha_stops[b]);
        if a == b {
            return sa.alpha;
        }
        sa.alpha + (sb.alpha - sa.alpha) * frac(sa.t, sb.t, t)
    }

    /// The ramp at `t`: the colour times the intensity, with the alpha from
    /// the alpha stops. The colour is sRGB-encoded whatever `space` is, and
    /// may run past 1 where the intensity does.
    pub fn sample(&self, t: f64) -> Vec4f {
        let (c, i) = self.color_at(t);
        vec4(c.x * i, c.y * i, c.z * i, self.alpha_at(t))
    }

    /// `count` samples from one end to the other, both ends included.
    pub fn samples(&self, count: usize) -> Vec<Vec4f> {
        match count {
            0 => Vec::new(),
            1 => vec![self.sample(0.0)],
            n => (0..n)
                .map(|i| self.sample(i as f64 / (n - 1) as f64))
                .collect(),
        }
    }

    pub fn count(&self, kind: MarkKind) -> usize {
        match kind {
            MarkKind::Color => self.stops.len(),
            MarkKind::Alpha => self.alpha_stops.len(),
        }
    }

    /// Where a mark sits, or None when there is no such mark.
    pub fn mark_t(&self, mark: GradientMark) -> Option<f64> {
        match mark {
            GradientMark::Color(i) => self.stops.get(i).map(|s| s.t),
            GradientMark::Alpha(i) => self.alpha_stops.get(i).map(|s| s.t),
        }
    }

    /// Add a colour stop; returns where it went.
    pub fn add_stop(&mut self, t: f64, color: Vec4f, intensity: f32) -> usize {
        let mut s = ColorStop::with_intensity(unit(t), color, intensity);
        s.color = vec4(unit_f32(color.x), unit_f32(color.y), unit_f32(color.z), 1.0);
        s.intensity = if intensity.is_finite() {
            intensity.max(1.0)
        } else {
            1.0
        };
        insert_sorted(&mut self.stops, s)
    }

    /// Add a colour stop carrying the colour the ramp already has there.
    pub fn add_stop_sampled(&mut self, t: f64) -> usize {
        let (c, i) = self.color_at(unit(t));
        self.add_stop(t, c, i)
    }

    pub fn add_alpha(&mut self, t: f64, alpha: f32) -> usize {
        insert_sorted(
            &mut self.alpha_stops,
            AlphaStop::new(unit(t), unit_f32(alpha)),
        )
    }

    pub fn add_alpha_sampled(&mut self, t: f64) -> usize {
        let a = self.alpha_at(unit(t));
        self.add_alpha(t, a)
    }

    /// Add a mark of either kind carrying what the ramp has there.
    pub fn add_sampled(&mut self, kind: MarkKind, t: f64) -> GradientMark {
        match kind {
            MarkKind::Color => GradientMark::Color(self.add_stop_sampled(t)),
            MarkKind::Alpha => GradientMark::Alpha(self.add_alpha_sampled(t)),
        }
    }

    /// Remove a mark. The last mark of its kind stays, and so false.
    pub fn remove(&mut self, mark: GradientMark) -> bool {
        self.take(mark).is_some()
    }

    fn take(&mut self, mark: GradientMark) -> Option<HeldStop> {
        match mark {
            GradientMark::Color(i) if self.stops.len() > 1 && i < self.stops.len() => {
                Some(HeldStop::Color(self.stops.remove(i)))
            }
            GradientMark::Alpha(i) if self.alpha_stops.len() > 1 && i < self.alpha_stops.len() => {
                Some(HeldStop::Alpha(self.alpha_stops.remove(i)))
            }
            _ => None,
        }
    }

    fn put(&mut self, held: HeldStop, t: f64) -> GradientMark {
        match held {
            HeldStop::Color(mut s) => {
                s.t = unit(t);
                GradientMark::Color(insert_sorted(&mut self.stops, s))
            }
            HeldStop::Alpha(mut s) => {
                s.t = unit(t);
                GradientMark::Alpha(insert_sorted(&mut self.alpha_stops, s))
            }
        }
    }

    /// Move a mark to `t`, re-sorting; returns the mark at its new index.
    pub fn move_mark(&mut self, mark: GradientMark, t: f64) -> GradientMark {
        fn shift<T: Keyed>(list: &mut Vec<T>, i: usize, t: f64) -> usize {
            if i >= list.len() {
                return i;
            }
            let mut s = list.remove(i);
            s.set_key(unit(t));
            insert_sorted(list, s)
        }
        match mark {
            GradientMark::Color(i) => GradientMark::Color(shift(&mut self.stops, i, t)),
            GradientMark::Alpha(i) => GradientMark::Alpha(shift(&mut self.alpha_stops, i, t)),
        }
    }

    pub fn set_stop_color(&mut self, index: usize, color: Vec4f) {
        if let Some(s) = self.stops.get_mut(index) {
            s.color = vec4(unit_f32(color.x), unit_f32(color.y), unit_f32(color.z), 1.0);
        }
    }

    pub fn set_stop_intensity(&mut self, index: usize, intensity: f32) {
        if let Some(s) = self.stops.get_mut(index) {
            s.intensity = if intensity.is_finite() {
                intensity.max(1.0)
            } else {
                1.0
            };
        }
    }

    pub fn set_alpha(&mut self, index: usize, alpha: f32) {
        if let Some(s) = self.alpha_stops.get_mut(index) {
            s.alpha = unit_f32(alpha);
        }
    }

    /// The stretches the bar is drawn in, one per gap between consecutive
    /// positions where any stop of either kind sits.
    pub fn pieces(&self) -> Vec<GradientPiece> {
        let mut ts: Vec<f64> = vec![0.0, 1.0];
        ts.extend(self.stops.iter().map(|s| s.t));
        ts.extend(self.alpha_stops.iter().map(|s| s.t));
        ts.sort_by(|a, b| a.total_cmp(b));
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let mut out = Vec::with_capacity(ts.len());
        for w in ts.windows(2) {
            let (t0, t1) = (w[0], w[1]);
            if t1 - t0 < 1e-9 {
                continue;
            }
            // No stop lies strictly inside the stretch, so the pair its
            // middle falls between is the pair for both of its ends. At a
            // hard step, where two stops share a position, that is the pair
            // on this stretch's side of it.
            let m = (t0 + t1) * 0.5;
            let (a, b) = locate(&self.stops, m);
            let (sa, sb) = (&self.stops[a], &self.stops[b]);
            let colour = |t: f64| -> (Vec4f, f32) {
                if a == b {
                    return (sa.color, sa.intensity);
                }
                let f = frac(sa.t, sb.t, t);
                (
                    mix_in_space(sa.color, sb.color, f, self.space),
                    sa.intensity + (sb.intensity - sa.intensity) * f,
                )
            };
            let (a, b) = locate(&self.alpha_stops, m);
            let (qa, qb) = (&self.alpha_stops[a], &self.alpha_stops[b]);
            let alpha = |t: f64| -> f32 {
                if a == b {
                    qa.alpha
                } else {
                    qa.alpha + (qb.alpha - qa.alpha) * frac(qa.t, qb.t, t)
                }
            };
            let (c0, i0) = colour(t0);
            let (c1, i1) = colour(t1);
            out.push(GradientPiece {
                t0,
                t1,
                c0: vec4(c0.x, c0.y, c0.z, alpha(t0)),
                c1: vec4(c1.x, c1.y, c1.z, alpha(t1)),
                i0,
                i1,
            });
        }
        out
    }

    /// The ramp written down: every colour stop as position, hex and
    /// intensity, then every alpha stop.
    pub fn describe(&self) -> String {
        let space = match self.space {
            GradientSpace::Srgb => "srgb",
            GradientSpace::Linear => "linear",
            GradientSpace::Oklab => "oklab",
        };
        let colours: Vec<String> = self
            .stops
            .iter()
            .map(|s| {
                format!(
                    "{:.3} {} x{:.2}",
                    s.t,
                    format_color_hex([s.color.x, s.color.y, s.color.z, 1.0], false),
                    s.intensity
                )
            })
            .collect();
        let alphas: Vec<String> = self
            .alpha_stops
            .iter()
            .map(|s| format!("{:.3} a{:.2}", s.t, s.alpha))
            .collect();
        format!("{space} | {} | {}", colours.join(", "), alphas.join(", "))
    }
}

// ===========================================================================
// Actions
// ===========================================================================

#[derive(Clone, Debug, Default)]
pub enum GradientEditorAction {
    /// The ramp moved under the hand, or was edited in the fields. Follow
    /// this; do not write it down on every report.
    Changed(Gradient),
    /// A gesture finished, or a key or a field committed an edit. This is
    /// the one to record.
    Ended(Gradient),
    /// The selected mark changed: another mark, the same mark at a new index
    /// after it passed another, or none.
    Selected(Option<GradientMark>),
    #[default]
    None,
}

// ===========================================================================
// DSL
// ===========================================================================

script_mod! {
    use mod.prelude.widgets_internal.*

    // Declared before the `use` below, because a block's `use` only sees
    // what already exists when it runs; the bar's shader matches on it.
    let GradientSpace = set_type_default() do #(GradientSpace::script_api(vm))
    mod.widgets.GradientSpace = GradientSpace
    let GradientPreset = set_type_default() do #(GradientPreset::script_api(vm))
    mod.widgets.GradientPreset = GradientPreset

    use mod.widgets.*

    mod.widgets.ColorStop = #(ColorStop::script_api(vm))
    mod.widgets.AlphaStop = #(AlphaStop::script_api(vm))

    set_type_default() do #(DrawGradientStrip::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawGradientBar::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawGradientMark::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.GradientEditorBase = #(GradientEditor::register_widget(vm))

    /** A colour ramp edited on the ramp: colour marks under the bar, alpha
     * marks over it, a press to add one, a drag to move one, a drag well
     * away to remove one, and a row of fields for the selected mark. */
    mod.widgets.GradientEditor = set_type_default() do mod.widgets.GradientEditorBase{
        width: Fill
        height: Fit
        spacing: theme.space_2
        /** the ramp to start from when stops or alpha_stops are empty: GradientPreset.Grayscale Fire Spectrum Ocean Fade Ember */
        preset: GradientPreset.Grayscale
        /** how colours are blended between stops: GradientSpace.Srgb Linear Oklab */
        space: GradientSpace.Srgb
        /** mark width in pixels 7..24 step 1 */
        mark_width: 11.0
        /** mark height in pixels 8..28 step 1 */
        mark_height: 15.0
        /** bar height in pixels 8..64 step 1 */
        bar_height: 26.0
        /** share of the bar, from the top, drawn over a checker to show alpha 0..1 step 0.05 */
        alpha_band: 0.5
        /** how far past the strip a dragged mark must go to be removed, in pixels 8..96 step 1 */
        remove_distance: 28.0
        /** the most intensity a colour mark may take 1..64 step 1 */
        max_intensity: 16.0
        /** how far one arrow press moves the selected mark 0.001..0.1 step 0.001 */
        nudge: 0.01
        /** offer an intensity field for colour marks */
        with_intensity: true
        /** show the row of fields for the selected mark */
        show_fields: true
        /** drawn at all */
        visible: true

        // uniform(), not plain: nothing here has a Rust field behind it.
        draw_bg +: {
            /** corner rounding of the bar 0..12 step 0.5 */
            radius: uniform(theme.radius_xs)
            border_color: uniform(theme.color_outline_variant)
            focus_color: uniform(theme.color_primary)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // The ring the bar sits in: one point wider than the bar on
                // every side. The bar's pieces are drawn over the inside, so
                // what shows of this is the ring alone. `box` rounds by twice
                // its radius, so half a point more keeps the corners
                // concentric with the bar's.
                sdf.box(
                    self.bar.x - 1.0
                    self.bar.y - 1.0
                    self.bar.z + 2.0
                    self.bar.w + 2.0
                    self.radius + 0.5
                )
                sdf.fill(self.border_color.mix(self.focus_color, self.focus))
                return sdf.result
            }
        }

        draw_bar +: {
            /** corner rounding at the two ends of the bar 0..12 step 0.5 */
            radius: uniform(theme.radius_xs)
            /** the side of one square of the transparency checker 2..16 step 1 */
            checker_size: uniform(5.0)
            checker_light: uniform(theme.color_surface_container_highest)
            checker_dark: uniform(theme.color_surface_container_lowest)

            srgb_decode1: fn(c: float) -> float {
                return mix(c / 12.92, pow((max(c, 0.0) + 0.055) / 1.055, 2.4), step(0.04045, c))
            }
            srgb_encode1: fn(c: float) -> float {
                let v = max(c, 0.0)
                return mix(v * 12.92, 1.055 * pow(v, 1.0 / 2.4) - 0.055, step(0.0031308, v))
            }
            srgb_decode: fn(c: vec3) -> vec3 {
                return vec3(self.srgb_decode1(c.x), self.srgb_decode1(c.y), self.srgb_decode1(c.z))
            }
            srgb_encode: fn(c: vec3) -> vec3 {
                return vec3(self.srgb_encode1(c.x), self.srgb_encode1(c.y), self.srgb_encode1(c.z))
            }
            to_oklab: fn(c: vec3) -> vec3 {
                let lin = self.srgb_decode(c)
                let l = pow(max(dot(vec3(0.4122214708, 0.5363325363, 0.0514459929), lin), 0.0), 1.0 / 3.0)
                let m = pow(max(dot(vec3(0.2119034982, 0.6806995451, 0.1073969566), lin), 0.0), 1.0 / 3.0)
                let s = pow(max(dot(vec3(0.0883024619, 0.2817188376, 0.6299787005), lin), 0.0), 1.0 / 3.0)
                let lms = vec3(l, m, s)
                return vec3(
                    dot(vec3(0.2104542553, 0.7936177850, -0.0040720468), lms)
                    dot(vec3(1.9779984951, -2.4285922050, 0.4505937099), lms)
                    dot(vec3(0.0259040371, 0.7827717662, -0.8086757660), lms)
                )
            }
            from_oklab: fn(lab: vec3) -> vec3 {
                let l = dot(vec3(1.0, 0.3963377774, 0.2158037573), lab)
                let m = dot(vec3(1.0, -0.1055613458, -0.0638541728), lab)
                let s = dot(vec3(1.0, -0.0894841775, -1.2914855480), lab)
                let lms = vec3(l * l * l, m * m * m, s * s * s)
                return self.srgb_encode(vec3(
                    dot(vec3(4.0767416621, -3.3077115913, 0.2309699292), lms)
                    dot(vec3(-1.2684380046, 2.6097574011, -0.3413193965), lms)
                    dot(vec3(-0.0041960863, -0.7034186147, 1.7076147010), lms)
                ))
            }
            // The same blend `Gradient::sample` does, so the bar and a
            // sample agree.
            blend_space: fn(a: vec3, b: vec3, f: float) -> vec3 {
                let out = match self.space {
                    GradientSpace.Linear => self.srgb_encode(self.srgb_decode(a).mix(self.srgb_decode(b), f))
                    GradientSpace.Oklab => self.from_oklab(self.to_oklab(a).mix(self.to_oklab(b), f))
                    _ => a.mix(b, f)
                }
                return out
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let f = clamp(self.pos.x, 0.0, 1.0)
                let base = self.blend_space(self.color0.rgb, self.color1.rgb, f)
                let lit = base * mix(self.intensity.x, self.intensity.y, f)
                // A display shows no more than white; the HDR value is
                // clipped for the picture only.
                let shown = min(lit, vec3(1.0, 1.0, 1.0))
                let alpha = mix(self.color0.w, self.color1.w, f)
                // The checker is laid out in the bar's frame, not the
                // piece's, so it runs on unbroken from one piece to the next.
                let p = self.rect_pos + self.pos * self.rect_size
                let odd = modf(floor(p.x / self.checker_size) + floor(p.y / self.checker_size), 2.0)
                let back = self.checker_light.rgb.mix(self.checker_dark.rgb, odd)
                let band = 1.0 - step(self.alpha_px, self.pos.y * h)
                let rgb = shown.mix(back.mix(shown, alpha), band)
                // Inner edges run past the quad, so two pieces meet with no
                // anti-aliased seam; only the two ends of the bar are shaped.
                let l = mix(-2.0, 0.0, self.ends.x)
                let r = mix(w + 2.0, w, self.ends.y)
                sdf.box_x(l, 0.0, r - l, h, self.radius * self.ends.x, self.radius * self.ends.y)
                sdf.fill(vec4(rgb, 1.0))
                return sdf.result
            }
        }

        draw_mark +: {
            border_color: uniform(theme.color_outline)
            hot_color: uniform(theme.color_on_surface_variant)
            selected_color: uniform(theme.color_primary)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let up = self.point_up
                // A body away from the bar and a point towards it: a colour
                // mark hangs under the bar pointing up, an alpha mark stands
                // over it pointing down.
                let reach = min(w * 0.5 - 1.0, h * 0.4)
                sdf.box(1.0, 1.0 + reach * up, w - 2.0, h - 2.0 - reach, 1.0)
                sdf.pointer(
                    w * 0.5
                    mix(h - 1.0 - reach, 1.0 + reach, up)
                    w * 0.5
                    mix(h - 1.0, 1.0, up)
                )
                let line = self.border_color
                    .mix(self.hot_color, self.hot)
                    .mix(self.selected_color, self.selected)
                // A mark on its way out is drawn faint: letting go here
                // removes it.
                let keep = 1.0 - 0.6 * self.ghost
                sdf.fill_keep(vec4(self.fill_color.rgb, self.fill_color.a * keep))
                sdf.stroke(vec4(line.rgb, line.a * keep), 0.75 + 0.5 * self.selected)
                return sdf.result
            }
        }

        // A slot, not a named child: the editor draws and wires it itself.
        fields: View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{y: 0.5}
            picker := ColorPickerButton{with_alpha: false with_recent: true}
            Label{text: "Location"}
            position := ValueInput{min: 0. max: 100. step: 0.5 precision: 1. suffix: "%"}
            amount_group := View{
                width: Fit
                height: Fit
                flow: Right
                spacing: theme.space_2
                align: Align{y: 0.5}
                amount_label := Label{text: "Intensity"}
                amount := ValueInput{min: 1. max: 16. step: 0.05 precision: 2.}
            }
        }
    }
}

// ===========================================================================
// Draw shaders
// ===========================================================================

/// The strip behind the bar and the marks: the ring round the bar, and the
/// one area every press on the strip is tested against.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGradientStrip {
    #[deref]
    draw_super: DrawQuad,
    /// The bar's rect inside this quad: x, y, width, height.
    #[live]
    bar: Vec4f,
    /// 1 while the editor has the keyboard.
    #[live]
    focus: f32,
}

/// One stretch of the bar, blended from its two ends in the shader.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGradientBar {
    #[deref]
    draw_super: DrawQuad,
    /// sRGB colour and alpha at the left end and the right end.
    #[live]
    color0: Vec4f,
    #[live]
    color1: Vec4f,
    /// Intensity at the left end (x) and the right end (y).
    #[live]
    intensity: Vec2f,
    /// 1 where this piece is the bar's left end (x) or right end (y), which
    /// are the only corners that are rounded.
    #[live]
    ends: Vec2f,
    /// How far down from the top the alpha checker runs, in pixels.
    #[live]
    alpha_px: f32,
    #[live]
    space: GradientSpace,
}

/// One mark.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGradientMark {
    #[deref]
    draw_super: DrawQuad,
    /// The colour the body is filled with: the stop's colour, or the alpha
    /// as a grey.
    #[live]
    fill_color: Vec4f,
    /// 1 for a mark under the bar pointing up, 0 for one over it pointing
    /// down.
    #[live]
    point_up: f32,
    #[live]
    selected: f32,
    #[live]
    hot: f32,
    /// 1 for a mark being dragged away, which letting go removes.
    #[live]
    ghost: f32,
}

// ===========================================================================
// Geometry
// ===========================================================================

/// Where everything sits on the strip, from its rect. The hit test and the
/// drawing both come through here, so what is drawn is what is grabbable.
#[derive(Copy, Clone, Debug)]
struct StripGeo {
    rect: Rect,
    mark_w: f64,
    mark_h: f64,
    bar_h: f64,
}

/// How far outside a mark's rect a press still takes it.
const MARK_SLOP: f64 = 2.0;

impl StripGeo {
    /// The bar: inset by half a mark on either side so marks at both ends
    /// stay inside the strip.
    fn bar(&self) -> Rect {
        Rect {
            pos: dvec2(
                self.rect.pos.x + self.mark_w * 0.5,
                self.rect.pos.y + self.mark_h,
            ),
            size: dvec2((self.rect.size.x - self.mark_w).max(1.0), self.bar_h),
        }
    }

    fn x_of(&self, t: f64) -> f64 {
        let bar = self.bar();
        bar.pos.x + unit(t) * bar.size.x
    }

    fn t_of(&self, x: f64) -> f64 {
        let bar = self.bar();
        unit((x - bar.pos.x) / bar.size.x)
    }

    fn mark_rect(&self, kind: MarkKind, t: f64) -> Rect {
        let y = match kind {
            MarkKind::Alpha => self.rect.pos.y,
            MarkKind::Color => self.rect.pos.y + self.mark_h + self.bar_h,
        };
        Rect {
            pos: dvec2(self.x_of(t) - self.mark_w * 0.5, y),
            size: dvec2(self.mark_w, self.mark_h),
        }
    }

    /// How far above or below the strip a point is; 0 inside it.
    fn outside_by(&self, y: f64) -> f64 {
        let top = self.rect.pos.y;
        let bottom = top + self.rect.size.y;
        if y < top {
            top - y
        } else if y > bottom {
            y - bottom
        } else {
            0.0
        }
    }

    /// The mark under a point: the nearest one whose rect (with a little
    /// slop) holds it, the selected mark winning a tie and then the one
    /// drawn on top.
    fn mark_at(
        &self,
        g: &Gradient,
        p: DVec2,
        prefer: Option<GradientMark>,
    ) -> Option<GradientMark> {
        let mut best: Option<(f64, GradientMark)> = None;
        let mut consider = |mark: GradientMark, t: f64| {
            let r = self.mark_rect(mark.kind(), t);
            let r = Rect {
                pos: dvec2(r.pos.x - MARK_SLOP, r.pos.y - MARK_SLOP),
                size: dvec2(r.size.x + MARK_SLOP * 2.0, r.size.y + MARK_SLOP * 2.0),
            };
            if !r.contains(p) {
                return;
            }
            let d = (p.x - self.x_of(t)).abs();
            // Marks are offered in drawing order, so on a tie the later one
            // is the one on top, unless the earlier one is selected.
            let better = match best {
                None => true,
                Some((bd, bm)) => {
                    if (d - bd).abs() < 1e-9 {
                        Some(bm) != prefer
                    } else {
                        d < bd
                    }
                }
            };
            if better {
                best = Some((d, mark));
            }
        };
        for (i, s) in g.alpha_stops.iter().enumerate() {
            consider(GradientMark::Alpha(i), s.t);
        }
        for (i, s) in g.stops.iter().enumerate() {
            consider(GradientMark::Color(i), s.t);
        }
        best.map(|(_, m)| m)
    }

    /// What kind of mark a press on bare strip adds: alpha over the bar and
    /// in the bar's checkered band, colour under it and in the solid rest.
    fn kind_at(&self, p: DVec2, alpha_band: f64) -> Option<MarkKind> {
        if !self.rect.contains(p) {
            return None;
        }
        let bar = self.bar();
        let split = bar.pos.y + bar.size.y * alpha_band.clamp(0.0, 1.0);
        Some(if p.y < split {
            MarkKind::Alpha
        } else {
            MarkKind::Color
        })
    }
}

// ===========================================================================
// The widget
// ===========================================================================

/// What the DSL declared, as last taken up. The ramp is only rebuilt from
/// the declaration when the declaration itself changes, so an unrelated
/// re-apply (a theme switch) does not throw away what was edited by hand.
#[derive(Clone, Debug, PartialEq)]
struct Declared {
    stops: Vec<ColorStop>,
    alpha_stops: Vec<AlphaStop>,
    preset: GradientPreset,
    space: GradientSpace,
}

/// A press in flight on a mark.
#[derive(Clone, Debug)]
struct Drag {
    kind: MarkKind,
    /// The mark's index while it is in the ramp.
    index: usize,
    /// From the mark's centre to the press, so the mark moves under the
    /// finger rather than jumping to it.
    offset: f64,
    /// The stop, while it is dragged far enough away to be removed.
    held: Option<HeldStop>,
    /// Where the finger is, for drawing a held mark.
    at: DVec2,
    /// Whether this gesture has changed the ramp.
    dirty: bool,
}

/// What the field row was last given, so it is only rewritten when it
/// would show something new.
#[derive(Clone, Debug, PartialEq)]
struct FieldState {
    kind: Option<MarkKind>,
    t: f64,
    color: Vec4f,
    amount: f64,
    max_intensity: f64,
    with_intensity: bool,
}

#[derive(Script, WidgetRegister, WidgetRef, WidgetSet)]
pub struct GradientEditor {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawGradientStrip,
    #[live]
    draw_bar: DrawGradientBar,
    #[live]
    draw_mark: DrawGradientMark,

    /// The row of fields for the selected mark.
    #[live]
    pub fields: WidgetRef,

    /// The colour stops as declared. Empty takes them from `preset`.
    #[live]
    pub stops: Vec<ColorStop>,
    /// The alpha stops as declared. Empty takes them from `preset`.
    #[live]
    pub alpha_stops: Vec<AlphaStop>,
    #[live]
    pub space: GradientSpace,
    #[live]
    pub preset: GradientPreset,

    /// Mark and bar geometry. Rust owns these because the hit test needs
    /// them; the shaders are handed the rects every draw.
    #[live(11.0)]
    pub mark_width: f64,
    #[live(15.0)]
    pub mark_height: f64,
    #[live(26.0)]
    pub bar_height: f64,
    #[live(0.5)]
    pub alpha_band: f64,
    #[live(28.0)]
    pub remove_distance: f64,
    #[live(16.0)]
    pub max_intensity: f64,
    #[live(0.01)]
    pub nudge: f64,
    #[live(true)]
    pub with_intensity: bool,
    #[live(true)]
    pub show_fields: bool,
    #[live(true)]
    visible: bool,

    #[rust]
    gradient: Gradient,
    #[rust]
    adopted: Option<Declared>,
    #[rust]
    selected: Option<GradientMark>,
    #[rust]
    hot: Option<GradientMark>,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    focused: bool,
    #[rust]
    synced: Option<FieldState>,
    #[rust]
    whole: Area,
}

impl ScriptHook for GradientEditor {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.adopt();
    }
}

impl GradientEditor {
    /// Take up the declaration if it changed since it was last taken up.
    fn adopt(&mut self) {
        let now = Declared {
            stops: self.stops.clone(),
            alpha_stops: self.alpha_stops.clone(),
            preset: self.preset,
            space: self.space,
        };
        match &self.adopted {
            Some(prev)
                if prev.stops == now.stops
                    && prev.alpha_stops == now.alpha_stops
                    && prev.preset == now.preset =>
            {
                if prev.space != now.space {
                    self.gradient.space = now.space;
                }
            }
            _ => {
                let base = Gradient::preset(now.preset);
                let stops = if now.stops.is_empty() {
                    base.stops
                } else {
                    now.stops.clone()
                };
                let alpha = if now.alpha_stops.is_empty() {
                    base.alpha_stops
                } else {
                    now.alpha_stops.clone()
                };
                self.gradient = Gradient::new(stops, alpha, now.space);
                self.selected = Some(GradientMark::Color(0));
                self.drag = None;
                self.hot = None;
                self.synced = None;
            }
        }
        self.adopted = Some(now);
    }

    pub fn gradient(&self) -> &Gradient {
        &self.gradient
    }

    /// Replace the ramp. The selection keeps its kind and is pulled back
    /// inside the new lists.
    pub fn set_gradient(&mut self, cx: &mut Cx, gradient: Gradient) {
        let mut gradient = gradient;
        gradient.normalize();
        self.gradient = gradient;
        self.drag = None;
        self.hot = None;
        self.selected = self.selected.map(|m| self.clamp_mark(m));
        self.sync_fields(cx);
        self.redraw(cx);
    }

    pub fn selected(&self) -> Option<GradientMark> {
        self.selected
    }

    pub fn select(&mut self, cx: &mut Cx, mark: Option<GradientMark>) {
        let mark = mark.map(|m| self.clamp_mark(m));
        if self.selected != mark {
            self.selected = mark;
            cx.widget_action(self.uid, GradientEditorAction::Selected(mark));
            self.sync_fields(cx);
            self.draw_bg.redraw(cx);
        }
    }

    fn clamp_mark(&self, mark: GradientMark) -> GradientMark {
        let n = self.gradient.count(mark.kind()).max(1);
        GradientMark::new(mark.kind(), mark.index().min(n - 1))
    }

    fn geometry(&self, rect: Rect) -> StripGeo {
        StripGeo {
            rect,
            mark_w: self.mark_width.max(3.0),
            mark_h: self.mark_height.max(3.0),
            bar_h: self.bar_height.max(2.0),
        }
    }

    fn strip_height(&self) -> f64 {
        self.mark_height.max(3.0) * 2.0 + self.bar_height.max(2.0)
    }

    fn changed(&mut self, cx: &mut Cx) {
        cx.widget_action(
            self.uid,
            GradientEditorAction::Changed(self.gradient.clone()),
        );
    }

    fn ended(&mut self, cx: &mut Cx) {
        cx.widget_action(self.uid, GradientEditorAction::Ended(self.gradient.clone()));
    }

    /// Report one discrete edit: the change and its end together.
    fn committed(&mut self, cx: &mut Cx) {
        self.changed(cx);
        self.ended(cx);
        self.sync_fields(cx);
        self.draw_bg.redraw(cx);
    }

    fn open_picker(&mut self, cx: &mut Cx) {
        if !self.show_fields {
            return;
        }
        if let Some(GradientMark::Color(_)) = self.selected {
            self.sync_fields(cx);
            if let Some(mut picker) = self.field(live_id!(picker)).borrow_mut::<ColorPicker>() {
                picker.open_panel(cx);
            }
        }
    }

    fn field(&self, id: LiveId) -> WidgetRef {
        self.fields.child(id)
    }

    /// The amount label and field, which hide together.
    fn amount_group(&self) -> WidgetRef {
        self.fields.child(live_id!(amount_group))
    }

    /// Remove the selected mark, and select its neighbour of the same kind.
    fn remove_selected(&mut self, cx: &mut Cx) {
        let Some(mark) = self.selected else {
            return;
        };
        if self.gradient.remove(mark) {
            let n = self.gradient.count(mark.kind());
            self.selected = None;
            self.select(
                cx,
                Some(GradientMark::new(mark.kind(), mark.index().min(n - 1))),
            );
            self.committed(cx);
        }
    }

    /// Push the selected mark into the field row, if it would show
    /// something new. Never while a field is being edited: ValueInput
    /// refuses a value under the hand, and the picker is only given a colour
    /// that differs from its own.
    fn sync_fields(&mut self, cx: &mut Cx) {
        if !self.show_fields || self.fields.is_empty() {
            return;
        }
        // While a mark is held off the bar the row keeps showing it: it
        // comes back if the drag does, and a row that emptied and refilled
        // under a passing drag would jump about for nothing.
        if self.drag.as_ref().map_or(false, |d| d.held.is_some()) {
            return;
        }
        let kind = self.selected.map(|m| m.kind());
        let (t, color, amount) = match self.selected {
            Some(GradientMark::Color(i)) => match self.gradient.stops.get(i) {
                Some(s) => (s.t, s.color, s.intensity as f64),
                None => (0.0, Vec4f::default(), 0.0),
            },
            Some(GradientMark::Alpha(i)) => match self.gradient.alpha_stops.get(i) {
                Some(s) => (s.t, Vec4f::default(), s.alpha as f64 * 100.0),
                None => (0.0, Vec4f::default(), 0.0),
            },
            None => (0.0, Vec4f::default(), 0.0),
        };
        let state = FieldState {
            kind,
            t,
            color,
            amount,
            max_intensity: self.max_intensity,
            with_intensity: self.with_intensity,
        };
        if self.synced.as_ref() == Some(&state) {
            return;
        }
        let kind_changed = self
            .synced
            .as_ref()
            .map(|s| (s.kind, s.max_intensity, s.with_intensity))
            != Some((kind, self.max_intensity, self.with_intensity));
        self.synced = Some(state);

        let picker = self.field(live_id!(picker));
        let group = self.amount_group();
        let amount_field = group.child(live_id!(amount));
        if kind_changed {
            let is_color = kind == Some(MarkKind::Color);
            if !is_color {
                // Closed before it is hidden: a hidden picker neither draws
                // its panel nor hears the press that would close it.
                if let Some(mut p) = picker.borrow_mut::<ColorPicker>() {
                    p.close_panel(cx, false);
                }
            }
            picker.set_visible(cx, is_color);
            let (label, min, max, step, precision, suffix) = match kind {
                Some(MarkKind::Alpha) => ("Alpha", 0.0, 100.0, 1.0, 0.0, "%"),
                _ => (
                    "Intensity",
                    1.0,
                    self.max_intensity.max(1.0),
                    0.05,
                    2.0,
                    "\u{d7}",
                ),
            };
            let show_amount = kind == Some(MarkKind::Alpha)
                || (kind == Some(MarkKind::Color) && self.with_intensity);
            group.child(live_id!(amount_label)).set_text(cx, label);
            group.set_visible(cx, show_amount);
            if let Some(mut f) = amount_field.borrow_mut::<ValueInput>() {
                f.min = min;
                f.max = max;
                f.step = step;
                f.precision = precision;
                f.suffix = suffix.to_string();
            }
        }
        if kind == Some(MarkKind::Color) {
            if let Some(mut p) = picker.borrow_mut::<ColorPicker>() {
                p.set_color(cx, color);
            }
        }
        if kind.is_some() {
            if let Some(mut f) = self.field(live_id!(position)).borrow_mut::<ValueInput>() {
                f.set_value(cx, t * 100.0);
            }
            if let Some(mut f) = amount_field.borrow_mut::<ValueInput>() {
                f.set_value(cx, amount);
            }
        }
        self.fields.redraw(cx);
    }

    /// Fold what the field row reported back into the ramp.
    fn take_field_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let picker_uid = self.field(live_id!(picker)).widget_uid();
        let position_uid = self.field(live_id!(position)).widget_uid();
        let amount_uid = self.amount_group().child(live_id!(amount)).widget_uid();
        let mut changed = false;
        let mut ended = false;
        for action in actions {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            let Some(mark) = self.selected else {
                continue;
            };
            if wa.widget_uid == picker_uid {
                let GradientMark::Color(i) = mark else {
                    continue;
                };
                match wa.cast::<ColorAction>() {
                    ColorAction::Changed(c) => {
                        self.gradient.set_stop_color(i, c);
                        changed = true;
                    }
                    ColorAction::Ended(c) => {
                        self.gradient.set_stop_color(i, c);
                        changed = true;
                        ended = true;
                    }
                    _ => {}
                }
            } else if wa.widget_uid == position_uid {
                if let ValueInputAction::Changed(v) = wa.cast::<ValueInputAction>() {
                    let moved = self.gradient.move_mark(mark, v / 100.0);
                    self.select(cx, Some(moved));
                    changed = true;
                    ended = true;
                }
            } else if wa.widget_uid == amount_uid {
                if let ValueInputAction::Changed(v) = wa.cast::<ValueInputAction>() {
                    match mark {
                        GradientMark::Color(i) => self
                            .gradient
                            .set_stop_intensity(i, v.min(self.max_intensity.max(1.0)) as f32),
                        GradientMark::Alpha(i) => self.gradient.set_alpha(i, (v / 100.0) as f32),
                    }
                    changed = true;
                    ended = true;
                }
            }
        }
        if changed {
            self.changed(cx);
            if ended {
                self.ended(cx);
            }
            self.sync_fields(cx);
            self.draw_bg.redraw(cx);
        }
    }

    fn draw_strip(&mut self, cx: &mut Cx2d, rect: Rect) {
        let geo = self.geometry(rect);
        let bar = geo.bar();

        self.draw_bg.bar = vec4(
            (bar.pos.x - rect.pos.x) as f32,
            (bar.pos.y - rect.pos.y) as f32,
            bar.size.x as f32,
            bar.size.y as f32,
        );
        self.draw_bg.focus = if self.focused { 1.0 } else { 0.0 };
        self.draw_bg.draw_abs(cx, rect);

        // The bar, one quad per stretch between stops.
        let pieces = self.gradient.pieces();
        let last = pieces.len().saturating_sub(1);
        self.draw_bar.space = self.gradient.space;
        self.draw_bar.alpha_px = (bar.size.y * self.alpha_band.clamp(0.0, 1.0)) as f32;
        for (k, piece) in pieces.iter().enumerate() {
            let x0 = bar.pos.x + piece.t0 * bar.size.x;
            let x1 = bar.pos.x + piece.t1 * bar.size.x;
            self.draw_bar.color0 = piece.c0;
            self.draw_bar.color1 = piece.c1;
            self.draw_bar.intensity = vec2(piece.i0, piece.i1);
            self.draw_bar.ends = vec2(
                if k == 0 { 1.0 } else { 0.0 },
                if k == last { 1.0 } else { 0.0 },
            );
            self.draw_bar.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x0, bar.pos.y),
                    size: dvec2((x1 - x0).max(0.0), bar.size.y),
                },
            );
        }

        // The marks, the selected one last so it sits on top.
        let mut marks: Vec<(GradientMark, f64, Vec4f)> = Vec::new();
        for (i, s) in self.gradient.alpha_stops.iter().enumerate() {
            marks.push((
                GradientMark::Alpha(i),
                s.t,
                vec4(s.alpha, s.alpha, s.alpha, 1.0),
            ));
        }
        for (i, s) in self.gradient.stops.iter().enumerate() {
            marks.push((GradientMark::Color(i), s.t, s.color));
        }
        if let Some(sel) = self.selected {
            if let Some(at) = marks.iter().position(|(m, _, _)| *m == sel) {
                let m = marks.remove(at);
                marks.push(m);
            }
        }
        for (mark, t, fill) in marks {
            self.draw_mark.fill_color = vec4(fill.x, fill.y, fill.z, 1.0);
            self.draw_mark.point_up = if mark.kind() == MarkKind::Color {
                1.0
            } else {
                0.0
            };
            self.draw_mark.selected = if Some(mark) == self.selected {
                1.0
            } else {
                0.0
            };
            self.draw_mark.hot = if Some(mark) == self.hot { 1.0 } else { 0.0 };
            self.draw_mark.ghost = 0.0;
            self.draw_mark.draw_abs(cx, geo.mark_rect(mark.kind(), t));
        }

        // A mark dragged far enough away to be removed follows the finger,
        // faint.
        if let Some(drag) = &self.drag {
            if let Some(held) = drag.held {
                let (fill, up) = match held {
                    HeldStop::Color(s) => (vec4(s.color.x, s.color.y, s.color.z, 1.0), 1.0),
                    HeldStop::Alpha(s) => (vec4(s.alpha, s.alpha, s.alpha, 1.0), 0.0),
                };
                self.draw_mark.fill_color = fill;
                self.draw_mark.point_up = up;
                self.draw_mark.selected = 0.0;
                self.draw_mark.hot = 0.0;
                self.draw_mark.ghost = 1.0;
                let x = drag.at.x - drag.offset;
                self.draw_mark.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x - geo.mark_w * 0.5, drag.at.y - geo.mark_h * 0.5),
                        size: dvec2(geo.mark_w, geo.mark_h),
                    },
                );
            }
        }
    }

    fn handle_strip(&mut self, cx: &mut Cx, event: &Event) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let geo = self.geometry(fe.rect);
                let hot = geo.mark_at(&self.gradient, fe.abs, self.selected);
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(if hot.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Crosshair
                });
            }
            Hit::FingerHoverOut(_) => {
                if self.hot.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                let geo = self.geometry(fe.rect);
                let p = fe.abs;
                if let Some(mark) = geo.mark_at(&self.gradient, p, self.selected) {
                    self.select(cx, Some(mark));
                    let t = self.gradient.mark_t(mark).unwrap_or(0.0);
                    self.drag = Some(Drag {
                        kind: mark.kind(),
                        index: mark.index(),
                        offset: p.x - geo.x_of(t),
                        held: None,
                        at: p,
                        dirty: false,
                    });
                    if fe.tap_count >= 2 && mark.kind() == MarkKind::Color {
                        self.open_picker(cx);
                    }
                } else if let Some(kind) = geo.kind_at(p, self.alpha_band) {
                    // A press on bare strip adds a mark that changes
                    // nothing yet, and the drag carries on with it.
                    let mark = self.gradient.add_sampled(kind, geo.t_of(p.x));
                    self.select(cx, Some(mark));
                    self.drag = Some(Drag {
                        kind,
                        index: mark.index(),
                        offset: 0.0,
                        held: None,
                        at: p,
                        dirty: true,
                    });
                    self.changed(cx);
                    self.sync_fields(cx);
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let Some(mut drag) = self.drag.take() else {
                    return;
                };
                let geo = self.geometry(fe.rect);
                let t = geo.t_of(fe.abs.x - drag.offset);
                let away = geo.outside_by(fe.abs.y) > self.remove_distance;
                drag.at = fe.abs;
                let mut changed = false;
                match drag.held.take() {
                    None => {
                        let mark = GradientMark::new(drag.kind, drag.index);
                        if away && self.gradient.count(drag.kind) > 1 {
                            drag.held = self.gradient.take(mark);
                            // Not through `select`: the field row keeps the
                            // mark while it is held, see `sync_fields`.
                            self.selected = None;
                            cx.widget_action(self.uid, GradientEditorAction::Selected(None));
                            changed = true;
                        } else if self.gradient.mark_t(mark) != Some(t) {
                            let moved = self.gradient.move_mark(mark, t);
                            drag.index = moved.index();
                            self.select(cx, Some(moved));
                            changed = true;
                        }
                    }
                    Some(held) => {
                        if away {
                            drag.held = Some(held);
                        } else {
                            let mark = self.gradient.put(held, t);
                            drag.index = mark.index();
                            self.select(cx, Some(mark));
                            changed = true;
                        }
                    }
                }
                drag.dirty |= changed;
                self.drag = Some(drag);
                if changed {
                    self.changed(cx);
                    self.sync_fields(cx);
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(_) => {
                if let Some(drag) = self.drag.take() {
                    if drag.held.is_some() {
                        // Let go far from the bar: the mark is gone. Its
                        // neighbour of the same kind takes the selection.
                        let n = self.gradient.count(drag.kind);
                        self.select(
                            cx,
                            Some(GradientMark::new(drag.kind, drag.index.min(n - 1))),
                        );
                    }
                    if drag.dirty {
                        self.ended(cx);
                    }
                    self.sync_fields(cx);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::KeyFocus(_) => {
                self.focused = true;
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.focused = false;
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let step = if ke.modifiers.shift {
                    self.nudge * 10.0
                } else {
                    self.nudge
                };
                match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                        if let Some(mark) = self.selected {
                            let dir = if ke.key_code == KeyCode::ArrowLeft {
                                -1.0
                            } else {
                                1.0
                            };
                            let t = self.gradient.mark_t(mark).unwrap_or(0.0);
                            let next = unit(t + dir * step);
                            if next != t {
                                let moved = self.gradient.move_mark(mark, next);
                                self.select(cx, Some(moved));
                                self.committed(cx);
                            }
                        }
                    }
                    KeyCode::Delete | KeyCode::Backspace => {
                        self.remove_selected(cx);
                    }
                    KeyCode::ReturnKey => {
                        self.open_picker(cx);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl WidgetNode for GradientEditor {
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
        self.draw_bg.redraw(cx);
    }

    /// The field row under its own name, so `ids!(editor.fields.amount)`
    /// reaches the amount field.
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if self.show_fields && !self.fields.is_empty() {
            visit(live_id!(fields), self.fields.clone());
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        if self.show_fields {
            self.fields.find_widgets_from_point(cx, point, found);
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
        if matches!(self.whole, Area::Empty) {
            cx.redraw_all();
        } else {
            self.whole.redraw(cx);
        }
    }
}

impl Widget for GradientEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.adopt();
        // Pushed BEFORE the fields draw: a redraw asked for during a draw is
        // dropped.
        self.sync_fields(cx.cx.cx);

        let mut layout = self.layout;
        layout.flow = Flow::Down;
        // Measured before the turtle opens, and handed down as a FIXED width:
        // the fields ask for Fill, and a Fill inside a Fit is laid out and
        // never painted.
        let outer = cx.turtle().next_walk_width(walk.width, walk.margin);
        let padding = layout.padding.left + layout.padding.right;
        let inner = inner_width(outer, padding, 320.0);

        cx.begin_turtle(walk, layout);
        let rect = cx.walk_turtle(Walk {
            width: Size::Fixed(inner),
            height: Size::Fixed(self.strip_height()),
            ..Walk::default()
        });
        self.draw_strip(cx, rect);
        if self.show_fields && !self.fields.is_empty() {
            cx.widget_tree_insert_child(self.uid, live_id!(fields), self.fields.clone());
            let fields_walk = Walk {
                width: Size::Fixed(inner),
                height: Size::fit(),
                ..Walk::default()
            };
            let _ = self.fields.draw_walk(cx, scope, fields_walk);
        }
        cx.end_turtle_with_area(&mut self.whole);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible {
            return;
        }
        // The fields first. The picker's panel hangs over the page and may
        // cover the strip, so it must see a press before the strip does;
        // and a double press that opens the panel must not be seen by the
        // panel afterwards as a press outside it, which would close it again.
        if self.show_fields && !self.fields.is_empty() {
            let actions = cx.capture_actions(|cx| self.fields.handle_event(cx, event, scope));
            if !actions.is_empty() {
                self.take_field_actions(cx, &actions);
            }
        }
        self.handle_strip(cx, event);
    }

    fn text(&self) -> String {
        self.gradient.describe()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.gradient.describe())
    }
}

impl GradientEditorRef {
    /// The ramp as it stands.
    pub fn gradient(&self) -> Gradient {
        self.borrow()
            .map(|inner| inner.gradient.clone())
            .unwrap_or_default()
    }

    pub fn set_gradient(&self, cx: &mut Cx, gradient: Gradient) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_gradient(cx, gradient);
        }
    }

    /// Replace the ramp with a preset, keeping the current colour space.
    pub fn set_preset(&self, cx: &mut Cx, preset: GradientPreset) {
        if let Some(mut inner) = self.borrow_mut() {
            let mut g = Gradient::preset(preset);
            g.space = inner.gradient.space;
            inner.set_gradient(cx, g);
        }
    }

    /// The ramp at `t`: colour times intensity, alpha from the alpha stops.
    pub fn sample(&self, t: f64) -> Vec4f {
        self.borrow()
            .map(|inner| inner.gradient.sample(t))
            .unwrap_or_default()
    }

    pub fn selected_mark(&self) -> Option<GradientMark> {
        self.borrow().and_then(|inner| inner.selected)
    }

    pub fn select(&self, cx: &mut Cx, mark: Option<GradientMark>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.select(cx, mark);
        }
    }

    /// The latest ramp while it moves under the hand.
    pub fn changed(&self, actions: &Actions) -> Option<Gradient> {
        let mut last = None;
        for action in actions.filter_widget_actions_cast::<GradientEditorAction>(self.widget_uid())
        {
            if let GradientEditorAction::Changed(g) = action {
                last = Some(g);
            }
        }
        last
    }

    /// The ramp a gesture or an edit settled on. One per gesture.
    pub fn ended(&self, actions: &Actions) -> Option<Gradient> {
        let mut last = None;
        for action in actions.filter_widget_actions_cast::<GradientEditorAction>(self.widget_uid())
        {
            if let GradientEditorAction::Ended(g) = action {
                last = Some(g);
            }
        }
        last
    }

    /// The new selection, when it changed.
    pub fn selected(&self, actions: &Actions) -> Option<Option<GradientMark>> {
        let mut last = None;
        for action in actions.filter_widget_actions_cast::<GradientEditorAction>(self.widget_uid())
        {
            if let GradientEditorAction::Selected(m) = action {
                last = Some(m);
            }
        }
        last
    }
}

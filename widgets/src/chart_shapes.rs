//! Six more chart shapes: pie, donut, radial bar, funnel, radar and bubble.
//!
//! [`crate::chart`] plots a SERIES against axes — a line, an area, bars,
//! dots, candles — and everything on that page answers "what happened
//! next". The shapes here answer a different question and so want a
//! different picture: how a whole divides, how far each of a few things
//! got, how a stage-by-stage count falls away, how one thing scores on
//! several scales at once, and where a handful of named points sit when
//! each carries a weight as well as a position. There is no time axis on
//! this page and nothing to pan.
//!
//! # Where the data comes from
//!
//! One line of markup per part: `"Rent 420"`, `"Public transport 120"`,
//! `"Speed: 4 3 5 2"`. The words are the label, the numbers are the
//! numbers, and a colon separates the two when a label would otherwise
//! swallow one. A list of strings is what markup can carry here — a list of
//! numbers is not a live type — and writing the data as lines keeps it
//! readable in the file that declares it. `set_rows` takes the same thing
//! from Rust. The reader is [`crate::chart::parse_row`], the one every chart
//! in the library reads its lines through.
//!
//! # How the shapes are drawn
//!
//! A wedge or an arc is two comparisons per pixel — a radius test and an
//! angle test — over one quad; a funnel band is one interpolation and one
//! comparison; a radar's face is a fan of triangles, each a quad with three
//! half-plane tests. Nothing here is an outline walked as a path, because
//! paths do not paint reliably in this renderer, and the whole of any one
//! chart is a handful of draw calls.
//!
//! Angles are radians measured CLOCKWISE FROM STRAIGHT UP, in [`Wedges`] and
//! in the shader alike. One measure in one place is what stops the wedge
//! that is seen and the wedge a pointer picks from drifting apart.
//!
//! # What this deliberately does not do
//!
//! **It does not invent data.** A chart given nothing draws its empty frame
//! — the track ring, the grid — and stops. The plots in [`crate::chart`]
//! fabricate a plausible series when they are handed none, which makes a
//! chart whose data never arrived look exactly like one that is working;
//! that is a trap worth not repeating, and an empty circle is a true
//! picture of an empty list.
//!
//! It draws no legend. A legend is a list of labels beside a picture, which
//! is a layout decision belonging to whatever is placing the chart, and
//! every shape here labels its own parts in place where there is room for
//! it.
//!
//! It does not sort, group or roll up a long tail into "other". The order
//! on screen is the order the lines were written, so a caller can point at
//! part three and mean the third line.
//!
//! And it does not animate. A wedge that grows on first draw is a wedge
//! that is the wrong size for a moment, and a chart is read in that moment
//! as often as any other.
use crate::{
    badge::measure,
    chart::{fmt_value, parse_rows, Row},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};
use std::f64::consts::TAU;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawWedge::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            if self.span <= 0.0 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let c = self.rect_size * 0.5
            let p = self.pos * self.rect_size - c
            let r = length(p)
            if r < self.inner || r > self.outer {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            // Clockwise from straight up: the pixel grid grows downward, so
            // up is the negative y, and atan2 counts the other way round.
            let mut a = atan2(p.x, 0.0 - p.y)
            if a < 0.0 {
                a = a + 2.0 * PI
            }
            let mut t = a - self.a0
            if t < 0.0 {
                t = t + 2.0 * PI
            }
            if t > self.span {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            // The sides are as far away in points as the arc they cut, so
            // one distance in pixels feathers all four edges the same.
            let mut side = min(t, self.span - t) * r
            // A ring that goes all the way round has no sides to feather,
            // and feathering the seam where it closes draws a hairline
            // across it.
            if self.span > 2.0 * PI - 0.001 {
                side = 1000.0
            }
            let edge = min(min(r - self.inner, self.outer - r), side)
            let aa = clamp(edge, 0.0, 1.5) / 1.5
            let col = self.color + vec4(0.1, 0.1, 0.1, 0.0) * self.hot
            // Premultiplied, like everything an Sdf2d returns: this shader
            // builds its colour by hand rather than filling a shape, so it
            // has to do for itself what fill() would have done, or the
            // feathered edges come back brighter than the wedge they edge.
            return Pal.premul(vec4(col.xyz, col.w * aa))
        }
    }

    set_type_default() do #(DrawBand::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let p = self.pos * self.rect_size
            let t = p.y / max(self.rect_size.y, 1.0)
            let half = mix(self.top_w, self.bottom_w, t) * 0.5
            let d = half - abs(p.x - self.rect_size.x * 0.5)
            let aa = clamp(d, 0.0, 1.0)
            let col = self.color + vec4(0.1, 0.1, 0.1, 0.0) * self.hot
            return Pal.premul(vec4(col.xyz, col.w * aa))
        }
    }

    set_type_default() do #(DrawTri::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let p = self.pos * self.rect_size
            let ab = self.p1 - self.p0
            let bc = self.p2 - self.p1
            let ca = self.p0 - self.p2
            // Twice the signed area turns all three edge tests the same way
            // round, so a caller may hand its points over in whichever
            // order it walked them.
            let mut s = 1.0
            if ab.x * (self.p2.y - self.p0.y) - ab.y * (self.p2.x - self.p0.x) < 0.0 {
                s = 0.0 - 1.0
            }
            let d0 = (ab.x * (p.y - self.p0.y) - ab.y * (p.x - self.p0.x)) * s / max(length(ab), 0.001)
            let d1 = (bc.x * (p.y - self.p1.y) - bc.y * (p.x - self.p1.x)) * s / max(length(bc), 0.001)
            let d2 = (ca.x * (p.y - self.p2.y) - ca.y * (p.x - self.p2.x)) * s / max(length(ca), 0.001)
            let aa = clamp(min(min(d0, d1), d2) + 0.5, 0.0, 1.0)
            return Pal.premul(vec4(self.color.xyz, self.color.w * aa))
        }
    }

    set_type_default() do #(DrawSeg::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let p = self.pos * self.rect_size
            let pa = p - self.p0
            let ba = self.p1 - self.p0
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
            let d = length(pa - ba * h)
            let aa = 1.0 - smoothstep(self.thickness * 0.5 - 0.75, self.thickness * 0.5 + 0.75, d)
            return vec4(self.color.rgb * self.color.a * aa, self.color.a * aa)
        }
    }

    set_type_default() do #(DrawDot::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            sdf.circle(c.x, c.y, max(min(c.x, c.y) - self.border_size, 0.5))
            sdf.fill_keep(self.color + vec4(0.1, 0.1, 0.1, 0.0) * self.hot)
            sdf.stroke(self.border_color, self.border_size)
            return sdf.result
        }
    }

    mod.widgets.ChartFigureBase = #(ChartFigure::register_widget(vm))

    /** The surface the six shapes are drawn on: the palette, the text, the
     * data lines and the room around the figure. On its own it is an empty
     * box, and it is worth naming only because every shape here reads its
     * colours and its data from the same place. */
    mod.widgets.ChartFigure = set_type_default() do mod.widgets.ChartFigureBase{
        width: Fill
        height: Fill

        /** one line per part: a label and its numbers, "Rent 420" */
        series: []
        /** room left inside the box, in pixels 0..64 step 1 */
        pad: 10.
        /** the gap drawn between two neighbouring parts, in degrees 0..8 step 0.25 */
        gap: 1.
        /** where the first part begins, in degrees clockwise from straight up 0..360 step 5 */
        start_angle: 0.
        /** what a whole turn or a full-width band stands for; 0 uses the largest value present */
        max_value: 0.
        /** name each part on the figure where there is room */
        show_labels: true
        /** put each part's number on the figure where there is room */
        show_values: true

        /** the colours parts are cut from, in order and then round again */
        color_1: theme.color_primary
        color_2: theme.color_tertiary
        color_3: theme.color_success
        color_4: theme.color_warning
        color_5: theme.color_secondary
        color_6: theme.color_info
        /** the empty part of a track, and the ground under a plot */
        color_track: theme.color_surface_container_high
        /** rings, spokes and grid rules */
        color_grid: theme.color_outline_variant
        /** the box behind the figure; transparent, so a chart sits on the page it is placed on */
        color_bg: theme.color_u_hidden
        /** words drawn outside a part, where no part's colour is behind them */
        color_label: theme.color_text_meta

        draw_text +: {
            // 1.0, because every label here is centred by arithmetic that
            // reads the font size as the line's whole height.
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
            color: theme.color_text_meta
        }
        draw_value +: {
            text_style: theme.font_bold{font_size: theme.font_size_p line_spacing: 1.0}
            color: theme.color_text
        }
    }

    mod.widgets.PieChartBase = #(PieChart::register_widget(vm))

    /** A circle cut into one wedge per part, each wedge the part's share of
     * the total. */
    mod.widgets.PieChart = set_type_default() do mod.widgets.PieChartBase{
        width: Fill
        height: Fill
    }

    mod.widgets.DonutChartBase = #(DonutChart::register_widget(vm))

    /** A pie with a hole, and the total — or whatever the pointer is on —
     * read out in the middle of it. */
    mod.widgets.DonutChart = set_type_default() do mod.widgets.DonutChartBase{
        width: Fill
        height: Fill
        /** the hole, as a share of the radius 0..0.9 step 0.02 */
        hole: 0.58
        /** a word under the middle readout — "total", "this month" */
        caption: ""

        draw_centre +: {
            text_style: theme.font_bold{font_size: theme.font_size_2 line_spacing: 1.0}
            color: theme.color_text
        }
    }

    mod.widgets.RadialBarChartBase = #(RadialBarChart::register_widget(vm))

    /** One arc per part, nested, each arc the part's share of a full turn. */
    mod.widgets.RadialBarChart = set_type_default() do mod.widgets.RadialBarChartBase{
        width: Fill
        height: Fill
        /** how thick one track is, in pixels 4..48 step 1 */
        track_size: 16.
        /** the space between two tracks, in pixels 0..24 step 1 */
        track_gap: 6.
        /** the strip on the left the names are written in; 0 centres the rings and drops the names 0..200 step 4 */
        label_width: 96.
    }

    mod.widgets.FunnelChartBase = #(FunnelChart::register_widget(vm))

    /** A stack of bands, each as wide as its part is against the widest. */
    mod.widgets.FunnelChart = set_type_default() do mod.widgets.FunnelChartBase{
        width: Fill
        height: Fill
        /** the space between two bands, in pixels 0..24 step 1 */
        band_gap: 6.
        /** slope each band's bottom edge to meet the next band's top */
        taper: true
    }

    mod.widgets.RadarChartBase = #(RadarChart::register_widget(vm))

    /** One closed face per part over a web of named axes. */
    mod.widgets.RadarChart = set_type_default() do mod.widgets.RadarChartBase{
        width: Fill
        height: Fill
        /** the axes, the first straight up and the rest clockwise */
        axes: []
        /** how many rings the web is drawn with 1..8 step 1 */
        rings: 4
        /** how solid a face is over the web under it 0..1 step 0.05 */
        fill_alpha: 0.28
        /** the outline around a face, in pixels 0..6 step 0.5 */
        line_width: 2.
    }

    mod.widgets.BubbleChartBase = #(BubbleChart::register_widget(vm))

    /** Named points on a plot, each with a weight the circle's AREA shows. */
    mod.widgets.BubbleChart = set_type_default() do mod.widgets.BubbleChartBase{
        width: Fill
        height: Fill
        /** the smallest circle, in pixels 2..40 step 1 */
        dot_min: 7.
        /** the largest circle, in pixels 4..90 step 1 */
        dot_max: 30.
        /** how solid a circle is over the ones behind it 0..1 step 0.05 */
        fill_alpha: 0.5
        /** how many squares the grid is divided into each way 0..10 step 1 */
        grid: 4
        /** the strip on the left the y numbers are written in 0..90 step 2 */
        axis_width: 36.
        /** the strip below the plot the x numbers are written in 0..60 step 2 */
        axis_height: 16.
    }
}

// ---- The shader layers ----

/// An annular sector: everything between two radii and two angles. A pie
/// wedge is one with no hole, a donut wedge is one with a hole, and a
/// radial bar's track and its arc are two of them over each other.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawWedge {
    #[deref]
    draw_super: DrawQuad,
    /// Where the wedge begins, radians clockwise from straight up, brought
    /// into one turn.
    #[live]
    a0: f32,
    /// How far it goes round from there. A full turn is a whole ring.
    #[live]
    span: f32,
    #[live]
    inner: f32,
    #[live]
    outer: f32,
    #[live]
    color: Vec4f,
    #[live]
    hot: f32,
}

/// A trapezoid centred in its quad: `top_w` wide at the top edge and
/// `bottom_w` wide at the bottom, both in pixels.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBand {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    top_w: f32,
    #[live]
    bottom_w: f32,
    #[live]
    color: Vec4f,
    #[live]
    hot: f32,
}

/// One filled triangle, its corners in pixels from the quad's top left. A
/// polygon is a fan of these.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTri {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    p0: Vec2f,
    #[live]
    p1: Vec2f,
    #[live]
    p2: Vec2f,
    #[live]
    color: Vec4f,
}

/// One anti-aliased line, its ends in pixels from the quad's top left.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSeg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    p0: Vec2f,
    #[live]
    p1: Vec2f,
    #[live]
    color: Vec4f,
    #[live(2.0)]
    thickness: f32,
}

/// A filled circle with a rim, drawn to fill its quad.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDot {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live(1.0)]
    border_size: f32,
    #[live]
    hot: f32,
}

// ---- The angles ----

/// The wedges a list of values cuts a circle into.
///
/// Angles are radians measured CLOCKWISE FROM STRAIGHT UP — the measure the
/// shader uses too, so the wedge that is drawn and the wedge a pointer
/// picks are the same wedge.
///
/// Spans do NOT wrap. An end is always at or after its start, and a span
/// may run past a full turn rather than coming back round to a smaller
/// number. Wrapping is the classic way to lose the wedge that straddles
/// twelve o'clock — its start becomes the larger number, and a caller that
/// assumes otherwise draws the rest of the circle instead of one wedge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wedges {
    spans: Vec<(f64, f64)>,
    total: f64,
}

impl Wedges {
    /// Cut a circle up, the first wedge beginning at `start`.
    ///
    /// A negative value counts as nothing. A circle cannot show a debt as a
    /// share of itself, and a wedge of negative width would run backwards
    /// over its neighbour.
    pub fn new(values: &[f64], start: f64) -> Self {
        let total: f64 = values.iter().map(|v| v.max(0.0)).sum();
        let mut spans = Vec::with_capacity(values.len());
        let mut acc = 0.0;
        for v in values {
            let before = acc;
            acc += v.max(0.0);
            // Both edges come from the RUNNING TOTAL rather than from
            // adding one width onto the last edge. Widths added one at a
            // time drift, and the drift all lands on the last wedge, which
            // is the one that has to close the circle exactly. Summed this
            // way the last edge is start + TAU to the bit, so a full turn
            // of parts really does meet where it started.
            //
            // A total of nothing gives every wedge nothing rather than
            // dividing by it: an empty circle is what an empty list looks
            // like.
            let (a, b) = if total > 0.0 {
                (start + before / total * TAU, start + acc / total * TAU)
            } else {
                (start, start)
            };
            spans.push((a, b));
        }
        Self { spans, total }
    }

    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub fn total(&self) -> f64 {
        self.total
    }

    pub fn span(&self, i: usize) -> (f64, f64) {
        self.spans.get(i).copied().unwrap_or((0.0, 0.0))
    }

    pub fn width(&self, i: usize) -> f64 {
        let (a, b) = self.span(i);
        b - a
    }

    /// What share of the whole wedge `i` is, 0..1.
    pub fn share(&self, i: usize) -> f64 {
        if self.total > 0.0 {
            self.width(i) / TAU
        } else {
            0.0
        }
    }

    /// The wedge an angle falls in, or nothing when there is nothing for it
    /// to fall in.
    ///
    /// Wedges of no width are skipped rather than tested: a part worth
    /// nothing is drawn as nothing, and a hit test that could land on one
    /// would report a part the pointer cannot see.
    pub fn at(&self, angle: f64) -> Option<usize> {
        if self.total <= 0.0 {
            return None;
        }
        let mut last = None;
        for (i, (a0, a1)) in self.spans.iter().enumerate() {
            let width = a1 - a0;
            if width <= 0.0 {
                continue;
            }
            if (angle - a0).rem_euclid(TAU) < width {
                return Some(i);
            }
            last = Some(i);
        }
        // The wedges cover the whole circle, so arriving here means an
        // angle sat on the closing edge and rounding put it a hair past.
        // The last wedge with any width owns it, rather than the caller
        // being told the pointer is over nothing while it is over the pie.
        last
    }
}

/// Where the point for axis `i` of `n` sits `radius` out from a centre: to
/// the right by `.0` and BELOW by `.1`.
///
/// Axis 0 points straight up and the rest follow it clockwise, so a chart's
/// first axis is at twelve o'clock however many axes it has and wherever it
/// is drawn.
pub fn spoke_offset(i: usize, n: usize, radius: f64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let a = i as f64 * TAU / n as f64;
    (a.sin() * radius, 0.0 - a.cos() * radius)
}

/// How wide each band of a funnel is, as a share of the widest, 0..1.
///
/// Against the LARGEST value and not against the total: the stages of a
/// funnel do not add up to anything, because the same people are counted
/// again in every stage they reached. A band's width is a magnitude beside
/// other magnitudes, and drawing it as a share of a meaningless sum makes
/// every band narrower the more stages there are.
pub fn funnel_widths(values: &[f64]) -> Vec<f64> {
    let max = values.iter().fold(0.0f64, |m, v| m.max(*v));
    if max <= 0.0 {
        return vec![0.0; values.len()];
    }
    values.iter().map(|v| (v.max(0.0) / max).clamp(0.0, 1.0)).collect()
}

/// What share of a full turn each value takes, 0..1.
///
/// `max` is what a whole turn stands for; a `max` of nothing uses the
/// largest value present, which makes the longest arc a full circle. A
/// value past `max` is a full turn and no more — an arc that laps itself
/// says nothing a full one does not, and reads as a smaller number.
pub fn turn_shares(values: &[f64], max: f64) -> Vec<f64> {
    let top = if max > 0.0 {
        max
    } else {
        values.iter().fold(0.0f64, |m, v| m.max(*v))
    };
    if top <= 0.0 {
        return vec![0.0; values.len()];
    }
    values.iter().map(|v| (v / top).clamp(0.0, 1.0)).collect()
}

/// The radius a weight gets, between `r_min` and `r_max`.
///
/// The weight scales the AREA and not the radius. A circle twice as wide
/// carries four times the ink, so a radius taken straight from the number
/// shows the big ones as four times what they are — the one place a bubble
/// chart is routinely a lie, and it costs one square root not to be.
pub fn bubble_radius(v: f64, lo: f64, hi: f64, r_min: f64, r_max: f64) -> f64 {
    let t = if hi > lo {
        ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    (r_min * r_min + t * (r_max * r_max - r_min * r_min)).sqrt()
}

/// Ink that can be read on `on`: near-black over a light colour, near-white
/// over a dark one.
///
/// A palette is a list of colours and not a list of pairs, so the ink for a
/// label sitting ON a part has to be worked out rather than looked up. The
/// weights are the usual ones for how bright the eye finds each channel.
pub fn ink_on(on: Vec4f) -> Vec4f {
    let luma = 0.299 * on.x + 0.587 * on.y + 0.114 * on.z;
    if luma > 0.55 {
        Vec4f { x: 0.06, y: 0.06, z: 0.08, w: 1.0 }
    } else {
        Vec4f { x: 0.97, y: 0.97, z: 0.98, w: 1.0 }
    }
}

/// What a figure reports about its parts. Parts are numbered in the order
/// their lines were written, so part three is the third line.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ChartPartAction {
    /// The part the pointer is over, or nothing once it has left them all.
    Hovered(Option<usize>),
    /// The part a press landed on.
    Picked(usize),
    #[default]
    None,
}

fn chart_picked(actions: &Actions, uid: WidgetUid) -> Option<usize> {
    match actions.find_widget_action(uid)?.cast() {
        ChartPartAction::Picked(i) => Some(i),
        _ => None,
    }
}

fn chart_hovered(actions: &Actions, uid: WidgetUid) -> Option<Option<usize>> {
    match actions.find_widget_action(uid)?.cast() {
        ChartPartAction::Hovered(i) => Some(i),
        _ => None,
    }
}

// ---- ChartFigure: the surface the six shapes share ----

/// How far below the top of its line box a glyph's ink starts, as a share
/// of the font size. `draw_abs` takes the LINE BOX, so centring that alone
/// leaves every label riding high.
const INK_DROP: f64 = 0.30;

/// The room left round a wedge inside its own quad: the rim is feathered
/// over about a pixel and a half, and a wedge drawn hard against the quad's
/// edge loses that feather.
const WEDGE_PAD: f64 = 2.0;

/// The palette, the text, the data and the room round the figure — the
/// things all six shapes need and none of them should each keep a copy of.
/// It is a widget in its own right only because the six reach it by
/// deref, the same way the plots in [`crate::chart`] reach `ChartView`.
#[derive(Script, ScriptHook, Widget)]
pub struct ChartFigure {
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
    draw_bg: DrawColor,
    #[live]
    draw_rule: DrawColor,
    #[live]
    draw_wedge: DrawWedge,
    #[live]
    draw_band: DrawBand,
    #[live]
    draw_tri: DrawTri,
    #[live]
    draw_seg: DrawSeg,
    #[live]
    draw_dot: DrawDot,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_value: DrawText,

    /// One line per part, as markup writes it. See [`crate::chart::parse_row`].
    #[live]
    pub series: Vec<String>,

    #[live]
    pub color_1: Vec4f,
    #[live]
    pub color_2: Vec4f,
    #[live]
    pub color_3: Vec4f,
    #[live]
    pub color_4: Vec4f,
    #[live]
    pub color_5: Vec4f,
    #[live]
    pub color_6: Vec4f,
    #[live]
    pub color_track: Vec4f,
    #[live]
    pub color_grid: Vec4f,
    #[live]
    pub color_bg: Vec4f,
    #[live]
    pub color_label: Vec4f,

    #[live(10.0)]
    pub pad: f64,
    #[live(1.0)]
    pub gap: f64,
    #[live]
    pub start_angle: f64,
    #[live]
    pub max_value: f64,
    #[live(true)]
    pub show_labels: bool,
    #[live(true)]
    pub show_values: bool,

    #[rust]
    rows: Vec<Row>,
    /// The lines `rows` was read from, so a live edit rebuilds them and an
    /// unchanged list costs nothing.
    #[rust]
    seeded_from: Vec<String>,
    #[rust]
    rect: Rect,
    #[rust]
    hot: Option<usize>,
}

impl Widget for ChartFigure {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.begin(cx, walk);
        DrawStep::done()
    }
}

impl ChartFigure {
    /// Re-read the markup lines if they have changed since the last draw.
    fn sync(&mut self) {
        if self.seeded_from != self.series {
            self.seeded_from = self.series.clone();
            self.rows = parse_rows(&self.series);
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// One number per part: the first on each line.
    pub fn values(&self) -> Vec<f64> {
        self.rows.iter().map(|row| row.value()).collect()
    }

    /// Take the parts from Rust instead of from markup. The markup lines
    /// are marked as already read, so the next draw does not put them back.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.seeded_from = self.series.clone();
        self.hot = None;
    }

    /// The colour part `i` is drawn in, the six going round again after the
    /// sixth. Rather than fading or darkening past the sixth: two parts the
    /// same colour are honestly ambiguous, where two nearly the same colour
    /// look distinguishable and are not.
    pub fn part_color(&self, i: usize) -> Vec4f {
        match i % 6 {
            0 => self.color_1,
            1 => self.color_2,
            2 => self.color_3,
            3 => self.color_4,
            4 => self.color_5,
            _ => self.color_6,
        }
    }

    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        self.rect = cx.walk_turtle(walk);
        self.draw_bg.color = self.color_bg;
        self.draw_bg.draw_abs(cx, self.rect);
        self.rect
    }

    /// The box the figure itself may use, the padding taken off.
    fn inner_rect(&self) -> Rect {
        let p = self.pad.max(0.0);
        Rect {
            pos: dvec2(self.rect.pos.x + p, self.rect.pos.y + p),
            size: dvec2((self.rect.size.x - p * 2.0).max(1.0), (self.rect.size.y - p * 2.0).max(1.0)),
        }
    }

    /// Note the part under the pointer, saying whether it moved. Callers
    /// report the move; this only remembers it and asks for a redraw.
    fn set_hot(&mut self, cx: &mut Cx, hot: Option<usize>) -> bool {
        if self.hot == hot {
            return false;
        }
        self.hot = hot;
        self.draw_bg.redraw(cx);
        true
    }

    // ---- The shapes ----

    #[allow(clippy::too_many_arguments)]
    fn wedge(
        &mut self,
        cx: &mut Cx2d,
        centre: DVec2,
        a0: f64,
        a1: f64,
        inner: f64,
        outer: f64,
        color: Vec4f,
        hot: bool,
    ) {
        if a1 <= a0 || outer <= inner.max(0.0) {
            return;
        }
        let half = outer + WEDGE_PAD;
        let span = a1 - a0;
        self.draw_wedge.a0 = a0.rem_euclid(TAU) as f32;
        // A whole turn goes over as a hair more than one, so the seam where
        // the ring closes falls INSIDE the wedge instead of exactly on its
        // edge, where the shader working in single precision would drop a
        // hairline of background across it.
        self.draw_wedge.span = if span >= TAU { (TAU * 1.002) as f32 } else { span as f32 };
        self.draw_wedge.inner = inner.max(0.0) as f32;
        self.draw_wedge.outer = outer as f32;
        self.draw_wedge.color = color;
        self.draw_wedge.hot = if hot { 1.0 } else { 0.0 };
        self.draw_wedge.draw_abs(
            cx,
            Rect {
                pos: dvec2(centre.x - half, centre.y - half),
                size: dvec2(half * 2.0, half * 2.0),
            },
        );
    }

    fn band(&mut self, cx: &mut Cx2d, rect: Rect, top_w: f64, bottom_w: f64, color: Vec4f, hot: bool) {
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        self.draw_band.top_w = top_w.max(0.0) as f32;
        self.draw_band.bottom_w = bottom_w.max(0.0) as f32;
        self.draw_band.color = color;
        self.draw_band.hot = if hot { 1.0 } else { 0.0 };
        self.draw_band.draw_abs(cx, rect);
    }

    fn tri(&mut self, cx: &mut Cx2d, a: DVec2, b: DVec2, c: DVec2, color: Vec4f) {
        let m = 2.0;
        let x0 = a.x.min(b.x).min(c.x) - m;
        let y0 = a.y.min(b.y).min(c.y) - m;
        let x1 = a.x.max(b.x).max(c.x) + m;
        let y1 = a.y.max(b.y).max(c.y) + m;
        if x1 - x0 <= 0.0 || y1 - y0 <= 0.0 {
            return;
        }
        self.draw_tri.p0 = Vec2f { x: (a.x - x0) as f32, y: (a.y - y0) as f32 };
        self.draw_tri.p1 = Vec2f { x: (b.x - x0) as f32, y: (b.y - y0) as f32 };
        self.draw_tri.p2 = Vec2f { x: (c.x - x0) as f32, y: (c.y - y0) as f32 };
        self.draw_tri.color = color;
        self.draw_tri
            .draw_abs(cx, Rect { pos: dvec2(x0, y0), size: dvec2(x1 - x0, y1 - y0) });
    }

    fn seg(&mut self, cx: &mut Cx2d, a: DVec2, b: DVec2, thickness: f64, color: Vec4f) {
        let m = thickness + 2.0;
        let x0 = a.x.min(b.x) - m;
        let y0 = a.y.min(b.y) - m;
        let x1 = a.x.max(b.x) + m;
        let y1 = a.y.max(b.y) + m;
        self.draw_seg.p0 = Vec2f { x: (a.x - x0) as f32, y: (a.y - y0) as f32 };
        self.draw_seg.p1 = Vec2f { x: (b.x - x0) as f32, y: (b.y - y0) as f32 };
        self.draw_seg.thickness = thickness as f32;
        self.draw_seg.color = color;
        self.draw_seg
            .draw_abs(cx, Rect { pos: dvec2(x0, y0), size: dvec2(x1 - x0, y1 - y0) });
    }

    #[allow(clippy::too_many_arguments)]
    fn dot(
        &mut self,
        cx: &mut Cx2d,
        at: DVec2,
        radius: f64,
        color: Vec4f,
        border_color: Vec4f,
        border_size: f64,
        hot: bool,
    ) {
        let r = radius.max(1.0);
        self.draw_dot.color = color;
        self.draw_dot.border_color = border_color;
        self.draw_dot.border_size = border_size as f32;
        self.draw_dot.hot = if hot { 1.0 } else { 0.0 };
        self.draw_dot.draw_abs(
            cx,
            Rect { pos: dvec2(at.x - r, at.y - r), size: dvec2(r * 2.0, r * 2.0) },
        );
    }

    fn rule(&mut self, cx: &mut Cx2d, rect: Rect, color: Vec4f) {
        self.draw_rule.color = color;
        self.draw_rule.draw_abs(cx, rect);
    }

    // ---- The words ----

    /// Draw `text` with its left edge at `at.x` and its middle at `at.y`.
    fn label_from(&mut self, cx: &mut Cx2d, at: DVec2, text: &str, color: Vec4f) {
        let size = self.draw_text.text_style.font_size as f64;
        self.draw_text.color = color;
        self.draw_text
            .draw_abs(cx, dvec2(at.x, at.y - size * 0.5 - size * INK_DROP), text);
    }

    /// Draw `text` with its middle at `at`.
    fn label_at(&mut self, cx: &mut Cx2d, at: DVec2, text: &str, color: Vec4f) {
        let w = measure(&self.draw_text, cx, text);
        self.label_from(cx, dvec2(at.x - w * 0.5, at.y), text, color);
    }

    /// Draw `text` with its right edge at `at.x`.
    fn label_to(&mut self, cx: &mut Cx2d, at: DVec2, text: &str, color: Vec4f) {
        let w = measure(&self.draw_text, cx, text);
        self.label_from(cx, dvec2(at.x - w, at.y), text, color);
    }

    fn value_from(&mut self, cx: &mut Cx2d, at: DVec2, text: &str, color: Vec4f) {
        let size = self.draw_value.text_style.font_size as f64;
        self.draw_value.color = color;
        self.draw_value
            .draw_abs(cx, dvec2(at.x, at.y - size * 0.5 - size * INK_DROP), text);
    }

    fn value_at(&mut self, cx: &mut Cx2d, at: DVec2, text: &str, color: Vec4f) {
        let w = measure(&self.draw_value, cx, text);
        self.value_from(cx, dvec2(at.x - w * 0.5, at.y), text, color);
    }

    fn value_to(&mut self, cx: &mut Cx2d, at: DVec2, text: &str, color: Vec4f) {
        let w = measure(&self.draw_value, cx, text);
        self.value_from(cx, dvec2(at.x - w, at.y), text, color);
    }

    /// A name over a number, both centred on `at`, in a space `room_x` wide
    /// and `room_y` tall.
    ///
    /// What does not fit is dropped, the name first: the number is what the
    /// part is worth and the name is usually recoverable from where the
    /// part sits. Nothing is shrunk to fit, because a chart whose slivers
    /// are set in a smaller face reads as a chart whose slivers matter
    /// less, and it is the same face at every size that makes two parts
    /// comparable at a glance.
    #[allow(clippy::too_many_arguments)]
    fn stacked(
        &mut self,
        cx: &mut Cx2d,
        at: DVec2,
        label: &str,
        value: &str,
        room_x: f64,
        room_y: f64,
        ink: Vec4f,
    ) {
        let size = self.draw_text.text_style.font_size as f64;
        let mut want_label = self.show_labels && !label.is_empty();
        let mut want_value = self.show_values && !value.is_empty();
        if want_label {
            let w = measure(&self.draw_text, cx, label);
            let lines = if want_value { 2.0 } else { 1.0 };
            if w + 6.0 > room_x || size * 1.25 * lines > room_y {
                want_label = false;
            }
        }
        if want_value {
            let w = measure(&self.draw_value, cx, value);
            if w + 4.0 > room_x || size * 1.2 > room_y {
                want_value = false;
            }
        }
        let n = f64::from(u8::from(want_label) + u8::from(want_value));
        if n == 0.0 {
            return;
        }
        let mut k = 0.0;
        if want_label {
            let y = at.y + (k - (n - 1.0) * 0.5) * size * 1.25;
            self.label_at(cx, dvec2(at.x, y), label, ink);
            k += 1.0;
        }
        if want_value {
            let y = at.y + (k - (n - 1.0) * 0.5) * size * 1.25;
            self.value_at(cx, dvec2(at.x, y), value, ink);
        }
    }
}

/// Draw a ring of wedges and label them.
///
/// A pie and a donut differ in one number — where the hole is — and in
/// nothing else, so they share this rather than each keeping a copy of it
/// to drift from.
fn draw_ring(
    figure: &mut ChartFigure,
    cx: &mut Cx2d,
    centre: DVec2,
    inner_r: f64,
    outer_r: f64,
    wedges: &Wedges,
) {
    let half_gap = (figure.gap.to_radians() * 0.5).max(0.0);
    for i in 0..wedges.len() {
        let (a0, a1) = wedges.span(i);
        let width = a1 - a0;
        if width <= 0.0 {
            continue;
        }
        // The drawn gap never eats more than half a wedge, so a crowded
        // chart keeps something of every part it was given rather than
        // silently losing the small ones to the spacing.
        let g = half_gap.min(width * 0.25);
        let color = figure.part_color(i);
        let hot = figure.hot == Some(i);
        figure.wedge(cx, centre, a0 + g, a1 - g, inner_r, outer_r, color, hot);
    }
    if !figure.show_labels && !figure.show_values {
        return;
    }
    // Labels ride in the middle of the band, or a little inside the rim on
    // a pie, where the wedge is at its widest and the words have the most
    // room before they run into the neighbours.
    let r_label = if inner_r > 0.0 { (inner_r + outer_r) * 0.5 } else { outer_r * 0.62 };
    let band = outer_r - inner_r.max(0.0);
    for i in 0..wedges.len() {
        let width = wedges.width(i);
        if width <= 0.0 {
            continue;
        }
        let (a0, a1) = wedges.span(i);
        let mid = (a0 + a1) * 0.5;
        let at = dvec2(centre.x + mid.sin() * r_label, centre.y - mid.cos() * r_label);
        let label = figure.rows.get(i).map(|row| row.label.clone()).unwrap_or_default();
        let value = format!("{:.0}%", wedges.share(i) * 100.0);
        let ink = ink_on(figure.part_color(i));
        // The room a wedge offers is the arc it cuts at the label's own
        // radius: a sliver gets no words rather than words spilling across
        // the parts either side of it.
        figure.stacked(cx, at, &label, &value, width * r_label, band, ink);
    }
}

/// The angle a point sits at from `centre`, clockwise from straight up, and
/// how far out it is. The one place the hit tests turn screen coordinates
/// into the measure [`Wedges`] speaks.
fn polar(centre: DVec2, at: DVec2) -> (f64, f64) {
    let d = at - centre;
    ((d.x).atan2(-d.y).rem_euclid(TAU), (d.x * d.x + d.y * d.y).sqrt())
}

// ---- PieChart ----

/// A circle cut into one wedge per part, each the part's share of the
/// total.
///
/// It answers one question — how does this whole divide — and it answers it
/// badly past about seven parts, where the wedges become too close in angle
/// to rank by eye. That is a property of circles and not of this code; a
/// list of parts too long to read as a pie is a bar chart.
#[derive(Script, ScriptHook, Widget)]
pub struct PieChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    figure: ChartFigure,

    #[rust]
    centre: DVec2,
    #[rust]
    radius: f64,
    #[rust]
    wedges: Wedges,
}

impl PieChart {
    fn hit_at(&self, abs: DVec2) -> Option<usize> {
        let (angle, r) = polar(self.centre, abs);
        if r > self.radius {
            return None;
        }
        self.wedges.at(angle)
    }
}

impl Widget for PieChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.figure.uid;
        match event.hits(cx, self.figure.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.hit_at(fe.abs);
                if self.figure.set_hot(cx, hot) {
                    cx.widget_action(uid, ChartPartAction::Hovered(hot));
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.figure.set_hot(cx, None) {
                    cx.widget_action(uid, ChartPartAction::Hovered(None));
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some(i) = self.hit_at(fe.abs) {
                    cx.widget_action(uid, ChartPartAction::Picked(i));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.figure.sync();
        self.figure.begin(cx, walk);
        let inner = self.figure.inner_rect();
        self.centre = inner.center();
        self.radius = (inner.size.x.min(inner.size.y) * 0.5).max(1.0);
        let values = self.figure.values();
        self.wedges = Wedges::new(&values, self.figure.start_angle.to_radians());
        let centre = self.centre;
        let radius = self.radius;
        let wedges = self.wedges.clone();
        draw_ring(&mut self.figure, cx, centre, 0.0, radius, &wedges);
        DrawStep::done()
    }
}

impl PieChartRef {
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<Row>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.figure.set_rows(rows);
            inner.figure.draw_bg.redraw(cx);
        }
    }

    /// The part a press landed on.
    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        chart_picked(actions, self.widget_uid())
    }

    /// The part the pointer moved onto, or `Some(None)` when it left them
    /// all. The outer `None` means this chart said nothing at all.
    pub fn hovered(&self, actions: &Actions) -> Option<Option<usize>> {
        chart_hovered(actions, self.widget_uid())
    }
}

// ---- DonutChart ----

/// A pie with a hole, and a readout in the middle of it.
///
/// The hole is not decoration. A pie asks the eye to compare angles at the
/// middle, where every wedge is a point; a donut leaves only the arcs,
/// which are the part of a wedge the eye is actually good at. And the hole
/// is the one place on a circular chart with room for a number, so the
/// total lives there — or, while the pointer is on a part, that part does.
#[derive(Script, ScriptHook, Widget)]
pub struct DonutChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    figure: ChartFigure,
    #[live]
    draw_centre: DrawText,

    /// The hole, as a share of the radius.
    #[live(0.58)]
    pub hole: f64,
    /// A word under the middle readout — "total", "this month".
    #[live]
    pub caption: String,

    #[rust]
    centre: DVec2,
    #[rust]
    radius: f64,
    #[rust]
    hole_r: f64,
    #[rust]
    wedges: Wedges,
}

impl DonutChart {
    fn hit_at(&self, abs: DVec2) -> Option<usize> {
        let (angle, r) = polar(self.centre, abs);
        if r > self.radius || r < self.hole_r {
            return None;
        }
        self.wedges.at(angle)
    }

    /// The middle readout: the part under the pointer while there is one,
    /// and the total otherwise. Two lines, big over small.
    fn draw_middle(&mut self, cx: &mut Cx2d) {
        let (big, small) = match self.figure.hot.and_then(|i| self.figure.rows.get(i)) {
            Some(row) => (fmt_value(row.value()), row.label.clone()),
            None => (
                fmt_value(self.figure.rows.iter().map(|row| row.value()).sum::<f64>()),
                self.caption.clone(),
            ),
        };
        let size = self.draw_centre.text_style.font_size as f64;
        // Words only when the hole can hold them: a readout clipped by the
        // ring around it looks like a rendering fault rather than a number.
        let room = self.hole_r * 1.7;
        let w = measure(&self.draw_centre, cx, &big);
        if w > room || self.hole_r < size {
            return;
        }
        let has_small = !small.is_empty();
        let dy = if has_small { size * 0.42 } else { 0.0 };
        self.draw_centre.draw_abs(
            cx,
            dvec2(
                self.centre.x - w * 0.5,
                self.centre.y - dy - size * 0.5 - size * INK_DROP,
            ),
            &big,
        );
        if has_small {
            let sw = measure(&self.figure.draw_text, cx, &small);
            if sw <= room {
                let label_color = self.figure.color_label;
                self.figure.label_at(
                    cx,
                    dvec2(self.centre.x, self.centre.y + size * 0.62),
                    &small,
                    label_color,
                );
            }
        }
    }
}

impl Widget for DonutChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.figure.uid;
        match event.hits(cx, self.figure.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.hit_at(fe.abs);
                if self.figure.set_hot(cx, hot) {
                    cx.widget_action(uid, ChartPartAction::Hovered(hot));
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.figure.set_hot(cx, None) {
                    cx.widget_action(uid, ChartPartAction::Hovered(None));
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some(i) = self.hit_at(fe.abs) {
                    cx.widget_action(uid, ChartPartAction::Picked(i));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.figure.sync();
        self.figure.begin(cx, walk);
        let inner = self.figure.inner_rect();
        self.centre = inner.center();
        self.radius = (inner.size.x.min(inner.size.y) * 0.5).max(1.0);
        self.hole_r = (self.radius * self.hole.clamp(0.0, 0.9)).max(0.0);
        let values = self.figure.values();
        self.wedges = Wedges::new(&values, self.figure.start_angle.to_radians());
        let centre = self.centre;
        let (hole_r, radius) = (self.hole_r, self.radius);
        let wedges = self.wedges.clone();
        draw_ring(&mut self.figure, cx, centre, hole_r, radius, &wedges);
        self.draw_middle(cx);
        DrawStep::done()
    }
}

impl DonutChartRef {
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<Row>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.figure.set_rows(rows);
            inner.figure.draw_bg.redraw(cx);
        }
    }

    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        chart_picked(actions, self.widget_uid())
    }

    pub fn hovered(&self, actions: &Actions) -> Option<Option<usize>> {
        chart_hovered(actions, self.widget_uid())
    }
}

// ---- RadialBarChart ----

/// One arc per part, nested, each the part's share of a full turn.
///
/// Unlike a pie the parts here do not divide anything: each arc is measured
/// against `max_value` — or against the largest part, if none is given — so
/// three arcs of 80% are three arcs of 80% and not a third of the circle
/// each. That makes it the shape for progress against a target rather than
/// for a breakdown.
///
/// The outermost track is the first part. Rings further in are shorter for
/// the same share, which flatters the last part, so this is a shape for a
/// handful of parts and not for a ranking.
#[derive(Script, ScriptHook, Widget)]
pub struct RadialBarChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    figure: ChartFigure,

    #[live(16.0)]
    pub track_size: f64,
    #[live(6.0)]
    pub track_gap: f64,
    /// The strip on the left the names are written in. Nothing centres the
    /// rings in the whole box and drops the names, which is what a chart
    /// small enough to be a tile wants.
    #[live(96.0)]
    pub label_width: f64,

    #[rust]
    centre: DVec2,
    /// Each track's inner and outer radius, in drawing order, so the hit
    /// test reads the rings that were actually drawn rather than working
    /// them out a second time and disagreeing.
    #[rust]
    tracks: Vec<(f64, f64)>,
}

impl RadialBarChart {
    fn hit_at(&self, abs: DVec2) -> Option<usize> {
        let (_, r) = polar(self.centre, abs);
        self.tracks
            .iter()
            .position(|(inner, outer)| r >= *inner && r <= *outer)
    }
}

impl Widget for RadialBarChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.figure.uid;
        match event.hits(cx, self.figure.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.hit_at(fe.abs);
                if self.figure.set_hot(cx, hot) {
                    cx.widget_action(uid, ChartPartAction::Hovered(hot));
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.figure.set_hot(cx, None) {
                    cx.widget_action(uid, ChartPartAction::Hovered(None));
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some(i) = self.hit_at(fe.abs) {
                    cx.widget_action(uid, ChartPartAction::Picked(i));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.figure.sync();
        self.figure.begin(cx, walk);
        let box_rect = self.figure.inner_rect();
        let gutter = self.label_width.max(0.0).min(box_rect.size.x * 0.5);
        let radius = (((box_rect.size.x - gutter) * 0.5).min(box_rect.size.y * 0.5)).max(1.0);
        self.centre = dvec2(
            box_rect.pos.x + gutter + radius,
            box_rect.pos.y + box_rect.size.y * 0.5,
        );
        self.tracks.clear();

        let values = self.figure.values();
        let shares = turn_shares(&values, self.figure.max_value);
        let start = self.figure.start_angle.to_radians();
        let step = self.track_size.max(1.0) + self.track_gap.max(0.0);
        let centre = self.centre;
        for i in 0..values.len() {
            let outer = radius - i as f64 * step;
            let inner = outer - self.track_size.max(1.0);
            // Rings that would fold through the middle are not drawn at
            // all. A chart that keeps going draws the last few parts as
            // dots at the centre, which reads as data rather than as the
            // box having run out.
            if inner <= 2.0 {
                break;
            }
            self.tracks.push((inner, outer));
            let hot = self.figure.hot == Some(i);
            let track = self.figure.color_track;
            self.figure
                .wedge(cx, centre, start, start + TAU, inner, outer, track, false);
            let color = self.figure.part_color(i);
            self.figure.wedge(
                cx,
                centre,
                start,
                start + shares[i] * TAU,
                inner,
                outer,
                color,
                hot,
            );
        }

        if gutter <= 0.0 {
            return DrawStep::done();
        }
        let edge = box_rect.pos.x + gutter - 8.0;
        for i in 0..self.tracks.len() {
            let (inner, outer) = self.tracks[i];
            // Each name sits level with the top of its own ring, so the
            // names stack in ring order and no two land on the same line.
            let y = centre.y - (inner + outer) * 0.5;
            let label = self.figure.rows.get(i).map(|row| row.label.clone()).unwrap_or_default();
            let color = self.figure.part_color(i);
            if self.figure.show_values {
                let text = format!("{:.0}%", shares[i] * 100.0);
                self.figure.value_to(cx, dvec2(edge, y), &text, color);
                let w = measure(&self.figure.draw_value, cx, &text);
                if self.figure.show_labels && !label.is_empty() {
                    self.figure.label_to(cx, dvec2(edge - w - 6.0, y), &label, color);
                }
            } else if self.figure.show_labels {
                self.figure.label_to(cx, dvec2(edge, y), &label, color);
            }
        }
        DrawStep::done()
    }
}

impl RadialBarChartRef {
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<Row>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.figure.set_rows(rows);
            inner.figure.draw_bg.redraw(cx);
        }
    }

    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        chart_picked(actions, self.widget_uid())
    }

    pub fn hovered(&self, actions: &Actions) -> Option<Option<usize>> {
        chart_hovered(actions, self.widget_uid())
    }
}

// ---- FunnelChart ----

/// A stack of bands, each as wide as its part is against the widest.
///
/// The stages of a funnel are not shares of anything — the same people are
/// counted again in every stage they reached — so a band's width is its
/// value against the LARGEST value, and the first band is full width by
/// construction.
///
/// With `taper` on, each band's bottom edge is as wide as the next band's
/// top, which draws the drop between two stages as a visible slope rather
/// than as a step the eye has to measure. Off, the bands are plain centred
/// bars, which is the honest shape when the order is not a sequence.
#[derive(Script, ScriptHook, Widget)]
pub struct FunnelChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    figure: ChartFigure,

    #[live(6.0)]
    pub band_gap: f64,
    #[live(true)]
    pub taper: bool,

    /// The top and bottom of each band, so the hit test agrees with what
    /// was drawn.
    #[rust]
    bands: Vec<(f64, f64)>,
}

impl FunnelChart {
    /// A band's whole row is the target, not the trapezoid inside it. A
    /// stage that has narrowed to a sliver is exactly the one worth
    /// pointing at, and a hit test that shrinks with the band would put it
    /// out of reach.
    fn hit_at(&self, abs: DVec2) -> Option<usize> {
        self.bands
            .iter()
            .position(|(top, bottom)| abs.y >= *top && abs.y <= *bottom)
    }
}

impl Widget for FunnelChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.figure.uid;
        match event.hits(cx, self.figure.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.hit_at(fe.abs);
                if self.figure.set_hot(cx, hot) {
                    cx.widget_action(uid, ChartPartAction::Hovered(hot));
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.figure.set_hot(cx, None) {
                    cx.widget_action(uid, ChartPartAction::Hovered(None));
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some(i) = self.hit_at(fe.abs) {
                    cx.widget_action(uid, ChartPartAction::Picked(i));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.figure.sync();
        self.figure.begin(cx, walk);
        let box_rect = self.figure.inner_rect();
        self.bands.clear();
        let values = self.figure.values();
        let n = values.len();
        if n == 0 {
            return DrawStep::done();
        }
        let widths = funnel_widths(&values);
        let gap = self.band_gap.max(0.0).min(box_rect.size.y / (n as f64 * 2.0));
        let band_h = ((box_rect.size.y - gap * (n as f64 - 1.0)) / n as f64).max(1.0);
        let full = box_rect.size.x;
        for i in 0..n {
            let top = box_rect.pos.y + i as f64 * (band_h + gap);
            let rect = Rect { pos: dvec2(box_rect.pos.x, top), size: dvec2(full, band_h) };
            self.bands.push((top, top + band_h));
            let top_w = widths[i] * full;
            let bottom_w = if self.taper && i + 1 < n { widths[i + 1] * full } else { top_w };
            let color = self.figure.part_color(i);
            let hot = self.figure.hot == Some(i);
            self.figure.band(cx, rect, top_w, bottom_w, color, hot);

            let label = self.figure.rows.get(i).map(|row| row.label.clone()).unwrap_or_default();
            let value = fmt_value(values[i]);
            let at = dvec2(box_rect.pos.x + full * 0.5, top + band_h * 0.5);
            // The narrowest the band gets is what the words have to fit in,
            // so a tapered band is measured at its bottom edge and not at
            // its top.
            let room = top_w.min(bottom_w);
            let ink = ink_on(color);
            self.figure.stacked(cx, at, &label, &value, room, band_h, ink);
        }
        DrawStep::done()
    }
}

impl FunnelChartRef {
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<Row>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.figure.set_rows(rows);
            inner.figure.draw_bg.redraw(cx);
        }
    }

    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        chart_picked(actions, self.widget_uid())
    }

    pub fn hovered(&self, actions: &Actions) -> Option<Option<usize>> {
        chart_hovered(actions, self.widget_uid())
    }
}

// ---- RadarChart ----

/// One closed face per part over a web of named axes.
///
/// Every axis runs from nothing at the middle to `max_value` at the rim —
/// ONE scale for all of them. A radar with a scale per axis can be made to
/// say anything, since the shape then depends on numbers that are nowhere
/// on the picture; here two faces can be compared because the same distance
/// from the middle means the same thing wherever it is measured.
///
/// The room a face encloses is not proportional to anything, and it changes
/// when the axes are reordered. Read the distances, not the area.
#[derive(Script, ScriptHook, Widget)]
pub struct RadarChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    figure: ChartFigure,

    /// The axes, the first straight up and the rest clockwise.
    #[live]
    pub axes: Vec<String>,
    #[live(4usize)]
    pub rings: usize,
    #[live(0.28)]
    pub fill_alpha: f64,
    #[live(2.0)]
    pub line_width: f64,
}

impl RadarChart {
    /// How many spokes the web has: the names given, or however many
    /// numbers the longest part carries when no names were.
    fn axis_count(&self) -> usize {
        let widest = self.figure.rows.iter().map(|row| row.values.len()).max().unwrap_or(0);
        self.axes.len().max(widest)
    }

    /// What the rim stands for, never nothing: a web whose rim is zero has
    /// every point at the middle and shows no shape at all.
    fn top_value(&self) -> f64 {
        if self.figure.max_value > 0.0 {
            return self.figure.max_value;
        }
        let top = self
            .figure
            .rows
            .iter()
            .flat_map(|row| row.values.iter())
            .fold(0.0f64, |m, v| m.max(*v));
        if top > 0.0 {
            top
        } else {
            1.0
        }
    }
}

impl Widget for RadarChart {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.figure.sync();
        self.figure.begin(cx, walk);
        let box_rect = self.figure.inner_rect();
        let n = self.axis_count();
        if n < 3 {
            // Two axes are a line and one is a point. Neither is a radar,
            // and drawing one anyway would invite it to be read as one.
            return DrawStep::done();
        }
        let names = self.figure.show_labels && !self.axes.is_empty();
        let room = if names { 30.0 } else { 0.0 };
        let radius = ((box_rect.size.x.min(box_rect.size.y) * 0.5) - room).max(1.0);
        let centre = box_rect.center();
        let grid = self.figure.color_grid;
        let rings = self.rings.max(1);

        // The web: one closed polygon per ring, then the spokes.
        for k in 1..=rings {
            let r = radius * k as f64 / rings as f64;
            for i in 0..n {
                let (ax, ay) = spoke_offset(i, n, r);
                let (bx, by) = spoke_offset((i + 1) % n, n, r);
                self.figure.seg(
                    cx,
                    dvec2(centre.x + ax, centre.y + ay),
                    dvec2(centre.x + bx, centre.y + by),
                    1.0,
                    grid,
                );
            }
        }
        for i in 0..n {
            let (ax, ay) = spoke_offset(i, n, radius);
            self.figure
                .seg(cx, centre, dvec2(centre.x + ax, centre.y + ay), 1.0, grid);
        }

        // The faces. Each is a fan from the middle, which fills it exactly
        // because every corner sits on its own spoke and so the shape is
        // always star-shaped about that middle.
        let top = self.top_value();
        let alpha = self.fill_alpha.clamp(0.0, 1.0) as f32;
        let count = self.figure.rows.len();
        for s in 0..count {
            let color = self.figure.part_color(s);
            let fill = Vec4f { w: color.w * alpha, ..color };
            let mut points = Vec::with_capacity(n);
            for i in 0..n {
                let v = self.figure.rows[s].value_at(i);
                let (dx, dy) = spoke_offset(i, n, radius * (v / top).clamp(0.0, 1.0));
                points.push(dvec2(centre.x + dx, centre.y + dy));
            }
            for i in 0..n {
                self.figure.tri(cx, centre, points[i], points[(i + 1) % n], fill);
            }
            for i in 0..n {
                self.figure
                    .seg(cx, points[i], points[(i + 1) % n], self.line_width, color);
            }
            for p in &points {
                self.figure
                    .dot(cx, *p, self.line_width + 1.5, color, color, 0.0, false);
            }
        }

        if names {
            let label_color = self.figure.color_label;
            for i in 0..n.min(self.axes.len()) {
                let (dx, dy) = spoke_offset(i, n, radius + 14.0);
                let name = self.axes[i].clone();
                self.figure
                    .label_at(cx, dvec2(centre.x + dx, centre.y + dy), &name, label_color);
            }
        }
        DrawStep::done()
    }
}

impl RadarChartRef {
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<Row>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.figure.set_rows(rows);
            inner.figure.draw_bg.redraw(cx);
        }
    }
}

// ---- BubbleChart ----

/// Named points on a plot, each carrying a weight the circle's AREA shows.
///
/// Three numbers a line — across, up, and how much — so it says what a
/// scatter says and one thing more. The weight scales the area and not the
/// radius; see [`bubble_radius`] for why that is the whole difference
/// between this and a chart that quadruples its own biggest number.
///
/// The axes are fitted to the data and drawn as a plain grid with the four
/// extremes written round it. There are no nice round tick values: with a
/// handful of labelled points the labels are the reading, and a grid here
/// is for judging position, not for looking numbers up.
#[derive(Script, ScriptHook, Widget)]
pub struct BubbleChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    figure: ChartFigure,

    #[live(7.0)]
    pub dot_min: f64,
    #[live(30.0)]
    pub dot_max: f64,
    #[live(0.5)]
    pub fill_alpha: f64,
    #[live(4usize)]
    pub grid: usize,
    #[live(36.0)]
    pub axis_width: f64,
    #[live(16.0)]
    pub axis_height: f64,

    /// Where each circle was drawn and how big, so the hit test reads what
    /// is on screen rather than working the layout out a second time.
    #[rust]
    bubbles: Vec<(DVec2, f64)>,
}

impl BubbleChart {
    /// The smallest circle the point is inside, which is the one drawn
    /// nearest the top and the one the eye takes itself to be pointing at.
    fn hit_at(&self, abs: DVec2) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (i, (at, r)) in self.bubbles.iter().enumerate() {
            let d = abs - *at;
            if d.x * d.x + d.y * d.y <= r * r && best.map(|(_, br)| *r < br).unwrap_or(true) {
                best = Some((i, *r));
            }
        }
        best.map(|(i, _)| i)
    }
}

impl Widget for BubbleChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.figure.uid;
        match event.hits(cx, self.figure.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.hit_at(fe.abs);
                if self.figure.set_hot(cx, hot) {
                    cx.widget_action(uid, ChartPartAction::Hovered(hot));
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.figure.set_hot(cx, None) {
                    cx.widget_action(uid, ChartPartAction::Hovered(None));
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some(i) = self.hit_at(fe.abs) {
                    cx.widget_action(uid, ChartPartAction::Picked(i));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.figure.sync();
        self.figure.begin(cx, walk);
        let box_rect = self.figure.inner_rect();
        self.bubbles.clear();
        let gutter = self.axis_width.max(0.0).min(box_rect.size.x * 0.4);
        let footer = self.axis_height.max(0.0).min(box_rect.size.y * 0.4);
        let plot = Rect {
            pos: dvec2(box_rect.pos.x + gutter, box_rect.pos.y),
            size: dvec2(
                (box_rect.size.x - gutter).max(1.0),
                (box_rect.size.y - footer).max(1.0),
            ),
        };

        let grid = self.figure.color_grid;
        let divisions = self.grid;
        for k in 0..=divisions {
            let t = if divisions == 0 { 0.0 } else { k as f64 / divisions as f64 };
            self.figure.rule(
                cx,
                Rect {
                    pos: dvec2(plot.pos.x + plot.size.x * t, plot.pos.y),
                    size: dvec2(1.0, plot.size.y),
                },
                grid,
            );
            self.figure.rule(
                cx,
                Rect {
                    pos: dvec2(plot.pos.x, plot.pos.y + plot.size.y * t),
                    size: dvec2(plot.size.x, 1.0),
                },
                grid,
            );
        }

        let count = self.figure.rows.len();
        if count == 0 {
            return DrawStep::done();
        }
        let mut x_lo = f64::INFINITY;
        let mut x_hi = f64::NEG_INFINITY;
        let mut y_lo = f64::INFINITY;
        let mut y_hi = f64::NEG_INFINITY;
        let mut w_lo = f64::INFINITY;
        let mut w_hi = f64::NEG_INFINITY;
        for row in &self.figure.rows {
            let (x, y) = (row.value_at(0), row.value_at(1));
            let w = if row.values.len() > 2 { row.value_at(2) } else { 1.0 };
            x_lo = x_lo.min(x);
            x_hi = x_hi.max(x);
            y_lo = y_lo.min(y);
            y_hi = y_hi.max(y);
            w_lo = w_lo.min(w);
            w_hi = w_hi.max(w);
        }
        // A padding of a twelfth keeps the outermost circles off the frame,
        // where half of one would be clipped and read as a smaller weight.
        let x_pad = (x_hi - x_lo).max(1.0) / 12.0;
        let y_pad = (y_hi - y_lo).max(1.0) / 12.0;
        let (x0, x1) = (x_lo - x_pad, x_hi + x_pad);
        let (y0, y1) = (y_lo - y_pad, y_hi + y_pad);
        let to_px = |x: f64, y: f64| -> DVec2 {
            dvec2(
                plot.pos.x + (x - x0) / (x1 - x0) * plot.size.x,
                plot.pos.y + (1.0 - (y - y0) / (y1 - y0)) * plot.size.y,
            )
        };

        let alpha = self.fill_alpha.clamp(0.0, 1.0) as f32;
        for i in 0..count {
            let row = &self.figure.rows[i];
            let (x, y) = (row.value_at(0), row.value_at(1));
            let w = if row.values.len() > 2 { row.value_at(2) } else { 1.0 };
            let label = row.label.clone();
            let at = to_px(x, y);
            let r = bubble_radius(w, w_lo, w_hi, self.dot_min.max(1.0), self.dot_max.max(2.0));
            self.bubbles.push((at, r));
            let color = self.figure.part_color(i);
            let fill = Vec4f { w: color.w * alpha, ..color };
            let hot = self.figure.hot == Some(i);
            self.figure.dot(cx, at, r, fill, color, 1.5, hot);
            if self.figure.show_labels && !label.is_empty() {
                let ink = self.figure.color_label;
                let text_w = measure(&self.figure.draw_text, cx, &label);
                // A name goes in its circle when it fits and under it when
                // it does not, rather than being dropped: the whole point
                // of a bubble over a dot is that the point has a name.
                let at = if text_w + 6.0 <= r * 2.0 { at } else { dvec2(at.x, at.y + r + 8.0) };
                self.figure.label_at(cx, at, &label, ink);
            }
        }

        if footer > 0.0 || gutter > 0.0 {
            let ink = self.figure.color_label;
            let base = plot.pos.y + plot.size.y + footer * 0.5;
            self.figure
                .label_from(cx, dvec2(plot.pos.x, base), &fmt_value(x0), ink);
            self.figure
                .label_to(cx, dvec2(plot.pos.x + plot.size.x, base), &fmt_value(x1), ink);
            let edge = plot.pos.x - 6.0;
            self.figure
                .label_to(cx, dvec2(edge, plot.pos.y + 6.0), &fmt_value(y1), ink);
            self.figure.label_to(
                cx,
                dvec2(edge, plot.pos.y + plot.size.y - 6.0),
                &fmt_value(y0),
                ink,
            );
        }
        DrawStep::done()
    }
}

impl BubbleChartRef {
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<Row>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.figure.set_rows(rows);
            inner.figure.draw_bg.redraw(cx);
        }
    }

    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        chart_picked(actions, self.widget_uid())
    }

    pub fn hovered(&self, actions: &Actions) -> Option<Option<usize>> {
        chart_hovered(actions, self.widget_uid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn wedges_summing_to_a_full_turn_close_exactly_where_they_started() {
        let w = Wedges::new(&[3.0, 1.0, 7.0, 2.0, 11.0], 0.0);
        assert_eq!(w.span(0).0, 0.0);
        // Not "close to": the last edge has to BE the first one, or a ring
        // shows a hairline of background where it comes back round. Every
        // edge is a running total over the whole, so nothing accumulates.
        assert_eq!(w.span(4).1, TAU, "the last wedge closed the circle exactly");
        // And no wedge overlaps or leaves a gap: each begins where the one
        // before it ended.
        for i in 1..w.len() {
            assert_eq!(w.span(i).0, w.span(i - 1).1);
        }
        let sum: f64 = (0..w.len()).map(|i| w.share(i)).sum();
        assert!(close(sum, 1.0));
    }

    #[test]
    fn a_start_angle_turns_the_whole_ring_and_nothing_else() {
        let quarter = TAU * 0.25;
        let w = Wedges::new(&[1.0, 1.0, 1.0, 1.0], quarter);
        assert_eq!(w.span(0).0, quarter);
        assert_eq!(w.span(3).1, quarter + TAU, "the spans run on past a turn");
        // Which is the point of not wrapping: the wedge that straddles
        // twelve o'clock is still a span whose end is after its start.
        for i in 0..w.len() {
            assert!(w.span(i).1 > w.span(i).0);
        }
        // And an angle still lands in the wedge it looks like it is in.
        assert_eq!(w.at(quarter + 0.01), Some(0));
        assert_eq!(w.at(0.01), Some(3), "just past twelve is the last wedge");
    }

    #[test]
    fn a_total_of_nothing_is_an_empty_circle_and_not_a_division_by_zero() {
        let w = Wedges::new(&[0.0, 0.0, 0.0], 0.0);
        assert_eq!(w.total(), 0.0);
        for i in 0..w.len() {
            assert_eq!(w.width(i), 0.0);
            assert_eq!(w.share(i), 0.0);
        }
        assert_eq!(w.at(1.0), None, "there is nothing under the pointer");
        // The same for a list with nothing in it at all.
        let empty = Wedges::new(&[], 0.0);
        assert!(empty.is_empty());
        assert_eq!(empty.at(0.0), None);
    }

    #[test]
    fn one_slice_of_everything_is_the_whole_circle() {
        let w = Wedges::new(&[0.0, 5.0, 0.0], 0.0);
        assert_eq!(w.width(1), TAU);
        assert!(close(w.share(1), 1.0));
        // Whichever way the pointer is, it is over that one part — and
        // never over the two worth nothing, which are drawn as nothing.
        for step in 0..16 {
            let a = step as f64 * TAU / 16.0;
            assert_eq!(w.at(a), Some(1));
        }
    }

    #[test]
    fn a_negative_value_counts_as_nothing_rather_than_running_backwards() {
        let w = Wedges::new(&[10.0, -4.0, 10.0], 0.0);
        assert_eq!(w.width(1), 0.0);
        assert!(close(w.share(0), 0.5), "the two positives split the circle");
        assert!(close(w.share(2), 0.5));
    }

    #[test]
    fn the_first_axis_of_a_radar_is_at_twelve_oclock() {
        let up = spoke_offset(0, 4, 10.0);
        assert!(close(up.0, 0.0) && close(up.1, -10.0), "straight up is a negative y");
        let right = spoke_offset(1, 4, 10.0);
        assert!(close(right.0, 10.0) && close(right.1, 0.0), "and the next is clockwise");
        let down = spoke_offset(2, 4, 10.0);
        assert!(close(down.0, 0.0) && close(down.1, 10.0));
        let left = spoke_offset(3, 4, 10.0);
        assert!(close(left.0, -10.0) && close(left.1, 0.0));
    }

    #[test]
    fn radar_points_sit_on_their_own_spoke_at_their_own_distance() {
        // Three axes, the second at a third of the rim: every point is
        // exactly as far out as its value says, and on the line the web
        // drew for it.
        for i in 0..3 {
            let full = spoke_offset(i, 3, 90.0);
            let third = spoke_offset(i, 3, 30.0);
            assert!(close(full.0 / 3.0, third.0));
            assert!(close(full.1 / 3.0, third.1));
            assert!(close((third.0 * third.0 + third.1 * third.1).sqrt(), 30.0));
        }
        // A web with no axes has nowhere to put a point, and says so
        // rather than dividing by however many axes there are.
        assert_eq!(spoke_offset(0, 0, 90.0), (0.0, 0.0));
    }

    #[test]
    fn funnel_bands_are_measured_against_the_widest_and_not_the_total() {
        let w = funnel_widths(&[1000.0, 500.0, 250.0]);
        assert!(close(w[0], 1.0), "the first stage fills the box");
        assert!(close(w[1], 0.5));
        assert!(close(w[2], 0.25));
        // Adding a stage does not narrow the ones already there, which is
        // what measuring against a sum would do.
        let more = funnel_widths(&[1000.0, 500.0, 250.0, 125.0]);
        assert!(close(more[0], 1.0) && close(more[1], 0.5));
        assert_eq!(funnel_widths(&[0.0, 0.0]), vec![0.0, 0.0]);
    }

    #[test]
    fn an_arc_past_its_maximum_is_a_full_turn_and_no_more() {
        let s = turn_shares(&[50.0, 100.0, 150.0], 100.0);
        assert!(close(s[0], 0.5) && close(s[1], 1.0));
        assert!(close(s[2], 1.0), "a lap and a half would read as half a lap");
        // No maximum given: the largest part is the full circle.
        let auto = turn_shares(&[20.0, 40.0], 0.0);
        assert!(close(auto[0], 0.5) && close(auto[1], 1.0));
        assert_eq!(turn_shares(&[0.0], 0.0), vec![0.0]);
    }

    #[test]
    fn a_bubbles_weight_scales_its_area_and_not_its_width() {
        let (r_min, r_max) = (5.0, 25.0);
        assert!(close(bubble_radius(0.0, 0.0, 100.0, r_min, r_max), r_min));
        assert!(close(bubble_radius(100.0, 0.0, 100.0, r_min, r_max), r_max));
        // Halfway between the two weights is halfway between the two
        // AREAS, which sits further out than halfway between the two radii:
        // the outer ring of a wide circle carries a great deal of area for
        // very little width. Straight-line radii would put it at 15, and
        // every weight above the middle would then be drawn as more than
        // it is.
        let mid = bubble_radius(50.0, 0.0, 100.0, r_min, r_max);
        assert!(close(mid * mid, (r_min * r_min + r_max * r_max) * 0.5));
        assert!(mid > 15.0);
        // Every weight the same: one range of nothing, and every circle
        // the largest rather than a division by zero.
        assert!(close(bubble_radius(7.0, 7.0, 7.0, r_min, r_max), r_max));
    }

    #[test]
    fn ink_reverses_over_a_light_part() {
        let dark = Vec4f { x: 0.1, y: 0.12, z: 0.3, w: 1.0 };
        let light = Vec4f { x: 0.95, y: 0.92, z: 0.7, w: 1.0 };
        assert!(ink_on(dark).x > 0.5, "light ink on a dark wedge");
        assert!(ink_on(light).x < 0.5, "dark ink on a light one");
        // Green counts for far more than blue: a saturated blue is dark to
        // the eye however high its channel goes, and a yellow is not.
        let blue = Vec4f { x: 0.0, y: 0.0, z: 1.0, w: 1.0 };
        let yellow = Vec4f { x: 1.0, y: 1.0, z: 0.0, w: 1.0 };
        assert!(ink_on(blue).x > 0.5);
        assert!(ink_on(yellow).x < 0.5);
    }
}

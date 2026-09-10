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
//! from Rust.
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
use crate::{badge::measure, makepad_derive_widget::*, makepad_draw::*, widget::*};
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

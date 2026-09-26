//! The diagonal name: a heading turned on its side, for a column too narrow
//! to hold it flat.
//!
//! Three things in this library write one. The panel kit's
//! `FabDiagonalLabel` (fab_controls.rs) was the first, over the theme
//! matrix's 26-point knob columns; `DiagonalLabel` here is the same control
//! drawn from the app theme's tokens rather than the panel's own table; and
//! the tables — `Table` and `DataGrid` — turn their own heading row when
//! `header_angle` is set. All three place the name with the same arithmetic,
//! and that is the reason this module exists: where a name starts is the one
//! thing that has to be exact, because a name standing over the wrong column
//! names the wrong column, and three copies of that sum would be three
//! chances to get it wrong.
//!
//! The pure half — [`diagonal_run`] and [`diagonal_row_height`] — is settled
//! and tested without a window. [`draw_diagonal_name`] is the other half:
//! the glyph walk along the placed baseline, which is the same everywhere
//! too and differs only in whose `DrawRotatedText` it draws with.
//!
//! Registered names:
//! * `mod.widgets.DiagonalLean` — which way a name leans, and so which side
//!   its ink hangs over.
//! * `mod.widgets.DiagonalLabel` — the themed diagonal name. It draws
//!   OUTSIDE its own box on purpose, so the row it stands in wants
//!   `clip_x: false` and room on the side it leans over.
use crate::{
    makepad_derive_widget::*, makepad_draw::text::geom::Point, makepad_draw::*, widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The lean is declared here rather than in the panel kit because three
    // controls take one now, and the kit is the one that has to be droppable.
    // Bound to a local as well: the `use` above sees what existed before this
    // block ran, so the label below could not otherwise name it.
    let DiagonalLean = set_type_default() do #(DiagonalLean::script_api(vm))
    mod.widgets.DiagonalLean = DiagonalLean

    mod.widgets.DiagonalLabelBase = #(DiagonalLabel::register_widget(vm))
    /** A name written across the corner of the box it names, for a column
     * too narrow to hold it flat. The ink overflows its own box on purpose,
     * so the row these stand in wants `clip_x: false` and room at the end
     * they lean over. */
    mod.widgets.DiagonalLabel = set_type_default() do mod.widgets.DiagonalLabelBase{
        // Fill so a header row divides itself between its columns the way
        // the rows under it do. 60 down holds a name of about 70 points at
        // 45 degrees; a host that knows its own longest name should fix this
        // for itself, which is what `diagonal_row_height` is for.
        width: Fill
        height: 60
        text: ""
        /** how far from the horizontal the name is turned, in degrees 0..90 step 5 */
        angle: 45.0
        /** Fall hangs the name over the LEFT, Rise over the right */
        lean: DiagonalLean.Rise

        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{
                font_size: theme.font_size_p
                // The name is placed by hand from its own baseline, so a
                // line box taller than the line would only move the ink
                // down inside it.
                line_spacing: 1.0
            }
        }
    }

    /** The heading face: the name in the weight a table's headings wear. */
    mod.widgets.DiagonalLabelHeading = mod.widgets.DiagonalLabel{
        draw_text +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
                line_spacing: 1.0
            }
        }
    }
}

/// Which way a diagonal name runs across the column it names.
///
/// Both are wanted, and what the choice really decides is which edge the ink
/// hangs over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook)]
#[repr(u32)]
pub enum DiagonalLean {
    /// The name ENDS on its column's bottom centre, having begun up and to
    /// the left, and reads downward to the right. The overflow is to the
    /// LEFT, and over a matrix that is the empty corner above the row-name
    /// column — so the LAST column's name is never cut in half by the
    /// panel's right edge. That is why it is the pick.
    #[pick]
    Fall = 0,
    /// The name STARTS on its column's bottom centre and rises to the upper
    /// right: the spreadsheet convention. The overflow is to the RIGHT, so a
    /// host that picks this owes its last column that much room — which a
    /// table has, because the ground to the right of its last column is its
    /// own.
    Rise = 1,
}

/// Where a diagonal name's baseline runs, and the box its ink takes.
///
/// Every coordinate is absolute, in the same space as the box handed in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagonalRun {
    /// The first glyph's pen point.
    pub start: DVec2,
    /// One advance past the last glyph's: where the baseline stops.
    pub end: DVec2,
    /// The rotation every glyph is turned by, in radians and in SCREEN
    /// terms — y points down, so a `Fall` is positive and a `Rise`
    /// negative.
    pub angle: f64,
    /// Everything the ink covers. Normally wider than the box and usually
    /// taller: it is what must NOT be clipped, and what a host measures a
    /// header row against.
    pub bounds: Rect,
}

/// How tall a header row has to be to hold a name of `text_width` turned by
/// `angle_deg`, in points.
///
/// The name lies along the hypotenuse and its line stands across it, so the
/// row needs both: `text_width * sin(angle) + line_height * cos(angle)`. It
/// is the same for either lean — they differ in which way the ink leans, not
/// in how much room it takes. A host that cannot ask the widget (a height
/// written in the DSL) can do this sum with a ruler: at 45 degrees it is
/// about 0.71 of the longest name plus 0.71 of a line.
pub fn diagonal_row_height(text_width: f64, line_height: f64, angle_deg: f64) -> f64 {
    let rad = angle_deg.clamp(0.0, 90.0).to_radians();
    text_width.max(0.0) * rad.sin() + line_height.max(0.0) * rad.cos()
}

/// How far a name of `text_width` reaches SIDEWAYS from the middle of the
/// column it names, in points: the room a host owes the ink on the side it
/// leans over.
///
/// It is the run's own horizontal reach, `text_width * cos(angle)`, and it
/// is deliberately measured from the anchor rather than from the column's
/// edge: the column's width is usually being decided by the same sum, and a
/// reserve that depends on the answer cannot be used to find it.
pub fn diagonal_overhang(text_width: f64, angle_deg: f64) -> f64 {
    let rad = angle_deg.clamp(0.0, 90.0).to_radians();
    text_width.max(0.0) * rad.cos()
}

/// Where the name goes over the box it names.
///
/// The anchor is the box's BOTTOM CENTRE in both leans, and that is the
/// whole point of the control: whatever the name is, one end of it stands on
/// the middle of its own column, right above whatever the column holds. What
/// changes with the lean is WHICH end, and so which side the rest hangs over.
///
/// At nought degrees this degenerates to a plain horizontal label sitting on
/// the box's bottom edge — a `Fall` ending on the centre, a `Rise` starting
/// there — which is the honest answer rather than a special case: a column
/// wide enough not to need the trick does not need a different control.
pub fn diagonal_run(
    box_: Rect,
    text_width: f64,
    line_height: f64,
    angle_deg: f64,
    lean: DiagonalLean,
) -> DiagonalRun {
    let rad = angle_deg.clamp(0.0, 90.0).to_radians();
    let (sin, cos) = (rad.sin(), rad.cos());
    let width = text_width.max(0.0);
    let line = line_height.max(0.0);
    let anchor = dvec2(box_.pos.x + box_.size.x * 0.5, box_.pos.y + box_.size.y);
    // The glyph band lies from the baseline UP by one line, so the rotated
    // band reaches `width * sin + line * cos` above the anchor either way.
    let top = anchor.y - width * sin - line * cos;
    let (angle, start, end, left, right) = match lean {
        DiagonalLean::Fall => {
            let start = dvec2(anchor.x - width * cos, anchor.y - width * sin);
            // Leaning down to the right, the band's far corner is the one
            // that reaches furthest right; the pen point is the left edge.
            (rad, start, anchor, start.x, start.x + width * cos + line * sin)
        }
        DiagonalLean::Rise => {
            let end = dvec2(anchor.x + width * cos, anchor.y - width * sin);
            // Leaning up to the right, the band hangs back over the pen.
            (-rad, anchor, end, anchor.x - line * sin, anchor.x + width * cos)
        }
    };
    DiagonalRun {
        start,
        end,
        angle,
        bounds: Rect {
            pos: dvec2(left, top),
            size: dvec2(right - left, anchor.y - top),
        },
    }
}

/// Write `text` across the corner of `box_`, and say where it went.
///
/// The one copy of the glyph walk: a straight baseline is one direction for
/// every glyph, so the walk along it is the run's own pen advances and
/// nothing else — the rotation origin is the pen point, the ink sits one
/// bearing along from it, and every glyph turns by the same angle. One draw
/// call for the whole name rather than one per letter.
///
/// Nothing is clipped here and no turtle is opened: the ink is MEANT to
/// cross its neighbours, and the caller owes it the room. `glyphs` is the
/// caller's own scratch, kept so a wall of these does not allocate once a
/// frame each. `None` when there is nothing to draw — an empty name, or a
/// box with a side that no one has sized, which has no centre to stand a
/// name on.
pub fn draw_diagonal_name(
    draw_text: &mut DrawRotatedText,
    cx: &mut Cx2d,
    glyphs: &mut Vec<PathGlyphInstance>,
    box_: Rect,
    text: &str,
    angle_deg: f64,
    lean: DiagonalLean,
) -> Option<DiagonalRun> {
    if text.is_empty() || !box_.size.x.is_finite() || !box_.size.y.is_finite() {
        return None;
    }
    let run = draw_text.prepare_single_line_run(cx, text)?;
    let placed = diagonal_run(
        box_,
        run.width_in_lpxs as f64,
        (run.ascender_in_lpxs - run.descender_in_lpxs) as f64,
        angle_deg,
        lean,
    );
    let dir = dvec2(placed.angle.cos(), placed.angle.sin());
    glyphs.clear();
    for glyph in &run.glyphs {
        if glyph.advance_in_lpxs <= 0.0 {
            continue;
        }
        let pen = placed.start + dir * glyph.pen_x_in_lpxs as f64;
        let ink = pen + dir * glyph.offset_x_in_lpxs as f64;
        glyphs.push(PathGlyphInstance {
            glyph_origin: Point::new(ink.x as f32, ink.y as f32),
            rotation_origin: Point::new(pen.x as f32, pen.y as f32),
            font_size_in_lpxs: glyph.font_size_in_lpxs,
            rasterized: glyph.rasterized,
            angle: placed.angle as f32,
        });
    }
    // The camera, warp and fade uniforms this shader carries for the map are
    // left where their defaults are — identity, no fold, no fade — so what is
    // drawn is the placement above and nothing on top of it.
    draw_text.begin_glyph_batch(cx);
    draw_text.draw_path_glyphs(cx, glyphs);
    draw_text.end_glyph_batch(cx);
    Some(placed)
}

/// How wide `text` is in one line of `draw_text`'s face, in points: what
/// [`diagonal_row_height`] and [`diagonal_overhang`] want for a name that
/// has not been drawn yet.
pub fn diagonal_text_width(draw_text: &DrawRotatedText, cx: &mut Cx2d, text: &str) -> f64 {
    match draw_text.prepare_single_line_run(cx, text) {
        Some(run) => run.width_in_lpxs as f64,
        None => 0.0,
    }
}

/// A name written across the corner of the box it names, in the app theme's
/// own face and ink. No gesture, no focus, no actions: it is a label.
///
/// The panel kit has one of these too, and this is not a duplicate of it:
/// `FabDiagonalLabel` draws from the panel's private table on purpose, so
/// that a panel looks like a panel wherever it is dropped, and an app's
/// heading has to follow the app's theme instead. The arithmetic and the
/// glyph walk are shared; only the palette differs.
#[derive(Script, ScriptHook, Widget)]
pub struct DiagonalLabel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_text: DrawRotatedText,
    #[walk]
    walk: Walk,

    /// The name.
    #[live]
    text: String,
    /// How far from the horizontal the name is turned, in degrees, 0 to 90.
    #[live(45.0)]
    angle: f64,
    /// Which way it leans, and so which side it hangs over.
    #[live(DiagonalLean::Rise)]
    lean: DiagonalLean,

    /// The claimed box, kept so a set of the name can ask for the frame that
    /// shows it. The glyphs are not it: a label with nothing in it draws no
    /// glyphs and would then have no area to redraw from, which is exactly
    /// the moment a host fills the header in.
    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    glyphs: Vec<PathGlyphInstance>,
    /// Where the last draw actually put the ink.
    #[rust]
    last_run: Option<DiagonalRun>,
}

impl DiagonalLabel {
    /// Where the last draw put the name, or `None` before it has drawn one.
    pub fn last_run(&self) -> Option<DiagonalRun> {
        self.last_run
    }

    /// How tall a header row has to be for `longest` at this label's angle
    /// and face, in points.
    ///
    /// The host asks once, with the longest name it will ever write, and
    /// fixes the row at the answer; every shorter name then hangs from the
    /// same bottom line.
    pub fn row_height_for(&self, cx: &mut Cx2d, longest: &str) -> f64 {
        match self.draw_text.prepare_single_line_run(cx, longest) {
            Some(run) => diagonal_row_height(
                run.width_in_lpxs as f64,
                (run.ascender_in_lpxs - run.descender_in_lpxs) as f64,
                self.angle,
            ),
            None => 0.0,
        }
    }
}

impl Widget for DiagonalLabel {
    fn text(&self) -> String {
        self.text.clone()
    }

    /// Says nothing when the name has not changed: a header written into a
    /// fixed slot on every draw would otherwise dirty the draw list every
    /// frame and redraw the page forever.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text == v {
            return;
        }
        self.text.clear();
        self.text.push_str(v);
        self.redraw(cx);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // The box is CLAIMED and then drawn over, never drawn in: no turtle
        // is begun here, so this widget pushes no clip of its own and the
        // name is free to cross its neighbours.
        let rect = cx.walk_turtle(walk);
        cx.add_aligned_rect_area(&mut self.area, rect);
        let mut glyphs = std::mem::take(&mut self.glyphs);
        self.last_run = draw_diagonal_name(
            &mut self.draw_text,
            cx,
            &mut glyphs,
            rect,
            &self.text,
            self.angle,
            self.lean,
        );
        self.glyphs = glyphs;
        DrawStep::done()
    }
}

impl DiagonalLabelRef {
    pub fn text(&self) -> String {
        self.borrow().map_or_else(String::new, |inner| inner.text())
    }

    /// Emits nothing. See [`DiagonalLabel::set_text`].
    pub fn set_text(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, text);
        }
    }

    /// Where the last draw put the name. See [`DiagonalLabel::last_run`].
    pub fn last_run(&self) -> Option<DiagonalRun> {
        self.borrow().and_then(|inner| inner.last_run())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column() -> Rect {
        Rect { pos: dvec2(100.0, 40.0), size: dvec2(26.0, 20.0) }
    }

    /// Whichever way it leans, one end of the name stands on the middle of
    /// its own column's bottom edge. That is what makes it a header rather
    /// than a decoration: a name anchored anywhere else names a column it is
    /// not over.
    #[test]
    fn a_name_stands_on_the_middle_of_its_own_column() {
        let b = column();
        let anchor = dvec2(b.pos.x + b.size.x * 0.5, b.pos.y + b.size.y);
        let fall = diagonal_run(b, 50.0, 10.0, 45.0, DiagonalLean::Fall);
        assert!((fall.end.x - anchor.x).abs() < 1e-9 && (fall.end.y - anchor.y).abs() < 1e-9);
        let rise = diagonal_run(b, 50.0, 10.0, 45.0, DiagonalLean::Rise);
        assert!((rise.start.x - anchor.x).abs() < 1e-9 && (rise.start.y - anchor.y).abs() < 1e-9);
    }

    /// The two leans hang over opposite sides, which is the whole reason
    /// there are two of them.
    #[test]
    fn the_leans_hang_over_opposite_sides() {
        let b = column();
        let fall = diagonal_run(b, 60.0, 10.0, 45.0, DiagonalLean::Fall);
        let rise = diagonal_run(b, 60.0, 10.0, 45.0, DiagonalLean::Rise);
        assert!(fall.bounds.pos.x < b.pos.x, "a fall hangs over the left");
        assert!(
            rise.bounds.pos.x + rise.bounds.size.x > b.pos.x + b.size.x,
            "a rise hangs over the right"
        );
    }

    /// The reserve a host keeps for the ink is the run's own sideways reach,
    /// so a name placed on the last column cannot land outside it.
    #[test]
    fn the_overhang_covers_the_reach() {
        let b = column();
        let middle = b.pos.x + b.size.x * 0.5;
        for lean in [DiagonalLean::Rise, DiagonalLean::Fall] {
            for angle in [15.0, 30.0, 45.0, 60.0] {
                let run = diagonal_run(b, 80.0, 10.0, angle, lean);
                let reach = match lean {
                    DiagonalLean::Rise => run.bounds.pos.x + run.bounds.size.x - middle,
                    DiagonalLean::Fall => middle - run.bounds.pos.x,
                };
                assert!(
                    reach <= diagonal_overhang(80.0, angle) + 1e-9,
                    "the {lean:?} at {angle} degrees reaches past its reserve"
                );
            }
        }
    }

    /// A taller row for a longer name and for a steeper angle, and at nought
    /// degrees exactly one flat line — which is what makes an angle of
    /// nought the old flat heading rather than a special case in the caller.
    #[test]
    fn the_row_grows_with_the_name_and_the_angle() {
        assert!(diagonal_row_height(20.0, 10.0, 45.0) < diagonal_row_height(60.0, 10.0, 45.0));
        assert!(diagonal_row_height(60.0, 10.0, 45.0) < diagonal_row_height(60.0, 10.0, 60.0));
        assert!((diagonal_row_height(50.0, 10.0, 0.0) - 10.0).abs() < 1e-9);
        assert!((diagonal_overhang(50.0, 0.0) - 50.0).abs() < 1e-9);
    }
}

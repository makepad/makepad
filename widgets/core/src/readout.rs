//! Readout -- a number or a short code drawn as seven-segment cells.
//!
//! The library could print a value in any font it had, and that is the one
//! thing an instrument does not do: a display lights segments, and the
//! segments it is not lighting are still there, faintly, behind the ones it
//! is. That ghost is what makes a readout read as a display rather than as
//! text set in a digital-looking face, and it is why this is a widget with
//! its own shader and not a font.
//!
//! # Cells
//!
//! The text is turned into cells before anything is drawn (`readout_cells`):
//! one cell per digit, letter, minus or space, a decimal point riding on the
//! cell before it the way it does on a real display, and a colon as a narrow
//! cell of its own. With `cells` at zero the readout is as wide as its text;
//! with a count it is always that wide, padded with blank cells on the side
//! away from `justify`, so a changing number does not move its neighbours.
//!
//! # Batching
//!
//! Every cell is one quad whose segments, point, colon, ghost and slant ride
//! in its instance, so every readout in a window that shares the ink and the
//! stroke goes out as one draw call however many digits they show. Nothing
//! reads `draw_pass.time`: a readout that is not being written costs nothing.
//!
//! # No halo
//!
//! Measured on the references, lit digits carry no light beyond their own
//! antialiased edge. The face draws none, and a sheet that wants one draws
//! it in its own `pixel`.
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    /** Which end of a fixed row of cells the text sits against. */
    mod.widgets.ReadoutJustify = #(ReadoutJustify::script_api(vm))

    mod.widgets.DrawReadoutCellBase = #(DrawReadoutCell::script_component(vm))
    set_type_default() do #(DrawReadoutCell::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ReadoutBase = #(Readout::register_widget(vm))
    /** A number or a short code drawn as seven-segment cells, the unlit
     * segments faintly behind the lit ones. Minus, blank, decimal point and
     * colon; a slant; a fixed number of cells or as many as the text. */
    mod.widgets.Readout = set_type_default() do mod.widgets.ReadoutBase{
        width: Fit
        height: Fit
        /** what the readout shows: digits, a few letters, minus, point, colon and space */
        text: "0"
        /** a fixed number of cells; 0 makes the readout as wide as its text 0..16 step 1 */
        cells: 0
        /** the end of a fixed row the text sits against */
        justify: mod.widgets.ReadoutJustify.Right
        /** the height of a digit, in points 6..96 step 1 */
        digit_height: theme.size_icon_m
        /** the advance of a digit cell, as a share of the digit height 0.4..1.2 step 0.01 */
        pitch: 0.7
        /** the advance of a colon cell, as a share of the digit height 0.2..0.6 step 0.01 */
        colon_pitch: 0.34
        /** how much of an unlit segment shows behind the lit ones 0..0.3 step 0.01 */
        ghost: theme.screen_ghost
        /** the lean of the digits, as a run over the digit height; 0.1 is about six degrees 0..0.25 step 0.01 */
        slant: 0.0
        /** dimmed, for a value that is not live 0..1 step 1 */
        disabled: false

        // `mask`..`opacity` are the instances: they ride in the draw
        // struct, so every other property here is a uniform or the
        // instance slots stop lining up with the struct.
        draw_bg +: {
            mask: 0.0
            dot: 0.0
            colon: 0.0
            span: 0.0
            ghost: 0.1
            slant: 0.0
            opacity: 1.0
            /** segment thickness, as a share of the digit height 0.05..0.2 step 0.005 */
            stroke: uniform(0.12)
            /** the clear gap at each end of a segment, as a share of the digit height; never under a device pixel across a joint 0..0.06 step 0.002 */
            gap: uniform(0.018)
            /** the width of a digit, as a share of its height 0.3..0.8 step 0.01 */
            glyph_width: uniform(0.5)
            /** the lit ink; the ghost is this ink at `ghost` strength */
            color: uniform(theme.color_screen_ink)

            // One mitred segment in digit units (the digit height is 1):
            // a bar from `a` to `b`, `s` thick, its ends cut at 45 degrees
            // and pulled back by the gap so neighbours do not touch. At a
            // small size the gap is held to a device pixel across the
            // mitre, or the joints fill in.
            segment: fn(q: vec2, a: vec2, b: vec2, s: float) -> float {
                let m = (a + b) * 0.5
                let horiz = step(abs(a.y - b.y), abs(a.x - b.x))
                let r = q - m
                let t = abs(mix(vec2(r.y, r.x), r, horiz))
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5) / max(self.rect_size.y, 1.0)
                let half_len = length(b - a) * 0.5 - max(self.gap, px * 0.71)
                return max(t.y - s * 0.5, (t.x + t.y - half_len) * 0.70710678)
            }

            // 1 when bit `b` (a power of two) of the mask is set.
            lit_bit: fn(b: float) -> float {
                return step(0.5, fract(floor(self.mask / b) * 0.5))
            }

            // The distance to the lit segments (x) and to all seven (y),
            // for the digit laid out with its top left at the origin.
            digit: fn(q: vec2) -> vec2 {
                let s = self.stroke
                let w = self.glyph_width
                let l = s * 0.5
                let r = w - s * 0.5
                let t = s * 0.5
                let m = 0.5
                let bo = 1.0 - s * 0.5
                let da = self.segment(q, vec2(l, t), vec2(r, t), s)
                let db = self.segment(q, vec2(r, t), vec2(r, m), s)
                let dc = self.segment(q, vec2(r, m), vec2(r, bo), s)
                let dd = self.segment(q, vec2(l, bo), vec2(r, bo), s)
                let de = self.segment(q, vec2(l, m), vec2(l, bo), s)
                let df = self.segment(q, vec2(l, t), vec2(l, m), s)
                let dg = self.segment(q, vec2(l, m), vec2(r, m), s)
                let all = min(min(min(da, db), min(dc, dd)), min(min(de, df), dg))
                var lit = 10.0
                lit = min(lit, da + (1.0 - self.lit_bit(1.0)) * 10.0)
                lit = min(lit, db + (1.0 - self.lit_bit(2.0)) * 10.0)
                lit = min(lit, dc + (1.0 - self.lit_bit(4.0)) * 10.0)
                lit = min(lit, dd + (1.0 - self.lit_bit(8.0)) * 10.0)
                lit = min(lit, de + (1.0 - self.lit_bit(16.0)) * 10.0)
                lit = min(lit, df + (1.0 - self.lit_bit(32.0)) * 10.0)
                lit = min(lit, dg + (1.0 - self.lit_bit(64.0)) * 10.0)
                return vec2(lit, all)
            }

            pixel: fn() {
                let h = max(self.rect_size.y, 1.0)
                // The quad overhangs the cell by the slant on both sides, so
                // a leaning digit is not cut at its own cell's edge.
                let over = (self.rect_size.x - self.span) * 0.5
                let p = (self.pos * self.rect_size - vec2(over, 0.0)) / h
                // Sheared about the middle: the top leans right.
                let q = vec2(p.x + (p.y - 0.5) * self.slant, p.y)
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5) / h
                let s = self.stroke
                var lit = 10.0
                var all = 10.0
                if self.colon > 0.5 {
                    // Two dots, centred in the narrow cell.
                    let cx = self.span / h * 0.5
                    let rr = s * 0.62
                    let d1 = length(q - vec2(cx, 0.31)) - rr
                    let d2 = length(q - vec2(cx, 0.69)) - rr
                    all = min(d1, d2)
                    lit = all
                } else {
                    // The digit sits a little right of the cell's start, and
                    // the point in the room its advance leaves after it.
                    let x0 = (self.span / h - self.glyph_width) * 0.3
                    let dq = q - vec2(x0, 0.0)
                    let dd = self.digit(dq)
                    let room = self.span / h - x0 - self.glyph_width
                    let dp = length(dq - vec2(self.glyph_width + room * 0.45, 1.0 - s * 0.62)) - s * 0.62
                    all = min(dd.y, dp)
                    lit = min(dd.x, dp + (1.0 - step(0.5, self.dot)) * 10.0)
                }
                let a_lit = Finish.cover(lit, px)
                let a_all = Finish.cover(all, px)
                let a = max(a_lit, a_all * clamp(self.ghost, 0.0, 1.0)) * self.opacity
                return vec4(self.color.rgb * a * self.color.a, a * self.color.a)
            }
        }
    }
}

/// Which end of a fixed row of cells the text sits against.
#[derive(Copy, Clone, Debug, PartialEq, Default, Script, ScriptHook)]
pub enum ReadoutJustify {
    /// Numbers: the last digit stays put as the value grows.
    #[pick]
    #[default]
    Right,
    /// Codes and words.
    Left,
}

/// One cell of a readout: the segments it lights, a point riding on it,
/// or a colon in place of a digit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadoutCell {
    /// Segments a to g as bits 1 to 64: a the top, then clockwise, g the middle.
    pub mask: u8,
    pub dot: bool,
    pub colon: bool,
}

impl ReadoutCell {
    pub const BLANK: ReadoutCell = ReadoutCell { mask: 0, dot: false, colon: false };

    fn glyph(mask: u8) -> Self {
        Self { mask, ..Self::BLANK }
    }
}

/// The segments a character lights, or `None` for a character a seven
/// segment cell cannot show. Letters take the one form each that reads on
/// seven segments, upper or lower case as the display can draw them.
pub fn segments_of(c: char) -> Option<u8> {
    Some(match c {
        '0' | 'O' => 63,
        '1' | 'I' => 6,
        '2' => 91,
        '3' => 79,
        '4' => 102,
        '5' | 'S' | 's' => 109,
        '6' => 125,
        '7' => 7,
        '8' => 127,
        '9' => 111,
        '-' => 64,
        '_' => 8,
        '=' => 72,
        ' ' => 0,
        '°' => 99,
        'A' | 'a' => 119,
        'B' | 'b' => 124,
        'C' => 57,
        'c' => 88,
        'D' | 'd' => 94,
        'E' | 'e' => 121,
        'F' | 'f' => 113,
        'G' | 'g' => 61,
        'H' => 118,
        'h' => 116,
        'i' => 4,
        'J' | 'j' => 30,
        'L' | 'l' => 56,
        'N' | 'n' => 84,
        'o' => 92,
        'P' | 'p' => 115,
        'Q' | 'q' => 103,
        'R' | 'r' => 80,
        'T' | 't' => 120,
        'U' => 62,
        'u' => 28,
        'Y' | 'y' => 110,
        _ => return None,
    })
}

/// The cells a text is drawn in.
///
/// A point or a comma rides on the cell before it, as it does on a display;
/// one with no digit to ride on (at the start, after a colon or after
/// another point) takes a blank cell of its own. A colon is a cell of its
/// own. A character no cell can show is a blank. With `count` above zero the
/// row is exactly that long: padded with blanks on the side away from
/// `justify`, or, when the text is longer, cut from that side, so what stays
/// is the end the text is justified to.
pub fn readout_cells(text: &str, count: usize, justify: ReadoutJustify) -> Vec<ReadoutCell> {
    let mut cells: Vec<ReadoutCell> = Vec::with_capacity(text.len().max(count));
    for c in text.chars() {
        match c {
            '.' | ',' => match cells.last_mut() {
                Some(last) if !last.colon && !last.dot => last.dot = true,
                _ => cells.push(ReadoutCell { dot: true, ..ReadoutCell::BLANK }),
            },
            ':' => cells.push(ReadoutCell { colon: true, ..ReadoutCell::BLANK }),
            c => cells.push(ReadoutCell::glyph(segments_of(c).unwrap_or(0))),
        }
    }
    if count == 0 || cells.len() == count {
        return cells;
    }
    if cells.len() > count {
        let cut = cells.len() - count;
        return match justify {
            ReadoutJustify::Right => cells.split_off(cut),
            ReadoutJustify::Left => {
                cells.truncate(count);
                cells
            }
        };
    }
    let pad = vec![ReadoutCell::BLANK; count - cells.len()];
    match justify {
        ReadoutJustify::Right => pad.into_iter().chain(cells).collect(),
        ReadoutJustify::Left => cells.into_iter().chain(pad).collect(),
    }
}

/// A number as a readout shows it: `decimals` places after the point, and a
/// minus in front of a negative value. When the readout has a fixed number
/// of cells and the number needs more, every cell is a minus rather than the
/// number cut short: a display that dropped a digit would show a different
/// number, and one showing dashes is plainly out of range.
pub fn readout_number_text(value: f64, decimals: usize, cells: usize) -> String {
    if !value.is_finite() {
        return "-".repeat(cells.max(1));
    }
    let mut text = format!("{:.*}", decimals, value);
    // A value that rounds to zero is not negative.
    if text.starts_with('-') && text[1..].chars().all(|c| c == '0' || c == '.') {
        text.remove(0);
    }
    let needed = readout_cells(&text, 0, ReadoutJustify::Right).len();
    if cells > 0 && needed > cells {
        return "-".repeat(cells);
    }
    text
}

/// One readout cell's shader. The instances are the whole of what differs
/// between one cell and the next, so every cell batches.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawReadoutCell {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    mask: f32,
    #[live]
    dot: f32,
    #[live]
    colon: f32,
    /// The cell's advance in points; the quad is wider by the slant.
    #[live]
    span: f32,
    #[live]
    ghost: f32,
    #[live]
    slant: f32,
    #[live]
    opacity: f32,
}

/// A number or a short code drawn as seven-segment cells.
#[derive(Script, ScriptHook, Widget)]
pub struct Readout {
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
    draw_bg: DrawReadoutCell,
    /// What the readout shows.
    #[live]
    pub text: String,
    /// A fixed number of cells; zero makes the readout as wide as its text.
    #[live]
    pub cells: usize,
    /// The end of a fixed row the text sits against.
    #[live]
    pub justify: ReadoutJustify,
    /// The height of a digit, in points.
    #[live(16.0)]
    pub digit_height: f64,
    /// A digit cell's advance, as a share of the digit height.
    #[live(0.7)]
    pub pitch: f64,
    /// A colon cell's advance, as a share of the digit height.
    #[live(0.34)]
    pub colon_pitch: f64,
    /// How much of an unlit segment shows behind the lit ones.
    #[live(0.1)]
    pub ghost: f64,
    /// The lean of the digits, as a run over the digit height.
    #[live]
    pub slant: f64,
    #[live]
    pub disabled: bool,
    #[rust]
    #[area]
    area: Area,
}

impl Readout {
    /// The cells the current text is drawn in.
    pub fn cells_now(&self) -> Vec<ReadoutCell> {
        readout_cells(&self.text, self.cells, self.justify)
    }

    /// Show a text: digits, the letters seven segments can draw, minus,
    /// point, colon and space.
    pub fn set_text(&mut self, cx: &mut Cx, text: &str) {
        if self.text != text {
            self.text = text.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    /// Show a number with `decimals` places after the point. A number that
    /// does not fit a fixed row of cells shows as a row of minus signs.
    pub fn set_number(&mut self, cx: &mut Cx, value: f64, decimals: usize) {
        let text = readout_number_text(value, decimals, self.cells);
        self.set_text(cx, &text);
    }

    fn span_of(&self, cell: &ReadoutCell) -> f64 {
        let h = self.digit_height.max(1.0);
        if cell.colon {
            h * self.colon_pitch.max(0.05)
        } else {
            h * self.pitch.max(0.1)
        }
    }
}

impl Widget for Readout {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let cells = self.cells_now();
        let h = self.digit_height.max(1.0);
        let spans: Vec<f64> = cells.iter().map(|cell| self.span_of(cell)).collect();
        let total: f64 = spans.iter().sum();
        let pad = self.layout.padding;
        // A fitted readout is exactly as large as its cells, worked out here
        // rather than walked, because the cells are drawn where they go and
        // not walked one by one.
        let mut walk = walk;
        if walk.width.is_fit() {
            walk.width = Size::Fixed(total + pad.left + pad.right);
        }
        if walk.height.is_fit() {
            walk.height = Size::Fixed(h + pad.top + pad.bottom);
        }
        cx.begin_turtle(walk, Layout::default());
        let rect = cx.turtle().rect();
        let inner_w = rect.size.x - pad.left - pad.right;
        let inner_h = rect.size.y - pad.top - pad.bottom;
        let mut x = match self.justify {
            ReadoutJustify::Right => rect.pos.x + pad.left + (inner_w - total).max(0.0),
            ReadoutJustify::Left => rect.pos.x + pad.left,
        };
        let y = rect.pos.y + pad.top + ((inner_h - h) * 0.5).max(0.0);
        let slant = self.slant.clamp(-0.5, 0.5);
        // The quad reaches past the cell by the lean and a point more, so a
        // slanted digit is drawn whole and the antialiased edge is not cut.
        let over = slant.abs() * h * 0.5 + 1.0;
        self.draw_bg.ghost = self.ghost.clamp(0.0, 1.0) as f32;
        self.draw_bg.slant = slant as f32;
        self.draw_bg.opacity = if self.disabled { 0.45 } else { 1.0 };
        for (cell, span) in cells.iter().zip(spans) {
            self.draw_bg.mask = cell.mask as f32;
            self.draw_bg.dot = if cell.dot { 1.0 } else { 0.0 };
            self.draw_bg.colon = if cell.colon { 1.0 } else { 0.0 };
            self.draw_bg.span = span as f32;
            self.draw_bg.draw_abs(
                cx,
                Rect { pos: dvec2(x - over, y), size: dvec2(span + over * 2.0, h) },
            );
            x += span;
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        Readout::set_text(self, cx, v);
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
        Some(self.text.clone())
    }
}

impl ReadoutRef {
    pub fn set_number(&self, cx: &mut Cx, value: f64, decimals: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_number(cx, value, decimals);
        }
    }

    /// The cells the readout is showing now.
    pub fn cells(&self) -> Vec<ReadoutCell> {
        self.borrow().map(|inner| inner.cells_now()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masks(cells: &[ReadoutCell]) -> Vec<u8> {
        cells.iter().map(|c| c.mask).collect()
    }

    /// Digits map to their segments, one cell each, in order.
    #[test]
    fn digits_are_one_cell_each() {
        let cells = readout_cells("0123456789", 0, ReadoutJustify::Right);
        assert_eq!(masks(&cells), vec![63, 6, 91, 79, 102, 109, 125, 7, 127, 111]);
        assert!(cells.iter().all(|c| !c.dot && !c.colon));
    }

    /// A point rides on the digit before it rather than taking a cell, the
    /// way a display draws it; one with nothing to ride on takes a blank.
    #[test]
    fn a_point_rides_on_the_digit_before_it() {
        let cells = readout_cells("-12.5", 0, ReadoutJustify::Right);
        assert_eq!(masks(&cells), vec![64, 6, 91, 109], "minus, one, two, five: four cells");
        assert_eq!(cells.iter().map(|c| c.dot).collect::<Vec<_>>(), vec![false, false, true, false]);
        let lead = readout_cells(".5", 0, ReadoutJustify::Right);
        assert_eq!(lead.len(), 2, "a point with no digit before it is a blank cell");
        assert!(lead[0].dot && lead[0].mask == 0);
        let twice = readout_cells("1..", 0, ReadoutJustify::Right);
        assert_eq!(twice.len(), 2, "a second point does not stack on the first");
        let comma = readout_cells("3,1", 0, ReadoutJustify::Right);
        assert!(comma[0].dot, "a comma is a point");
    }

    /// A colon is a narrow cell of its own, and a point never rides on it.
    #[test]
    fn a_colon_is_a_cell_of_its_own() {
        let cells = readout_cells("12:30", 0, ReadoutJustify::Right);
        assert_eq!(cells.len(), 5);
        assert!(cells[2].colon && cells[2].mask == 0);
        let after = readout_cells(":.", 0, ReadoutJustify::Right);
        assert_eq!(after.len(), 2, "the point after a colon takes its own cell");
    }

    /// A fixed row is exactly that long whatever the text: blanks on the
    /// side away from the justified end, and a long text cut from that side.
    #[test]
    fn a_fixed_row_pads_and_cuts_on_the_far_side() {
        let right = readout_cells("42", 4, ReadoutJustify::Right);
        assert_eq!(masks(&right), vec![0, 0, 102, 91]);
        let left = readout_cells("42", 4, ReadoutJustify::Left);
        assert_eq!(masks(&left), vec![102, 91, 0, 0]);
        let cut_right = readout_cells("12345", 3, ReadoutJustify::Right);
        assert_eq!(masks(&cut_right), vec![79, 102, 109], "the last three stay");
        let cut_left = readout_cells("12345", 3, ReadoutJustify::Left);
        assert_eq!(masks(&cut_left), vec![6, 91, 79], "the first three stay");
    }

    /// Short codes: the letters seven segments can draw, blanks for the rest.
    #[test]
    fn letters_take_the_form_that_reads_and_others_are_blank() {
        let cells = readout_cells("dB oFF", 0, ReadoutJustify::Left);
        assert_eq!(masks(&cells), vec![94, 124, 0, 92, 113, 113]);
        assert_eq!(segments_of('W'), None);
        assert_eq!(masks(&readout_cells("W", 0, ReadoutJustify::Left)), vec![0]);
    }

    /// A number with its decimals, a minus only where the value is below
    /// zero after rounding, and dashes rather than a number cut short.
    #[test]
    fn a_number_is_written_with_its_decimals_or_as_dashes_when_it_does_not_fit() {
        assert_eq!(readout_number_text(3.14159, 2, 0), "3.14");
        assert_eq!(readout_number_text(-12.0, 1, 0), "-12.0");
        assert_eq!(readout_number_text(-0.004, 2, 0), "0.00", "a value that rounds to zero is not negative");
        assert_eq!(readout_number_text(440.0, 1, 4), "440.0", "the point rides, so 440.0 is four cells");
        assert_eq!(readout_number_text(12345.0, 0, 4), "----");
        assert_eq!(readout_number_text(f64::NAN, 1, 3), "---");
    }
}

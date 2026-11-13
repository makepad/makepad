//! Static, unhinted TrueType outlines for an embedded PDF font program.

use {
    crate::{
        sfnt::{self, put_u16, put_u32, u16_at},
        SubsetError,
    },
    ttf_parser::{GlyphId, OutlineBuilder, Tag},
};

#[derive(Clone, Copy, PartialEq)]
struct Point {
    x: i16,
    y: i16,
    on_curve: bool,
}

#[derive(Default)]
struct Builder {
    points: Vec<Point>,
    ends: Vec<usize>,
    start: usize,
    error: Option<SubsetError>,
}

impl Builder {
    fn point(&mut self, x: f32, y: f32, on_curve: bool) {
        if self.error.is_some() {
            return;
        }
        let (x, y) = (x.round(), y.round());
        let fits = |v: f32| v.is_finite() && v >= i16::MIN as f32 && v <= i16::MAX as f32;
        if !fits(x) || !fits(y) {
            self.error = Some(SubsetError::Unsupported("static glyph coordinates need a Type 3 font"));
            return;
        }
        self.points.push(Point { x: x as i16, y: y as i16, on_curve });
    }

    fn write(&self, out: &mut Vec<u8>) -> Result<[i16; 4], SubsetError> {
        let contours = i16::try_from(self.ends.len())
            .map_err(|_| SubsetError::Unsupported("too many static contours; needs a Type 3 font"))?;
        u16::try_from(self.points.len())
            .map_err(|_| SubsetError::Unsupported("too many static points; needs a Type 3 font"))?;
        let first = self.points.first().ok_or(SubsetError::Malformed("outline without points"))?;
        if self.ends.last().copied() != Some(self.points.len() - 1) {
            return Err(SubsetError::Malformed("unfinished glyph contour"));
        }
        let mut bbox = [first.x, first.y, first.x, first.y];
        for point in &self.points {
            bbox[0] = bbox[0].min(point.x);
            bbox[1] = bbox[1].min(point.y);
            bbox[2] = bbox[2].max(point.x);
            bbox[3] = bbox[3].max(point.y);
        }
        out.extend_from_slice(&contours.to_be_bytes());
        for value in bbox {
            out.extend_from_slice(&value.to_be_bytes());
        }
        for &end in &self.ends {
            out.extend_from_slice(&(end as u16).to_be_bytes());
        }
        out.extend_from_slice(&0u16.to_be_bytes()); // No instructions.
        // Full signed deltas keep the encoding simple; only the on-curve
        // flag is needed. Coordinates accumulate across contour boundaries.
        out.extend(self.points.iter().map(|point| u8::from(point.on_curve)));
        for axis in 0..2 {
            let mut previous = 0;
            for point in &self.points {
                let value = i32::from(if axis == 0 { point.x } else { point.y });
                let delta = i16::try_from(value - previous)
                    .map_err(|_| SubsetError::Unsupported("static glyph deltas need a Type 3 font"))?;
                out.extend_from_slice(&delta.to_be_bytes());
                previous = value;
            }
        }
        Ok(bbox)
    }
}

impl OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = self.points.len();
        self.point(x, y, true);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.point(x, y, true);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.point(x1, y1, false);
        self.point(x, y, true);
    }

    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
        self.error = Some(SubsetError::Unsupported("cubic outlines need a Type 3 font"));
    }

    fn close(&mut self) {
        if self.error.is_some() || self.points.len() == self.start {
            return;
        }
        // ttf-parser emits the closing line/quad explicitly. Leaving its
        // endpoint in the list would add another closing line on reparse.
        // Keep the control point of a closing quad, and keep a one-point
        // contour's start. An off-curve start in the source has already
        // become move_to (possibly an implied midpoint), so it is on-curve
        // here and the emitted segment order is preserved.
        if self.points.len() > self.start + 1 && self.points.last() == self.points.get(self.start) {
            self.points.pop();
        }
        self.ends.push(self.points.len() - 1);
        self.start = self.points.len();
    }
}

/// A static TrueType font of `glyphs` from face `index` of `data` (a font or a collection) at the
/// variation `coords` (user-space `(axis tag, value)` pairs, as the text stack sets them; an axis
/// the face lacks is ignored): each listed glyph's outline and advance as `ttf-parser` gives them
/// there, written as a simple glyph (composites flattened, hinting dropped); every other id up to
/// the highest listed one is empty. Glyph ids are kept. The font holds `glyf`, `loca`, `head`,
/// `hhea`, `hmtx` and `maxp`: what a PDF CIDFontType2 program needs.
pub fn static_instance(data: &[u8], index: u32, coords: &[(u32, f32)], glyphs: &[u16]) -> Result<Vec<u8>, SubsetError> {
    let mut face = ttf_parser::Face::parse(data, index)
        .map_err(|_| SubsetError::Malformed("the parser cannot read the font"))?;
    if face.tables().glyf.is_none() {
        return Err(SubsetError::Unsupported("a face without glyf outlines needs a Type 3 font"));
    }
    if glyphs.iter().any(|&glyph| glyph >= face.number_of_glyphs()) {
        return Err(SubsetError::Malformed("glyph id outside the face"));
    }
    for &(tag, value) in coords {
        // Do not remap avar for an absent axis: set_variation does that
        // even when its axis lookup misses.
        let tag = Tag(tag);
        if face.variation_axes().into_iter().any(|axis| axis.tag == tag) {
            face.set_variation(tag, value);
        }
    }
    // RawFace selects the directory within a collection, while its table
    // offsets remain relative to the whole input. Borrow only the headers;
    // no splitting, copying or walking of the source glyf is needed.
    let need = |tag: &[u8; 4]| {
        face.raw_face().table(Tag::from_bytes(tag))
            .ok_or(SubsetError::Malformed("a required table is missing"))
    };
    let mut head = need(b"head")?.to_vec();
    let mut hhea = need(b"hhea")?.to_vec();
    u16_at(&head, 52)?;
    u16_at(&hhea, 34)?;
    let mut keep = glyphs.to_vec();
    keep.sort_unstable();
    keep.dedup();
    let count = keep.last().copied().unwrap_or(0) + 1;
    let mut remaining = keep.as_slice();
    let mut glyf = Vec::new();
    let mut loca = Vec::with_capacity(4 * (usize::from(count) + 1));
    let mut hmtx = vec![0; 4 * usize::from(count)];
    let mut bounds: Option<[i16; 4]> = None;
    let mut min_right: Option<i32> = None;
    let mut advance_max = 0;
    let mut max_points = 0;
    let mut max_contours = 0;
    for glyph in 0..count {
        let offset = u32::try_from(glyf.len()).map_err(|_| SubsetError::Overflow)?;
        loca.extend_from_slice(&offset.to_be_bytes());
        if remaining.first() != Some(&glyph) {
            continue;
        }
        remaining = &remaining[1..];
        let id = GlyphId(glyph);
        let advance = face.glyph_hor_advance(id).ok_or(SubsetError::Malformed("missing glyph advance"))?;
        put_u16(&mut hmtx, 4 * usize::from(glyph), advance);
        advance_max = advance_max.max(advance);
        let mut builder = Builder::default();
        let outlined = face.outline_glyph(id, &mut builder).is_some();
        if let Some(error) = builder.error.take() {
            return Err(error);
        }
        // A failed outline may have delivered partial callbacks. Match the
        // loader's None result, discarding them and retaining the advance.
        if !outlined {
            continue;
        }
        let bbox = builder.write(&mut glyf)?;
        put_u16(&mut hmtx, 4 * usize::from(glyph) + 2, bbox[0] as u16);
        max_points = max_points.max(builder.points.len() as u16);
        max_contours = max_contours.max(builder.ends.len() as u16);
        bounds = Some(match bounds {
            Some(old) => [old[0].min(bbox[0]), old[1].min(bbox[1]), old[2].max(bbox[2]), old[3].max(bbox[3])],
            None => bbox,
        });
        // hhea bearings and extent describe glyphs with contours. Empty
        // glyphs still contribute to advanceWidthMax above. With lsb=xMin,
        // the right bearing is advance-xMax and the extent is xMax.
        let right = i32::from(advance) - i32::from(bbox[2]);
        min_right = Some(min_right.map_or(right, |old| old.min(right)));
    }
    let offset = u32::try_from(glyf.len()).map_err(|_| SubsetError::Overflow)?;
    loca.extend_from_slice(&offset.to_be_bytes());
    let bbox = bounds.unwrap_or([0; 4]);
    for (i, value) in bbox.into_iter().enumerate() {
        put_u16(&mut head, 36 + 2 * i, value as u16);
    }
    put_u16(&mut head, 50, 1); // Long loca offsets.
    put_u16(&mut hhea, 10, advance_max);
    put_u16(&mut hhea, 12, bbox[0] as u16);
    let min_right = i16::try_from(min_right.unwrap_or(0))
        .map_err(|_| SubsetError::Unsupported("static horizontal metrics need a Type 3 font"))?;
    put_u16(&mut hhea, 14, min_right as u16);
    put_u16(&mut hhea, 16, bbox[2] as u16);
    put_u16(&mut hhea, 34, count);
    let mut maxp = vec![0; 32];
    put_u32(&mut maxp, 0, 0x0001_0000);
    put_u16(&mut maxp, 4, count);
    put_u16(&mut maxp, 6, max_points);
    put_u16(&mut maxp, 8, max_contours);
    put_u16(&mut maxp, 14, 1); // No twilight zone or instructions.
    Ok(sfnt::write(0x0001_0000, vec![
        (*b"glyf", glyf),
        (*b"loca", loca),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"maxp", maxp),
    ]))
}

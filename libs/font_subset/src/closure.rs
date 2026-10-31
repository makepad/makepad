//! The glyph closure: the glyphs shaped runs need (shaped as the draw
//! crate's shaper shapes them), the glyphs GSUB can reach from a set of
//! characters, composite components and `.notdef`.

use {
    crate::{glyf, sfnt::Tables, SubsetError},
    rustybuzz::{ttf_parser, UnicodeBuffer},
    std::collections::BTreeSet,
    ttf_parser::{gsub::SubstitutionSubtable, GlyphId},
    unicode_segmentation::UnicodeSegmentation,
};

/// One run of text as the runtime shapes it.
#[derive(Clone, Debug, Default)]
pub struct ShapeRun {
    pub text: String,
    /// Axis values (`(tag, value)`, as `FontFace::set_variations` takes
    /// them). A run whose axes animate is listed at each value it reaches
    /// where the font substitutes by axis range (GSUB feature variations),
    /// or its characters go in [`ClosureInputs::gsub_chars`].
    pub variations: Vec<(u32, f32)>,
    /// OpenType features as the shaper's `(tag, value)` pairs.
    pub features: Vec<(u32, u32)>,
    /// Shape right to left (a run the bidi algorithm resolved RTL).
    pub rtl: bool,
}

/// What a subset must draw.
#[derive(Clone, Debug, Default)]
pub struct ClosureInputs {
    pub runs: Vec<ShapeRun>,
    /// Characters whose every GSUB-reachable glyph is kept, whatever the
    /// features, script or axis values (text not known in advance, such as
    /// a counter's digits).
    pub gsub_chars: BTreeSet<char>,
}

impl ClosureInputs {
    /// Every character the inputs name: the `keep_chars` for [`crate::subset`].
    pub fn chars(&self) -> BTreeSet<char> {
        let mut chars = self.gsub_chars.clone();
        for run in &self.runs {
            chars.extend(run.text.chars());
        }
        chars
    }
}

fn tag(value: u32) -> ttf_parser::Tag {
    ttf_parser::Tag::from_bytes(&value.to_be_bytes())
}

/// How many word segments either side of a possible line break are shaped
/// on their own: contextual substitutions reach a few glyphs at most.
const BREAK_CONTEXT: usize = 3;

/// The texts `run` is shaped as: the whole run, and the text on either side
/// of every word boundary as it shapes when a line wraps there (the
/// layouter shapes each line on its own, so a word at a line edge loses its
/// neighbour's context and can take another contextual form).
fn texts(run: &ShapeRun) -> Vec<&str> {
    let bounds: Vec<usize> = run
        .text
        .split_word_bound_indices()
        .map(|(at, _)| at)
        .chain([run.text.len()])
        .collect();
    let mut texts = vec![run.text.as_str()];
    for i in 1..bounds.len().saturating_sub(1) {
        texts.push(&run.text[bounds[i.saturating_sub(BREAK_CONTEXT)]..bounds[i]]);
        texts.push(&run.text[bounds[i]..bounds[(i + BREAK_CONTEXT).min(bounds.len() - 1)]]);
    }
    texts
}

/// The shaper's face for `run`'s axis values.
fn shaper_face<'a>(font: &'a [u8], run: &ShapeRun) -> Result<rustybuzz::Face<'a>, SubsetError> {
    let mut face = rustybuzz::Face::from_slice(font, 0).ok_or(SubsetError::Malformed("the shaper cannot read the font"))?;
    let variations: Vec<rustybuzz::Variation> = run
        .variations
        .iter()
        .map(|&(t, value)| rustybuzz::Variation { tag: tag(t), value })
        .collect();
    face.set_variations(&variations);
    Ok(face)
}

/// `text` shaped as the draw crate's shaper shapes it: glyph id, cluster,
/// advances and offsets.
fn shape_text(face: &rustybuzz::Face, run: &ShapeRun, text: &str) -> Vec<(u32, u32, i32, i32, i32, i32)> {
    let features: Vec<rustybuzz::Feature> =
        run.features.iter().map(|&(t, value)| rustybuzz::Feature::new(tag(t), value, ..)).collect();
    let mut buffer = UnicodeBuffer::new();
    buffer.set_direction(if run.rtl {
        rustybuzz::Direction::RightToLeft
    } else {
        rustybuzz::Direction::LeftToRight
    });
    for (cluster, grapheme) in text.grapheme_indices(true) {
        for c in grapheme.chars() {
            buffer.add(c, cluster as u32);
        }
    }
    let glyphs = rustybuzz::shape(face, &features, buffer);
    glyphs
        .glyph_infos()
        .iter()
        .zip(glyphs.glyph_positions())
        .map(|(i, p)| (i.glyph_id, i.cluster, p.x_advance, p.y_advance, p.x_offset, p.y_offset))
        .collect()
}

fn shape(font: &[u8], run: &ShapeRun, out: &mut BTreeSet<u16>) -> Result<(), SubsetError> {
    let face = shaper_face(font, run)?;
    for text in texts(run) {
        out.extend(shape_text(&face, run, text).iter().map(|g| g.0 as u16));
    }
    Ok(())
}

#[derive(Default, PartialEq)]
struct Outline(Vec<u32>);

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.extend([0, x.to_bits(), y.to_bits()]);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.extend([1, x.to_bits(), y.to_bits()]);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.extend([2, x1.to_bits(), y1.to_bits(), x.to_bits(), y.to_bits()]);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.extend([3, x1.to_bits(), y1.to_bits(), x2.to_bits(), y2.to_bits(), x.to_bits(), y.to_bits()]);
    }
    fn close(&mut self) {
        self.0.push(4);
    }
}

/// Checks that `subset` draws `inputs` exactly as `original`: every run (and
/// its line-edge pieces) shapes to the same glyphs at the same positions, and
/// every glyph shaped has the same outline, at the run's axis values. The
/// error names the first difference.
pub fn verify(original: &[u8], subset: &[u8], inputs: &ClosureInputs) -> Result<(), String> {
    for run in &inputs.runs {
        let a = shaper_face(original, run).map_err(|e| e.to_string())?;
        let b = shaper_face(subset, run).map_err(|e| e.to_string())?;
        for text in texts(run) {
            let (sa, sb) = (shape_text(&a, run, text), shape_text(&b, run, text));
            if sa != sb {
                return Err(format!("{text:?} at {:?} shapes differently: {sa:?} vs {sb:?}", run.variations));
            }
            for &(glyph, ..) in &sa {
                let outline = |face: &rustybuzz::Face| {
                    let mut outline = Outline::default();
                    let bbox = face.outline_glyph(GlyphId(glyph as u16), &mut outline);
                    (bbox, outline)
                };
                if outline(&a) != outline(&b) {
                    return Err(format!("glyph {glyph} of {text:?} at {:?} has another outline", run.variations));
                }
            }
        }
    }
    Ok(())
}

/// The axes (tags as `u32`) that every one of `instances` (axis values a
/// face was laid out with; an axis left out is at its default) leaves at
/// the default: what [`crate::SubsetOptions::pin_axes`] can pin.
pub fn unvaried_axes(font: &[u8], instances: &[Vec<(u32, f32)>]) -> Result<Vec<u32>, SubsetError> {
    let face = ttf_parser::Face::parse(font, 0).map_err(|_| SubsetError::Malformed("the parser cannot read the font"))?;
    Ok(face
        .variation_axes()
        .into_iter()
        .filter(|axis| {
            let t = axis.tag.0;
            instances.iter().flatten().all(|&(tag, value)| {
                tag != t || value.clamp(axis.min_value, axis.max_value) == axis.def_value
            })
        })
        .map(|axis| axis.tag.0)
        .collect())
}

/// Adds every glyph any GSUB lookup can substitute for glyphs in `glyphs`
/// (all lookups, so every feature, script, language and feature variation),
/// until nothing more is reachable.
pub fn gsub_closure(face: &ttf_parser::Face, glyphs: &mut BTreeSet<u16>) {
    let Some(gsub) = face.tables().gsub else {
        return;
    };
    loop {
        let before = glyphs.len();
        for lookup in gsub.lookups {
            for subtable in lookup.subtables.into_iter::<SubstitutionSubtable>() {
                let coverage = subtable.coverage();
                let covered: Vec<(u16, u16)> = glyphs
                    .iter()
                    .filter_map(|&g| coverage.get(GlyphId(g)).map(|index| (g, index)))
                    .collect();
                for (g, index) in covered {
                    match &subtable {
                        SubstitutionSubtable::Single(single) => match single {
                            ttf_parser::gsub::SingleSubstitution::Format1 { delta, .. } => {
                                glyphs.insert(g.wrapping_add(*delta as u16));
                            }
                            ttf_parser::gsub::SingleSubstitution::Format2 { substitutes, .. } => {
                                glyphs.extend(substitutes.get(index).map(|s| s.0));
                            }
                        },
                        SubstitutionSubtable::Multiple(multiple) => {
                            if let Some(sequence) = multiple.sequences.get(index) {
                                glyphs.extend(sequence.substitutes.into_iter().map(|s| s.0));
                            }
                        }
                        SubstitutionSubtable::Alternate(alternate) => {
                            if let Some(set) = alternate.alternate_sets.get(index) {
                                glyphs.extend(set.alternates.into_iter().map(|s| s.0));
                            }
                        }
                        SubstitutionSubtable::Ligature(ligature) => {
                            if let Some(set) = ligature.ligature_sets.get(index) {
                                for lig in set {
                                    if lig.components.into_iter().all(|c| glyphs.contains(&c.0)) {
                                        glyphs.insert(lig.glyph.0);
                                    }
                                }
                            }
                        }
                        SubstitutionSubtable::ReverseChainSingle(reverse) => {
                            glyphs.extend(reverse.substitutes.get(index).map(|s| s.0));
                        }
                        // Contextual lookups substitute only through other
                        // lookups, which this walk visits anyway.
                        SubstitutionSubtable::Context(_) | SubstitutionSubtable::ChainContext(_) => {}
                    }
                }
            }
        }
        if glyphs.len() == before {
            break;
        }
    }
}

/// The glyphs a subset keeps for `inputs`: every run shaped with the
/// runtime's shaper at its axis values and features, the GSUB closure of
/// [`ClosureInputs::gsub_chars`], `.notdef`, and the components of every
/// composite glyph among them.
pub fn closure(font: &[u8], inputs: &ClosureInputs) -> Result<BTreeSet<u16>, SubsetError> {
    let tables = Tables::read(font)?;
    let face = ttf_parser::Face::parse(font, 0).map_err(|_| SubsetError::Malformed("the parser cannot read the font"))?;
    let mut glyphs = BTreeSet::from([0u16]);
    for run in &inputs.runs {
        shape(font, run, &mut glyphs)?;
    }
    if !inputs.gsub_chars.is_empty() {
        let mut reachable: BTreeSet<u16> =
            inputs.gsub_chars.iter().filter_map(|&c| face.glyph_index(c)).map(|g| g.0).collect();
        gsub_closure(&face, &mut reachable);
        glyphs.extend(reachable);
    }
    if let (Some(glyf_table), Some(loca)) = (tables.get(b"glyf"), tables.get(b"loca")) {
        let head = tables.get(b"head").ok_or(SubsetError::Malformed("no head table"))?;
        let long = crate::sfnt::u16_at(head, 50)? != 0;
        let glyphs_data = glyf::split(glyf_table, loca, long, face.number_of_glyphs())?;
        glyf::component_closure(&glyphs_data, &mut glyphs)?;
    }
    Ok(glyphs)
}

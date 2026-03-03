use {
    super::{
        font::{Font, GlyphId},
        slice::SliceExt,
        substr::Substr,
    },
    rustybuzz,
    rustybuzz::UnicodeBuffer,
    std::{
        collections::{HashMap, VecDeque},
        hash::{Hash, Hasher},
        mem,
        rc::Rc,
    },
    unicode_segmentation::UnicodeSegmentation,
};

/// Float wrapper that supports Hash and Eq via bit representation.
#[derive(Clone, Copy, Debug)]
pub struct Ems(pub f32);

impl Hash for Ems {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl PartialEq for Ems {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Ems {}

impl Default for Ems {
    fn default() -> Self {
        Ems(0.0)
    }
}

/// Text direction for shaping.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

#[derive(Debug)]
pub struct Shaper {
    reusable_glyphs: Vec<Vec<ShapedGlyph>>,
    reusable_unicode_buffer: UnicodeBuffer,
    cache_size: usize,
    cached_params: VecDeque<ShapeParams>,
    cached_results: HashMap<ShapeParams, Rc<ShapedText>>,
}

impl Shaper {
    pub fn new(settings: Settings) -> Self {
        Self {
            reusable_glyphs: Vec::new(),
            reusable_unicode_buffer: UnicodeBuffer::new(),
            cache_size: settings.cache_size,
            cached_params: VecDeque::with_capacity(settings.cache_size),
            cached_results: HashMap::with_capacity(settings.cache_size),
        }
    }

    pub fn get_or_shape(&mut self, params: ShapeParams) -> Rc<ShapedText> {
        if let Some(result) = self.cached_results.get(&params) {
            return result.clone();
        }
        if self.cached_params.len() == self.cache_size {
            let params = self.cached_params.pop_front().unwrap();
            self.cached_results.remove(&params);
        }
        let result = Rc::new(self.shape(params.clone()));
        self.cached_params.push_back(params.clone());
        self.cached_results.insert(params, result.clone());
        result
    }

    fn shape(&mut self, params: ShapeParams) -> ShapedText {
        let mut glyphs = Vec::new();
        if params.fonts.is_empty() {
            println!("WARNING: encountered empty font family");
        } else {
            self.shape_recursive(
                &params.text,
                &params.fonts,
                &params.features,
                params.direction,
                0,
                params.text.len(),
                &mut glyphs,
            );
        }

        // Post-process: apply letter-spacing and word-spacing
        let letter_spacing = params.letter_spacing.0;
        let word_spacing = params.word_spacing.0;
        if letter_spacing != 0.0 || word_spacing != 0.0 {
            let text = params.text.as_bytes();
            for glyph in glyphs.iter_mut() {
                glyph.advance_in_ems += letter_spacing;
                if glyph.cluster < text.len() && text[glyph.cluster] == b' ' {
                    glyph.advance_in_ems += word_spacing;
                }
            }
        }

        ShapedText {
            text: params.text,
            width_in_ems: glyphs.iter().map(|glyph| glyph.advance_in_ems).sum(),
            glyphs,
        }
    }

    fn shape_recursive(
        &mut self,
        text: &str,
        fonts: &[Rc<Font>],
        features: &[(u32, u32)],
        direction: Direction,
        start: usize,
        end: usize,
        out_glyphs: &mut Vec<ShapedGlyph>,
    ) {
        let (font, fonts) = fonts.split_first().unwrap();
        let mut glyphs = self.reusable_glyphs.pop().unwrap_or(Vec::new());
        self.shape_step(text, font, features, direction, start, end, &mut glyphs);
        let mut glyph_groups = glyphs
            .group_by(|glyph_0, glyph_1| glyph_0.cluster == glyph_1.cluster)
            .peekable();
        while let Some(glyph_group) = glyph_groups.next() {
            if glyph_group.iter().any(|glyph| glyph.id == 0) && !fonts.is_empty() {
                let missing_start = glyph_group[0].cluster;
                while glyph_groups.peek().map_or(false, |glyph_group| {
                    glyph_group.iter().any(|glyph| glyph.id == 0)
                }) {
                    glyph_groups.next();
                }
                let missing_end = glyph_groups
                    .peek()
                    .map_or(end, |next_glyph_group| next_glyph_group[0].cluster);
                self.shape_recursive(text, fonts, features, direction, missing_start, missing_end, out_glyphs);
            } else {
                out_glyphs.extend(glyph_group.iter().cloned());
            }
        }
        drop(glyph_groups);
        glyphs.clear();
        self.reusable_glyphs.push(glyphs);
    }

    fn shape_step(
        &mut self,
        text: &str,
        font: &Rc<Font>,
        features: &[(u32, u32)],
        direction: Direction,
        start: usize,
        end: usize,
        out_glyphs: &mut Vec<ShapedGlyph>,
    ) {
        let mut unicode_buffer = mem::take(&mut self.reusable_unicode_buffer);
        match direction {
            Direction::Ltr => unicode_buffer.set_direction(rustybuzz::Direction::LeftToRight),
            Direction::Rtl => unicode_buffer.set_direction(rustybuzz::Direction::RightToLeft),
        }
        for (index, grapheme) in text[start..end].grapheme_indices(true) {
            let cluster = start + index;
            for char in grapheme.chars() {
                unicode_buffer.add(char, cluster as u32);
            }
        }
        let rb_features: Vec<rustybuzz::Feature> = features
            .iter()
            .map(|&(tag, value)| rustybuzz::Feature::new(
                rustybuzz::ttf_parser::Tag::from_bytes(&tag.to_be_bytes()),
                value,
                ..,
            ))
            .collect();
        let glyph_buffer = rustybuzz::shape(font.rustybuzz_face(), &rb_features, unicode_buffer);
        out_glyphs.extend(
            glyph_buffer
                .glyph_infos()
                .iter()
                .zip(glyph_buffer.glyph_positions())
                .map(|(glyph_info, glyph_position)| ShapedGlyph {
                    font: font.clone(),
                    id: glyph_info.glyph_id as u16,
                    cluster: glyph_info.cluster as usize,
                    advance_in_ems: glyph_position.x_advance as f32 / font.units_per_em(),
                    offset_in_ems: glyph_position.x_offset as f32 / font.units_per_em(),
                    y_offset_in_ems: glyph_position.y_offset as f32 / font.units_per_em(),
                }),
        );

        self.reusable_unicode_buffer = glyph_buffer.clear();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub cache_size: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ShapeParams {
    pub text: Substr,
    pub fonts: Rc<[Rc<Font>]>,
    pub direction: Direction,
    pub letter_spacing: Ems,
    pub word_spacing: Ems,
    /// OpenType feature tag/value pairs for shaping.
    pub features: Rc<Vec<(u32, u32)>>,
}

#[derive(Clone, Debug)]
pub struct ShapedText {
    pub text: Substr,
    pub width_in_ems: f32,
    pub glyphs: Vec<ShapedGlyph>,
}

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub font: Rc<Font>,
    pub id: GlyphId,
    pub cluster: usize,
    pub advance_in_ems: f32,
    pub offset_in_ems: f32,
    pub y_offset_in_ems: f32,
}

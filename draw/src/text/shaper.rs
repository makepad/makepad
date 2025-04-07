use {
    super::{
        font::{Font, GlyphId},
        substr::Substr,
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::UnicodeBuffer,
    std::{
        collections::{HashMap, VecDeque},
        hash::Hash,
        rc::Rc,
    },
};

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
        self.shape_recursive(
            &params.text,
            &params.fonts,
            0,
            params.text.len(),
            &mut glyphs,
        );
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
        start: usize,
        end: usize,
        out_glyphs: &mut Vec<ShapedGlyph>,
    ) {
        use super::slice::SliceExt;

        let (font, fonts) = fonts.split_first().unwrap();
        let mut glyphs = self.reusable_glyphs.pop().unwrap_or(Vec::new());
        self.shape_step(text, font, start, end, &mut glyphs);
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
                self.shape_recursive(text, fonts, missing_start, missing_end, out_glyphs);
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
        start: usize,
        end: usize,
        out_glyphs: &mut Vec<ShapedGlyph>,
    ) {
        use {std::mem, unicode_segmentation::UnicodeSegmentation};

        let mut unicode_buffer = mem::take(&mut self.reusable_unicode_buffer);
        for (index, grapheme) in text[start..end].grapheme_indices(true) {
            let cluster = start + index;
            for char in grapheme.chars() {
                unicode_buffer.add(char, cluster as u32);
            }
        }
        let glyph_buffer = rustybuzz::shape(font.rustybuzz_face(), &[], unicode_buffer);
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
}

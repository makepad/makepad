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
    reusable_shaped_glyphs: Vec<Vec<ShapedGlyph>>,
    reusable_unicode_buffer: UnicodeBuffer,
    cache_size: usize,
    cached_params: VecDeque<ShapeParams>,
    cached_shaped_texts: HashMap<ShapeParams, Rc<ShapedText>>,
}

impl Shaper {
    pub fn new(settings: Settings) -> Self {
        Self {
            reusable_shaped_glyphs: Vec::new(),
            reusable_unicode_buffer: UnicodeBuffer::new(),
            cache_size: settings.cache_size,
            cached_params: VecDeque::with_capacity(settings.cache_size),
            cached_shaped_texts: HashMap::with_capacity(settings.cache_size),
        }
    }

    pub fn get_or_shape(&mut self, params: ShapeParams) -> Rc<ShapedText> {
        if !self.cached_shaped_texts.contains_key(&params) {
            if self.cached_params.len() == self.cache_size {
                let params = self.cached_params.pop_front().unwrap();
                self.cached_shaped_texts.remove(&params);
            }
            let text: ShapedText = self.shape(params.clone());
            self.cached_params.push_back(params.clone());
            self.cached_shaped_texts
                .insert(params.clone(), Rc::new(text));
        }
        self.cached_shaped_texts.get(&params).unwrap().clone()
    }

    fn shape(&mut self, params: ShapeParams) -> ShapedText {
        let mut text = ShapedText::default();
        self.shape_recursive(&params.text, &params.fonts, 0, params.text.len(), &mut text);
        text
    }

    fn shape_recursive(
        &mut self,
        text: &str,
        fonts: &[Rc<Font>],
        start: usize,
        end: usize,
        output: &mut ShapedText,
    ) {
        fn group_glyphs_by_cluster(
            glyphs: &[ShapedGlyph],
        ) -> impl Iterator<Item = &[ShapedGlyph]> + '_ {
            use std::iter;

            let mut start = 0;
            iter::from_fn(move || {
                if start == glyphs.len() {
                    return None;
                }
                let end = glyphs[start..]
                    .windows(2)
                    .position(|window| window[0].cluster != window[1].cluster)
                    .map_or(glyphs.len(), |index| start + index + 1);
                let group = &glyphs[start..end];
                start = end;
                Some(group)
            })
        }

        let (font, fonts) = fonts.split_first().unwrap();
        let mut glyphs = self.reusable_shaped_glyphs.pop().unwrap_or(Vec::new());
        self.shape_step(text, font, start, end, &mut glyphs);
        let mut glyph_groups = group_glyphs_by_cluster(&glyphs).peekable();
        while let Some(glyph_group) = glyph_groups.next() {
            if glyph_group.iter().any(|glyph| glyph.id == 0) && !fonts.is_empty() {
                let missing_start = glyph_group[0].cluster;
                while glyph_groups.peek().map_or(false, |glyph_group| {
                    glyph_group.iter().any(|glyph| glyph.id == 0)
                }) {
                    glyph_groups.next();
                }
                let missing_end = if let Some(glyph_group) = glyph_groups.peek() {
                    glyph_group[0].cluster
                } else {
                    end
                };
                self.shape_recursive(text, fonts, missing_start, missing_end, output);
            } else {
                for glyph in glyph_group.iter().cloned() {
                    output.push_glyph(glyph);
                }
            }
        }
        drop(glyph_groups);
        glyphs.clear();
        self.reusable_shaped_glyphs.push(glyphs);
    }

    fn shape_step(
        &mut self,
        text: &str,
        font: &Rc<Font>,
        start: usize,
        end: usize,
        output: &mut Vec<ShapedGlyph>,
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
        output.extend(
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

#[derive(Clone, Debug, Default)]
pub struct ShapedText {
    pub glyphs: Vec<ShapedGlyph>,
}

impl ShapedText {
    pub fn width_in_ems(&self) -> f32 {
        self.glyphs.iter().map(|glyph| glyph.advance_in_ems).sum()
    }

    pub fn push_glyph(&mut self, glyph: ShapedGlyph) {
        self.glyphs.push(glyph);
    }
}

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub font: Rc<Font>,
    pub id: GlyphId,
    pub cluster: usize,
    pub advance_in_ems: f32,
    pub offset_in_ems: f32,
}

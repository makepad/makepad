use {
    super::{
        font::{AllocatedGlyph, Font, GlyphId},
        substr::Substr,
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::UnicodeBuffer,
    std::{
        collections::{HashMap, VecDeque},
        fmt,
        hash::Hash,
        rc::Rc,
    },
};

const CACHE_SIZE: usize = 1024;

#[derive(Debug)]
pub struct Shaper {
    reusable_shaped_glyphs: Vec<Vec<ShapedGlyph>>,
    reusable_unicode_buffer: UnicodeBuffer,
    cache_size: usize,
    cached_shape_params: VecDeque<ShapeParams>,
    cached_shaped_glyphs: HashMap<ShapeParams, Rc<Vec<ShapedGlyph>>>,
}

impl Shaper {
    pub fn new() -> Self {
        Self {
            reusable_shaped_glyphs: Vec::new(),
            reusable_unicode_buffer: UnicodeBuffer::new(),
            cache_size: CACHE_SIZE,
            cached_shape_params: VecDeque::with_capacity(CACHE_SIZE),
            cached_shaped_glyphs: HashMap::with_capacity(CACHE_SIZE),
        }
    }

    pub fn get_or_shape(&mut self, params: &ShapeParams) -> Rc<Vec<ShapedGlyph>> {
        if !self.cached_shaped_glyphs.contains_key(params) {
            if self.cached_shape_params.len() == self.cache_size {
                let params = self.cached_shape_params.pop_front().unwrap();
                self.cached_shaped_glyphs.remove(&params);
            }
            let glyphs = self.shape(params);
            self.cached_shape_params.push_back(params.clone());
            self.cached_shaped_glyphs
                .insert(params.clone(), Rc::new(glyphs));
        }
        self.cached_shaped_glyphs.get(params).unwrap().clone()
    }

    fn shape(&mut self, params: &ShapeParams) -> Vec<ShapedGlyph> {
        let mut glyphs = Vec::new();
        self.shape_recursive(
            &params.text,
            &params.fonts,
            0,
            params.text.len(),
            &mut glyphs,
        );
        glyphs
    }

    fn shape_recursive(
        &mut self,
        text: &str,
        fonts: &[Rc<Font>],
        start: usize,
        end: usize,
        output: &mut Vec<ShapedGlyph>,
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
                output.extend(glyph_group.iter().cloned());
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ShapeParams {
    pub text: Substr,
    pub fonts: Rc<[Rc<Font>]>,
}

#[derive(Clone)]
pub struct ShapedGlyph {
    pub font: Rc<Font>,
    pub id: GlyphId,
    pub cluster: usize,
    pub advance_in_ems: f32,
    pub offset_in_ems: f32,
}

impl ShapedGlyph {
    pub fn height_in_ems(&self) -> f32 {
        self.font.ascender_in_ems() - self.font.descender_in_ems()
    }
}

impl fmt::Debug for ShapedGlyph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShapedGlyph")
            .field("font", &self.font.id())
            .field("id", &self.id)
            .field("cluster", &self.cluster)
            .field("advance_in_ems", &self.advance_in_ems)
            .field("offset_in_ems", &self.offset_in_ems)
            .finish()
    }
}

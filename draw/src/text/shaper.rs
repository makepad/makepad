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
    reusable_glyphs: Vec<Vec<Glyph>>,
    reusable_unicode_buffer: UnicodeBuffer,
    cache_size: usize,
    cached_params: VecDeque<ShapeParams>,
    cached_results: HashMap<ShapeParams, Rc<ShapeResult>>,
}

impl Shaper {
    pub fn new() -> Self {
        Self {
            reusable_glyphs: Vec::new(),
            reusable_unicode_buffer: UnicodeBuffer::new(),
            cache_size: CACHE_SIZE,
            cached_params: VecDeque::with_capacity(CACHE_SIZE),
            cached_results: HashMap::with_capacity(CACHE_SIZE),
        }
    }

    pub fn get_or_shape(&mut self, params: &ShapeParams) -> Rc<ShapeResult> {
        if !self.cached_results.contains_key(params) {
            if self.cached_params.len() == self.cache_size {
                let inputs = self.cached_params.pop_front().unwrap();
                self.cached_results.remove(&inputs);
            }
            let result = self.shape(params);
            self.cached_params.push_back(params.clone());
            self.cached_results
                .insert(params.clone(), Rc::new(result));
        }
        self.cached_results.get(params).unwrap().clone()
    }

    fn shape(&mut self, input: &ShapeParams) -> ShapeResult {
        let mut glyphs = Vec::new();
        self.shape_recursive(&input.text, &input.fonts, 0, input.text.len(), &mut glyphs);
        ShapeResult { glyphs }
    }

    fn shape_recursive(
        &mut self,
        text: &str,
        fonts: &[Rc<Font>],
        start: usize,
        end: usize,
        output: &mut Vec<Glyph>,
    ) {
        fn group_glyphs_by_cluster(glyphs: &[Glyph]) -> impl Iterator<Item = &[Glyph]> + '_ {
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
        let mut glyphs = self.reusable_glyphs.pop().unwrap_or(Vec::new());
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
        self.reusable_glyphs.push(glyphs);
    }

    fn shape_step(
        &mut self,
        text: &str,
        font: &Rc<Font>,
        start: usize,
        end: usize,
        output: &mut Vec<Glyph>,
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
                .map(|(glyph_info, glyph_position)| Glyph {
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

#[derive(Clone, Debug)]
pub struct ShapeResult {
    pub glyphs: Vec<Glyph>,
}

impl ShapeResult {
    pub fn width_in_ems(&self) -> f32 {
        self.glyphs.iter().map(|glyph| glyph.advance_in_ems).sum()
    }
}

#[derive(Clone)]
pub struct Glyph {
    pub font: Rc<Font>,
    pub id: GlyphId,
    pub cluster: usize,
    pub advance_in_ems: f32,
    pub offset_in_ems: f32,
}

impl Glyph {
    pub fn allocate(&self, dpx_per_em: f32) -> Option<AllocatedGlyph> {
        self.font.allocate_glyph(self.id, dpx_per_em)
    }
}

impl fmt::Debug for Glyph {
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
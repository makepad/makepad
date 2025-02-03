use {
    super::font::{AllocatedGlyph, Font, GlyphId},
    makepad_rustybuzz as rustybuzz,
    rustybuzz::UnicodeBuffer,
    std::{borrow::Borrow, collections::{HashMap, VecDeque}, hash::{Hash, Hasher}, rc::Rc},
};

#[derive(Debug)]
pub struct ShaperWithCache {
    shaper: Shaper,
    cache_size: usize,
    cache_keys: VecDeque<OwnedCacheKey>,
    cache: HashMap<OwnedCacheKey, Rc<Vec<Glyph>>>,
}

impl ShaperWithCache {
    pub fn new(cache_size: usize) -> Self {
        Self {
            shaper: Shaper::new(),
            cache_size,
            cache_keys: VecDeque::with_capacity(cache_size),
            cache: HashMap::with_capacity(cache_size),
        }
    }

    pub fn shape(
        &mut self,
        text: &str,
        fonts: &[Rc<Font>],
    ) -> Rc<Vec<Glyph>> {
        let key = BorrowedCacheKey(text, fonts);
        if !self.cache.contains_key(&key as &dyn CacheKey) {
            if self.cache_keys.len() == self.cache_size {
                let key = self.cache_keys.pop_front().unwrap();
                self.cache.remove(&key);
            }
            let key = key.to_owned();
            let glyphs = Rc::new(self.shaper.shape(text, fonts));
            self.cache_keys.push_back(key.clone());
            self.cache.insert(key, glyphs);
        }
        self.cache.get(&key as &dyn CacheKey).unwrap().clone()
    }
}

#[derive(Debug)]
pub struct Shaper {
    reusable_glyphs_stack: Vec<Vec<Glyph>>,
    reusable_unicode_buffer: Option<UnicodeBuffer>,
}

impl Shaper {
    pub fn new() -> Self {
        Self {
            reusable_glyphs_stack: Vec::new(),
            reusable_unicode_buffer: Some(UnicodeBuffer::new()),
        }
    }

    pub fn shape(&mut self, text: &str, fonts: &[Rc<Font>]) -> Vec<Glyph> {
        let mut glyphs = Vec::new();
        self.shape_recursive(text, fonts, 0, text.len(), &mut glyphs);
        glyphs
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
        let mut glyphs = self.reusable_glyphs_stack.pop().unwrap_or(Vec::new());
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
        self.reusable_glyphs_stack.push(glyphs);
    }

    fn shape_step(
        &mut self,
        text: &str,
        font: &Rc<Font>,
        start: usize,
        end: usize,
        output: &mut Vec<Glyph>,
    ) {
        use unicode_segmentation::UnicodeSegmentation;

        let mut unicode_buffer = self.reusable_unicode_buffer.take().unwrap();
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
        self.reusable_unicode_buffer = Some(glyph_buffer.clear());
    }
}


#[derive(Clone, Debug)]
pub struct Glyph {
    pub font: Rc<Font>,
    pub id: GlyphId,
    pub cluster: usize,
    pub advance_in_ems: f32,
    pub offset_in_ems: f32,
}

impl Glyph {
    pub fn allocate(&self, font_size_in_pxs: f32) -> Option<AllocatedGlyph> {
        self.font.allocate_glyph(self.id, font_size_in_pxs)
    }
}

trait CacheKey {
    fn text(&self) -> &str;

    fn fonts(&self) -> &[Rc<Font>];
}

impl Eq for dyn CacheKey + '_ {}

impl Hash for dyn CacheKey + '_ {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.text().hash(hasher);
        self.fonts().hash(hasher);
    }
}

impl PartialEq for dyn CacheKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        if self.text() != other.text() {
            return false;
        }
        if self.fonts() != other.fonts() {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct OwnedCacheKey(Rc<str>, Rc<[Rc<Font>]>);

impl<'a> Borrow<dyn CacheKey + 'a> for OwnedCacheKey {
    fn borrow(&self) -> &(dyn CacheKey + 'a) {
        self
    }
}

impl CacheKey for OwnedCacheKey {
    fn text(&self) -> &str {
        &self.0
    }

    fn fonts(&self) -> &[Rc<Font>] {
        &self.1
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct BorrowedCacheKey<'a>(&'a str, &'a [Rc<Font>]);

impl<'a> BorrowedCacheKey<'a> {
    fn to_owned(&self) -> OwnedCacheKey {
        OwnedCacheKey(self.0.into(), self.1.into())
    }
}

impl CacheKey for BorrowedCacheKey<'_> {
    fn text(&self) -> &str {
        self.0
    }

    fn fonts(&self) -> &[Rc<Font>] {
        self.1
    }
}

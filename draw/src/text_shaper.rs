use {
    crate::font_loader::{FontId, FontLoader},
    makepad_rustybuzz::UnicodeBuffer,
    std::{
        borrow::Borrow,
        collections::{HashMap, VecDeque},
        hash::{Hash, Hasher},
        rc::Rc,
    },
    unicode_segmentation::UnicodeSegmentation,
};

const MAX_CACHE_SIZE: usize = 4096;

#[derive(Debug)]
pub struct TextShaper {
    cache_keys: VecDeque<OwnedCacheKey>,
    cache: HashMap<OwnedCacheKey, Vec<GlyphInfo>>,
    buffers: Vec<UnicodeBuffer>,
}

impl TextShaper {
    pub fn new() -> Self {
        Self {
            cache_keys: VecDeque::new(),
            cache: HashMap::new(),
            buffers: Vec::new(),
        }
    }

    pub fn get_or_shape<'a>(
        &'a mut self,
        font_loader: &mut FontLoader,
        is_secret: bool,
        text: &str,
        font_ids: &[FontId],
    ) -> &'a [GlyphInfo] {
        let cache_key = BorrowedCacheKey {
            is_secret,
            text,
            font_ids,
        };
        if !self.cache.contains_key(&cache_key as &dyn CacheKey) {
            let cache_key = cache_key.to_owned();
            let glyph_infos = self.shape(font_loader, is_secret, text, font_ids);
            if self.cache_keys.len() == MAX_CACHE_SIZE {
                let cache_key = self.cache_keys.pop_front().unwrap();
                self.cache.remove(&cache_key);
            }
            self.cache_keys.push_back(cache_key.clone());
            self.cache.insert(cache_key, glyph_infos);
        }
        &self.cache[&cache_key as &dyn CacheKey]
    }

    fn shape(
        &mut self,
        font_loader: &mut FontLoader,
        is_secret: bool,
        text: &str,
        font_ids: &[FontId],
    ) -> Vec<GlyphInfo> {
        if is_secret {
            self.shape_secret(font_loader, text, font_ids)
        } else {
            self.shape_no_secret(font_loader, text, font_ids)
        }
    }

    fn shape_secret(
        &mut self,
        font_loader: &mut FontLoader,
        text: &str,
        font_ids: &[FontId],
    ) -> Vec<GlyphInfo> {
        let Some((font_id, glyph_id)) = font_ids.iter().copied().find_map(|font_id| {
            let font = font_loader[font_id].as_mut().unwrap();
            let glyph_id = font.glyph_id('•').0 as usize;
            if glyph_id == 0 {
                None
            } else {
                Some((font_id, glyph_id))
            }
        }) else {
            return Vec::new();
        };
        text.grapheme_indices(true)
            .map(|(index, _)| GlyphInfo {
                font_id,
                glyph_id,
                cluster: index,
            })
            .collect()
    }

    fn shape_no_secret(
        &mut self,
        font_loader: &mut FontLoader,
        text: &str,
        font_ids: &[FontId],
    ) -> Vec<GlyphInfo> {
        let mut glyph_infos = Vec::new();
        self.shape_no_secret_recursive(font_loader, text, font_ids, 0, &mut glyph_infos);
        glyph_infos
    }

    fn shape_no_secret_recursive(
        &mut self,
        font_loader: &mut FontLoader,
        text: &str,
        font_ids: &[FontId],
        base_cluster: usize,
        output: &mut Vec<GlyphInfo>,
    ) -> bool {
        let mut font_ids = font_ids;
        let (font_id, font) = loop {
            let Some((&font_id, remaining_font_ids)) = font_ids.split_first() else {
                return false;
            };
            font_ids = remaining_font_ids;
            if let Some(font) = &font_loader[font_id] {
                break (font_id, font);
            };
        };

        let mut buffer = self.buffers.pop().unwrap_or_else(|| UnicodeBuffer::new());
        buffer.push_str(text);
        let buffer = font
            .owned_font_face
            .with_ref(|face| makepad_rustybuzz::shape(face, &[], buffer));
        let glyph_infos = buffer.glyph_infos();
        
        let mut start_glyph = 0;
        while start_glyph < glyph_infos.len() {
            let start_glyph_is_missing = glyph_infos[start_glyph].glyph_id == 0;
            let mut end_glyph = start_glyph;
            while end_glyph < glyph_infos.len() {
                let end_glyph_is_missing = glyph_infos[end_glyph].glyph_id == 0;
                if start_glyph_is_missing != end_glyph_is_missing {
                    break;
                }
                end_glyph += 1;
            }

            let start_cluster = glyph_infos[start_glyph].cluster.try_into().unwrap();
            let end_cluster: usize = if end_glyph < glyph_infos.len() {
                glyph_infos[end_glyph].cluster.try_into().unwrap()
            } else {
                text.len()
            };

            debug_assert!(
                start_cluster <= end_cluster,
                "HarfBuzz guarantees monotonic cluster values"
            );

            if start_glyph_is_missing && self.shape_no_secret_recursive(
                font_loader,
                &text[start_cluster..end_cluster],
                font_ids,
                base_cluster + start_cluster,
                output,
            ) {
                start_glyph = end_glyph;
            }
            while start_glyph < end_glyph {
                let start_cluster: usize = glyph_infos[start_glyph].cluster.try_into().unwrap();
                output.push(GlyphInfo {
                    font_id,
                    glyph_id: glyph_infos[start_glyph].glyph_id.try_into().unwrap(),
                    cluster: base_cluster + start_cluster,
                });
                start_glyph += 1;
            }
        }

        self.buffers.push(buffer.clear());
        true
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GlyphInfo {
    pub font_id: FontId,
    pub glyph_id: usize,
    pub cluster: usize,
}

trait CacheKey {
    fn is_secret(&self) -> bool;
    fn text(&self) -> &str;
    fn font_ids(&self) -> &[usize];
}

impl Eq for dyn CacheKey + '_ {}

impl Hash for dyn CacheKey + '_ {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.is_secret().hash(hasher);
        self.text().hash(hasher);
        self.font_ids().hash(hasher);
    }
}

impl PartialEq for dyn CacheKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        if !self.is_secret() == other.is_secret() {
            return false;
        }
        if self.text() != other.text() {
            return false;
        }
        if self.font_ids() != other.font_ids() {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct OwnedCacheKey {
    is_secret: bool,
    text: Rc<str>,
    font_ids: Rc<[FontId]>,
}

impl<'a> Borrow<dyn CacheKey + 'a> for OwnedCacheKey {
    fn borrow(&self) -> &(dyn CacheKey + 'a) {
        self
    }
}

impl CacheKey for OwnedCacheKey {
    fn is_secret(&self) -> bool {
        self.is_secret
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn font_ids(&self) -> &[usize] {
        &self.font_ids
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct BorrowedCacheKey<'a> {
    is_secret: bool,
    text: &'a str,
    font_ids: &'a [FontId],
}

impl<'a> BorrowedCacheKey<'a> {
    fn to_owned(&self) -> OwnedCacheKey {
        OwnedCacheKey {
            is_secret: self.is_secret,
            text: self.text.into(),
            font_ids: self.font_ids.into(),
        }
    }
}

impl CacheKey for BorrowedCacheKey<'_> {
    fn is_secret(&self) -> bool {
        self.is_secret
    }

    fn text(&self) -> &str {
        self.text
    }

    fn font_ids(&self) -> &[usize] {
        self.font_ids
    }
}

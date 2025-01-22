use {
    crate::font_loader::{FontLoader, FontId},
    makepad_rustybuzz::UnicodeBuffer,
    unicode_segmentation::UnicodeSegmentation,
    std::{
        borrow::Borrow,
        collections::{HashMap, VecDeque},
        hash::{Hash, Hasher},
        rc::Rc,
    },
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
        text.grapheme_indices(true).map(|(index, _)| {
            GlyphInfo {
                font_id,
                glyph_id,
                cluster: index,
            }
        }).collect()
    }

    fn shape_no_secret(
        &mut self,
        font_loader: &mut FontLoader,
        text: &str,
        font_ids: &[FontId],
    ) -> Vec<GlyphInfo> {
        let mut glyph_infos = Vec::new();
        self.shape_no_secret_recursive(
            font_loader,
            text,
            font_ids,
            0,
            &mut glyph_infos,
        );
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
        // Get the preferred font to be used currently.
        let Some((&font_id, remaining_font_ids)) = font_ids.split_first() else {
            return false;
        };

        // Verify if the font is available, and if not, try the fallback font.
        let Some(font) = &font_loader[font_id] else {
            return self.shape_no_secret_recursive(
                font_loader,
                text,
                remaining_font_ids,
                base_cluster,
                output,
            );
        };

        // Create and configure the HarfBuzz buffer.
        let mut buffer = self.buffers.pop().unwrap_or_else(|| UnicodeBuffer::new());
        buffer.push_str(text);

        // Shape the text using HarfBuzz.
        let buffer = font.owned_font_face.with_ref(|face| {
            makepad_rustybuzz::shape(face, &[], buffer)
        });

        let infos = buffer.glyph_infos();

        // Track the processed text position to avoid reprocessing characters
        // that have already been handled through font fallback
        let mut skip_to = 0;

        let mut info_iter = infos.iter();
        while let Some(info) = info_iter.next() {
            // If this position has already been processed, skip it.
            if (info.cluster as usize) < skip_to {
                continue;
            }
            // Calculate the absolute cluster position.
            let absolute_cluster = base_cluster + info.cluster as usize;

            // Handle valid glyphs.
            if info.glyph_id != 0 {
                output.push(GlyphInfo {
                    font_id,
                    glyph_id: info.glyph_id as usize,
                    cluster: absolute_cluster,
                });
                continue;
            }

            // Handle missing glyphs.
            let start = info.cluster as usize;

            // Find the position of the next valid glyph.
            let next_cluster = {
                let mut preview_iter = info_iter.clone();
                let next_valid = preview_iter
                    .find(|next| next.glyph_id != 0)
                    .map(|next| next.cluster as usize)
                    .unwrap_or(text.len());
                next_valid
            };

            // Allow cluster values to remain the same or increase.
            debug_assert!(
                start <= next_cluster,
                "HarfBuzz guarantees monotonic cluster values"
            );

            // Recursively call,
            // trying to process the current character with the fallback font.
            if self.shape_no_secret_recursive(
                font_loader,
                &text[start..next_cluster],
                remaining_font_ids,
                base_cluster + start,
                output,
            ) {
                skip_to = next_cluster;
                continue;
            }

            output.push(GlyphInfo {
                font_id,
                glyph_id: info.glyph_id as usize,
                cluster: absolute_cluster,
            });
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

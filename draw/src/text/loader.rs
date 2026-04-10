use {
    super::{
        font::{Font, FontId},
        font_face::FontFace,
        font_family::{FontFamily, FontFamilyId},
        rasterizer,
        rasterizer::Rasterizer,
        shaper,
        shaper::Shaper,
    },
    crate::makepad_platform::SharedBytes,
    fxhash::FxHashMap,
    std::{cell::RefCell, rc::Rc},
};

pub type FontData = SharedBytes;

#[derive(Clone, Debug)]
pub struct Loader {
    shaper: Rc<RefCell<Shaper>>,
    rasterizer: Rc<RefCell<rasterizer::Rasterizer>>,
    pub(crate) font_family_definitions: FxHashMap<FontFamilyId, FontFamilyDefinition>,
    font_definitions: FxHashMap<FontId, FontDefinition>,
    font_family_cache: FxHashMap<FontFamilyId, Rc<FontFamily>>,
    font_cache: FxHashMap<FontId, Rc<Font>>,
}

impl Loader {
    pub fn new(settings: Settings) -> Self {
        let loader = Self {
            shaper: Rc::new(RefCell::new(Shaper::new(settings.shaper))),
            rasterizer: Rc::new(RefCell::new(Rasterizer::new(settings.rasterizer))),
            font_family_definitions: FxHashMap::default(),
            font_definitions: FxHashMap::default(),
            font_family_cache: FxHashMap::default(),
            font_cache: FxHashMap::default(),
        };
        //builtins::define(&mut loader);
        loader
    }

    pub fn rasterizer(&self) -> &Rc<RefCell<Rasterizer>> {
        &self.rasterizer
    }

    pub fn is_font_family_known(&self, id: FontFamilyId) -> bool {
        self.font_family_definitions.contains_key(&id) || self.font_family_cache.contains_key(&id)
    }

    pub fn is_font_known(&self, id: FontId) -> bool {
        if self.font_definitions.contains_key(&id) {
            return true;
        }
        if self.font_cache.contains_key(&id) {
            return true;
        }
        false
    }

    pub fn define_font_family(&mut self, id: FontFamilyId, definition: FontFamilyDefinition) {
        debug_assert!(
            !self.is_font_family_known(id),
            "can't redefine a font family that is already known"
        );
        self.font_family_definitions.insert(id, definition);
    }

    pub fn set_font_family_definition(
        &mut self,
        id: FontFamilyId,
        definition: FontFamilyDefinition,
    ) {
        // Skip cache eviction if the definition is unchanged.
        if let Some(existing) = self.font_family_definitions.get(&id) {
            if *existing == definition {
                return;
            }
        }
        if let Some(cached) = self.font_family_cache.get(&id) {
            let cached_ids: Vec<FontId> = cached.fonts().iter().map(|f| f.id()).collect();
            if cached_ids == definition.font_ids
                && definition.expected_member_count == definition.font_ids.len()
            {
                return;
            }
        }
        self.font_family_cache.remove(&id);
        self.font_family_definitions.insert(id, definition);
    }

    pub fn define_font(&mut self, id: FontId, definition: FontDefinition) {
        debug_assert!(
            !self.is_font_known(id),
            "can't redefine a font that is already known"
        );
        self.font_definitions.insert(id, definition);
    }

    pub fn get_or_load_font_family(&mut self, id: FontFamilyId) -> &Rc<FontFamily> {
        if !self.font_family_cache.contains_key(&id) {
            let font_family = self.load_font_family(id);
            self.font_family_cache.insert(id, Rc::new(font_family));
        }
        self.font_family_cache.get(&id).unwrap()
    }

    pub fn get_or_load_font_family_rc(&mut self, id: FontFamilyId) -> Rc<FontFamily> {
        self.get_or_load_font_family(id).clone()
    }

    fn load_font_family(&mut self, id: FontFamilyId) -> FontFamily {
        let definition = self
            .font_family_definitions
            .get(&id)
            .cloned()
            .unwrap_or_else(|| panic!("font family {:?} is not defined", id));
        FontFamily::new(
            id,
            self.shaper.clone(),
            definition
                .font_ids
                .into_iter()
                .map(|font_id| self.get_or_load_font(font_id).clone())
                .collect(),
        )
    }

    pub fn get_or_load_font(&mut self, id: FontId) -> &Rc<Font> {
        if !self.font_cache.contains_key(&id) {
            let font = self.load_font(id);
            self.font_cache.insert(id, Rc::new(font));
        }
        self.font_cache.get(&id).unwrap()
    }

    fn load_font(&mut self, id: FontId) -> Font {
        let definition = self
            .font_definitions
            .remove(&id)
            .expect("font is not defined");
        let mut face = FontFace::from_data_and_index(definition.data, definition.index)
            .expect("failed to load font from definition");
        let mut variations = definition.variations;
        if let Some(weight) = definition.weight {
            const FONT_WEIGHT_AXIS_TAG: u32 = u32::from_be_bytes(*b"wght");
            if let Some(existing) = variations
                .iter_mut()
                .find(|(tag, _)| *tag == FONT_WEIGHT_AXIS_TAG)
            {
                existing.1 = weight;
            } else {
                variations.push((FONT_WEIGHT_AXIS_TAG, weight));
            }
        }
        if !variations.is_empty() {
            face.set_variations(&variations);
        }
        Font::new(
            id,
            self.rasterizer.clone(),
            face,
            definition.ascender_fudge_in_ems,
            definition.descender_fudge_in_ems,
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub shaper: shaper::Settings,
    pub rasterizer: rasterizer::Settings,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontFamilyDefinition {
    pub font_ids: Vec<FontId>,
    pub expected_member_count: usize,
}

#[derive(Clone, Debug)]
pub struct FontDefinition {
    pub data: FontData,
    pub index: u32,
    pub ascender_fudge_in_ems: f32,
    pub descender_fudge_in_ems: f32,
    /// Convenience mapping for the OpenType `wght` axis. `None` keeps the font default.
    pub weight: Option<f32>,
    /// Font variation axis settings as (tag_u32, value) pairs.
    pub variations: Vec<(u32, f32)>,
}

#[cfg(test)]
mod tests {
    use super::{FontDefinition, Loader};
    use crate::{
        makepad_platform::SharedBytes,
        text::{font::FontId, layouter},
    };
    use std::path::PathBuf;

    fn bundled_font_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../widgets/resources/IBMPlexSans-Text.ttf")
    }

    #[test]
    fn get_or_load_font_reuses_cached_instance() {
        let mut loader = Loader::new(layouter::Settings::default().loader);
        let font_id: FontId = 0xCAFE_BABE_u64.into();
        let font_data = SharedBytes::from_file_mmap_or_read(bundled_font_path())
            .expect("font bytes should load");

        loader.define_font(
            font_id,
            FontDefinition {
                data: font_data,
                index: 0,
                ascender_fudge_in_ems: -0.1,
                descender_fudge_in_ems: 0.0,
                weight: None,
                variations: Vec::new(),
            },
        );

        let first = loader.get_or_load_font(font_id).clone();
        let second = loader.get_or_load_font(font_id).clone();
        assert!(std::rc::Rc::ptr_eq(&first, &second));
    }

    fn bundled_variable_font_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../widgets/resources/jetbrains_mono_variable.ttf")
    }

    fn outline_signature(
        font: &crate::text::font::Font,
        glyph_id: crate::text::font::GlyphId,
    ) -> u64 {
        use crate::text::glyph_outline::Command;
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let outline = font
            .glyph_outline(glyph_id)
            .expect("glyph outline should exist");
        let mut hasher = DefaultHasher::new();
        for command in outline.commands() {
            match command {
                Command::MoveTo(p) => {
                    0u8.hash(&mut hasher);
                    p.x.to_bits().hash(&mut hasher);
                    p.y.to_bits().hash(&mut hasher);
                }
                Command::LineTo(p) => {
                    1u8.hash(&mut hasher);
                    p.x.to_bits().hash(&mut hasher);
                    p.y.to_bits().hash(&mut hasher);
                }
                Command::QuadTo(c, p) => {
                    2u8.hash(&mut hasher);
                    c.x.to_bits().hash(&mut hasher);
                    c.y.to_bits().hash(&mut hasher);
                    p.x.to_bits().hash(&mut hasher);
                    p.y.to_bits().hash(&mut hasher);
                }
                Command::CurveTo(c1, c2, p) => {
                    3u8.hash(&mut hasher);
                    c1.x.to_bits().hash(&mut hasher);
                    c1.y.to_bits().hash(&mut hasher);
                    c2.x.to_bits().hash(&mut hasher);
                    c2.y.to_bits().hash(&mut hasher);
                    p.x.to_bits().hash(&mut hasher);
                    p.y.to_bits().hash(&mut hasher);
                }
                Command::Close => {
                    4u8.hash(&mut hasher);
                }
            }
        }
        hasher.finish()
    }

    #[test]
    fn variable_font_weight_changes_outline_shape() {
        let mut loader = Loader::new(layouter::Settings::default().loader);
        let font_data = SharedBytes::from_file_mmap_or_read(bundled_variable_font_path())
            .expect("variable font bytes should load");
        let regular_id: FontId = 0xD00D_0001_u64.into();
        let bold_id: FontId = 0xD00D_0002_u64.into();

        loader.define_font(
            regular_id,
            FontDefinition {
                data: font_data.clone(),
                index: 0,
                ascender_fudge_in_ems: 0.0,
                descender_fudge_in_ems: 0.0,
                weight: Some(200.0),
                variations: Vec::new(),
            },
        );
        loader.define_font(
            bold_id,
            FontDefinition {
                data: font_data,
                index: 0,
                ascender_fudge_in_ems: 0.0,
                descender_fudge_in_ems: 0.0,
                weight: Some(800.0),
                variations: Vec::new(),
            },
        );

        let regular = loader.get_or_load_font(regular_id).clone();
        let bold = loader.get_or_load_font(bold_id).clone();
        let glyph_id = regular.with_ttf_parser_face(|face| {
            face.glyph_index('B')
                .expect("test glyph should exist in variable font")
                .0
        });

        assert_ne!(
            outline_signature(regular.as_ref(), glyph_id),
            outline_signature(bold.as_ref(), glyph_id),
            "different weight masters should produce different outlines"
        );
    }
}

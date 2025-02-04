use {
    super::{
        atlas::Atlas,
        faces::Faces,
        font::{Font, FontId},
        font_family::{FontFamily, FontFamilyId},
        geometry::Size,
        pixels::{Bgra, R},
        shaper::ShaperWithCache,
    },
    std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc},
};

const IBM_PLEX_SANS_TEXT: &[u8] = include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
const LXG_WEN_KAI_REGULAR: &[u8] =
    include_bytes!("../../../widgets/resources/LXGWWenKaiRegular.ttf");
const NOTO_COLOR_EMOJI: &[u8] = include_bytes!("../../../widgets/resources/NotoColorEmoji.ttf");

#[derive(Clone, Debug)]
pub struct Loader {
    definitions: Definitions,
    shaper: Rc<RefCell<ShaperWithCache>>,
    grayscale_atlas: Rc<RefCell<Atlas<R<u8>>>>,
    color_atlas: Rc<RefCell<Atlas<Bgra<u8>>>>,
    font_family_cache: HashMap<FontFamilyId, Rc<FontFamily>>,
    font_cache: HashMap<FontId, Rc<Font>>,
}

impl Loader {
    pub fn new(
        options: Options,
        definitions: Definitions,
    ) -> Self {
        Self {
            shaper: Rc::new(RefCell::new(ShaperWithCache::new(options.shaper_cache_size))),
            grayscale_atlas: Rc::new(RefCell::new(Atlas::new(options.grayscale_atlas_size))),
            color_atlas: Rc::new(RefCell::new(Atlas::new(options.color_atlas_size))),
            definitions,
            font_family_cache: HashMap::new(),
            font_cache: HashMap::new(),
        }
    }

    pub fn grayscale_atlas(&self) -> &Rc<RefCell<Atlas<R<u8>>>> {
        &self.grayscale_atlas
    }

    pub fn color_atlas(&self) -> &Rc<RefCell<Atlas<Bgra<u8>>>> {
        &self.color_atlas
    }

    pub fn font_family(&mut self, font_family_id: &FontFamilyId) -> &Rc<FontFamily> {
        if !self.font_family_cache.contains_key(font_family_id) {
            let definition = self
                .definitions
                .font_families
                .remove(font_family_id)
                .unwrap_or_else(|| panic!("font family {:?} is not defined", font_family_id));
            let font_family = Rc::new(FontFamily::new(
                font_family_id.clone(),
                self.shaper.clone(),
                definition
                    .font_ids
                    .into_iter()
                    .map(|font_id| self.font(&font_id).clone())
                    .collect(),
            ));
            self.font_family_cache
                .insert(font_family_id.clone(), font_family);
        }
        self.font_family_cache.get(font_family_id).unwrap()
    }

    pub fn font(&mut self, font_id: &FontId) -> &Rc<Font> {
        if !self.font_cache.contains_key(font_id) {
            let definition = self
                .definitions
                .fonts
                .remove(font_id)
                .unwrap_or_else(|| panic!("font {:?} is not defined", font_id));
            let font = Rc::new(Font::new(
                font_id.clone(),
                self.grayscale_atlas.clone(),
                self.color_atlas.clone(),
                Faces::from_data_and_index(definition.data, definition.index).unwrap(),
            ));
            self.font_cache.insert(font_id.clone(), font);
        }
        self.font_cache.get(font_id).unwrap()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub shaper_cache_size: usize,
    pub grayscale_atlas_size: Size<usize>,
    pub color_atlas_size: Size<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            shaper_cache_size: 512,
            grayscale_atlas_size: Size::new(512, 512),
            color_atlas_size: Size::new(512, 512),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Definitions {
    pub font_families: HashMap<FontFamilyId, FontFamilyDefinition>,
    pub fonts: HashMap<FontId, FontDefinition>,
}

impl Default for Definitions {
    fn default() -> Self {
        Self {
            font_families: [(
                FontFamilyId::Sans,
                FontFamilyDefinition {
                    font_ids: [
                        "IBM Plex Sans Text".into(),
                        "LXG WWen Kai Regular".into(),
                        "Noto Color Emoji".into(),
                    ]
                    .into(),
                },
            )]
            .into_iter()
            .collect(),
            fonts: [
                (
                    "IBM Plex Sans Text".into(),
                    FontDefinition {
                        data: Cow::Borrowed(IBM_PLEX_SANS_TEXT).into(),
                        index: 0,
                    },
                ),
                (
                    "LXG WWen Kai Regular".into(),
                    FontDefinition {
                        data: Cow::Borrowed(LXG_WEN_KAI_REGULAR).into(),
                        index: 0,
                    },
                ),
                (
                    "Noto Color Emoji".into(),
                    FontDefinition {
                        data: Cow::Borrowed(NOTO_COLOR_EMOJI).into(),
                        index: 0,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FontFamilyDefinition {
    pub font_ids: Vec<FontId>,
}

#[derive(Clone, Debug)]
pub struct FontDefinition {
    pub data: Rc<Cow<'static, [u8]>>,
    pub index: u32,
}

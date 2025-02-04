use {
    super::{
        atlas::{Atlas, ColorAtlas, GrayscaleAtlas},
        faces::Faces,
        font::{Font, FontId},
        font_data,
        font_family::{FontFamily, FontFamilyId},
        geom::Size,
        pixels::{Bgra, R},
        shaper::Shaper,
    },
    std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc},
};

#[derive(Clone, Debug)]
pub struct Loader {
    definitions: Definitions,
    shaper: Rc<RefCell<Shaper>>,
    grayscale_atlas: Rc<RefCell<GrayscaleAtlas>>,
    color_atlas: Rc<RefCell<ColorAtlas>>,
    font_family_cache: HashMap<FontFamilyId, Rc<FontFamily>>,
    font_cache: HashMap<FontId, Rc<Font>>,
}

const GRAYSCALE_ATLAS_SIZE: Size<usize> = Size::new(256, 256);
const COLOR_ATLAS_SIZE: Size<usize> = Size::new(256, 256);

impl Loader {
    pub fn new(definitions: Definitions) -> Self {
        Self {
            shaper: Rc::new(RefCell::new(Shaper::new())),
            grayscale_atlas: Rc::new(RefCell::new(Atlas::new(GRAYSCALE_ATLAS_SIZE))),
            color_atlas: Rc::new(RefCell::new(Atlas::new(COLOR_ATLAS_SIZE))),
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

    pub fn get_or_load_font_family(&mut self, font_family_id: &FontFamilyId) -> &Rc<FontFamily> {
        if !self.font_family_cache.contains_key(font_family_id) {
            let font_family = self.load_font_family(font_family_id.clone());
            self.font_family_cache
                .insert(font_family_id.clone(), Rc::new(font_family));
        }
        self.font_family_cache.get(font_family_id).unwrap()
    }

    fn load_font_family(&mut self, font_family_id: FontFamilyId) -> FontFamily {
        let definition = self
            .definitions
            .font_families
            .remove(&font_family_id)
            .unwrap_or_else(|| panic!("font family {} is not defined", font_family_id));
        FontFamily::new(
            font_family_id,
            self.shaper.clone(),
            definition
                .font_ids
                .into_iter()
                .map(|font_id| self.get_or_load_font(&font_id).clone())
                .collect(),
        )
    }

    pub fn get_or_load_font(&mut self, font_id: &FontId) -> &Rc<Font> {
        if !self.font_cache.contains_key(font_id) {
            let font = self.load_font(font_id.clone());
            self.font_cache.insert(font_id.clone(), Rc::new(font));
        }
        self.font_cache.get(font_id).unwrap()
    }

    fn load_font(&mut self, font_id: FontId) -> Font {
        let definition = self
            .definitions
            .fonts
            .remove(&font_id)
            .unwrap_or_else(|| panic!("font {} is not defined", font_id));
        Font::new(
            font_id.clone(),
            self.grayscale_atlas.clone(),
            self.color_atlas.clone(),
            Faces::from_data_and_index(definition.data, definition.index)
                .unwrap_or_else(|| panic!("failed to load font {} from definition", font_id)),
        )
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
                "Sans".into(),
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
                        data: Cow::Borrowed(font_data::IBM_PLEX_SANS_TEXT).into(),
                        index: 0,
                    },
                ),
                (
                    "LXG WWen Kai Regular".into(),
                    FontDefinition {
                        data: Cow::Borrowed(font_data::LXG_WEN_KAI_REGULAR).into(),
                        index: 0,
                    },
                ),
                (
                    "Noto Color Emoji".into(),
                    FontDefinition {
                        data: Cow::Borrowed(font_data::NOTO_COLOR_EMOJI).into(),
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

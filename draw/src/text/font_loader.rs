use {
    super::{
        font::{Font, FontId},
        font_atlas::{ColorAtlas, FontAtlas, GrayscaleAtlas},
        font_data,
        font_face::FontFaceDefinition,
        font_family::{FontFamily, FontFamilyId},
        geom::Size,
        pixels::{Bgra, R},
        shaper::Shaper,
    },
    std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc},
};

#[derive(Clone, Debug)]
pub struct FontLoader {
    definitions: FontDefinitions,
    shaper: Rc<RefCell<Shaper>>,
    grayscale_atlas: Rc<RefCell<GrayscaleAtlas>>,
    color_atlas: Rc<RefCell<ColorAtlas>>,
    font_family_cache: HashMap<FontFamilyId, Rc<FontFamily>>,
    font_cache: HashMap<FontId, Rc<Font>>,
}

const GRAYSCALE_ATLAS_SIZE: Size<usize> = Size::new(512, 512);
const COLOR_ATLAS_SIZE: Size<usize> = Size::new(512, 512);

impl FontLoader {
    pub fn new(definitions: FontDefinitions) -> Self {
        Self {
            shaper: Rc::new(RefCell::new(Shaper::new())),
            grayscale_atlas: Rc::new(RefCell::new(FontAtlas::new(GRAYSCALE_ATLAS_SIZE))),
            color_atlas: Rc::new(RefCell::new(FontAtlas::new(COLOR_ATLAS_SIZE))),
            definitions,
            font_family_cache: HashMap::new(),
            font_cache: HashMap::new(),
        }
    }

    pub fn grayscale_atlas(&self) -> &Rc<RefCell<FontAtlas<R<u8>>>> {
        &self.grayscale_atlas
    }

    pub fn color_atlas(&self) -> &Rc<RefCell<FontAtlas<Bgra<u8>>>> {
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
        let font_ids = self
            .definitions
            .families
            .remove(&font_family_id)
            .unwrap_or_else(|| panic!("font family {} is not defined", font_family_id));
        FontFamily::new(
            font_family_id,
            self.shaper.clone(),
            font_ids
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
            .faces
            .remove(&font_id)
            .unwrap_or_else(|| panic!("font {} is not defined", font_id));
        Font::new(
            font_id.clone(),
            self.grayscale_atlas.clone(),
            self.color_atlas.clone(),
            definition,
        )
        .unwrap_or_else(|| panic!("failed to create font {} from definition", font_id))
    }
}

#[derive(Clone, Debug)]
pub struct FontDefinitions {
    pub families: HashMap<FontFamilyId, Vec<FontId>>,
    pub faces: HashMap<FontId, FontFaceDefinition>,
}

impl Default for FontDefinitions {
    fn default() -> Self {
        Self {
            families: [
                (
                    "Sans".into(),
                    [
                        "IBM Plex Sans Text".into(),
                        "LXG WWen Kai Regular".into(),
                        "Noto Color Emoji".into(),
                    ]
                    .into(),
                ),
                (
                    "Monospace".into(),
                    ["Liberation Mono Regular".into()].into(),
                ),
            ]
            .into_iter()
            .collect(),
            faces: [
                (
                    "IBM Plex Sans Text".into(),
                    FontFaceDefinition {
                        data: Cow::Borrowed(font_data::IBM_PLEX_SANS_TEXT).into(),
                        index: 0,
                    },
                ),
                (
                    "LXG WWen Kai Regular".into(),
                    FontFaceDefinition {
                        data: Cow::Borrowed(font_data::LXG_WEN_KAI_REGULAR).into(),
                        index: 0,
                    },
                ),
                (
                    "Noto Color Emoji".into(),
                    FontFaceDefinition {
                        data: Cow::Borrowed(font_data::NOTO_COLOR_EMOJI).into(),
                        index: 0,
                    },
                ),
                (
                    "Liberation Mono Regular".into(),
                    FontFaceDefinition {
                        data: Cow::Borrowed(font_data::LIBERATION_MONO_REGULAR).into(),
                        index: 0,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        }
    }
}

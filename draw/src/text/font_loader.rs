use {
    super::{
        font::{Font, FontId},
        font_atlas::{ColorAtlas, FontAtlas, GrayscaleAtlas},
        font_data,
        font_face::FontFace,
        font_family::{FontFamily, FontFamilyId},
        geom::Size,
        image::{Rgba, R},
        sdfer,
        sdfer::Sdfer,
        shape,
        shape::Shaper,
    },
    std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc},
};

#[derive(Clone, Debug)]
pub struct FontLoader {
    shaper: Rc<RefCell<Shaper>>,
    sdfer: Rc<RefCell<Sdfer>>,
    grayscale_atlas: Rc<RefCell<GrayscaleAtlas>>,
    color_atlas: Rc<RefCell<ColorAtlas>>,
    definitions: FontDefinitions,
    font_family_cache: HashMap<FontFamilyId, Rc<FontFamily>>,
    font_cache: HashMap<FontId, Rc<Font>>,
}

impl FontLoader {
    pub fn new(definitions: FontDefinitions, settings: Settings) -> Self {
        Self {
            shaper: Rc::new(RefCell::new(Shaper::new(settings.shaper))),
            sdfer: Rc::new(RefCell::new(Sdfer::new(settings.sdfer))),
            grayscale_atlas: Rc::new(RefCell::new(FontAtlas::new(settings.grayscale_atlas_size))),
            color_atlas: Rc::new(RefCell::new(FontAtlas::new(settings.grayscale_atlas_size))),
            definitions,
            font_family_cache: HashMap::new(),
            font_cache: HashMap::new(),
        }
    }

    pub fn sdfer(&self) -> &Rc<RefCell<Sdfer>> {
        &self.sdfer
    }

    pub fn grayscale_atlas(&self) -> &Rc<RefCell<FontAtlas<R>>> {
        &self.grayscale_atlas
    }

    pub fn color_atlas(&self) -> &Rc<RefCell<FontAtlas<Rgba>>> {
        &self.color_atlas
    }

    pub fn get_or_load_font_family(&mut self, id: FontFamilyId) -> &Rc<FontFamily> {
        if !self.font_family_cache.contains_key(&id) {
            let font_family = self.load_font_family(id);
            self.font_family_cache.insert(id, Rc::new(font_family));
        }
        self.font_family_cache.get(&id).unwrap()
    }

    fn load_font_family(&mut self, id: FontFamilyId) -> FontFamily {
        let definition = self
            .definitions
            .families
            .remove(&id)
            .expect("font family is not defined");
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
            .definitions
            .fonts
            .remove(&id)
            .expect("font is not defined");
        Font::new(
            id.clone(),
            self.sdfer.clone(),
            self.grayscale_atlas.clone(),
            self.color_atlas.clone(),
            FontFace::from_data_and_index(definition.data, definition.index)
                .expect("failed to load font from definition"),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub shaper: shape::Settings,
    pub sdfer: sdfer::Settings,
    pub grayscale_atlas_size: Size<usize>,
    pub color_atlas_size: Size<usize>,
}

#[derive(Clone, Debug)]
pub struct FontDefinitions {
    pub families: HashMap<FontFamilyId, FontFamilyDefinition>,
    pub fonts: HashMap<FontId, FontDefinition>,
}

impl Default for FontDefinitions {
    fn default() -> Self {
        Self {
            families: [
                (
                    "Sans".into(),
                    FontFamilyDefinition {
                        font_ids: [
                            "IBM Plex Sans Text".into(),
                            "LXG WWen Kai Regular".into(),
                            "Noto Color Emoji".into(),
                        ]
                        .into(),
                    },
                ),
                (
                    "Monospace".into(),
                    FontFamilyDefinition {
                        font_ids: ["Liberation Mono Regular".into()].into(),
                    },
                ),
            ]
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
                (
                    "Liberation Mono Regular".into(),
                    FontDefinition {
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

#[derive(Clone, Debug)]
pub struct FontFamilyDefinition {
    pub font_ids: Vec<FontId>,
}

#[derive(Clone, Debug)]
pub struct FontDefinition {
    pub data: Rc<Cow<'static, [u8]>>,
    pub index: u32,
}

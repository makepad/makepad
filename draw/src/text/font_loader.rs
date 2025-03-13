use {
    super::{
        builtins,
        font::{Font, FontId},
        font_atlas::{ColorAtlas, FontAtlas, GrayscaleAtlas},
        font_face::FontFace,
        font_family::{FontFamily, FontFamilyId},
        geom::Size,
        image::{Rgba, R},
        sdfer,
        sdfer::Sdfer,
        shape,
        shape::Shaper,
    },
    std::{cell::RefCell, collections::HashMap, rc::Rc},
};

#[derive(Clone, Debug)]
pub struct FontLoader {
    shaper: Rc<RefCell<Shaper>>,
    sdfer: Rc<RefCell<Sdfer>>,
    grayscale_atlas: Rc<RefCell<GrayscaleAtlas>>,
    color_atlas: Rc<RefCell<ColorAtlas>>,
    font_family_definitions: HashMap<FontFamilyId, FontFamilyDefinition>,
    font_definitions: HashMap<FontId, FontDefinition>,
    font_family_cache: HashMap<FontFamilyId, Rc<FontFamily>>,
    font_cache: HashMap<FontId, Rc<Font>>,
}

impl FontLoader {
    pub fn new(settings: Settings) -> Self {
        let mut loader = Self {
            shaper: Rc::new(RefCell::new(Shaper::new(settings.shaper))),
            sdfer: Rc::new(RefCell::new(Sdfer::new(settings.sdfer))),
            grayscale_atlas: Rc::new(RefCell::new(FontAtlas::new(settings.grayscale_atlas_size))),
            color_atlas: Rc::new(RefCell::new(FontAtlas::new(settings.grayscale_atlas_size))),
            font_family_definitions: HashMap::new(),
            font_definitions: HashMap::new(),
            font_family_cache: HashMap::new(),
            font_cache: HashMap::new(),
        };
        builtins::define(&mut loader);
        loader
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

    pub fn is_font_family_known(&self, id: FontFamilyId) -> bool {
        if self.font_family_definitions.contains_key(&id) {
            return true;
        }
        if self.font_family_cache.contains_key(&id) {
            return true;
        }
        false
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

    fn load_font_family(&mut self, id: FontFamilyId) -> FontFamily {
        let definition = self
            .font_family_definitions
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
            .font_definitions
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
pub struct FontFamilyDefinition {
    pub font_ids: Vec<FontId>,
}

#[derive(Clone, Debug)]
pub struct FontDefinition {
    pub data: Rc<Vec<u8>>,
    pub index: u32,
}

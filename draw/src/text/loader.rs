use {
    super::{
        //builtins,
        font::{Font, FontId},
        font_face::FontFace,
        font_family::{FontFamily, FontFamilyId},
        rasterizer,
        rasterizer::Rasterizer,
        shaper,
        shaper::Shaper,
    },
    std::{cell::RefCell, collections::HashMap, rc::Rc},
};

#[derive(Clone, Debug)]
pub struct Loader {
    shaper: Rc<RefCell<Shaper>>,
    rasterizer: Rc<RefCell<rasterizer::Rasterizer>>,
    font_family_definitions: HashMap<FontFamilyId, FontFamilyDefinition>,
    font_definitions: HashMap<FontId, FontDefinition>,
    font_family_cache: HashMap<FontFamilyId, Rc<FontFamily>>,
    font_cache: HashMap<FontId, Rc<Font>>,
}

impl Loader {
    pub fn new(settings: Settings) -> Self {
        let loader = Self {
            shaper: Rc::new(RefCell::new(Shaper::new(settings.shaper))),
            rasterizer: Rc::new(RefCell::new(Rasterizer::new(settings.rasterizer))),
            font_family_definitions: HashMap::new(),
            font_definitions: HashMap::new(),
            font_family_cache: HashMap::new(),
            font_cache: HashMap::new(),
        };
        //builtins::define(&mut loader);
        loader
    }

    pub fn rasterizer(&self) -> &Rc<RefCell<Rasterizer>> {
        &self.rasterizer
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
            self.rasterizer.clone(),
            FontFace::from_data_and_index(definition.data, definition.index)
                .expect("failed to load font from definition"),
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

#[derive(Clone, Debug)]
pub struct FontFamilyDefinition {
    pub font_ids: Vec<FontId>,
}

#[derive(Clone, Debug)]
pub struct FontDefinition {
    pub data: Rc<Vec<u8>>,
    pub index: u32,
    pub ascender_fudge_in_ems: f32,
    pub descender_fudge_in_ems: f32,
}

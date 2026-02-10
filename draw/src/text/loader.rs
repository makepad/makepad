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
    std::{cell::RefCell, collections::HashMap, rc::Rc},
};

#[cfg(feature = "system-fonts")]
use makepad_platform::os::system_fonts::SystemFontProvider;

/// Source for font data - either embedded bytes or system font lookup.
#[derive(Clone, Debug)]
pub enum FontSource {
    Embedded { data: Rc<Vec<u8>>, index: u32 },
    #[cfg(feature = "system-fonts")]
    System { family: String },
}

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
        Self {
            shaper: Rc::new(RefCell::new(Shaper::new(settings.shaper))),
            rasterizer: Rc::new(RefCell::new(Rasterizer::new(settings.rasterizer))),
            font_family_definitions: HashMap::new(),
            font_definitions: HashMap::new(),
            font_family_cache: HashMap::new(),
            font_cache: HashMap::new(),
        }
    }

    pub fn rasterizer(&self) -> &Rc<RefCell<Rasterizer>> {
        &self.rasterizer
    }

    pub fn is_font_family_known(&self, id: FontFamilyId) -> bool {
        self.font_family_definitions.contains_key(&id) || self.font_family_cache.contains_key(&id)
    }

    pub fn is_font_known(&self, id: FontId) -> bool {
        self.font_definitions.contains_key(&id) || self.font_cache.contains_key(&id)
    }

    pub fn define_font_family(&mut self, id: FontFamilyId, definition: FontFamilyDefinition) {
        debug_assert!(!self.is_font_family_known(id), "can't redefine font family");
        self.font_family_definitions.insert(id, definition);
    }

    pub fn define_font(&mut self, id: FontId, definition: FontDefinition) {
        debug_assert!(!self.is_font_known(id), "can't redefine font");
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
        let definition = self.font_family_definitions.remove(&id)
            .unwrap_or_else(|| panic!("font family {:?} is not defined", id));
        FontFamily::new(
            id,
            self.shaper.clone(),
            definition.font_ids.into_iter()
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
        let definition = self.font_definitions.remove(&id).expect("font is not defined");

        let (data, index) = match definition.source {
            FontSource::Embedded { data, index } => (data, index),
            #[cfg(feature = "system-fonts")]
            FontSource::System { family } => {
                let provider = makepad_platform::os::get_system_font_provider();
                let font_data = provider.query_font(&family)
                    .unwrap_or_else(|e| panic!("Failed to load system font '{}': {}", family, e));
                (Rc::new(font_data.data), font_data.index)
            }
        };

        Font::new(
            id.clone(),
            self.rasterizer.clone(),
            FontFace::from_data_and_index(data, index).expect("failed to load font"),
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
    pub source: FontSource,
    pub ascender_fudge_in_ems: f32,
    pub descender_fudge_in_ems: f32,
}

impl FontDefinition {
    pub fn from_data(data: Rc<Vec<u8>>, index: u32) -> Self {
        Self {
            source: FontSource::Embedded { data, index },
            ascender_fudge_in_ems: 0.0,
            descender_fudge_in_ems: 0.0,
        }
    }

    pub fn from_data_with_fudge(data: Rc<Vec<u8>>, index: u32, ascender: f32, descender: f32) -> Self {
        Self {
            source: FontSource::Embedded { data, index },
            ascender_fudge_in_ems: ascender,
            descender_fudge_in_ems: descender,
        }
    }

    #[cfg(feature = "system-fonts")]
    pub fn from_system(family: impl Into<String>) -> Self {
        Self {
            source: FontSource::System { family: family.into() },
            ascender_fudge_in_ems: 0.0,
            descender_fudge_in_ems: 0.0,
        }
    }
}

use {
    super::{
        font::Font,
        intern::Intern,
        shaper::{Direction, Ems, ShapeParams, ShapedText, Shaper},
        substr::Substr,
    },
    std::{
        cell::RefCell,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FontDiagnostics {
    pub role: String,
    pub set: String,
    pub tried: Vec<String>,
}

impl Default for FontDiagnostics {
    fn default() -> Self {
        Self {
            role: "custom".to_string(),
            set: "unknown".to_string(),
            tried: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FontFamilyId(u64);

impl From<u64> for FontFamilyId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<&str> for FontFamilyId {
    fn from(value: &str) -> Self {
        Self(value.intern().as_ptr() as u64)
    }
}

#[derive(Debug)]
pub struct FontFamily {
    id: FontFamilyId,
    shaper: Rc<RefCell<Shaper>>,
    fonts: Rc<[Rc<Font>]>,
    diagnostics: Rc<FontDiagnostics>,
}

impl FontFamily {
    pub fn new(
        id: FontFamilyId,
        shaper: Rc<RefCell<Shaper>>,
        fonts: Rc<[Rc<Font>]>,
        diagnostics: FontDiagnostics,
    ) -> Self {
        Self {
            id,
            shaper,
            fonts,
            diagnostics: Rc::new(diagnostics),
        }
    }

    pub fn id(&self) -> FontFamilyId {
        self.id
    }

    pub fn get_or_shape(&self, text: Substr) -> Rc<ShapedText> {
        self.shaper.borrow_mut().get_or_shape(ShapeParams {
            text,
            fonts: self.fonts.clone(),
            direction: Direction::default(),
            letter_spacing: Ems(0.0),
            word_spacing: Ems(0.0),
            features: Rc::new(Vec::new()),
            diagnostics: self.diagnostics.clone(),
        })
    }

    pub fn fonts(&self) -> &[Rc<Font>] {
        &self.fonts
    }

    pub fn diagnostics(&self) -> &FontDiagnostics {
        &self.diagnostics
    }
}

impl Eq for FontFamily {}

impl Hash for FontFamily {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PartialEq for FontFamily {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

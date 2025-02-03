use {
    super::{
        font::Font,
        shaper::{Glyph, ShaperWithCache},
    },
    std::{
        cell::RefCell,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FontFamilyId {
    Sans,
}

#[derive(Debug)]
pub struct FontFamily {
    id: FontFamilyId,
    shaper: Rc<RefCell<ShaperWithCache>>,
    fonts: Vec<Rc<Font>>,
}

impl FontFamily {
    pub fn new(
        id: FontFamilyId,
        shaper: Rc<RefCell<ShaperWithCache>>,
        fonts: Vec<Rc<Font>>,
    ) -> Self {
        Self { id, shaper, fonts }
    }

    pub fn id(&self) -> &FontFamilyId {
        &self.id
    }

    pub fn shape(&self, text: &str) -> Rc<Vec<Glyph>> {
        self.shaper.borrow_mut().shape(text, &self.fonts)
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

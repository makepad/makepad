use {
    super::{
        font::Font,
        shaper::{Glyph, Shaper},
    },
    std::{cell::RefCell, rc::Rc},
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FontFamilyId {
    Sans,
}

#[derive(Debug)]
pub struct FontFamily {
    id: FontFamilyId,
    shaper: Rc<RefCell<Shaper>>,
    fonts: Vec<Rc<Font>>,
}

impl FontFamily {
    pub fn new(id: FontFamilyId, shaper: Rc<RefCell<Shaper>>, fonts: Vec<Rc<Font>>) -> Self {
        Self { id, shaper, fonts }
    }

    pub fn id(&self) -> &FontFamilyId {
        &self.id
    }

    pub fn shape(&self, text: &str) -> Vec<Glyph> {
        let mut shapr = self.shaper.borrow_mut();
        shapr.shape(text, self.fonts())
    }

    pub fn fonts(&self) -> &[Rc<Font>] {
        &self.fonts
    }
}

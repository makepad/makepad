use {
    super::{
        font::Font,
        shaper::{ShapeParams, ShapedText, Shaper},
        substr::Substr,
    },
    std::{
        cell::RefCell,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

pub type FontFamilyId = Rc<str>;

#[derive(Debug)]
pub struct FontFamily {
    id: FontFamilyId,
    shaper: Rc<RefCell<Shaper>>,
    fonts: Rc<[Rc<Font>]>,
}

impl FontFamily {
    pub fn new(id: FontFamilyId, shaper: Rc<RefCell<Shaper>>, fonts: Rc<[Rc<Font>]>) -> Self {
        Self { id, shaper, fonts }
    }

    pub fn id(&self) -> &FontFamilyId {
        &self.id
    }

    pub fn get_or_shape(&self, text: Substr) -> Rc<ShapedText> {
        self.shaper.borrow_mut().get_or_shape(ShapeParams {
            text: text.into(),
            fonts: self.fonts.clone(),
        })
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

use {
    super::{
        font::Font,
        substr::Substr,
        shaper::{ShapeParams, ShapeResult, Shaper},
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

    pub fn get_or_shape_text(&self, text: Substr) -> Rc<ShapeResult> {
        self.shaper
            .borrow_mut()
            .get_or_shape(&ShapeParams {
                text: text.into(),
                fonts: self.fonts.clone(),
            })
    }

    pub fn compute_text_width_in_ems(&self, text: Substr) -> f32 {
        self.get_or_shape_text(text).width_in_ems()
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

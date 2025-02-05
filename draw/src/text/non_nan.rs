use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NonNanF32(f32);

impl NonNanF32 {
    pub fn new(value: f32) -> Option<Self> {
        if value.is_nan() {
            return None;
        }
        Some(Self(value))
    }

    pub fn into_inner(self) -> f32 {
        self.0
    }
}

impl Eq for NonNanF32 {}

impl Hash for NonNanF32 {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.0.to_bits().hash(hasher);
    }
}

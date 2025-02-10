#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct R<T> {
    pub r: T,
}

impl<T> R<T> {
    pub const fn new(r: T) -> Self {
        Self { r }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct Rgba<T> {
    pub r: T,
    pub g: T,
    pub b: T,
    pub a: T,
}

impl<T> Rgba<T> {
    pub fn new(r: T, g: T, b: T, a: T) -> Self {
        Self { r, g, b, a }
    }
}

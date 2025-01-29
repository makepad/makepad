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
pub struct Bgra<T> {
    pub b: T,
    pub g: T,
    pub r: T,
    pub a: T,
}

impl<T> Bgra<T> {
    pub fn new(b: T, g: T, r: T, a: T) -> Self {
        Self { b, g, r, a }
    }
}

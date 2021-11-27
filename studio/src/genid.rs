use makepad_microserde::{SerBin, DeBin};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SerBin, DeBin)]
pub struct GenId {
    pub index: usize,
    pub generation: usize,
}

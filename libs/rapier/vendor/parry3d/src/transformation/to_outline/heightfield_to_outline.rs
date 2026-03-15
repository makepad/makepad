use crate::math::Real;
use crate::shape::{HeightField, HeightFieldCellStatus};
use crate::math::Vector3;

impl HeightField {
    /// Outlines this heightfield’s shape using polylines.
    pub fn to_outline(&self) -> (Vec<Vector>, Vec<[u32; 2]>) {
        todo!()
    }
}

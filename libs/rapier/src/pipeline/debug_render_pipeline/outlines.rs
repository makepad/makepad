use crate::geometry::{Ball, Cuboid};
#[cfg(feature = "dim3")]
use crate::geometry::{Cone, Cylinder};
use crate::math::Vector;
use std::any::TypeId;
use std::collections::HashMap;

#[cfg(feature = "dim2")]
pub fn instances(nsubdivs: u32) -> HashMap<TypeId, Vec<Vector>> {
    let mut result = HashMap::new();
    result.insert(
        TypeId::of::<Cuboid>(),
        Cuboid::new(Vector::splat(0.5)).to_polyline(),
    );
    result.insert(TypeId::of::<Ball>(), Ball::new(0.5).to_polyline(nsubdivs));
    result
}

#[cfg(feature = "dim3")]
#[allow(clippy::type_complexity)]
pub fn instances(nsubdivs: u32) -> HashMap<TypeId, (Vec<Vector>, Vec<[u32; 2]>)> {
    let mut result = HashMap::new();
    result.insert(
        TypeId::of::<Cuboid>(),
        Cuboid::new(Vector::splat(0.5)).to_outline(),
    );
    result.insert(TypeId::of::<Ball>(), Ball::new(0.5).to_outline(nsubdivs));
    result.insert(
        TypeId::of::<Cone>(),
        Cone::new(0.5, 0.5).to_outline(nsubdivs),
    );
    result.insert(
        TypeId::of::<Cylinder>(),
        Cylinder::new(0.5, 0.5).to_outline(nsubdivs),
    );
    result
}

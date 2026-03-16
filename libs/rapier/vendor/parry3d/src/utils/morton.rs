//! Morton encoding of 3D vectors.

// From https://github.com/DGriffin91/obvhs/tree/main/src/ploc/morton.rs
// MIT/Apache 2 license.

//---------------------------------------------------
// --- 21 bit resolution per channel morton curve ---
//---------------------------------------------------

use crate::math::Vector;

#[inline]
fn split_by_3_u64(a: u32) -> u64 {
    let mut x = a as u64 & 0x1fffff; // we only look at the first 21 bits
    x = (x | x << 32) & 0x1f00000000ffff;
    x = (x | x << 16) & 0x1f0000ff0000ff;
    x = (x | x << 8) & 0x100f00f00f00f00f;
    x = (x | x << 4) & 0x10c30c30c30c30c3;
    x = (x | x << 2) & 0x1249249249249249;
    x
}

#[inline]
/// Encode x,y,z position into a u64 morton value.
/// Input should be 0..=2u32.pow(21) (or 1u32 << 21)
fn morton_encode_u64(x: u32, y: u32, z: u32) -> u64 {
    split_by_3_u64(x) | split_by_3_u64(y) << 1 | split_by_3_u64(z) << 2
}

#[inline]
/// Encode a 3D position into a u64 morton value.
/// Input should be 0.0..=1.0
pub fn morton_encode_u64_unorm(p: Vector) -> u64 {
    #[cfg(feature = "dim2")]
    #[cfg_attr(feature = "f64", expect(clippy::unnecessary_cast))]
    let p = glamx::DVec2::new(p.x as f64, p.y as f64) * (1 << 21) as f64;
    #[cfg(feature = "dim3")]
    #[cfg_attr(feature = "f64", expect(clippy::unnecessary_cast))]
    let p = glamx::DVec3::new(p.x as f64, p.y as f64, p.z as f64) * (1 << 21) as f64;

    #[cfg(feature = "dim2")]
    return morton_encode_u64(p.x as u32, p.y as u32, 0);
    #[cfg(feature = "dim3")]
    return morton_encode_u64(p.x as u32, p.y as u32, p.z as u32);
}

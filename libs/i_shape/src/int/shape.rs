use alloc::vec::Vec;
use i_float::int::point::IntPoint;

pub type IntContour<T> = Vec<IntPoint<T>>;
pub type IntShape<T> = Vec<IntContour<T>>;
pub type IntShapes<T> = Vec<IntShape<T>>;

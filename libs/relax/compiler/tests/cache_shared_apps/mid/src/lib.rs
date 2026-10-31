//! A middle crate whose source never changes in the matrix (app -> mid -> fw).
pub fn mid_area() -> u32 { let s = sc_fw::Shape::new(2, 3); sc_fw::area(&s) + 1 }
pub fn mid_shape_size() -> usize { std::mem::size_of::<sc_fw::Shape>() }

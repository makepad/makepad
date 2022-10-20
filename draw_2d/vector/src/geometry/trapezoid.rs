/// A trapezoid in 2-dimensional Euclidian space.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Trapezoid {
    pub xs: [f32; 2],
    pub ys: [f32; 4],
}

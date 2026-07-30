use alloc::vec::Vec;

pub type Path<P> = Vec<P>;
pub type Paths<P> = Vec<Path<P>>;
pub type Contour<P> = Vec<P>;
pub type Shape<P> = Vec<Contour<P>>;
pub type Shapes<P> = Vec<Shape<P>>;

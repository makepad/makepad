use crate::Point;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub vertices: [Point; 3],
}

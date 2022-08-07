use crate::{Point, Transformation, Vector};

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct LinearTransformation {
    pub x: Vector,
    pub y: Vector,
}

impl LinearTransformation {
    pub fn new(x: Vector, y: Vector) -> LinearTransformation {
        LinearTransformation { x, y }
    }

    pub fn identity() -> LinearTransformation {
        LinearTransformation::new(Vector::new(1.0, 0.0), Vector::new(0.0, 1.0))
    }

    pub fn scaling(v: Vector) -> LinearTransformation {
        LinearTransformation::new(Vector::new(v.x, 0.0), Vector::new(0.0, v.y))
    }

    pub fn uniform_scaling(k: f32) -> LinearTransformation {
        LinearTransformation::scaling(Vector::new(k, k))
    }

    pub fn scale(self, v: Vector) -> LinearTransformation {
        LinearTransformation::new(self.x * v.x, self.y * v.y)
    }

    pub fn uniform_scale(self, k: f32) -> LinearTransformation {
        LinearTransformation::new(self.x * k, self.y * k)
    }

    pub fn compose(self, other: LinearTransformation) -> LinearTransformation {
        LinearTransformation::new(
            self.transform_vector(other.x),
            self.transform_vector(other.y),
        )
    }
}

impl Transformation for LinearTransformation {
    fn transform_point(&self, p: Point) -> Point {
        (self.x * p.x + self.y * p.y).to_point()
    }

    fn transform_vector(&self, v: Vector) -> Vector {
        self.x * v.x + self.y * v.y
    }
}

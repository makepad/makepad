use crate::{LinearTransformation, Point, Transform, Transformation, Vector};

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct AffineTransformation {
    pub xy: LinearTransformation,
    pub z: Vector,
}

impl AffineTransformation {
    pub fn new(xy: LinearTransformation, z: Vector) -> AffineTransformation {
        AffineTransformation { xy, z }
    }

    pub fn identity() -> AffineTransformation {
        AffineTransformation::new(LinearTransformation::identity(), Vector::zero())
    }

    pub fn scaling(v: Vector) -> AffineTransformation {
        AffineTransformation::new(LinearTransformation::scaling(v), Vector::zero())
    }

    pub fn uniform_scaling(k: f32) -> AffineTransformation {
        AffineTransformation::new(LinearTransformation::uniform_scaling(k), Vector::zero())
    }

    pub fn translation(v: Vector) -> AffineTransformation {
        AffineTransformation::new(LinearTransformation::identity(), v)
    }

    pub fn scale(self, v: Vector) -> AffineTransformation {
        AffineTransformation::new(self.xy.scale(v), self.z.scale(v))
    }

    pub fn uniform_scale(self, k: f32) -> AffineTransformation {
        AffineTransformation::new(self.xy.uniform_scale(k), self.z * k)
    }

    pub fn translate(self, v: Vector) -> AffineTransformation {
        AffineTransformation::new(self.xy, self.z + v)
    }
}

impl Transformation for AffineTransformation {
    fn transform_point(&self, p: Point) -> Point {
        p.transform(&self.xy) + self.z
    }

    fn transform_vector(&self, v: Vector) -> Vector {
        v.transform(&self.xy)
    }
}

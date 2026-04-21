use crate::{
    bounding_volume::Aabb,
    math::{Pose, Vector, DIM},
    shape::Triangle,
};

impl Triangle {
    /// Computes the world-space [`Aabb`] of this triangle, transformed by `pos`.
    #[inline]
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        self.transformed(pos).local_aabb()
    }

    /// Computes the local-space [`Aabb`] of this triangle.
    #[inline]
    pub fn local_aabb(&self) -> Aabb {
        let a = self.a;
        let b = self.b;
        let c = self.c;

        let mut min = Vector::ZERO;
        let mut max = Vector::ZERO;

        for d in 0..DIM {
            min[d] = a[d].min(b[d]).min(c[d]);
            max[d] = a[d].max(b[d]).max(c[d]);
        }

        Aabb::new(min, max)
    }
}

#[cfg(test)]
#[cfg(feature = "dim3")]
mod test {
    use crate::{
        bounding_volume::details::support_map_aabb,
        math::{Pose, Real, Rotation, Vector},
        shape::Triangle,
    };
    use core::f64::consts::FRAC_PI_2;

    #[test]
    fn triangle_aabb_matches_support_map_aabb() {
        let t = Triangle::new(
            Vector::new(0.3, -0.1, 0.2),
            Vector::new(-0.7, 1.0, 0.0),
            Vector::new(-0.7, 1.5, 0.0),
        );

        let m = Pose::from_parts(
            Vector::new(-0.2, 5.0, 0.2),
            Rotation::from_euler(glamx::EulerRot::XYZ, 0.0, FRAC_PI_2 as Real, 0.0),
        );

        assert_eq!(t.aabb(&m), support_map_aabb(&m, &t));

        // TODO: also test local Aabb once support maps have a local Aabb
        // function too
    }
}

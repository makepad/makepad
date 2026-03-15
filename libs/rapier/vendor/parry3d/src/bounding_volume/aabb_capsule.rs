use crate::bounding_volume::Aabb;
use crate::math::{Pose, Vector};
use crate::shape::Capsule;

impl Capsule {
    /// The axis-aligned bounding box of this capsule.
    #[inline]
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        self.transform_by(pos).local_aabb()
    }

    /// The axis-aligned bounding box of this capsule.
    #[inline]
    pub fn local_aabb(&self) -> Aabb {
        let a = self.segment.a;
        let b = self.segment.b;
        let mins = a.min(b) - Vector::splat(self.radius);
        let maxs = a.max(b) + Vector::splat(self.radius);
        Aabb::new(mins, maxs)
    }
}

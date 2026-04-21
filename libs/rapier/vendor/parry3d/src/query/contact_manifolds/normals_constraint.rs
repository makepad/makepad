use crate::math::{Pose, Vector};

// NOTE: the 'static requirement is only needed for the following impl to work:
//       impl<'a> TypedCompositeShape for dyn CompositeShape
//       We can probably work something out if that becomes too restrictive in
//       the future.
/// Constraints of contact normals, generally for internal edges resolution.
///
/// This trait used for applying constraints on normal direction for contact manifolds calculation.
/// Non-convex shapes will generally simplify collision-detection as a collection of simpler
/// convex-based collision-detection problems. However, that partial convex formulation allows
/// some contact normals that are theoretically impossible (in a convex analysis sense). The normal
/// constraints aims to correct/remove invalid normals, avoiding some artifacts in physics
/// simulations. In particular, this addresses the well-known "internal edges" problem on triangle
/// meshes and heightfields.
pub trait NormalConstraints: 'static {
    /// Corrects in-place or discards the specified normal (assumed to be unit-sized) based on the
    /// constraints encoded by `Self`.
    ///
    /// If this method returns `false` then the contacts associated to that normal should be
    /// considered invalid and be ignored by the collision-detection pipeline.
    fn project_local_normal_mut(&self, normal: &mut Vector) -> bool;
    /// Corrects or discards the specified normal (assumed to be unit-sized) based on the constraints
    /// encoded by `Self`.
    ///
    /// If this method returns `None` then the contacts associated to that normal should be
    /// considered invalid and be ignored by the collision-detection pipeline.
    fn project_local_normal(&self, mut normal: Vector) -> Option<Vector> {
        self.project_local_normal_mut(&mut normal).then_some(normal)
    }

    // NOTE: the normal is assumed to be unit-sized.
    /// Applies normal correction to the unit vectors `normal1` and `normal2` based on the
    /// assumption that `normal1` is in the same coordinates space as `Self`.
    ///
    /// The `normal2` will be modified to be equal to `-normal1` expressed in the local coordinate
    /// space of the second shape.
    ///
    /// If this method returns `false` then the contacts associated to that normal should be
    /// considered invalid and be ignored by the collision-detection pipeline.
    fn project_local_normal1(
        &self,
        pos12: &Pose,
        normal1: &mut Vector,
        normal2: &mut Vector,
    ) -> bool {
        if !self.project_local_normal_mut(normal1) {
            return false;
        }

        *normal2 = pos12.rotation.inverse() * -*normal1;

        true
    }

    /// Applies normal correction to the unit vectors `normal1` and `normal2` based on the
    /// assumption that `normal2` is in the same coordinates space as `Self`.
    ///
    /// The `normal1` will be modified to be equal to `-normal2` expressed in the local coordinate
    /// space of the first shape.
    ///
    /// If this method returns `false` then the contacts associated to that normal should be
    /// considered invalid and be ignored by the collision-detection pipeline.
    fn project_local_normal2(
        &self,
        pos12: &Pose,
        normal1: &mut Vector,
        normal2: &mut Vector,
    ) -> bool {
        if !self.project_local_normal_mut(normal2) {
            return false;
        }

        *normal1 = pos12.rotation * (-*normal2);

        true
    }
}

impl NormalConstraints for () {
    fn project_local_normal_mut(&self, _: &mut Vector) -> bool {
        true
    }
}

/// A pair of [`NormalConstraints`].
pub trait NormalConstraintsPair {
    /// Applies the normal constraints to `normal1` and `normal2`.
    ///
    /// This trait is mostly used internally to combine two [`NormalConstraints`] conveniently.
    fn project_local_normals(
        &self,
        pos12: &Pose,
        normal1: &mut Vector,
        normal2: &mut Vector,
    ) -> bool;
}

// We generally use Option<&dyn NormalConstraints> instead of the naked
// trait-object in our codebase so providing this impl is convenient.
impl NormalConstraintsPair
    for (
        Option<&dyn NormalConstraints>,
        Option<&dyn NormalConstraints>,
    )
{
    fn project_local_normals(
        &self,
        pos12: &Pose,
        normal1: &mut Vector,
        normal2: &mut Vector,
    ) -> bool {
        if let Some(proj) = self.0 {
            if !proj.project_local_normal1(pos12, normal1, normal2) {
                return false;
            }
        }

        if let Some(proj) = self.1 {
            proj.project_local_normal2(pos12, normal1, normal2)
        } else {
            true
        }
    }
}

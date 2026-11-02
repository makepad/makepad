//! `SmFilletExecutive::UpdateTopologyChanges` (SmFilletExecutive.cpp4481–4535).
//! The executive lists and attribute users remain in their actual traversal order.

use super::p3::FilletExecutive;
use crate::geom::solution_array::SolutionObject;
use crate::intersect::point_class::ClassObject;
use crate::kernel::Kernel;
use crate::Result;

/// Actual attribute users and retained geometry/corner update operations.
pub trait TopologyChangesAccess<O> {
    /// An actual user in the attribute store's identity domain.
    type User: Copy;
    /// Read actual marker users, without filtering them or changing their order.
    /// # Errors
    /// Missing attribute or invalid store context.
    fn change_users(&self, attribute: O) -> Result<Vec<Self::User>>;
    /// Native `SM_CAST_PTR(SmFace, user)` in the actual body domain.
    /// # Errors
    /// Unknown identity or unavailable owning-body context.
    fn change_face(&self, user: Self::User) -> Result<Option<O>>;
    /// Actual geometries of this retained solver.
    /// # Errors
    /// Missing solver or actual geometry storage.
    fn change_geometries(&self, solver: O) -> Result<Vec<O>>;
    /// Update the actual geometry's split-face references.
    /// # Errors
    /// Actual classification or topology update errors.
    fn change_geometry(&mut self, k: &Kernel, geometry: O, old: O, new: O) -> Result<()>;
    /// Update the actual corner's split-face references.
    /// # Errors
    /// Actual classification or topology update errors.
    fn change_corner(&mut self, k: &Kernel, corner: O, old: O, new: O) -> Result<()>;
}

fn ignored(status: Result<()>) -> Result<()> {
    match status {
        Err(error) if error.is_unported() => Err(error),
        _ => Ok(()),
    }
}

/// The shared native loop, using the actual executive's retained lists.
/// Per-object statuses are ignored upstream; genuine missing capabilities propagate.
/// # Errors
/// Invalid actual users/objects or a genuine missing update capability.
pub fn update_topology_changes<O: Copy + PartialEq>(
    k: &Kernel,
    solvers: &[O],
    corners: &[O],
    access: &mut impl TopologyChangesAccess<O>,
    attribute: O,
    original: O,
) -> Result<()> {
    let users = access.change_users(attribute)?;
    if users.len() < 2 {
        return Ok(());
    }
    let mut faces = Vec::new();
    for user in users {
        if let Some(face) = access.change_face(user)? {
            if face != original {
                faces.push(face);
            }
        }
    }
    if faces.is_empty() {
        return Ok(());
    }
    for &solver in solvers {
        for geometry in access.change_geometries(solver)? {
            for &face in &faces {
                ignored(access.change_geometry(k, geometry, original, face))?;
            }
        }
    }
    for &corner in corners {
        for &face in &faces {
            ignored(access.change_corner(k, corner, original, face))?;
        }
    }
    Ok(())
}

impl<O: ClassObject + SolutionObject + Ord> FilletExecutive<O> {
    /// Reconcile actual retained fillet objects with faces copied from a marker.
    /// # Errors
    /// Actual lookup errors or a genuine missing split-face update capability.
    pub fn update_topology_changes_with(
        &self,
        k: &Kernel,
        access: &mut impl TopologyChangesAccess<O>,
        attribute: O,
        original: O,
    ) -> Result<()> {
        update_topology_changes(k, &self.solvers, &self.corners, access, attribute, original)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SolidError;

    #[derive(Default)]
    struct Changes {
        users: Vec<Option<usize>>,
        events: Vec<(bool, usize, usize)>,
        stop: Option<usize>,
    }
    impl TopologyChangesAccess<usize> for Changes {
        type User = Option<usize>;
        fn change_users(&self, _: usize) -> Result<Vec<Self::User>> {
            Ok(self.users.clone())
        }
        fn change_face(&self, user: Self::User) -> Result<Option<usize>> {
            Ok(user)
        }
        fn change_geometries(&self, solver: usize) -> Result<Vec<usize>> {
            Ok(vec![solver * 10, solver * 10 + 1])
        }
        fn change_geometry(&mut self, _: &Kernel, g: usize, _: usize, face: usize) -> Result<()> {
            self.events.push((false, g, face));
            if self.stop == Some(g) {
                return Err(SolidError::unported(
                    "actual update dependency",
                    "blend.fillet_geom_p5",
                ));
            }
            Err(SolidError::sm_err("ordinary ignored update"))
        }
        fn change_corner(&mut self, _: &Kernel, c: usize, _: usize, face: usize) -> Result<()> {
            self.events.push((true, c, face));
            Ok(())
        }
    }
    #[test]
    fn ordered_updates_ignore_ordinary_errors_then_visit_corners() {
        let mut changes = Changes {
            users: vec![Some(8), None, Some(3), Some(9)],
            ..Changes::default()
        };
        update_topology_changes(&Kernel::default(), &[2, 1], &[7, 6], &mut changes, 0, 3).unwrap();
        assert_eq!(
            changes.events,
            vec![
                (false, 20, 8),
                (false, 20, 9),
                (false, 21, 8),
                (false, 21, 9),
                (false, 10, 8),
                (false, 10, 9),
                (false, 11, 8),
                (false, 11, 9),
                (true, 7, 8),
                (true, 7, 9),
                (true, 6, 8),
                (true, 6, 9)
            ]
        );
    }
    #[test]
    fn genuine_missing_update_retains_prefix_and_stops_before_corners() {
        let mut changes = Changes {
            users: vec![Some(3), Some(8)],
            stop: Some(21),
            ..Changes::default()
        };
        assert!(
            update_topology_changes(&Kernel::default(), &[2, 1], &[7], &mut changes, 0, 3).is_err()
        );
        assert_eq!(changes.events, vec![(false, 20, 8), (false, 21, 8)]);
    }
    #[test]
    fn single_user_returns_before_any_object_projection() {
        struct Single;
        impl TopologyChangesAccess<usize> for Single {
            type User = usize;
            fn change_users(&self, _: usize) -> Result<Vec<usize>> {
                Ok(vec![0])
            }
            fn change_face(&self, _: usize) -> Result<Option<usize>> {
                panic!("native returns before cast")
            }
            fn change_geometries(&self, _: usize) -> Result<Vec<usize>> {
                panic!("native returns before solvers")
            }
            fn change_geometry(&mut self, _: &Kernel, _: usize, _: usize, _: usize) -> Result<()> {
                panic!("no update")
            }
            fn change_corner(&mut self, _: &Kernel, _: usize, _: usize, _: usize) -> Result<()> {
                panic!("no update")
            }
        }
        update_topology_changes(&Kernel::default(), &[1], &[2], &mut Single, 0, 3).unwrap();
    }
}

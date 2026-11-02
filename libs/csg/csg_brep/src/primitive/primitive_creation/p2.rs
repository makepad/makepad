//! Partial analytic primitives, tapered extrusions, curve and pipe sweeps,
//! planar-path sweeps, skins and face blends.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`,
//! `source/SMLib/inc/SmPrimitiveCreation.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_partial_cone`] | six-argument `CreateCone` |
//! | [`PrimitiveCreation::create_partial_sphere`] | four-argument `CreateSphere` |
//! | [`PrimitiveCreation::create_partial_torus`] | five-argument `CreateTorus` |
//! | [`PrimitiveCreation::create_taper_extrude`] | `CreateTaperExtrude` |
//! | [`PrimitiveCreation::create_curve_sweep_from_edges`] | `CreateCurveSweepFromEdges` |
//! | [`PrimitiveCreation::create_curve_sweep_from_faces`] | `CreateCurveSweepFromFaces` |
//! | [`PrimitiveCreation::create_pipe_sweep`] | `CreatePipeSweep` |
//! | [`PrimitiveCreation::create_sweep_along_planar_path`] | eight-argument static `CreateSweepAlongPlanarPath` |
//! | `sweep_profiles_along_path` (private) | `SweepProfilesAlongPath` |
//! | [`PrimitiveCreation::create_skin_primitive`] | nine-argument `CreateSkinPrimitive` |
//! | `skin_primitive_internal` (private) | `sm_CreateSkinPrimitiveInternal` |
//! | [`PrimitiveCreation::create_blend_primitive_between_faces`] | face/edge `CreateBlendPrimitive` |
//! | [`PrimitiveCreationAccess`] | B-rep services these entry points call |
//!
//! Deviations: typed ids and owned curve slots replace pointers; the caller's
//! [`PrimitiveCreationAccess`] resolves B-reps, contexts and services, and its
//! defaults return `Unported` for units that are not ported yet. Every caller
//! tail after such a call is ported. Upstream null dereferences (a missing edge
//! curve, a failed copy) become errors. Ignored statuses keep their outputs and
//! pass only `Unported` on. Debug drawing (`SM_DEBUG_CODE`) is not ported. No
//! recursion; loops are bounded by their input lists.

mod access;
mod blend;
mod curve_sweeps;
mod planar_path;
mod rotational;
mod skin;
mod taper;
mod taper_walls;

pub use super::p1::PrimitiveCreation;
pub use access::PrimitiveCreationAccess;
pub use taper_walls::{CornerRefusal, TaperCornerRefusal};

use crate::curve::curve::{Curve, CurveKind};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;

/// The B-spline an `SmBSplineCurve` cast reads: B-splines and the analytic kinds
/// built on one (`None` for the kinds upstream's cast would misread).
fn as_bspline(curve: &Curve) -> Option<BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(c) => Some(c.clone()),
        CurveKind::Line(c) => Some(c.base().clone()),
        CurveKind::Ellipse(c) => Some(c.base().clone()),
        CurveKind::Parabola(c) => Some(c.base().clone()),
        CurveKind::Hyperbola(c) => Some(c.base().clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! Entry checks of the p2 primitives on a B-rep with only its infinite region.
    //! Difftest compares these exits too; here a removed check cannot hide behind a
    //! deferred construction.
    use super::*;
    use crate::curve::composite_curve::{OffsetCornerType, OffsetMapAttribute};
    use crate::error::{Result, SolidError};
    use crate::geom::attribute::TagData;
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::geom::context::MarkType;
    use crate::geom::object::ObjectRef;
    use crate::kernel::Kernel;
    use crate::math::{Point3, Vec3};
    use crate::nurbs::b_spline_curve::p1::NurbCircleParam;
    use crate::primitive::primitive_creation::{p3::PrimitiveAccess, SweepOptions};
    use crate::sweep::topology_sweep::access::{EdgeRef, FaceRef, SweepAccess, SweepMergeId};
    use crate::sweep::topology_sweep::SweepEntity;
    use crate::topology::brep::p10::TrimType;
    use crate::topology::edge::NotifyEnv;
    use crate::topology::{Brep, BrepId, FaceId, Region, RegionId};

    struct Model {
        k: Kernel,
        b: Brep,
        /// More B-reps, as ids 1, 2, … after `b` (id 0).
        extra: Vec<Brep>,
        /// The ids released through `delete_brep`.
        deleted: Vec<BrepId>,
        /// Hand out the one B-rep as a scratch B-rep, so a taper reaches its normal.
        scratch: bool,
    }
    fn none<T>() -> Result<T> {
        Err(SolidError::invalid_input("p2 test model", "no topology"))
    }
    impl Model {
        fn new() -> Result<Self> {
            let mut b = Brep::new();
            let region = b.add_region_record(Region {
                in_brep: true,
                ..Default::default()
            })?;
            b.set_infinite_region(Some(region));
            Ok(Self {
                k: Kernel::default(),
                b,
                extra: Vec::new(),
                deleted: Vec::new(),
                scratch: false,
            })
        }
    }
    impl SweepAccess for Model {
        fn brep(&self, id: BrepId) -> Result<&Brep> {
            match id.index() {
                0 => Ok(&self.b),
                i => self.extra.get(i as usize - 1).map_or_else(none, Ok),
            }
        }
        fn brep_mut(&mut self, id: BrepId) -> Result<&mut Brep> {
            match id.index() {
                0 => Ok(&mut self.b),
                i => self.extra.get_mut(i as usize - 1).map_or_else(none, Ok),
            }
        }
        fn kernel(&self, _: BrepId) -> Result<&Kernel> {
            Ok(&self.k)
        }
        fn kernel_mut(&mut self, _: BrepId) -> Result<&mut Kernel> {
            Ok(&mut self.k)
        }
        fn with_brep_mut<T>(
            &mut self,
            _: BrepId,
            _: impl FnOnce(
                &mut Kernel,
                &mut Brep,
                &dyn Fn(FaceId) -> ObjectRef,
                &mut NotifyEnv<'_>,
            ) -> Result<T>,
        ) -> Result<T> {
            none()
        }
        fn with_breps_mut<T>(
            &mut self,
            _: BrepId,
            _: BrepId,
            _: impl FnOnce(
                &mut Kernel,
                &mut Brep,
                &mut Brep,
                &dyn Fn(crate::EntityRef) -> ObjectRef,
                &dyn Fn(FaceId) -> ObjectRef,
                Option<&mut crate::geom::context::Context>,
                &mut NotifyEnv<'_>,
            ) -> Result<T>,
        ) -> Result<T> {
            none()
        }
        fn with_brep_notifications<T>(
            &mut self,
            _: BrepId,
            _: impl FnOnce(&mut Brep, &mut NotifyEnv<'_>) -> Result<T>,
        ) -> Result<T> {
            none()
        }
        fn retain_hcr(&mut self, _: BrepId, _: crate::blend::hcr::Hcr) -> Result<BrepId> {
            none()
        }
        fn hcr_session(&self, _: BrepId) -> Result<&crate::blend::hcr::Hcr> {
            none()
        }
        fn new_brep(&mut self, _: BrepId) -> Result<BrepId> {
            if self.scratch {
                return Ok(B);
            }
            Err(SolidError::unported("SmBrep::SmBrep", "topology.brep_p3"))
        }
        fn delete_brep(&mut self, id: BrepId) {
            self.deleted.push(id);
        }
        fn new_merge(
            &mut self,
            _: BrepId,
            _: BrepId,
            _: BrepId,
            _: f64,
            _: f64,
        ) -> Result<SweepMergeId> {
            none()
        }
        fn merge_operands(&self, _: SweepMergeId) -> Result<(BrepId, BrepId)> {
            none()
        }
        fn merge_relations(
            &mut self,
            _: SweepMergeId,
        ) -> Result<&mut crate::boolean::topology_intersector::p1::Relations> {
            none()
        }
        fn copy_faces(
            &mut self,
            _: BrepId,
            _: &[FaceId],
            _: BrepId,
            _: SweepMergeId,
            _: &mut crate::sweep::topology_sweep::access::BTreeEdgeMap,
        ) -> Result<()> {
            none()
        }
        fn piecewise_merge(&mut self, _: SweepMergeId, _: bool, _: bool) -> Result<BrepId> {
            none()
        }
        fn union(&mut self, _: SweepMergeId) -> Result<BrepId> {
            none()
        }
        fn delete_merge(&mut self, _: SweepMergeId) {}
        fn tag(&self, _: SweepEntity) -> Result<Option<TagData>> {
            none()
        }
        fn add_tag(&mut self, _: SweepEntity, _: TagData) -> Result<()> {
            none()
        }
        fn curve_tag(&self, _: EdgeRef) -> Result<Option<TagData>> {
            none()
        }
        fn set_editing(&mut self, _: BrepId, editing: bool) -> Result<bool> {
            let previous = self.b.editing_enabled();
            self.b.set_editing_enabled(editing);
            Ok(previous)
        }
        fn lock_mark(&mut self, _: BrepId, _: i32) -> Result<MarkType> {
            none()
        }
        fn unlock_mark(&mut self, _: BrepId, _: MarkType) -> Result<()> {
            none()
        }
        fn is_marked(&self, _: SweepEntity, _: MarkType) -> Result<bool> {
            none()
        }
        fn mark(&mut self, _: SweepEntity, _: MarkType) -> Result<()> {
            none()
        }
        fn collect_faces(&mut self, _: FaceRef, _: MarkType, _: &mut Vec<FaceId>) -> Result<()> {
            none()
        }
    }
    impl PrimitiveAccess for Model {
        fn delete_primitive_faces(&mut self, _: BrepId, _: &[FaceId]) -> Result<()> {
            none()
        }
        fn with_primitive_brep<T>(
            &mut self,
            _: BrepId,
            body: impl FnOnce(&mut Kernel, &mut Brep) -> Result<T>,
        ) -> Result<T> {
            body(&mut self.k, &mut self.b)
        }
        fn axis_inside_face(&mut self, _: FaceRef, _: &Curve) -> Result<bool> {
            none()
        }
    }
    impl PrimitiveCreationAccess for Model {
        fn copy_faces_into(&mut self, _: BrepId, _: &[FaceId], _: BrepId) -> Result<()> {
            none()
        }
        fn attach_offset_map(&mut self, _: FaceRef, _: OffsetMapAttribute) -> Result<()> {
            none()
        }
        fn offset_map(&self, _: FaceRef) -> Result<Option<OffsetMapAttribute>> {
            none()
        }
        fn project_and_trim(
            &mut self,
            _: BrepId,
            _: &BSplineCurve,
            _: Vec3,
            _: Point3,
            _: TrimType,
            _: f64,
        ) -> Result<()> {
            none()
        }
    }

    const B: BrepId = BrepId::from_index(0);
    fn primitive() -> PrimitiveCreation {
        PrimitiveCreation::new(Some(RegionId::from_index(0)), 0, false)
    }
    fn code<T>(result: &Result<T>) -> Option<i32> {
        match result {
            Err(SolidError::Upstream { code, .. }) => Some(*code),
            Err(SolidError::InvalidInput { .. }) => Some(1007),
            _ => None,
        }
    }

    #[test]
    fn partial_cone_checks_height_and_turn_before_edit_mode() -> Result<()> {
        let frame = Axis2Placement::IDENTITY;
        let mut a = Model::new()?;
        let low = primitive().create_partial_cone(&mut a, B, 0.0, 1.0, 1.0, 0.0, 90.0, &frame);
        assert_eq!((code(&low), a.b.editing_enabled()), (Some(1001), false));
        let unturned =
            primitive().create_partial_cone(&mut a, B, 1.0, 1.0, 1.0, 30.0, 30.0, &frame);
        assert_eq!(
            (code(&unturned), a.b.editing_enabled()),
            (Some(1001), false)
        );
        // an axis shorter than the scaled zero fails after edit mode is on, as upstream
        let short =
            primitive().create_partial_cone(&mut a, B, 1.5e-12, 1.0, 1.0, 0.0, 90.0, &frame);
        assert_eq!((code(&short), a.b.editing_enabled()), (Some(1001), true));
        Ok(())
    }

    #[test]
    fn a_skin_of_one_profile_fails_with_cleared_outputs_and_report() -> Result<()> {
        let mut a = Model::new()?;
        let circle = BSplineCurve::create_circle_segment(
            3,
            &Axis2Placement::IDENTITY,
            5.0,
            0.0,
            360.0,
            NurbCircleParam::Quadratic,
        )?;
        let mut curves = vec![Some(Curve::from(circle))];
        let mut starts = vec![FaceId::from_index(9)];
        let mut sides = vec![FaceId::from_index(9)];
        let mut ends = Vec::new();
        let mut transferred = vec![true, true, true];
        let result = primitive().create_skin_primitive(
            &mut a,
            B,
            &[1],
            &mut curves,
            0.0,
            3,
            true,
            &mut starts,
            &mut sides,
            &mut ends,
            &mut transferred,
        );
        assert_eq!(code(&result), Some(1001));
        assert!(starts.is_empty() && sides.is_empty() && ends.is_empty());
        assert_eq!(transferred, vec![false]);
        assert!(curves.first().is_some_and(Option::is_some));
        Ok(())
    }

    #[test]
    fn a_curves_normal_that_cannot_be_computed_is_zero() -> Result<()> {
        // ComputeCrvsNormal zeroes its output before failing on one linear curve, so
        // a small normal given to be computed comes back zero
        let mut a = Model::new()?;
        a.scratch = true;
        let line = BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::X)?;
        let mut normal = Vec3::new(1e-3, 0.0, 0.0);
        let _ = primitive().create_taper_extrude(
            &mut a,
            B,
            vec![Curve::from(line)],
            1.0,
            5.0,
            3,
            &mut normal,
            OffsetCornerType::LinearExtension,
        );
        assert_eq!(normal, Vec3::ZERO);
        Ok(())
    }

    #[test]
    fn sweeps_of_nothing_and_a_planar_path_without_paths_fail() -> Result<()> {
        let mut a = Model::new()?;
        let path = BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::Z)?;
        let (mut s, mut d, mut e) = (Vec::new(), Vec::new(), Vec::new());
        let edges = primitive().create_curve_sweep_from_edges(
            &mut a,
            B,
            &[],
            &path,
            None,
            None,
            1e-5,
            SweepOptions::default(),
            &mut s,
            &mut d,
            &mut e,
        );
        assert_eq!(code(&edges), Some(1001));
        let faces = primitive().create_curve_sweep_from_faces(
            &mut a,
            B,
            &[],
            &path,
            None,
            None,
            1e-5,
            SweepOptions::default(),
            &mut s,
            &mut d,
            &mut e,
        );
        assert_eq!(code(&faces), Some(1001));
        let planar = PrimitiveCreation::create_sweep_along_planar_path(
            &mut a,
            B,
            &[],
            &[],
            1e-5,
            true,
            true,
            true,
        );
        assert_eq!(code(&planar), Some(1007));
        // without a target region the pipe still resets its lists first
        let mut orphan = PrimitiveCreation::new(None, 0, false);
        let mut stale = vec![FaceId::from_index(1)];
        let pipe = orphan.create_pipe_sweep(
            &mut a, B, 1.0, &path, 0.0, true, false, &mut stale, &mut d, &mut e,
        );
        assert_eq!(code(&pipe), Some(1006));
        assert!(stale.is_empty());
        Ok(())
    }

    /// The model's B-rep (id 0) and `more` others (ids 1, 2, …), each with an infinite
    /// region to tell it from an empty B-rep, and with its id plus one as its tolerance to
    /// tell them apart (a new B-rep's key is always 0).
    fn models(more: u32) -> Result<(Model, Vec<BrepId>)> {
        let mut a = Model::new()?;
        a.b.set_tolerance(1.0);
        let mut ids = vec![B];
        for i in 1..=more {
            let mut b = Brep::new();
            let region = b.add_region_record(Region {
                in_brep: true,
                ..Default::default()
            })?;
            b.set_infinite_region(Some(region));
            b.set_tolerance(f64::from(i) + 1.0);
            a.extra.push(b);
            ids.push(BrepId::from_index(i));
        }
        Ok((a, ids))
    }
    fn keys(a: &Model, ids: &[BrepId]) -> Result<Vec<f64>> {
        ids.iter()
            .map(|&id| a.brep(id).map(Brep::tolerance))
            .collect()
    }
    fn whole(a: &Model, id: BrepId) -> bool {
        a.brep(id).is_ok_and(|b| b.infinite_region().is_some())
    }

    #[test]
    fn the_tolerance_and_manifold_queries_call_the_brep_ports() -> Result<()> {
        let mut a = Model::new()?;
        a.set_brep_tolerance(B, 0.5)?;
        assert_eq!(a.b.tolerance(), 0.5);
        // zero takes the tolerance from the B-rep's box (query.brep, now ported)
        let zero = a.set_brep_tolerance(B, 0.0);
        assert!(zero.is_ok(), "{zero:?}");
        // an empty box: the default zone tolerance
        assert_eq!(a.b.tolerance(), 1.0e-5);
        // only the infinite region: not a solid
        assert!(!a.is_manifold_solid(B)?);
        Ok(())
    }

    #[test]
    fn boolean_trees_checks_its_list_before_moving_a_brep() -> Result<()> {
        let (mut a, ids) = models(1)?;
        let before = keys(&a, &ids)?;
        let mut empty = Vec::new();
        assert_eq!(code(&a.boolean_trees(&mut empty, &[0])), Some(1007));
        let mut twice = vec![ids[0], ids[0]];
        assert_eq!(code(&a.boolean_trees(&mut twice, &[0, 1, -1])), Some(1007));
        assert_eq!(twice, [ids[0], ids[0]]);
        let mut unknown = vec![ids[0], BrepId::from_index(7)];
        assert!(a.boolean_trees(&mut unknown, &[0]).is_err());
        // a leaf past the list fails in the merge before it takes a B-rep: all go back
        let mut both = ids.clone();
        assert_eq!(code(&a.boolean_trees(&mut both, &[5])), Some(1007));
        assert_eq!(both, ids);
        assert_eq!(keys(&a, &ids)?, before);
        assert!(whole(&a, ids[0]) && whole(&a, ids[1]));
        assert!(a.deleted.is_empty());
        Ok(())
    }

    #[test]
    fn boolean_trees_puts_the_breps_back_in_its_order() -> Result<()> {
        // a tree of one leaf combines nothing: its B-rep comes first, then the unused
        // ones in their order, as sOrderedBreps returns, and no slot is left over
        let (mut a, ids) = models(2)?;
        let before = keys(&a, &ids)?;
        let mut list = ids.clone();
        a.boolean_trees(&mut list, &[2])?;
        assert_eq!(list, ids);
        assert_eq!(keys(&a, &ids)?, [before[2], before[0], before[1]]);
        assert!(a.deleted.is_empty());
        Ok(())
    }

    #[test]
    fn a_union_leaves_its_result_first_and_releases_the_other_slot() -> Result<()> {
        // the union of the two leaves runs to its end (its FindAndSetInfiniteRegion,
        // topology.brep_p5, is wired): one result for the one tree, in the first slot;
        // the second slot is released and dropped from the list
        let (mut a, ids) = models(1)?;
        let mut list = ids.clone();
        a.boolean_trees(&mut list, &[0, 1, -1])?;
        assert_eq!(list, [ids[0]]);
        assert!(whole(&a, ids[0]));
        assert_eq!(a.deleted, [ids[1]]);
        Ok(())
    }

    #[test]
    fn a_line_paths_box_is_its_end_points_as_an_smline_gives() -> Result<()> {
        // SmLine::CalculateBoundingBox: the box of the interval's two end points, not
        // enlarged; the pipe's B-spline path takes the same box
        use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
        let (start, end) = (Vec3::new(1.0, -2.0, 0.5), Vec3::new(-3.0, 4.0, 7.25));
        let line = BSplineCurve::create_line_segment(3, start, end)?;
        let request = BoundingBoxRequest {
            normal: true,
            ..BoundingBoxRequest::default()
        };
        let boxes = line.calculate_bounding_box(&line.natural_interval(), request)?;
        let mut expected = crate::geom::extent3d::Extent3d::from_point(start);
        expected.add_point(end);
        let normal = boxes
            .normal
            .ok_or_else(|| SolidError::sm_err("no normal box"))?;
        assert_eq!(
            (normal.min(), normal.max()),
            (expected.min(), expected.max())
        );
        Ok(())
    }
}

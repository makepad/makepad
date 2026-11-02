//! Rail classification and rollover processing for a fillet solver.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp` (5714–5822, 7161–7899).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletSolver::test_roll_over`], [`FilletSolver::test_roll_over_with_ops`] | `SmFilletSolver::TestRollOver` |
//! | [`FilletSolver::process_roll_over`], [`FilletSolver::process_roll_over_with_ops`] | `SmFilletSolver::ProcessRollOver` |
//!
//! Deviations: pseudo-B-rep objects use typed arenas and geometry uses the same
//! typed ids as the solver list. Completed mutations survive failures. Only unavailable
//! callees are stubbed; the entire caller continuation is ported, including both
//! full-rollover calls, the segment walk and topology/surface/corner updates.
//! The four-argument classification constructor calls the real `reset`
//! (`intersect.curve_class_p2`), preserving its defaults and source tolerance.
//! Internal short intervals are retained in descending order for refinement.
//! See `p5/recovery.rs` for the bounded segment-walk deviation. No recursion.

use super::p1::{FilletSolver, FilletSolverOps};
use crate::blend::fillet_classification::{FilletClassObject, LegacyClassificationObjects};
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilletBrep, FilletCornerId, FilletCornerType, FilletGeomId, FilletGeomType,
};
use crate::blend::fillet_geom::p3::SplitCorners;
#[cfg(test)]
use crate::curve::curve::p2::OwnerChange;
use crate::error::{ignored, EntityRef, Result, SolidError};
#[cfg(test)]
use crate::geom::object::ObjectRef;
use crate::intersect::curve_class::p1::{CurveClassification, IntervalPosition};
use crate::intersect::curve_class::p2::ResetOptions;
use crate::intersect::point_class::PointClassType;
use crate::kernel::Kernel;
use crate::query::face::p2::{curve_on_classify, CurveOnClassifyObjects};
use crate::surface::surf_of_revolution::SurfOfRevolution;
use crate::topology::{edge, edgeuse::p1 as eu, edgeuse::p2, Brep, EdgeId, EdgeuseId};

#[path = "p5/recovery.rs"]
mod recovery;

#[path = "p5/rails_fit.rs"]
mod rails_fit;

const OP: &str = "blend::fillet_solver::p5";

/// Resolve handles used by `TestRollOver`, without assuming the two B-reps' arena
/// indices identify the same objects. Curves returned here must be resolvable by
/// the inherited classification geometry resolver.
pub trait RolloverObjects: CurveOnClassifyObjects<EntityRef> {
    /// Identity of a rail's 3D curve in the pseudo-B-rep.
    fn rail_curve(&self, rail: EdgeId) -> EntityRef;
    /// Identity of the original face's UV curve on the rail edgeuse's mate.
    fn rail_mate_uv_curve(&self, mate: EdgeuseId) -> Option<EntityRef>;
    /// The actual virtual `GetCornerType` result for the specified corner.
    ///
    /// # Errors
    /// A resolver must report an unresolved corner, rather than invent its type.
    fn corner_type(&self, corner: FilletCornerId) -> Result<FilletCornerType>;
}

/// Actual identity and classification services, borrowed only during each classification.
pub trait RailClassificationContext {
    /// Actual retained rail curve identity.
    fn rail_curve(&self, rail: EdgeId) -> FilletClassObject;
    /// Actual retained mate UV identity, when present.
    fn rail_mate_uv_curve(&self, pseudo: &Brep, mate: EdgeuseId) -> Option<FilletClassObject>;
    /// Classify the attached record using the two actual topology domains.
    ///
    /// # Errors
    /// Classifier errors, retaining all partial record writes.
    fn classify(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &Brep,
        face: crate::topology::FaceId,
        class: &mut CurveClassification<FilletClassObject>,
    ) -> Result<()>;
    /// Actual corner kind.
    ///
    /// # Errors
    /// An unresolved corner.
    fn corner_type(&self, corner: FilletCornerId) -> Result<FilletCornerType>;
    /// The executive's corner table (`m_vFilletCorners`), which the rollover
    /// recovery's `CalcRailIntEdgeuse` reads.
    fn corners(&self) -> CornerLookup<'_>;
    /// Run `f` on the executive's corners as `ProcessRollOver` reaches them (the
    /// corner type, `CalcRailIntEdgeuse`'s table, and `TopologySplit`'s
    /// `GetFilletCornerOfVertex` and `AddFilletVertex`). The default is this
    /// context's [`Self::corner_type`] and [`Self::corners`] without a vertex-corner
    /// map (a caller without an executive).
    ///
    /// # Errors
    /// Those of `f`.
    fn with_split_corners(
        &mut self,
        f: &mut dyn FnMut(&mut dyn SplitCorners) -> Result<()>,
    ) -> Result<()>
    where
        Self: Sized,
    {
        f(&mut ContextCorners(&*self))
    }
}

/// A [`RailClassificationContext`]'s corner type and table as [`SplitCorners`]: no
/// vertex has a corner (`GetFilletCornerOfVertex` is `NULL`).
struct ContextCorners<'c, C>(&'c C);

impl<C: RailClassificationContext> SplitCorners for ContextCorners<'_, C> {
    fn corner_of_vertex(&self, _vertex: crate::topology::VertexId) -> Option<FilletCornerId> {
        None
    }
    fn lookup(&self) -> CornerLookup<'_> {
        self.0.corners()
    }
    fn corner_mut(
        &mut self,
        _id: FilletCornerId,
    ) -> Result<&mut crate::blend::fillet_corner::p3::FilletCorner> {
        Err(SolidError::null_pointer(OP))
    }
    fn corner_type(&self, id: FilletCornerId) -> Result<FilletCornerType> {
        self.0.corner_type(id)
    }
}

struct LegacyRolloverContext<'a, T>(&'a T);
impl<T: RolloverObjects> RailClassificationContext for LegacyRolloverContext<'_, T> {
    fn rail_curve(&self, rail: EdgeId) -> FilletClassObject {
        FilletClassObject::Legacy(self.0.rail_curve(rail))
    }
    fn rail_mate_uv_curve(&self, _: &Brep, mate: EdgeuseId) -> Option<FilletClassObject> {
        self.0
            .rail_mate_uv_curve(mate)
            .map(FilletClassObject::Legacy)
    }
    fn classify(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        _: &Brep,
        face: crate::topology::FaceId,
        class: &mut CurveClassification<FilletClassObject>,
    ) -> Result<()> {
        curve_on_classify(
            k,
            target,
            face,
            &LegacyClassificationObjects(self.0),
            true,
            class,
            true,
            false,
            true,
        )?;
        Ok(())
    }
    fn corner_type(&self, corner: FilletCornerId) -> Result<FilletCornerType> {
        self.0.corner_type(corner)
    }
    /// The legacy entry points hold no executive: an empty corner table.
    fn corners(&self) -> CornerLookup<'_> {
        CornerLookup::default()
    }
}

pub use super::p1::RailOwnership as RolloverOwnership;

impl FilletSolver {
    /// Classify both rails against their original faces, squeeze tiny end
    /// intervals, refine their classifications and process rollover. Iterates a
    /// snapshot of the solver's geometry list, as upstream does. Every entry is
    /// the geometry's actual id in the caller's pseudo-B-rep arena.
    ///
    /// # Errors
    /// Missing rails/curves/mates are invalid inputs (upstream dereferences them).
    /// Classification and unported recovery errors propagate; completed earlier
    /// changes and the caller's adjusted-corner list are retained.
    pub fn test_roll_over(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        objects: &impl RolloverObjects,
        adjusted_corners: &mut Vec<FilletCornerId>,
        ownership: &mut impl RolloverOwnership,
    ) -> Result<()> {
        self.test_roll_over_with_context(
            k,
            target,
            fillets,
            &mut LegacyRolloverContext(objects),
            adjusted_corners,
            ownership,
        )
    }

    /// Classify actual retained rails through a scoped, short-lived geometry provider.
    ///
    /// # Errors
    /// As [`Self::test_roll_over`], preserving attached partial classifications.
    pub fn test_roll_over_with_context(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        context: &mut impl RailClassificationContext,
        adjusted_corners: &mut Vec<FilletCornerId>,
        ownership: &mut impl RolloverOwnership,
    ) -> Result<()> {
        Self::test_roll_over_with_ops(
            self,
            k,
            target,
            fillets,
            context,
            adjusted_corners,
            ownership,
        )
    }

    /// Classify rails and recover rollover with the actual concrete solver's virtuals.
    /// Existing base entry points delegate to the same implementation.
    ///
    /// # Errors
    /// As [`Self::test_roll_over_with_context`], retaining each mutation before failure.
    pub fn test_roll_over_with_ops(
        solver: &mut dyn FilletSolverOps,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        context: &mut impl RailClassificationContext,
        adjusted_corners: &mut Vec<FilletCornerId>,
        ownership: &mut impl RolloverOwnership,
    ) -> Result<()> {
        let geoms = solver.solver().fillet_geoms();
        for id in geoms {
            for ii in 0..2 {
                let rail = fillets
                    .geom(id)?
                    .rail(ii)
                    .ok_or_else(|| SolidError::invalid_input(OP, "TestRollOver: missing rail"))?;
                let curve = edge::curve(fillets.brep(), rail)?.ok_or_else(|| {
                    SolidError::invalid_input(OP, "TestRollOver: missing rail curve")
                })?;
                let interval = curve.natural_interval();
                let primary = eu::primary_edgeuse(fillets.brep(), rail)?.ok_or_else(|| {
                    SolidError::invalid_input(OP, "TestRollOver: missing primary edgeuse")
                })?;
                let mate = eu::mate(fillets.brep(), primary)?
                    .ok_or_else(|| SolidError::invalid_input(OP, "TestRollOver: missing mate"))?;
                let mut cc = CurveClassification::new(k);
                cc.reset(
                    k,
                    &ResetOptions {
                        curve: Some(context.rail_curve(rail)),
                        interval: Some(interval),
                        uv_curve: context.rail_mate_uv_curve(fillets.brep(), mate),
                        zone_tol_3d: solver.solver().approx_tol_3d,
                        ..ResetOptions::default()
                    },
                )?;
                fillets.fillet_edge_mut(rail)?.set_curve_class(Some(cc));
                let face = fillets.fillet_edge(rail)?.original_face().ok_or_else(|| {
                    SolidError::invalid_input(OP, "TestRollOver: missing original face")
                })?;
                // Install the classification before calling the fallible operation:
                // upstream leaves that object attached even when classification fails.
                fillets.with_rail_classification(rail, |pseudo, cc| {
                    context.classify(k, target, pseudo, face, cc)
                })?;
                if k.fixes.fillet_rail_rim_on_face {
                    join_intervals_on_face(fillets, rail)?;
                }
                finish_classification(
                    solver,
                    k,
                    target,
                    fillets,
                    id,
                    ii,
                    rail,
                    &mut MissingClassification,
                )?;
            }
            context.with_split_corners(&mut |corners| {
                Self::process_roll_over_with_ops(
                    solver,
                    k,
                    target,
                    fillets,
                    id,
                    corners,
                    adjusted_corners,
                    ownership,
                )
            })?;
        }
        Ok(())
    }

    /// Process an existing pair of rail classifications. Blend patches and
    /// single-interval rails that both remain classified need no repair.
    /// `corners` resolves the type of the corner on rail 0's start vertex only when
    /// full rollover is needed; `adjusted_corners` is append-only upstream output.
    ///
    /// # Errors
    /// A missing classification or corner returns `SM_ERR_NULL_POINTER`. Unsupported
    /// full-rollover corner types return `SM_ERR`. Recovery through unavailable
    /// units returns `Unported`; invalid handles are reported by the arena.
    pub fn process_roll_over(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        id: FilletGeomId,
        corners: &mut dyn SplitCorners,
        adjusted_corners: &mut Vec<FilletCornerId>,
        ownership: &mut impl RolloverOwnership,
    ) -> Result<()> {
        Self::process_roll_over_with_ops(
            self,
            k,
            target,
            fillets,
            id,
            corners,
            adjusted_corners,
            ownership,
        )
    }

    /// Process rollover through the actual concrete solver's virtual operations.
    ///
    /// # Errors
    /// As [`Self::process_roll_over`], preserving the original mutation/error order.
    #[allow(clippy::too_many_arguments)]
    pub fn process_roll_over_with_ops(
        solver: &mut dyn FilletSolverOps,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        id: FilletGeomId,
        corners: &mut dyn SplitCorners,
        adjusted_corners: &mut Vec<FilletCornerId>,
        ownership: &mut impl RolloverOwnership,
    ) -> Result<()> {
        recovery::Recovery {
            solver,
            k,
            target,
            fillets,
            corners,
            ownership,
        }
        .process(id, adjusted_corners, &mut recovery::Missing)
    }
}

/// WR-301 (#485): the rail leaves its face only where it runs outside it. Neighbouring
/// intervals on the face or on its boundary (a rail touching a vertex of the boundary or
/// running along a piece of an edge, as a blend whose setback reaches exactly the rim of
/// a hole does) are joined into one, so the rollover sees only the intervals that leave
/// the face (source fork `f6ed05a`).
fn join_intervals_on_face(fillets: &mut FilletBrep, rail: EdgeId) -> Result<()> {
    let cc = fillets
        .fillet_edge_mut(rail)?
        .curve_class
        .as_mut()
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    for index in (1..cc.size()).rev() {
        let this = cc.at(index)?.mid;
        let prev = cc.at(index - 1)?.mid;
        if prev.point_class() == PointClassType::Unknown
            || this.point_class() == PointClassType::Unknown
        {
            continue;
        }
        if prev.point_class() != PointClassType::Face {
            cc.at_mut(index - 1)?.mid = this;
        }
        cc.connect_intervals(index, IntervalPosition::Start, false)?;
    }
    Ok(())
}

trait ClassificationOps {
    fn connect(
        &mut self,
        _k: &Kernel,
        fillets: &mut FilletBrep,
        rail: EdgeId,
        interval: usize,
        at_end: bool,
        allow_removal_of_ends: bool,
    ) -> Result<()> {
        fillets
            .fillet_edge_mut(rail)?
            .curve_class
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(OP))?
            .connect_intervals(
                interval,
                if at_end {
                    IntervalPosition::End
                } else {
                    IntervalPosition::Start
                },
                allow_removal_of_ends,
            )
    }
    fn refine(
        &mut self,
        solver: &mut dyn FilletSolverOps,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        _geom: FilletGeomId,
        which: usize,
        rail: EdgeId,
        small: &[usize],
    ) -> Result<()> {
        let mut classification = fillets
            .fillet_edge_mut(rail)?
            .curve_class
            .take()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let result = super::p3::refine_rail_curve_classification_scoped_with_ops(
            solver,
            k,
            target,
            fillets,
            which,
            small,
            rail,
            &mut classification,
        );
        // The classification is detached only to share immutable rail data with
        // the callee; restore its partial writes on every return path.
        fillets.fillet_edge_mut(rail)?.curve_class = Some(classification);
        result
    }
}
struct MissingClassification;
impl ClassificationOps for MissingClassification {}

fn finish_classification(
    solver: &mut dyn FilletSolverOps,
    k: &Kernel,
    target: &mut Brep,
    fillets: &mut FilletBrep,
    geom: FilletGeomId,
    which: usize,
    rail: EdgeId,
    ops: &mut impl ClassificationOps,
) -> Result<()> {
    let small = squeeze_rail(k, fillets, rail, solver.solver().approx_tol_3d, ops)?;
    // Refinement is warning-only upstream, but retain its completed mutations
    // and propagate an unavailable implementation rather than faking success.
    ignored(ops.refine(solver, k, target, fillets, geom, which, rail, &small))?;
    Ok(())
}

fn squeeze_rail(
    k: &Kernel,
    fillets: &mut FilletBrep,
    rail: EdgeId,
    tolerance: f64,
    ops: &mut impl ClassificationOps,
) -> Result<Vec<usize>> {
    let count = fillets
        .fillet_edge(rail)?
        .curve_class()
        .ok_or_else(|| SolidError::null_pointer(OP))?
        .size();
    let mut small = Vec::new();
    if count > 1 {
        for index in (0..count).rev() {
            let interval = fillets
                .fillet_edge(rail)?
                .curve_class()
                .ok_or_else(|| SolidError::null_pointer(OP))?
                .sub_interval(index)?;
            let curve =
                edge::curve(fillets.brep(), rail)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            let start = curve.evaluate_point(k, interval.min())?;
            let end = curve.evaluate_point(k, interval.max())?;
            if start.distance(end) > 50.0 * tolerance {
                continue;
            }
            if index == 0 {
                ops.connect(k, fillets, rail, 0, false, true)?;
            } else if index == count - 1 {
                ops.connect(k, fillets, rail, index, true, true)?;
            } else {
                small.push(index);
            }
        }
    }
    Ok(small)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_geom::p1::{FilletEdge, FilletGeom, FilletVertex};
    use crate::intersect::point_class::PointClassType;
    use crate::math::Vec3;

    /// A corner store resolving only corner types, through `F`.
    struct CornerTypes<F>(F);

    impl<F: Fn(FilletCornerId) -> Result<FilletCornerType>> SplitCorners for CornerTypes<F> {
        fn corner_of_vertex(&self, _: crate::topology::VertexId) -> Option<FilletCornerId> {
            None
        }
        fn lookup(&self) -> CornerLookup<'_> {
            CornerLookup::default()
        }
        fn corner_mut(
            &mut self,
            _: FilletCornerId,
        ) -> Result<&mut crate::blend::fillet_corner::p3::FilletCorner> {
            Err(SolidError::null_pointer(OP))
        }
        fn corner_type(&self, id: FilletCornerId) -> Result<FilletCornerType> {
            (self.0)(id)
        }
    }

    struct NoOwnership;
    impl RolloverOwnership for NoOwnership {
        fn edge_owner(&self, _: EdgeId) -> Result<(ObjectRef, Option<ObjectRef>)> {
            Err(SolidError::invalid_input(OP, "unexpected owner lookup"))
        }
        fn owner_changed(
            &mut self,
            _: &mut crate::curve::curve::Curve,
            _: OwnerChange,
        ) -> Result<()> {
            Err(SolidError::invalid_input(
                OP,
                "unexpected owner notification",
            ))
        }
    }

    fn classified_rails(k: &Kernel, fb: &mut FilletBrep) -> FilletGeomId {
        let mut geom = FilletGeom::new(k, None, FilletGeomType::Default).unwrap();
        for i in 0..2 {
            let a = fb.brep_mut().add_vertex(Vec3::ZERO).unwrap();
            let b = fb.brep_mut().add_vertex(Vec3::X).unwrap();
            let edge = fb.brep_mut().add_edge(a, b).unwrap();
            let vu = fb
                .brep_mut()
                .add_vertexuse_record(crate::topology::Vertexuse {
                    vertex: Some(a),
                    ..crate::topology::Vertexuse::default()
                })
                .unwrap();
            let eu = fb
                .brep_mut()
                .add_edgeuse_record(crate::topology::Edgeuse {
                    edge: Some(edge),
                    vertexuse: Some(vu),
                    ..crate::topology::Edgeuse::default()
                })
                .unwrap();
            fb.brep_mut().edge_mut(edge).unwrap().edgeuses.push(eu);

            fb.insert_fillet_vertex(a, FilletVertex::new(k, Some(FilletCornerId(42 + i as u32))));
            let mut record = FilletEdge::with_corner(None);
            let mut cc = CurveClassification::new(k);
            cc.set_size(k, 1).unwrap();
            cc.at_mut(0)
                .unwrap()
                .mid
                .set_class_object(PointClassType::Edge, Some(EntityRef::Edge(edge)));
            record.set_indexed_curve_class(Some(&cc)).unwrap();
            fb.insert_fillet_edge(edge, record);
            geom.set_rail(i, Some(edge));
        }
        fb.add_geom(geom)
    }

    #[test]
    fn resolver_is_lazy_and_failure_preserves_caller_output() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let id = classified_rails(&k, &mut fb);
        let mut solver = FilletSolver::new(1e-5, 1e-3, 1e-3);
        let mut target = Brep::new();
        let mut adjusted = vec![FilletCornerId(9), FilletCornerId(9)];
        solver
            .process_roll_over(
                &k,
                &mut target,
                &mut fb,
                id,
                &mut CornerTypes(|_| panic!("known rails must not resolve a corner")),
                &mut adjusted,
                &mut NoOwnership,
            )
            .unwrap();
        let rail = fb.geom(id).unwrap().rail(1).unwrap();
        fb.fillet_edge_mut(rail)
            .unwrap()
            .curve_class
            .as_mut()
            .unwrap()
            .at_mut(0)
            .unwrap()
            .mid
            .set_class_object(PointClassType::Unknown, None);
        let refusal = SolidError::invalid_input(OP, "unresolved corner");
        let result = solver.process_roll_over(
            &k,
            &mut target,
            &mut fb,
            id,
            &mut CornerTypes(|corner| {
                assert_eq!(corner, FilletCornerId(42));
                Err(refusal.clone())
            }),
            &mut adjusted,
            &mut NoOwnership,
        );
        assert_eq!(result, Err(refusal));
        assert_eq!(adjusted, [FilletCornerId(9), FilletCornerId(9)]);
        let result = solver.process_roll_over(
            &k,
            &mut target,
            &mut fb,
            id,
            &mut CornerTypes(|_| Ok(FilletCornerType::OneByOne)),
            &mut adjusted,
            &mut NoOwnership,
        );
        assert_eq!(
            result,
            Err(SolidError::invalid_input(
                "blend::fillet_solver::p3",
                "solver without edge use",
            ))
        );
    }
}

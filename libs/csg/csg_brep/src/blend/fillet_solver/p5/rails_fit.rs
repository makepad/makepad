//! `SmFilletSolver::RailsFitTheirFaces` and `LeavesPastAnEnd`: whether every rail of a
//! solver's fillet stays on the face it rolls on.
//!
//! Not upstream (SMLib 1.0.1 has neither): the fork's check, source fork
//! `fc43974` (refusal), `967f7ab` (`LeavesPastAnEnd`) and `518566f` (a run of off-face
//! intervals is judged once, WR-302, source issue #841 and #1024). Used only
//! with the opt-in `Kernel::fixes.fillet_refuse_rails_leaving_faces`.
//!
//! Deviations: a run of off-face intervals between two stretches on the face is skipped
//! when it both leaves and returns past an end of the filleted edge (#1580); `518566f`
//! tries [`leaves_past_an_end`] only for the first and last runs and refuses such a
//! round. The fork's later cliff-overflow construction (`SmFilletCliffOverflow.cpp`,
//! which builds a convex blend over a cliff and subtracts what it removes) is not
//! ported, so every blend that leaves its face between its ends is refused, as in
//! `fc43974`.

use super::{RailClassificationContext, OP};
use crate::blend::fillet_classification::FilletClassObject;
use crate::blend::fillet_geom::p1::{FilletBrep, FilletGeomType};
use crate::blend::fillet_solver::p1::{
    test_manifold_tangent_rollover, FilletSolver, FilletSolverOps,
};
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::global_solver::SolverOperation;
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::curve_class::p2::ResetOptions;
use crate::intersect::point_class::{PointClassType, PointClassification};
use crate::kernel::Kernel;
use crate::math::BIG_DOUBLE;
use crate::topology::{edge, edgeuse::p1 as eu, Brep, FaceId};

/// `SmFilletSolver::LeavesPastAnEnd`: whether a rail that leaves the face it rolls on at
/// `leave` (rail parameter `rail_param`) does so past an end of the filleted edge, where
/// the corner at that end trims it; otherwise it leaves the face beside the edge.
///
/// At an end the fillet stops in one of two ways, and its rail leaves the face only after
/// it: the face ends there in its side edge (the edge before or after the filleted edge
/// in the face's loop), and the rail leaves across that side edge or through one of its
/// vertices; or the face goes on past the end, and the rail stays on it to the end's cross
/// section and beyond (the edge point nearest where it leaves is an end).
fn leaves_past_an_end(
    solver: &FilletSolver,
    k: &Kernel,
    target: &Brep,
    face: FaceId,
    rail: &Curve,
    rail_param: f64,
    leave: &PointClassification<FilletClassObject>,
) -> Result<bool> {
    let fil_eu = solver
        .edgeuse(0)?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let fil_edge = eu::edge(target, fil_eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let edge_curve = edge::curve(target, fil_edge)?.ok_or_else(|| SolidError::null_pointer(OP))?;

    // Across or through the face's side edge at an end.
    let (leave_edge, leave_vertex) = match (leave.point_class(), leave.object()) {
        (PointClassType::Edge, Some(o)) => match o.target_entity()? {
            EntityRef::Edge(e) => (Some(e), None),
            _ => (None, None),
        },
        (PointClassType::Vertex, Some(o)) => match o.target_entity()? {
            EntityRef::Vertex(v) => (None, Some(v)),
            _ => (None, None),
        },
        _ => (None, None),
    };
    for use_ in edge::edgeuses(target, fil_edge, None, None)? {
        if eu::face(target, use_)? != Some(face) || !eu::is_loop_edgeuse(target, use_)? {
            continue;
        }
        for next in [
            eu::cw_edgeuse(target, use_)?,
            eu::ccw_edgeuse(target, use_)?,
        ] {
            let Some(side) = next.map(|n| eu::edge(target, n)).transpose()?.flatten() else {
                continue;
            };
            if side == fil_edge {
                continue;
            }
            let first = edge::vertex(target, side)?;
            let past = leave_edge == Some(side)
                || leave_vertex.is_some_and(|v| {
                    first == Some(v)
                        || edge::other_vertex(target, side, first).is_ok_and(|o| o == Some(v))
                });
            if past {
                return Ok(true);
            }
        }
    }

    // On or beyond the cross section at an end.
    let ivl = edge::interval(target, fil_edge)?;
    let leave_point = rail.evaluate_point(k, rail_param)?;
    let dropped = edge_curve.drop_point(
        k,
        &ivl,
        leave_point,
        None,
        BIG_DOUBLE,
        None,
        SolverOperation::Minimize,
    )?;
    if !dropped.success {
        return Err(SolidError::sm_err(OP));
    }
    let foot = edge_curve.evaluate_point(k, dropped.parameter)?;
    for end in [ivl.min(), ivl.max()] {
        if foot.distance(edge_curve.evaluate_point(k, end)?) <= solver.approx_tol_3d {
            return Ok(true);
        }
    }
    Ok(false)
}

impl FilletSolver {
    /// `SmFilletSolver::RailsFitTheirFaces` (the fork's refusal, see the module doc):
    /// whether every rail of this solver's fillet, untrimmed, stays on the face it rolls
    /// on. Each rail is classified against its face. It may run off the face past the
    /// filleted edge's ends ([`leaves_past_an_end`]) and off across a tangent side edge (a
    /// rollover onto the next face). A rail that is nowhere on its face, or that leaves it
    /// between its ends across an edge that is not tangent (a wall, an opening's rim)
    /// means the blend is larger than the faces: `false`. Touching or running along the
    /// face's boundary is on the face.
    ///
    /// The classification is made as `TestRollOver`'s is and is not kept: each rail's
    /// attached classification is as it was.
    ///
    /// # Errors
    /// Missing rails, faces or edge uses (upstream dereferences them); those of the
    /// classification, the drops and the tangency test.
    pub fn rails_fit_their_faces_with_ops(
        solver: &mut dyn FilletSolverOps,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        context: &mut impl RailClassificationContext,
    ) -> Result<bool> {
        let mut fit = true;
        for id in solver.solver().fillet_geoms() {
            if fillets.geom(id)?.fillet_geom_type() != FilletGeomType::Default {
                continue;
            }
            for ii in 0..2 {
                let rail = fillets
                    .geom(id)?
                    .rail(ii)
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                let curve = edge::curve(fillets.brep(), rail)?
                    .ok_or_else(|| SolidError::null_pointer(OP))?
                    .clone();
                let face = fillets
                    .fillet_edge(rail)?
                    .original_face()
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                let primary = eu::primary_edgeuse(fillets.brep(), rail)?
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                let mate = eu::mate(fillets.brep(), primary)?
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                let mut cc = CurveClassification::new(k);
                cc.reset(
                    k,
                    &ResetOptions {
                        curve: Some(context.rail_curve(rail)),
                        interval: Some(curve.natural_interval()),
                        uv_curve: context.rail_mate_uv_curve(fillets.brep(), mate),
                        zone_tol_3d: solver.solver().approx_tol_3d,
                        ..ResetOptions::default()
                    },
                )?;
                let attached = fillets.fillet_edge_mut(rail)?.curve_class.replace(cc);
                let classified = fillets.with_rail_classification(rail, |pseudo, cc| {
                    context.classify(k, target, pseudo, face, cc)
                });
                let intervals = fillets
                    .fillet_edge(rail)?
                    .curve_class()
                    .map(|cc| cc.intervals().to_vec());
                fillets.fillet_edge_mut(rail)?.curve_class = attached;
                classified?;
                let intervals = intervals.ok_or_else(|| SolidError::null_pointer(OP))?;

                let n = intervals.len();
                let off_face = |j: usize| {
                    intervals
                        .get(j)
                        .is_some_and(|i| i.mid.point_class() == PointClassType::Unknown)
                };
                let mut on_face = false;
                for (j, ivl) in intervals.iter().enumerate() {
                    if ivl.mid.point_class() != PointClassType::Unknown {
                        on_face = true;
                        continue;
                    }
                    // A run of intervals off the face (split where the rail passes through
                    // a point of the face's boundary from outside to outside) leaves the
                    // face once: judged at its first interval, from where it leaves to the
                    // rail's end or start.
                    if j > 0 && off_face(j - 1) {
                        continue;
                    }
                    let mut j_last = j;
                    while off_face(j_last + 1) {
                        j_last += 1;
                    }
                    let last = intervals
                        .get(j_last)
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    // Where the rail leaves the face: the first interval ends there, the
                    // others start.
                    let (leave, d_leave) = if j == 0 {
                        (&last.end, last.interval.max())
                    } else {
                        (&ivl.start, ivl.interval.min())
                    };
                    let past_end = |point: &PointClassification<FilletClassObject>, d: f64| {
                        leaves_past_an_end(solver.solver(), k, target, face, &curve, d, point)
                    };
                    if j == 0 || j_last + 1 == n {
                        if past_end(leave, d_leave)? {
                            continue;
                        }
                    } else if past_end(leave, d_leave)? && past_end(&last.end, last.interval.max())?
                    {
                        // A run between two stretches on the face that lies wholly past an
                        // end (it leaves and comes back past it) is trimmed by the corner
                        // there too (source issue #1580; the fork's `518566f`
                        // tried only the first and last runs).
                        continue;
                    }
                    let side = match (leave.point_class(), leave.object()) {
                        (PointClassType::Edge, Some(o)) => match o.target_entity()? {
                            EntityRef::Edge(e) => Some(e),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(side) = side {
                        let tolerance = solver.solver().tangency_tolerance;
                        if test_manifold_tangent_rollover(k, target, side, tolerance)?.0 {
                            continue; // a rollover onto the next face
                        }
                    }
                    fit = false;
                }
                if !on_face {
                    fit = false;
                }
            }
        }
        Ok(fit)
    }
}

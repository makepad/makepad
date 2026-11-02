//! The loop gaps an extended offset leaves and their closing edges (opt-in, fork
//! source fork 5d428ae, 7ff0a94, 34318f4).
//!
//! Upstream: the source fork's `source/SMLib/src/SmOffsetExecutive.cpp` (not in the
//! pinned SMLib).
//!
//! | Rust                                               | Upstream (fork)                                    |
//! |----------------------------------------------------|----------------------------------------------------|
//! | [`OffsetExecutive::close_loop_gaps`]               | `SmOffsetExecutive::CloseLoopGaps`                 |
//! | `close_loop_gap` (private)                         | `SmOffsetExecutive::CloseLoopGap`                  |
//! | [`OffsetExecutive::split_edges_at_step_corners`]   | `SmOffsetExecutive::SplitEdgesAtStepCorners`       |
//!
//! Deviations: the offset B-rep is taken out of the executive for the edits and put back
//! on every return. `MakeVertexSplitEdge`'s `SM_ERR` (a split refused near an end) is a
//! successful Rust return with `split == false`; it fails here as the fork's `SER` does.
//! `GetCreateExtendedSurface` keys the face's surface by the result identity of its face,
//! as the executive's edge offsets do. Bounds: each face's gap search stops with `SM_ERR`
//! after 64 passes (the fork's guard); the other loops run over finite lists.

use super::p1::OffsetExecutive;
use super::p2::OffsetNotifications;
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::kernel::Kernel;
use crate::math::{max2, Point3, EFF_ZERO};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::edge::closest_point;
use crate::surface::surface::p4::{global_point_solve_starts, NoObject};
use crate::topology::brep::p1 as bp1;
use crate::topology::brep::p9::make_vertex_split_edge_with_state;
use crate::topology::edge::{self, NotificationStores, NotifyState, SetCurveOptions};
use crate::topology::edgeuse::{p1 as eu, p3 as eu3};
use crate::topology::face::{p2 as face2, p4 as face4};
use crate::topology::{vertexuse, Brep, EdgeId, EdgeuseId, FaceId, VertexId};

const OP: &str = "offset::offset_executive::step_gaps";

/// `SM_ERR_OFFSET_EDGE_VANISHES` (the fork's `SmMessages.h`): an offset moves a vertex
/// along an edge past the edge's other end, a change of topology it does not make.
pub(crate) const SM_ERR_OFFSET_EDGE_VANISHES: i32 = 1043;

/// The gap search's passes per face (the fork's `lGuard`).
const MAX_GAP_PASSES: usize = 64;

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

fn sm_err() -> SolidError {
    SolidError::sm_err(OP)
}

/// The vertex of an edge use's vertex use (`GetVertexuse()->GetVertex()`).
fn edgeuse_vertex(b: &Brep, u: EdgeuseId) -> Result<VertexId> {
    let vu = eu::vertexuse(b, u)?.ok_or_else(|| null("edge use without vertex use"))?;
    vertexuse::vertex(b, vu)?.ok_or_else(|| null("vertex use without vertex"))
}

/// `pEU->NormalizedEvaluate(t, FALSE, rPoint)`.
fn point_at(k: &Kernel, b: &mut Brep, u: EdgeuseId, t: f64) -> Result<Point3> {
    Ok(eu3::normalized_evaluate(k, b, u, t, false, false, true)?.point)
}

/// The parameter and value of the first solution of `pCurve->GlobalPointSolve(ivl,
/// SM_SO_MINIMIZE, point, tol, NULL, NULL, SM_SR_SINGLE, ..)`.
fn closest_on(
    k: &Kernel,
    curve: &BSplineCurve,
    ivl: &Extent1d,
    point: Point3,
    tol: f64,
) -> Result<Option<(f64, f64)>> {
    let c = Curve::from(curve.clone());
    let sols = c.global_point_solve(
        k,
        ivl,
        SolverOperation::Minimize,
        point,
        tol,
        SolutionRequested::Single,
        &GlobalPointSolveOptions::<NoObject>::default(),
    )?;
    match sols.get(0) {
        Some(s) => Ok(Some((s.start.param(0)?, s.start.solution_value))),
        None => Ok(None),
    }
}

impl OffsetExecutive {
    /// `CloseLoopGaps` (fork, opt-in `Kernel::fixes.local_operation_step_walls`, local
    /// operations only; the fork runs it for every extended offset since 7ff0a94, not
    /// ported yet): where consecutive edge uses of an offset face no longer meet at their
    /// shared vertex (a local operation's step left an edge behind), the edge whose end lies away from the
    /// vertex gets the straight segment to it (which must lie on the face's extended
    /// surface) and is split there, or is trimmed to it when the vertex moved into it.
    /// Each face is searched again after each gap closed.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a gap neither end of which is at the vertex, a segment off the
    /// surface, or more than [`MAX_GAP_PASSES`] gaps on one face;
    /// [`SM_ERR_OFFSET_EDGE_VANISHES`] where the vertex moved to or past the edge's other
    /// end; those of the evaluations, solves, curve edits
    /// and the split; [`SolidError::InvalidInput`] for missing topology.
    pub(super) fn close_loop_gaps(
        &mut self,
        kernel: &mut Kernel,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let mut b = self
            .take_offset_brep()
            .ok_or_else(|| null("no offset B-rep"))?;
        let r = self.close_loop_gaps_in(kernel, &mut b, notifications);
        self.set_offset_brep(b);
        r
    }

    fn close_loop_gaps_in(
        &mut self,
        kernel: &mut Kernel,
        b: &mut Brep,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let approx_tol = self.approximation_tol_3d();
        for f in bp1::faces(b, None)? {
            // Each gap closed changes the loop: look again until none is left (each gap
            // is closed once).
            let mut passes = 0;
            let mut closed = true;
            while closed {
                closed = false;
                passes += 1;
                if passes > MAX_GAP_PASSES {
                    return Err(sm_err());
                }
                let fu =
                    face2::upward_faceuse(b, f)?.ok_or_else(|| null("face without face use"))?;
                let loopuses = b.faceuse(fu)?.loopuses.clone();
                'loops: for lu in loopuses {
                    let eus = eu::loopuse_edgeuses(b, lu)?;
                    let num = eus.len();
                    for (kk, &this) in eus.iter().enumerate() {
                        let next = *eus
                            .get((kk + 1) % num)
                            .ok_or_else(|| null("loop without edge uses"))?;
                        if this == next {
                            continue;
                        }
                        let v = edgeuse_vertex(b, next)?;
                        let mate =
                            eu::mate(b, this)?.ok_or_else(|| null("edge use without mate"))?;
                        if edgeuse_vertex(b, mate)? != v {
                            continue;
                        }
                        let tol_this = b
                            .edge(eu::edge(b, this)?.ok_or_else(|| null("edge use without edge"))?)?
                            .tolerance;
                        let tol_next = b
                            .edge(eu::edge(b, next)?.ok_or_else(|| null("edge use without edge"))?)?
                            .tolerance;
                        let tol = 10.0 * max2(max2(tol_this, tol_next), approx_tol);
                        let end = point_at(kernel, b, this, 1.0)?;
                        let start = point_at(kernel, b, next, 0.0)?;
                        if end.distance(start) <= tol {
                            continue;
                        }
                        let sv = b.vertex(v)?.point;
                        if end.distance(sv) <= tol && start.distance(sv) > tol {
                            self.close_loop_gap(kernel, b, f, next, true, sv, tol, notifications)?;
                        } else if start.distance(sv) <= tol && end.distance(sv) > tol {
                            self.close_loop_gap(kernel, b, f, this, false, sv, tol, notifications)?;
                        } else {
                            return Err(sm_err());
                        }
                        closed = true;
                        break 'loops;
                    }
                }
            }
        }
        Ok(())
    }

    /// `CloseLoopGap(pFace, pEdgeuse, bAtStart, crVertexPoint, dTol)`: one gap closed
    /// (see [`OffsetExecutive::close_loop_gaps`]).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one fork routine
    fn close_loop_gap(
        &mut self,
        kernel: &mut Kernel,
        b: &mut Brep,
        f: FaceId,
        u: EdgeuseId,
        at_start: bool,
        vertex_point: Point3,
        tol: f64,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let e = eu::edge(b, u)?.ok_or_else(|| null("edge use without edge"))?;
        let surface = b
            .face(f)?
            .surface
            .as_ref()
            .ok_or_else(|| null("face without surface"))?
            .clone();

        // The edge curve's end at the gap: its start where the edge use runs with it and
        // starts there.
        let same = eu::orientation(b, u)? == OrientType::Same;
        let at_curve_start = at_start == same;
        let trimmed = edge::create_trimmed_nurbs_curve(kernel, b, e)?
            .ok_or_else(|| null("edge without NURBS curve"))?;
        let mut curve = match trimmed.kind() {
            CurveKind::BSpline(c) => c.clone(),
            _ => return Err(null("trimmed NURBS curve is not a B-spline")),
        };
        let ivl = curve.natural_interval();
        let gap_end = curve.evaluate_point(if at_curve_start { ivl.min() } else { ivl.max() })?;

        // A vertex that moved along the edge into it (a step down): the edge is trimmed to
        // it.
        if let Some((at, value)) = closest_on(kernel, &curve, &ivl, vertex_point, tol)? {
            if value <= tol {
                let mut keep = if at_curve_start {
                    Extent1d::from_min_max(at, ivl.max())
                } else {
                    Extent1d::from_min_max(ivl.min(), at)
                };
                let long_enough = keep.length() > EFF_ZERO * (1.0 + ivl.max_dimension());
                if !long_enough {
                    // (fork 34318f4) the vertex moved to the edge's other end or past it:
                    // the edge vanishes, the face turns inside out
                    return Err(SolidError::Upstream {
                        operation: OP,
                        code: SM_ERR_OFFSET_EDGE_VANISHES,
                        entity: None,
                    });
                }
                curve.trim(kernel, &mut keep, TrimMode::Notify)?;
                let new_ivl = curve.natural_interval();
                set_edge_curve(kernel, b, e, curve, notifications)?;
                edge::set_interval(b, e, &new_ivl)?;
                return Ok(());
            }
        }

        // The straight segment must lie on the face's surface (as extended for the offset:
        // the face's own surface ends at its old boundary).
        let key = (notifications.result_key_of)(EntityRef::Face(f)).key.0;
        let ext = self
            .get_create_extended_surface(kernel, key, &surface)?
            .clone();
        let ext_domain = ext.natural_uv_domain_with_nurb(kernel)?;
        for ii in 1..4 {
            let w = f64::from(ii) / 4.0;
            let pnt = vertex_point + (gap_end - vertex_point) * w;
            let sols = global_point_solve_starts(
                kernel,
                &ext,
                &ext_domain,
                SolverOperation::Minimize,
                pnt,
                tol,
                SolutionRequested::Single,
            )?;
            let on = sols.first().is_some_and(|s| s.solution_value <= tol);
            if !on {
                return Err(sm_err());
            }
        }

        // The segment, from the vertex to the gap end, joined at the gap end.
        let mut segment = BSplineCurve::create_line_segment(3, vertex_point, gap_end)?;
        curve.join_with(kernel, !at_curve_start, &mut segment, true, Some(tol))?;

        // Where they join: the parameter of the gap end on the joined curve.
        let joined = curve.natural_interval();
        let joint = closest_on(kernel, &curve, &joined, gap_end, tol)?
            .ok_or_else(sm_err)?
            .0;

        // The edge takes the joined curve and is split at the joint.
        set_edge_curve(kernel, b, e, curve, notifications)?;
        edge::set_interval(b, e, &joined)?;
        let mut stores = NotificationStores {
            records: &mut *notifications.records,
            attributes: &mut *notifications.attributes,
            key_of: notifications.result_key_of,
        };
        let mut state = NotifyState::from_kernel_stores(&mut stores);
        let output = make_vertex_split_edge_with_state(
            kernel,
            b,
            e,
            joint,
            &mut state,
            notifications.result_curve_ref,
        )?;
        if !output.split {
            return Err(sm_err());
        }
        output.vertex.ok_or_else(|| null("split without vertex"))?;
        Ok(())
    }

    /// `SplitEdgesAtStepCorners` (fork 7ff0a94, opt-in
    /// `Kernel::fixes.local_operation_step_walls`, local operations only): every edge of
    /// the offset B-rep that runs past a corner of a step wall (within ten times its
    /// tolerance or the approximation tolerance, and not at its own ends) is split there,
    /// so that each part meets its own neighbour. In a local operation the only cap faces
    /// are step walls.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a refused split; those of the closest point and the split;
    /// [`SolidError::InvalidInput`] for missing topology.
    pub(super) fn split_edges_at_step_corners(
        &mut self,
        kernel: &mut Kernel,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let mut b = self
            .take_offset_brep()
            .ok_or_else(|| null("no offset B-rep"))?;
        let r = self.split_edges_at_step_corners_in(kernel, &mut b, notifications);
        self.set_offset_brep(b);
        r
    }

    fn split_edges_at_step_corners_in(
        &mut self,
        kernel: &mut Kernel,
        b: &mut Brep,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let approx_tol = self.approximation_tol_3d();
        let walls: Vec<FaceId> = self.edge_to_faces.all_seconds();
        let mut corners: Vec<VertexId> = Vec::new();
        for w in walls {
            for v in face4::vertices(b, w, None, false, None)? {
                if !corners.contains(&v) {
                    corners.push(v);
                }
            }
        }
        for v in corners {
            let p = b.vertex(v)?.point;
            let edges: Vec<EdgeId> = bp1::edges(b, None)?;
            for e in edges {
                let tol = 10.0 * max2(b.edge(e)?.tolerance, approx_tol);
                let start = edge::start_vertex(b, e)?.ok_or_else(|| null("edge without vertex"))?;
                let end = edge::end_vertex(b, e)?.ok_or_else(|| null("edge without vertex"))?;
                if b.vertex(start)?.point.distance(p) <= tol
                    || b.vertex(end)?.point.distance(p) <= tol
                {
                    continue;
                }
                let Some(cp) = closest_point(kernel, b, e, p)? else {
                    continue;
                };
                let near = cp.distance <= tol;
                if !near {
                    continue;
                }
                let mut stores = NotificationStores {
                    records: &mut *notifications.records,
                    attributes: &mut *notifications.attributes,
                    key_of: notifications.result_key_of,
                };
                let mut state = NotifyState::from_kernel_stores(&mut stores);
                let output = make_vertex_split_edge_with_state(
                    kernel,
                    b,
                    e,
                    cp.parameter,
                    &mut state,
                    notifications.result_curve_ref,
                )?;
                if !output.split {
                    return Err(sm_err());
                }
            }
        }
        Ok(())
    }
}

/// `pEdge->SetCurve(pCurve, TRUE); pCurve->SetOwner(pEdge)` on the offset B-rep.
fn set_edge_curve(
    kernel: &mut Kernel,
    b: &mut Brep,
    e: EdgeId,
    curve: BSplineCurve,
    notifications: &mut OffsetNotifications<'_>,
) -> Result<()> {
    let mut stores = NotificationStores {
        records: &mut *notifications.records,
        attributes: &mut *notifications.attributes,
        key_of: notifications.result_key_of,
    };
    let mut state = NotifyState::from_kernel_stores(&mut stores);
    let options = SetCurveOptions {
        delete_old_curve: true,
        dbg_warn_leaks: true,
        call_notify: true,
    };
    state.with_env(kernel, |env| {
        edge::set_curve(b, e, Some(Curve::from(curve)), options, env, [None, None])?;
        let owner = (env.key_of)(EntityRef::Edge(e));
        if let Some(c) = b.edge_mut(e)?.curve.as_mut() {
            c.set_owner(Some(owner), None);
        }
        b.set_geometry_owner_key(EntityRef::Edge(e), Some(owner));
        Ok(())
    })
}

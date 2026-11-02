//! Offset faces whose loops cross one another, rebuilt before the piecewise merge.
//!
//! Upstream: none at the pinned commit; ported from the SMLib fork (source fork,
//! "SMLib: an offset face whose loops cross is rebuilt from the stretches that bound
//! it", `source/SMLib/src/SmOffsetExecutive.cpp`), source issue #854, WR-345.
//!
//! | Rust                                         | Fork                                           |
//! |----------------------------------------------|------------------------------------------------|
//! | [`OffsetExecutive::resolve_crossing_loops`]  | `SmOffsetExecutive::ResolveCrossingLoops`       |
//! | `resolve_crossing_loops_of_face` (private)   | `SmOffsetExecutive::ResolveCrossingLoopsOfFace` |
//! | `LoopStretch` (private)                      | `sm_LoopStretch`                               |
//! | `unresolved` (private)                       | `sm_UnresolvedCrossing`                        |
//! | `walls_touch` (private)                      | `sm_WallsTouch` (a759622)                      |
//! | `loop_directions_at` (private)               | `sm_LoopDirectionsAt` (a759622)                |
//! | `loop_touches_at` (private)                  | `sm_LoopTouchesAt` (a759622)                   |
//! | `LoopContact` (private)                      | `sm_LoopContact` (a759622)                     |
//!
//! Deviations: opt-in (`Kernel::fixes.offset_resolve_crossing_loops`, read by
//! `do_extended_offset`). The reason a crossing is not resolved is the error's
//! `operation` text instead of an `smos_ErrorMessage` report. `SmBrep::MakeFaceFromLoopOnFace`
//! is not ported: a face that comes apart is built with one `MakeFaceWithCurves` per
//! outer loop (each a sheet of its own, as the fork's `SplitDisconnectedShells` leaves
//! them), and one that comes apart and keeps holes as well is not resolved
//! (`SM_ERR_BAD_INTERSECTIONS`). No recursion; every loop is bounded by the face's
//! edge uses and crossings. The fork's later a759622 ("an offset whose walls would meet
//! is refused as such": every contact is examined before any crossing, and loops that
//! touch without crossing fail with `SM_ERR_OFFSET_WALLS_TOUCH`) is a second opt-in,
//! `Kernel::fixes.offset_walls_touch_refused`, read here; it has no effect without
//! `offset_resolve_crossing_loops`, which runs this routine at all. With it off the
//! routine is 6169ea1's (contacts checked as they are found, a touch reported as
//! whichever crossing check fails first, `SM_ERR_BAD_INTERSECTIONS`).

use super::p1::OffsetExecutive;
use super::p2::OffsetNotifications;
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::NotifyOperation;
use crate::geom::point_set::PointContainment;
use crate::geom::solution_array::SolutionType;
use crate::geom::tol::ANG_TOL_DEG;
use crate::geom::vector2d::Point2;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::query::r#loop::contains_uv_point;
use crate::surface::surface::p1::{map_to_2d, zone_tol_3d};
use crate::surface::surface::p4::NoObject;
use crate::topology::brep::p1 as bp1;
use crate::topology::brep::p3::{delete_face_with_body_identity, DeleteFaceOptions};
use crate::topology::brep::p7::{make_face_with_curves_with_kernel_context, FaceCurvesInput};
use crate::topology::edge::{self, NotificationStores, NotifyState};
use crate::topology::edgeuse::{p1 as eu, p3 as eu3};
use crate::topology::face::p2 as face;
use crate::topology::topology;
use crate::topology::topology_traverser::face_edges;
use crate::topology::{loopuse, vertexuse, Brep, EdgeId, EdgeuseId, FaceId, LoopId, LoopuseStart};

const OP: &str = "offset::offset_executive::crossing_loops::resolve_crossing_loops";

/// `SM_ERR_BAD_INTERSECTIONS`.
const SM_ERR_BAD_INTERSECTIONS: i32 = 1027;
use crate::error::SM_ERR_OFFSET_WALLS_TOUCH;
/// `SM_ERR_ASSERT_FAILURE`.
const SM_ERR_ASSERT_FAILURE: i32 = 1005;

/// A stretch of a loop of an offset face between two of its crossings with other loops
/// or the ends of its edge use, in the loop's direction (`sm_LoopStretch`).
#[derive(Debug, Clone)]
struct LoopStretch {
    /// `lLoop`: its loop, 0 the outer loop.
    loop_index: usize,
    /// `pEU`: the edge use (of the face's `SM_OT_SAME` face use) it is a stretch of.
    edgeuse: EdgeuseId,
    /// `pUV`: the index of that edge use's UV trim curve.
    uv: usize,
    /// `dLo`, `dHi`: its edge-parameter interval.
    lo: f64,
    hi: f64,
    /// `lStart`, `lEnd`: the nodes it starts and ends at, a crossing (`>= 0`) or the
    /// vertex ending edge use `k` of the face (`-1 - k`).
    start: i64,
    end: i64,
    /// `bKeep`: it bounds the face.
    keep: bool,
    /// `sMid`: its middle point.
    mid: Point3,
}

/// A crossing of the loops of an offset face that is not resolved
/// (`sm_UnresolvedCrossing`): the offset fails with `SM_ERR_BAD_INTERSECTIONS` rather
/// than merge a face whose loops cross; `why` names the configuration.
fn unresolved(why: &'static str) -> SolidError {
    SolidError::Upstream {
        operation: why,
        code: SM_ERR_BAD_INTERSECTIONS,
        entity: None,
    }
}

/// Loops of an offset face that touch without crossing (`sm_WallsTouch`, fork a759622):
/// the offset walls they come from meet there, so the offset would pinch to nothing
/// along a line (or over a stretch). No face is bounded there, and the offset is refused
/// with `SM_ERR_OFFSET_WALLS_TOUCH`; `why` names the configuration.
fn walls_touch(why: &'static str) -> SolidError {
    SolidError::Upstream {
        operation: why,
        code: SM_ERR_OFFSET_WALLS_TOUCH,
        entity: None,
    }
}

/// Where edge uses `a` and `b` of different loops meet (`sm_LoopContact`, fork a759622):
/// their indices and their edge parameters there.
#[derive(Debug, Clone, Copy)]
struct LoopContact {
    a: usize,
    b: usize,
    t_a: f64,
    t_b: f64,
}

/// The first derivative of `curve` at `t` (`Evaluate(t, 1, from_left, aEval)`, `aEval[1]`).
fn curve_tangent(k: &Kernel, curve: &Curve, t: f64, from_left: bool) -> Result<Vec3> {
    let eval = curve.evaluate(k, t, 1, from_left, true)?;
    Ok(*at(&eval, 1)?)
}

/// The directions in which a loop (its edge uses `loop_edgeuses`) leaves the point `p`
/// of its edge use `on` (at edge parameter `t`) (`sm_LoopDirectionsAt`, fork a759622):
/// both ways along `on`'s curve where `p` lies inside the edge; where `p` is a vertex of
/// the loop (within `tol_3d`), along each of the loop's edges from it.
fn loop_directions_at(
    k: &Kernel,
    b: &Brep,
    loop_edgeuses: &[EdgeuseId],
    on: EdgeuseId,
    t: f64,
    p: Point3,
    tol_3d: f64,
) -> Result<Vec<Vec3>> {
    let mut dirs = Vec::new();
    let on_edge = edge_of(b, on)?;
    if p.distance(vertex_point(b, edge::start_vertex(b, on_edge)?)?) > tol_3d
        && p.distance(vertex_point(b, edge::end_vertex(b, on_edge)?)?) > tol_3d
    {
        let d = curve_tangent(k, edge_curve(b, on_edge)?, t, true)?;
        dirs.push(d);
        dirs.push(-d);
        return Ok(dirs);
    }
    for &edgeuse in loop_edgeuses {
        let e = edge_of(b, edgeuse)?;
        let ivl = b.edge(e)?.interval;
        if p.distance(vertex_point(b, edge::start_vertex(b, e)?)?) <= tol_3d {
            dirs.push(curve_tangent(k, edge_curve(b, e)?, ivl.min(), true)?);
        }
        if p.distance(vertex_point(b, edge::end_vertex(b, e)?)?) <= tol_3d {
            dirs.push(-curve_tangent(k, edge_curve(b, e)?, ivl.max(), false)?);
        }
    }
    Ok(dirs)
}

/// Whether loop B touches loop A where they meet (`sm_LoopTouchesAt`, fork a759622;
/// `normal` the face's normal there, `dirs_a` and `dirs_b` the directions each leaves
/// that point in): it does when it leaves along one of A's directions (they are tangent
/// there, to SMLib's angle tolerance for parallel vectors), or when both its directions
/// lie on the same side of A, so that it comes up to A and goes back without passing
/// through it. A loop passing the point more than once is not told apart (`false`).
fn loop_touches_at(dirs_a: &[Vec3], dirs_b: &[Vec3], normal: Vec3) -> Result<bool> {
    let ([a0, a1], [b0, b1]) = (dirs_a, dirs_b) else {
        return Ok(false);
    };
    for &db in &[*b0, *b1] {
        for &da in &[*a0, *a1] {
            let (parallel, anti) = db.parallel_sense(da, ANG_TOL_DEG);
            if parallel && !anti.unwrap_or(false) {
                return Ok(true);
            }
        }
    }
    // The side of A from dirs_a[0] counterclockwise about the normal to dirs_a[1], and
    // where each of B's lies.
    let mut side = normal.ccw_angle_between(*a0, *a1)?;
    if side <= 0.0 {
        side += 2.0 * core::f64::consts::PI;
    }
    let mut on_side = [false; 2];
    for (slot, &db) in on_side.iter_mut().zip(&[*b0, *b1]) {
        let mut ang = normal.ccw_angle_between(*a0, db)?;
        if ang < 0.0 {
            ang += 2.0 * core::f64::consts::PI;
        }
        *slot = ang < side;
    }
    Ok(on_side[0] == on_side[1])
}

/// What [`cut_at_crossings_refusing_touch`] reads of `ResolveCrossingLoopsOfFace`'s
/// locals: the face's edge uses per loop, every edge use in loop order with its loop and
/// UV trim curve, where each loop's edge uses start, and the 3D and UV tolerances.
struct CrossingInputs<'a> {
    loop_edgeuses: &'a [Vec<EdgeuseId>],
    loop_of: &'a [usize],
    edgeuses: &'a [EdgeuseId],
    uvs: &'a [Curve],
    first_of_loop: &'a [usize],
    tol_3d: f64,
    uv_tol: f64,
}

/// The crossings of the loops of offset face `f` as fork a759622 finds them, cutting each
/// edge use's `cuts` there and returning their number: every contact of edge uses of
/// different loops is found first (in UV space, as `SmFace` test #38 finds them), loops
/// that touch at any of them are refused (`SM_ERR_OFFSET_WALLS_TOUCH`), then a crossing
/// at a loop vertex is refused (`SM_ERR_BAD_INTERSECTIONS`) and the rest cut the loops.
fn cut_at_crossings_refusing_touch(
    kernel: &Kernel,
    b: &Brep,
    f: FaceId,
    inp: &CrossingInputs<'_>,
    cuts: &mut [Vec<(f64, i64)>],
) -> Result<i64> {
    let (edgeuses, uvs, loop_of, tol_3d) = (inp.edgeuses, inp.uvs, inp.loop_of, inp.tol_3d);
    // Where edge uses of different loops meet, in UV space.
    let mut contacts: Vec<LoopContact> = Vec::new();
    for aa in 0..edgeuses.len() {
        let first_b = *at(inp.first_of_loop, *at(loop_of, aa)? + 1)?;
        for bb in first_b..edgeuses.len() {
            let (uv_a, uv_b) = (at(uvs, aa)?, at(uvs, bb)?);
            let solutions = uv_a.global_curve_intersect(
                kernel,
                &uv_a.natural_interval(),
                uv_b,
                &uv_b.natural_interval(),
                inp.uv_tol,
                &CurveIntersectOptions::<NoObject>::default(),
            )?;
            for solution in solutions.as_slice() {
                if solution.solution_type != SolutionType::SingleValue {
                    return Err(walls_touch(
                        "Offset face loops run along one another: the offset walls meet there",
                    ));
                }
                contacts.push(LoopContact {
                    a: aa,
                    b: bb,
                    t_a: solution.start.param(0)?,
                    t_b: solution.start.param(1)?,
                });
            }
        }
    }
    if contacts.is_empty() {
        return Ok(0);
    }

    // Loops that touch without crossing bound no face there, however the rest of it is
    // resolved.
    let surface = required(b.face(f)?.surface.as_ref())?;
    for c in &contacts {
        let (e_a, e_b) = (
            edge_of(b, *at(edgeuses, c.a)?)?,
            edge_of(b, *at(edgeuses, c.b)?)?,
        );
        let p_a = edge_curve(b, e_a)?.evaluate_point(kernel, c.t_a)?;
        let p_b = edge_curve(b, e_b)?.evaluate_point(kernel, c.t_b)?;
        let uv3 = at(uvs, c.a)?.evaluate_point(kernel, c.t_a)?;
        let normal =
            surface.evaluate_normal(kernel, Point2::new(uv3.x, uv3.y), Side::Left, Side::Left)?;
        let dirs_a = loop_directions_at(
            kernel,
            b,
            at(inp.loop_edgeuses, *at(loop_of, c.a)?)?,
            *at(edgeuses, c.a)?,
            c.t_a,
            p_a,
            tol_3d,
        )?;
        let dirs_b = loop_directions_at(
            kernel,
            b,
            at(inp.loop_edgeuses, *at(loop_of, c.b)?)?,
            *at(edgeuses, c.b)?,
            c.t_b,
            p_b,
            tol_3d,
        )?;
        if loop_touches_at(&dirs_a, &dirs_b, normal)? {
            return Err(walls_touch(
                "Offset face loops touch without crossing: the offset walls meet along a line",
            ));
        }
    }

    // The crossings: where each edge use is cut.
    let mut num_crossings: i64 = 0;
    for c in &contacts {
        let (e_a, e_b) = (
            edge_of(b, *at(edgeuses, c.a)?)?,
            edge_of(b, *at(edgeuses, c.b)?)?,
        );
        let p_a = edge_curve(b, e_a)?.evaluate_point(kernel, c.t_a)?;
        let p_b = edge_curve(b, e_b)?.evaluate_point(kernel, c.t_b)?;
        if p_a.distance(vertex_point(b, edge::start_vertex(b, e_a)?)?) <= tol_3d
            || p_a.distance(vertex_point(b, edge::end_vertex(b, e_a)?)?) <= tol_3d
            || p_b.distance(vertex_point(b, edge::start_vertex(b, e_b)?)?) <= tol_3d
            || p_b.distance(vertex_point(b, edge::end_vertex(b, e_b)?)?) <= tol_3d
        {
            return Err(unresolved("Offset face loops cross at a loop vertex"));
        }
        at_mut(cuts, c.a)?.push((c.t_a, num_crossings));
        at_mut(cuts, c.b)?.push((c.t_b, num_crossings));
        num_crossings += 1;
    }
    Ok(num_crossings)
}

fn assert_failure() -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code: SM_ERR_ASSERT_FAILURE,
        entity: None,
    }
}

fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| SolidError::null_pointer(OP))
}

fn at<T>(v: &[T], i: usize) -> Result<&T> {
    v.get(i).ok_or_else(|| SolidError::null_pointer(OP))
}

fn at_mut<T>(v: &mut [T], i: usize) -> Result<&mut T> {
    v.get_mut(i).ok_or_else(|| SolidError::null_pointer(OP))
}

fn index_i64(i: usize) -> Result<i64> {
    i64::try_from(i).map_err(|_| SolidError::null_pointer(OP))
}

/// The vertex node of edge use `k`: `-1 - k`.
fn vertex_node(k: usize) -> Result<i64> {
    Ok(-1 - index_i64(k)?)
}

fn edge_of(b: &Brep, edgeuse: EdgeuseId) -> Result<EdgeId> {
    required(eu::edge(b, edgeuse)?)
}

fn edge_curve(b: &Brep, e: EdgeId) -> Result<&Curve> {
    required(b.edge(e)?.curve.as_ref())
}

fn vertex_point(b: &Brep, v: Option<crate::topology::VertexId>) -> Result<Point3> {
    Ok(b.vertex(required(v)?)?.point)
}

/// `SmEdge::ClosestPoint`: `(found, distance)` of the nearest point of the edge's curve
/// over its interval (a minimising global point solve at the edge's tolerance).
fn edge_closest_distance(k: &Kernel, b: &Brep, e: EdgeId, point: Point3) -> Result<Option<f64>> {
    let record = b.edge(e)?;
    let curve = required(record.curve.as_ref())?;
    let solutions = curve.global_point_solve(
        k,
        &record.interval,
        SolverOperation::Minimize,
        point,
        record.tolerance,
        SolutionRequested::All,
        &GlobalPointSolveOptions::<NoObject>::default(),
    )?;
    Ok(solutions
        .as_slice()
        .first()
        .map(|solution| solution.start.solution_value))
}

impl OffsetExecutive {
    /// `ResolveCrossingLoops` (fork, not upstream): rebuilds every offset face whose
    /// loops cross one another. Each loop of an offset face is made of the offsets of
    /// its edges and is trimmed only where its own consecutive edges meet; nothing trims
    /// one loop of a face against another, so an inner loop whose offset grows past the
    /// outer loop's crosses it. Such a face is not valid (`SmFace` test #38) and the
    /// merge cannot use it.
    ///
    /// # Errors
    ///
    /// `SM_ERR_BAD_INTERSECTIONS` for crossings this does not resolve (see
    /// `resolve_crossing_loops_of_face`); with `Kernel::fixes.offset_walls_touch_refused`,
    /// `SM_ERR_OFFSET_WALLS_TOUCH` for loops that touch without crossing; those of the
    /// evaluations, intersections and face construction.
    pub(super) fn resolve_crossing_loops(
        &mut self,
        kernel: &mut Kernel,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        let mut offset = required(self.take_offset_brep())?;
        let status = bp1::faces(&offset, None).and_then(|faces| {
            faces.into_iter().try_for_each(|f| {
                self.resolve_crossing_loops_of_face(kernel, &mut offset, f, notifications)
            })
        });
        self.set_offset_brep(offset);
        status
    }

    /// `ResolveCrossingLoopsOfFace` (fork, not upstream): when the loops of offset face
    /// `f` cross one another, replaces it with the face(s) of its surface inside the
    /// outer loop and outside every inner one, bounded by the stretches of the loops
    /// between their crossings.
    ///
    /// The crossings are found in the face's UV space as `SmFace` test #38 finds them;
    /// every loop is cut there. A stretch of the outer loop bounds the face where it lies
    /// outside every inner loop, a stretch of an inner loop where it lies inside the
    /// outer loop and outside the other inner ones (`SmLoop::ContainsUVPoint` at its
    /// middle). The kept stretches, in their loops' directions, close into loops at the
    /// crossings; a new loop holding a stretch of the old outer loop bounds the face
    /// from outside. The face is rebuilt from each stretch's edge curve and UV trim
    /// curve trimmed to it, on a copy of its surface; the new faces, edges and vertices
    /// take the attributes and offset-map places of those they come from.
    #[allow(clippy::too_many_lines)] // one fork routine
    fn resolve_crossing_loops_of_face(
        &mut self,
        kernel: &mut Kernel,
        b: &mut Brep,
        f: FaceId,
        notifications: &mut OffsetNotifications<'_>,
    ) -> Result<()> {
        // (the face's tolerance model: SmTol::GetZoneTol3d reads the surface's face)
        let (zone_tol, uv_tol) = {
            let surface = required(b.face(f)?.surface.as_ref())?;
            let zone_tol = zone_tol_3d(kernel, surface)?;
            let uv_mid = surface.natural_uv_domain_with_nurb(kernel)?.mid();
            (zone_tol, map_to_2d(kernel, zone_tol, uv_mid, surface)?)
        };
        let mut fu = required(face::upward_faceuse(b, f)?)?;
        if eu::faceuse_orientation(b, fu)? != OrientType::Same {
            fu = required(eu::faceuse_mate(b, fu)?)?;
        }

        // The face's edge loops (the outer loop first) and the points of its vertex loops.
        let mut loops: Vec<LoopId> = Vec::new();
        let mut loop_edgeuses: Vec<Vec<EdgeuseId>> = Vec::new();
        let mut loop_points: Vec<Point3> = Vec::new();
        for lu in eu::faceuse_loopuses(b, fu, false, None)? {
            if let LoopuseStart::Vertexuse(_) = b.loopuse(lu)?.start {
                let vus = loopuse::vertexuses(b, lu, None)?;
                let vu = required(vus.first().copied().flatten())?;
                loop_points.push(vertex_point(b, vertexuse::vertex(b, vu)?)?);
                continue;
            }
            loops.push(required(b.loopuse(lu)?.loop_)?);
            loop_edgeuses.push(eu::loopuse_edgeuses(b, lu)?);
        }
        if loops.len() < 2 {
            return Ok(());
        }

        // Every edge use, in loop order, with its UV trim curve.
        let mut loop_of: Vec<usize> = Vec::new();
        let mut edgeuses: Vec<EdgeuseId> = Vec::new();
        let mut uvs: Vec<Curve> = Vec::new();
        let mut first_of_loop: Vec<usize> = Vec::new();
        let mut tol_3d = self.approximation_tol_3d();
        for (ll, list) in loop_edgeuses.iter().enumerate() {
            first_of_loop.push(edgeuses.len());
            for &edgeuse in list {
                eu3::get_or_create_uv_trim_curve(kernel, b, edgeuse, false, false, true)?.status?;
                let uv = required(eu::uv_trim_curve(b, edgeuse)?.0.cloned())?;
                let e = edge_of(b, edgeuse)?;
                if !uv.natural_interval().are_equal(&b.edge(e)?.interval, 0.0) {
                    return Err(assert_failure());
                }
                loop_of.push(ll);
                edgeuses.push(edgeuse);
                uvs.push(uv);
                tol_3d = tol_3d.max(b.edge(e)?.tolerance);
            }
        }
        first_of_loop.push(edgeuses.len());
        tol_3d *= 10.0;

        // The crossings of edge uses of different loops, in UV space (as SmFace test
        // #38 finds them).
        // per edge use: (parameter, crossing)
        let mut cuts: Vec<Vec<(f64, i64)>> = vec![Vec::new(); edgeuses.len()];
        let mut num_crossings: i64 = 0;
        if kernel.fixes.offset_walls_touch_refused {
            // Fork a759622: every contact is looked at before any crossing is resolved
            // or refused.
            num_crossings = cut_at_crossings_refusing_touch(
                kernel,
                b,
                f,
                &CrossingInputs {
                    loop_edgeuses: &loop_edgeuses,
                    loop_of: &loop_of,
                    edgeuses: &edgeuses,
                    uvs: &uvs,
                    first_of_loop: &first_of_loop,
                    tol_3d,
                    uv_tol,
                },
                &mut cuts,
            )?;
        } else {
            for aa in 0..edgeuses.len() {
                let first_b = *at(&first_of_loop, *at(&loop_of, aa)? + 1)?;
                for bb in first_b..edgeuses.len() {
                    let (uv_a, uv_b) = (at(&uvs, aa)?, at(&uvs, bb)?);
                    let solutions = uv_a.global_curve_intersect(
                        kernel,
                        &uv_a.natural_interval(),
                        uv_b,
                        &uv_b.natural_interval(),
                        uv_tol,
                        &CurveIntersectOptions::<NoObject>::default(),
                    )?;
                    for solution in solutions.as_slice() {
                        if solution.solution_type != SolutionType::SingleValue {
                            return Err(unresolved("Offset face loops run along one another"));
                        }
                        let t_a = solution.start.param(0)?;
                        let t_b = solution.start.param(1)?;
                        let e_a = edge_of(b, *at(&edgeuses, aa)?)?;
                        let e_b = edge_of(b, *at(&edgeuses, bb)?)?;
                        let p_a = edge_curve(b, e_a)?.evaluate_point(kernel, t_a)?;
                        let p_b = edge_curve(b, e_b)?.evaluate_point(kernel, t_b)?;
                        if p_a.distance(vertex_point(b, edge::start_vertex(b, e_a)?)?) <= tol_3d
                            || p_a.distance(vertex_point(b, edge::end_vertex(b, e_a)?)?) <= tol_3d
                            || p_b.distance(vertex_point(b, edge::start_vertex(b, e_b)?)?) <= tol_3d
                            || p_b.distance(vertex_point(b, edge::end_vertex(b, e_b)?)?) <= tol_3d
                        {
                            return Err(unresolved("Offset face loops meet at a loop vertex"));
                        }
                        at_mut(&mut cuts, aa)?.push((t_a, num_crossings));
                        at_mut(&mut cuts, bb)?.push((t_b, num_crossings));
                        num_crossings += 1;
                    }
                }
            }
        }
        if num_crossings == 0 {
            return Ok(());
        }

        // Until the merge every offset face is a sheet of its own: its edges bound it
        // alone.
        for &edgeuse in &edgeuses {
            if !eu::edge_is_lamina(b, edge_of(b, edgeuse)?)? {
                return Err(unresolved(
                    "Offset face whose loops cross shares an edge with another face",
                ));
            }
        }

        // Cut every edge use at its crossings into stretches, in the loop's direction.
        let mut stretches: Vec<LoopStretch> = Vec::new();
        for (gg, &edgeuse) in edgeuses.iter().enumerate() {
            let e = edge_of(b, edgeuse)?;
            let same = eu::orientation(b, edgeuse)? == OrientType::Same;
            let ivl = b.edge(e)?.interval;
            let edge_cuts = at_mut(&mut cuts, gg)?;
            edge_cuts.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            if !same {
                edge_cuts.reverse();
            }
            let edge_cuts = edge_cuts.clone();
            let loop_index = *at(&loop_of, gg)?;
            let first = *at(&first_of_loop, loop_index)?;
            let prev = if gg == first {
                *at(&first_of_loop, loop_index + 1)? - 1
            } else {
                gg - 1
            };
            let mut from = if same { ivl.min() } else { ivl.max() };
            let mut from_node = vertex_node(prev)?;
            for cc in 0..=edge_cuts.len() {
                let (to, to_node) = match edge_cuts.get(cc) {
                    Some(&(t, crossing)) => (t, crossing),
                    None => (if same { ivl.max() } else { ivl.min() }, vertex_node(gg)?),
                };
                let lo = from.min(to);
                let hi = from.max(to);
                let curve = edge_curve(b, e)?;
                let from_pt = curve.evaluate_point(kernel, lo)?;
                let to_pt = curve.evaluate_point(kernel, hi)?;
                if hi.partial_cmp(&lo) != Some(core::cmp::Ordering::Greater)
                    || from_pt.distance(to_pt) <= tol_3d
                {
                    return Err(unresolved("Three offset face loops cross at one point"));
                }
                let mid = curve.evaluate_point(kernel, 0.5 * (lo + hi))?;
                stretches.push(LoopStretch {
                    loop_index,
                    edgeuse,
                    uv: gg,
                    lo,
                    hi,
                    start: from_node,
                    end: to_node,
                    keep: false,
                    mid,
                });
                from = to;
                from_node = to_node;
            }
        }

        // Which stretches bound the face: of the outer loop, those outside every inner
        // loop; of an inner loop, those inside the outer loop and outside the other
        // inner ones.
        for ss in 0..stretches.len() {
            let stretch = at(&stretches, ss)?;
            let uv3 =
                at(&uvs, stretch.uv)?.evaluate_point(kernel, 0.5 * (stretch.lo + stretch.hi))?;
            let uv = Point2::new(uv3.x, uv3.y);
            let own_loop = stretch.loop_index;
            let mut keep = true;
            for (ll, &l) in loops.iter().enumerate() {
                if ll == own_loop {
                    continue;
                }
                let found = contains_uv_point(kernel, b, l, None, uv, zone_tol)?.containment;
                if found != PointContainment::Inside && found != PointContainment::Outside {
                    return Err(unresolved(
                        "Offset face loop stretch is neither inside nor outside another loop",
                    ));
                }
                let wanted = if ll == 0 {
                    PointContainment::Inside
                } else {
                    PointContainment::Outside
                };
                if found != wanted {
                    keep = false;
                }
            }
            at_mut(&mut stretches, ss)?.keep = keep;
        }

        // Close the kept stretches into loops: each goes on to the one kept stretch that
        // starts where it ends (at a crossing, the stretch of the other loop that bounds
        // the face).
        let mut next: Vec<Option<usize>> = vec![None; stretches.len()];
        let mut prev_count: Vec<usize> = vec![0; stretches.len()];
        for (ss, stretch) in stretches.iter().enumerate() {
            if !stretch.keep {
                continue;
            }
            for (tt, other) in stretches.iter().enumerate() {
                if !other.keep || other.start != stretch.end {
                    continue;
                }
                if at(&next, ss)?.is_some() {
                    return Err(unresolved(
                        "Offset face loops leave a crossing more than one way",
                    ));
                }
                *at_mut(&mut next, ss)? = Some(tt);
                *at_mut(&mut prev_count, tt)? += 1;
            }
            if at(&next, ss)?.is_none() {
                return Err(unresolved("Offset face loops leave a crossing no way"));
            }
        }
        let mut new_loops: Vec<Vec<usize>> = Vec::new();
        let mut new_loop_is_outer: Vec<bool> = Vec::new();
        let mut done: Vec<bool> = vec![false; stretches.len()];
        for ss in 0..stretches.len() {
            if !at(&stretches, ss)?.keep || *at(&done, ss)? {
                continue;
            }
            let mut chain = Vec::new();
            let mut outer = false;
            let mut tt = ss;
            while !*at(&done, tt)? {
                if *at(&prev_count, tt)? != 1 {
                    return Err(unresolved(
                        "Offset face loops arrive at a crossing more than one way",
                    ));
                }
                *at_mut(&mut done, tt)? = true;
                chain.push(tt);
                if at(&stretches, tt)?.loop_index == 0 {
                    outer = true;
                }
                tt = required(*at(&next, tt)?)?;
            }
            new_loops.push(chain);
            new_loop_is_outer.push(outer);
        }

        // Outer loops first (MakeFaceWithCurves takes the first loop for the outer one).
        let outer_loops: Vec<usize> = (0..new_loops.len())
            .filter(|&ll| new_loop_is_outer.get(ll).copied().unwrap_or(false))
            .collect();
        let hole_loops: Vec<usize> = (0..new_loops.len())
            .filter(|&ll| !new_loop_is_outer.get(ll).copied().unwrap_or(false))
            .collect();
        // (deviation: SmBrep::MakeFaceFromLoopOnFace is not ported, so a face that
        // comes apart is made one face per outer loop, and only without holes)
        if outer_loops.len() > 1 && !hole_loops.is_empty() {
            return Err(unresolved(
                "Offset face whose loops cross comes apart around a hole",
            ));
        }
        let face_loops: Vec<Vec<usize>> = if outer_loops.len() > 1 {
            outer_loops.iter().map(|&ll| vec![ll]).collect()
        } else if outer_loops.is_empty() {
            Vec::new()
        } else {
            vec![outer_loops.iter().chain(&hole_loops).copied().collect()]
        };

        // The face(s) the old one becomes; none when no stretch of its outer loop bounds
        // it.
        let uv_domain = b.face(f)?.uv_domain;
        let mut new_faces: Vec<FaceId> = Vec::new();
        for order in &face_loops {
            let mut input = FaceCurvesInput {
                loop_points: if new_faces.is_empty() {
                    loop_points.clone()
                } else {
                    Vec::new()
                },
                ..FaceCurvesInput::default()
            };
            let mut curves_3d = Vec::new();
            let mut uv_curves = Vec::new();
            for &ll in order {
                let chain = at(&new_loops, ll)?;
                input.loops.push(chain.len());
                for &ss in chain {
                    let stretch = at(&stretches, ss)?;
                    let e = edge_of(b, stretch.edgeuse)?;
                    let mut curve_3d = edge_curve(b, e)?.copy(kernel)?;
                    let mut ivl_3d = Extent1d::from_min_max(stretch.lo, stretch.hi);
                    curve_3d.trim(kernel, &mut ivl_3d, TrimMode::Notify)?;
                    curves_3d.push(Some(curve_3d));
                    let mut uv_curve = at(&uvs, stretch.uv)?.copy(kernel)?;
                    let mut ivl_uv = Extent1d::from_min_max(stretch.lo, stretch.hi);
                    uv_curve.trim(kernel, &mut ivl_uv, TrimMode::Notify)?;
                    uv_curves.push(uv_curve);
                    input
                        .orientations
                        .push(eu::orientation(b, stretch.edgeuse)?);
                }
            }
            input.curves_3d = Some(curves_3d);
            input.uv_curves = Some(uv_curves);
            let new_surface = required(b.face(f)?.surface.as_ref())?.copy(kernel)?;
            let region = b.infinite_region();
            let key_of = notifications.result_key_of;
            let mut stores = NotificationStores {
                records: &mut *notifications.records,
                attributes: &mut *notifications.attributes,
                key_of,
            };
            let made = make_face_with_curves_with_kernel_context(
                kernel,
                b,
                region,
                input,
                new_surface,
                &uv_domain,
                OrientType::Same,
                None,
                &|face| key_of(EntityRef::Face(face)),
                &mut stores,
            )?;
            new_faces.push(required(made.face)?);
        }

        // The new faces, edges and vertices take the attributes of the face, edges and
        // vertices they come from, and the old edge uses' places in the edge use map go
        // to the new ones.
        let key_of = notifications.result_key_of;
        for &new_face in &new_faces {
            let mut stores = NotificationStores {
                records: &mut *notifications.records,
                attributes: &mut *notifications.attributes,
                key_of,
            };
            NotifyState::from_kernel_stores(&mut stores).with_kernel_env(kernel, |_, env| {
                topology::notify(
                    env.records,
                    env.attributes,
                    env.context.as_deref_mut(),
                    NotifyOperation::Copy,
                    key_of(EntityRef::Face(f)),
                    Some(key_of(EntityRef::Face(new_face))),
                    None,
                    None,
                )
            })?;
            for new_edge in face_edges(b, new_face)? {
                // the stretch whose middle lies on it
                let mut from = None;
                for stretch in stretches.iter().filter(|s| s.keep) {
                    if let Some(dist) = edge_closest_distance(kernel, b, new_edge, stretch.mid)? {
                        if dist <= tol_3d {
                            from = Some(stretch.edgeuse);
                            break;
                        }
                    }
                }
                let old_eu = required(from)?;
                let old_edge = edge_of(b, old_eu)?;
                let new_vertices = [
                    edge::start_vertex(b, new_edge)?,
                    edge::end_vertex(b, new_edge)?,
                ];
                let old_vertices = [
                    edge::start_vertex(b, old_edge)?,
                    edge::end_vertex(b, old_edge)?,
                ];
                let mut pairs = vec![(EntityRef::Edge(old_edge), EntityRef::Edge(new_edge))];
                for &new_v in &new_vertices {
                    for &old_v in &old_vertices {
                        if vertex_point(b, new_v)?.distance(vertex_point(b, old_v)?) <= tol_3d {
                            pairs.push((
                                EntityRef::Vertex(required(old_v)?),
                                EntityRef::Vertex(required(new_v)?),
                            ));
                        }
                    }
                }
                let mut stores = NotificationStores {
                    records: &mut *notifications.records,
                    attributes: &mut *notifications.attributes,
                    key_of,
                };
                NotifyState::from_kernel_stores(&mut stores).with_kernel_env(
                    kernel,
                    |_, env| {
                        for &(old, new) in &pairs {
                            topology::notify(
                                env.records,
                                env.attributes,
                                env.context.as_deref_mut(),
                                NotifyOperation::Copy,
                                key_of(old),
                                Some(key_of(new)),
                                None,
                                None,
                            )?;
                        }
                        Ok(())
                    },
                )?;
                let new_eus = b.edge(new_edge)?.edgeuses.clone();
                for old in [Some(old_eu), eu::mate(b, old_eu)?] {
                    let old = required(old)?;
                    let old_orientation =
                        eu::faceuse_orientation(b, required(eu::faceuse(b, old)?)?)?;
                    let originals = self.edgeuse_to_edgeuses.firsts(old);
                    for &new_eu in &new_eus {
                        let new_fu = required(eu::faceuse(b, new_eu)?)?;
                        if eu::faceuse_orientation(b, new_fu)? != old_orientation {
                            continue;
                        }
                        for &original in &originals {
                            self.edgeuse_to_edgeuses.relate_pair(original, new_eu)?;
                        }
                    }
                }
            }
        }

        // The new faces take the old one's places in the face maps; the old face, its
        // edge uses and their pairs go.
        for original in self.faceuse_to_faces.firsts(f) {
            self.faceuse_to_faces.remove_pair(original, f)?;
            for &new_face in &new_faces {
                self.faceuse_to_faces.relate_pair(original, new_face)?;
            }
        }
        for original in self.edge_to_faces.firsts(f) {
            self.edge_to_faces.remove_pair(original, f)?;
            for &new_face in &new_faces {
                self.edge_to_faces.relate_pair(original, new_face)?;
            }
        }
        for original in self.vertex_to_faces.firsts(f) {
            self.vertex_to_faces.remove_pair(original, f)?;
            for &new_face in &new_faces {
                self.vertex_to_faces.relate_pair(original, new_face)?;
            }
        }
        for blended in self.blend_to_faces.seconds(f) {
            self.blend_to_faces.remove_pair(f, blended)?;
            for &new_face in &new_faces {
                self.blend_to_faces.relate_pair(new_face, blended)?;
            }
        }
        for blend in self.blend_to_faces.firsts(f) {
            self.blend_to_faces.remove_pair(blend, f)?;
            for &new_face in &new_faces {
                self.blend_to_faces.relate_pair(blend, new_face)?;
            }
        }
        for &edgeuse in &edgeuses {
            for old in [Some(edgeuse), eu::mate(b, edgeuse)?] {
                let old = required(old)?;
                for original in self.edgeuse_to_edgeuses.firsts(old) {
                    self.edgeuse_to_edgeuses.remove_pair(original, old)?;
                }
            }
        }
        let body = notifications.result_brep;
        let mut stores = NotificationStores {
            records: &mut *notifications.records,
            attributes: &mut *notifications.attributes,
            key_of,
        };
        NotifyState::from_kernel_stores(&mut stores).with_kernel_env(kernel, |kernel, env| {
            delete_face_with_body_identity(
                kernel,
                b,
                env,
                f,
                DeleteFaceOptions {
                    delete_edges_vertices: true,
                    do_region_nesting: true,
                    region_to_keep: None,
                },
                body,
            )
        })?;
        Ok(())
    }
}

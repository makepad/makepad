//! Sweeps of a planar profile along a planar path of several curves.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_sweep_along_planar_path`] | eight-argument static `CreateSweepAlongPlanarPath` |
//! | `sweep_profiles_along_path` (private) | `SweepProfilesAlongPath` |
//!
//! Deviations: the static function takes the access and the B-rep whose context
//! upstream's `crContext` is; it returns the new B-rep. A path curve that is not a
//! B-spline (an invalid C cast upstream, UB-113) is an error. Statuses upstream
//! ignores keep the previous values of their outputs, as its reused locals do; a
//! cutting curve that was never built (a null dereference upstream) is an error.
//! `CreateExtendedCurve`'s end flag 1 is the start and 0 or 2 the end, as in NLib.
//! B-reps of a failed sweep stay with the access, as upstream leaves them. Not upstream
//! (WR-400, source issue #837, opt-in `Kernel::fixes.planar_path_uncapped_open_profiles`):
//! `SweepProfilesAlongPath` asks for caps only for a profile that closes. No recursion.

use super::{as_bspline, PrimitiveCreationAccess};
use crate::curve::curve::p9::CurveSolveOptions;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::kernel::Kernel;
use crate::math::{rad_to_deg, Vec3, BIG_DOUBLE};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::crv_adv::p1::CurveEnd;
use crate::primitive::primitive_creation::p1::{
    extend_path_curves, position_profile_curves, PrimitiveCreation,
};
use crate::primitive::primitive_creation::SweepOptions;
use crate::topology::brep::p10::TrimType;
use crate::topology::BrepId;

const OP: &str = "primitive::primitive_creation::p2::planar_path";

/// The solver handles of the cutting curve (0) and an intersection curve (1).
#[derive(Debug, Clone, Copy, PartialEq)]
struct PathCurve(u8, Extent1d);
impl SolutionObject for PathCurve {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.1)
    }
}

/// `Evaluate(t, 1, from_left, out)` (non-zero tangents) whose ignored status keeps
/// the previous position and tangent.
fn evaluate_keep(
    k: &Kernel,
    curve: &Curve,
    t: f64,
    from_left: bool,
    out: &mut [Vec3; 2],
) -> Result<()> {
    if let Some(values) = ignored(curve.evaluate(k, t, 1, from_left, true))? {
        for (target, value) in out.iter_mut().zip(values) {
            *target = value;
        }
    }
    Ok(())
}

/// `CreateExtendedCurve(dist, end, SM_CT_G1, out)` whose ignored status leaves `out`.
fn extend_into(
    curve: &BSplineCurve,
    distance: f64,
    end: CurveEnd,
    out: &mut Option<BSplineCurve>,
) -> Result<()> {
    if let Some(Some(next)) =
        ignored(curve.create_extended_curve_by_distance(distance, end, ContinuityType::G1))?
    {
        *out = Some(next);
    }
    Ok(())
}

/// `CreateLineSegment(3, start, end, out)` whose ignored status leaves `out`.
fn line_into(start: Vec3, end: Vec3, out: &mut Option<BSplineCurve>) -> Result<()> {
    if let Some(line) = ignored(BSplineCurve::create_line_segment(3, start, end))? {
        *out = Some(line);
    }
    Ok(())
}

/// Not upstream (WR-400, source issue #837): whether the profile curves
/// close, every curve end meeting another curve end (a closed curve's own other end)
/// within `tol`.
fn profile_closes(k: &Kernel, profiles: &[Curve], tol: f64) -> Result<bool> {
    let mut ends = Vec::new();
    for curve in profiles {
        let domain = curve.natural_interval();
        ends.push(curve.evaluate_point(k, domain.min())?);
        ends.push(curve.evaluate_point(k, domain.max())?);
    }
    Ok(ends.iter().enumerate().all(|(i, end)| {
        ends.iter()
            .enumerate()
            .any(|(j, other)| j != i && end.distance(*other) < tol)
    }))
}

/// Whether `SweepProfilesAlongPath` asks `CreateCurveSweep` for caps: always upstream.
/// With `Kernel::fixes.planar_path_uncapped_open_profiles` (not upstream, WR-400,
/// source issue #837) only for a profile that closes: an open profile bounds
/// no planar region, its cap fails and `CreateCurveSweep` returns before stitching its
/// walls (a failure upstream ignores).
fn cap_profile_ends(k: &Kernel, profiles: &[Curve], tol: f64) -> Result<bool> {
    if !k.fixes.planar_path_uncapped_open_profiles {
        return Ok(true);
    }
    profile_closes(k, profiles, tol)
}

/// `SweepProfilesAlongPath`: sweep the positioned profiles along one path segment,
/// always capped (see [`cap_profile_ends`]), then remove the unwanted caps. The
/// sweep's and removals' statuses are ignored, as upstream (only `Unported` passes).
fn sweep_profiles_along_path(
    a: &mut impl PrimitiveCreationAccess,
    profiles: &[Curve],
    path: &BSplineCurve,
    tol: f64,
    cap_start: bool,
    cap_end: bool,
    move_profile_center_to_path: bool,
    brep: BrepId,
) -> Result<()> {
    let cap_ends = cap_profile_ends(a.kernel(brep)?, profiles, tol)?;

    // CreateCurveSweep somehow mangles the path curve
    let path = path.copy();

    let mut start_faces = Vec::new();
    let mut side_faces = Vec::new();
    let mut end_faces = Vec::new();
    let options = SweepOptions {
        cap_ends,
        translational_sweep: false,
        move_profile_center_to_path,
        orient_profile_perpendicular_to_path: false,
        move_to_profile: false,
        profile_at_path_start: false,
    };
    let region = a.infinite_region(brep)?;
    let mut primitive = PrimitiveCreation::new(Some(region), 0, false);
    let profiles: Vec<Option<Curve>> = profiles.iter().map(|c| Some(c.clone())).collect();
    ignore_status(primitive.create_curve_sweep(
        a,
        brep,
        &profiles,
        &path,
        None,
        None,
        tol,
        options,
        false,
        &mut start_faces,
        &mut side_faces,
        &mut end_faces,
    ))?;

    if !cap_start {
        a.set_editing(brep, true)?;
        ignore_status(a.remove_faces(brep, &start_faces))?;
    }
    if !cap_end {
        a.set_editing(brep, true)?;
        ignore_status(a.remove_faces(brep, &end_faces))?;
    }
    Ok(())
}

/// Reference points on either side of a cutting line, then trim the B-reps on both
/// sides (`ProjectAndTrim(..., SM_TT_DELETE_POINT, &dTol, NULL, NULL)`, ignored).
fn trim_both(
    a: &mut impl PrimitiveCreationAccess,
    breps: [BrepId; 2],
    cutting: &BSplineCurve,
    normal: Vec3,
    references: [Vec3; 2],
    tol: f64,
) -> Result<()> {
    let [before, after] = breps;
    let [reference1, reference2] = references;
    ignore_status(a.project_and_trim(
        before,
        cutting,
        normal,
        reference2,
        TrimType::DeletePoint,
        tol,
    ))?;
    ignore_status(a.project_and_trim(
        after,
        cutting,
        normal,
        reference1,
        TrimType::DeletePoint,
        tol,
    ))
}

impl PrimitiveCreation {
    /// Sweep the planar `profiles` along the ordered, connected planar `paths`
    /// (eight-argument static `CreateSweepAlongPlanarPath`): one capped sweep per
    /// path curve, mitred at tangent discontinuities, the pieces united, stitched
    /// and classified. `cap_start` / `cap_end` cap the ends of an open path; a closed
    /// path has no end caps. Returns the new B-rep, in `context_of`'s context.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for no path curves or a path curve that is not a B-spline;
    /// `SM_ERR` when the pieces do not unite into one B-rep, or for a manifold
    /// result without material; those of the union, the final stitch and the
    /// region classification (the result is deleted on those).
    pub fn create_sweep_along_planar_path(
        a: &mut impl PrimitiveCreationAccess,
        context_of: BrepId,
        profiles: &[Curve],
        paths: &[Curve],
        tol: f64,
        cap_start: bool,
        cap_end: bool,
        move_profile_center_to_path: bool,
    ) -> Result<BrepId> {
        let first_path = paths
            .first()
            .ok_or_else(|| SolidError::invalid_input(OP, "no path curves"))?;
        let last_path = paths
            .last()
            .ok_or_else(|| SolidError::invalid_input(OP, "no path curves"))?;
        let mut bspline_paths = Vec::with_capacity(paths.len());
        for path in paths {
            bspline_paths
                .push(as_bspline(path).ok_or_else(|| {
                    SolidError::invalid_input(OP, "path curve is not a B-spline")
                })?);
        }

        // Determine whether path curves form a closed and periodic loop.
        let mut start_pv = [Vec3::UNDEF; 2];
        let mut end_pv = [Vec3::UNDEF; 2];
        evaluate_keep(
            a.kernel(context_of)?,
            first_path,
            first_path.natural_interval().min(),
            true,
            &mut start_pv,
        )?;
        evaluate_keep(
            a.kernel(context_of)?,
            last_path,
            last_path.natural_interval().max(),
            true,
            &mut end_pv,
        )?;
        let [start_point, start_tangent] = start_pv;
        let [end_point, end_tangent] = end_pv;
        let closed = start_point.distance(end_point) < tol;
        let periodic = closed && start_tangent.is_parallel_to(end_tangent, 2.0);

        // Determine max size of profile
        let k = a.kernel(context_of)?;
        let mut total = Extent3d::new();
        let mut bounds = Extent3d::new();
        for profile in profiles {
            if let Some(boxes) = ignored(profile.calculate_bounding_box(
                k,
                &profile.natural_interval(),
                BoundingBoxRequest {
                    normal: true,
                    ..BoundingBoxRequest::default()
                },
                true,
            ))? {
                if let Some(normal) = boxes.normal {
                    bounds = normal;
                }
            }
            total.add_point(bounds.min());
            total.add_point(bounds.max());
        }
        let max_dimension = total.max_dimension();
        let mut extend = max_dimension * 2.0_f64.sqrt();
        extend *= 2.0;

        // Extend all the path curves at start and end to make sure sweeps go past corner
        let extended = extend_path_curves(&bspline_paths, closed, periodic, extend)?;

        // get the normal to the plane of the path
        let path_normal =
            ignored(crate::curve::curve::p3::compute_crvs_normal(k, paths))?.unwrap_or(Vec3::UNDEF);

        // Prepare for loop where we position profile curves, sweep, and trim
        let mut new_breps: Vec<BrepId> = Vec::new();
        let mut before_brep: Option<BrepId> = None;
        let mut first_brep: Option<BrepId> = None;
        let mut cut_line: Option<BSplineCurve> = None;
        let mut cutting: Option<BSplineCurve> = None;
        let mut temp_cutting: Option<BSplineCurve> = None;
        let mut before = [Vec3::UNDEF; 2];
        let mut after = [Vec3::UNDEF; 2];
        let mut first_pv = [Vec3::UNDEF; 2];
        let mut angle_between = 0.0;

        for (ii, (path, extended_path)) in paths.iter().zip(&extended).enumerate() {
            // find start point and derivative of new path segment
            let range = path.natural_interval();
            evaluate_keep(a.kernel(context_of)?, path, range.min(), false, &mut after)?;
            after[1] = after[1].unitize_or_z();

            // position profile curves at the start of the path segment
            let mut new_profiles = Vec::new();
            position_profile_curves(
                a.kernel(context_of)?,
                profiles,
                tol,
                &Curve::from(extended_path.clone()),
                &mut new_profiles,
            )?;

            // sweep the profiles along this path segment creating new brep
            let after_brep = a.new_brep(context_of)?;
            let cap_this_start = ii == 0 && !closed && cap_start;
            let cap_this_end = ii == paths.len() - 1 && !closed && cap_end;
            sweep_profiles_along_path(
                a,
                &new_profiles,
                extended_path,
                tol,
                cap_this_start,
                cap_this_end,
                move_profile_center_to_path,
                after_brep,
            )?;
            new_breps.push(after_brep);

            // The first segment only records its derivatives; later ones cut both
            // breps at the mitre.
            if ii == 0 {
                first_brep = Some(after_brep);
                first_pv = after;
            } else if !before[1].is_parallel_to(after[1], 2.0) {
                // Average the derivatives to determine cutting plane normal.
                let mut average = (-before[1]) + after[1];
                average = average.unitize_or_z();

                // An inner corner flips the average direction to extend correctly.
                if let Some(angle) = ignored(path_normal.ccw_angle_between(-before[1], after[1]))? {
                    angle_between = angle;
                }
                angle_between = rad_to_deg(angle_between);
                if angle_between > 0.0 {
                    average = -average;
                }

                // Create temp cutting curve: the line from the corner along the
                // average derivative, then extended backwards.
                let cutting_point = after[0] + (average * extend);
                line_into(after[0], cutting_point, &mut cut_line)?;
                let line = cut_line
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                extend_into(line, extend, CurveEnd::Start, &mut temp_cutting)?;

                cutting = None;
                let previous = ii
                    .checked_sub(1)
                    .and_then(|i| paths.get(i))
                    .ok_or_else(|| SolidError::sm_err(OP))?;
                if path.is_linear(a.kernel(context_of)?, tol, false)?.linear
                    && previous
                        .is_linear(a.kernel(context_of)?, tol, false)?
                        .linear
                {
                    cutting.clone_from(&temp_cutting);
                } else {
                    // Intersect the temp cutting curve with every intersection curve
                    // and build the real cutting curve through the two best points.
                    let before_id = before_brep.ok_or_else(|| SolidError::null_pointer(OP))?;
                    // `pBrepBefore->BrepIntersect(crContext, pBrepAfter, ..)`: the faces'
                    // caches, UV trim curves and classifications are made on their own
                    // B-reps (`after_brep` is of `context_of`'s context)
                    let intersections = ignored(a.with_breps_mut(
                        before_id,
                        after_brep,
                        |k, before, after, _, _, _, _| {
                            crate::topology::brep::p1::brep_intersect_mut(
                                k, before, after, tol, 2.0, false,
                            )
                        },
                    ))?
                    .map(|(curves, _)| curves)
                    .unwrap_or_default();
                    if intersections.len() > 1 {
                        let temp = temp_cutting
                            .as_ref()
                            .ok_or_else(|| SolidError::null_pointer(OP))?;
                        let temp = Curve::from(temp.clone());
                        let mut best = Vec3::UNDEF;
                        let mut best2 = Vec3::UNDEF;
                        let mut best_dist = BIG_DOUBLE;
                        let mut best_dist2 = BIG_DOUBLE;
                        for intersection in &intersections {
                            let options = CurveSolveOptions {
                                target_distance: None,
                                opt_vectors: None,
                                handles: [
                                    Some(PathCurve(0, temp.natural_interval())),
                                    Some(PathCurve(1, intersection.natural_interval())),
                                ],
                            };
                            let solutions = ignored(temp.global_curve_solve(
                                a.kernel(context_of)?,
                                &temp.natural_interval(),
                                intersection,
                                &intersection.natural_interval(),
                                SolverOperation::Minimize,
                                tol,
                                &options,
                                SolutionRequested::Single,
                            ))?;
                            let Some(solutions) = solutions else {
                                continue;
                            };
                            for index in 0..solutions.len() {
                                let solution = solutions.get_at(index)?;
                                let on_line = match solution.start.param(0) {
                                    Ok(t) => {
                                        ignored(temp.evaluate_point(a.kernel(context_of)?, t))?
                                            .unwrap_or(Vec3::UNDEF)
                                    }
                                    Err(_) => Vec3::UNDEF,
                                };
                                let on_intersection = match solution.start.param(1) {
                                    Ok(t) => ignored(
                                        intersection.evaluate_point(a.kernel(context_of)?, t),
                                    )?
                                    .unwrap_or(Vec3::UNDEF),
                                    Err(_) => Vec3::UNDEF,
                                };
                                let dist = on_intersection.distance(on_line);
                                // keep track of the best two solutions
                                if dist < best_dist {
                                    if best.is_initialized() {
                                        best2 = best;
                                    }
                                    best_dist2 = best_dist;
                                    best = on_intersection;
                                    best_dist = dist;
                                } else if dist < best_dist2 {
                                    best2 = on_intersection;
                                    best_dist2 = dist;
                                }
                            }
                        }
                        if best.is_initialized()
                            && best2.is_initialized()
                            && best.distance(best2) > tol * 10.0
                        {
                            let mut line = None;
                            line_into(best, best2, &mut line)?;
                            let mut line = line.ok_or_else(|| SolidError::null_pointer(OP))?;
                            let mut extended_line = None;
                            extend_into(&line, extend, CurveEnd::End, &mut extended_line)?;
                            if let Some(next) = extended_line {
                                line = next;
                            }
                            extend_into(&line, extend, CurveEnd::Start, &mut cutting)?;
                        }
                    }
                }

                // Just as a backup: this should still work.
                if cutting.is_none() {
                    cutting.clone_from(&temp_cutting);
                }
                if let Some(curve) = cutting.as_ref() {
                    // determine reference points on either side of the cutting line
                    let reference1 = before[0] + (-before[1] * 0.1);
                    let reference2 = after[0] + (after[1] * 0.1);
                    let before_id = before_brep.ok_or_else(|| SolidError::null_pointer(OP))?;
                    trim_both(
                        a,
                        [before_id, after_brep],
                        curve,
                        path_normal,
                        [reference1, reference2],
                        tol,
                    )?;
                }
            }

            // Get ready for next path segment
            before_brep = Some(after_brep);

            // find end derivative of path before
            evaluate_keep(a.kernel(context_of)?, path, range.max(), true, &mut before)?;
            before[1] = before[1].unitize_or_z();

            // a closed path trims the end of the last and the beginning of the first
            if closed && ii == paths.len() - 1 {
                let after_id = first_brep.ok_or_else(|| SolidError::null_pointer(OP))?;
                after = first_pv;

                // tangent continuous paths need no trim
                if !before[1].is_parallel_to(after[1], 2.0) {
                    before[1] = before[1].unitize_or_z();
                    after[1] = after[1].unitize_or_z();
                    let mut average = (-before[1]) + after[1];
                    if let Some(angle) =
                        ignored(path_normal.ccw_angle_between(-before[1], after[1]))?
                    {
                        angle_between = angle;
                    }
                    angle_between = rad_to_deg(angle_between);
                    if angle_between > 0.0 {
                        average = -average;
                    }

                    // the cutting line runs 50 along the average and back
                    let cutting_point = after[0] + (average * 50.0);
                    line_into(after[0], cutting_point, &mut cut_line)?;
                    let line = cut_line
                        .as_ref()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    extend_into(line, 50.0, CurveEnd::Start, &mut cutting)?;
                    let curve = cutting
                        .as_ref()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;

                    let before_id = before_brep.ok_or_else(|| SolidError::null_pointer(OP))?;
                    let (reference1, reference2) = if before_id == after_id {
                        (
                            before[0] + (before[1] * 10.0),
                            after[0] + (-after[1] * 10.0),
                        )
                    } else {
                        (
                            before[0] + (-before[1] * 30.0),
                            after[0] + (after[1] * 30.0),
                        )
                    };
                    if before_id == after_id {
                        ignore_status(a.stitch_faces(before_id, 0.01))?;
                        ignore_status(a.project_and_trim(
                            before_id,
                            curve,
                            path_normal,
                            reference2,
                            TrimType::Split,
                            tol,
                        ))?;
                        let mut remove = Vec::new();
                        let brep = a.brep(before_id)?;
                        for edge in crate::topology::brep::p1::edges(brep, None)? {
                            if crate::topology::edge::is_lamina(brep, edge)? {
                                remove.extend(crate::topology::edge::faces(brep, edge, None)?);
                            }
                        }
                        ignore_status(a.remove_faces(before_id, &remove))?;
                        ignore_status(a.remove_topological_edges_and_vertices(before_id))?;
                    } else {
                        trim_both(
                            a,
                            [before_id, after_id],
                            curve,
                            path_normal,
                            [reference1, reference2],
                            tol,
                        )?;
                    }
                }
            }
        }

        // merge all results into one brep: the last first, then a union per piece
        let count = new_breps.len();
        let mut ordered = Vec::with_capacity(count);
        let mut tree: Vec<i64> = vec![0];
        ordered.extend(new_breps.pop());
        for ii in 1..count {
            ordered.extend(new_breps.pop());
            tree.push(i64::try_from(ii).map_err(|_| SolidError::sm_err(OP))?);
            // Convert to different form of Boolean numbering (0 - 1)
            tree.push(-1);
        }
        a.boolean_trees(&mut ordered, &tree)?;
        if ordered.len() != 1 {
            // Something went wrong
            return Err(SolidError::sm_err(OP));
        }
        let sweep = *ordered.first().ok_or_else(|| SolidError::sm_err(OP))?;

        if let Err(e) = a.stitch_faces(sweep, tol) {
            a.delete_brep(sweep);
            return Err(e);
        }
        ignore_status(a.remove_topological_edges_and_vertices(sweep))?;
        // A second attempt can remove additional edges
        ignore_status(a.remove_topological_edges_and_vertices(sweep))?;

        // Solid intent with a manifold boundary gets the nested-solid parity
        // classification.
        let solid_intent = closed || (cap_start && cap_end);
        let manifold = a.is_manifold_solid(sweep)?;
        if solid_intent && manifold {
            if let Err(e) = a.with_primitive_brep(sweep, |k, b| {
                crate::topology::brep::p1::set_region_is_void_flags_for_nested_solids(k, b)
            }) {
                a.delete_brep(sweep);
                return Err(e);
            }
        }

        // Reject a manifold boundary whose regions are all void.
        let brep = a.brep(sweep)?;
        let mut has_material = false;
        for region in crate::topology::brep::p1::regions(brep, None)? {
            if brep.region(region)?.is_void() == 0 {
                has_material = true;
                break;
            }
        }
        if manifold && !has_material {
            a.delete_brep(sweep);
            return Err(SolidError::sm_err(OP));
        }
        Ok(sweep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;
    use crate::primitive::api_curves;

    fn segment(a: [f64; 3], b: [f64; 3]) -> Curve {
        let [ax, ay, az] = a;
        let [bx, by, bz] = b;
        Curve::from(
            api_curves::create_line_segment(Point3::new(ax, ay, az), Point3::new(bx, by, bz))
                .expect("segment"),
        )
    }

    /// An open profile wire asks for no caps with the opt-in (upstream always asks, and
    /// its cap fails); a closed one still does, and the open profile sweeps to a sheet.
    #[test]
    fn issue_837_open_profile_sweeps_without_caps() {
        let open = [
            segment([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            segment([1.0, 0.0, 0.0], [1.0, 1.0, 0.0]),
        ];
        let closed = [
            segment([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            segment([1.0, 0.0, 0.0], [1.0, 1.0, 0.0]),
            segment([1.0, 1.0, 0.0], [0.0, 0.0, 0.0]),
        ];
        let mut k = Kernel::default();
        assert!(cap_profile_ends(&k, &open, 1e-6).unwrap());
        k.fixes.planar_path_uncapped_open_profiles = true;
        assert!(!profile_closes(&k, &open, 1e-6).unwrap());
        assert!(profile_closes(&k, &closed, 1e-6).unwrap());
        assert!(!cap_profile_ends(&k, &open, 1e-6).unwrap());
        assert!(cap_profile_ends(&k, &closed, 1e-6).unwrap());

        let path = segment([0.0, 0.0, 0.0], [0.0, 0.0, 5.0]);
        let sheet = crate::sweep::api_brep::create_sweep_along_planar_path(
            &mut k,
            &[&open[0], &open[1]],
            &[&path],
            false,
            3,
        )
        .unwrap();
        assert_eq!(sheet.face_list().len(), 2);
        assert!(!crate::topology::brep::p2::is_manifold_solid(&sheet).unwrap());
    }
}

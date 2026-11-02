//! `SmEdgeuse`, part 2: the radial sector tests (`IsTangentSector`,
//! `IsConvexRadialSector`), the step-off binormal, the UV trim curve checks
//! (`CheckAndSetUVTrimCurve`, `RebuildUVTrimCurve`) and the edge use's notification
//! handler.
//!
//! Upstream: `source/SMLib/src/SmEdgeuse.cpp`, `source/SMLib/inc/SmEdgeuse.h`.
//!
//! | Rust                                   | Upstream                                         |
//! |----------------------------------------|--------------------------------------------------|
//! | [`crate::topology::Edgeuse`] (`Default`, `Drop`) | `SmEdgeuse::SmEdgeuse`, `SmEdgeuse::~SmEdgeuse` |
//! | [`is_tangent_sector`]                  | `SmEdgeuse::IsTangentSector`                     |
//! | [`evaluate_binormal_step_off`], [`StepOffOptions`], [`BinormalEval`] | `SmEdgeuse::EvaluateBinormalStepOff` |
//! | [`evaluate_binormal_step_off_into`]   | `SmEdgeuse::EvaluateBinormalStepOff` (out-params in place) |
//! | [`is_convex_radial_sector`]            | `SmEdgeuse::IsConvexRadialSector`                |
//! | [`rebuild_uv_trim_curve`]              | `SmEdgeuse::RebuildUVTrimCurve`                  |
//! | [`check_and_set_uv_trim_curve`], [`CheckAndSetUvTrimCurve`] | `SmEdgeuse::CheckAndSetUVTrimCurve` |
//! | [`notify`]                             | `SmEdgeuse::Notify`                              |
//! | [`is_edgeuse_side_in_surface`], [`EdgeuseSide`] | `SmEdgeuse::IsEdgeuseSideInSurface`     |
//! | [`has_dog_leg_near_pole`], [`DogLeg`]  | `SmEdgeuse::HasDogLegNearPole`                   |
//! | `is_contained` (private)               | `SM_IS_CONTAINED` (`SmMath.h`)                   |
//!
//! Deviations:
//! - **Object model.** As [`super::p1`]: an edge use is a record in the [`Brep`]'s
//!   arenas and the methods are free functions taking the B-rep and its id. The
//!   constructor is `Edgeuse::default()` (members in the constructor's state); the
//!   destructor is `Drop` (the trim curve and drop-curve record are owned by the
//!   record). Their `Notify` calls (`SM_NO_CONSTRUCTION`, `SM_NO_DESTRUCTION`) are not
//!   modelled, as in [`super::p1`].
//! - **Null pointers.** Where upstream dereferences a null pointer the port returns
//!   [`SolidError::InvalidInput`]; `NER` is [`SolidError::null_pointer`].
//! - **Out-params.** `EvaluateBinormalStepOff` returns all its outputs in a
//!   [`BinormalEval`]; its optional inputs are a [`StepOffOptions`].
//!   [`evaluate_binormal_step_off_into`] writes them in place for callers that read
//!   them after a failed call, as upstream's out-params.
//!   `CheckAndSetUVTrimCurve` takes the curve by value (upstream deletes it on every
//!   path except an error return before the restore: either it stays attached, or the
//!   restoring `SetUVTrimCurve(old, 0.0, TRUE)` deletes it, although the header says
//!   the caller keeps it; `plan/upstream-issues.md` WR-85) and returns
//!   `rdMaxDistToSurf` as an `Option` (upstream leaves the caller's value when the
//!   curve is not good enough) with `rbTrimReplaced`. Its returned status (that of the
//!   first sampled distance measurement) is the `Err` returned after the old curve is
//!   restored.
//! - **Ignored statuses.** `IsTangentSector` ignores (`SE`) the statuses of
//!   `EvaluateBinormal` and `Unitize`. A failed `Unitize` stores `(0, 0, 1)`
//!   ([`Vec3::unitize_or_z`]), as upstream. A failed `EvaluateBinormal` leaves its
//!   outputs unset, which upstream reads as uninitialised locals
//!   (`SmVector3d() = default`); the port uses the `SM_UNDEF_DOUBLE` defaults
//!   (`binormal_ignoring_status`).
//!   `IsConvexRadialSector` likewise ignores its `Unitize` statuses, and returns a
//!   failing `EvaluateBinormalStepOff` status through `SER` from an `SmBoolean`
//!   function, i.e. as `TRUE` (`plan/upstream-issues.md` WR-84); the port returns
//!   `Ok(true)` for such an error.
//! - **Unported callees.** `SmEdgeuse::EvaluateBinormal` and
//!   `SmEdgeuse::CreateUVTrimCurve` (unit `topology.edgeuse_p3`, ported: only their
//!   own deferred callees remain) and
//!   `SmVertex::SetTolerance` (unit `topology.vertex`, reached through
//!   `SmEdge::SetTolerance`) return [`SolidError::Unported`].
//!   `SmCrvOnSurf` preserves the face surface's concrete dispatch in
//!   `CheckAndSetUVTrimCurve`.
//! - **Notification.** [`notify`] runs the edge use's own part and then
//!   [`topo::notify`] (`SmTopology::Notify`) with the caller's records, attribute
//!   store and context; it propagates errors upstream does not report (`Notify`
//!   returns `void`).

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::attribute::AttributeStore;
use crate::geom::context::Context;
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::local_solve_nd::{BoundaryHandler, LocalSolveNd};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::solution_array::SolutionEnd;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec2, Vec3, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::DropCurveFail;
use crate::surface::surface::p7::drop_points_to_tangent_plane;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1::{
    brep_handle, drop_curve_fail, edge, edge_brep, edge_is_lamina, face, faceuse, mate,
    orientation, radial, set_uv_trim_curve, uv_trim_curve, FindStepOff, UvTrimCurveArg,
};
use crate::topology::edgeuse::p3::{create_uv_trim_curve, evaluate_binormal, BinormalOptions};
use crate::topology::topology::{self as topo, TopologyRecords};
use crate::topology::{Brep, EdgeuseId, ObjectHandle};

const OP: &str = "topology::edgeuse::p2";

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

// ---- unported callees -------------------------------------------------------------------

/// The outputs of `SmEdgeuse::EvaluateBinormal` / `EvaluateBinormalStepOff`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinormalEval {
    /// `rBinormalPoint`: the 3D point (on the edge, or the step-off point).
    pub point: Point3,
    /// `rBinormal`: the binormal (unit at the edge; `normal × tangent` at a step-off
    /// point).
    pub binormal: Vec3,
    /// `pOptEdgeuseTangent`: the (non-unit) edge use tangent.
    pub tangent: Vec3,
    /// `pOptEdgeuseFaceNormal` / `pOptFaceuseNormal`: the face use normal.
    pub face_normal: Vec3,
    /// `pOptUVParameter` / `pOptStepOffUV`: the surface parameters of the point.
    pub uv: Point2,
}

/// `SmSurface::GlobalPointSolve` (unit `surface.surface_p4`): the start of every
/// solution.
fn surface_global_point_solve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &crate::geom::extent2d::Extent2d,
    op: SolverOperation,
    test_point: Point3,
    distance_tolerance: f64,
    requested: SolutionRequested,
) -> Result<Vec<SolutionEnd>> {
    crate::surface::surface::p4::global_point_solve_starts(
        k,
        surface,
        uv_domain,
        op,
        test_point,
        distance_tolerance,
        requested,
    )
}

/// `pE->SetTolerance(tolerance)` of the edge use's edge (`SmEdge::SetTolerance` with
/// its default arguments).
fn edge_set_tolerance(b: &mut Brep, eu: EdgeuseId, tolerance: f64) -> Result<()> {
    let e = edge(b, eu)?.ok_or_else(|| null("SetTolerance: edge use without edge"))?;
    crate::topology::edge::set_tolerance(b, e, tolerance, true, true)
}

/// Whether `SER` of a failing status in an `SmBoolean` function returns `TRUE`
/// (`plan/upstream-issues.md` WR-52): every upstream status does;
/// [`SolidError::Unported`] and [`SolidError::InvalidInput`] (a null pointer
/// upstream dereferences) and [`SolidError::NotFound`] propagate.
fn ser_as_true(e: &SolidError) -> bool {
    !matches!(
        e,
        SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::NotFound { .. }
    )
}

// ---- sector tests -------------------------------------------------------------------------

/// `SE(EvaluateBinormal(...))` into locals: a failing status is ignored and the outputs
/// stay at their constructed `SM_UNDEF_DOUBLE` values (upstream's vectors and points
/// default-construct to them; a failure partway through `EvaluateBinormal`, which
/// has written some outputs already, is not modelled). [`SolidError::Unported`], null
/// dereferences and dangling ids propagate.
pub(crate) fn binormal_ignoring_status(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    parameter: f64,
    use_3d_only: bool,
) -> Result<BinormalEval> {
    match evaluate_binormal(
        k,
        b,
        eu,
        parameter,
        use_3d_only,
        &BinormalOptions::default(),
    ) {
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::NotFound { .. }),
        ) => Err(e),
        Err(_) => Ok(BinormalEval {
            point: Point3::UNDEF,
            binormal: Vec3::UNDEF,
            tangent: Vec3::UNDEF,
            face_normal: Vec3::UNDEF,
            uv: Point2::UNDEF,
        }),
        Ok(r) => Ok(r),
    }
}

/// `IsTangentSector`: true when, at the five edge samples `0.34, 0.78, 0.55, 0.12,
/// 0.93` of the edge interval, the binormals of this edge use and its radial partner
/// point against each other (with `coincidence_test`: the same way) and each is
/// perpendicular, within `tangency_tol_deg` degrees, to the other's face use normal.
/// False for an edge use without edge and for a lamina edge. With
/// `ok_to_make_uv_trim_curve` false, edge uses without UV trim curve evaluate in 3D.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (no primary edge use, no radial partner);
/// [`SolidError::Unported`] from `EvaluateBinormal`; its other failures are ignored
/// (`SE`).
pub fn is_tangent_sector(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    tangency_tol_deg: f64,
    coincidence_test: bool,
    ok_to_make_uv_trim_curve: bool,
) -> Result<bool> {
    let Some(e) = edge(b, eu)? else {
        return Ok(false);
    };

    // Lamina Edges have nothing to be tangent (or coincident) with.
    if edge_is_lamina(b, e)? {
        return Ok(false);
    }

    // init an array of edge parameter values
    let params = [0.34, 0.78, 0.55, 0.12, 0.93];

    let eu0 = eu;
    let eu1 = radial(b, eu0)?;

    let max_dot_value = ((90.0 - tangency_tol_deg) * PI / 180.0).cos();

    // for every parameter value
    for p in params {
        // locals
        let test_param = b.edge(e)?.interval.evaluate(p);

        let has_uv_trim_curve = uv_trim_curve(b, eu)?.0.is_some();
        let use_3d_only = !ok_to_make_uv_trim_curve && !has_uv_trim_curve;

        // evaluate this edgeuses binormal vector pointing into sector's 1st face
        let r0 = binormal_ignoring_status(k, b, eu0, test_param, use_3d_only)?;

        // evaluate sector's other edgeuse's binormal vector pointing into sector's 2nd face
        let eu1 = eu1.ok_or_else(|| null("IsTangentSector: edge use without radial"))?;
        let r1 = binormal_ignoring_status(k, b, eu1, test_param, use_3d_only)?;

        // normalize the sample vectors
        let faceuse_normal = r0.face_normal.unitize_or_z();
        let faceuse_normal2 = r1.face_normal.unitize_or_z();
        let bin_vec = r0.binormal.unitize_or_z();
        let bin_vec2 = r1.binormal.unitize_or_z();

        // finds anything > 90 deg. avoids confusion between angles near 0.0 and
        // angles near 180; extended to allow testing for coincident faces
        if !coincidence_test && bin_vec.dot(bin_vec2) > 0.0 {
            return Ok(false);
        }
        if coincidence_test && bin_vec.dot(bin_vec2) < 0.0 {
            return Ok(false);
        }

        let dot1 = bin_vec2.dot(faceuse_normal);
        if dot1.abs() > max_dot_value {
            // sBinVec2 is not perpendicular to sFaceuseNormal to within tolerance
            return Ok(false);
        }
        let dot2 = bin_vec.dot(faceuse_normal2);
        if dot2.abs() > max_dot_value {
            // sBinVec1 is not perpendicular to sFaceuseNormal2 to within tolerance
            return Ok(false);
        }
    }

    // the sector is tangent
    Ok(true)
}

/// The optional inputs of [`evaluate_binormal_step_off`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepOffOptions {
    /// `pOptCenterPoint`: the plane point and sphere centre (`None`: the surface point
    /// at the edge parameter).
    pub center_point: Option<Point3>,
    /// `pOptPlaneNormal`: the plane normal (`None`: the edge use tangent).
    pub plane_normal: Option<Vec3>,
    /// `bOKToMakeUVTrimCurve` (default `TRUE`): UV trim curves may be built when
    /// missing.
    pub ok_to_make_uv_trim_curve: bool,
}

impl Default for StepOffOptions {
    fn default() -> Self {
        Self {
            center_point: None,
            plane_normal: None,
            ok_to_make_uv_trim_curve: true,
        }
    }
}

/// `EvaluateBinormalStepOff`: the binormal `normal × tangent` at the surface point
/// `step_off_distance` from the edge point at `parameter`, found by intersecting the
/// surface, the plane through the centre point perpendicular to the tangent and the
/// sphere of that radius around the centre (local solve from a tangent-plane guess,
/// the clamped guess when it does not converge). A distance below `SM_EFF_ZERO`
/// returns the binormal at the edge.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; `SM_ERR` when the step leaves the
/// face's UV domain; [`SolidError::null_pointer`] without face or surface;
/// [`SolidError::InvalidInput`] without face use; the evaluators' errors;
/// [`SolidError::Unported`] from `EvaluateBinormal`'s deferred callees (see
/// [`crate::topology::edgeuse::p3`]).
pub fn evaluate_binormal_step_off(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    parameter: f64,
    step_off_distance: f64,
    options: &StepOffOptions,
) -> Result<BinormalEval> {
    let mut out = BinormalEval {
        point: Point3::UNDEF,
        binormal: Vec3::UNDEF,
        tangent: Vec3::UNDEF,
        face_normal: Vec3::UNDEF,
        uv: Point2::UNDEF,
    };
    evaluate_binormal_step_off_into(k, b, eu, parameter, step_off_distance, options, &mut out)?;
    Ok(out)
}

/// [`evaluate_binormal_step_off`] writing its outputs into the caller's record in
/// upstream's order, so that a failing call leaves what upstream's out-params hold:
/// once `EvaluateBinormal` succeeded, the point, binormal (unitised in place; a failed
/// `Unitize` stores `(0, 0, 1)`), tangent, face normal and UV of the edge point are
/// set, and a failure later (stepping off the face, a failed evaluation) keeps them.
/// Only on success are the step-off point, binormal, face normal and UV written. A
/// failing `EvaluateBinormal` (its outputs are written piecemeal upstream) leaves the
/// record untouched here. Callers that read the outputs after a failed call
/// (`SmEdge::RadialSectorClassify`) start from the previous call's values, as
/// upstream's locals.
///
/// # Errors
///
/// As [`evaluate_binormal_step_off`].
pub fn evaluate_binormal_step_off_into(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    parameter: f64,
    step_off_distance: f64,
    options: &StepOffOptions,
    out: &mut BinormalEval,
) -> Result<()> {
    // evaluate binormal at the edge - set outputs for stepOffDistance = 0.0
    let at_edge = evaluate_binormal(
        k,
        b,
        eu,
        parameter,
        !options.ok_to_make_uv_trim_curve,
        &BinormalOptions::default(),
    )?;
    *out = at_edge;
    let uv = at_edge.uv;
    let tangent = at_edge.tangent;

    // low work - StepOffDistance = 0.0
    if step_off_distance < EFF_ZERO {
        return Ok(());
    }

    // edgeuse locals
    let f = face(b, eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let face_rec = b.face(f)?;
    let surface = face_rec
        .surface
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let uv_domain = face_rec.uv_domain;

    // evaluate surface at edge point
    let mut surf_point = surface.evaluate_point(k, uv)?;

    // set startpoint = stepoffdistance from surfPoint in binormal direction
    let binormal = match at_edge.binormal.unitize() {
        Ok(v) => v,
        Err(e) => {
            out.binormal = Vec3::new(0.0, 0.0, 1.0);
            return Err(e);
        }
    };
    out.binormal = binormal;
    let start_point = surf_point + step_off_distance * binormal;

    // drop start point to surface to get a guess point
    let dropped =
        drop_points_to_tangent_plane(k, surface, uv, Side::Left, Side::Left, &[start_point])?;
    let mut uv_guess = dropped
        .first()
        .copied()
        .ok_or_else(|| null("DropPointsToTangentPlane: no point"))?;

    // Return an error if we step off the face UVDomain.
    if !uv_domain.contains_point(uv_guess, 0.0) {
        // Use NORMALIZE here: if the point is past the end of the Face, we don't want
        // it returning success with a boundary point with large deviation from the
        // normal [B579]
        let solutions = surface_global_point_solve(
            k,
            surface,
            &uv_domain,
            SolverOperation::Normalize,
            start_point,
            face_rec.tolerance,
            SolutionRequested::All,
        )?;
        let Some(first) = solutions.first() else {
            // stepoffdistance stepped off face UVDomain
            return Err(SolidError::sm_err(OP));
        };
        uv_guess = Point2::new(first.param(0)?, first.param(1)?);

        // still stepping off face UVDomain
        if !uv_domain.contains_point(uv_guess, 0.0) {
            return Err(SolidError::sm_err(OP));
        }
    }

    let mut intervals = ExtentNd::new(2);
    if let Some(x) = intervals.get_mut(0) {
        *x = Extent1d::from_min_max(uv_domain.min().x, uv_domain.max().x);
    }
    if let Some(x) = intervals.get_mut(1) {
        *x = Extent1d::from_min_max(uv_domain.min().y, uv_domain.max().y);
    }

    // In general when doing local solver don't check for periodicity.
    let periodicities = [false, false];
    let guess_t = [uv_guess.x, uv_guess.y];

    // use local solver to find point on surface stepOffDistance from edge point in
    // binormal direction.
    if let Some(c) = options.center_point {
        surf_point = c;
    }
    let plane_normal = options.plane_normal.unwrap_or(tangent);

    let mut eval_fun = FindStepOff::new(k, step_off_distance, surf_point, plane_normal, surface);
    let mut ls = LocalSolveNd::new(&mut eval_fun, Some(&intervals), Some(&periodicities));
    ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 3);

    // intersect: plane, surface and sphere
    let sol = ls.solve_it(&guess_t, EFF_ZERO_SQRT * 100.0)?;
    let mut solution_vector = [
        sol.x.first().copied().unwrap_or(0.0),
        sol.x.get(1).copied().unwrap_or(0.0),
    ];

    // If we didn't converge just take the guess point as good enough.
    if !sol.found {
        let uv_pnt = uv_domain.clamp_point(uv_guess);
        solution_vector = [uv_pnt.x, uv_pnt.y];
    }

    // evaluate stepoffPoint face normal - (negate from surf normal for opposite face
    // orientations)
    let uv_found = Point2::new(solution_vector[0], solution_vector[1]);

    let mut normal = surface.evaluate_normal(k, uv_found, Side::Left, Side::Left)?;
    let fu = faceuse(b, eu)?.ok_or_else(|| null("EvaluateBinormalStepOff: no face use"))?;
    if b.faceuse(fu)?.orientation == OrientType::Opposite {
        normal = -normal;
    }

    // set output
    out.binormal = normal.cross(tangent);
    out.point = surface.evaluate_point(k, uv_found)?;
    out.face_normal = normal;
    out.uv = uv_found;
    Ok(())
}

/// `IsConvexRadialSector`: true when, at `num_samples` evenly spaced interior samples
/// of the edge interval (stepped `20 * edge tolerance` into the faces when
/// `opt_step_off`), the face normals and the radial partner's binormals do not both
/// point the same way (beyond `tol_deg` degrees of concavity; `0`: beyond
/// `-SM_EFF_ZERO`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (no edge, no primary edge use, no radial partner);
/// [`SolidError::Unported`] and the null dereferences of
/// [`evaluate_binormal_step_off`] (its other errors make the result `true`, as
/// upstream's `SER`).
pub fn is_convex_radial_sector(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    num_samples: u64,
    opt_step_off: bool,
    tol_deg: f64,
) -> Result<bool> {
    // locals: radial partner, edge->Interval
    let radial_eu = radial(b, eu)?;
    let e = edge(b, eu)?.ok_or_else(|| null("IsConvexRadialSector: edge use without edge"))?;
    let ivl = b.edge(e)?.interval;
    let step_off_dist = if opt_step_off {
        20.0 * b.edge(e)?.tolerance
    } else {
        0.0
    };

    // convert to radians
    let dot_product_limit = if tol_deg == 0.0 {
        -EFF_ZERO
    } else {
        (PI * (90.0 - tol_deg) / 180.0).cos()
    };

    let options = StepOffOptions::default();
    // for every requested sample point
    for i in 0..num_samples {
        // sample evenly skipping endPoints
        let t = (i as f64 + 1.0) / (num_samples as f64 + 1.0);

        // SER in an SmBoolean function: a failing status is returned as TRUE
        let r = match evaluate_binormal_step_off(k, b, eu, ivl.evaluate(t), step_off_dist, &options)
        {
            Ok(r) => r,
            Err(e) if ser_as_true(&e) => return Ok(true),
            Err(e) => return Err(e),
        };
        let radial_eu =
            radial_eu.ok_or_else(|| null("IsConvexRadialSector: edge use without radial"))?;
        let r2 = match evaluate_binormal_step_off(
            k,
            b,
            radial_eu,
            ivl.evaluate(t),
            step_off_dist,
            &options,
        ) {
            Ok(r) => r,
            Err(e) if ser_as_true(&e) => return Ok(true),
            Err(e) => return Err(e),
        };
        let (mut bin, mut norm, mut bin2, mut norm2) =
            (r.binormal, r.face_normal, r2.binormal, r2.face_normal);
        if tol_deg != 0.0 {
            bin = bin.unitize_or_z();
            norm = norm.unitize_or_z();
            bin2 = bin2.unitize_or_z();
            norm2 = norm2.unitize_or_z();
        }

        // when SurfaceNormal is in same direction of OtherSurface Binormal - its not
        // convex
        if norm2.dot(bin) > dot_product_limit && norm.dot(bin2) > dot_product_limit {
            return Ok(false);
        }
    }

    Ok(true)
}

// ---- UV trim curves -----------------------------------------------------------------------

/// `RebuildUVTrimCurve`: computes a new UV trim curve and attaches it with its
/// largest distance to the surface, deleting the attached one.
///
/// # Errors
///
/// [`SolidError::Unported`] from `CreateUVTrimCurve`'s deferred callees; errors of
/// [`set_uv_trim_curve`].
pub fn rebuild_uv_trim_curve(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<()> {
    // create new TrimCurve
    let created = create_uv_trim_curve(k, b, eu, false, None)?;
    let (max_distance_to_surface, new_uv_trim_curve) = (created.max_dist_to_surf, created.curve);

    // Attach to Edgeuse - replace existing data when needed
    set_uv_trim_curve(
        b,
        eu,
        UvTrimCurveArg::New(Box::new(new_uv_trim_curve)),
        max_distance_to_surface,
        true,
        None,
    )?;
    Ok(())
}

/// The outputs of [`check_and_set_uv_trim_curve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheckAndSetUvTrimCurve {
    /// `rdMaxDistToSurf`: the distance between the edge curve and the UV curve on
    /// the surface, set only when the curve was good enough.
    pub max_dist_to_surf: Option<f64>,
    /// `rbTrimReplaced`: the curve is now the edge use's UV trim curve.
    pub trim_replaced: bool,
}

/// `CheckAndSetUVTrimCurve`: attaches `uv_trim_curve` when the edge curve and the
/// curve traced on the face's surface are close: their 8-sample distance is below
/// `100 *` the B-rep tolerance or a tenth of the edge's approximate length (then
/// measured precisely unless it is below a thousandth of the B-rep tolerance). With
/// `adjust_tolerances`, when the edge tolerance is below twice that distance, a
/// re-dropped UV curve replaces it if it is more than twice as close, else the edge
/// tolerance grows. Otherwise the old UV trim curve (and drop-curve record) is put
/// back.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (no face, surface, edge, edge curve, mate, B-rep);
/// the first distance measurement's error (returned after the old curve is restored);
/// errors of the precise measurement; [`SolidError::Unported`] for a non-B-spline
/// surface (`curve.crv_on_surf`), `CreateUVTrimCurve`'s deferred callees and
/// `SmEdge::SetTolerance` (`topology.edge`).
pub fn check_and_set_uv_trim_curve(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    uv_trim_curve_in: Curve,
    adjust_tolerances: bool,
    opt_drop_curve_fail: Option<&mut DropCurveFail<ObjectHandle>>,
) -> Result<CheckAndSetUvTrimCurve> {
    // init output
    let mut out = CheckAndSetUvTrimCurve {
        max_dist_to_surf: None,
        trim_replaced: false,
    };

    // locals
    uv_trim_curve(b, eu)?;
    let f = face(b, eu)?.ok_or_else(|| null("CheckAndSetUVTrimCurve: edge use without face"))?;
    let e = edge(b, eu)?.ok_or_else(|| null("CheckAndSetUVTrimCurve: edge use without edge"))?;

    // save a copy of OldDropCurveFail data
    let old_drop_curve_fail = match drop_curve_fail(b, eu)?.0 {
        Some(old) => {
            let mut copy = DropCurveFail::default();
            copy.assign(old);
            copy.brep = brep_handle(b, eu)?;
            Some(copy)
        }
        None => None,
    };

    // attach pUVTrimCurve to this Edgeuse (FALSE = don't delete existing UVTrimCurve)
    let old_uv_trim_curve = set_uv_trim_curve(
        b,
        eu,
        UvTrimCurveArg::New(Box::new(uv_trim_curve_in)),
        0.0,
        false,
        opt_drop_curve_fail,
    )?;
    let uv_copy = uv_trim_curve(b, eu)?
        .0
        .ok_or_else(|| null("CheckAndSetUVTrimCurve: attached curve missing"))?
        .clone_for_evaluation()?;

    // define the 3D curve for the UVCurve projected through the Surface
    let surface = b
        .face(f)?
        .surface
        .as_ref()
        .ok_or_else(|| null("CheckAndSetUVTrimCurve: face without surface"))?;
    let map_3d = CrvOnSurf::new(k, uv_copy, surface.clone_for_evaluation()?, None, 0)?;
    let natural = map_3d.natural_interval();
    let map_3d = Curve::from(map_3d);
    let edge_rec = b.edge(e)?;
    let interval = edge_rec.interval;
    let edge_curve = edge_rec
        .curve
        .as_ref()
        .ok_or_else(|| null("CheckAndSetUVTrimCurve: edge without curve"))?;

    // find approx max dist between edge and 3d Projection of UVCurve through the
    // Surface using 8 sample points
    let rtn = edge_curve.curve_max_distance_between(
        k,
        &interval,
        &map_3d,
        natural.min(),
        natural.max(),
        8,
        None,
    );
    let mut first_err = None;
    match rtn {
        Err(err) if err.is_unported() => return Err(err),
        Err(err) => first_err = Some(err),
        // when MaxDist was computed without hiccups (the incorrect-direction flag is
        // set on every path a sampled measurement takes)
        Ok(md) if md.incorrect_dir == Some(false) => {
            let mut max_dist_to_surf = md.distance;
            let brep_tol = edge_brep(b, e)?
                .ok_or_else(|| null("CheckAndSetUVTrimCurve: edge without B-rep"))?
                .tolerance();

            // remember when EdgeCurve/UVTrimcurve distance is very close
            let precise_curve = max_dist_to_surf < brep_tol / 1000.00;

            // when the two curves are kind of close - mark the bUVGoodEnough bit as TRUE
            let mut uv_good_enough = max_dist_to_surf < brep_tol * 100.0;

            // give bad curves one last chance to be ok
            if !uv_good_enough {
                // check approx pEdgeCurve length with 10 sample points
                let (edge_length, _) = edge_curve.approximate_length(k, &interval, 10, false)?;

                // if max dist between curves is near the edge (based on edge length)
                if max_dist_to_surf < edge_length / 10.0 {
                    uv_good_enough = true;
                }
            }

            // when this UVCurve is GoodEnough and the curve is not Precise
            if uv_good_enough && !precise_curve {
                // find accurate max dist between edge and projected UVCurve
                let md2 = edge_curve.curve_max_distance_between(
                    k,
                    &interval,
                    &map_3d,
                    natural.min(),
                    natural.max(),
                    0,
                    None,
                )?;
                max_dist_to_surf = md2.distance;
                if md2.incorrect_dir != Some(false) {
                    uv_good_enough = false;
                }
            }

            // When UVTrimCurve is good enough
            if uv_good_enough {
                // save the max dist value for output
                out.max_dist_to_surf = Some(max_dist_to_surf);

                // when adjusting edge and face tolerances
                if adjust_tolerances {
                    let curr_tol = b.edge(e)?.tolerance;

                    // when edge tolerance is less than 2*max dist between curves
                    if curr_tol < 2.0 * max_dist_to_surf {
                        // Try reprojecting to get a better UV curve
                        let created = create_uv_trim_curve(k, b, eu, false, None)?;
                        let (mut tmp_max_dist_to_surf, tmp_uv_crv) =
                            (created.max_dist_to_surf, created.curve);
                        if max_dist_to_surf > tmp_max_dist_to_surf * 2.0 {
                            out.max_dist_to_surf = Some(max_dist_to_surf);
                            if !adjust_tolerances {
                                tmp_max_dist_to_surf = 0.0;
                            }
                            set_uv_trim_curve(
                                b,
                                eu,
                                UvTrimCurveArg::New(Box::new(tmp_uv_crv)),
                                tmp_max_dist_to_surf,
                                true,
                                None,
                            )?;
                        } else {
                            edge_set_tolerance(b, eu, 2.0 * max_dist_to_surf)?;
                        }
                    }
                }

                // done with this edgeuse - it has a good enough UVTrimCurve
                out.trim_replaced = true;
            }
        }
        Ok(_) => {}
    }

    // when UVTrimCurve was replaced - delete OldUVTrimCurve
    if !out.trim_replaced {
        // UV Curve was not very accurate - restore the old one
        let mut tgt_drop = old_drop_curve_fail;
        let arg = match old_uv_trim_curve {
            Some(c) => UvTrimCurveArg::New(Box::new(c)),
            None => UvTrimCurveArg::Null,
        };
        set_uv_trim_curve(b, eu, arg, 0.0, true, tgt_drop.as_mut())?;
    }

    match first_err {
        Some(err) => Err(err),
        None => Ok(out),
    }
}

// ---- notification -------------------------------------------------------------------------

/// `delete pEU`: `~SmEdgeuse` notifies `SM_NO_DESTRUCTION` (`Notify(op, this, NULL,
/// NULL)` through the edge use's context, `GetContext()`: the environment's, else
/// `k`'s), whose status it ignores ([`SolidError::Unported`] propagates); its UV trim
/// and drop-failure curves are dropped with its record; `~SmAObject`
/// (`ClearAttributes`) removes it from every attribute it uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; the notification's
/// [`SolidError::Unported`]; those of `ClearAttributes`.
pub fn destroy(
    k: &mut Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    env: &mut crate::topology::edge::NotifyEnv<'_>,
) -> Result<()> {
    b.edgeuse(eu)?;
    let this = (env.key_of)(crate::error::EntityRef::Edgeuse(eu));
    crate::topology::edge::with_object_context(k, env, |n| {
        crate::error::ignore_status(notify(
            b,
            eu,
            &mut *n.records,
            &mut *n.attributes,
            n.context.as_deref_mut(),
            NotifyOperation::Destruction,
            this,
            [Some(this), None, None],
        ))
    })?;
    b.remove_edgeuse_record(eu)?;
    env.attributes.clear_attributes(this.key)
}

/// `Notify`: a split (`SplitInBrep`, `Split`) or a geometry change (`ChangeGeometry`)
/// deletes the UV trim curve and resets the cached edge/face gaps of the edge use and
/// its mate; then the notification goes to [`topo::notify`] (`SmTopology::Notify`)
/// with `records`, `attributes` and `context`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (no mate, and those of [`set_uv_trim_curve`]); errors of
/// [`topo::notify`].
pub fn notify<R: TopologyRecords + ?Sized>(
    b: &mut Brep,
    eu: EdgeuseId,
    records: &mut R,
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data: [Option<ObjectRef>; 3],
) -> Result<()> {
    use NotifyOperation as N;
    if matches!(operation, N::SplitInBrep | N::ChangeGeometry | N::Split) {
        // Clear UVTrimCurve (TRUE = delete existing UVTrimCurve)
        set_uv_trim_curve(b, eu, UvTrimCurveArg::Null, 0.0, true, None)?;

        // Clear Cached Gap data
        let mate_eu = mate(b, eu)?;
        b.edgeuse_mut(eu)?.max_edge_face_gap.reset();
        let m = mate_eu.ok_or_else(|| null("Notify: edge use without mate"))?;
        b.edgeuse_mut(m)?.max_edge_face_gap.reset();
    }

    // Propagate notification up hierarchy
    let [data1, data2, data3] = data;
    topo::notify(
        records, attributes, context, operation, this, data1, data2, data3,
    )
}

// ---- side and pole checks (SmEdgeuse::IsEdgeuseSideInSurface, HasDogLegNearPole) ----------

/// `SM_IS_CONTAINED(c, a, b)` (`SmMath.h`): `c` inside or on `[a, b]` (either order),
/// widened by `SmTol::GetScaledZero(a, b)`.
fn is_contained(c: f64, a: f64, b: f64) -> bool {
    let tz = crate::geom::tol::scaled_zero2(&a, &b);
    ((a <= c + tz) && (c - tz <= b)) || ((b <= c + tz) && (c - tz <= a))
}

/// `SM_DEG2RAD(adeg)` = `adeg * (SM_PI / 180.0)`.
fn deg_to_rad(deg: f64) -> f64 {
    deg * (PI / 180.0)
}

/// The result of [`is_edgeuse_side_in_surface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeuseSide {
    /// The return value: `true` when the inside side of the edge use lies in the
    /// surface's domain.
    pub in_surface: bool,
    /// `*pOptOnNaturalBoundary`: `true` unless a sample point off every natural boundary
    /// was found.
    pub on_natural_boundary: bool,
}

/// `SmEdgeuse::IsEdgeuseSideInSurface(lSmpCount, pOptPeriodicU, pOptPeriodicV,
/// pOptOnNaturalBoundary)`: drops `smp_count` points of the edge curve (in edge use
/// order) onto the face's surface and classifies them against the natural UV domain's
/// sides (to twice the surface zone tolerance mapped to UV in U and in V). A sample off
/// every side answers `true` at once. Otherwise the edge use runs along a side: a
/// degenerate edge (its sample box below twice the zone tolerance) is `true`, and else
/// the UV steps must run the way that side's inside requires (all but one of them;
/// for a periodic side without UV trim curve either way). An edge use without face,
/// surface or edge is `true`. `periodic_u` / `periodic_v` override
/// `SmSurface::IsPeriodic`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for an edge
/// without curve (upstream dereferences null); those of `IsPeriodic`, the curve and
/// surface evaluations and `MapTo2d` that upstream does not ignore. The statuses of
/// the evaluations and of `DropPoint` are ignored, as upstream ([`SolidError::Unported`]
/// still propagates).
pub fn is_edgeuse_side_in_surface(
    k: &Kernel,
    b: &Brep,
    eu: EdgeuseId,
    smp_count: u64,
    periodic_u: Option<bool>,
    periodic_v: Option<bool>,
) -> Result<EdgeuseSide> {
    use crate::geom::extent2d::{SS_UMAX, SS_UMIN, SS_VMAX, SS_VMIN};
    use crate::geom::extent3d::Extent3d;
    use crate::nurbs::b_spline_surface::SurfParamType;
    use crate::surface::surface::p1::{map_to_2d_dir, zone_tol_3d};
    use crate::surface::surface::p5::DropPointOptions;

    // init return and ouput vals
    let mut rtn = true;
    let mut out = EdgeuseSide {
        in_surface: true,
        on_natural_boundary: true,
    };

    // locals
    let f = face(b, eu)?;
    let surface = match f {
        Some(f) => b.face(f)?.surface.as_ref(),
        None => None,
    };
    let e = edge(b, eu)?;

    // no work - missing Face, Surface, or Edge
    let (Some(surface), Some(e)) = (surface, e) else {
        return Ok(out);
    };

    // classify periodic surface
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let periodic_u = match periodic_u {
        Some(p) => p,
        None => surface.is_periodic(k, &uv_domain, SurfParamType::U)?,
    };
    let periodic_v = match periodic_v {
        Some(p) => p,
        None => surface.is_periodic(k, &uv_domain, SurfParamType::V)?,
    };

    // locals
    let edge_rec = b.edge(e)?;
    let edge_curve = edge_rec
        .curve
        .as_ref()
        .ok_or_else(|| null("IsEdgeuseSideInSurface: edge without curve"))?;
    let edge_interval = edge_rec.interval;
    let same = orientation(b, eu)? == OrientType::Same;
    let mut uv_drop_points: Vec<Point2> = Vec::new();
    let mut uv_classify = SS_UMIN | SS_VMIN | SS_UMAX | SS_VMAX;
    // `SmPoint2d` / `SmPoint3d` default to undefined coordinates
    let mut uv_drop_point = Point2::UNDEF;
    let mut uv_trim_curve_3d = Point3::UNDEF;
    let mut edge_point = Point3::UNDEF;
    let u_dir = Vec2::new(1.0, 0.0);
    let v_dir = Vec2::new(0.0, 1.0);
    let (mut up_u, mut down_u, mut up_v, mut down_v) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut edge_box_3d = Extent3d::default();
    let uv_trim_curve = uv_trim_curve(b, eu)?.0;

    // Tol = 3d Surface Zone Tol mapped for each sample point to 2d through the surface
    // function
    let surf_zone_tol_3d = zone_tol_3d(k, surface)?;

    // for every sample point - check for Edgeuse point on Surface boundary whose inside
    // direction points out of the Surface domain
    for ii in 0..smp_count {
        // param
        let last = smp_count.wrapping_sub(1);
        let norm_param = if same {
            if ii == 0 {
                0.0
            } else if ii == last {
                1.0
            } else {
                ii as f64 / last as f64
            }
        } else if ii == 0 {
            1.0
        } else if ii == last {
            0.0
        } else {
            1.0 - (ii as f64 / last as f64)
        };
        let param = edge_interval.evaluate(norm_param);

        // Edge->3d Point (status ignored: a failed evaluation leaves the last point)
        if let Some(p) = ignored(edge_curve.evaluate_point(k, param))? {
            edge_point = p;
        }

        // next Drop GuessUV = UVTrimCurve(dParam) or last Drop point
        let mut guess = None;
        if let Some(trim) = uv_trim_curve {
            if let Some(p) = ignored(trim.evaluate_point(k, param))? {
                uv_trim_curve_3d = p;
            }
            guess = Some(Point2::new(uv_trim_curve_3d.x, uv_trim_curve_3d.y));
        } else if ii > 0 {
            guess = uv_drop_points.last().copied();
        }

        // Drop Point (status ignored)
        let opts = DropPointOptions {
            guess_uv: guess,
            operation: SolverOperation::Minimize,
            ..DropPointOptions::default()
        };
        let drop = surface
            .drop_point::<crate::surface::surface::p4::NoObject>(k, edge_point, &uv_domain, &opts);
        match ignored(drop)? {
            // with a guess `rResultUV` starts as the guess; without one it is written
            // only by a successful drop
            Some(d) if d.success || guess.is_some() => uv_drop_point = d.result_uv,
            Some(_) => (),
            None => {
                if let Some(g) = guess {
                    uv_drop_point = g;
                }
            }
        }

        // Tol  2d mapping of Surface ZoneTol3d
        let tol_2d_u = map_to_2d_dir(k, surf_zone_tol_3d, uv_drop_point, u_dir, surface)?;
        let tol_2d_v = map_to_2d_dir(k, surf_zone_tol_3d, uv_drop_point, v_dir, surface)?;

        // classify sUVPoint - to XSectTol3d numerical tolerances mapped to 2d (diff in U
        // and V directions)
        let uv_loc_u =
            uv_domain.point_boundaries(uv_drop_point, 2.0 * tol_2d_u) & (SS_UMIN | SS_UMAX);
        let uv_loc_v =
            uv_domain.point_boundaries(uv_drop_point, 2.0 * tol_2d_v) & (SS_VMIN | SS_VMAX);
        let uv_loc = uv_loc_u | uv_loc_v;

        // save the drop and classify data per sample point
        edge_box_3d.add_point(edge_point);
        uv_drop_points.push(uv_drop_point);

        // accumulate the classifications and the UV incremental step directions
        uv_classify &= uv_loc;
        if let (Some(prev), Some(cur)) = (uv_drop_points.iter().rev().nth(1), uv_drop_points.last())
        {
            if prev.x < cur.x {
                up_u += 1;
            }
            if prev.x > cur.x {
                down_u += 1;
            }
            if prev.y < cur.y {
                up_v += 1;
            }
            if prev.y > cur.y {
                down_v += 1;
            }
        }

        // low work - found a classify point not on a boundary
        if uv_classify == 0 {
            // both sides of this edgeuse are inside at this point, so return TRUE
            out.on_natural_boundary = false;
            return Ok(out);
        }
    }

    // any Edgeuse with an Edge which is degenerate (a database problem to be healed) is
    // classified as having a side inside the surface
    if edge_box_3d.max_dimension() < 2.0 * surf_zone_tol_3d {
        return Ok(out);
    }

    // check direction for edges running a natural boundary (all but one of the
    // increments in the right direction; either way for a periodic side without UV trim
    // curve, as upstream)
    let need = smp_count.wrapping_sub(2);
    let no_trim = uv_trim_curve.is_none();
    if uv_classify & SS_UMIN != 0 {
        rtn = down_v >= need || (no_trim && periodic_u && up_v >= need);
    } else if uv_classify & SS_UMAX != 0 {
        rtn = up_v >= need || (no_trim && periodic_u && down_v >= need);
    } else if uv_classify & SS_VMIN != 0 {
        rtn = up_u >= need || (no_trim && periodic_v && down_u >= need);
    } else if uv_classify & SS_VMAX != 0 {
        rtn = down_u >= need || (no_trim && periodic_v && up_u >= need);
    }

    // all done
    out.in_surface = rtn;
    Ok(out)
}

/// The result of [`has_dog_leg_near_pole`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DogLeg {
    /// The return value: `true` (a problem) when the UV trim curve's control polygon
    /// turns to follow a singular side of the surface domain.
    pub has_dog_leg: bool,
    /// `*pOptIndx`: the control point index at the first turn (unset upstream when there
    /// is none).
    pub index: Option<usize>,
}

/// `SmEdgeuse::HasDogLegNearPole(pOptIndx)`: whether the UV trim curve of the edge use
/// (its own or its mate's) has a control polygon that turns near a singular side of the
/// face surface's natural domain (within a fiftieth of the domain). A curve of the
/// intersectors' form (cubic, piecewise C0) is tested only at its first and last
/// Bezier segments with `smgu_IsDogLeg` (35 / 45 degrees); any other curve at every
/// interior control point near a singular side, by the polygon's bend there (over 50
/// degrees) with one leg within 10 degrees of the U direction and the other at 35 to
/// 145 degrees to it. No UV trim curve, no face surface or no singular side is `false`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of `GetSingularities` and of the
/// curve's NURB (`SM_ERR` without one; upstream dereferences `NULL`);
/// [`SolidError::InvalidInput`] for an intersector-form polygon of fewer than four
/// points (upstream reads before the array) or a UV trim curve that is not an
/// `SmBSplineCurve`.
pub fn has_dog_leg_near_pole(k: &Kernel, b: &Brep, eu: EdgeuseId) -> Result<DogLeg> {
    use crate::curve::curve::CurveKind;
    use crate::geom::extent2d::{SS_NONE, SS_UMAX, SS_UMIN, SS_VMAX, SS_VMIN};
    use crate::geom::geom_utility::p1::is_dog_leg;
    use crate::math::rad_to_deg;
    use std::borrow::Cow;

    let mut out = DogLeg {
        has_dog_leg: false,
        index: None,
    };

    // no work - no UVTrimCurve to check
    let Some(uv_trim_curve) = uv_trim_curve(b, eu)?.0 else {
        return Ok(out);
    };

    // no work - no Surface to check
    let Some(f) = face(b, eu)? else {
        return Ok(out);
    };
    let Some(surface) = b.face(f)?.surface.as_ref() else {
        return Ok(out);
    };

    // rtn: SM_SS_NONE or an orof: SM_SS_UMIN, SM_SS_VMIN, SM_SS_UMAX, SM_SS_VMAX
    let sing = surface
        .get_singularities_of_face(k, crate::topology::face::p2::tolerance(b, f)?, false, false)?
        .singular;

    // no work - no Singularities to check
    if sing == SS_NONE {
        return Ok(out);
    }

    // locals
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let tol_dist_u = uv_domain.x_length() / 50.0;
    let tol_dist_v = uv_domain.y_length() / 50.0;
    let u_min = uv_domain.u_min();
    let u_max = uv_domain.u_max();
    let v_min = uv_domain.v_min();
    let v_max = uv_domain.v_max();

    // BSpline Control Points (GetPolygon makes a missing NURB)
    let base: Cow<'_, crate::nurbs::b_spline_curve::BSplineCurve> = match uv_trim_curve.kind() {
        CurveKind::BSpline(c) => Cow::Borrowed(c),
        CurveKind::Line(c) => c.base_with_nurb()?,
        CurveKind::Ellipse(c) => Cow::Borrowed(c.base()),
        CurveKind::Parabola(c) => Cow::Borrowed(c.base()),
        CurveKind::Hyperbola(c) => Cow::Borrowed(c.base()),
        _ => {
            return Err(null(
                "HasDogLegNearPole: UV trim curve is not an SmBSplineCurve",
            ))
        }
    };
    let control_points = base.polygon(&base.natural_interval())?;
    let num = control_points.len();
    let uv = |i: usize| -> Result<Vec2> {
        control_points
            .get(i)
            .map(|p| Vec2::new(p.x, p.y))
            .ok_or_else(|| null("HasDogLegNearPole: control point index out of range"))
    };
    let on_sing = |p: &Point3| {
        let on_u = ((sing & SS_UMIN) != 0 && (p.x - u_min).abs() < tol_dist_u)
            || ((sing & SS_UMAX) != 0 && (p.x - u_max).abs() < tol_dist_u);
        let on_v = ((sing & SS_VMIN) != 0 && (p.y - v_min).abs() < tol_dist_v)
            || ((sing & SS_VMAX) != 0 && (p.y - v_max).abs() < tol_dist_v);
        (on_u, on_v)
    };

    // Special case curves built by the intersectors and DropCurve (expected to be the
    // common case for UVTrimCurves)
    if base.is_intersector_approx()? {
        // tunable locals
        let perp_vec_deg = 35.0;
        let parallel_vec_deg = 45.0;

        // see if start/end is on a singularity
        for ii in 0..2usize {
            // iter 0=Start Seg, 1=End Seg (`lNum - 4` is unsigned upstream)
            let indx = if ii == 0 {
                0
            } else {
                num.checked_sub(4)
                    .ok_or_else(|| null("HasDogLegNearPole: fewer than four control points"))?
            };

            // beg/end ControlPoint
            let cpt = control_points
                .get(indx + ii * 3)
                .ok_or_else(|| null("HasDogLegNearPole: control point index out of range"))?;

            // check for control points near constant U / V Singularity boundaries
            let (on_sing_u, on_sing_v) = on_sing(cpt);

            // when start/end UVTrimCurve segment starts/ends on Pole and has a dog-leg shape
            if (on_sing_u || on_sing_v)
                && is_dog_leg(
                    uv(indx)?,
                    uv(indx + 1)?,
                    uv(indx + 2)?,
                    uv(indx + 3)?,
                    perp_vec_deg,
                    parallel_vec_deg,
                )
            {
                // set optional output
                out.index = Some(indx + 1);
                out.has_dog_leg = true;
                break;
            }
        }
    } else {
        // UVTrimCurve was not created by an intersector or dropCurve

        // for every control point - see if its near the singularity boundary
        let mut ii = 0usize;
        while ii < num && !out.has_dog_leg {
            let cpt = *control_points
                .get(ii)
                .ok_or_else(|| null("HasDogLegNearPole: control point index out of range"))?;

            // check for control points near constant U / V Singularity boundaries
            let (do_u_check, do_v_check) = on_sing(&cpt);

            // when control point is near singularity boundary
            if do_u_check || do_v_check {
                // for U and V checks
                for jj in 0..2 {
                    // skip check when control point is not near constant U / V poles
                    if jj == 0 && !do_u_check {
                        continue;
                    }
                    if jj == 1 && !do_v_check {
                        continue;
                    }

                    // when control point is an interior control point
                    if ii != 0 && ii + 1 < num {
                        let last_cpt = *control_points.get(ii - 1).ok_or_else(|| {
                            null("HasDogLegNearPole: control point index out of range")
                        })?;
                        let next_cpt = *control_points.get(ii + 1).ok_or_else(|| {
                            null("HasDogLegNearPole: control point index out of range")
                        })?;
                        let last_vec = cpt - last_cpt;
                        let next_vec = next_cpt - cpt;

                        // Angle in Control Polygon at target ControlPoint (a zero vector
                        // gives 0, as upstream)
                        let ang_vec_rad = last_vec.angle_between(next_vec).unwrap_or(0.0);

                        // Angles between ControlPolygon Next/Last Vecs and the
                        // singularity direction (upstream sets [1 0 0] for both checks)
                        let sing_vec = Vec3::new(1.0, 0.0, 0.0);
                        let ang_last_pole_rad = last_vec.angle_between(sing_vec).unwrap_or(0.0);
                        let ang_next_pole_rad = next_vec.angle_between(sing_vec).unwrap_or(0.0);

                        let ang_last_pole_deg = rad_to_deg(ang_last_pole_rad);
                        let ang_next_pole_deg = rad_to_deg(ang_next_pole_rad);

                        // when ControlPolygon turns and one CtrlPolygon segment runs along
                        // the singularity and one does not
                        if ang_vec_rad > deg_to_rad(50.0)
                            && (((is_contained(ang_last_pole_deg, 0.0, 10.0)
                                || is_contained(ang_last_pole_deg, 170.0, 180.0))
                                && is_contained(ang_next_pole_deg, 35.0, 145.0))
                                || ((is_contained(ang_next_pole_deg, 0.0, 10.0)
                                    || is_contained(ang_next_pole_deg, 170.0, 180.0))
                                    && is_contained(ang_last_pole_deg, 35.0, 145.0)))
                        {
                            // call it a turn and quit
                            out.has_dog_leg = true;
                            out.index = Some(ii);
                            break;
                        }
                    }
                }
            }
            ii += 1;
        }
    }

    // all done
    Ok(out)
}

/// Ignores a status upstream ignores ([`SolidError::Unported`] still propagates).
fn ignored<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{Edge, Edgeuse, EdgeuseOwner, Vertex};

    /// A B-rep with one edge holding a single loop-less edge use.
    fn lone_edgeuse() -> (Brep, EdgeuseId) {
        let mut b = Brep::new();
        let v = b
            .add_vertex_record(Vertex {
                point: Point3::new(0.0, 0.0, 0.0),
                tolerance: 0.0,
                ..Vertex::default()
            })
            .unwrap();
        let eu = EdgeuseId::from_index(0);
        let e = b
            .add_edge_record(Edge {
                vertices: [v, v],
                edgeuses: vec![eu],
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 1e-6,
                curve: None,
            })
            .unwrap();
        let id = b
            .add_edgeuse_record(Edgeuse {
                edge: Some(e),
                owner: EdgeuseOwner::Unknown,
                ..Edgeuse::default()
            })
            .unwrap();
        (b, id)
    }

    #[test]
    fn tangent_sector_without_edge_is_false() {
        let mut b = Brep::new();
        let eu = b.add_edgeuse_record(Edgeuse::default()).unwrap();
        assert!(!is_tangent_sector(&Kernel::default(), &mut b, eu, 1.0, false, true).unwrap());
    }

    #[test]
    fn convex_with_no_samples_is_true() {
        let (mut b, eu) = lone_edgeuse();
        assert!(is_convex_radial_sector(&Kernel::default(), &mut b, eu, 0, true, 0.0).unwrap());
    }

    #[test]
    fn convex_samples_reach_evaluate_binormal() {
        // an edge use that is not in a loop has no radial partner: upstream
        // dereferences it
        let (mut b, eu) = lone_edgeuse();
        let e = is_convex_radial_sector(&Kernel::default(), &mut b, eu, 3, false, 2.0);
        assert!(!matches!(e, Err(ref x) if x.is_unported()));
    }

    #[test]
    fn step_off_needs_a_loop_edge_use() {
        // EvaluateBinormal rejects an edge use that is not in a loop (SM_ERR_INVALID_INPUT)
        let (mut b, eu) = lone_edgeuse();
        let e = evaluate_binormal_step_off(
            &Kernel::default(),
            &mut b,
            eu,
            0.5,
            0.1,
            &StepOffOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(e, SolidError::Upstream { code: 1007, .. }));
    }

    #[test]
    fn rebuild_needs_a_face_use() {
        // CreateUVTrimCurve: NER(pFU)
        let (mut b, eu) = lone_edgeuse();
        let e = rebuild_uv_trim_curve(&Kernel::default(), &mut b, eu).unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }));
    }

    #[test]
    fn notify_pre_edit_changes_nothing() {
        let (mut b, eu) = lone_edgeuse();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::default();
        let this = ObjectRef {
            type_code: super::super::p1::TYPE,
            key: crate::geom::attribute::ObjectKey(7),
        };
        notify(
            &mut b,
            eu,
            &mut records,
            &mut attrs,
            None,
            NotifyOperation::PreEdit,
            this,
            [None, None, None],
        )
        .unwrap();
        assert!(b.edgeuse(eu).unwrap().uv_trim_curve.is_none());
    }

    #[test]
    fn check_and_set_needs_a_face() {
        let (mut b, eu) = lone_edgeuse();
        let c = Curve::from(crate::nurbs::b_spline_curve::BSplineCurve::new(2));
        let e = check_and_set_uv_trim_curve(&Kernel::default(), &mut b, eu, c, false, None)
            .unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }));
    }
}

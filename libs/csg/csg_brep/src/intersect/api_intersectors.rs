//! `SM_API` intersectors: curve/curve, surface/surface, face/face and B-rep/B-rep
//! intersections and the section of a B-rep by a plane.
//!
//! Upstream: `source/SM_API/src/SmApiIntersectors.cpp`, `source/SM_API/inc/SmApiIntersectors.h`.
//!
//! | Rust                                                  | Upstream                       |
//! |-------------------------------------------------------|--------------------------------|
//! | [`intersect_curves`], [`CurveIntersections`]          | `SmApiIntersectCurves`         |
//! | [`intersect_surfaces`]                                | `SmApiIntersectSurfaces`       |
//! | [`intersect_faces`], [`FaceBodies`], [`Intersections`] | `SmApiIntersectFaces`         |
//! | [`intersect_breps`], [`BrepOperands`]                 | `SmApiIntersectBreps`          |
//! | [`intersect_curve_surface`], [`CurveHits`]            | `SmApiIntersectCurveSurface`   |
//! | [`intersect_curve_face`]                              | `SmApiIntersectCurveFace`      |
//! | [`intersect_curve_brep`]                              | `SmApiIntersectCurveBrep`      |
//! | `append_curve_surface_hits` (private)                 | `sm_AppendCurveSurfaceHits`    |
//! | `has_own_curve_with_ends` (private)                   | `sm_HasOwnCurveWithEnds`       |
//! | [`intersect_brep_with_plane`]                         | `SmApiIntersectBrepWithPlane`  |
//!
//! Deviations: `SmApiGetOrCreateContext()` (and an object's `GetContext()`) is the
//! caller's [`Kernel`]. Objects are references, so upstream's `SM_ERR_INVALID_INPUT`
//! for a null object or output array has no counterpart; a pointer equality of the
//! two operands (`pCurve1 == pCurve2`, `pBrep1 == pBrep2`) is the operands' identity
//! ([`core::ptr::eq`], or [`BrepOperands::Same`] / [`FaceBodies::One`]). The results are
//! returned: upstream appends to (`SmApiIntersectCurves`,
//! `SmApiIntersectBrepWithPlane`) or fills the caller's arrays, which the caller does
//! with them. [`intersect_breps`] returns its outputs with the status, so the partial
//! outputs of a failed call are kept as upstream leaves them in the caller's arrays
//! (`SmApiIntersectFaces` deletes its own); [`intersect_surfaces`] likewise returns the
//! curves a failed `GlobalSurfaceIntersect` left in the caller's array with its status
//! ([`Surface::global_surface_intersect_with_partial`]). A failed
//! `EvaluatePoint` of an intersection point (status ignored upstream, which then adds
//! the previous point again) repeats the previous point here too, and
//! [`crate::math::Vec3::UNDEF`] before the first. The curve/surface, curve/face and
//! curve/B-rep calls return fresh [`CurveHits`] only on success: upstream appends to the
//! caller's arrays and on a failure removes what it appended (`sm_RestoreCurveSurfaceHits`),
//! so its "own" entries (`lFirstOwnCurve`, `lFirstOwnPoint`) are all of [`CurveHits`] here.
//! Their solve's solutions name the curve and the surface, whose surface domain is read
//! kernel-free (a lazy analytic's NURB-less base answers an empty domain). A face's
//! surface is solved as an evaluation copy (same owner and cache slot) and classified
//! through the B-rep's [`BrepFaceServices`], so it reaches the face's trimmed cache as
//! upstream's does through `GetFace()`; there the curve is a [`CurveFaceObject`] (a free
//! curve with its natural interval). `SM_ASSERT_VALID` and the
//! `SM_DEBUG_CODE` drawing are not ported.

use crate::curve::curve::Curve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::solution_array::{
    ObjectKind, SolutionArray, SolutionEnd, SolutionObject, SolutionType,
};
use crate::geom::tol::{zone_tol3d, USE_DEFAULT};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::point_class::PointClassType;
use crate::kernel::Kernel;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::query::brep::{BrepFaceServices, CurveFaceObject, CurveFaceServices};
use crate::surface::surface::Surface;
use crate::surface::surface_cache::face_context::FaceQueries;
use crate::topology::brep::planar_section::{
    create_planar_section_curves, PlanarSectionOptions, SectionOutputs,
};
use crate::topology::brep_cache::CacheObject;
use crate::topology::face::p3::{face_intersect_breps, FaceBreps, FaceIntersectOptions};
use crate::topology::Brep;
use crate::topology::FaceId;

const OP: &str = "intersect::api_intersectors::intersect_brep_with_plane";
const OP_CURVES: &str = "intersect::api_intersectors::intersect_curves";
const OP_SURFACES: &str = "intersect::api_intersectors::intersect_surfaces";
const OP_BREPS: &str = "intersect::api_intersectors::intersect_breps";
const OP_CURVE_SURFACE: &str = "intersect::api_intersectors::intersect_curve_surface";
const OP_CURVE_FACE: &str = "intersect::api_intersectors::intersect_curve_face";
const OP_CURVE_BREP: &str = "intersect::api_intersectors::intersect_curve_brep";

/// `SmApiIntersectCurves`' fixed tolerance (`dTol`).
const CURVE_TOLERANCE: f64 = 0.001;

/// `SmApiIntersectFaces`' and `SmApiIntersectBreps`' angle tolerance (`dAngTol`, degrees).
const ANGLE_TOLERANCE_DEG: f64 = 20.0;

/// The solution object of the unnamed curve solves here (their solutions name no
/// object).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Unnamed;

impl SolutionObject for Unnamed {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Other
    }
}

/// The outputs of [`intersect_curves`], one entry per solution.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CurveIntersections {
    /// `p3DPoints`: the points on the first curve at [`CurveIntersections::params1`]
    /// (when asked).
    pub points: Option<Vec<Point3>>,
    /// `parametersOnCurve1`: each solution's start parameter on the first curve.
    pub params1: Vec<f64>,
    /// `parametersOnCurve2`: each solution's start parameter on the second curve.
    pub params2: Vec<f64>,
}

/// `SmApiIntersectCurves`: the intersections of `curve1` and `curve2` over their natural
/// intervals within the fixed tolerance `0.001` (`GlobalCurveIntersect`), or the
/// self-intersections of `curve1` when both are the same curve
/// (`GlobalCurveSelfIntersect`). An overlap is reported by its start. With
/// `want_points` the points on `curve1` are evaluated too.
///
/// # Errors
///
/// `SM_ERR` when there is no intersection; those of the curve intersection.
pub fn intersect_curves(
    k: &Kernel,
    curve1: &Curve,
    curve2: &Curve,
    want_points: bool,
) -> Result<CurveIntersections> {
    let ivl1 = curve1.natural_interval();
    let solutions = if core::ptr::eq(curve1, curve2) {
        curve1.global_curve_self_intersect::<Unnamed>(k, &ivl1, CURVE_TOLERANCE, None, None)?
    } else {
        curve1.global_curve_intersect::<Unnamed>(
            k,
            &ivl1,
            curve2,
            &curve2.natural_interval(),
            CURVE_TOLERANCE,
            &crate::curve::curve::p8::CurveIntersectOptions::default(),
        )?
    };

    // If no intersections return error
    if solutions.is_empty() {
        return Err(SolidError::sm_err(OP_CURVES));
    }

    // Extract info from solution array
    let mut out = CurveIntersections {
        points: want_points.then(Vec::new),
        ..CurveIntersections::default()
    };
    let mut pt = Vec3::UNDEF;
    for sol in solutions.as_slice() {
        let t1 = sol.start.param(0)?;
        out.params1.push(t1);
        out.params2.push(sol.start.param(1)?);
        // get all points on curve1 (3d points on curve2 are the same)
        if let Some(points) = out.points.as_mut() {
            if let Some(p) = ignored(curve1.evaluate_point(k, t1))? {
                pt = p;
            }
            points.push(pt);
        }
    }
    Ok(out)
}

/// `SmApiIntersectSurfaces`: the 3D intersection curves of `surface1` and `surface2`
/// over their natural domains (`GlobalSurfaceIntersect`, both surfaces' edges used to
/// find start points, the default tolerances), with the status. On a failed
/// intersection the curves are those it left in the output array (a special-case
/// intersection's curves appended before its error,
/// [`Surface::global_surface_intersect_with_partial`]), as upstream leaves them in the
/// caller's array.
///
/// # Errors
///
/// In the status: `SM_ERR` when there is no intersection curve; those of the surface
/// intersection.
pub fn intersect_surfaces(
    k: &Kernel,
    surface1: &Surface,
    surface2: &Surface,
) -> (Vec<Curve>, Result<()>) {
    let domains = surface1
        .natural_uv_domain_with_nurb(k)
        .and_then(|d1| Ok((d1, surface2.natural_uv_domain_with_nurb(k)?)));
    let (domain1, domain2) = match domains {
        Ok(d) => d,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let (result, status) = surface1.global_surface_intersect_with_partial(
        k,
        &domain1,
        surface2,
        &domain2,
        [true, true],
        None,
        None,
    );
    // check result
    let status = status.and_then(|()| {
        if result.curves_3d.is_empty() {
            Err(SolidError::sm_err(OP_SURFACES))
        } else {
            Ok(())
        }
    });
    (result.curves_3d, status)
}

/// The curves and points of [`intersect_faces`] and [`intersect_breps`].
#[derive(Debug, Clone, Default)]
pub struct Intersections {
    /// The 3D intersection curves.
    pub curves: Vec<Curve>,
    /// The single-point intersections (when asked).
    pub points: Option<Vec<Point3>>,
}

/// The B-reps of the two faces of [`intersect_faces`].
#[derive(Debug)]
pub enum FaceBodies<'a> {
    /// Both faces are of this B-rep.
    One(&'a mut Brep),
    /// The first face's B-rep and the second face's.
    Two(&'a mut Brep, &'a mut Brep),
}

/// `SmApiIntersectFaces`: the 3D curves (and, with `want_points`, the single points)
/// where `face1` meets `face2` (`SmFace::FaceIntersect` at the first face's tolerance
/// and an angle tolerance of 20 degrees). No intersection is a success.
///
/// # Errors
///
/// Those of the face intersection (upstream then deletes the partial outputs).
pub fn intersect_faces(
    k: &Kernel,
    bodies: FaceBodies<'_>,
    face1: FaceId,
    face2: FaceId,
    want_points: bool,
) -> Result<Intersections> {
    let breps = match bodies {
        FaceBodies::One(b) => FaceBreps::One(b),
        FaceBodies::Two(b, other) => FaceBreps::Two(b, other),
    };
    let tol = match &breps {
        FaceBreps::One(b) | FaceBreps::Two(b, _) => crate::topology::face::p2::tolerance(b, face1)?,
    };
    let out = face_intersect_breps(
        k,
        breps,
        face1,
        face2,
        tol,
        ANGLE_TOLERANCE_DEG,
        &FaceIntersectOptions {
            want_points,
            ..FaceIntersectOptions::default()
        },
    )?;
    Ok(Intersections {
        curves: out.curves_3d,
        points: out.points_3d,
    })
}

/// The operands of [`intersect_breps`].
#[derive(Debug)]
pub enum BrepOperands<'a> {
    /// One B-rep intersected with itself (`pBrep1 == pBrep2`).
    Same(&'a mut Brep),
    /// Two B-reps.
    Two(&'a mut Brep, &'a mut Brep),
}

/// `SmApiIntersectBreps`: the 3D curves (and, with `want_points`, the singular
/// points) where the faces of the two B-reps meet (`SmBrep::BrepIntersect` at the first
/// B-rep's tolerance and an angle tolerance of 20 degrees), with the status. On a failed
/// face pair the outputs are those the earlier pairs left, as upstream leaves them in
/// the caller's arrays.
///
/// # Errors
///
/// In the status: `SM_ERR` when there is neither a curve nor (asked for) a point; those
/// of the B-rep intersection.
pub fn intersect_breps(
    k: &Kernel,
    breps: BrepOperands<'_>,
    want_points: bool,
) -> (Intersections, Result<()>) {
    let (curves, points, status) = match breps {
        BrepOperands::Same(b) => {
            let tol = b.tolerance();
            crate::topology::brep::p1::brep_intersect_self_with_partial(
                k,
                b,
                tol,
                ANGLE_TOLERANCE_DEG,
                want_points,
            )
        }
        BrepOperands::Two(b, other) => {
            let tol = b.tolerance();
            crate::topology::brep::p1::brep_intersect_mut_with_partial(
                k,
                b,
                other,
                tol,
                ANGLE_TOLERANCE_DEG,
                want_points,
            )
        }
    };
    // check result
    let status = status.and_then(|()| {
        if curves.is_empty() && points.as_ref().is_none_or(Vec::is_empty) {
            Err(SolidError::sm_err(OP_BREPS))
        } else {
            Ok(())
        }
    });
    (Intersections { curves, points }, status)
}

/// The solution objects of the curve/surface solve of [`intersect_curve_surface`],
/// [`intersect_curve_face`] and [`intersect_curve_brep`]: upstream's solutions name the
/// curve and the surface (`GlobalCurveIntersect`), whose natural interval and domain the
/// solver reads back (`SmSolution::GetParameterInterval`). Compares by identity, like a
/// pointer.
#[derive(Clone, Copy)]
enum Operand<'a> {
    Curve(&'a Curve),
    Surface(&'a Surface),
}

impl PartialEq for Operand<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Curve(a), Self::Curve(b)) => core::ptr::eq(*a, *b),
            (Self::Surface(a), Self::Surface(b)) => core::ptr::eq(*a, *b),
            _ => false,
        }
    }
}

impl SolutionObject for Operand<'_> {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Curve(_) => ObjectKind::Curve,
            Self::Surface(_) => ObjectKind::Surface,
        }
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        match self {
            Self::Curve(c) => Some(c.natural_interval()),
            Self::Surface(_) => None,
        }
    }

    fn natural_uv_domain(&self) -> Option<Extent2d> {
        match self {
            Self::Curve(_) => None,
            // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
            Self::Surface(s) => Some(s.natural_uv_domain_if_built()),
        }
    }
}

/// The curves and points of [`intersect_curve_surface`], [`intersect_curve_face`] and
/// [`intersect_curve_brep`].
#[derive(Debug, Clone, Default)]
pub struct CurveHits {
    /// The overlaps: copies of the curve trimmed to each range where it lies on the
    /// surface.
    pub curves: Vec<Curve>,
    /// The points where the curve meets the surface.
    pub points: Vec<Point3>,
}

/// One solution of the curve/surface solve: whether it is a range
/// (`SM_ST_RANGE_OF_VALUES`), its start and its end (`[curve t, surface u, surface v]`).
type CurveSurfaceSolution = (bool, SolutionEnd, SolutionEnd);

/// The solutions `sm_AppendCurveSurfaceHits` walks: `pSurface->GlobalCurveIntersect` of
/// `curve` over its natural interval and `surface` over `uv_domain`, within the fixed
/// tolerance `0.001`.
fn curve_surface_solutions(
    k: &Kernel,
    curve: &Curve,
    surface: &Surface,
    uv_domain: &Extent2d,
) -> Result<Vec<CurveSurfaceSolution>> {
    let solutions = surface.global_curve_intersect(
        k,
        [Operand::Curve(curve), Operand::Surface(surface)],
        uv_domain,
        curve,
        &curve.natural_interval(),
        CURVE_TOLERANCE,
    )?;
    Ok(solution_list(&solutions))
}

/// [`curve_surface_solutions`] of face `f`'s surface (`surface`, an evaluation copy of
/// it) over the face's UV domain, through the B-rep's face services: the surface reaches
/// its face's trimmed cache (`smsurf_GetSurfaceCache`) as upstream's does through
/// `GetFace()`.
fn curve_face_solutions(
    k: &Kernel,
    curve: &Curve,
    services: &BrepFaceServices<'_>,
    f: FaceId,
    surface: &Surface,
    uv_domain: &Extent2d,
) -> Result<Vec<CurveSurfaceSolution>> {
    let interval = curve.natural_interval();
    let solutions = surface.global_curve_intersect_with_context(
        k,
        [
            Some(CurveFaceObject::Curve(interval)),
            Some(CurveFaceObject::Brep(CacheObject::Surface(f))),
        ],
        None,
        &mut CurveFaceServices(services),
        uv_domain,
        curve,
        &interval,
        CURVE_TOLERANCE,
    )?;
    Ok(solution_list(&solutions))
}

/// Each solution's [`CurveSurfaceSolution`].
fn solution_list<O: SolutionObject>(solutions: &SolutionArray<O>) -> Vec<CurveSurfaceSolution> {
    solutions
        .as_slice()
        .iter()
        .map(|sol| {
            (
                sol.solution_type == SolutionType::RangeOfValues,
                sol.start,
                sol.end,
            )
        })
        .collect()
}

/// `sm_HasOwnCurveWithEnds`: whether one of `curves` (this call's own overlaps) starts
/// and ends within `tol` of `start` and `end`, either way round.
fn has_own_curve_with_ends(
    k: &Kernel,
    curves: &[Curve],
    start: Point3,
    end: Point3,
    tol: f64,
) -> Result<bool> {
    for curve in curves {
        let ivl = curve.natural_interval();
        let curve_start = curve.evaluate_point(k, ivl.min())?;
        let curve_end = curve.evaluate_point(k, ivl.max())?;
        if (curve_start.distance(start) <= tol && curve_end.distance(end) <= tol)
            || (curve_start.distance(end) <= tol && curve_end.distance(start) <= tol)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `sm_AppendCurveSurfaceHits` after the solve: appends to `hits` the overlaps (a copy of
/// `curve` trimmed to each range) and the points (`curve` at each point solution) of
/// `solutions`. With `face` (its B-rep's face services and the face), a solution whose
/// surface UV (a range's UV midpoint) the face classifies outside (`SM_PC_UNKNOWN`) is
/// dropped. With `skip_duplicates`, a point
/// within `0.001` of one already in `hits`, or a range whose ends match those of an
/// overlap already in `hits`, is skipped.
fn append_curve_surface_hits(
    k: &Kernel,
    curve: &Curve,
    solutions: &[CurveSurfaceSolution],
    face: Option<(&BrepFaceServices<'_>, FaceId)>,
    skip_duplicates: bool,
    hits: &mut CurveHits,
) -> Result<()> {
    let tol = CURVE_TOLERANCE;
    for &(range, start, end) in solutions {
        if let Some((services, f)) = face {
            let mut uv = Point2::new(start.param(1)?, start.param(2)?);
            if range {
                uv = Point2::new(
                    0.5 * (start.param(1)? + end.param(1)?),
                    0.5 * (start.param(2)? + end.param(2)?),
                );
            }
            // pFace->PointClassify(sUV, SmTol::GetZoneTol3d(pFace), FALSE, TRUE, sPC) of a
            // default SmPointClassification (the zone tolerance argument is not used:
            // the classification's own tolerance is the default one)
            let (class, _) = services.face_point_classify(
                k,
                CacheObject::Face(f),
                uv,
                zone_tol3d(k, USE_DEFAULT),
            )?;
            if class == PointClassType::Unknown.code() {
                continue;
            }
        }

        if range {
            if skip_duplicates {
                let range_start = curve.evaluate_point(k, start.param(0)?)?;
                let range_end = curve.evaluate_point(k, end.param(0)?)?;
                if has_own_curve_with_ends(k, &hits.curves, range_start, range_end, tol)? {
                    continue;
                }
            }

            let mut trimmed = curve.copy(k)?;
            let mut ivl = Extent1d::from_min_max(start.param(0)?, end.param(0)?);
            trimmed.trim(k, &mut ivl, TrimMode::Notify)?;
            hits.curves.push(trimmed);
            continue;
        }

        let pt = curve.evaluate_point(k, start.param(0)?)?;
        let duplicate = skip_duplicates && hits.points.iter().any(|p| p.distance(pt) <= tol);
        if !duplicate {
            hits.points.push(pt);
        }
    }
    Ok(())
}

/// `SM_ERR` when a call found neither an overlap nor a point.
fn found(hits: CurveHits, op: &'static str) -> Result<CurveHits> {
    if hits.points.is_empty() && hits.curves.is_empty() {
        return Err(SolidError::sm_err(op));
    }
    Ok(hits)
}

/// `SmApiIntersectCurveSurface`: where `curve` (over its natural interval) meets
/// `surface` (over its natural domain) within the fixed tolerance `0.001`
/// (`GlobalCurveIntersect`): the overlaps as copies of the curve trimmed to them, and the
/// points.
///
/// # Errors
///
/// `SM_ERR` when there is neither an overlap nor a point; those of the solve, of the
/// copies and trims and of the evaluations.
pub fn intersect_curve_surface(k: &Kernel, curve: &Curve, surface: &Surface) -> Result<CurveHits> {
    let solutions =
        curve_surface_solutions(k, curve, surface, &surface.natural_uv_domain_with_nurb(k)?)?;
    let mut hits = CurveHits::default();
    append_curve_surface_hits(k, curve, &solutions, None, false, &mut hits)?;
    found(hits, OP_CURVE_SURFACE)
}

/// `SmApiIntersectCurveFace`: [`intersect_curve_surface`] with the surface of face `f`
/// of `b` over the face's UV domain, keeping only the hits the face classifies inside or
/// on its boundary (an overlap by its UV midpoint; overlaps are not clipped to the face).
///
/// # Errors
///
/// `SM_ERR` for a face without surface or when there is neither an overlap nor a point;
/// those of the solve, of the face classification, of the copies and trims and of the
/// evaluations.
pub fn intersect_curve_face(
    k: &Kernel,
    curve: &Curve,
    b: &mut Brep,
    f: FaceId,
) -> Result<CurveHits> {
    // (an evaluation copy of the face's surface: same owner and cache slot)
    let surface = crate::topology::face::p2::surface(b, f)?
        .ok_or_else(|| SolidError::sm_err(OP_CURVE_FACE))?
        .clone_for_evaluation()?;
    let uv_domain = crate::topology::face::p2::uv_domain(b, f)?;
    let services = BrepFaceServices::new(b);
    let solutions = curve_face_solutions(k, curve, &services, f, &surface, &uv_domain)?;
    let mut hits = CurveHits::default();
    append_curve_surface_hits(k, curve, &solutions, Some((&services, f)), false, &mut hits)?;
    found(hits, OP_CURVE_FACE)
}

/// `SmApiIntersectCurveBrep`: [`intersect_curve_face`] over every face of `b` with a
/// surface (`GetFaces` order), each point or overlap reported once: a point within
/// `0.001` of an earlier one, or an overlap whose ends match an earlier one (a curve
/// crossing or lying along a shared edge), is skipped.
///
/// # Errors
///
/// `SM_ERR` when there is neither an overlap nor a point; those of
/// [`intersect_curve_face`] on any face (no partial result is returned).
pub fn intersect_curve_brep(k: &Kernel, curve: &Curve, b: &mut Brep) -> Result<CurveHits> {
    let faces = crate::topology::brep::p1::faces(b, None)?;
    // (one set of face services for the call: each face's cache is kept, as upstream's
    // on its surface)
    let services = BrepFaceServices::new(b);
    let mut hits = CurveHits::default();
    for f in faces {
        // (an evaluation copy of the face's surface: same owner and cache slot)
        let face_surface = services.with_brep(|b| {
            let Some(surface) = crate::topology::face::p2::surface(b, f)? else {
                return Ok(None);
            };
            Ok(Some((
                surface.clone_for_evaluation()?,
                crate::topology::face::p2::uv_domain(b, f)?,
            )))
        })?;
        let Some((surface, uv_domain)) = face_surface else {
            continue;
        };
        let solutions = curve_face_solutions(k, curve, &services, f, &surface, &uv_domain)?;
        append_curve_surface_hits(k, curve, &solutions, Some((&services, f)), true, &mut hits)?;
    }
    found(hits, OP_CURVE_BREP)
}

/// `SmApiIntersectBrepWithPlane`: the section curves of `b` by the infinite plane
/// through `plane_point` with normal `plane_normal` (any length; it is unitized):
/// `SmBrep::CreatePlanarSectionCurves` with the default tolerances, 3D curves only.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) for a zero normal; those of
/// [`create_planar_section_curves`].
pub fn intersect_brep_with_plane(
    k: &Kernel,
    b: &mut Brep,
    plane_point: Point3,
    plane_normal: Vec3,
) -> Result<Vec<Curve>> {
    let unit_normal = plane_normal
        .unitize()
        .map_err(|_| SolidError::invalid_input(OP, "zero plane normal"))?;

    // The kernel resets the array it is given, so collect into a local one (the caller
    // appends it, keeping any curves already in its array).
    let section = create_planar_section_curves(
        k,
        b,
        plane_point,
        unit_normal,
        &PlanarSectionOptions {
            outputs: SectionOutputs {
                curves_3d: true,
                ..SectionOutputs::default()
            },
            ..PlanarSectionOptions::default()
        },
    )?;
    Ok(section.curves_3d.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(points: &[[f64; 3]]) -> Curve {
        let points: Vec<Point3> = points
            .iter()
            .map(|p| Point3::new(p[0], p[1], p[2]))
            .collect();
        Curve::from(crate::primitive::api_curves::create_curve(&points).unwrap())
    }

    #[test]
    fn intersect_curves_reports_the_crossings_and_no_crossing_is_an_error() {
        // SM_API_test TestSmIntersectCurves
        let k = Kernel::default();
        let a = curve(&[[0., 0., 0.], [10., -3., 0.], [20., 3., 0.], [30., -6., 0.]]);
        let b = curve(&[[10., -5., 0.], [15., -3., 0.], [10., 3., 0.], [20., 6., 0.]]);
        let r = intersect_curves(&k, &a, &b, true).unwrap();
        assert!(!r.params1.is_empty());
        assert_eq!(r.params1.len(), r.params2.len());
        let points = r.points.unwrap();
        assert_eq!(points.len(), r.params1.len());
        for (p, t) in points.iter().zip(&r.params1) {
            assert_eq!(*p, a.evaluate_point(&k, *t).unwrap());
        }
        let far = curve(&[
            [0., 50., 0.],
            [10., 51., 0.],
            [20., 52., 0.],
            [30., 53., 0.],
        ]);
        let e = intersect_curves(&k, &a, &far, false).unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }), "{e:?}");
    }

    #[test]
    fn intersect_breps_of_overlapping_boxes_gives_curves() {
        // SM_API_test TestSmIntersectBreps (with a second box for speed)
        let mut k = Kernel::default();
        let mut a =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 5.0, 10.0, 15.0)
                .unwrap();
        let mut b = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(3.0, 5.0, 2.0),
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
        let (r, status) = intersect_breps(&k, BrepOperands::Two(&mut a, &mut b), true);
        status.unwrap();
        assert!(!r.curves.is_empty());
        assert!(r.points.is_some());
    }

    #[test]
    fn curve_through_a_box_meets_two_faces_and_misses_far_away() {
        // SM_API_test TestSmIntersectCurveBrep / CurveFace / CurveSurface
        let mut k = Kernel::default();
        let mut b =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 10.0, 10.0, 10.0)
                .unwrap();
        let line = Curve::from(
            crate::primitive::api_curves::create_line_segment(
                Point3::new(3.0, 4.0, -5.0),
                Point3::new(3.0, 4.0, 15.0),
            )
            .unwrap(),
        );
        let hits = intersect_curve_brep(&k, &line, &mut b).unwrap();
        assert!(hits.curves.is_empty());
        assert_eq!(hits.points.len(), 2, "{:?}", hits.points);
        for p in &hits.points {
            assert!((p.x - 3.0).abs() < 1e-9 && (p.y - 4.0).abs() < 1e-9);
        }

        let faces = crate::topology::brep::p1::faces(&b, None).unwrap();
        let on_faces: usize = faces
            .iter()
            .map(|&f| intersect_curve_face(&k, &line, &mut b, f).map_or(0, |h| h.points.len()))
            .sum();
        assert_eq!(on_faces, 2);

        let far = Curve::from(
            crate::primitive::api_curves::create_line_segment(
                Point3::new(30.0, 40.0, -5.0),
                Point3::new(30.0, 40.0, 15.0),
            )
            .unwrap(),
        );
        let e = intersect_curve_brep(&k, &far, &mut b).unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }), "{e:?}");
    }

    #[test]
    fn intersect_surfaces_returns_the_curves_with_the_status() {
        let k = Kernel::default();
        let frame = |z: f64| {
            let mut f = crate::geom::axis2_placement::Axis2Placement::default();
            f.set_canonical(Point3::new(0.0, 0.0, z), Vec3::X, Vec3::Y)
                .unwrap();
            f
        };
        let sphere = Surface::from(
            crate::surface::sphere::Sphere::create_canonical(&k, &frame(0.0), 2.0).unwrap(),
        );
        let plane = |z: f64| {
            Surface::from(crate::surface::plane::Plane::create_canonical(&frame(z)).unwrap())
        };
        let (curves, status) = intersect_surfaces(&k, &sphere, &plane(1.0));
        status.unwrap();
        assert!(!curves.is_empty());
        let (curves, status) = intersect_surfaces(&k, &sphere, &plane(5.0));
        assert!(
            matches!(status, Err(SolidError::Upstream { .. })),
            "{status:?}"
        );
        assert!(curves.is_empty());
    }

    #[test]
    fn zero_normal_is_invalid_input_and_length_does_not_matter() {
        let mut k = Kernel::default();
        let mut b =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 5.0, 10.0, 15.0)
                .unwrap();
        let at = Point3::new(0.0, 0.0, 7.0);
        let r = intersect_brep_with_plane(&k, &mut b, at, Vec3::ZERO);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        let unit = intersect_brep_with_plane(&k, &mut b, at, Vec3::Z).unwrap();
        let long = intersect_brep_with_plane(&k, &mut b, at, Vec3::new(0.0, 0.0, 5.0)).unwrap();
        assert_eq!(unit.len(), 4);
        assert_eq!(long.len(), unit.len());
    }
}

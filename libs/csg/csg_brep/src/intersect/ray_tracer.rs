//! The ray tracer: a voxel grid of surface subdivision leaves that rays are fired
//! into, and the local Newton solver that intersects a ray with one leaf.
//!
//! Upstream: `source/SMLib/src/SmRayTracer.cpp`, `source/SMLib/inc/SmRayTracer.h`.
//!
//! | Rust                                              | Upstream                                  |
//! |---------------------------------------------------|-------------------------------------------|
//! | [`LightSource`], [`LightSource::new`]             | `SmLightSource`, its constructor          |
//! | `Drop` (implicit)                                 | `~SmLightSource`                          |
//! | [`Lsi3LocalSolver`], [`Lsi3LocalSolver::new`]     | `SmLSI3LocalSolver`, its constructors     |
//! | [`Lsi3LocalSolver::local_solve`], [`LocalSolveResult`] | `SmLSI3LocalSolver::LocalSolve`      |
//! | [`RayTracer`], [`RayTracer::new`]                 | `SmRayTracer`, `SmRayTracer::SmRayTracer` |
//! | `Drop` (implicit)                                 | `~SmRayTracer`                            |
//! | [`RayTracer::set_solver_tolerance`]               | `SmRayTracer::SetSolverTolerance`         |
//! | [`RayTracer::fire_ray`], [`RayHit`]               | `SmRayTracer::FireRay`                    |
//! | [`RayTracer::add_surface_to_grid`], [`RayTracer::add_surface_with_bbox`], [`RayTracer::add_surface_with_fetched_cache`] | `SmRayTracer::AddSurfaceToGrid` |
//! | [`RayTracer::add_poly_brep_to_grid`]              | `SmRayTracer::AddPolyBrepToGrid`          |
//! | [`RayTracer::poly_brep`], [`RayTracer::poly_brep_count`] | `m_sPolyBreps`                     |
//! | [`RayRenderer`], [`RayRenderer::new`]               | `SmRayRenderer`, `SmRayRenderer::SmRayRenderer` |
//! | `Drop` (implicit)                                 | `~SmRayRenderer`                          |
//! | [`RayRenderer::do_render`], [`RayRenderer::render_pixel`], [`RayRenderer::render_ray`] | `SmRayRenderer::DoRender`, `RenderPixel`, `RenderRay` |
//! | [`RayRenderer::set_sample_rate`], [`RayRenderer::add_light_source`], [`RayRenderer::pixels_shaded`] | `SetSampleRate`, `AddLightSource`, `GetPixelsShaded` |
//! | [`RayObject`]                                   | the `SmObject*` of `SmSolution::m_apObjects` |
//! | [`SurfaceGridElement::from_leaf_data`], [`SurfaceGridElement::init`] | `SmSurfaceGridElement::Init` (ported in `geom::grid`) |
//! | `line_plane_intersect` (private)                  | `sm_LinePlaneIntersect`                   |
//! | `is_point_valid` (private)                        | `sm_IsPointValid`                         |
//! | `convert_to_parameter_delta` (private)            | `sm_ConvertToParameterDelta`              |
//! | `find_additional_tangent_guess` (private)         | `sm_FindAdditionalTangentGuess`           |
//! | `find_poly_hit` (private)                         | `sm_FindPolyHit`                          |
//!
//! Deviations:
//! - **Pointers.** The tracer owns its grid by value (`~SmRayTracer` deletes it);
//!   surfaces are borrowed and addressed by index into the tracer's surface list
//!   (`m_sSurfaces`, [`RayTracer::register_surface`]), which is what a surface grid
//!   element and a [`RayObject::Surface`] hold. The context reference
//!   (`m_crContext`) has no counterpart: it is only read by `GetMemoryUsed` and
//!   `Dump`.
//! - **Out-params** become return values: [`RayHit`] for `FireRay` (the solution
//!   starts as a default-constructed `SmSolution`, as every caller passes one;
//!   `rGridElements`/`rMinDistances`/`rMaxDistances` hold the last 3DDDA step's
//!   lists) and [`LocalSolveResult`] for `LocalSolve`. `LocalSolve`'s status is
//!   ignored by `FireRay`, but its outputs are used, so the status travels in
//!   [`LocalSolveResult::status`]; [`SolidError::Unported`] is always propagated.
//! - **Solution nodes.** `LocalSolve` stores the grid element pointer in
//!   `m_apNodes[0]` through an illegal cast that only `SmRayRenderer::RenderRay`
//!   reads; `FireRay` clears it. The Rust `LocalSolve` sets `nodes[0]` to a marker
//!   node (the element's surface index) so that "set" compares as upstream's pointer;
//!   nothing reads its value, and the solution `FireRay` returns has it empty.
//! - **Grid elements of other types.** `FireRay` asserts (debug builds only) that a
//!   non-polygon element is a surface element and casts it regardless; an element
//!   of another type (user data, hierarchy) is [`SolidError::InvalidInput`] here.
//! - **Surface leaf data.** `SmSurfaceGridElement::Init` lives in `geom::grid`
//!   ([`SurfaceGridElement::init`], and [`SurfaceGridElement::from_leaf_data`] for the
//!   part after the Bezier patch data). [`RayTracer::add_surface_to_grid`] makes the
//!   leaves as `ExpandHierarchy` does ([`Grid::expand_hierarchy`] over the surface's
//!   cache, which holds the surface by its index in the tracer's list); the
//!   `AddLeaf` test op fills a leaf in itself and adds it with
//!   [`Grid::add_surface_element`].
//! - **`SE` in `SmSurfaceGridElement::Init`.** Upstream signals an error and carries
//!   on; see the status deviation of `geom::grid` (a failed `Evaluate` is retried
//!   without `non_zero_tangents`, a failed `EvaluateNormal` keeps the cross product,
//!   [`SolidError::Unported`] is always propagated).
//! - **Poly B-reps.** The tracer holds each poly B-rep it was given mutably
//!   (`SmPolyFace::GetNormal` and `PointInPolygon` cache through `const_cast`
//!   upstream) together with the `SmBrepCache` that `AddPolyBrepToGrid` builds; the
//!   cache is owned by the tracer rather than the cache manager (the `SM_NO_PRE_EDIT`
//!   notification just dropped any cache the poly B-rep had, so `GetOrCreateObjectCache`
//!   always makes a new one). A polygon grid element holds its tree node, and the
//!   tracer maps the element to its poly B-rep; a [`RayObject::PolyFace`] names the
//!   poly B-rep by its index in the tracer's list. Upstream returns from
//!   `AddPolyBrepToGrid` with elements already in the grid when `AddElement` fails;
//!   the poly B-rep is then not listed, here as upstream, and a later ray reaching such
//!   an element is [`SolidError::InvalidInput`].
//! - **Marks.** `FireRay` takes a new `SM_MT_MARK` on the context of every poly B-rep
//!   (`SmNewMarkAndLock`, unlocked on return) and `sm_FindPolyHit` marks each poly face
//!   it tests with it; here each ray keeps the set of poly faces it has tested. (The
//!   loop meant to skip poly B-reps sharing a context `continue`s only its inner loop;
//!   a second new mark on the same context changes no face test.)
//! - **`sm_FindPolyHit` recursion** on the parent node, after the node's own faces,
//!   is a loop up the parent links in the same order, bounded by the tree's node count
//!   (a longer chain is a cycle: [`SolidError::InvalidInput`]).
//! - `GetMemoryUsed`, `Dump` and the drawing blocks are not reached by the upstream
//!   tests and not ported.
//! - **Renderer.** [`RayRenderer`] owns its tracer and light sources by value.
//!   `DoRender` returns the pixel colours (upstream keeps one row in a local and prints
//!   a progress line per row with `smos_WriteBuffer`, which is dropped); the colours
//!   upstream leaves unset before `DoRender` are black; the hit node `RenderRay`
//!   passes as the skip element is `m_apNodes[0]`, which `FireRay` always clears, so
//!   no element is skipped; unsigned products wrap as upstream's `ULONG`.
//! - **Unported callees.** `SmFace::PointClassify` goes through the caller's
//!   [`RayFaceClassifier`]; a face-owned surface traced without one is
//!   [`SolidError::Unported`] (`query.face_p2`, a dispatch gap). The surface cache and
//!   its tree (`smsurf_GetSurfaceCache`, `GetTree`), the poly types and the poly B-rep
//!   cache are ported.
//! - `SM_ERR` results the upstream ignores or `SER`s keep their behaviour; the
//!   `SM_DEBUG_CODE` blocks are not ported. No recursion depends on input here
//!   (`sm_FindPolyHit`'s is the bounded loop above).

use crate::curve::curve::p2::evaluate_geometric;
use crate::curve::curve::Curve;
use crate::curve::line::Line;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{
    self as gu, circle_coplanar_circle_intersect, line_closest_point,
    line_coplanar_circle_intersect, line_plane_intersect_uv, CircleCircle,
};
use crate::geom::grid::{
    ElementId, ElementSource, Grid, GridItem, HierarchyElement, HierarchyId, SurfaceGridElement,
};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject, SolutionType};
use crate::geom::tree::{NodeId, Tree};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, PI};
use crate::nurbs::knots::Side;
use crate::query::poly::{PolyBrep, PolyFaceId, PolyNotifyEnv};
use crate::surface::surface::p1::evaluate_1st_derivatives;
use crate::surface::surface::p2::surface_cache;
use crate::surface::surface::Surface;
use crate::surface::surface_cache::p1::SurfaceCache;
use crate::topology::brep_cache::{
    arena_identity, make_brep_cache, BrepCache, CacheObject, CacheSource,
};
use crate::topology::topology::TopologyRecords;
use crate::topology::{EdgeId, FaceId, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// A light source (`SmLightSource`): white light from a point or in a direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightSource {
    /// `m_vLightDirectionOrOrigin`: light position or direction.
    pub direction_or_origin: Vec3,
    /// `m_bIsPointLightSource`: true for a point source, false for diffuse light.
    pub is_point_light_source: bool,
}

impl LightSource {
    /// `SmLightSource(crLightDirOrOrigin, bIsPointLight)`.
    #[must_use]
    pub const fn new(direction_or_origin: Vec3, is_point_light_source: bool) -> Self {
        Self {
            direction_or_origin,
            is_point_light_source,
        }
    }
}

/// An object a ray solution refers to (`SmObject*` of `SmSolution::m_apObjects`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RayObject {
    /// A surface: its index in the tracer's surface list.
    Surface(usize),
    /// The face of the hit point (`SmFace`).
    Face(FaceId),
    /// The edge of the hit point (`SmEdge`).
    Edge(EdgeId),
    /// The vertex of the hit point (`SmVertex`).
    Vertex(VertexId),
    /// A poly face that was hit (`SmPolyFace`): the poly B-rep's index in the
    /// tracer's poly B-rep list and the face in it.
    PolyFace(usize, PolyFaceId),
}

impl SolutionObject for RayObject {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Surface(_) => ObjectKind::Surface,
            Self::Face(_) => ObjectKind::Face,
            Self::Edge(_) => ObjectKind::Edge,
            Self::Vertex(_) => ObjectKind::Vertex,
            Self::PolyFace(..) => ObjectKind::PolyFace,
        }
    }
}

/// A poly B-rep in the tracer (`m_sPolyBreps`) with the `SmBrepCache` that
/// `AddPolyBrepToGrid` built for it: its polygon grid elements are leaves of the
/// cache's surface tree.
#[derive(Debug)]
struct TracedPolyBrep<'s> {
    brep: &'s mut PolyBrep,
    cache: BrepCache,
}

/// The outputs of `SmSurface::EvaluateNormalSection`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalSection {
    /// `rTangentPlaneDirection`: the unit direction projected into the tangent plane.
    pub tangent_plane_direction: Vec3,
    /// `rdCurvatureValue`: the curvature of the surface curve in that direction.
    pub curvature: f64,
    /// `rSurfaceNormalVector`: the unit surface normal.
    pub normal: Vec3,
}

/// `SmSurface::EvaluateNormalSection` (`bUFromLeft`, `bVFromLeft` both true).
fn surface_evaluate_normal_section(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    direction: Vec3,
) -> Result<NormalSection> {
    let s = surface.evaluate_normal_section(k, uv, Side::Left, Side::Left, direction)?;
    Ok(NormalSection {
        tangent_plane_direction: s.tangent,
        curvature: s.curvature,
        normal: s.normal,
    })
}

/// `SmSurface::GetFace`: the face owning the surface, if any.
fn surface_face(surface: &Surface) -> Result<Option<ObjectRef>> {
    surface.face()
}

/// `sm_LinePlaneIntersect`: the line against the plane `plane_point + u * plane_u +
/// v * plane_v`; `Some((point, uv, line parameter))`, or `None` when the line is
/// parallel to the plane (upstream `SM_ERR`).
fn line_plane_intersect(
    line_point: Point3,
    line_vector: Vec3,
    plane_point: Point3,
    plane_u: Vec3,
    plane_v: Vec3,
) -> Option<(Point3, Point2, f64)> {
    let (t, u, v) =
        line_plane_intersect_uv(line_point, line_vector, plane_point, plane_u, plane_v).ok()?;
    Some((line_point + t * line_vector, Vec2::new(u, v), t))
}

/// Classifies a point of a face-owned surface against the surface's face
/// (`SmFace::PointClassify` of `sm_IsPointValid`), for a tracer whose surfaces are
/// faces of a B-rep the caller prepared.
pub trait RayFaceClassifier: core::fmt::Debug {
    /// Whether `uv` is a valid point of the tracer's surface `surface` (index in the
    /// tracer's list) and the topology object coincident with it. A classification
    /// error keeps the point (`TRUE`, no object), as upstream; [`SolidError::Unported`]
    /// is returned as an error.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the classification.
    fn is_point_valid(
        &self,
        k: &Kernel,
        surface: usize,
        uv: Point2,
        src_zone_tol: f64,
    ) -> Result<(bool, Option<RayObject>)>;
}

/// `sm_IsPointValid`: whether `uv` is a valid point of the surface (inside its face,
/// or its natural domain when it has no face), and the topology object coincident
/// with the point.
fn is_point_valid(
    k: &Kernel,
    surface: &Surface,
    index: usize,
    faces: Option<&dyn RayFaceClassifier>,
    uv: Point2,
    src_zone_tol: f64,
) -> Result<(bool, Option<RayObject>)> {
    match surface_face(surface)? {
        // no face - check uv against the natural domain
        None => Ok((
            surface
                .natural_uv_domain_with_nurb(k)?
                .contains_point(uv, 0.0),
            None,
        )),
        Some(_face) => match faces {
            Some(faces) => faces.is_point_valid(k, index, uv, src_zone_tol),
            None => {
                // DISPATCH GAP: SmFace::PointClassify (bare surface ray trace without a B-rep)
                Err(SolidError::unported(
                    "SmFace::PointClassify",
                    "query.face_p2",
                ))
            }
        },
    }
}

/// `sm_ConvertToParameterDelta`: the parameter step that moves along `tangent` so
/// that the projection of `delta` falls on the new point.
fn convert_to_parameter_delta(tangent: Vec3, delta: Vec3) -> f64 {
    let tan_len = tangent.length();
    if tan_len < EFF_ZERO {
        return 0.0;
    }
    let unit_tan = tangent / tan_len;
    let dot = unit_tan.dot(delta);
    dot / tan_len
}

/// The outputs of `sm_FindAdditionalTangentGuess`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TangentGuess {
    /// `rbFoundGuess`.
    found: bool,
    /// `rNewUVGuess`.
    uv: Point2,
    /// `rdNewTGuess`.
    t: f64,
}

/// `sm_FindAdditionalTangentGuess`: near a tangential ray/surface contact, a second
/// intersection guess from the circles of the surface normal section and the curve.
fn find_additional_tangent_guess(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    current_uv: Point2,
    current_t: f64,
) -> Result<TangentGuess> {
    let mut guess = TangentGuess {
        found: false,
        uv: Vec2::new(0.0, 0.0),
        t: 0.0,
    };
    let no_guess = guess;

    let mut normal = surface.evaluate_normal(k, current_uv, Side::Left, Side::Left)?;
    let pv = curve.evaluate(k, current_t, 1, true, true)?;
    let (pv0, pv1) = match pv.as_slice() {
        [a, b, ..] => (*a, *b),
        _ => return Err(SolidError::sm_err("intersect::ray_tracer")),
    };
    // First do a quick test to determin the angle of intersection between the
    // tangent plane of the surface ane the tangent of the curve.  If not within 20
    // degrees assume that we don't have a tangency touching situation.
    if !normal.is_perpendicular_to(pv1, 20.0) {
        return Ok(no_guess);
    }
    // Lets take a normal section on the surface along the direction of the tangent
    // of the curve.  This should give us two circles or a line and a circle to
    // intersect.
    let section = surface_evaluate_normal_section(k, surface, current_uv, pv1)?;
    let tan_plane_proj = section.tangent_plane_direction;
    let normal_curvature = section.curvature;
    normal = section.normal;

    // First compute the projected tangent vector.
    let surf_point = surface.evaluate_point(k, current_uv)?;

    // Get the geometric properties of the curve at the intersection
    let geom_vec = evaluate_geometric(k, curve, current_t, 2, true)?;
    let mut curvature_vector = *geom_vec
        .get(2)
        .ok_or_else(|| SolidError::sm_err("intersect::ray_tracer"))?;
    let curve_curvature = curvature_vector.length();
    if curve_curvature > EFF_ZERO {
        curvature_vector = curvature_vector.unitize()?;
    }

    // Compute the normal to the plane which is defined by the surface normal and
    // the projected vector.
    let projection_plane_normal = tan_plane_proj.cross(normal).unitize()?;

    // If the normal curvature is zero then use a line instead of a circle for the
    // surface. Also if the radius of one circle is greater then twice the other use
    // a line for the one with the larger radius.
    let line_circle_intersection;
    let mut circle1_radius;
    let mut circle2_radius = 0.0;
    let line_point;
    let mut circle1_center;
    let mut circle2_center = Vec3::new(0.0, 0.0, 0.0);
    let line_vec;
    let mut circle1_normal;
    let mut circle2_normal = Vec3::new(0.0, 0.0, 0.0);
    if normal_curvature.abs() < EFF_ZERO {
        // If the curve also has zero curvature then we are done
        if curve_curvature.abs() < EFF_ZERO {
            return Ok(no_guess); // Done with no new guesses needed
        }
        circle1_radius = (1.0 / curve_curvature).abs();
        circle1_center = pv0 + curvature_vector * circle1_radius;
        circle1_normal = pv1.cross(curvature_vector);
        if circle1_normal.length_squared() < EFF_ZERO_SQ {
            return Ok(no_guess);
        }
        circle1_normal = circle1_normal.unitize()?;
        line_point = surf_point;
        line_vec = tan_plane_proj.unitize()?;
        line_circle_intersection = true;
    } else {
        // have surface with curvature
        circle1_radius = (1.0 / normal_curvature).abs();
        normal = normal.unitize()?;
        circle1_center = surf_point + normal * (1.0 / normal_curvature);
        circle1_normal = projection_plane_normal;
        if curve_curvature.abs() < EFF_ZERO || (1.0 / curve_curvature).abs() > 2.0 * circle1_radius
        {
            line_point = pv0;
            line_vec = pv1.unitize()?;
            line_circle_intersection = true;
        } else {
            circle2_radius = (1.0 / curve_curvature).abs();
            if circle1_radius > 2.0 * circle2_radius {
                circle1_radius = circle2_radius;
                circle1_center = pv0 + curvature_vector * circle2_radius;
                circle2_normal = pv1.cross(curvature_vector);
                if circle2_normal.length_squared() < EFF_ZERO_SQ {
                    return Ok(no_guess);
                }
                line_point = surf_point;
                line_vec = tan_plane_proj.unitize()?;
                line_circle_intersection = true;
            } else {
                // nearly equal radii
                curvature_vector = curvature_vector.unitize()?;
                circle2_center = pv0 + curvature_vector * circle2_radius;
                circle2_normal = pv1.cross(curvature_vector);
                if circle2_normal.length_squared() < EFF_ZERO_SQ {
                    return Ok(no_guess);
                }
                line_point = Vec3::new(0.0, 0.0, 0.0);
                line_vec = Vec3::new(0.0, 0.0, 0.0);
                line_circle_intersection = false;
            }
        }
    }

    let found_vector = if line_circle_intersection {
        let params = line_coplanar_circle_intersect(
            line_point,
            line_vec,
            circle1_center,
            circle1_normal,
            circle1_radius,
            EFF_ZERO,
        )?;
        // If only have one or zero intersections we are done without picking up
        // extra guess
        let [p0, p1] = params.as_slice() else {
            return Ok(no_guess);
        };
        // Find the other intersection -- i.e. the one we don't have
        let mut line_t = *p0;
        if p1.abs() > p0.abs() {
            line_t = *p1;
        }
        line_t * line_vec
    } else {
        // Do circle circle intersection
        let hits = if circle1_radius > circle2_radius {
            circle_coplanar_circle_intersect(
                circle1_normal,
                circle1_center,
                circle1_radius,
                circle2_center,
                circle2_radius,
                EFF_ZERO,
            )?
        } else {
            circle_coplanar_circle_intersect(
                circle2_normal,
                circle2_center,
                circle2_radius,
                circle1_center,
                circle1_radius,
                EFF_ZERO,
            )?
        };
        // If only have one or zero intersections we are done without picking up
        // extra guess
        let CircleCircle::Crossing(t0, t1) = hits else {
            return Ok(no_guess);
        };
        let mut fv = t0 - surf_point;
        if fv.length() < t1.distance(surf_point) {
            fv = t1 - surf_point;
        }
        fv
    };

    // Use the found vector to compute an approximate step in the curve and surface
    // parameters.
    let (_point, du, dv) =
        evaluate_1st_derivatives(k, surface, current_uv, Side::Left, Side::Left)?;

    guess.found = true;
    guess.t = current_t + convert_to_parameter_delta(pv1, found_vector);
    guess.uv = Vec2::new(
        current_uv.x + convert_to_parameter_delta(du, found_vector),
        current_uv.y + convert_to_parameter_delta(dv, found_vector),
    );

    if !interval.contains_value(guess.t, 0.0) && !uv_domain.contains_point(guess.uv, 0.0) {
        // If the guess is outside of the domain we'll be able to pick it up using
        // another method.
        return Ok(no_guess);
    }
    guess.t = interval.clamp_value(guess.t);
    guess.uv = uv_domain.clamp_point(guess.uv);

    Ok(guess) // Found a guess - loaded it now we are done
}

/// The ray solver for one surface subdivision leaf (`SmLSI3LocalSolver`): a Newton
/// iteration from guesses taken off the leaf's mid-point tangent plane or corners.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members
pub struct Lsi3LocalSolver {
    /// `m_d3dTolerance`: near-miss tolerance, used for line intersections.
    pub d3d_tolerance: f64,
    /// `m_sUVDomain`.
    pub uv_domain: Extent2d,
    /// `m_vLinePoint`.
    pub line_point: Point3,
    /// `m_vLineVector`.
    pub line_vector: Vec3,
    /// `m_sLineInterval`.
    pub line_interval: Extent1d,
    /// `m_bRayFire`.
    pub ray_fire: bool,
    /// `m_dDistanceTolerance`: used for line intersections (and as a step-off distance
    /// by callers).
    pub distance_tolerance: f64,
    /// `m_dRayAccuracy`: convergence tolerance for line-surface intersections.
    pub ray_accuracy: f64,
    /// `m_bHaveTMinTMax`.
    pub have_t_min_t_max: bool,
    /// `m_dTMinOfBBox`.
    pub t_min_of_bbox: f64,
    /// `m_dTMaxOfBBox`.
    pub t_max_of_bbox: f64,
}

impl Default for Lsi3LocalSolver {
    /// `SmLSI3LocalSolver()`: tolerance 0 and the line interval `[0, 1e10]`; the
    /// members upstream leaves uninitialised are zero.
    fn default() -> Self {
        Self {
            d3d_tolerance: 0.0,
            uv_domain: Extent2d::new(),
            line_point: Vec3::new(0.0, 0.0, 0.0),
            line_vector: Vec3::new(0.0, 0.0, 0.0),
            line_interval: Extent1d::from_raw(0.0, 1.0e10),
            ray_fire: false,
            distance_tolerance: 0.0,
            ray_accuracy: 0.0,
            have_t_min_t_max: false,
            t_min_of_bbox: 0.0,
            t_max_of_bbox: 0.0,
        }
    }
}

/// What [`Lsi3LocalSolver::local_solve`] returns.
#[derive(Debug, Clone)]
pub struct LocalSolveResult {
    /// The returned status: `Ok` for `SM_SUCCESS`. The outputs below are valid
    /// whatever it is (`FireRay` ignores the status but reads them).
    pub status: Result<()>,
    /// `rbNeedsMoreSubdivision`: always false.
    pub needs_more_subdivision: bool,
    /// `rbFoundIntersection`: a valid point was found.
    pub found_intersection: bool,
    /// `rSolution`: when `found_intersection`, `start` holds the ray parameter, the
    /// surface `uv`, the 3D point and the normal; `objects` the surface and the
    /// coincident topology object.
    pub solution: Solution<RayObject>,
}

impl Lsi3LocalSolver {
    /// `SmLSI3LocalSolver(pSurfaceTree, crUVDomain, crLinePoint, crLineVector,
    /// cpLineInterval, bRayFire, dDistanceTolerance, dRayAccuracy)` (the unused
    /// tree argument is dropped).
    #[must_use]
    pub fn new(
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        ray_fire: bool,
        distance_tolerance: f64,
        ray_accuracy: f64,
    ) -> Self {
        Self {
            d3d_tolerance: 0.0,
            uv_domain: *uv_domain,
            line_point,
            line_vector,
            line_interval: line_interval.map_or(Extent1d::from_raw(0.0, 1.0e10), |i| *i),
            ray_fire,
            distance_tolerance,
            ray_accuracy,
            have_t_min_t_max: false,
            t_min_of_bbox: 0.0,
            t_max_of_bbox: 0.0,
        }
    }

    /// A ray/leaf intersection by Newton-Raphson (`LocalSolve`): the intersection of
    /// the line with `surface` over the leaf `element` with the smallest line
    /// parameter that lies in the line interval and is a valid surface point.
    #[must_use]
    pub fn local_solve(
        &self,
        k: &Kernel,
        surface: &Surface,
        element: &SurfaceGridElement<usize>,
        faces: Option<&dyn RayFaceClassifier>,
    ) -> LocalSolveResult {
        let mut out = LocalSolveResult {
            status: Ok(()),
            needs_more_subdivision: false,
            found_intersection: false,
            solution: Solution::new(),
        };
        out.status = self.local_solve_into(k, surface, element, faces, &mut out);
        out
    }

    fn local_solve_into(
        &self,
        k: &Kernel,
        surface: &Surface,
        element: &SurfaceGridElement<usize>,
        faces: Option<&dyn RayFaceClassifier>,
        out: &mut LocalSolveResult,
    ) -> Result<()> {
        // locals
        let mid_point = element.mid_point;
        let mid_du = element.mid_du;
        let mid_dv = element.mid_dv;
        let mid_normal = element.mid_normal;

        // get patch UVDomain, regular BBox, PolarBox and PseudoBox
        let uv_domain = &element.uv_domain;
        let polar_box = &element.polar_box;
        let mut pseudo_box = element.pseudo_box;
        pseudo_box.expand_absolute(self.distance_tolerance);

        let mut uv_point_delta = Vec2::new(0.0, 0.0);
        let mut point_3d = Vec3::new(0.0, 0.0, 0.0);

        // Get PatchMidNormal/LineVector dot product - to measure angle
        let dot_norm = self.line_vector.dot(mid_normal);

        // when PatchMidPoint/LineVector is large( more than 60) - use Corner Guess Points
        let mut use_corners = dot_norm.abs() < 0.4;
        // get ray/MidTangentPlane intersection
        match line_plane_intersect(self.line_point, self.line_vector, mid_point, mid_du, mid_dv) {
            Some((p, uv_delta, _t)) => {
                point_3d = p;
                uv_point_delta = uv_delta;
            }
            None => use_corners = true,
        }

        // when using corners - let 3DGuessPoint = Closest LinePoint to patchMidPoint
        if use_corners {
            let line_param = line_closest_point(self.line_point, self.line_vector, mid_point);
            point_3d = self.line_point + line_param * self.line_vector;
        }

        // when the 3DGuessPoint is not in the pseudoBox try to eliminate intersection
        // possibility by looking at the psuedoBoxPlane/Line intersections and quitting
        // when no intersection can be found inside both the bounding and pseudo box
        // of the node.
        if !pseudo_box.contains_point(point_3d, 0.0) {
            // If the point is not in the pseudo box, check whether the ray intersects
            // it anywhere (the status is ignored: no intersections).
            let num_ints = pseudo_box
                .intersect_line(self.line_point, self.line_vector)
                .map_or(0, |hit| hit.count());

            // If no intersection found just return here
            if num_ints == 0 {
                return Ok(());
            }
        }

        // Arrive here when we need to look for a line/patch intersection using
        // numerical methods.
        let d3d_accuracy = if self.ray_accuracy <= EFF_ZERO / 10.0 {
            EFF_ZERO * 100.0 * (1.0 + point_3d.max_dimension())
        } else {
            self.ray_accuracy
        };

        // Get Line/PatchMidPoint intersection UVPoint in patch coordinates
        let mut uv_point = uv_domain.evaluate(0.5, 0.5) + uv_point_delta;

        // set ray interval
        let mut tmp_ivl = Extent1d::from_raw(-1.0e10, 1.0e10);
        if !self.line_interval.is_init() {
            tmp_ivl = self.line_interval;
        }

        // NR Solver locals
        let mut min_t_found = BIG_DOUBLE;

        // Add the first GuessPoint to the guess Point array
        let mut uv_points: Vec<f64> = vec![uv_point.x, uv_point.y];
        let mut total_points = 1_usize;

        // when needed - add in the corner points to the guess point array
        if use_corners {
            for (nx, ny) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                uv_point = uv_domain.evaluate(nx, ny);
                uv_points.push(uv_point.x);
                uv_points.push(uv_point.y);
            }
            total_points = 5;
        }

        let orig_points = total_points;

        // For every guess point - call the intersector to seek a solution.
        let mut ii = 0_usize;
        while ii < total_points {
            let (Some(&gu), Some(&gv)) = (uv_points.get(ii * 2), uv_points.get(ii * 2 + 1)) else {
                return Err(SolidError::sm_err("intersect::ray_tracer::local_solve"));
            };
            uv_point = Vec2::new(gu, gv);

            // SmSurface::LocalLineIntersect (SER)
            let hit = surface.local_line_intersect(
                k,
                uv_domain,
                self.line_point,
                self.line_vector,
                &tmp_ivl,
                self.d3d_tolerance,
                d3d_accuracy,
                uv_point,
            )?;

            // skip to next guess when no intersection was found
            if !hit.found {
                ii += 1;
                continue;
            }

            // save solution with lowest in-range ray parameter value
            if hit.t < min_t_found && self.line_interval.contains_value(hit.t, 0.0) {
                // see if point is valid (within surface trim boundaries)
                let (valid, object) = is_point_valid(
                    k,
                    surface,
                    element.surface,
                    faces,
                    hit.uv,
                    self.d3d_tolerance,
                )?;
                if valid {
                    // save the solution
                    out.found_intersection = true;
                    min_t_found = hit.t;
                    let sol = &mut out.solution;
                    sol.num_objects = 1;
                    for (slot, o) in sol
                        .objects
                        .iter_mut()
                        .zip([Some(RayObject::Surface(element.surface)), object])
                    {
                        *slot = o;
                    }
                    sol.num_variables = 9;
                    let values = [
                        hit.t,
                        hit.uv.x,
                        hit.uv.y,
                        hit.point.x,
                        hit.point.y,
                        hit.point.z,
                        hit.normal.x,
                        hit.normal.y,
                        hit.normal.z,
                    ];
                    for (slot, v) in sol.start.parameters.iter_mut().zip(values) {
                        *slot = v;
                    }
                    sol.start.solution_value = hit.deviation;
                    sol.solution_type = SolutionType::SingleValue;
                    // the grid element pointer upstream stores through an illegal
                    // cast (see the module header); only "set" matters
                    sol.nodes[0] = Some(NodeId(element.surface));
                }
            }

            // Try to get done right here.  If we have an intersection then we can
            // look at the vector map of the surface and try to eliminate the
            // possibility of additional intersections.
            let line_vec = self.line_vector;

            // If the angle is less than 45 degrees than we can quit here with out
            // additional work to test for more tangent guesses.
            let mut angle = line_vec.angle_between(hit.normal)?;
            if angle > PI / 2.0 {
                angle = PI - angle;
            }
            if angle < 60.0 * PI / 180.0 {
                return Ok(());
            }

            let line_vec = line_vec.unitize()?;
            if !polar_box.has_perpendicular_to_vector(line_vec) {
                return Ok(());
            }

            // If we have found an intersection look closely at it and see if there
            // might be another intersection very close to it.  If there is compute a
            // guess point for that other intersection and add it to the list of
            // guesses. This should pick up any near tangent cases - like ray firing
            // just inside of a silhouette.
            if ii < orig_points {
                let tmp_line =
                    Curve::from(Line::infinite(self.line_point, self.line_vector, 3, false));
                let guess = find_additional_tangent_guess(
                    k, surface, uv_domain, &tmp_line, &tmp_ivl, hit.uv, hit.t,
                )?;
                if guess.found {
                    total_points += 1;
                    uv_points.push(guess.uv.x);
                    uv_points.push(guess.uv.y);
                }
            }
            ii += 1;
        }
        Ok(())
    }
}

/// What [`RayTracer::fire_ray`] returns.
#[derive(Debug, Clone)]
pub struct RayHit {
    /// `rbRayHitsSomething`.
    pub hits_something: bool,
    /// `rSolution`: `start[0]` is the ray parameter, `start[1..3]` the surface `uv`
    /// (`BIG_DOUBLE` for poly faces), `start[3..6]` the 3D point, `start[6..9]` the
    /// normal; `objects[0]` the surface and `objects[1]` the associated topology
    /// object (a face, edge or vertex, or none).
    pub solution: Solution<RayObject>,
    /// `rGridElements`: the elements whose boxes the ray crosses in the last voxel
    /// walked, ordered by ray parameter.
    pub grid_elements: Vec<ElementId>,
    /// `rMinDistances`: the entry parameter of each of those boxes.
    pub min_distances: Vec<f64>,
    /// `rMaxDistances`: the exit parameter of each of those boxes.
    pub max_distances: Vec<f64>,
}

/// The ray tracer (`SmRayTracer`): a grid of surface subdivision leaves and the
/// solver that intersects rays with them.
#[derive(Debug)]
pub struct RayTracer<'s> {
    /// `m_dRayTolerance`.
    ray_tolerance: f64,
    /// `m_dRayAccuracy`.
    ray_accuracy: f64,
    /// `m_pGrid`: the voxel model.
    grid: Grid<usize>,
    /// `m_sLSISolver`: the ray/leaf solver.
    solver: Lsi3LocalSolver,
    /// `m_sSurfaces`: the surfaces added to the grid.
    surfaces: Vec<&'s Surface>,
    /// `m_sPolyBreps`: the poly B-reps added to the grid, with their caches.
    poly_breps: Vec<TracedPolyBrep<'s>>,
    /// The poly B-rep (index in `poly_breps`) whose cache tree holds the node of
    /// each polygon grid element (upstream reaches the tree through the node).
    poly_elements: BTreeMap<ElementId, usize>,
    /// `m_pSkipElement`: an element `FireRay` does not test.
    pub skip_element: Option<ElementId>,
    /// How a face-owned surface's points are classified against its face.
    faces: Option<&'s dyn RayFaceClassifier>,
    /// `m_bHitAnyThing`: stop at the first hit instead of finding the closest.
    pub hit_any_thing: bool,
}

impl<'s> RayTracer<'s> {
    /// `SmRayTracer(crContext, pGrid, dRayTolerance, dRayAccuracy)`: takes ownership
    /// of `grid`.
    #[must_use]
    pub fn new(grid: Grid<usize>, ray_tolerance: f64, ray_accuracy: f64) -> Self {
        let solver = Lsi3LocalSolver {
            uv_domain: Extent2d::from_points(
                Vec2::new(-BIG_DOUBLE, -BIG_DOUBLE),
                Vec2::new(BIG_DOUBLE, BIG_DOUBLE),
            ),
            have_t_min_t_max: true,
            ray_fire: true,
            distance_tolerance: ray_tolerance,
            ray_accuracy,
            // SetMinMax(0.0, 1.0E10) cannot fail
            line_interval: Extent1d::from_raw(0.0, 1.0e10),
            ..Lsi3LocalSolver::default()
        };
        Self {
            ray_tolerance,
            ray_accuracy,
            grid,
            solver,
            surfaces: Vec::new(),
            poly_breps: Vec::new(),
            poly_elements: BTreeMap::new(),
            faces: None,
            skip_element: None,
            hit_any_thing: false,
        }
    }

    /// The grid (`GetGrid`).
    #[must_use]
    pub const fn grid(&self) -> &Grid<usize> {
        &self.grid
    }

    /// The grid, mutably (`GetGrid`).
    pub fn grid_mut(&mut self) -> &mut Grid<usize> {
        &mut self.grid
    }

    /// The ray solver (`m_sLSISolver`).
    #[must_use]
    pub const fn solver(&self) -> &Lsi3LocalSolver {
        &self.solver
    }

    /// `m_dRayTolerance`.
    #[must_use]
    pub const fn ray_tolerance(&self) -> f64 {
        self.ray_tolerance
    }

    /// `m_dRayAccuracy`.
    #[must_use]
    pub const fn ray_accuracy(&self) -> f64 {
        self.ray_accuracy
    }

    /// `SetSolverTolerance`: the solver's near-miss tolerance.
    pub fn set_solver_tolerance(&mut self, tol: f64) {
        self.solver.d3d_tolerance = tol;
    }

    /// `m_sSurfaces.Add(pSurface)`: records `surface` and returns its index, the
    /// handle surface grid elements and solution objects use.
    pub fn register_surface(&mut self, surface: &'s Surface) -> usize {
        self.surfaces.push(surface);
        self.surfaces.len() - 1
    }

    /// Sets how the points of face-owned surfaces are classified against their faces
    /// (`SmFace::PointClassify` of `sm_IsPointValid`).
    pub fn set_face_classifier(&mut self, faces: &'s dyn RayFaceClassifier) {
        self.faces = Some(faces);
    }

    /// The surfaces added so far (`m_sSurfaces`).
    #[must_use]
    pub fn surfaces(&self) -> &[&'s Surface] {
        &self.surfaces
    }

    /// The number of poly B-reps added so far (`m_sPolyBreps.GetSize()`).
    #[must_use]
    pub fn poly_brep_count(&self) -> usize {
        self.poly_breps.len()
    }

    /// The poly B-rep at `index` in the tracer's poly B-rep list (`m_sPolyBreps`),
    /// the index a [`RayObject::PolyFace`] holds.
    #[must_use]
    pub fn poly_brep(&self, index: usize) -> Option<&PolyBrep> {
        self.poly_breps.get(index).map(|p| &*p.brep)
    }

    /// Intersects a ray with the geometry in the grid (`FireRay`): the nearest hit
    /// (any hit when [`RayTracer::hit_any_thing`]) with ray parameter in
    /// `[-EFF_ZERO, dMaxRayParameter)`; use `BIG_DOUBLE` for an infinite ray.
    ///
    /// # Errors
    ///
    /// Those of the grid walk and of the poly face tests; [`SolidError::Unported`] from
    /// the solver; [`SolidError::InvalidInput`] for a grid element that is neither a
    /// surface nor a polygon element, whose surface is not registered or whose poly
    /// B-rep is not listed.
    pub fn fire_ray(
        &mut self,
        k: &Kernel,
        ray_point: Point3,
        ray_vector: Vec3,
        max_ray_parameter: f64,
    ) -> Result<RayHit> {
        const OP: &str = "intersect::ray_tracer::RayTracer::fire_ray";
        // init output
        let mut result = RayHit {
            hits_something: false,
            solution: Solution::new(),
            grid_elements: Vec::new(),
            min_distances: Vec::new(),
            max_distances: Vec::new(),
        };

        // Use pPolyBrep->Context->Mark as a tag for new ray-firing: one new mark per
        // ray for every poly face (the faces this ray has tested)
        let mut marked: BTreeSet<(usize, PolyFaceId)> = BTreeSet::new();

        // Init Grid scratch ray walking data with first Voxel/Ray intersection
        let hits_nothing = self.grid.setup_3ddda(ray_point, ray_vector)?;

        // Quit when ray hits no voxels
        if hits_nothing {
            return Ok(result);
        }
        let mut hit_t = max_ray_parameter;

        // traverse ray from voxel to voxel looking for 1st GridElement Intersection.
        // Stop when ray intersects an element or when ray exits grid.
        loop {
            // move to the next voxel that contains any not yet marked element whose
            // box the ray crosses
            let step = self.grid.step_3ddda(BIG_DOUBLE)?;
            result.grid_elements = step.elements;
            result.min_distances = step.min_distances;
            result.max_distances = step.max_distances;
            let exits_grid = step.exited_grid;

            // For every GridElement found in the current Voxel
            for (ii, &id) in result.grid_elements.iter().enumerate() {
                let (Some(&min_d), Some(&max_d)) =
                    (result.min_distances.get(ii), result.max_distances.get(ii))
                else {
                    return Err(SolidError::InvalidInput {
                        operation: OP,
                        reason: "grid step lists differ in length",
                    });
                };
                // skip hits which come after current hit pramameter value
                if min_d > hit_t {
                    continue;
                }

                // skip hits on the current SkipNode
                if self.skip_element == Some(id) {
                    continue;
                }

                let item = self.grid.element(id)?.item();

                // handle PolyBrep Faces
                let leaf = match item {
                    GridItem::Polygon(node) => {
                        // get SmGridElement's SmTreeNode - PolyBreps store SmTreeNodes in
                        // SmGridElements
                        let index =
                            *self
                                .poly_elements
                                .get(&id)
                                .ok_or(SolidError::InvalidInput {
                                    operation: OP,
                                    reason: "polygon grid element of no listed poly B-rep",
                                })?;
                        let traced =
                            self.poly_breps
                                .get_mut(index)
                                .ok_or(SolidError::InvalidInput {
                                    operation: OP,
                                    reason: "polygon grid element of no listed poly B-rep",
                                })?;
                        let tree = traced
                            .cache
                            .surface_tree()
                            .ok_or(SolidError::null_pointer(OP))?;

                        // Handle polygon ray firing
                        let hit = find_poly_hit(
                            k,
                            tree,
                            traced.brep,
                            node,
                            PolyRay {
                                point: ray_point,
                                vector: ray_vector,
                                hit_any_thing: self.hit_any_thing,
                            },
                            hit_t,
                            &mut |face| marked.insert((index, face)),
                        )?;
                        if let Some((t, hit_face)) = hit {
                            hit_t = t;
                            let hit_pnt = ray_point + ray_vector * hit_t;
                            let (hit_normal, _) =
                                traced.brep.face_normal(k, hit_face, false, true)?;
                            let out = &mut result.solution;
                            out.solution_type = SolutionType::SingleValue;
                            out.num_variables = 1;
                            out.start.parameters[0] = hit_t; // ray parameter
                            out.start.parameters[1] = BIG_DOUBLE;
                            out.start.parameters[2] = BIG_DOUBLE;
                            out.start.parameters[3] = hit_pnt.x; // ray/face intersection point
                            out.start.parameters[4] = hit_pnt.y;
                            out.start.parameters[5] = hit_pnt.z;
                            out.start.parameters[6] = hit_normal.x; // face normal at xsect
                            out.start.parameters[7] = hit_normal.y;
                            out.start.parameters[8] = hit_normal.z;
                            out.num_objects = 1;
                            out.objects[0] = Some(RayObject::PolyFace(index, hit_face));
                            out.objects[1] = Some(RayObject::PolyFace(index, hit_face));
                            result.hits_something = true;
                            if self.hit_any_thing {
                                break;
                            }
                        }
                        continue;
                    }
                    GridItem::Surface(leaf) => leaf,
                    GridItem::Hierarchy(_) | GridItem::User(_) => {
                        return Err(SolidError::InvalidInput {
                            operation: OP,
                            reason: "grid element is neither a surface nor a polygon element",
                        });
                    }
                };

                // handle Brep Surfaces

                // init solver parameters
                self.solver.line_point = ray_point;
                self.solver.line_vector = ray_vector;
                self.solver.t_min_of_bbox = min_d;
                self.solver.t_max_of_bbox = max_d;

                // skip ElementBBox/Ray intersections that happen before the ray Start
                // Point
                if self.solver.t_max_of_bbox < -EFF_ZERO {
                    continue;
                }

                // solve for a ray/GridElementGeometry intersection
                let surface = *self
                    .surfaces
                    .get(leaf.surface)
                    .ok_or(SolidError::InvalidInput {
                        operation: OP,
                        reason: "surface grid element refers to an unregistered surface",
                    })?;
                let solved = self.solver.local_solve(k, surface, &leaf, self.faces);
                // (the status is ignored; Unported always propagates)
                if let Err(e) = &solved.status {
                    if e.is_unported() {
                        return Err(e.clone());
                    }
                }
                let found = solved.found_intersection;
                let sol = solved.solution;

                // when an intersection was found that happens before any others and
                // is in the positive part of the ray
                let t = sol.start.parameters.first().copied().unwrap_or(0.0);
                if found && t < hit_t && t > -EFF_ZERO {
                    // put the solution in the output
                    hit_t = t;
                    let out = &mut result.solution;
                    out.start.solution_value = sol.start.solution_value;
                    out.solution_type = sol.solution_type;

                    // Put T parameter of line in 0, UV Point in 1-2, 3D point in 3-5
                    // and 3D Normal into 6-8
                    for (slot, v) in out
                        .start
                        .parameters
                        .iter_mut()
                        .zip(sol.start.parameters.iter())
                        .take(9)
                    {
                        *slot = *v;
                    }

                    out.nodes[0] = None; // GWC no longer set as output
                    out.num_objects = 2;
                    out.objects[0] = sol.objects[0]; // surface pointer
                    out.objects[1] = sol.objects[1]; // face pointer or NULL

                    result.hits_something = true;
                    // If we are looking for any hit just stop here otherwise make
                    // sure we get the closest hit.
                    if self.hit_any_thing {
                        break;
                    }
                } // end found a keeper intersection check
            } // end iter every found Grid Element

            // Stop when ray exits grid
            if exits_grid {
                break;
            }

            // Stop when ray hit an element
            if hit_t < self.grid.dda().ray_t {
                break;
            }
        } // end while(TRUE) stepping from voxel to voxel

        Ok(result)
    }

    /// Adds a surface to the grid as a hierarchy element that expands into its
    /// subdivision leaves (`AddSurfaceToGrid`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the surface cache or its tree is missing (or has no top node),
    /// and those of the cache and tree builds, of
    /// [`RayTracer::add_surface_with_bbox`] and of [`Grid::expand_hierarchy`].
    pub fn add_surface_to_grid(&mut self, k: &Kernel, surface: &'s Surface) -> Result<()> {
        // build the surface spatial decomposition by building the surface cache; its
        // nodes refer to the surface (`mBA_pSurface`) by the index the tracer lists it
        // under below
        let cache = surface_cache::<usize>(k, surface, Some(self.surfaces.len()))?;
        self.add_surface_with_cache(k, surface, cache)
    }

    /// [`RayTracer::add_surface_to_grid`] for a caller that built the surface's cache
    /// itself, as upstream's `smsurf_GetSurfaceCache` does for a face-owned surface
    /// through its face. `cache` was built with the surface handle
    /// `self.surfaces().len()`.
    ///
    /// # Errors
    ///
    /// As [`RayTracer::add_surface_to_grid`] after the cache build.
    pub fn add_surface_with_cache(
        &mut self,
        k: &Kernel,
        surface: &'s Surface,
        mut cache: SurfaceCache<usize>,
    ) -> Result<()> {
        const OP: &str = "intersect::ray_tracer::add_surface_with_cache";
        let top = cache
            .get_tree(k)?
            .top_node()
            .ok_or(SolidError::sm_err(OP))?;
        let bbox = cache.get_tree(k)?.node(top)?.bbox;
        let id = self.add_surface_with_bbox(surface, &bbox)?;

        // Expand the hierarchy element to avoid recomputing the surface cache a 2nd
        // time: every lowest Bezier leaf becomes `SmSurfaceGridElement::Init`
        self.grid.expand_hierarchy(k, id, &mut cache)
    }

    /// [`RayTracer::add_surface_with_cache`] for a cache whose `GetTree` calls the
    /// caller has already run, in upstream order: `AddSurfaceToGrid`'s two and
    /// `ExpandHierarchy`'s one. A face-owned trimmed cache rebuilds its tree through
    /// its face ([`TrimSrfCache::get_tree_with_face`](crate::surface::trim_srf_cache::TrimSrfCache::get_tree_with_face)),
    /// which may change the face's B-rep, so it cannot run while the tracer borrows
    /// the surfaces. Here the tree is read as is.
    ///
    /// # Errors
    ///
    /// As [`RayTracer::add_surface_with_cache`]; `SM_ERR` when the cache has no tree
    /// or the tree no top node.
    pub fn add_surface_with_fetched_cache(
        &mut self,
        k: &Kernel,
        surface: &'s Surface,
        mut cache: SurfaceCache<usize>,
    ) -> Result<()> {
        const OP: &str = "intersect::ray_tracer::add_surface_with_fetched_cache";
        let tree = cache.tree().ok_or(SolidError::sm_err(OP))?;
        let top = tree.top_node().ok_or(SolidError::sm_err(OP))?;
        let bbox = tree.node(top)?.bbox;
        let id = self.add_surface_with_bbox(surface, &bbox)?;
        self.grid.expand_hierarchy_fetched(k, id, &mut cache)
    }

    /// Embeds the faces of a poly B-rep in the grid (`AddPolyBrepToGrid`): notifies
    /// the poly B-rep of an edit (`env` is its notification environment), builds its
    /// `SmBrepCache` and adds every leaf of the cache's surface tree to the grid as a
    /// polygon element over the leaf's box expanded by `expansion_tolerance`. The
    /// poly B-rep joins the tracer's poly B-rep list, so it stays borrowed.
    ///
    /// # Errors
    ///
    /// Those of [`PolyBrep::notify`], of the cache build and of
    /// [`Grid::add_element`]; a null pointer when the cache has no surface tree.
    pub fn add_poly_brep_to_grid<R: TopologyRecords + ?Sized, T>(
        &mut self,
        k: &Kernel,
        poly_brep: &'s mut PolyBrep,
        env: &mut PolyNotifyEnv<'_, R, T>,
        expansion_tolerance: f64,
    ) -> Result<()> {
        const OP: &str = "intersect::ray_tracer::RayTracer::add_poly_brep_to_grid";
        // warn PolyBrep its about to be modifided
        let this = Some(env.this);
        poly_brep.notify(env, NotifyOperation::PreEdit, this, None, None)?;

        // Compute a Brep Cache - a spatial decomposition of the vertices, edges and
        // faces contained in 3 SmTree objects used as a VertexTree, an EdgeTree, and a
        // FaceTree (the notification dropped any cache the poly B-rep had)
        let cache = make_brep_cache(k, CacheSource::PolyBrep(&*poly_brep), &arena_identity)?;

        // get all Surface Tree Nodes
        let tree = cache.surface_tree().ok_or(SolidError::null_pointer(OP))?;
        let index = self.poly_breps.len();

        // For every Surface Tree Node
        for node_id in tree.all_tree_nodes() {
            let node = tree.node(node_id)?;

            // skip non-leaf nodes
            if node.child1.is_some() {
                continue;
            }

            // Get PNode's boundingBox increased by Tol
            let mut bbox = node.bbox;
            bbox.expand_absolute(expansion_tolerance);

            // Add leaf node to Grid: an SmGridElement holding the node, listed in every
            // voxel that its box meets
            let id = self
                .grid
                .add_element(ElementSource::PolyBrep(node_id), &bbox)?;
            self.poly_elements.insert(id, index);
        } // end iter every Surface Tree Node

        // Add the PolyBrep to the SmRayTracer PolyBrep List
        self.poly_breps.push(TracedPolyBrep {
            brep: poly_brep,
            cache,
        });

        // all done
        Ok(())
    }

    /// `AddSurfaceToGrid` up to the expansion, from the point where the top node of
    /// the surface cache's tree is known: `top_node_bbox` is that node's box. Lists
    /// the surface and adds a surface hierarchy element for it over the box grown by
    /// a twentieth of its bounding sphere radius. The element is still to be expanded
    /// ([`Grid::expand_hierarchy`] over the surface's cache).
    ///
    /// # Errors
    ///
    /// Those of [`Grid::add_hierarchy_element`].
    pub fn add_surface_with_bbox(
        &mut self,
        surface: &'s Surface,
        top_node_bbox: &Extent3d,
    ) -> Result<HierarchyId> {
        // set the bounding box = cached normal box increased by a small amount
        let mut surf_bbox = *top_node_bbox;

        // Expand it a little just to be safe
        let radius = surf_bbox.sphere_bound().radius;
        surf_bbox.expand_absolute(radius / 20.0);

        // store the surface in the RayTracer lists
        let index = self.register_surface(surface);

        // add the surface to the grid's hierarchical element list
        let hierarchy = HierarchyElement::new_surface(index, self.ray_tolerance);
        self.grid.add_hierarchy_element(hierarchy, &surf_bbox)
    }
}

/// The image renderer (`SmRayRenderer`): fires one ray per pixel sample into a
/// [`RayTracer`] and shades the hits from the light sources. The view looks along
/// the z axis of the inverse geometry transform into the unit square of `view_volume`.
#[derive(Debug)]
pub struct RayRenderer<'s> {
    /// `m_pRayTracer`: owned, as upstream.
    ray_tracer: RayTracer<'s>,
    /// `m_vInvTransform`: the inverse of the view transformation.
    inv_transform: Axis2Placement,
    /// `m_vViewVolume`.
    view_volume: Extent3d,
    /// `m_lNumXPixels`.
    num_x_pixels: u64,
    /// `m_lNumYPixels`.
    num_y_pixels: u64,
    /// `m_lSampleRate`: samples per pixel in one direction.
    sample_rate: u64,
    /// `m_vLightSources`.
    light_sources: Vec<LightSource>,
    /// `m_bDoShadows`.
    do_shadows: bool,
    /// `m_vBackgroundColor`.
    background_color: Vec3,
    /// `m_vDefaultObjectColor`.
    default_object_color: Vec3,
    /// `m_vDefaultAmbientColor`.
    default_ambient_color: Vec3,
    /// `m_lPixelsShaded`.
    pixels_shaded: u64,
}

impl<'s> RayRenderer<'s> {
    /// `SmRayRenderer(pRayTracer, crGeometryTransform, crViewVolume, lNumXPixels,
    /// lNumYPixels)`: takes ownership of the tracer. The colours upstream leaves
    /// unset until `DoRender` are black.
    #[must_use]
    pub fn new(
        ray_tracer: RayTracer<'s>,
        geometry_transform: &Axis2Placement,
        view_volume: &Extent3d,
        num_x_pixels: u64,
        num_y_pixels: u64,
    ) -> Self {
        Self {
            ray_tracer,
            inv_transform: geometry_transform.invert(),
            view_volume: *view_volume,
            num_x_pixels,
            num_y_pixels,
            sample_rate: 1,
            light_sources: Vec::new(),
            do_shadows: false,
            background_color: Vec3::new(0.0, 0.0, 0.0),
            default_object_color: Vec3::new(0.0, 0.0, 0.0),
            default_ambient_color: Vec3::new(0.0, 0.0, 0.0),
            pixels_shaded: 0,
        }
    }

    /// The ray tracer (`m_pRayTracer`).
    #[must_use]
    pub const fn ray_tracer(&self) -> &RayTracer<'s> {
        &self.ray_tracer
    }

    /// The ray tracer, mutably.
    pub fn ray_tracer_mut(&mut self) -> &mut RayTracer<'s> {
        &mut self.ray_tracer
    }

    /// `GetPixelsShaded`.
    #[must_use]
    pub const fn pixels_shaded(&self) -> u64 {
        self.pixels_shaded
    }

    /// `SetSampleRate`.
    pub fn set_sample_rate(&mut self, sample_rate: u64) {
        self.sample_rate = sample_rate;
    }

    /// `AddLightSource`.
    pub fn add_light_source(&mut self, light: LightSource) {
        self.light_sources.push(light);
    }

    /// Renders the whole image (`DoRender`): sets the colours and renders every
    /// pixel row by row. Returns the pixel colours in row-major order (upstream
    /// keeps one row in a local and prints a progress line per row).
    ///
    /// # Errors
    ///
    /// Those of [`RayRenderer::render_pixel`].
    pub fn do_render(
        &mut self,
        k: &Kernel,
        do_shadows: bool,
        background_color: Vec3,
        default_object_color: Vec3,
    ) -> Result<Vec<Vec3>> {
        self.do_shadows = do_shadows;
        self.background_color = background_color;
        self.default_object_color = default_object_color;
        self.default_ambient_color = default_object_color / 10.0;

        let mut image = Vec::new();
        for iy in 0..self.num_y_pixels {
            for ix in 0..self.num_x_pixels {
                // Here we are working on pixel ix,iy as measured from the upper left
                // hand corner.
                image.push(self.render_pixel(k, ix, iy)?);
            }
        }
        Ok(image)
    }

    /// The colour of one pixel (`RenderPixel`): the average colour of
    /// `sample_rate`² rays from within the pixel.
    ///
    /// # Errors
    ///
    /// Those of [`RayRenderer::render_ray`].
    pub fn render_pixel(&mut self, k: &Kernel, pixel_x: u64, pixel_y: u64) -> Result<Vec3> {
        // init output
        let mut pixel_color = Vec3::new(0.0, 0.0, 0.0);

        // Get Ray direction
        let ray_vector = self.inv_transform.z_axis();

        // get lower corner parameter value for this pixel
        let x = (pixel_x as f64 * 1.0) / self.num_x_pixels as f64;
        let y = (pixel_y as f64 * 1.0) / self.num_y_pixels as f64;

        // figure the sample stepping size for one pixel assuming all pixels sit in a
        // 1x1 unit square.
        let size_x = 1.0 / self.num_x_pixels.wrapping_mul(self.sample_rate) as f64;
        let size_y = 1.0 / self.num_y_pixels.wrapping_mul(self.sample_rate) as f64;

        // for every sample point within the pixel
        for ixs in 0..self.sample_rate {
            for iys in 0..self.sample_rate {
                // pick a pixel start point
                let eval_x = x + ixs as f64 * size_x;
                let eval_y = y + iys as f64 * size_y;
                let ray_point = self.view_volume.evaluate(eval_x, eval_y, 1.0);

                // fire ray into model and pick an associated color
                let ray_color = self.render_ray(k, ray_point, ray_vector)?;

                // accumulate sample point color values
                pixel_color += ray_color;
            }
        }

        // set output color = average of all sample point colors
        Ok(pixel_color / self.sample_rate.wrapping_mul(self.sample_rate) as f64)
    }

    /// The colour of a ray (`RenderRay`): the background when it hits nothing, else
    /// the ambient colour plus the object colour weighted by the cosine to every
    /// light that is not shadowed, clamped to 1.
    ///
    /// # Errors
    ///
    /// Those of [`RayTracer::fire_ray`].
    pub fn render_ray(&mut self, k: &Kernel, ray_point: Point3, ray_vector: Vec3) -> Result<Vec3> {
        // init state for upcoming Ray intersection
        self.ray_tracer.hit_any_thing = false; // Only take closest hit
        self.ray_tracer.skip_element = None;

        // intersect ray with shapes stored within m_pRayTracer
        let hit = self
            .ray_tracer
            .fire_ray(k, ray_point, ray_vector, BIG_DOUBLE)?;

        // when nothing was hit - set output color to Background color
        if !hit.hits_something {
            return Ok(self.background_color);
        }

        // arrive here when input ray hits something

        // get local hit data
        self.pixels_shaded += 1;
        let param = |i: usize| hit.solution.start.parameters.get(i).copied().unwrap_or(0.0);
        let point = Vec3::new(param(3), param(4), param(5));
        let normal = Vec3::new(param(6), param(7), param(8));

        // init output color to Ambient
        let mut ray_color = self.default_ambient_color;

        // for every light source
        for light in &self.light_sources {
            // get angle between lightSource and hitNormal directions
            let mut dot = light.direction_or_origin.dot(normal);

            // when doing shadows - trace a ray towards the light. If it hits anything,
            // this point is in shadow and won't get any of this light's color added to
            // it.
            if self.do_shadows {
                let shadow_vec = light.direction_or_origin;

                // (the hit node, which upstream would skip when the shadow vector is not
                // near 90 degrees to the normal, is the null `m_apNodes[0]`: no skip)
                self.ray_tracer.skip_element = None;

                // start the ray just beyond the current hit point
                let shadow_point = point + 0.0001 * shadow_vec;

                // save a little time - look for any hit not just the closest hit
                self.ray_tracer.hit_any_thing = true;

                // shoot the ray to the light - see if anything else is hit
                let shadow = self
                    .ray_tracer
                    .fire_ray(k, shadow_point, shadow_vec, BIG_DOUBLE)?;

                // something was hit - set dDot to zero to remember this point is in
                // shadow
                if shadow.hits_something {
                    dot = 0.0;
                }
            }

            // add Default Object Color to current ambient light
            if dot > 0.0 {
                ray_color += dot * self.default_object_color;
            }
        }

        // Clamp colors to 1.0
        if ray_color.x > 1.0 {
            ray_color.x = 1.0;
        }
        if ray_color.y > 1.0 {
            ray_color.y = 1.0;
        }
        if ray_color.z > 1.0 {
            ray_color.z = 1.0;
        }

        Ok(ray_color)
    }
}

/// The ray of [`find_poly_hit`].
#[derive(Debug, Clone, Copy)]
struct PolyRay {
    /// `crRayPoint`: the ray's start point.
    point: Point3,
    /// `crRayVector`: the ray's unit direction.
    vector: Vec3,
    /// `bHitAnyThing`: stop at the first hit.
    hit_any_thing: bool,
}

/// `sm_FindPolyHit`: intersects the ray with the poly faces of `node` (a node of the
/// `SmBrepCache` surface tree `tree` of `brep`) and of its parents. A face hit with ray
/// parameter in `(-EFF_ZERO, hit_t)` lowers `hit_t`; the last such hit is returned
/// with its face (with `hit_any_thing`, the first). `mark` marks a face for this ray
/// and is false when the ray has tested the face before (`IsMarked` / `Mark`), which
/// then is skipped.
///
/// Upstream recurses on the parent node after the node's own faces; this walks up the
/// parents in a loop in the same order, bounded by the tree's node count.
fn find_poly_hit(
    k: &Kernel,
    tree: &Tree<CacheObject>,
    brep: &mut PolyBrep,
    node: NodeId,
    ray: PolyRay,
    mut hit_t: f64,
    mark: &mut dyn FnMut(PolyFaceId) -> bool,
) -> Result<Option<(f64, PolyFaceId)>> {
    const OP: &str = "intersect::ray_tracer::find_poly_hit";
    let mut hit_face = None;
    let mut current = Some(node);
    let max_depth = tree.all_tree_nodes().len();
    let mut depth = 0usize;
    while let Some(node) = current {
        // (a parent chain longer than the tree has nodes is a cycle)
        depth += 1;
        if depth > max_depth {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "tree parent links form a cycle",
            });
        }

        // for every object in pNode
        for list in tree.node_object_list(node)? {
            let Some(CacheObject::PolyFace(poly_face)) = tree.list(list)?.object else {
                return Err(SolidError::null_pointer(OP));
            };

            // skip already processed PolyFaces (and mark this one)
            if !mark(poly_face) {
                continue; // This face has been processed before
            }

            // get PolyFace bounding sphere/Ray Distance
            let (sphere_center, sphere_radius) =
                brep.face_compute_sphere_bound(poly_face, false)?;
            let vec_to_center = sphere_center - ray.point;
            let center_t = vec_to_center.dot(ray.vector);
            let point_on_ray = ray.point + ray.vector * center_t;
            let vec_to = point_on_ray - sphere_center;
            let dist_sq = vec_to.length_squared();
            let sph_rad_sq = sphere_radius * sphere_radius;

            // skip PolyFaces whose bounding Sphere does not intersect the ray
            if dist_sq > sph_rad_sq {
                continue;
            }
            let (normal, _) = brep.face_normal(k, poly_face, false, true)?;
            let poly_loop = brep
                .face_outer_poly_loop(poly_face)?
                .ok_or(SolidError::null_pointer(OP))?;
            let first_edge = brep
                .loop_first_poly_edge(poly_loop)?
                .ok_or(SolidError::null_pointer(OP))?;
            let origin = brep.edge_start_point(first_edge)?;
            let Ok(t_param) = gu::line_plane_intersect(ray.point, ray.vector, origin, normal)
            else {
                continue;
            };
            let point = ray.point + ray.vector * t_param;
            if point.distance_squared(sphere_center) > sph_rad_sq {
                continue; // Intersection is outside of sphere radius.
            }
            let inside = brep.face_point_in_polygon(k, poly_face, point, true)?;
            if inside && t_param < hit_t && t_param > -EFF_ZERO {
                hit_t = t_param;
                hit_face = Some(poly_face);
                if ray.hit_any_thing {
                    return Ok(hit_face.map(|f| (hit_t, f)));
                }
            }
        } // end iter every pNode Object

        // continue on the parent node
        current = tree.node(node)?.parent;
    }
    Ok(hit_face.map(|f| (hit_t, f)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent3d::Extent3d;
    use crate::geom::polar_box::PolarBox;
    use crate::geom::pseudo_box::PseudoBox;

    fn grid() -> Grid<usize> {
        let bbox = Extent3d::from_bounds(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
        Grid::new(&bbox, [2, 2, 2]).unwrap()
    }

    /// A poly B-rep of horizontal squares `[2, 8] x [2, 8]` at the heights `zs`, wound
    /// counter-clockwise seen from above, and their faces.
    fn stacked_squares(k: &Kernel, zs: &[f64]) -> (PolyBrep, Vec<PolyFaceId>) {
        let mut b = PolyBrep::new(k, 1.0e-5);
        let r = b.add_region().unwrap();
        let s = b.add_shell(r).unwrap();
        let mut faces = Vec::new();
        for &z in zs {
            let f = b.add_face(k, s, 1.0e-5, None).unwrap();
            let pts = [
                Vec3::new(2.0, 2.0, z),
                Vec3::new(8.0, 2.0, z),
                Vec3::new(8.0, 8.0, z),
                Vec3::new(2.0, 8.0, z),
            ];
            let vs: Vec<_> = pts
                .iter()
                .map(|&p| b.add_vertex(k, p, 1.0e-5).unwrap())
                .collect();
            let l = b.start_poly_edge_loop(f).unwrap();
            for j in 0..4 {
                b.add_poly_edge(k, l, 1.0e-5, pts[j], pts[(j + 1) % 4], vs[j])
                    .unwrap();
            }
            b.finish_poly_edge_loop(l).unwrap();
            faces.push(f);
        }
        (b, faces)
    }

    /// Adds `brep` to `tracer` with an empty notification environment.
    fn add_poly<'s>(k: &Kernel, tracer: &mut RayTracer<'s>, brep: &'s mut PolyBrep) {
        use crate::geom::a_object::CacheStore;
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::query::poly::PolyObjectKind;
        let mut records: BTreeMap<ObjectKey, crate::topology::topology::Topology> = BTreeMap::new();
        let mut attributes = AttributeStore::new();
        let mut caches: CacheStore<()> = CacheStore::new();
        let mut env = PolyNotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            caches: &mut caches,
            context: None,
            this: ObjectRef {
                type_code: PolyObjectKind::Brep.type_code(),
                key: ObjectKey(1),
            },
        };
        tracer
            .add_poly_brep_to_grid(k, brep, &mut env, 1.0e-3)
            .unwrap();
    }

    #[test]
    fn a_poly_brep_adds_one_polygon_element_per_surface_tree_leaf() {
        let k = Kernel::default();
        let (mut b, _) = stacked_squares(&k, &[2.0, 6.0]);
        let mut tracer = RayTracer::new(grid(), 1.0e-6, 1.0e-9);
        add_poly(&k, &mut tracer, &mut b);
        assert_eq!(tracer.poly_brep_count(), 1);
        let elements = tracer.poly_elements.len();
        assert!(elements >= 1);
        let tree = tracer.poly_breps[0].cache.surface_tree().unwrap();
        let leaves = tree
            .all_tree_nodes()
            .into_iter()
            .filter(|&n| tree.node(n).unwrap().child1.is_none())
            .count();
        assert_eq!(elements, leaves);
    }

    #[test]
    fn a_ray_hits_the_nearest_poly_face() {
        let k = Kernel::default();
        let (mut b, faces) = stacked_squares(&k, &[2.0, 6.0]);
        let mut tracer = RayTracer::new(grid(), 1.0e-6, 1.0e-9);
        add_poly(&k, &mut tracer, &mut b);
        let hit = tracer
            .fire_ray(
                &k,
                Vec3::new(5.0, 5.0, 9.5),
                Vec3::new(0.0, 0.0, -1.0),
                BIG_DOUBLE,
            )
            .unwrap();
        assert!(hit.hits_something);
        let s = &hit.solution;
        assert_eq!(s.solution_type, SolutionType::SingleValue);
        assert_eq!(s.num_variables, 1);
        assert_eq!(s.num_objects, 1);
        assert_eq!(s.objects[0], Some(RayObject::PolyFace(0, faces[1])));
        assert_eq!(s.objects[1], Some(RayObject::PolyFace(0, faces[1])));
        assert_eq!(
            s.start.parameters[..9],
            [3.5, BIG_DOUBLE, BIG_DOUBLE, 5.0, 5.0, 6.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn a_poly_face_behind_the_ray_start_is_not_hit() {
        let k = Kernel::default();
        let (mut b, faces) = stacked_squares(&k, &[2.0, 6.0]);
        let mut tracer = RayTracer::new(grid(), 1.0e-6, 1.0e-9);
        add_poly(&k, &mut tracer, &mut b);
        let hit = tracer
            .fire_ray(
                &k,
                Vec3::new(5.0, 5.0, 4.0),
                Vec3::new(0.0, 0.0, -1.0),
                BIG_DOUBLE,
            )
            .unwrap();
        assert!(hit.hits_something);
        assert_eq!(
            hit.solution.objects[0],
            Some(RayObject::PolyFace(0, faces[0]))
        );
        assert_eq!(hit.solution.start.parameters[0], 2.0);
    }

    #[test]
    fn rays_outside_or_along_the_poly_faces_hit_nothing() {
        let k = Kernel::default();
        let (mut b, _) = stacked_squares(&k, &[2.0, 6.0]);
        let mut tracer = RayTracer::new(grid(), 1.0e-6, 1.0e-9);
        add_poly(&k, &mut tracer, &mut b);
        // through the faces' planes outside the squares
        let miss = tracer
            .fire_ray(
                &k,
                Vec3::new(9.0, 5.0, 9.5),
                Vec3::new(0.0, 0.0, -1.0),
                BIG_DOUBLE,
            )
            .unwrap();
        assert!(!miss.hits_something);
        // parallel to the faces, between them
        let along = tracer
            .fire_ray(
                &k,
                Vec3::new(0.5, 5.0, 4.0),
                Vec3::new(1.0, 0.0, 0.0),
                BIG_DOUBLE,
            )
            .unwrap();
        assert!(!along.hits_something);
        // stopped before the first face
        let short = tracer
            .fire_ray(&k, Vec3::new(5.0, 5.0, 9.5), Vec3::new(0.0, 0.0, -1.0), 3.0)
            .unwrap();
        assert!(!short.hits_something);
    }

    #[test]
    fn hit_any_thing_stops_at_a_poly_hit() {
        let k = Kernel::default();
        let (mut b, faces) = stacked_squares(&k, &[2.0, 6.0]);
        let mut tracer = RayTracer::new(grid(), 1.0e-6, 1.0e-9);
        tracer.hit_any_thing = true;
        add_poly(&k, &mut tracer, &mut b);
        let hit = tracer
            .fire_ray(
                &k,
                Vec3::new(5.0, 5.0, 9.5),
                Vec3::new(0.0, 0.0, -1.0),
                BIG_DOUBLE,
            )
            .unwrap();
        assert!(hit.hits_something);
        assert!(faces
            .iter()
            .any(|&f| hit.solution.objects[0] == Some(RayObject::PolyFace(0, f))));
    }

    #[test]
    fn convert_to_parameter_delta_projects_on_the_tangent() {
        let d = convert_to_parameter_delta(Vec3::new(2.0, 0.0, 0.0), Vec3::new(3.0, 4.0, 0.0));
        assert!((d - 1.5).abs() < 1e-15);
        assert_eq!(
            convert_to_parameter_delta(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0)),
            0.0
        );
    }

    #[test]
    fn constructor_sets_the_solver_like_upstream() {
        let mut t = RayTracer::new(grid(), 1e-4, 1e-8);
        let s = t.solver();
        assert!(s.ray_fire && s.have_t_min_t_max);
        assert_eq!(s.distance_tolerance, 1e-4);
        assert_eq!(s.ray_accuracy, 1e-8);
        assert_eq!(s.line_interval, Extent1d::from_raw(0.0, 1.0e10));
        assert_eq!(s.d3d_tolerance, 0.0);
        t.set_solver_tolerance(0.5);
        assert_eq!(t.solver().d3d_tolerance, 0.5);
    }

    #[test]
    fn a_ray_that_misses_the_grid_hits_nothing() {
        let k = &Kernel::default();
        let mut t = RayTracer::new(grid(), 1e-4, 1e-8);
        let r = t
            .fire_ray(
                k,
                Vec3::new(-5.0, 20.0, 5.0),
                Vec3::new(1.0, 0.0, 0.0),
                BIG_DOUBLE,
            )
            .unwrap();
        assert!(!r.hits_something);
        assert!(r.grid_elements.is_empty());
    }

    #[test]
    fn a_ray_through_an_empty_grid_walks_out() {
        let k = &Kernel::default();
        let mut t = RayTracer::new(grid(), 1e-4, 1e-8);
        let r = t
            .fire_ray(
                k,
                Vec3::new(-5.0, 5.0, 5.0),
                Vec3::new(1.0, 0.0, 0.0),
                BIG_DOUBLE,
            )
            .unwrap();
        assert!(!r.hits_something);
    }

    #[test]
    fn a_leaf_the_ray_cannot_reach_is_rejected_before_the_solver() {
        let k = &Kernel::default();
        let plane = Surface::from(crate::surface::plane::Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0),
        ));
        let mut t = RayTracer::new(grid(), 1e-4, 1e-8);
        let s = t.register_surface(&plane);
        // a pseudo box far from the ray: the solver returns before LocalLineIntersect
        let far = Extent3d::from_bounds(0.0, 0.0, 8.0, 10.0, 10.0, 9.0);
        let leaf = SurfaceGridElement {
            surface: s,
            uv_domain: Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0),
            bbox: far,
            pseudo_box: PseudoBox::from_extent3d(&far),
            polar_box: PolarBox::from_vector(Vec3::new(0.0, 0.0, 1.0)),
            mid_point: Vec3::new(5.0, 5.0, 8.5),
            mid_du: Vec3::new(1.0, 0.0, 0.0),
            mid_dv: Vec3::new(0.0, 1.0, 0.0),
            mid_normal: Vec3::new(0.0, 0.0, 1.0),
        };
        t.grid_mut().add_surface_element(leaf, &far).unwrap();
        let r = t
            .fire_ray(
                k,
                Vec3::new(-5.0, 5.0, 1.0),
                Vec3::new(1.0, 0.0, 0.0),
                BIG_DOUBLE,
            )
            .unwrap();
        assert!(!r.hits_something);
    }

    #[test]
    fn adding_a_surface_expands_it_into_surface_leaves() {
        let plane = Surface::from(crate::surface::plane::Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ));
        let mut t = RayTracer::new(grid(), 1e-4, 1e-8);
        t.add_surface_to_grid(&Kernel::default(), &plane).unwrap();
        let leaves: Vec<_> = t
            .grid_mut()
            .elements()
            .iter()
            .filter_map(|e| match e.item() {
                GridItem::Surface(leaf) => Some(leaf),
                _ => None,
            })
            .collect();
        assert!(!leaves.is_empty());
        assert!(leaves.iter().all(|l| l.surface == 0));
        // a plane's normals are one vector: the leaf's polar box has it
        assert!(leaves
            .iter()
            .all(|l| l.polar_box.state() == crate::geom::polar_box::PolarBoxState::SinglePoint));
    }

    #[test]
    fn a_face_owned_surface_needs_the_trimmed_cache_of_its_face() {
        use crate::geom::assert_array::labels::FACE_TYPE;
        use crate::geom::attribute::ObjectKey;
        let mut plane = Surface::from(crate::surface::plane::Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ));
        plane.set_owner(Some(ObjectRef {
            type_code: FACE_TYPE,
            key: ObjectKey(1),
        }));
        let mut t = RayTracer::new(grid(), 1e-4, 1e-8);
        assert!(matches!(
            t.add_surface_to_grid(&Kernel::default(), &plane),
            Err(SolidError::Unported { .. })
        ));
        // nothing was added: upstream's leaves would be the trimmed ones
        assert!(t.surfaces().is_empty());
        assert!(t.grid_mut().elements().is_empty());
    }
}

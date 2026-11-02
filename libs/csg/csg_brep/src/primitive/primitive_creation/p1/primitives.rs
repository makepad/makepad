//! Primitive entry points through their topology construction dependencies.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`,
//! `source/SMLib/inc/SmPrimitiveCreation.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`create_rectangle`] | static `SmPrimitiveCreation::CreateRectangle` |
//! | [`create_circle`] | static `SmPrimitiveCreation::CreateCircle` |
//! | [`PrimitiveCreation::create_box`] | `SmPrimitiveCreation::CreateBox` |
//! | [`PrimitiveCreation::create_cylindrical_box`] | `CreateCylindricalBox` |
//! | [`PrimitiveCreation::create_cone`] | protected `CreateCone` |
//! | [`PrimitiveCreation::create_cylinder`] | `CreateCylinder` |
//! | `conic_solid` (private) | static `sm_CreateConicSolid` |
//! | [`PrimitiveCreation::create_sphere`] | protected `CreateSphere` |
//! | [`PrimitiveCreation::create_torus`] | protected `CreateTorus` |
//! | [`PrimitiveCreation::boolean_2d`] | `Boolean2D` |
//! | [`PrimitiveCreation::create_linear_sweep`] | protected curve `CreateLinearSweep` |
//! | [`PrimitiveCreation::create_sphere_no_pole`] | `CreateSphereNoPole` |
//! | [`PrimitiveCreation::create_cone_elliptic_ends`] | `CreateConeEllipticEnds` |
//! | [`analytic_rotational_sweep`] | `sm_CreateAnalyticRotationalSweep` |
//!
//! Deviations: missing topology edits return Unported at their first call.
//! Mutations after that call are deferred; no success is claimed for those paths.
//! Analytic cone and torus objects stay concrete until Surface dispatch supports
//! them. `SmBrep` is caller-owned; edits retain upstream edit-mode side effects.
//! Face construction uses caller notification records, attributes and identity keys.
//! No recursion.

use super::PrimitiveCreation;
use crate::curve::curve::Curve;
use crate::curve::ellipse::Ellipse;
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent2d::Extent2d;
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::math::{EFF_ZERO, EFF_ZERO_DEG};
use crate::nurbs::b_spline_curve::p1::{BSplineCurve, NurbCircleParam};
use crate::nurbs::b_spline_surface::p1::BSplineSurface;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::surface::cone::p1::Cone;
use crate::surface::plane::p1::Plane;
use crate::surface::sphere::Sphere;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::surface::torus::{Torus, TorusOptions};
use crate::topology::edge::NotifyEnv;
use crate::topology::{Brep, FaceId, RegionId};

#[derive(Debug, Clone, Copy, PartialEq)]
struct SweepCurve(u8, crate::geom::extent1d::Extent1d);
impl crate::geom::solution_array::SolutionObject for SweepCurve {
    fn kind(&self) -> crate::geom::solution_array::ObjectKind {
        crate::geom::solution_array::ObjectKind::Curve
    }
    fn natural_interval(&self) -> Option<crate::geom::extent1d::Extent1d> {
        Some(self.1)
    }
}
fn intersection_options(
    a: crate::geom::extent1d::Extent1d,
    b: crate::geom::extent1d::Extent1d,
) -> crate::curve::curve::p8::CurveIntersectOptions<SweepCurve> {
    crate::curve::curve::p8::CurveIntersectOptions {
        handles: [Some(SweepCurve(0, a)), Some(SweepCurve(1, b))],
        analytic_handles: [Some(SweepCurve(2, a)), Some(SweepCurve(3, b))],
        ..Default::default()
    }
}

/// Operations on coincident planar regions (`Sm2DBooleanOperationType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boolean2d {
    /// Union.
    Union = 0,
    /// Intersection.
    Intersection = 1,
    /// A minus B.
    Difference = 2,
    /// Exclusive or.
    ExclusiveOr = 3,
    /// Merge, preserving internal edges.
    Merge = 4,
}

pub(super) fn new_brep(k: &mut Kernel) -> Result<Brep> {
    crate::topology::brep::p3::new_brep(
        k,
        crate::geom::tol::USE_DEFAULT,
        crate::geom::tol::USE_DEFAULT,
    )
}

/// Runs `body` with a face-construction environment over an arena-only notification
/// store (`ArenaNotifyState`): no observer holds identities for these B-reps' objects.
fn with_face_env<T>(
    k: &mut Kernel,
    b: &mut Brep,
    body: impl FnOnce(
        &mut Kernel,
        &mut Brep,
        &mut crate::topology::brep::p8::FaceBuildEnv<'_, '_>,
    ) -> Result<T>,
) -> Result<T> {
    let mut state = crate::topology::brep::p2::ArenaNotifyState::new(b);
    let mut env = state.env(None);
    let mut marks = crate::topology::topology_traverser::Marks::default();
    let this_face =
        |f: FaceId| crate::topology::loopuse::notify_ref(crate::error::EntityRef::Face(f));
    let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
        env: &mut env,
        marks: &mut marks,
        this_face: &this_face,
    };
    body(k, b, &mut fenv)
}

fn create_face(k: &mut Kernel, b: &mut Brep, surface: Surface, domain: Extent2d) -> Result<()> {
    create_face_id(k, b, surface, domain).map(|_| ())
}

fn stitch(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    stitch_with_keys(k, b, &crate::topology::loopuse::notify_ref)
}

fn stitch_with_keys(
    k: &mut Kernel,
    b: &mut Brep,
    key_of: &dyn Fn(crate::error::EntityRef) -> crate::geom::object::ObjectRef,
) -> Result<()> {
    use crate::query::brep::BrepIdentity;
    use crate::topology::brep::p5::{stitch_and_orient_with_identity, StitchAndOrientOptions};
    let keys = |object| crate::topology::brep::p2::caller_cache_key(key_of, object);
    stitch_and_orient_with_identity(
        k,
        b,
        &StitchAndOrientOptions::default(),
        Some(BrepIdentity {
            keys: &keys,
            ..BrepIdentity::default()
        }),
    )
}

/// `CreatePlanarFaceWith3DCurves`; `None` when the curves bound no face.
fn planar_face(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    curves: &[Curve],
    tolerance: f64,
) -> Result<Option<FaceId>> {
    with_face_env(k, b, |k, b, fenv| {
        crate::topology::brep::p8::create_planar_face_with_3d_curves(
            k, b, region, curves, tolerance, fenv,
        )
    })
}
pub(super) fn planar_faces(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    curves: &[Curve],
    tolerance: f64,
) -> Result<Vec<FaceId>> {
    with_face_env(k, b, |k, b, fenv| {
        crate::topology::brep::p8::create_planar_faces_with_3d_curves(
            k, b, region, curves, tolerance, fenv,
        )
    })
}
fn cone_face(k: &mut Kernel, b: &mut Brep, surface: &Cone, domain: Extent2d) -> Result<FaceId> {
    create_face_id(k, b, Surface::from(surface.clone()), domain)
}
fn torus_face(k: &mut Kernel, b: &mut Brep, surface: &Torus, domain: Extent2d) -> Result<FaceId> {
    create_face_id(k, b, Surface::from(surface.clone()), domain)
}
fn extrusion_face(
    k: &mut Kernel,
    b: &mut Brep,
    surface: &crate::surface::surf_of_extrusion::p1::SurfOfExtrusion,
    domain: Extent2d,
) -> Result<()> {
    create_face(k, b, Surface::from(surface.clone()), domain)
}
fn trimmed_face(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    curves: Vec<Curve>,
    surface: Surface,
    domain: Extent2d,
    notifications: &mut NotifyEnv<'_>,
) -> Result<FaceId> {
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    detailed_face(
        k,
        b,
        region,
        crate::topology::face::p2::FaceCurves {
            curve_loops: vec![4],
            curve_orients: vec![OrientType::Same; 4],
            curves_3d: Some(curves),
            ..Default::default()
        },
        surface,
        domain,
        notifications,
    )
}

fn create_face_id(
    k: &mut Kernel,
    b: &mut Brep,
    surface: Surface,
    domain: Extent2d,
) -> Result<FaceId> {
    with_face_env(k, b, |k, b, fenv| {
        crate::topology::brep::p8::create_face_from_surface(k, b, Some(surface), &domain, fenv)
    })
}
/// Face construction under the same caller identities as its trimmed neighbours.
fn create_face_with_notifications(
    k: &mut Kernel,
    b: &mut Brep,
    surface: Surface,
    domain: Extent2d,
    notifications: &mut NotifyEnv<'_>,
) -> Result<FaceId> {
    let key_of = notifications.key_of;
    let mut marks = crate::topology::topology_traverser::Marks::default();
    let this_face = |face| key_of(crate::error::EntityRef::Face(face));
    let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
        env: notifications,
        marks: &mut marks,
        this_face: &this_face,
    };
    crate::topology::brep::p8::create_face_from_surface(k, b, Some(surface), &domain, &mut fenv)
}

/// `SmFace::GetEdges(rEdges)` with its defaults (`topology::face::p4::edges`).
fn face_edges(_k: &Kernel, b: &Brep, face: FaceId) -> Result<Vec<crate::topology::EdgeId>> {
    crate::topology::face::p4::edges(b, face, None, false, None)
}
pub(super) fn detailed_face(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    boundary: crate::topology::face::p2::FaceCurves,
    surface: Surface,
    domain: Extent2d,
    notifications: &mut NotifyEnv<'_>,
) -> Result<FaceId> {
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::topology::brep::p7::{make_face_with_curves, FaceCurvesInput};
    let loops = boundary
        .curve_loops
        .into_iter()
        .map(|count| {
            usize::try_from(count).map_err(|_| {
                SolidError::invalid_input(
                    "primitive::primitive_creation::p1::detailed_face",
                    "loop count exceeds usize",
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let input = FaceCurvesInput {
        loops,
        curves_3d: boundary
            .curves_3d
            .map(|curves| curves.into_iter().map(Some).collect()),
        uv_curves: boundary.uv_curves,
        orientations: boundary.curve_orients,
        loop_points: boundary.loop_points,
    };
    let key_of = notifications.key_of;
    make_face_with_curves(
        k,
        b,
        region,
        input,
        surface,
        &domain,
        OrientType::Same,
        None,
        &|face| key_of(crate::error::EntityRef::Face(face)),
        notifications,
    )?
    .face
    .ok_or_else(|| SolidError::null_pointer("primitive::primitive_creation::p1::detailed_face"))
}

/// `pSurface->DropCurve(ctx, pSurface->GetNaturalUVDomain(), *pCurve,
/// pCurve->GetNaturalInterval(), dTolerance, dMaxDist, dDeviation, rUVCurves)` with the
/// declaration's defaults: `output` is reset first (upstream `rUVCurves.ReSet()`) and
/// holds the UV curves only when the drop succeeds.
fn drop_uv_curves(
    k: &Kernel,
    surface: &Surface,
    curve: &Curve,
    tolerance: f64,
    output: &mut Vec<Curve>,
) -> Result<()> {
    output.clear();
    let dropped = surface.drop_curve::<NoObject>(
        k,
        &surface.natural_uv_domain_with_nurb(k)?,
        curve,
        &curve.natural_interval(),
        tolerance,
        true,
        None,
        true,
    )?;
    output.extend(dropped.uv_curves);
    Ok(())
}
/// `SmBrep::StitchFaces(tol, ...)` with the header defaults (`pOptFaces = NULL`,
/// `bAllowSpineEdges = FALSE`); the counts and gaps are unused.
pub(super) fn stitch_faces(k: &mut Kernel, b: &mut Brep, tolerance: f64) -> Result<()> {
    crate::topology::brep::p4::stitch_faces(k, b, tolerance, None, false).map(drop)
}

enum AnalyticSurface {
    Cone(Cone),
    Sphere(Sphere),
    Torus(Torus),
}
fn analytic_face(k: &mut Kernel, b: &mut Brep, surface: AnalyticSurface) -> Result<FaceId> {
    match surface {
        AnalyticSurface::Cone(surface) => {
            cone_face(k, b, &surface, surface.rev.base().natural_uv_domain())
        }
        AnalyticSurface::Sphere(surface) => {
            let domain = surface.base().base().natural_uv_domain();
            sphere_face(k, b, &surface, domain)
        }
        AnalyticSurface::Torus(surface) => {
            let domain = surface.base().base().natural_uv_domain();
            torus_face(k, b, &surface, domain)
        }
    }
}

fn sphere_surface(k: &Kernel, frame: &Axis2Placement, radius: f64) -> Result<Sphere> {
    Sphere::create_canonical(k, frame, radius)
}

fn sphere_face(k: &mut Kernel, b: &mut Brep, surface: &Sphere, domain: Extent2d) -> Result<FaceId> {
    create_face_id(k, b, Surface::from(surface.clone()), domain)
}

/// Static rectangular-face constructor. The tolerance is only used by release
/// assertions upstream; the new B-rep inherits its context's tolerance.
pub fn create_rectangle(
    k: &mut Kernel,
    _tolerance: f64,
    width: f64,
    height: f64,
    frame: &Axis2Placement,
) -> Result<Brep> {
    let p00 = frame.origin();
    let p10 = p00 + frame.x_axis() * width;
    let p01 = p00 + frame.y_axis() * height;
    let p11 = p10 + p01 - p00;
    let base = BSplineSurface::create_bilinear_surface(p00, p10, p01, p11)?;
    let plane = Surface::from(base).copy_and_add_analytics(k)?;
    let mut brep = new_brep(k)?;
    let domain = plane.natural_uv_domain_with_nurb(k)?;
    ignore_status(create_face(k, &mut brep, plane, domain))?;
    Ok(brep)
}

// CreateCircle ignores the segment-constructor status (SE) and tests pCircle.
// An unavailable port must remain visible; a failed native allocation/geometry
// instead yields the static constructor's null result and its SM_ERR status.
fn circle_error(error: SolidError) -> SolidError {
    if error.is_unported() {
        error
    } else {
        SolidError::sm_err("primitive::primitive_creation::p1::create_circle")
    }
}
fn circle_curve(radius: f64, frame: &Axis2Placement) -> Result<BSplineCurve> {
    BSplineCurve::create_circle_segment(3, frame, radius, 0.0, 360.0, NurbCircleParam::Quadratic)
        .map_err(circle_error)
}

/// Static circular-face constructor. `tolerance` is passed to the planarity
/// check; it does not replace the context's B-rep tolerance.
pub fn create_circle(
    k: &mut Kernel,
    tolerance: f64,
    radius: f64,
    frame: &Axis2Placement,
) -> Result<Brep> {
    let mut brep = new_brep(k)?;
    let circle = circle_curve(radius, frame)?;
    brep.set_editing_enabled(true);
    let region = brep.infinite_region();
    if let Err(error) = planar_face(k, &mut brep, region, &[Curve::from(circle)], tolerance) {
        return Err(circle_error(error));
    }
    brep.set_editing_enabled(false);
    Ok(brep)
}

impl PrimitiveCreation {
    /// Build six planar faces, then stitch and orient them (`CreateBox`).
    pub fn create_box(
        &mut self,
        k: &mut Kernel,
        brep: &mut Brep,
        length: f64,
        width: f64,
        height: f64,
        frame: &Axis2Placement,
    ) -> Result<()> {
        brep.set_editing_enabled(true);
        let mut placement = Axis2Placement::new(frame.origin(), frame.x_axis(), frame.y_axis());
        let planes = [
            (frame.x_axis(), frame.y_axis(), length, width, height),
            (frame.y_axis(), frame.z_axis(), width, height, length),
            (frame.x_axis(), frame.z_axis(), length, height, -width),
        ];
        for (index, (x, y, u, v, offset)) in planes.into_iter().enumerate() {
            if index != 0 {
                ignore_status(placement.set_canonical(frame.origin(), x, y))?;
            }
            let domain = Extent2d::from_bounds(0.0, 0.0, u, v);
            for side in 0..2 {
                if side == 1 {
                    placement.translate(placement.z_axis().unitize_or_z() * offset);
                }
                let mut plane = Plane::create_canonical(&placement)?;
                ignore_status(plane.adjust_step_uv_domain(&domain))?;
                let uv = plane.step_uv_domain();
                ignore_status(create_face(k, brep, Surface::from(plane), uv))?;
            }
        }
        ignore_status(stitch(k, brep))?;
        brep.set_editing_enabled(false);
        Ok(())
    }

    /// Cylindrical box validation, followed by the first trimmed plane edit.
    /// The angle bounds are degrees, with exact inclusive bounds upstream.
    /// `notifications` supplies the caller's topology records, attributes and keys.
    pub fn create_cylindrical_box(
        &mut self,
        k: &mut Kernel,
        brep: &mut Brep,
        dimensions: [f64; 5],
        frame: &Axis2Placement,
        notifications: &mut NotifyEnv<'_>,
    ) -> Result<()> {
        let [length, inner, outer, start, end] = dimensions;
        if !(length > EFF_ZERO
            && inner > EFF_ZERO
            && outer > inner + EFF_ZERO
            && (-360.0..=360.0).contains(&start)
            && end > start + EFF_ZERO
            && (0.0..=360.0).contains(&(end - start)))
        {
            return Err(SolidError::invalid_input(
                "primitive::primitive_creation::p1::create_cylindrical_box",
                "invalid length, radii or angular range",
            ));
        }
        brep.set_editing_enabled(true);
        let radians = core::f64::consts::PI / 180.0;
        let start_vec =
            frame.x_axis() * (start * radians).cos() + frame.y_axis() * (start * radians).sin();
        let mid = (start + end) / 2.0;
        let mid_vec =
            frame.x_axis() * (mid * radians).cos() + frame.y_axis() * (mid * radians).sin();
        let end_vec =
            frame.x_axis() * (end * radians).cos() + frame.y_axis() * (end * radians).sin();
        let delta = frame.z_axis() * length;
        let inner_points = [
            frame.origin() + start_vec * inner,
            frame.origin() + mid_vec * inner,
            frame.origin() + end_vec * inner,
        ];
        let outer_points = [
            frame.origin() + start_vec * outer,
            frame.origin() + mid_vec * outer,
            frame.origin() + end_vec * outer,
        ];
        let [a, b, c] = inner_points;
        let [d, e, f] = outer_points;
        let mut placement = *frame;
        for top in [false, true] {
            let (a, b, c, d, e, f) = if top {
                placement.translate(delta);
                (
                    a + delta,
                    b + delta,
                    c + delta,
                    d + delta,
                    e + delta,
                    f + delta,
                )
            } else {
                (a, b, c, d, e, f)
            };
            let mut plane = Plane::create_canonical(&placement)?;
            let domain = Extent2d::from_bounds(-outer, -outer, outer, outer);
            ignore_status(plane.adjust_step_uv_domain(&domain))?;
            let curves = vec![
                Curve::from(Line::from_points(a, d, 3)),
                Curve::from(Ellipse::circle_from_3_points(k, d, e, f, 3, false)?),
                Curve::from(Line::from_points(f, c, 3)),
                Curve::from(Ellipse::circle_from_3_points(k, c, b, a, 3, false)?),
            ];
            ignore_status(trimmed_face(
                k,
                brep,
                self.region(),
                curves,
                Surface::from(plane),
                domain,
                notifications,
            ))?;
        }
        for radial in [start_vec, end_vec] {
            ignore_status(placement.set_canonical(frame.origin(), radial, frame.z_axis()))?;
            let mut plane = Plane::create_canonical(&placement)?;
            ignore_status(
                plane.adjust_step_uv_domain(&Extent2d::from_bounds(inner, 0.0, outer, length)),
            )?;
            let domain = plane.natural_uv_domain();
            ignore_status(create_face_with_notifications(
                k,
                brep,
                Surface::from(plane),
                domain,
                notifications,
            ))?;
        }
        for radius in [inner, outer] {
            let mut cylinder = Cone::create_canonical_cylinder(k, frame, radius)?;
            ignore_status(
                cylinder.adjust_step_uv_domain(k, &Extent2d::from_bounds(start, 0.0, end, length)),
            )?;
            ignore_status(create_face_with_notifications(
                k,
                brep,
                Surface::from(cylinder.clone()),
                cylinder.rev.base().natural_uv_domain(),
                notifications,
            ))?;
        }
        ignore_status(stitch_with_keys(k, brep, notifications.key_of))?;
        brep.set_editing_enabled(false);
        Ok(())
    }

    /// Full analytic cone; edit mode stays enabled, including on error.
    pub fn create_cone(
        &mut self,
        k: &mut Kernel,
        brep: &mut Brep,
        height: f64,
        base: f64,
        top: f64,
        frame: &Axis2Placement,
    ) -> Result<()> {
        brep.set_editing_enabled(true);
        let cone = Cone::create_canonical(k, frame, base, top, height, false, false)?;
        self.conic_solid(k, brep, &cone, base, top)
    }

    /// Solid cylinder of `radius` and `height` on the frame (`CreateCylinder`): the
    /// canonical cylinder bounded to `[0, 360] x [0, height]` and its planar caps. A
    /// height below `SM_EFF_ZERO` or a failed bound is `SM_ERR`; edit mode ends
    /// disabled on success.
    ///
    /// # Errors
    ///
    /// `SM_ERR` as above; those of the cylinder construction and of the solid.
    pub fn create_cylinder(
        &mut self,
        k: &mut Kernel,
        brep: &mut Brep,
        height: f64,
        radius: f64,
        frame: &Axis2Placement,
    ) -> Result<()> {
        const OP: &str = "primitive::primitive_creation::p1::create_cylinder";
        // empty entity maps - m_vMapToSame, m_vMapToHigher, m_vMapFromSame, and m_vMapFromLower
        self.init_entity_maps();

        if height < EFF_ZERO {
            return Err(SolidError::sm_err(OP));
        }

        brep.set_editing_enabled(true);

        // CreateCanonical makes an infinite cylinder; bound it to v in [0, dHeight]
        let mut cylinder = Cone::create_canonical_cylinder(k, frame, radius)?;
        let domain = Extent2d::from_bounds(0.0, 0.0, 360.0, height);
        match cylinder.adjust_step_uv_domain(k, &domain) {
            Ok(()) => {}
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => return Err(SolidError::sm_err(OP)),
        }

        self.conic_solid(k, brep, &cylinder, radius, radius)?;

        brep.set_editing_enabled(false);
        Ok(())
    }

    /// `sm_CreateConicSolid`: the face of a bounded cone or cylinder surface and planar
    /// caps at its ends (each cap's edge curve made only for a cap that is built),
    /// stitched when a cap is made.
    fn conic_solid(
        &mut self,
        k: &mut Kernel,
        brep: &mut Brep,
        cone: &Cone,
        base: f64,
        top: f64,
    ) -> Result<()> {
        let domain = cone.rev.base().natural_uv_domain();
        cone_face(k, brep, cone, domain)?;
        let tolerance = brep.tolerance();

        // make base face
        let base_face = if base > tolerance {
            // only make the edge curve for a cap that is built
            let base_edge = cone
                .create_iso_parametric_curve(k, SurfParamType::V, domain.min().y, None, false)?
                .curve;
            planar_face(k, brep, self.region(), &[base_edge], tolerance)?
        } else {
            None
        };
        let top_face = if top > tolerance {
            let top_edge = cone
                .create_iso_parametric_curve(k, SurfParamType::V, domain.max().y, None, false)?
                .curve;
            planar_face(k, brep, self.region(), &[top_edge], tolerance)?
        } else {
            None
        };
        if base_face.is_some() || top_face.is_some() {
            stitch(k, brep)?;
        }
        Ok(())
    }

    /// Full analytic sphere. Maps clear even if surface creation fails; the
    /// temporary edit mode restores on every exit.
    pub fn create_sphere(
        &mut self,
        k: &mut Kernel,
        brep: &mut Brep,
        radius: f64,
        frame: &Axis2Placement,
    ) -> Result<()> {
        self.init_entity_maps();
        let editing = brep.editing_enabled();
        brep.set_editing_enabled(true);
        let result = (|| {
            let surface = sphere_surface(k, frame, radius)?;
            let domain = surface.base().base().natural_uv_domain();
            sphere_face(k, brep, &surface, domain).map(|_| ())
        })();
        brep.set_editing_enabled(editing);
        result
    }

    /// Full analytic torus, including the valid spindle representation.
    /// Maps clear and temporary edit mode restores even on constructor failure.
    pub fn create_torus(
        &mut self,
        k: &mut Kernel,
        brep: &mut Brep,
        major: f64,
        minor: f64,
        frame: &Axis2Placement,
    ) -> Result<()> {
        self.init_entity_maps();
        let editing = brep.editing_enabled();
        brep.set_editing_enabled(true);
        let result = (|| {
            let torus =
                Torus::create_canonical(k, frame, major, minor, TorusOptions::default(), None)?;
            let domain = torus.base().base().natural_uv_domain();
            torus_face(k, brep, &torus, domain).map(|_| ())
        })();
        brep.set_editing_enabled(editing);
        result
    }

    /// Planar boolean's ordered collection and empty-input early return. The
    /// result slot remains untouched on the empty path, including Difference.
    /// `notifications` belongs to the primary B-rep and its newly created faces.
    pub fn boolean_2d(
        &mut self,
        k: &mut Kernel,
        a: &mut Brep,
        b: &mut Brep,
        operation: Boolean2d,
        output: &mut Option<Brep>,
        marks: (
            &mut crate::topology::topology_traverser::Marks,
            &mut crate::topology::topology_traverser::Marks,
        ),
        notifications: &mut NotifyEnv<'_>,
    ) -> Result<()> {
        let previous = k.context.doing_boolean();
        let edit_a = a.editing_enabled();
        let edit_b = b.editing_enabled();
        let mut selected_result = false;
        k.context.set_doing_boolean(true);
        a.set_editing_enabled(true);
        b.set_editing_enabled(true);
        let result = (|| {
            let faces_a = crate::topology::brep::p1::faces(a, None)?;
            let faces_b = crate::topology::brep::p1::faces(b, None)?;
            if faces_a.is_empty() || faces_b.is_empty() {
                return Ok(());
            }
            let op = "primitive::primitive_creation::p1::boolean_2d";
            let tolerance = crate::math::max2(a.tolerance(), b.tolerance());
            let first = *faces_a.first().ok_or_else(|| SolidError::sm_err(op))?;
            let first_plane = a
                .face(first)?
                .surface
                .as_ref()
                .ok_or_else(|| SolidError::null_pointer(op))?
                .is_planar(k, tolerance)?;
            if !first_plane.is_planar {
                return Err(SolidError::invalid_input(op, "non-planar first surface"));
            }
            for (brep, faces) in [
                (&*a, faces_a.get(1..).unwrap_or_default()),
                (&*b, faces_b.as_slice()),
            ] {
                for face in faces {
                    let other = brep
                        .face(*face)?
                        .surface
                        .as_ref()
                        .ok_or_else(|| SolidError::null_pointer(op))?
                        .is_planar(k, tolerance)?;
                    if !other.is_planar {
                        return Err(SolidError::invalid_input(op, "non-planar surface"));
                    }
                    let d2 = ignored(crate::geom::geom_utility::p1::plane_point_distance(
                        first_plane.point,
                        first_plane.normal,
                        other.point,
                    ))?
                    .unwrap_or(crate::math::UNDEF_DOUBLE);
                    let d1 = ignored(crate::geom::geom_utility::p1::plane_point_distance(
                        other.point,
                        other.normal,
                        first_plane.point,
                    ))?
                    .unwrap_or(crate::math::UNDEF_DOUBLE);
                    if !(crate::math::max2(d2, d1) < tolerance * 2.0
                        && first_plane
                            .normal
                            .is_parallel_to(other.normal, EFF_ZERO_DEG))
                    {
                        return Err(SolidError::invalid_input(
                            op,
                            "surfaces are not coincident planes",
                        ));
                    }
                }
            }
            // extend the first surface's domain to cover all other face domains
            for (in_a, faces) in [
                (true, faces_a.get(1..).unwrap_or_default()),
                (false, faces_b.as_slice()),
            ] {
                for face in faces {
                    let brep: &Brep = if in_a { &*a } else { &*b };
                    let record = brep.face(*face)?;
                    let other_domain = record.uv_domain;
                    let other = record
                        .surface
                        .as_ref()
                        .ok_or_else(|| SolidError::null_pointer(op))?
                        .clone_for_evaluation()?;
                    let covered = cover_face_surface(
                        k,
                        a,
                        first,
                        &other,
                        tolerance * 2.0,
                        Some(&other_domain),
                    )?;
                    // Not upstream (WR-536, opt-in): b's faces are rebuilt on copies of
                    // the first surface below: one it does not cover would carry edges
                    // far off its surface
                    if k.fixes.boolean2d_plane_covers_far_faces && !covered {
                        return Err(SolidError::Upstream {
                            operation: op,
                            code: crate::error::SM_ERR_INVALID_INPUT,
                            entity: None,
                        });
                    }
                }
            }
            // extend the rest of the surface domains to cover the first surface's domain
            let first_surface = a
                .face(first)?
                .surface
                .as_ref()
                .ok_or_else(|| SolidError::null_pointer(op))?
                .clone_for_evaluation()?;
            for (brep, faces) in [
                (&mut *a, faces_a.get(1..).unwrap_or_default()),
                (&mut *b, faces_b.as_slice()),
            ] {
                for face in faces {
                    let covered =
                        cover_face_surface(k, brep, *face, &first_surface, tolerance * 2.0, None)?;
                    // (WR-536, opt-in) a plane that cannot be grown to cover the first face
                    if k.fixes.boolean2d_plane_covers_far_faces && !covered {
                        return Err(SolidError::Upstream {
                            operation: op,
                            code: crate::error::SM_ERR_INVALID_INPUT,
                            entity: None,
                        });
                    }
                }
            }
            super::boolean::finish(k, a, b, first, operation, output, marks, notifications)?;
            selected_result = true;
            Ok(())
        })();
        k.context.set_doing_boolean(previous);
        a.set_editing_enabled(edit_a);
        b.set_editing_enabled(edit_b);
        if selected_result {
            if let Some(result) = output.as_mut() {
                result.set_editing_enabled(edit_a);
            }
        }
        result
    }
}

/// `pFace->GetSurface()->CoverCoincidentSurface(pOther, tol, ..)` of `Boolean2D` on the
/// surface of face `face` of `brep`, followed by the `SmFace::RemoveUVTrimCurves`
/// (status ignored) a successful cover of a face-owned surface ends with. Returns
/// `bSuccess`, which upstream's `Boolean2D` ignores (WR-536 refuses on it).
///
/// Upstream's `STEPInversion` solves inside run on the face's own surface, whose
/// `GlobalPointSolve` classifies drops against the face (`GetFace()->PointClassify`).
/// The surface is moved out of its face for the call (keeping its owner, so the cache
/// it builds is its own) and lent with the B-rep through a face solve context; while it
/// is out, the face holds an evaluation copy of it for the B-rep reads of that cache
/// build. The real surface, possibly expanded, replaces the copy afterwards on every
/// exit, and the face tolerance a solve raised on the copy is synced back.
fn cover_face_surface(
    k: &Kernel,
    brep: &mut Brep,
    face: FaceId,
    other: &Surface,
    tol: f64,
    other_domain: Option<&Extent2d>,
) -> Result<bool> {
    use crate::query::brep::BrepFaceServices;
    use crate::topology::brep_cache::CacheObject;

    let op = "primitive::primitive_creation::p1::boolean_2d";
    let stand_in = brep
        .face(face)?
        .surface
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(op))?
        .clone_for_evaluation()?;
    let mut surface = brep
        .face_mut(face)?
        .surface
        .replace(stand_in)
        .ok_or_else(|| SolidError::null_pointer(op))?;
    let r = surface.cover_coincident_surface_deferring_trims_with_context(
        k,
        other,
        tol,
        other_domain,
        (
            &mut BrepFaceServices::new(&mut *brep),
            Some(CacheObject::Surface(face)),
        ),
    );
    brep.face_mut(face)?.surface = Some(surface);
    brep.sync_face_surface_tolerance(face)?;
    let r = r?;
    if r.remove_uv_trim_curves_of.is_some() {
        ignore_status(crate::topology::face::p1::remove_uv_trim_curves(brep, face))?;
    }
    Ok(r.success)
}

impl PrimitiveCreation {
    /// The curve overload of `CreateLinearSweep`. It consumes the curves, chooses
    /// a rectangular plane for an orthogonal line, a cylinder for a full circle
    /// swept along its axis, and an extrusion otherwise. Topology edits defer.
    pub fn create_linear_sweep(
        &mut self,
        k: &mut Kernel,
        brep: &mut Brep,
        curves: Vec<Curve>,
        direction: Vec3,
        distance: f64,
        cap_ends: bool,
    ) -> Result<()> {
        use crate::geom::vector2d::Vec2;
        use crate::surface::surf_of_extrusion::p1::SurfOfExtrusion;
        let previous = k.context.doing_boolean();
        k.context.set_doing_boolean(true);
        let result = (|| {
            // Every surface and the end cap use this unit direction and a positive
            // length, so they meet. A negative distance sweeps against the direction.
            let mut sweep_dir = direction.unitize()?;
            let mut distance = distance;
            if distance < 0.0 {
                sweep_dir *= -1.0;
                distance = -distance;
            }
            let mut analytic_domain = Extent2d::from_bounds(0.0, 0.0, 1.0, distance);
            for curve in &curves {
                let interval = match curve.kind() {
                    crate::curve::curve::CurveKind::Line(c) => c.step_interval(),
                    crate::curve::curve::CurveKind::Ellipse(c) => c.step_interval(),
                    crate::curve::curve::CurveKind::Hyperbola(c) => c.step_interval(),
                    crate::curve::curve::CurveKind::Parabola(c) => c.step_interval(),
                    _ => curve.natural_interval(),
                };
                analytic_domain.set_u_interval(&interval);
                let surface = if curve.is_linear(k, EFF_ZERO, false)?.linear {
                    let ends = curve.ends(k)?;
                    let tangent = (ends.end - ends.start).unitize_or_z();
                    if tangent.dot(sweep_dir).abs() < EFF_ZERO {
                        Some(Surface::from(Plane::new(
                            ends.start,
                            tangent,
                            sweep_dir,
                            Vec2::new((ends.end - ends.start).length(), distance),
                            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
                        )))
                    } else {
                        None
                    }
                } else if let Some((mut frame, radius, start, end)) =
                    curve.is_arc(k, 8, brep.tolerance())?
                {
                    let angle = end - start;
                    // A right cylinder only fits if the cap, translated along the
                    // sweep direction, is within tolerance of its far edge.
                    if angle < 360.0 + brep.tolerance()
                        && angle > 360.0 - brep.tolerance()
                        && distance * frame.z_axis().cross(sweep_dir).length() <= brep.tolerance()
                    {
                        if sweep_dir.dot(frame.z_axis()) < 0.0 {
                            frame.rotate_about_axis_at_point(
                                180.0 * core::f64::consts::PI / 180.0,
                                frame.origin(),
                                frame.x_axis(),
                            );
                        }
                        if let Some(mut cylinder) =
                            ignored(Cone::create_canonical_cylinder(k, &frame, radius))?
                        {
                            ignore_status(cylinder.adjust_step_uv_domain(
                                k,
                                &Extent2d::from_bounds(0.0, 0.0, 360.0, distance),
                            ))?;
                            let domain = cylinder.rev.base().natural_uv_domain();
                            ignore_status(cone_face(k, brep, &cylinder, domain))?;
                            continue;
                        }
                    }
                    None
                } else {
                    None
                };
                let Some(surface) = surface else {
                    let copy = curve.copy(k)?;
                    let mut extrusion = SurfOfExtrusion::create_canonical(k, copy, sweep_dir)?;
                    ignore_status(extrusion.adjust_step_uv_domain(k, &analytic_domain))?;
                    let domain = extrusion.base().natural_uv_domain();
                    ignore_status(extrusion_face(k, brep, &extrusion, domain))?;
                    continue;
                };
                let domain = surface.natural_uv_domain_with_nurb(k)?;
                ignore_status(create_face(k, brep, surface, domain))?;
            }
            if cap_ends {
                ignore_status(planar_faces(
                    k,
                    brep,
                    self.region(),
                    &curves,
                    brep.tolerance(),
                ))?;
                let mut translation = Axis2Placement::IDENTITY;
                translation.translate(sweep_dir * distance);
                let mut end_curves = Vec::new();
                for curve in &curves {
                    let mut copy = curve.copy(k)?;
                    ignore_status(copy.transform(k, &translation, None))?;
                    end_curves.push(copy);
                }
                ignore_status(planar_faces(
                    k,
                    brep,
                    self.region(),
                    &end_curves,
                    brep.tolerance(),
                ))?;
            }
            ignore_status(stitch(k, brep))?;
            brep.set_editing_enabled(false);
            Ok(())
        })();
        drop(curves);
        k.context.set_doing_boolean(previous);
        result
    }

    /// Assemble a sphere from two revolved centre patches and two trimmed caps,
    /// stitch at ten times the given tolerance, then translate to the centre.
    /// `notifications` supplies the caller's topology records, attributes and keys.
    pub fn create_sphere_no_pole(
        k: &mut Kernel,
        brep: &mut Brep,
        radius: f64,
        center: Vec3,
        tolerance: f64,
        notifications: &mut NotifyEnv<'_>,
    ) -> Result<()> {
        use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
        use crate::nurbs::crv_basic::ConicType;
        use crate::nurbs::crv_common::{circ_arc, Arc};
        use crate::nurbs::srf_common::revolved_srf;
        use crate::topology::face::p2::FaceCurves;
        let op = "primitive::primitive_creation::p1::create_sphere_no_pole";
        let start = -((0.8_f64 / 0.6).atan() * (180.0 / core::f64::consts::PI));
        let mut arc = circ_arc(
            &Arc {
                center: Vec3::ZERO,
                x: Vec3::X,
                y: Vec3::Y,
                r: radius,
                start,
                end: -start,
            },
            ConicType::Quadratic,
        )?;
        let mut patch = revolved_srf(&mut arc, Vec3::ZERO, Vec3::Y, 180.0, ConicType::Quadratic)?;
        let first_surface = BSplineSurface::from_gw_nurb(&patch);
        let first = create_face_with_notifications(
            k,
            brep,
            Surface::from(first_surface.clone()),
            first_surface.natural_uv_domain(),
            notifications,
        )?;
        let first_edges = face_edges(k, brep, first)?;
        patch.rotate_at_pt(Vec3::ZERO, Vec3::Y, 180.0)?;
        let second_surface = BSplineSurface::from_gw_nurb(&patch);
        let second = create_face_with_notifications(
            k,
            brep,
            Surface::from(second_surface.clone()),
            second_surface.natural_uv_domain(),
            notifications,
        )?;
        let second_edges = face_edges(k, brep, second)?;
        let mut dropped = Vec::new();
        for (rotation, index) in [(-90.0, 3), (180.0, 1)] {
            patch.rotate_at_pt(Vec3::ZERO, Vec3::X, rotation)?;
            let surface = Surface::from(BSplineSurface::from_gw_nurb(&patch));
            let domain = surface.natural_uv_domain_with_nurb(k)?;
            let mut boundary = FaceCurves {
                curve_loops: vec![2],
                curves_3d: Some(Vec::new()),
                uv_curves: Some(Vec::new()),
                ..FaceCurves::default()
            };
            for (edges, face) in [(&first_edges, first), (&second_edges, second)] {
                let edge = *edges
                    .get(index)
                    .ok_or_else(|| SolidError::invalid_input(op, "sphere patch boundary order"))?;
                let original = brep
                    .edge(edge)?
                    .curve
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(op))?;
                let copy = original.copy(k)?;
                ignore_status(drop_uv_curves(k, &surface, &copy, tolerance, &mut dropped))?;
                let uv = dropped
                    .first()
                    .ok_or_else(|| SolidError::null_pointer(op))?
                    .copy(k)?;
                let edgeuse =
                    crate::topology::edge::upward_edgeuse_of_face(brep, edge, Some(face))?
                        .ok_or_else(|| SolidError::null_pointer(op))?;
                let orient = crate::topology::edgeuse::p1::orientation(brep, edgeuse)?;
                boundary.curve_orients.push(if orient == OrientType::Same {
                    OrientType::Opposite
                } else {
                    OrientType::Same
                });
                if let Some(curves) = boundary.curves_3d.as_mut() {
                    curves.push(copy);
                }
                if let Some(curves) = boundary.uv_curves.as_mut() {
                    curves.push(uv);
                }
            }
            ignore_status(detailed_face(
                k,
                brep,
                brep.infinite_region(),
                boundary,
                surface,
                domain,
                notifications,
            ))?;
        }
        let keys =
            |object| crate::topology::brep::p2::caller_cache_key(notifications.key_of, object);
        let identity = crate::query::brep::BrepIdentity {
            keys: &keys,
            ..crate::query::brep::BrepIdentity::default()
        };
        let status = match crate::topology::brep::p4::stitch_faces_with_identity(
            k,
            brep,
            10.0 * tolerance,
            None,
            false,
            Some(identity),
        )
        .map(drop)
        {
            Err(e) if e.is_unported() => return Err(e),
            other => other,
        };
        let mut translation = Axis2Placement::IDENTITY;
        translation.translate(center);
        ignore_status(crate::topology::brep::p5::transform_with_notifications(
            k,
            brep,
            &translation,
            None,
            true,
            Some(notifications),
        ))?;
        status
    }

    /// Elliptic-ended cone. NLib errors map to `SM_ERR` before any B-rep edit.
    pub fn create_cone_elliptic_ends(
        k: &mut Kernel,
        brep: &mut Brep,
        inputs: &crate::nurbs::srf_nurb::EllipticCone,
        cap_ends: bool,
    ) -> Result<()> {
        use crate::nurbs::b_spline_surface::p2::IsoCurveOptions;
        let nurb = crate::nurbs::srf_nurb::cone_elliptic_ends(inputs).map_err(|_| {
            SolidError::sm_err("primitive::primitive_creation::p1::create_cone_elliptic_ends")
        })?;
        let surface = BSplineSurface::from_gw_nurb(&nurb);
        create_face(
            k,
            brep,
            Surface::from(surface.clone()),
            surface.natural_uv_domain(),
        )?;
        if !cap_ends {
            return Ok(());
        }
        let mut front = surface
            .create_iso_parametric_curve(k, SurfParamType::U, 0.0, IsoCurveOptions::default())?
            .curve;
        let rear = surface
            .create_iso_parametric_curve(k, SurfParamType::U, 1.0, IsoCurveOptions::default())?
            .curve;
        let interval = front.natural_interval();
        ignore_status(front.reverse_parameterization(&interval))?;
        planar_face(
            k,
            brep,
            brep.infinite_region(),
            &[Curve::from(front)],
            0.001,
        )?;
        planar_face(k, brep, brep.infinite_region(), &[Curve::from(rear)], 0.001)?;
        stitch(k, brep)
    }
}

/// Create an analytic rotational sweep (`sm_CreateAnalyticRotationalSweep`).
/// Lines on the axis, interior axis crossings and unsuitable circular arcs return
/// `SM_ERR`; topology construction defers at its exact owning unit.
pub fn analytic_rotational_sweep(
    k: &mut Kernel,
    brep: &mut Brep,
    curve: &Curve,
    axis_base: crate::math::Point3,
    axis: Vec3,
) -> Result<()> {
    use crate::curve::curve::Curve;
    use crate::curve::line::Line;
    use crate::geom::geom_utility::p1::{line_closest_point, line_point_distance};
    use crate::geom::solution_array::SolutionType;
    use crate::math::{Vec3, EFF_ZERO_SQRT};
    let op = "primitive::primitive_creation::p1::analytic_rotational_sweep";
    let tol = brep.tolerance();
    let ends = curve.ends(k)?;
    let (start_distance, _) = line_point_distance(axis_base, axis, ends.start);
    let (end_distance, _) = line_point_distance(axis_base, axis, ends.end);
    let mut analytic = None;
    if curve.is_linear(k, EFF_ZERO, false)?.linear {
        let length = (ends.end - ends.start).length();
        if length < tol {
            return Err(SolidError::invalid_input(
                op,
                "line shorter than B-rep tolerance",
            ));
        }
        let line = (ends.end - ends.start) / length;
        let scale = crate::math::max4(start_distance, end_distance, length, EFF_ZERO_SQRT);
        let angle = tol / scale * (180.0 / core::f64::consts::PI);
        if line.is_parallel_to(axis, angle) {
            if start_distance <= tol {
                return Err(SolidError::sm_err(op));
            }
            let y = line.cross(ends.start - axis_base).unitize()?;
            let x = y.cross(line);
            let base = axis_base + axis * line_closest_point(axis_base, axis, ends.start);
            let frame = Axis2Placement::new(base, x, y);
            if let Some(mut cylinder) =
                ignored(Cone::create_canonical_cylinder(k, &frame, start_distance))?
            {
                ignore_status(
                    cylinder
                        .adjust_step_uv_domain(k, &Extent2d::from_bounds(0.0, 0.0, 360.0, length)),
                )?;
                analytic = Some(AnalyticSurface::Cone(cylinder));
            }
        } else if line.is_perpendicular_to(axis, angle) {
            let base = axis_base + axis * line_closest_point(axis_base, axis, ends.start);
            let end_radius = line_point_distance(axis_base, axis, ends.end).0;
            let radial = if start_distance > end_radius {
                line * -1.0
            } else {
                line
            };
            let frame = Axis2Placement::new(base, radial, axis.cross(radial));
            let zero = crate::geom::tol::scaled_zero2(&ends.start, &ends.end);
            let mut circles = Vec::new();
            for radius in [start_distance, end_radius] {
                if radius > zero {
                    circles.push(Curve::from(Ellipse::create_canonical_circle(
                        k, &frame, radius, None, None,
                    )?));
                }
            }
            return match planar_face(k, brep, brep.infinite_region(), &circles, tol) {
                Ok(_) => Ok(()),
                Err(e) if e.is_unported() => Err(e),
                Err(_) => Err(SolidError::sm_err(op)),
            };
        } else {
            let (bottom_radius, bottom_parameter) =
                line_point_distance(axis_base, axis, ends.start);
            let (top_radius, top_parameter) = line_point_distance(axis_base, axis, ends.end);
            let axis_line = Curve::from(Line::infinite(axis_base, axis, 3, false));
            let solutions = ignored(axis_line.global_curve_intersect(
                k,
                &axis_line.natural_interval(),
                curve,
                &curve.natural_interval(),
                tol,
                &intersection_options(axis_line.natural_interval(), curve.natural_interval()),
            ))?;
            if let Some(solutions) = solutions {
                if !solutions.is_empty() {
                    let first = solutions.get_at(0)?;
                    if first.solution_type == SolutionType::SingleValue {
                        let interval = curve.natural_interval();
                        let param_tol = EFF_ZERO_SQRT / 100.0 * (1.0 + interval.length());
                        let t = first.start.param(1)?;
                        if !((interval.min() - t).abs() < param_tol
                            || (interval.max() - t).abs() < param_tol)
                        {
                            return Err(SolidError::sm_err(op));
                        }
                    } else if first.solution_type == SolutionType::RangeOfValues {
                        return Err(SolidError::sm_err(op));
                    }
                }
            }
            let bottom = axis_line.evaluate_point(k, bottom_parameter)?;
            let top = axis_line.evaluate_point(k, top_parameter)?;
            let height = (bottom - top).length();
            let tmp = if axis.dot(line) < 0.0 {
                axis * -1.0
            } else {
                axis
            };
            let y = tmp.cross(ends.start - axis_base).unitize()?;
            let frame = Axis2Placement::new(bottom, y.cross(tmp), y);
            if let Some(cone) = ignored(Cone::create_canonical(
                k,
                &frame,
                bottom_radius,
                top_radius,
                height,
                false,
                false,
            ))? {
                analytic = Some(AnalyticSurface::Cone(cone));
            }
        }
    } else if let Some((frame, radius, start, end)) = curve.is_arc(k, 8, tol)? {
        let angle = end - start;
        if angle < 180.0 + tol && angle > 180.0 - tol {
            if line_point_distance(axis_base, axis, ends.start).0 > tol
                || line_point_distance(axis_base, axis, ends.end).0 > tol
            {
                return Err(SolidError::sm_err(op));
            }
            if let Some(sphere) = ignored(sphere_surface(k, &frame, radius))? {
                analytic = Some(AnalyticSurface::Sphere(sphere));
            }
        } else if angle < 360.0 + tol && angle > 360.0 - tol {
            let axis_line = Curve::from(Line::infinite(axis_base, axis, 3, false));
            if let Some(solutions) = ignored(axis_line.global_curve_intersect(
                k,
                &axis_line.natural_interval(),
                curve,
                &curve.natural_interval(),
                tol,
                &intersection_options(axis_line.natural_interval(), curve.natural_interval()),
            ))? {
                if !solutions.is_empty() {
                    return Err(SolidError::sm_err(op));
                }
            }
            let (major, param) = line_point_distance(axis_base, axis, frame.origin());
            let origin = axis_base + axis * param;
            let normal = ignored(crate::curve::curve::p3::compute_crvs_normal(
                k,
                core::slice::from_ref(curve),
            ))?
            .unwrap_or(Vec3::UNDEF);
            let frame = Axis2Placement::new(origin, normal, normal.cross(axis));
            if let Some(torus) = ignored(Torus::create_canonical(
                k,
                &frame,
                major,
                radius,
                TorusOptions::default(),
                None,
            ))? {
                analytic = Some(AnalyticSurface::Torus(torus));
            }
        } else {
            return Err(SolidError::sm_err(op));
        }
    } else {
        return Err(SolidError::sm_err(op));
    }
    let Some(surface) = analytic else {
        return Err(SolidError::sm_err(op));
    };
    match analytic_face(k, brep, surface) {
        Ok(_) => Ok(()),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Err(SolidError::sm_err(op)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::topology_traverser::Marks;
    use crate::topology::{Face, ShellId};

    fn plane_brep(angle: f64) -> Brep {
        let angle = angle.to_radians();
        let frame = Axis2Placement::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(0.0, angle.cos(), angle.sin()),
        );
        let mut b = Brep::new();
        let mut face = Face::new(ShellId::from_index(0));
        face.uv_domain = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut plane = Plane::create_canonical(&frame).unwrap();
        plane.adjust_step_uv_domain(&face.uv_domain).unwrap();
        face.surface = Some(Surface::from(plane));
        b.add_face_record(face).unwrap();
        b
    }

    #[test]
    fn failed_static_circle_geometry_reports_native_sm_err() {
        let frame = Axis2Placement::default();
        for radius in [-1.0, 0.0, EFF_ZERO * 0.5] {
            let error = circle_curve(radius, &frame).unwrap_err();
            assert!(matches!(
                error,
                SolidError::Upstream {
                    operation: "primitive::primitive_creation::p1::create_circle",
                    code: crate::error::SM_ERR,
                    ..
                }
            ));
        }
        let circle = circle_curve(0.5, &frame).unwrap();
        let point = circle
            .evaluate_point(circle.natural_interval().min())
            .unwrap();
        assert_eq!(point, Vec3::new(0.5, 0.0, 0.0));
    }

    #[test]
    fn boolean_2d_covers_plane_domains_before_intersection() {
        let mut k = Kernel::default();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let mut notifications = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &|_| unreachable!("validation stops before topology edits"),
        };
        let mut a = plane_brep(0.0);
        let mut b = plane_brep(0.0);
        let fa = a.face_list()[0];
        let fb = b.face_list()[0];
        let domain = Extent2d::from_bounds(-2.0, -3.0, 4.0, 5.0);
        b.face_mut(fb).unwrap().uv_domain = domain;
        b.face_mut(fb)
            .unwrap()
            .surface
            .as_mut()
            .unwrap()
            .adjust_step_uv_domain(&k, &domain)
            .unwrap();
        let mut marks_a = Marks::default();
        let mut marks_b = Marks::default();
        let error = PrimitiveCreation::new(None, 17, true)
            .boolean_2d(
                &mut k,
                &mut a,
                &mut b,
                Boolean2d::Union,
                &mut None,
                (&mut marks_a, &mut marks_b),
                &mut notifications,
            )
            .unwrap_err();
        // past IntersectInsertRelate: copying B's unrelated face needs its face use,
        // which the bare fixture face record lacks
        assert!(
            matches!(
                error,
                SolidError::InvalidInput {
                    operation: "topology::face::p2",
                    ..
                }
            ),
            "{error:?}"
        );
        let covered_a = a
            .face(fa)
            .unwrap()
            .surface
            .as_ref()
            .unwrap()
            .natural_uv_domain_if_built();
        let covered_b = b
            .face(fb)
            .unwrap()
            .surface
            .as_ref()
            .unwrap()
            .natural_uv_domain_if_built();
        assert!(covered_a.min().x <= -2.0 && covered_a.min().y <= -3.0);
        assert!(covered_a.max().x >= 4.0 && covered_a.max().y >= 5.0);
        assert_eq!(covered_a, covered_b);
    }

    #[test]
    fn boolean_2d_uses_vector_default_parallel_tolerance_and_restores_state() {
        for (angle, invalid) in [
            (0.0, false),
            (0.00005, false),
            (0.0002, true),
            (0.01, true),
            (180.0, false),
        ] {
            let mut k = Kernel::default();
            let mut records = crate::topology::topology::NotifyRecords::new();
            let mut attributes = crate::geom::attribute::AttributeStore::default();
            let mut notifications = NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: None,
                key_of: &|_| unreachable!("validation stops before topology edits"),
            };
            let mut a = plane_brep(0.0);
            let mut b = plane_brep(angle);
            b.set_editing_enabled(true);
            let mut output = Some(Brep::new());
            let mut marks_a = Marks::default();
            let mut marks_b = Marks::default();
            let error = PrimitiveCreation::new(None, 17, true)
                .boolean_2d(
                    &mut k,
                    &mut a,
                    &mut b,
                    Boolean2d::Union,
                    &mut output,
                    (&mut marks_a, &mut marks_b),
                    &mut notifications,
                )
                .unwrap_err();
            // a coplanar pair goes on through IntersectInsertRelate and stops copying
            // B's bare face record (no face use); the others fail the coplanar check
            let expected = if invalid {
                "primitive::primitive_creation::p1::boolean_2d"
            } else {
                "topology::face::p2"
            };
            assert!(
                matches!(error, SolidError::InvalidInput { operation, .. } if operation == expected),
                "{angle}: {error}"
            );
            assert!(!a.editing_enabled());
            assert!(b.editing_enabled());
            assert!(!k.context.doing_boolean());
            assert!(output.is_some());
            assert!(a
                .face_list()
                .iter()
                .all(|f| a.face(*f).unwrap().surface.is_some()));
        }
    }
}

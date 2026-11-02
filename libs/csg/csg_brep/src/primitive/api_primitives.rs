//! High-level primitive creation (`SM_API` primitives): bounded plane, cylinder, cone
//! and torus surfaces in the canonical XY frame at a point, and the B-rep primitives
//! (plane and disc sheets, box, sphere, cylinder, cone, torus, pyramid, cylindrical
//! box, their partial variants, a pole-free sphere, an elliptic-ended cone, blend
//! faces and swung solids) with their input validation.
//!
//! Upstream: `source/SM_API/src/SmApiPrimitives.cpp`, `source/SM_API/inc/SmApiPrimitives.h`.
//!
//! | Rust                               | Upstream                                              |
//! |------------------------------------|-------------------------------------------------------|
//! | `is_positive_dim` (private)        | `SmApiIsPositiveDim` (static)                         |
//! | `is_non_zero_radius` (private)     | `SmApiIsNonZeroRadius` (static)                       |
//! | `cone_dims_valid` (private)        | `SmApiConeDimsValid` (static)                         |
//! | [`create_plane`]                   | `SmApiCreatePlane` (`SmBrep*&` overload)              |
//! | [`create_plane_surface`]           | `SmApiCreatePlane` (`SmPlane*&` overload)             |
//! | [`create_planar_circle`]           | `SmApiCreatePlanarCircle`                             |
//! | [`create_box`]                     | `SmApiCreateBox`                                      |
//! | [`create_sphere`]                  | `SmApiCreateSphere`                                   |
//! | [`create_cylinder`]                | `SmApiCreateCylinder` (`SmBrep*&` overload)           |
//! | [`create_cylinder_surface`]        | `SmApiCreateCylinder` (`SmSurface*&` overload)        |
//! | [`create_cone`]                    | `SmApiCreateCone` (`SmBrep*&` overload)               |
//! | [`create_cone_surface`]            | `SmApiCreateCone` (`SmSurface*&` overload)            |
//! | [`create_torus`]                   | `SmApiCreateTorus` (`SmBrep*&` overload)              |
//! | [`create_torus_surface`]           | `SmApiCreateTorus` (`SmSurface*&` overload)           |
//! | [`create_pyramid`]                 | `SmApiCreatePyramid`                                  |
//! | [`create_cylindrical_box`]         | `SmApiCreateCylindricalBox`                           |
//! | [`create_partial_sphere`]          | `SmApiCreatePartialSphere`                            |
//! | [`create_sphere_no_pole`]          | `SmApiCreateSphereNoPole`                             |
//! | [`create_partial_cone`]            | `SmApiCreatePartialCone`                              |
//! | [`create_cone_elliptic_ends`], [`ConeEllipticEnds`] | `SmApiCreateConeEllipticEnds`        |
//! | [`create_partial_cylinder`]        | `SmApiCreatePartialCylinder`                          |
//! | [`create_partial_torus`]           | `SmApiCreatePartialTorus`                             |
//! | [`create_blend_primitive`]         | `SmApiCreateBlendPrimitive`                           |
//! | [`create_swung_primitive`]         | `SmApiCreateSwungPrimitive`                           |
//! | [`create_offset_profile`]          | `SmApiCreateOffsetProfile`                            |
//!
//! Deviations:
//! - **Out-params.** The created surface or B-rep is the return value; on failure there
//!   is none (upstream clears `rpResult`). [`create_blend_primitive`] returns the new
//!   face's id.
//! - **Context.** `SmApiGetOrCreateContext()` is the caller's [`Kernel`].
//! - **Types.** The `SmSurface*&` overloads return the concrete surface: a [`Cone`] for
//!   the cylinder (an `SmCylinder`, [`Cone::is_cylinder_class`]) and the cone, a
//!   [`Torus`] for the torus. `SmBoolean` flags are `bool`. The curve lists of
//!   [`create_swung_primitive`] hold no `NULL` entries (upstream skips them); the B-rep,
//!   faces and edges of [`create_blend_primitive`] are a [`Brep`] and ids into it, so
//!   upstream's `NULL` checks (`SM_ERR_INVALID_INPUT`) hold by construction.
//! - **Statuses.** `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; errors of the
//!   called constructors pass through unchanged.
//! - [`create_sphere_no_pole_with_tolerance`] additionally exposes an explicit
//!   B-rep tolerance and the native constructor's boundary-fitting tolerance.
//! - **Context.** The B-rep entry points take the caller's [`Kernel`] (upstream's API
//!   context): `new SmBrep()` is [`crate::topology::brep::p3::new_brep`] and
//!   `SmPrimitiveCreation` runs on it directly (p1 methods) or through a
//!   [`PrimitiveModel`] (p2/p3 methods, which resolve their B-rep services through it).
//!   They stop at the first unported call inside `SmPrimitiveCreation`.
//! - [`create_offset_profile`] copies the borrowed curves (`SmCurve::Copy`) as upstream
//!   and lets [`p1::offset_profile`] own the copies; upstream's per-copy ownership
//!   bookkeeping after a partial transfer has nothing to free in Rust. A `None` curve is
//!   upstream's `NULL` entry.
//! - `SmApiCreateSkinPrimitive` and `SmApiCreateSkinFromFaces`, also in this file,
//!   belong to other units.
//! - No recursion.

use crate::curve::curve::Curve;
use crate::curve::line::Line;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent2d::Extent2d;
use crate::geom::tol::{USE_DEFAULT, ZONE_TOL_3D};
use crate::geom::vector3d::{is_valid_double, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::EFF_ZERO;
use crate::nurbs::srf_nurb::EllipticCone;
use crate::primitive::primitive_creation::model::PrimitiveModel;
use crate::primitive::primitive_creation::p1::{self, PrimitiveCreation};
use crate::surface::cone::Cone;
use crate::surface::plane::Plane;
use crate::surface::torus::{Torus, TorusOptions};
use crate::sweep::topology_sweep::access::SweepAccess;
use crate::topology::brep::p2::ArenaNotifyState;
use crate::topology::{Brep, EdgeId, FaceId};

const OP_BOX: &str = "primitive::api_primitives::create_box";
const OP_SPHERE: &str = "primitive::api_primitives::create_sphere";
const OP_CYLINDER: &str = "primitive::api_primitives::create_cylinder_surface";
const OP_CYLINDER_SOLID: &str = "primitive::api_primitives::create_cylinder";
const OP_CONE: &str = "primitive::api_primitives::create_cone";
const OP_CONE_SURFACE: &str = "primitive::api_primitives::create_cone_surface";
const OP_PYRAMID: &str = "primitive::api_primitives::create_pyramid";
const OP_CYLINDRICAL_BOX: &str = "primitive::api_primitives::create_cylindrical_box";
const OP_SPHERE_NO_POLE: &str = "primitive::api_primitives::create_sphere_no_pole";
const OP_OFFSET_PROFILE: &str = "primitive::api_primitives::create_offset_profile";

/// `SmApiIsPositiveDim`: a finite dimension (radius, length, height) above
/// `SM_EFF_ZERO`.
fn is_positive_dim(value: f64) -> bool {
    is_valid_double(value) && value > EFF_ZERO
}

/// `SmApiIsNonZeroRadius`: a finite, non-zero sphere radius (a negative radius is a
/// valid inside-out sphere).
fn is_non_zero_radius(radius: f64) -> bool {
    is_valid_double(radius) && radius.abs() > EFF_ZERO
}

/// `SmApiConeDimsValid`: finite radii that are not negative and not both zero, and a
/// positive height.
fn cone_dims_valid(radius_base: f64, radius_top: f64, height: f64) -> bool {
    if !is_valid_double(radius_base) || !is_valid_double(radius_top) {
        return false;
    }
    if radius_base < 0.0 || radius_top < 0.0 {
        return false;
    }
    if radius_base <= EFF_ZERO && radius_top <= EFF_ZERO {
        return false;
    }
    is_positive_dim(height)
}

/// The canonical XY frame at `origin` (`SmAxis2Placement` default constructor, then
/// `SetCanonical(origin, (1,0,0), (0,1,0))`, status ignored as upstream).
fn xy_frame(origin: Point3) -> Axis2Placement {
    let mut frame = Axis2Placement::default();
    let _ = frame.set_canonical(origin, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
    frame
}

/// The extent `[0, max_x] x [0, max_y]` (`SmExtent2d()` then `SetMinMax(0, 0, max_x,
/// max_y)`, status ignored as upstream: a negative bound leaves the empty default
/// extent).
fn extent_from_zero(max_x: f64, max_y: f64) -> Extent2d {
    let mut extent = Extent2d::default();
    let _ = extent.set_min_max_coords(0.0, 0.0, max_x, max_y);
    extent
}

/// The rectangular sheet B-rep of `x` by `y` in the XY plane with its corner at
/// `origin` (`SmApiCreatePlane`, `SmBrep*&` overload; tolerance `1.0e-5`).
///
/// # Errors
///
/// Construction errors, including [`SolidError::Unported`] from missing B-rep edits; upstream fails
/// with `SM_ERR` when no face is made.
pub fn create_plane(k: &mut Kernel, origin: Point3, x: f64, y: f64) -> Result<Brep> {
    let frame = xy_frame(origin);
    let mut brep = p1::create_rectangle(k, 1.0e-5, x, y, &frame)?;
    if brep.face_list().is_empty() {
        return Err(SolidError::sm_err(
            "primitive::api_primitives::create_plane",
        ));
    }
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(brep)
}

/// The plane bounded to `[0, length] x [0, width]` in the XY plane with its minimum
/// corner at `origin` (`SmApiCreatePlane`, `SmPlane*&` overload).
///
/// # Errors
///
/// Those of [`Plane::create_canonical`] and [`Plane::adjust_step_uv_domain`] (e.g. for
/// a zero length or width).
pub fn create_plane_surface(origin: Point3, length: f64, width: f64) -> Result<Plane> {
    let frame = xy_frame(origin);
    let mut plane = Plane::create_canonical(&frame)?;
    plane.adjust_step_uv_domain(&extent_from_zero(length, width))?;
    Ok(plane)
}

/// The disc sheet B-rep of `radius` in the XY plane centred at `origin`
/// (`SmApiCreatePlanarCircle`; tolerance `1.0e-5`).
///
/// # Errors
///
/// Construction errors, including [`SolidError::Unported`] from missing B-rep edits; upstream fails with
/// `SM_ERR` when no face is made.
pub fn create_planar_circle(k: &mut Kernel, origin: Point3, radius: f64) -> Result<Brep> {
    let frame = xy_frame(origin);
    let mut brep = p1::create_circle(k, 1.0e-5, radius, &frame)?;
    if brep.face_list().is_empty() {
        return Err(SolidError::sm_err(
            "primitive::api_primitives::create_planar_circle",
        ));
    }
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(brep)
}

/// The solid box of `length` (X) by `width` (Y) by `height` (Z) with its lower left
/// corner at `origin` (`SmApiCreateBox`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] unless every size is finite and above
/// `SM_EFF_ZERO`; then [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_box(
    k: &mut Kernel,
    origin: Point3,
    length: f64,
    width: f64,
    height: f64,
) -> Result<Brep> {
    if !is_positive_dim(length) || !is_positive_dim(width) || !is_positive_dim(height) {
        return Err(SolidError::invalid_input(OP_BOX, "non-positive size"));
    }
    let frame = xy_frame(origin);
    let (mut pc, mut brep) = new_primitive(k)?;
    pc.create_box(k, &mut brep, length, width, height, &frame)?;
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(brep)
}

/// The solid sphere of `radius` (negative: inside out) centred at `origin`, with poles
/// (`SmApiCreateSphere`: `CreateSphere(radius, 0, 360, frame)`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] unless the radius is finite and its magnitude above
/// `SM_EFF_ZERO`; then [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_sphere(k: &mut Kernel, origin: Point3, radius: f64) -> Result<Brep> {
    if !is_non_zero_radius(radius) {
        return Err(SolidError::invalid_input(OP_SPHERE, "zero radius"));
    }
    partial_sphere(k, origin, radius, 0.0, 360.0)
}

/// The solid cylinder of `radius` from the base circle centred at `origin` to
/// `height` along Z (`SmApiCreateCylinder`, `SmBrep*&` overload:
/// `CreateCylinder(height, radius, frame)`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] unless the radius and height are finite and above
/// `SM_EFF_ZERO`; those of [`PrimitiveCreation::create_cylinder`].
pub fn create_cylinder(k: &mut Kernel, origin: Point3, radius: f64, height: f64) -> Result<Brep> {
    if !is_positive_dim(radius) || !is_positive_dim(height) {
        return Err(SolidError::invalid_input(
            OP_CYLINDER_SOLID,
            "non-positive radius or height",
        ));
    }
    let frame = xy_frame(origin);
    let (mut pc, mut brep) = new_primitive(k)?;
    pc.create_cylinder(k, &mut brep, height, radius, &frame)?;
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(brep)
}

/// The cylinder surface (no caps) of `radius` about the Z axis through `origin`,
/// bounded to `[0, 360] x [0, height]` (`SmApiCreateCylinder`, `SmSurface*&`
/// overload).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] unless the radius and height are finite and above
/// `SM_EFF_ZERO`; those of [`Cone::create_canonical_cylinder`] and
/// [`Cone::adjust_step_uv_domain`].
pub fn create_cylinder_surface(
    k: &Kernel,
    origin: Point3,
    radius: f64,
    height: f64,
) -> Result<Cone> {
    if !is_positive_dim(radius) || !is_positive_dim(height) {
        return Err(SolidError::invalid_input(
            OP_CYLINDER,
            "non-positive radius or height",
        ));
    }
    let frame = xy_frame(origin);
    let mut cylinder = Cone::create_canonical_cylinder(k, &frame, radius)?;
    cylinder.adjust_step_uv_domain(k, &extent_from_zero(360.0, height))?;
    Ok(cylinder)
}

/// The solid cone from the base circle of `radius_base` centred at `origin` to the top
/// circle of `radius_top` at `height` along Z (`SmApiCreateCone`, `SmBrep*&`
/// overload: `CreateCone(height, radius_base, radius_top, 0, 360, frame)`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a non-finite or negative radius, both radii not
/// above `SM_EFF_ZERO`, or a height not finite and above `SM_EFF_ZERO`; then
/// [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_cone(
    k: &mut Kernel,
    origin: Point3,
    radius_base: f64,
    radius_top: f64,
    height: f64,
) -> Result<Brep> {
    if !cone_dims_valid(radius_base, radius_top, height) {
        return Err(SolidError::invalid_input(
            OP_CONE,
            "invalid cone dimensions",
        ));
    }
    partial_cone(k, origin, radius_base, radius_top, height, 0.0, 360.0)
}

/// The cone surface from the base circle of `radius_base` centred at `origin` to the
/// top circle of `radius_top` at `height` along Z, bounded to `[0, 360] x [0, height]`
/// (`SmApiCreateCone`, `SmSurface*&` overload).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] as [`create_cone`]; those of [`Cone::create_canonical`]
/// and [`Cone::adjust_step_uv_domain`].
pub fn create_cone_surface(
    k: &Kernel,
    origin: Point3,
    radius_base: f64,
    radius_top: f64,
    height: f64,
) -> Result<Cone> {
    if !cone_dims_valid(radius_base, radius_top, height) {
        return Err(SolidError::invalid_input(
            OP_CONE_SURFACE,
            "invalid cone dimensions",
        ));
    }
    let frame = xy_frame(origin);
    let mut cone =
        Cone::create_canonical(k, &frame, radius_base, radius_top, height, false, false)?;
    cone.adjust_step_uv_domain(k, &extent_from_zero(360.0, height))?;
    Ok(cone)
}

/// The solid torus of `radius_major` and `radius_minor` centred at `origin` about Z
/// (`SmApiCreateTorus`, `SmBrep*&` overload: `CreateTorus(major, minor, 0, 360,
/// frame)`).
///
/// # Errors
///
/// [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_torus(
    k: &mut Kernel,
    origin: Point3,
    radius_major: f64,
    radius_minor: f64,
) -> Result<Brep> {
    partial_torus(k, origin, radius_major, radius_minor, 0.0, 360.0)
}

/// The torus surface of `radius_major` and `radius_minor` centred at `origin` about Z
/// (`SmApiCreateTorus`, `SmSurface*&` overload: `SmTorus::CreateCanonical` with the
/// default options and domain).
///
/// # Errors
///
/// Those of [`Torus::create_canonical`] (`SM_ERR` for a radius not above
/// `SM_EFF_ZERO`).
pub fn create_torus_surface(
    k: &Kernel,
    origin: Point3,
    radius_major: f64,
    radius_minor: f64,
) -> Result<Torus> {
    let frame = xy_frame(origin);
    Torus::create_canonical(
        k,
        &frame,
        radius_major,
        radius_minor,
        TorusOptions::default(),
        None,
    )
}

/// The solid pyramid on the square base of side `length` centred at `base_center` in
/// the XY plane, with its apex `height` above the centre along Z
/// (`SmApiCreatePyramid`): the base and four triangular side faces
/// ([`crate::topology::brep::p8::create_planar_face_with_3d_curves`], tolerance `1e-5`)
/// stitched and oriented into a closed solid
/// ([`crate::topology::brep::p5::stitch_and_orient`]). The B-rep is `new SmBrep()` in
/// `k`'s context ([`crate::topology::brep::p3::new_brep`]); its edits notify an
/// arena-only store (no observer holds identities for the new B-rep's objects).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a length or height not above `0` (NaN passes, as
/// upstream); those of [`Line::create_line_segment`] for the edges (corners that
/// coincide), of the face builder and of the stitching.
pub fn create_pyramid(
    k: &mut Kernel,
    base_center: Point3,
    length: f64,
    height: f64,
) -> Result<Brep> {
    if length <= 0.0 || height <= 0.0 {
        return Err(SolidError::invalid_input(
            OP_PYRAMID,
            "non-positive length or height",
        ));
    }

    let half = length / 2.0;

    // Square base corners (CCW in the XY plane) and the apex above the center.
    let base = [
        base_center + Vec3::new(-half, -half, 0.0),
        base_center + Vec3::new(half, -half, 0.0),
        base_center + Vec3::new(half, half, 0.0),
        base_center + Vec3::new(-half, half, 0.0),
    ];
    let apex = base_center + Vec3::new(0.0, 0.0, height);
    let tol = 1.0e-5;

    // `new SmBrep()`: the B-rep and its infinite void region
    let mut brep = crate::topology::brep::p3::new_brep(k, USE_DEFAULT, USE_DEFAULT)?;
    let region = brep.infinite_region();

    let mut state = ArenaNotifyState::new(&brep);
    let mut env = state.env(None);
    let mut marks = crate::topology::topology_traverser::Marks::default();
    let this_face =
        |f: FaceId| crate::topology::loopuse::notify_ref(crate::error::EntityRef::Face(f));
    let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
        env: &mut env,
        marks: &mut marks,
        this_face: &this_face,
    };

    // Square base face: c0 -> c1 -> c2 -> c3.
    let mut base_loop: Vec<Curve> = Vec::new();
    for (ii, start) in base.iter().enumerate() {
        let end = base.get((ii + 1) % 4).copied().unwrap_or(*start);
        base_loop.push(Curve::from(Line::create_line_segment(
            3, *start, end, None,
        )?));
    }
    crate::topology::brep::p8::create_planar_face_with_3d_curves(
        k, &mut brep, region, &base_loop, tol, &mut fenv,
    )?;

    // Four triangular side faces: c_i -> c_{i+1} -> apex.
    for ii in 0..4 {
        let p0 = *base
            .get(ii)
            .ok_or(SolidError::invalid_input(OP_PYRAMID, "corner"))?;
        let p1 = *base
            .get((ii + 1) % 4)
            .ok_or(SolidError::invalid_input(OP_PYRAMID, "corner"))?;
        let side = vec![
            Curve::from(Line::create_line_segment(3, p0, p1, None)?),
            Curve::from(Line::create_line_segment(3, p1, apex, None)?),
            Curve::from(Line::create_line_segment(3, apex, p0, None)?),
        ];
        crate::topology::brep::p8::create_planar_face_with_3d_curves(
            k, &mut brep, region, &side, tol, &mut fenv,
        )?;
    }

    let keys = |object| crate::topology::brep::p2::caller_cache_key(env.key_of, object);
    crate::topology::brep::p5::stitch_and_orient_with_identity(
        k,
        &mut brep,
        &crate::topology::brep::p5::StitchAndOrientOptions::default(),
        Some(crate::query::brep::BrepIdentity {
            keys: &keys,
            ..crate::query::brep::BrepIdentity::default()
        }),
    )?;
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(brep)
}

/// The solid sector of the hollow cylinder of `inside_radius` and `outside_radius`
/// about the Z axis through `origin`, `length` along Z, from `start_angle_deg` to
/// `end_angle_deg` about Z from X (`SmApiCreateCylindricalBox`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] unless the length and inside radius are finite and
/// above `SM_EFF_ZERO`, the outside radius is finite and above the inside radius plus
/// `SM_EFF_ZERO`, the start angle is finite in `[-360, 360]`, and the end angle is
/// finite, above the start plus `SM_EFF_ZERO` and at most 360 degrees past it; then
/// [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_cylindrical_box(
    k: &mut Kernel,
    origin: Point3,
    length: f64,
    inside_radius: f64,
    outside_radius: f64,
    start_angle_deg: f64,
    end_angle_deg: f64,
) -> Result<Brep> {
    if !is_positive_dim(length)
        || !is_positive_dim(inside_radius)
        || !is_valid_double(outside_radius)
        || outside_radius <= inside_radius + EFF_ZERO
        || !is_valid_double(start_angle_deg)
        || !is_valid_double(end_angle_deg)
        || start_angle_deg < -360.0
        || start_angle_deg > 360.0
        || end_angle_deg <= start_angle_deg + EFF_ZERO
        || end_angle_deg - start_angle_deg > 360.0
    {
        return Err(SolidError::invalid_input(
            OP_CYLINDRICAL_BOX,
            "invalid cylindrical box dimensions",
        ));
    }
    let frame = xy_frame(origin);
    let (mut pc, mut brep) = new_primitive(k)?;
    let mut state = ArenaNotifyState::new(&brep);
    let mut env = state.env(None);
    pc.create_cylindrical_box(
        k,
        &mut brep,
        [
            length,
            inside_radius,
            outside_radius,
            start_angle_deg,
            end_angle_deg,
        ],
        &frame,
        &mut env,
    )?;
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(brep)
}

/// `SmPrimitiveCreation::CreateSphere(radius, start, end, frame)` into a new B-rep at
/// the XY frame of `origin` (the body shared by [`create_sphere`] and
/// [`create_partial_sphere`]).
fn partial_sphere(
    k: &mut Kernel,
    origin: Point3,
    radius: f64,
    start_angle_deg: f64,
    end_angle_deg: f64,
) -> Result<Brep> {
    let frame = xy_frame(origin);
    with_model(k, |pc, model| {
        pc.create_partial_sphere(
            model,
            PrimitiveModel::MAIN,
            radius,
            start_angle_deg,
            end_angle_deg,
            &frame,
        )
    })
}

/// The solid sphere sector of `radius` centred at `origin` from `start_angle_deg` to
/// `end_angle_deg` about Z from X (`SmApiCreatePartialSphere`; no input validation).
///
/// # Errors
///
/// [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_partial_sphere(
    k: &mut Kernel,
    origin: Point3,
    radius: f64,
    start_angle_deg: f64,
    end_angle_deg: f64,
) -> Result<Brep> {
    partial_sphere(k, origin, radius, start_angle_deg, end_angle_deg)
}

/// The solid sphere of `radius` (negative: inside out) centred at `center`, made of
/// patches without degenerate pole vertices (`SmApiCreateSphereNoPole`, tolerance
/// `SM_ZONE_TOL_3D`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] unless the radius is finite and its magnitude above
/// `SM_EFF_ZERO`, or an error constructing, projecting or stitching the patches.
pub fn create_sphere_no_pole(k: &mut Kernel, center: Point3, radius: f64) -> Result<Brep> {
    sphere_no_pole(k, center, radius, None)
}

/// A pole-free sphere with an explicit B-rep and boundary-fitting tolerance.
///
/// The tolerance must be finite and greater than `SM_EFF_ZERO`. Use a tighter
/// construction tolerance when precise mass properties must resolve the fitted
/// patch boundaries more accurately than the default `1e-5`.
///
/// # Errors
/// Invalid radius or tolerance, or an error constructing or stitching the patches.
pub fn create_sphere_no_pole_with_tolerance(
    k: &mut Kernel,
    center: Point3,
    radius: f64,
    tolerance: f64,
) -> Result<Brep> {
    if !is_positive_dim(tolerance) {
        return Err(SolidError::invalid_input(
            OP_SPHERE_NO_POLE,
            "invalid construction tolerance",
        ));
    }
    sphere_no_pole(k, center, radius, Some(tolerance))
}

fn sphere_no_pole(
    k: &mut Kernel,
    center: Point3,
    radius: f64,
    tolerance: Option<f64>,
) -> Result<Brep> {
    if !is_non_zero_radius(radius) {
        return Err(SolidError::invalid_input(OP_SPHERE_NO_POLE, "zero radius"));
    }
    let mut brep = new_brep(k)?;
    if let Some(tolerance) = tolerance {
        brep.set_tolerance(tolerance);
    }
    let mut state = ArenaNotifyState::new(&brep);
    let mut env = state.env(None);
    PrimitiveCreation::create_sphere_no_pole(
        k,
        &mut brep,
        radius,
        center,
        tolerance.unwrap_or(ZONE_TOL_3D),
        &mut env,
    )?;
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(brep)
}

/// `SmPrimitiveCreation::CreateCone(height, radius_base, radius_top, start, end,
/// frame)` into a new B-rep at the XY frame of `origin` (the body shared by
/// [`create_cone`] and [`create_partial_cone`]).
fn partial_cone(
    k: &mut Kernel,
    origin: Point3,
    radius_base: f64,
    radius_top: f64,
    height: f64,
    start_angle_deg: f64,
    end_angle_deg: f64,
) -> Result<Brep> {
    let frame = xy_frame(origin);
    with_model(k, |pc, model| {
        pc.create_partial_cone(
            model,
            PrimitiveModel::MAIN,
            height,
            radius_base,
            radius_top,
            start_angle_deg,
            end_angle_deg,
            &frame,
        )?;
        // CreateCone leaves edit mode on, in which face caches ignore the trim loops
        model.set_editing(PrimitiveModel::MAIN, false)?;
        Ok(())
    })
}

/// The solid cone sector from the base circle of `radius_base` centred at `origin` to
/// the top circle of `radius_top` at `height` along Z, from `start_angle_deg` to
/// `end_angle_deg` about Z from X (`SmApiCreatePartialCone`; no input validation).
///
/// # Errors
///
/// [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_partial_cone(
    k: &mut Kernel,
    origin: Point3,
    radius_base: f64,
    radius_top: f64,
    height: f64,
    start_angle_deg: f64,
    end_angle_deg: f64,
) -> Result<Brep> {
    partial_cone(
        k,
        origin,
        radius_base,
        radius_top,
        height,
        start_angle_deg,
        end_angle_deg,
    )
}

/// The arguments of [`create_cone_elliptic_ends`] (`SmApiCreateConeEllipticEnds`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ConeEllipticEnds {
    /// `dFrontRadius`: the radius at the front end.
    pub front_radius: f64,
    /// `dRearRadius`: the radius at the rear end.
    pub rear_radius: f64,
    /// `dHeight`: the distance between the end-ellipse centres.
    pub height: f64,
    /// `dFrontYTiltDeg`: the front end's tilt about the X axis (degrees).
    pub front_y_tilt_deg: f64,
    /// `dRearYTiltDeg`: the rear end's tilt about the X axis (degrees).
    pub rear_y_tilt_deg: f64,
    /// `dFrontXTiltDeg`: the front end's tilt about the Y axis (degrees).
    pub front_x_tilt_deg: f64,
    /// `dRearXTiltDeg`: the rear end's tilt about the Y axis (degrees).
    pub rear_x_tilt_deg: f64,
    /// `bEndCaps`: close off the end caps.
    pub end_caps: bool,
}

/// The cone with elliptic ends and independent tilts (`SmApiCreateConeEllipticEnds`;
/// no input validation).
///
/// # Errors
///
/// [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_cone_elliptic_ends(k: &mut Kernel, params: &ConeEllipticEnds) -> Result<Brep> {
    let mut brep = new_brep(k)?;
    PrimitiveCreation::create_cone_elliptic_ends(
        k,
        &mut brep,
        &EllipticCone {
            radius_front: params.front_radius,
            radius_rear: params.rear_radius,
            height: params.height,
            tilt_y_front: params.front_y_tilt_deg,
            tilt_y_rear: params.rear_y_tilt_deg,
            tilt_x_front: params.front_x_tilt_deg,
            tilt_x_rear: params.rear_x_tilt_deg,
        },
        params.end_caps,
    )?;
    // (not upstream: the port's cached topology fields, true for the caller)
    brep.refresh_cached_topology();
    Ok(brep)
}

/// The solid cylinder sector of `radius` from the base circle centred at `origin` to
/// `height` along Z, from `start_angle_deg` to `end_angle_deg` about Z from X
/// (`SmApiCreatePartialCylinder`: [`create_partial_cone`] with equal radii).
///
/// # Errors
///
/// Those of [`create_partial_cone`].
pub fn create_partial_cylinder(
    k: &mut Kernel,
    origin: Point3,
    radius: f64,
    height: f64,
    start_angle_deg: f64,
    end_angle_deg: f64,
) -> Result<Brep> {
    create_partial_cone(
        k,
        origin,
        radius,
        radius,
        height,
        start_angle_deg,
        end_angle_deg,
    )
}

/// `SmPrimitiveCreation::CreateTorus(major, minor, start, end, frame)` into a new B-rep
/// at the XY frame of `origin` (the body shared by [`create_torus`] and
/// [`create_partial_torus`]).
fn partial_torus(
    k: &mut Kernel,
    origin: Point3,
    radius_major: f64,
    radius_minor: f64,
    start_angle_deg: f64,
    end_angle_deg: f64,
) -> Result<Brep> {
    let frame = xy_frame(origin);
    with_model(k, |pc, model| {
        pc.create_partial_torus(
            model,
            PrimitiveModel::MAIN,
            radius_major,
            radius_minor,
            start_angle_deg,
            end_angle_deg,
            &frame,
        )
    })
}

/// The solid torus sector of `radius_major` and `radius_minor` centred at `origin`
/// about Z, from `start_angle_deg` to `end_angle_deg` about Z from X
/// (`SmApiCreatePartialTorus`; no input validation).
///
/// # Errors
///
/// [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_partial_torus(
    k: &mut Kernel,
    origin: Point3,
    radius_major: f64,
    radius_minor: f64,
    start_angle_deg: f64,
    end_angle_deg: f64,
) -> Result<Brep> {
    partial_torus(
        k,
        origin,
        radius_major,
        radius_minor,
        start_angle_deg,
        end_angle_deg,
    )
}

/// The blend face between the boundary edges `edge1` (of `face1`) and `edge2` (of
/// `face2`) of `brep`, added to `brep` (`SmApiCreateBlendPrimitive`): from the start
/// of `edge1`'s curve to the start (`same_dir_curves`) or end of `edge2`'s curve.
/// Returns `None` on upstream success with a null output face: the face/edge
/// overload ignores the inner blend-construction status. This is distinct from
/// failure; `brep` retains any mutations in either successful outcome.
///
/// # Errors
///
/// Invalid edge/face associations and topology lookup/restoration errors;
/// [`SolidError::Unported`] from dependencies the native caller requires.
pub fn create_blend_primitive(
    k: &mut Kernel,
    brep: &mut Brep,
    edge1: EdgeId,
    face1: FaceId,
    edge2: EdgeId,
    face2: FaceId,
    same_dir_curves: bool,
) -> Result<Option<FaceId>> {
    let mut pc = PrimitiveCreation::new(brep.infinite_region(), 0, true);
    let mut model = PrimitiveModel::new(k, core::mem::take(brep));
    let mut blend_face = None;
    let result = pc.create_blend_primitive_between_faces(
        &mut model,
        PrimitiveModel::MAIN,
        (face1, edge1),
        (face2, edge2),
        same_dir_curves,
        None,
        &mut blend_face,
    );
    *brep = model.into_brep()?;
    result?;
    Ok(blend_face)
}

/// The swung solid of the XY path curves and XZ profile curves scaled by `scale`
/// (`SmApiCreateSwungPrimitive`, tolerance `SM_ZONE_TOL_3D`). The curves are not
/// consumed.
///
/// # Errors
///
/// [`SolidError::Unported`] (`SmBrep::SmBrep`).
pub fn create_swung_primitive(
    k: &mut Kernel,
    xy_curves: &[Curve],
    xz_curves: &[Curve],
    scale: f64,
) -> Result<Brep> {
    let paths: Vec<Option<Curve>> = xy_curves.iter().cloned().map(Some).collect();
    let profiles: Vec<Option<Curve>> = xz_curves.iter().cloned().map(Some).collect();
    with_model(k, |pc, model| {
        pc.create_swung_primitive(
            model,
            PrimitiveModel::MAIN,
            &paths,
            &profiles,
            scale,
            ZONE_TOL_3D,
        )
    })
}

/// The offset of the closed planar profile `curves` by `offset_distance`
/// (`SmApiCreateOffsetProfile`: [`p1::offset_profile`] at `SM_ZONE_TOL_3D` on copies of
/// the curves). `offset_type` is 1 left, 2 right, 3 both; `round_corners` rounds
/// expanding corners (else they are extended); `shell_result` (left or right only)
/// returns the region between the profile and its offset. The curves are not consumed.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a `None` curve; those of the curve copies and of
/// [`p1::offset_profile`].
pub fn create_offset_profile(
    k: &mut Kernel,
    planar_curves: &[Option<&Curve>],
    offset_distance: f64,
    offset_type: u64,
    round_corners: bool,
    shell_result: bool,
) -> Result<Brep> {
    // OffsetProfile consumes its curve array on success, so pass private copies
    let mut copies = Vec::new();
    for curve in planar_curves {
        let Some(curve) = curve else {
            return Err(SolidError::invalid_input(OP_OFFSET_PROFILE, "null curve"));
        };
        copies.push(curve.copy(k)?);
    }
    let mut result = p1::offset_profile(
        k,
        &copies,
        ZONE_TOL_3D,
        offset_distance,
        offset_type,
        round_corners,
        shell_result,
    )?;
    // (not upstream: the port's cached topology fields, true for the caller)
    result.refresh_cached_topology();
    Ok(result)
}

/// `new SmBrep()` in `k`'s context ([`crate::topology::brep::p3::new_brep`]).
fn new_brep(k: &mut Kernel) -> Result<Brep> {
    crate::topology::brep::p3::new_brep(k, USE_DEFAULT, USE_DEFAULT)
}

/// `new SmBrep()` and `SmPrimitiveCreation sPC(pBrep->GetInfiniteRegion())`.
fn new_primitive(k: &mut Kernel) -> Result<(PrimitiveCreation, Brep)> {
    let brep = new_brep(k)?;
    Ok((
        PrimitiveCreation::new(brep.infinite_region(), 0, true),
        brep,
    ))
}

/// Runs `body` on a new B-rep's [`PrimitiveCreation`] through a [`PrimitiveModel`] and
/// returns the B-rep.
fn with_model(
    k: &mut Kernel,
    body: impl FnOnce(&mut PrimitiveCreation, &mut PrimitiveModel<'_>) -> Result<()>,
) -> Result<Brep> {
    // (PrimitiveModel::build_new refreshes the port's cached topology fields)
    PrimitiveModel::build_new(k, body).map(|(brep, ())| brep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o() -> Point3 {
        Point3::new(0.0, 0.0, 0.0)
    }

    #[test]
    fn validators_match_upstream() {
        assert!(is_positive_dim(1.0));
        assert!(!is_positive_dim(EFF_ZERO));
        assert!(!is_positive_dim(f64::NAN));
        assert!(!is_positive_dim(f64::INFINITY));
        assert!(is_non_zero_radius(-5.0));
        assert!(!is_non_zero_radius(0.0));
        assert!(cone_dims_valid(0.0, 1.0, 1.0));
        assert!(!cone_dims_valid(0.0, 0.0, 1.0));
        assert!(!cone_dims_valid(-1.0, 10.0, 20.0));
        assert!(!cone_dims_valid(5.0, 10.0, -20.0));
    }

    #[test]
    fn invalid_inputs_are_rejected_before_the_kernel() {
        let k = &mut Kernel::default();
        assert!(matches!(
            create_box(k, o(), 0.0, 10.0, 20.0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            create_sphere(k, o(), 0.0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            create_cylinder(k, o(), -1.0, 20.0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            create_cylinder_surface(k, o(), 5.0, -20.0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            create_cone_surface(k, o(), 5.0, 10.0, -20.0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            create_pyramid(k, o(), 0.0, 1.0),
            Err(SolidError::InvalidInput { .. })
        ));
        for (a0, a1) in [(400.0, 450.0), (90.0, 10.0), (45.0, 45.0), (0.0, 400.0)] {
            assert!(matches!(
                create_cylindrical_box(k, o(), 10.0, 2.0, 5.0, a0, a1),
                Err(SolidError::InvalidInput { .. })
            ));
        }
        assert!(matches!(
            create_sphere_no_pole(k, o(), 0.0),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn complete_solids_construct_and_partial_solids_report_dependencies() {
        let k = &mut Kernel::default();
        for (built, counts) in [
            (create_box(k, o(), 5.0, 10.0, 20.0), (6, 12, 8)),
            (create_pyramid(k, o(), 2.0, 3.0), (5, 8, 5)),
            (create_cylinder(k, o(), 5.0, 10.0), (3, 3, 2)),
            (create_cone(k, o(), 5.0, 2.0, 10.0), (3, 3, 2)),
            (create_sphere(k, o(), 5.0), (1, 1, 2)),
            (create_sphere_no_pole(k, o(), 5.0), (4, 6, 4)),
            (create_sphere_no_pole(k, o(), -5.0), (4, 6, 4)),
            (create_torus(k, o(), 10.0, 2.0), (1, 2, 1)),
            (
                create_cylindrical_box(k, o(), 10.0, 2.0, 5.0, 0.0, 90.0),
                (6, 12, 8),
            ),
        ] {
            let b = built.unwrap();
            let faces = crate::topology::brep::p1::faces(&b, None).unwrap().len();
            let edges = crate::topology::brep::p1::edges(&b, None).unwrap().len();
            let vertices = crate::topology::brep::p1::vertices(&b, None).unwrap().len();
            assert_eq!((faces, edges, vertices), counts);
            assert!(crate::topology::brep::p2::is_manifold_solid(&b).unwrap());
        }
        let sphere = create_partial_sphere(k, o(), 5.0, 0.0, 90.0).unwrap();
        assert!(crate::topology::brep::p2::is_manifold_solid(&sphere).unwrap());
        let cylinder = create_partial_cylinder(k, o(), 5.0, 10.0, 0.0, 90.0).unwrap();
        assert!(crate::topology::brep::p2::is_manifold_solid(&cylinder).unwrap());
        // with `fixes.revolve_analytic_profile_faces` (WR-448, #545) the swept face is
        // an analytic torus; upstream's is a B-spline surface
        for (fix, expected) in [(false, 0), (true, 1)] {
            k.fixes.revolve_analytic_profile_faces = fix;
            let torus = create_partial_torus(k, o(), 10.0, 2.0, 0.0, 90.0).unwrap();
            assert!(crate::topology::brep::p2::is_manifold_solid(&torus).unwrap());
            let tori = torus
                .face_list()
                .iter()
                .filter(|&&f| {
                    let surface = torus.face(f).unwrap().surface.as_ref().unwrap();
                    matches!(
                        surface.kind(),
                        crate::surface::surface::SurfaceKind::Torus(_)
                    )
                })
                .count();
            assert_eq!(tori, expected, "{fix}");
        }
        k.fixes.revolve_analytic_profile_faces = false;
        // edges without an edge use on their face
        let mut brep = Brep::new_with_infinite_region().unwrap();
        let (edge, face) = (EdgeId::from_index(0), FaceId::from_index(0));
        assert!(create_blend_primitive(k, &mut brep, edge, face, edge, face, true).is_err());
    }

    #[test]
    fn blend_success_can_return_a_face_or_no_face() {
        let mut k = Kernel::default();
        for (edge2_index, face2_index, has_face) in [(3, 2, false), (2, 2, true), (1, 1, true)] {
            let mut brep = create_box(&mut k, Vec3::ZERO, 1.0, 1.0, 1.0).unwrap();
            let edges = crate::topology::brep::p1::edges(&brep, None).unwrap();
            let faces = crate::topology::brep::p1::faces(&brep, None).unwrap();
            let blend = create_blend_primitive(
                &mut k,
                &mut brep,
                edges[0],
                faces[1],
                edges[edge2_index],
                faces[face2_index],
                true,
            )
            .unwrap();
            assert_eq!(blend.is_some(), has_face);
            let after_faces = crate::topology::brep::p1::faces(&brep, None).unwrap();
            assert_eq!(after_faces.len(), faces.len() + usize::from(has_face));
            if let Some(face) = blend {
                assert!(after_faces.contains(&face));
                assert!(brep.face(face).unwrap().surface.is_some());
            } else {
                assert_eq!(
                    crate::topology::brep::p1::edges(&brep, None).unwrap(),
                    edges
                );
            }
        }

        // Missing edge/face association remains an error, not success/None.
        let mut brep = create_box(&mut k, Vec3::ZERO, 1.0, 1.0, 1.0).unwrap();
        let edges = crate::topology::brep::p1::edges(&brep, None).unwrap();
        let faces = crate::topology::brep::p1::faces(&brep, None).unwrap();
        assert!(create_blend_primitive(
            &mut k, &mut brep, edges[0], faces[0], edges[1], faces[1], true,
        )
        .is_err());
        assert_eq!(
            crate::topology::brep::p1::faces(&brep, None).unwrap(),
            faces
        );
    }

    #[test]
    fn pole_free_sphere_translates_geometry_with_its_vertices() {
        let mut k = Kernel::default();
        let center = Vec3::new(1.0, -2.0, 0.5);
        let source = create_sphere_no_pole(&mut k, Vec3::ZERO, 3.0).unwrap();
        let moved = create_sphere_no_pole(&mut k, center, 3.0).unwrap();
        for (a, b) in crate::topology::brep::p1::faces(&source, None)
            .unwrap()
            .into_iter()
            .zip(crate::topology::brep::p1::faces(&moved, None).unwrap())
        {
            let before = source.face(a).unwrap();
            let after = moved.face(b).unwrap();
            let uv = before.uv_domain.evaluate(0.37, 0.61);
            let before = before
                .surface
                .as_ref()
                .unwrap()
                .evaluate_point(&k, uv)
                .unwrap();
            let after = after
                .surface
                .as_ref()
                .unwrap()
                .evaluate_point(&k, uv)
                .unwrap();
            assert!(after.distance(before + center) < 1e-12);
        }
        for (a, b) in crate::topology::brep::p1::edges(&source, None)
            .unwrap()
            .into_iter()
            .zip(crate::topology::brep::p1::edges(&moved, None).unwrap())
        {
            let before = source.edge(a).unwrap();
            let after = moved.edge(b).unwrap();
            let t = before.interval.evaluate(0.37);
            let before = before
                .curve
                .as_ref()
                .unwrap()
                .evaluate_point(&k, t)
                .unwrap();
            let after = after.curve.as_ref().unwrap().evaluate_point(&k, t).unwrap();
            assert!(after.distance(before + center) < 1e-12);
        }
    }

    #[test]
    fn pyramid_ray_fire_preserves_lazy_trim_creation() {
        use crate::intersect::point_class::PointClassType;
        use crate::query::brep::{BrepIdentity, BrepQuery};
        use crate::topology::brep::{p1 as bp1, p2 as bp2};
        use crate::topology::shell::{ray_fire, ShellRayInput, ShellRayState};
        use crate::topology::BrepId;

        let mut k = Kernel::default();
        let mut b = create_pyramid(&mut k, o(), 5.0, 20.0).unwrap();
        let trims = |b: &Brep| {
            b.edgeuses()
                .iter()
                .filter(|(_, eu)| eu.uv_trim_curve.is_some())
                .count()
        };
        // The native constructor leaves two of the sixteen face trims uncreated.
        // Preparing every face before the solve would create all sixteen.
        assert_eq!(trims(&b), 14);
        assert!(crate::topology::brep::p2::is_manifold_solid(&b).unwrap());

        let keys = |object| bp2::caller_cache_key(&crate::topology::loopuse::notify_ref, object);
        let brep_id = BrepId::from_index(0);
        let world = BrepQuery::new(BrepIdentity {
            brep_id,
            keys: &keys,
            owner_of: &|_| None,
        });
        let shell = bp1::shells(&mut k, &b, None).unwrap()[0];
        // A miss and side hit leave the trims lazy; the apex needs the last two.
        for (start, direction, class, count) in [
            (
                Vec3::new(1000.0, 1000.0, 1000.0),
                Vec3::X,
                PointClassType::Unknown,
                14,
            ),
            (Vec3::new(0.0, 0.0, 5.0), Vec3::X, PointClassType::Face, 14),
            (
                Vec3::new(0.0, 0.0, 40.0),
                -Vec3::Z,
                PointClassType::Vertex,
                16,
            ),
        ] {
            let result = ray_fire(
                &mut k,
                &mut b,
                brep_id,
                shell,
                &world,
                ShellRayInput {
                    start,
                    direction,
                    zone_tolerance: 1e-5,
                    sphere_bound: None,
                    state: ShellRayState {
                        region: None,
                        classification: crate::intersect::point_class::PointClassification::new(
                            1e-5,
                        ),
                    },
                },
            );
            result.status.unwrap();
            assert_eq!(result.state.classification.point_class(), class);
            assert_eq!(
                result.state.region.is_some(),
                class != PointClassType::Unknown
            );
            assert_eq!(trims(&b), count);
        }
    }

    #[test]
    fn static_sheets_build_one_face() {
        let k = &mut Kernel::default();
        for r in [
            create_plane(k, o(), 2.0, 3.0),
            create_planar_circle(k, o(), 2.0),
        ] {
            assert_eq!(r.map(|b| b.face_list().len()).ok(), Some(1));
        }
    }

    #[test]
    fn surfaces_are_bounded_as_upstream() {
        let k = Kernel::default();
        let plane = create_plane_surface(o(), 10.0, 15.0).unwrap();
        let d = plane.step_uv_domain();
        assert_eq!((d.max().x, d.max().y), (10.0, 15.0));
        assert!(create_plane_surface(o(), 0.0, 0.0).is_err());

        let cyl = create_cylinder_surface(&k, o(), 5.0, 20.0).unwrap();
        assert!(cyl.is_cylinder_class());
        let d = cyl.rev().step_uv_domain();
        assert_eq!((d.max().x, d.max().y), (360.0, 20.0));

        let cone = create_cone_surface(&k, o(), 5.0, 10.0, 20.0).unwrap();
        assert!((cone.top_radius() - 10.0).abs() < 1e-12);

        let torus = create_torus_surface(&k, o(), 10.0, 2.0).unwrap();
        assert_eq!(torus.major_radius(), 10.0);
        assert!(create_torus_surface(&k, o(), 5.0, 0.0).is_err());
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        // Inputs the difftest fences off on the C++ side (upstream crashes there).
        let k = Kernel::default();
        let nan = f64::NAN;
        let inf = f64::INFINITY;
        let _ = create_plane_surface(o(), nan, 1.0);
        let _ = create_plane_surface(o(), 1.0, nan);
        let _ = create_torus_surface(&k, o(), inf, 1.0);
        let _ = create_torus_surface(&k, o(), 1.0, inf);
        let _ = create_torus_surface(&k, o(), 5.0, f64::MAX);
        let _ = create_torus_surface(&k, o(), f64::MAX, f64::MAX);
        let _ = create_torus_surface(&k, Point3::new(nan, 0.0, inf), 3.0, 1.0);
        let _ = create_cylinder_surface(&k, Point3::new(nan, inf, 0.0), 1.0, 1.0e17);
        let _ = create_cone_surface(&k, Point3::new(1.0e300, 0.0, 0.0), 1.0e17, 0.0, 1.0e-11);
        let _ = create_pyramid(&mut Kernel::default(), Point3::new(nan, 0.0, 0.0), nan, 1.0);
        let _ = create_pyramid(
            &mut Kernel::default(),
            Point3::new(1.0e17, 0.0, 0.0),
            1.0,
            1.0,
        );
        let _ = create_cylindrical_box(&mut Kernel::default(), o(), 1.0, 1.0, 2.0, 0.0, 360.0);
        let _ = create_sphere_no_pole(&mut Kernel::default(), o(), 1.0e18);
    }
}

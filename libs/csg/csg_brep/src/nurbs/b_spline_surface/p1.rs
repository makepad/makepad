//! `SmBSplineSurface`, part 1: the surface object (an NLib surface plus the STEP form
//! flags), its constructors and copies, the bilinear, canonical, cone patch and linear
//! sweep factories, control net, knot, degree, count, domain and Greville queries,
//! bounding boxes, parallelism test, affine transformation, STEP inversion and reading
//! from a database stream.
//!
//! Upstream: `source/SMLib/src/SmBSplineSurface.cpp`, `source/SMLib/inc/SmBSplineSurface.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`BSplineSurface`]                                | `SmBSplineSurface`                               |
//! | [`BSplineSurface::new`], `Default`                | `SmBSplineSurface()`                             |
//! | [`BSplineSurface::from_data`]                     | `SmBSplineSurface(n, m, p, q, r, s, Pw, U, V)`   |
//! | [`BSplineSurface::from_nurb`]                     | `SmBSplineSurface(gw_SURFACE*, SmBoolean, const SmContext*)` |
//! | [`BSplineSurface::from_gw_nurb`]                  | `SmBSplineSurface(const gw_SURFACE*)`            |
//! | [`BSplineSurface::copy`], `Clone`                 | copy constructor, `Copy`                         |
//! | `Drop` (implicit)                                 | `~SmBSplineSurface`                              |
//! | [`BSplineSurface::approx_eq`]                     | `operator==`                                     |
//! | [`BSplineSurface::calculate_bounding_box`], [`BSplineSurface::calculate_bounding_box_of`], [`BoundingBoxRequest`], [`BoundingBoxes`] | `CalculateBoundingBox` |
//! | [`BSplineSurface::create_bilinear_surface`]       | `CreateBilinearSurface`                          |
//! | [`BSplineSurface::create_canonical`], [`CanonicalSurfaceData`] | `CreateCanonical`                   |
//! | `create_cone_patch_dir` (private)                 | `sm_CreateConePatch` (static)                    |
//! | [`BSplineSurface::create_cone_patch`]             | `CreateConePatch`                                |
//! | [`BSplineSurface::create_linear_sweep`]           | `CreateLinearSweep`                              |
//! | [`BSplineSurface::create_offset_surface`]         | `CreateOffsetSurface`                            |
//! | [`BSplineSurface::canonical`], [`SurfaceCanonical`] | `GetCanonical`                                 |
//! | [`BSplineSurface::control_point_net`], [`ControlPointNet`] | `GetControlPointNet`                    |
//! | [`BSplineSurface::control_points`]                | `GetControlPointsPointer`                        |
//! | [`BSplineSurface::number_natural_knots`]          | `GetNumberNaturalKnots`                          |
//! | [`BSplineSurface::number_control_points`]         | `GetNumberControlPoints`                         |
//! | [`BSplineSurface::is_degenerate_curve`]           | `IsDegenerateCurve`                              |
//! | [`BSplineSurface::is_parallel_to_vector`]         | `IsParallelToVector`                             |
//! | [`BSplineSurface::is_rational`]                   | `IsRational`                                     |
//! | [`BSplineSurface::greville_abscissa`]             | `GetGrevilleAbscissa`                            |
//! | [`BSplineSurface::degree`]                        | `GetDegree`                                      |
//! | [`BSplineSurface::knots`]                         | `GetKnots`                                       |
//! | [`BSplineSurface::natural_uv_domain`]             | `GetNaturalUVDomain`                             |
//! | [`BSplineSurface::make_nurb`]                     | `MakeNurb`                                       |
//! | [`BSplineSurface::step_inversion`]                | `STEPInversion`                                  |
//! | [`BSplineSurface::transform`]                     | `Transform`                                      |
//! | [`BSplineSurface::is_kind_of`]                    | `IsKindOf`                                       |
//! | [`BSplineSurface::read_from_db`]                  | `ReadFromDB`                                     |
//! | [`BSplineSurface::write_to_db`]                   | `WriteToDB`                                      |
//! | [`BSplineSurface::TYPE`], [`BSplineSurface::type_code`], [`BSplineSurface::type_string`], [`BSplineSurface::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`BSplineSurface::out_of_bounds_enabled_mut`]     | `GetOutOfBoundsEnabled`                          |
//! | [`BSplineSurface::is_out_of_bounds_enabled`]      | `IsOutOfBoundsEnabled`                           |
//! | [`BSplineSurface::gw_nurb`]                       | `GetGwNurbPointer`                               |
//! | [`BSplineSurface::get_or_create_gw_nurb`]         | `GetOrCreateGwNurbPointer`                       |
//! | [`BSplineSurface::rebuild_step_from_nurb_parameters`] | `RebuildSTEPFromNURBParameters`              |
//! | [`BSplineSurface::set_b_spline_surface_form`]     | `SetBSplineSurfaceForm`                          |
//! | [`BSplineSurface::knot_type`], [`BSplineSurface::is_nurb_borrowed`] | `m_eKnotType`, `m_bNurbIsBorrowed` (member reads) |
//! | [`BSplineSurface::toggle_swap_uv_bit`]            | `ToggleSwapUVBit`                                |
//! | [`BSplineSurfaceForm`], [`SurfParamType`], [`LocationType`] | `SmBSplineSurfaceForm`, `SmSurfParamType`, `SmLocationType` |
//!
//! Deviations:
//! - **Surface base.** `SmBSplineSurface` derives from `SmSurface` (units
//!   `surface.surface_*`), which is not ported. A surface has no owner, attributes or
//!   caches here, so `Transform`'s `Notify(SM_NO_PRE_EDIT / SM_NO_POST_EDIT)` calls
//!   (which only drop caches and notify owners) are omitted. `SmSurface::operator==`
//!   (same type) is the Rust type of [`BSplineSurface::approx_eq`]'s argument.
//! - **Derived classes.** Upstream `MakeNurb`, `IsKindOf`, `ToggleSwapUVBit` and
//!   `RebuildSTEPFromNURBParameters` are virtual and overridden by the analytic
//!   surfaces (`SmPlane`, `SmCone`, `SmSphere`, ..., units `surface.*`), which build
//!   their NURB lazily. Those types are not ported; this type is the plain B-spline,
//!   whose `MakeNurb` cannot create a NURB.
//! - **Missing NURB.** Upstream methods that dereference the `NULL` returned by
//!   `GetOrCreateGwNurbPointer` for a surface without NURB data (built by
//!   `SmBSplineSurface()`) crash; here they return [`SolidError::Upstream`] `SM_ERR`
//!   (the status of the failed `MakeNurb`). The counts, degree, domain, rationality
//!   and parallelism queries return `0` / `false` / the empty domain there, as
//!   upstream; `GetCanonical` returns `SM_ERR_NULL_POINTER` and `GetKnots` `SM_ERR`.
//!   Copying a surface without NURB gives one without NURB, as upstream
//!   (`sm_AllocateAndCopyNurbSurface(NULL)` is `NULL`).
//! - **Ownership.** The NURB is owned ([`Surface`]); the "borrowed" flag of the fast
//!   constructor only skips the knot fix-up and is reported by
//!   [`BSplineSurface::is_nurb_borrowed`].
//! - **Context.** The `SmContext` arguments only choose where upstream allocates the
//!   new object and are dropped. `ReadFromDB`'s type argument (unused upstream) is
//!   dropped and it always builds a new surface (upstream's "fill the surface passed
//!   in" branch and its `IsKindOf` check have no counterpart).
//! - **Errors.** Out-params become return values. A failing NLib routine becomes
//!   `SM_ERR` ([`SolidError::Upstream`], code 1001), as upstream's
//!   `if(NL_YES == ...) SER(SM_ERR)`; `SM_ERR_INVALID_INPUT` checks are
//!   [`SolidError::InvalidInput`]. Where upstream would index outside its arrays or
//!   allocate from wrapped or negative sizes (fewer multiplicities than knots or no
//!   control points in `CreateCanonical`, too short arrays for the data constructor,
//!   knot type / form values outside the enums in `ReadFromDB`) the port returns
//!   `InvalidInput`. An unreadable header or stream, a degree or a size `ReadFromDB`
//!   rejects itself (`SM_ERR_INVALID_INPUT`) is `InvalidInput` as well.
//! - **Canonical knots.** `CreateCanonical` expands the knots by the caller's
//!   multiplicities with `SmTArray::Add` (failed growth ignored). The port counts the
//!   expanded knots first and materialises them only after the control point rows
//!   are taken, which bounds them by the point count; the checks and their order are
//!   upstream's, so an absurd multiplicity fails the same point-count check without
//!   allocating. A total count above `usize::MAX` is `InvalidInput`.
//! - **Returned pointers.** `GetControlPointsPointer` returns a pointer into the NURB;
//!   [`BSplineSurface::control_points`] returns a copy of the control points, row by
//!   row (u index slowest).
//! - **`IsParallelToVector`** returns its `SER(sTestVec.Unitize())` status
//!   (`SM_ERR_INVALID_INPUT`, nonzero) as the boolean for a zero vector, so upstream
//!   callers see `TRUE`; the port returns `Ok(true)` there. Its line distance test
//!   passes the second line point as the *direction* of the line
//!   (`smgu_LinePointDistance(LinePnt, LineEnd, Pnt, ..)` binds the `SmVector3d`
//!   overload); the port keeps that.
//! - **`CreateLinearSweep`** extrudes the curve's own NURB, which NLib makes 3D in
//!   place for a 2D curve (upstream casts away `const`); the port takes the curve
//!   mutably for that reason.
//! - **Back-edges.** `IsKindOf` answers for `SmBSplineSurface_TYPE` and passes other
//!   types to `SmSurface::IsKindOf` ([`crate::surface::surface::p2::is_kind_of`]);
//!   a plain B-spline is never one of the analytic types `CalculateBoundingBox` and
//!   `IsParallelToVector` test for first. `CalculateBoundingBox` also calls
//!   `SmSurface::EvaluateNormal` (`surface::surface::p7`). The analytic surfaces that
//!   inherit `CalculateBoundingBox` (`SmCone`, `SmSphere`, `SmTorus`, ...) run it as
//!   [`BSplineSurface::calculate_bounding_box_of`] with themselves as `this`: their
//!   `IsKindOf` makes the polar box the sampled "analytic" one and their virtual
//!   `EvaluateNormal` gives the samples. Its zone tolerance expansion
//!   reads `GetFace()`, and a bare B-spline has no owner (the owner is held by the
//!   [`Surface`](crate::surface::surface::Surface) wrapper), so it uses
//!   `SM_ZONE_TOL_3D` unless the wrapper passes its face's cached tolerance as
//!   [`BoundingBoxRequest::expand_tolerance`]; `CreateOffsetSurface` uses the base `SmSurface` body and
//!   constructs an `OffsetSurface`, asking the copy for its singularities.
//!   `STEPInversion` runs `GlobalPointSolve` and `IsClosed` on the owning surface
//!   (including plane overrides) and calls the base `LocalPointSolve` and
//!   `IsSingularity`. Errors and unported prerequisites of these calls propagate.
//!   `IsDegenerateCurve` runs the `SmSurface` base body on a copy of the B-spline
//!   wrapped as a `Surface` (without owner).

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::line_point_distance;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::polar_box::{PolarBox, PolarBoxState};
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::SolutionObject;
use crate::geom::tol::are_same;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQ};
use crate::nurbs::b_spline_curve::p1::ExpandedKnots;
use crate::nurbs::b_spline_curve::{BSplineCurve, KnotType, NurbCircleParam};
use crate::nurbs::crv_basic::ConicType;
use crate::nurbs::iges::{iges_128_surface, Entity128};
use crate::nurbs::knots::MAX_DEGREE;
use crate::nurbs::math::{CPoint, MatrixType, RealMatrix, NOW};
use crate::nurbs::nurbs_crv::{extract_frame, fixup_knot_vector, get_knots, to_euclid};
use crate::nurbs::nurbs_srf::{allocate_nurb_surface, copy_nurb_surface};
use crate::nurbs::srf_basic::{Direction, Surface};
use crate::nurbs::srf_common::{corner_pts, cyl_cone, extrude_crv, CylCone};
use crate::surface::surface::p1::SurfaceSolveContext;
use crate::topology::database_io::{DatabaseReader, DatabaseWriter, FileType};

/// `SURF_BASE_TYPE` (`SmTypes.h`).
const SURF_BASE_TYPE: i64 = 15000;
/// `SmPlane_TYPE`.
const PLANE_TYPE: i64 = SURF_BASE_TYPE + 11;
/// `SmCone_TYPE`.
const CONE_TYPE: i64 = SURF_BASE_TYPE + 12;
/// `SmSphere_TYPE`.
const SPHERE_TYPE: i64 = SURF_BASE_TYPE + 14;
/// `SmTorus_TYPE`.
const TORUS_TYPE: i64 = SURF_BASE_TYPE + 15;

const OP_DATA: &str = "nurbs::b_spline_surface::p1::BSplineSurface::from_data";
const OP_NURB: &str = "nurbs::b_spline_surface::p1::BSplineSurface::make_nurb";
const OP_BBOX: &str = "nurbs::b_spline_surface::p1::BSplineSurface::calculate_bounding_box";
const OP_BILINEAR: &str = "nurbs::b_spline_surface::p1::BSplineSurface::create_bilinear_surface";
const OP_CANON: &str = "nurbs::b_spline_surface::p1::BSplineSurface::create_canonical";
const OP_CONE: &str = "nurbs::b_spline_surface::p1::BSplineSurface::create_cone_patch";
const OP_SWEEP: &str = "nurbs::b_spline_surface::p1::BSplineSurface::create_linear_sweep";
const OP_GET_CANON: &str = "nurbs::b_spline_surface::p1::BSplineSurface::canonical";
const OP_NET: &str = "nurbs::b_spline_surface::p1::BSplineSurface::control_point_net";
const OP_GREVILLE: &str = "nurbs::b_spline_surface::p1::BSplineSurface::greville_abscissa";
const OP_KNOTS: &str = "nurbs::b_spline_surface::p1::BSplineSurface::knots";
const OP_STEP_INV: &str = "nurbs::b_spline_surface::p1::BSplineSurface::step_inversion";
const OP_XFORM: &str = "nurbs::b_spline_surface::p1::BSplineSurface::transform";
const OP_READ: &str = "nurbs::b_spline_surface::p1::BSplineSurface::read_from_db";
/// `ReadFromDB`'s message when the stream fails part way (`ReadFailed`).
const READ_FAILED: &str = "NURBS surface data ends early or holds text that is not a number";
const OP_WRITE: &str = "nurbs::b_spline_surface::p1::BSplineSurface::write_to_db";

/// `SmBSplineSurfaceForm`: the STEP form of a B-spline surface; discriminants are the
/// upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BSplineSurfaceForm {
    /// `SM_SF_PLANE_SURF`.
    PlaneSurf = 0,
    /// `SM_SF_CYLINDRICAL_SURF`.
    CylindricalSurf = 1,
    /// `SM_SF_CONICAL_SURF`.
    ConicalSurf = 2,
    /// `SM_SF_SPHERICAL_SURF`.
    SphericalSurf = 3,
    /// `SM_SF_SURF_OF_REVOLUTION`.
    SurfOfRevolution = 4,
    /// `SM_SF_RULED_SURF`.
    RuledSurf = 5,
    /// `SM_SF_GENERALIZED_CONE`.
    GeneralizedCone = 6,
    /// `SM_SF_QUADRIC_SURF`.
    QuadricSurf = 7,
    /// `SM_SF_SURF_OF_LINEAR_EXTRUSION`.
    SurfOfLinearExtrusion = 8,
    /// `SM_SF_UNSPECIFIED`.
    #[default]
    Unspecified = 9,
    /// `SM_SF_POLYNOMIAL`.
    Polynomial = 10,
    /// `SM_SF_HELICAL_SWEEP`.
    HelicalSweep = 11,
}

impl BSplineSurfaceForm {
    /// The form with upstream value `code`, if any.
    #[must_use]
    pub fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            0 => Self::PlaneSurf,
            1 => Self::CylindricalSurf,
            2 => Self::ConicalSurf,
            3 => Self::SphericalSurf,
            4 => Self::SurfOfRevolution,
            5 => Self::RuledSurf,
            6 => Self::GeneralizedCone,
            7 => Self::QuadricSurf,
            8 => Self::SurfOfLinearExtrusion,
            9 => Self::Unspecified,
            10 => Self::Polynomial,
            11 => Self::HelicalSweep,
            _ => return None,
        })
    }
}

/// `SmSurfParamType`: a surface parameter direction (or side); discriminants are the
/// upstream values. The queries here treat [`SurfParamType::U`] as u and every other
/// value as v (or, for [`BSplineSurface::number_control_points`], as "both").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfParamType {
    /// `SM_SP_U`: the u direction.
    U = 0,
    /// `SM_SP_V`: the v direction.
    V = 1,
    /// `SM_SP_UNKNOWN`.
    Unknown = 2,
    /// `SM_SP_BOTH`.
    Both = 3,
    /// `SM_SP_NEITHER`.
    Neither = 4,
    /// `SM_SP_UMIN`.
    UMin = 5,
    /// `SM_SP_VMIN`.
    VMin = 6,
    /// `SM_SP_UMAX`.
    UMax = 7,
    /// `SM_SP_VMAX`.
    VMax = 8,
    /// `SM_SP_OUT`.
    Out = 9,
}

impl SurfParamType {
    /// The enumerator of a stored value (`(SmSurfParamType) lValue`), `None` outside
    /// the enum.
    #[must_use]
    pub const fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            0 => Self::U,
            1 => Self::V,
            2 => Self::Unknown,
            3 => Self::Both,
            4 => Self::Neither,
            5 => Self::UMin,
            6 => Self::VMin,
            7 => Self::UMax,
            8 => Self::VMax,
            9 => Self::Out,
            _ => return None,
        })
    }
}

/// `SmLocationType`: where a point lies on a surface; discriminants are the upstream
/// values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocationType {
    /// `SM_LT_POLE`: at a pole.
    Pole = 0,
    /// `SM_LT_U_SEAM`: on the seam of a surface closed in u.
    USeam = 1,
    /// `SM_LT_V_SEAM`: on the seam of a surface closed in v.
    VSeam = 2,
    /// `SM_LT_UV_SEAM`: on both seams.
    UvSeam = 3,
    /// `SM_LT_INTERIOR`: neither at a pole nor on a seam.
    Interior = 4,
    /// `SM_LT_EXTERIOR`: not on the surface or outside the domain.
    Exterior = 5,
}

/// The STEP-style data of [`BSplineSurface::create_canonical`].
#[derive(Debug, Clone, Copy)]
pub struct CanonicalSurfaceData<'a> {
    /// Degree in u, 1 to 32.
    pub u_degree: usize,
    /// Degree in v, 1 to 32.
    pub v_degree: usize,
    /// Euclidean control points, row by row (`P[i][j]` at `i * v_count + j`).
    pub control_points: &'a [Point3],
    /// STEP form of the new surface.
    pub form: BSplineSurfaceForm,
    /// Multiplicity of each unique u knot.
    pub u_multiplicities: &'a [usize],
    /// Multiplicity of each unique v knot.
    pub v_multiplicities: &'a [usize],
    /// Unique u knots in ascending order.
    pub u_knots: &'a [f64],
    /// Unique v knots in ascending order.
    pub v_knots: &'a [f64],
    /// STEP knot type of the new surface.
    pub knot_type: KnotType,
    /// Optional weight of each control point (same order as the points).
    pub weights: Option<&'a [f64]>,
    /// Optional domain whose bounds replace the parameter bounds taken from the knots.
    pub uv_domain: Option<Extent2d>,
}

/// The STEP-style data of a surface ([`BSplineSurface::canonical`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceCanonical {
    /// Degree in u.
    pub u_degree: usize,
    /// Degree in v.
    pub v_degree: usize,
    /// Euclidean control points, row by row (u index slowest).
    pub control_points: Vec<Point3>,
    /// STEP form.
    pub form: BSplineSurfaceForm,
    /// Multiplicity of each unique u knot.
    pub u_multiplicities: Vec<usize>,
    /// Multiplicity of each unique v knot.
    pub v_multiplicities: Vec<usize>,
    /// Unique u knots.
    pub u_knots: Vec<f64>,
    /// Unique v knots.
    pub v_knots: Vec<f64>,
    /// STEP knot type.
    pub knot_type: KnotType,
    /// Weight of each control point; empty for a non-rational surface.
    pub weights: Vec<f64>,
}

/// The control net of a surface in Euclidean form
/// ([`BSplineSurface::control_point_net`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ControlPointNet {
    /// Control points in u.
    pub u_count: usize,
    /// Control points in v.
    pub v_count: usize,
    /// Euclidean control points; `Pw[i][j]` is at `i * v_count + j`.
    pub points: Vec<Point3>,
    /// Weights, same order; empty for a non-rational surface.
    pub weights: Vec<f64>,
}

/// Which boxes [`BSplineSurface::calculate_bounding_box`] computes, with its optional
/// basis guesses.
#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)] // upstream's optional out-pointers and flag
pub struct BoundingBoxRequest<'a> {
    /// Compute the axis aligned box (`pNormalBox`).
    pub normal: bool,
    /// Compute the non-axis aligned box (`pPseudoBox`).
    pub pseudo: bool,
    /// Compute the normal vector box (`pPolarBox`).
    pub polar: bool,
    /// Basis guess for the pseudo box, used when it gives a smaller box
    /// (`pOptPseudoBasisGuess`).
    pub pseudo_guess: Option<&'a PseudoBox>,
    /// Basis guess for the polar box, used when it gives a smaller box
    /// (`pOptPolarBasisGuess`; ignored when uninitialised).
    pub polar_guess: Option<&'a PolarBox>,
    /// Expand the normal and pseudo boxes by the zone tolerance
    /// (`bExpandPosBoxesByZoneTol3d`, upstream default `TRUE`).
    pub expand_by_zone_tol: bool,
    /// The zone tolerance of the owning face to expand by (`GetFace()->GetTolerance()`):
    /// a bare B-spline has no owner, so the [`Surface`](crate::surface::surface::Surface)
    /// wrapper holding it passes its face's cached tolerance here; `None` expands by
    /// the ownerless `SM_ZONE_TOL_3D`.
    pub expand_tolerance: Option<f64>,
}

/// The boxes of [`BSplineSurface::calculate_bounding_box`] (each present when asked).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBoxes {
    /// Axis aligned box.
    pub normal: Option<Extent3d>,
    /// Non-axis aligned box.
    pub pseudo: Option<PseudoBox>,
    /// Normal vector box.
    pub polar: Option<PolarBox>,
}

/// A B-spline (NURBS) surface (`SmBSplineSurface`): an NLib surface with its STEP form
/// flags.
#[derive(Debug)]
pub struct BSplineSurface {
    /// The NURB (`m_pNurb`); empty is `NULL`. A lazy analytic kind fills it once from a
    /// read-only query ([`NurbSlot::fill`]), as upstream's `GetOrCreateGwNurbPointer`
    /// does through a `const_cast`.
    pub(crate) nurb: NurbSlot,
    /// `m_bNurbIsBorrowed`.
    pub(crate) nurb_is_borrowed: bool,
    /// `m_bOutOfBoundsEnabled`: evaluate beyond the natural domain. Interior-mutable
    /// (an `AtomicBool`, relaxed: a plain `Cell` would make every arena holding a
    /// surface non-`Sync`), so a solve can enable it for its scope on a shared surface,
    /// as upstream's `SmTemporaryChangeValue` does on a `const` one (no copy of the
    /// surface).
    pub(crate) out_of_bounds_enabled: AtomicBool,
    /// `m_eKnotType`.
    pub(crate) knot_type: KnotType,
    /// `m_eBSplineSurfaceForm`.
    pub(crate) form: BSplineSurfaceForm,
}

/// The storage of a B-spline surface's NURB (`m_pNurb`): set and edited through
/// `&mut` like an `Option`, and also fillable once through `&` ([`NurbSlot::fill`]).
///
/// Upstream's `GetOrCreateGwNurbPointer` runs the virtual `MakeNurb` when `m_pNurb` is
/// null and keeps the NURB on the object, also when called from a `const` method
/// (through a `const_cast`), so every later reader of that surface (`IntersectWithPlane`'s
/// "has a NURB" test, `operator==`, the copy constructor, `ToggleSwapUVBit`, transforms)
/// sees it. A `OnceLock` gives the port the same once-only fill from `&self` while
/// keeping the surface `Send + Sync`; the made value depends only on the surface's own
/// data, so the result is deterministic.
///
/// The cell sits behind an `Arc` so that an evaluation copy of a surface (which stands
/// for upstream's pointer to the one object, as the surface's cache slot does) can share
/// it ([`NurbSlot::shared`]): a NURB made through the copy is then kept on the surface
/// too. Every `&mut` access detaches a shared cell first (copy on write), so an edit of
/// one never reaches the other.
#[derive(Debug, Default)]
pub(crate) struct NurbSlot(Arc<OnceLock<Surface>>);

impl NurbSlot {
    /// No NURB (`NULL`).
    pub(crate) fn none() -> Self {
        Self(Arc::new(OnceLock::new()))
    }

    /// The same cell (for an evaluation copy that stands for the same surface object).
    pub(crate) fn shared(&self) -> Self {
        Self(Arc::clone(&self.0))
    }

    /// The NURB, if any.
    pub(crate) fn as_ref(&self) -> Option<&Surface> {
        self.0.get()
    }

    /// The NURB, mutable, if any (detaches a shared cell first).
    pub(crate) fn as_mut(&mut self) -> Option<&mut Surface> {
        Arc::make_mut(&mut self.0).get_mut()
    }

    /// Whether there is no NURB.
    pub(crate) fn is_none(&self) -> bool {
        self.0.get().is_none()
    }

    /// Whether there is a NURB.
    pub(crate) fn is_some(&self) -> bool {
        self.0.get().is_some()
    }

    /// Takes the NURB out, leaving none (detaches a shared cell first).
    pub(crate) fn take(&mut self) -> Option<Surface> {
        Arc::make_mut(&mut self.0).take()
    }

    /// Detaches a shared cell, so that what is made through this surface from now on
    /// is its own (for an edit of the surface's other data).
    pub(crate) fn detach(&mut self) {
        Arc::make_mut(&mut self.0);
    }

    /// Stores `made` when there is no NURB yet (the first fill wins, as upstream's
    /// `GetOrCreateGwNurbPointer` makes once) and returns the stored NURB.
    pub(crate) fn fill(&self, made: Surface) -> &Surface {
        self.0.get_or_init(|| made)
    }
}

impl From<Option<Surface>> for NurbSlot {
    fn from(nurb: Option<Surface>) -> Self {
        Self(Arc::new(nurb.map_or_else(OnceLock::new, OnceLock::from)))
    }
}

impl Clone for BSplineSurface {
    /// The copy constructor (see [`BSplineSurface::copy`]).
    fn clone(&self) -> Self {
        self.copy()
    }
}

impl Default for BSplineSurface {
    /// A surface without NURB data (`SmBSplineSurface()`).
    fn default() -> Self {
        Self::new()
    }
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// `InvalidInput` unless `ok` (upstream's `*_ER` input checks, which fail for NaN).
fn check(ok: bool, operation: &'static str, reason: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(operation, reason))
    }
}

/// `TO_EUCLID` of control point `[i][j]` of `sur`.
fn euclid_at(sur: &Surface, i: usize, j: usize) -> Result<Point3> {
    Ok(to_euclid(sur.cpt(i, j)?))
}

/// The control net in Euclidean form with the weights of rational points, as
/// `GetCanonical` / `GetControlPointNet` build it (division by `w`, `z` divided even
/// when missing).
fn euclidean_net(sur: &Surface) -> (Vec<Point3>, Vec<f64>) {
    let count = (sur.n() + 1) * (sur.m() + 1);
    let mut points = Vec::with_capacity(count);
    let mut weights = vec![0.0; count];
    let mut is_rational = false;
    let mut l_count = 0usize;
    for row in sur.rows() {
        for pw in row {
            let mut pnt = Point3::new(pw.x, pw.y, pw.z);
            if pw.w != NOW {
                is_rational = true;
                if let Some(w) = weights.get_mut(l_count) {
                    *w = pw.w;
                }
                pnt.x /= pw.w;
                pnt.y /= pw.w;
                pnt.z /= pw.w;
            }
            points.push(pnt);
            l_count += 1;
        }
    }
    if !is_rational {
        weights.clear();
    }
    (points, weights)
}

/// The NLib direction flag of a surface parameter (`SM_SP_U` is `NL_UDIR`, anything
/// else `NL_VDIR`).
fn nl_dir(param: SurfParamType) -> Direction {
    if param == SurfParamType::U {
        Direction::U
    } else {
        Direction::V
    }
}

// ---- unported SmSurface methods ------------------------------------------------------

/// `SmSurface::EvaluateNormal(uv, TRUE, TRUE, normal)`.
fn evaluate_normal(k: &Kernel, srf: &BSplineSurface, uv: Point2) -> Result<Vec3> {
    crate::surface::surface::p7::base_evaluate_normal(
        k,
        srf,
        uv,
        crate::nurbs::knots::Side::Left,
        crate::nurbs::knots::Side::Left,
    )
}

/// The zone tolerance of the owning face or brep, `SM_ZONE_TOL_3D` without owner
/// (`GetFace() ? GetFace()->GetTolerance() : GetBrep() ? ... : SM_ZONE_TOL_3D`). A
/// bare B-spline has no owner (the owner is held by the
/// [`Surface`](crate::surface::surface::Surface) wrapper, whose
/// [`calculate_bounding_box`](crate::surface::surface::Surface::calculate_bounding_box)
/// passes a face owner's tolerance as [`BoundingBoxRequest::expand_tolerance`]):
/// that tolerance when given, else `SM_ZONE_TOL_3D`. Callers that hold a `Surface` with
/// a face must pass it (as `Surface::calculate_bounding_box` and `Surface::is_planar`
/// do); the patch boxes of surface caches are ownerless.
fn owner_zone_tolerance(req: &BoundingBoxRequest<'_>) -> f64 {
    req.expand_tolerance
        .unwrap_or(crate::geom::tol::ZONE_TOL_3D)
}

/// `SmSurface::IsDegenerateCurve` (the base body, on an exact copy of the B-spline as
/// an [`SmSurface`](crate::surface::surface::Surface): upstream passes the call along
/// on the object itself, and [`BSplineSurface::copy`] would fix the knots up).
fn surface_is_degenerate_curve(
    k: &Kernel,
    srf: &BSplineSurface,
    angle_tol_deg: f64,
    uv_domain: Option<&Extent2d>,
    tol_3d: f64,
) -> Result<bool> {
    let s = crate::surface::surface::Surface::from(srf.clone_exact());
    crate::surface::surface::p1::base_is_degenerate_curve(k, &s, angle_tol_deg, uv_domain, tol_3d)
}

/// `SmSurface::LocalPointSolve(domain, SM_SO_INTERSECT, point, guess, ..)`: whether it
/// succeeded and the solution's start parameters.
fn local_point_solve(
    k: &Kernel,
    surface: &crate::surface::surface::Surface,
    domain: &Extent2d,
    point: Point3,
    guess: Point2,
) -> Result<(bool, Point2)> {
    // Retain the original virtual evaluator (in particular SmPlane's override).
    let r = surface.local_point_solve::<crate::surface::surface::p4::NoObject>(
        k,
        domain,
        SolverOperation::Intersect,
        point,
        guess,
    )?;
    Ok((
        r.found,
        Point2::new(r.solution.start.param(0)?, r.solution.start.param(1)?),
    ))
}

/// `SmSurface::GlobalPointSolve(domain, SM_SO_INTERSECT, point, tol, NULL,
/// SM_SR_ALL, ..)` on `surface` (the surface the B-spline part belongs to): the start
/// parameters of every solution.
/// [`global_point_solve`] under the caller's face context (the context and the
/// surface's handle in it).
fn global_point_solve_with_context<O: SolutionObject>(
    k: &Kernel,
    surface: &crate::surface::surface::Surface,
    domain: &Extent2d,
    point: Point3,
    tol_3d: f64,
    context: (&mut dyn SurfaceSolveContext<O>, Option<O>),
) -> Result<Vec<Point2>> {
    let (context, handle) = context;
    let mut solutions = crate::geom::solution_array::SolutionArray::<O>::new();
    surface.global_point_solve_into_with_context(
        k,
        handle,
        None,
        context,
        domain,
        SolverOperation::Intersect,
        point,
        tol_3d,
        None,
        SolutionRequested::All,
        &mut solutions,
    )?;
    Ok(solutions
        .as_slice()
        .iter()
        .map(|s| {
            Point2::new(
                s.start.parameters.first().copied().unwrap_or(0.0),
                s.start.parameters.get(1).copied().unwrap_or(0.0),
            )
        })
        .collect())
}

fn global_point_solve(
    k: &Kernel,
    surface: &crate::surface::surface::Surface,
    domain: &Extent2d,
    point: Point3,
    tol_3d: f64,
) -> Result<Vec<Point2>> {
    let starts = crate::surface::surface::p4::global_point_solve_starts(
        k,
        surface,
        domain,
        SolverOperation::Intersect,
        point,
        tol_3d,
        SolutionRequested::All,
    )?;
    Ok(starts
        .iter()
        .map(|s| {
            Point2::new(
                s.parameters.first().copied().unwrap_or(0.0),
                s.parameters.get(1).copied().unwrap_or(0.0),
            )
        })
        .collect())
}

/// `SmSurface::IsSingularity(uv, dir)`, preserving virtual plane dispatch.
fn is_singularity(
    k: &Kernel,
    surface: &crate::surface::surface::Surface,
    _srf: &BSplineSurface,
    uv: Point2,
) -> Result<bool> {
    Ok(surface.is_singularity(k, uv, EFF_ZERO, false)?.singular)
}

/// `SmSurface::IsClosed(domain, dir)` (unit `surface.surface_p7`), virtual:
/// `SmPlane::IsClosed` is an inline override returning FALSE (`SmPlane.h:432`).
fn is_closed(
    k: &Kernel,
    surface: &crate::surface::surface::Surface,
    srf: &BSplineSurface,
    domain: &Extent2d,
    dir: SurfParamType,
) -> Result<bool> {
    if let crate::surface::surface::SurfaceKind::Plane(p) = surface.kind() {
        return Ok(p.is_closed().0);
    }
    Ok(crate::surface::surface::p7::base_is_closed(k, srf, domain, dir, None, false)?.0)
}

impl BSplineSurface {
    /// `SmBSplineSurface_TYPE` (`SURF_BASE_TYPE + 2`).
    pub const TYPE: i64 = SURF_BASE_TYPE + 2;

    // ---- construction -----------------------------------------------------------------

    /// A surface without NURB data (`SmBSplineSurface()`); only set methods can give it
    /// one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nurb: NurbSlot::none(),
            nurb_is_borrowed: false,
            out_of_bounds_enabled: AtomicBool::new(false),
            knot_type: KnotType::Unspecified,
            form: BSplineSurfaceForm::Unspecified,
        }
    }

    /// The surface over a copy of the control points `pw[0..=n][0..=m]` and knots
    /// `u[0..=r]`, `v[0..=s]` of degrees `p`, `q` (`SmBSplineSurface(n, m, p, q, r, s,
    /// Pw, U, V)`); the knots are fixed up. The sizes are not checked against each
    /// other, as upstream.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `pw` holds fewer than `n + 1` rows of `m + 1` points or `u`
    /// / `v` fewer than `r + 1` / `s + 1` knots (upstream reads past them).
    pub fn from_data(
        n: usize,
        m: usize,
        p: usize,
        q: usize,
        r: usize,
        s: usize,
        pw: &[Vec<CPoint>],
        u: &[f64],
        v: &[f64],
    ) -> Result<Self> {
        // allocate single block Surface with internal pointers and size params set
        let mut new_sur = allocate_nurb_surface(n, m, p, q, r, s)?;

        // copy knot values from target surface to new surface.
        let src_u = u
            .get(..=r)
            .ok_or_else(|| invalid(OP_DATA, "fewer than r + 1 u knots"))?;
        new_sur.u_knots_mut().copy_from_slice(src_u);
        let src_v = v
            .get(..=s)
            .ok_or_else(|| invalid(OP_DATA, "fewer than s + 1 v knots"))?;
        new_sur.v_knots_mut().copy_from_slice(src_v);

        // copy control point values from target surface to new surface one row at a time
        for i in 0..=n {
            let src = pw
                .get(i)
                .and_then(|row| row.get(..=m))
                .ok_or_else(|| invalid(OP_DATA, "control net smaller than (n, m)"))?;
            for (j, &cp) in src.iter().enumerate() {
                *new_sur.cpt_mut(i, j)? = cp;
            }
        }

        // Edit knot values: close knots become same and near knots are separated.
        fixup_knot_vector(new_sur.u_knots_mut(), EFF_ZERO_PARAM);
        fixup_knot_vector(new_sur.v_knots_mut(), EFF_ZERO_PARAM);
        Ok(Self {
            nurb: Some(new_sur).into(),
            ..Self::new()
        })
    }

    /// The surface over an existing NURB (`SmBSplineSurface(gw_SURFACE*,
    /// bNurbIsBorrowed, pContext)`). An owned (not borrowed) NURB gets its knots fixed
    /// up (`sm_FixupKnotVector` with `SM_EFF_ZERO_PARAM`).
    #[must_use]
    pub fn from_nurb(mut nurb: Surface, borrowed: bool) -> Self {
        // when owned call sm_FixupKnotVector()
        if !borrowed {
            fixup_knot_vector(nurb.u_knots_mut(), EFF_ZERO_PARAM);
            fixup_knot_vector(nurb.v_knots_mut(), EFF_ZERO_PARAM);
        }
        Self {
            nurb: Some(nurb).into(),
            nurb_is_borrowed: borrowed,
            ..Self::new()
        }
    }

    /// The surface over a compact copy of `gw_nurb`, knots fixed up
    /// (`SmBSplineSurface(const gw_SURFACE*)`).
    #[must_use]
    pub fn from_gw_nurb(gw_nurb: &Surface) -> Self {
        let mut nurb = copy_nurb_surface(gw_nurb);
        fixup_knot_vector(nurb.u_knots_mut(), EFF_ZERO_PARAM);
        fixup_knot_vector(nurb.v_knots_mut(), EFF_ZERO_PARAM);
        Self {
            nurb: Some(nurb).into(),
            ..Self::new()
        }
    }

    /// A copy (copy constructor, `Copy`): out-of-bounds flag, knot type and form are
    /// copied; the NURB (if any) is copied compactly and its knots are fixed up; the
    /// copy owns its NURB.
    #[must_use]
    pub fn copy(&self) -> Self {
        let nurb = self.nurb.as_ref().map(|src| {
            let mut nurb = copy_nurb_surface(src);
            fixup_knot_vector(nurb.u_knots_mut(), EFF_ZERO_PARAM);
            fixup_knot_vector(nurb.v_knots_mut(), EFF_ZERO_PARAM);
            nurb
        });
        Self {
            nurb: nurb.into(),
            nurb_is_borrowed: false,
            out_of_bounds_enabled: AtomicBool::new(
                self.out_of_bounds_enabled.load(Ordering::Relaxed),
            ),
            knot_type: self.knot_type,
            form: self.form,
        }
    }

    /// A copy that keeps the NURB exactly as it is: [`BSplineSurface::copy`] fixes the
    /// knot vectors up (`fixup_knot_vector`), which can move near-coincident knots.
    /// Wrappers that stand in for upstream's solve "on the original object" (a
    /// `Surface` holding the surface) use this one.
    #[must_use]
    pub fn clone_exact(&self) -> Self {
        self.clone_exact_with(NurbSlot::from(self.nurb.as_ref().map(copy_nurb_surface)))
    }

    /// [`BSplineSurface::clone_exact`] for an evaluation copy that stands for this
    /// surface object: the copy shares this surface's NURB cell ([`NurbSlot::shared`]),
    /// so a NURB made through it is kept here too.
    #[must_use]
    pub(crate) fn clone_exact_shared(&self) -> Self {
        self.clone_exact_with(self.nurb.shared())
    }

    /// Detaches a NURB cell shared with evaluation copies (before an edit of this
    /// surface or of the analytic data it is made from).
    pub(crate) fn detach_nurb(&mut self) {
        self.nurb.detach();
    }

    fn clone_exact_with(&self, nurb: NurbSlot) -> Self {
        Self {
            nurb,
            nurb_is_borrowed: false,
            out_of_bounds_enabled: AtomicBool::new(
                self.out_of_bounds_enabled.load(Ordering::Relaxed),
            ),
            knot_type: self.knot_type,
            form: self.form,
        }
    }

    /// Upstream `operator==`: the same object, or both without NURB, or NURBs equal
    /// within `SM_EFF_ZERO` (`N_SrfsAreEqual`).
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }
        match (self.nurb.as_ref(), other.nurb.as_ref()) {
            (None, None) => true,
            (Some(a), Some(b)) => a.equals(b, EFF_ZERO, EFF_ZERO),
            _ => false,
        }
    }

    // ---- accessors --------------------------------------------------------------------

    /// The NURB, if any (`GetGwNurbPointer`).
    #[must_use]
    pub fn gw_nurb(&self) -> Option<&Surface> {
        self.nurb.as_ref()
    }

    /// The NURB, made first if missing (`GetOrCreateGwNurbPointer`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when there is none: a plain B-spline cannot make one (upstream returns
    /// `NULL`).
    pub fn get_or_create_gw_nurb(&mut self) -> Result<&mut Surface> {
        if self.nurb.is_none() {
            self.make_nurb()?;
        }
        self.nurb
            .as_mut()
            .ok_or_else(|| SolidError::sm_err(OP_NURB))
    }

    /// Makes the NURB representation (`MakeNurb`): a plain B-spline only checks that it
    /// has one.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn make_nurb(&mut self) -> Result<()> {
        if self.nurb.is_none() {
            return Err(SolidError::sm_err(OP_NURB));
        }
        Ok(())
    }

    /// The NURB for read-only use, made first if missing (`GetOrCreateGwNurbPointer`
    /// from a `const` method). A plain B-spline cannot make one, so this is
    /// [`BSplineSurface::gw_nurb`].
    fn nurb_ref(&self) -> Option<&Surface> {
        self.nurb.as_ref()
    }

    /// [`BSplineSurface::nurb_ref`] for methods that dereference it upstream.
    fn nurb_or_err(&self, operation: &'static str) -> Result<&Surface> {
        self.nurb_ref().ok_or_else(|| SolidError::sm_err(operation))
    }

    /// Mutable access to the out-of-bounds evaluation flag (`GetOutOfBoundsEnabled`).
    pub fn out_of_bounds_enabled_mut(&mut self) -> &mut bool {
        self.out_of_bounds_enabled.get_mut()
    }

    /// Whether evaluation beyond the natural domain is enabled (`IsOutOfBoundsEnabled`).
    #[must_use]
    pub fn is_out_of_bounds_enabled(&self) -> bool {
        self.out_of_bounds_enabled.load(Ordering::Relaxed)
    }

    /// Sets the out-of-bounds evaluation flag through a shared reference: for a
    /// temporary change that the caller restores (`SmTemporaryChangeValue` on
    /// `GetOutOfBoundsEnabled()` of a `const` surface).
    pub(crate) fn set_out_of_bounds_enabled_in_place(&self, enabled: bool) {
        self.out_of_bounds_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether the NURB is borrowed (`IsNurbBorrowed`).
    #[must_use]
    pub fn is_nurb_borrowed(&self) -> bool {
        self.nurb_is_borrowed
    }

    /// The STEP knot type (`m_eKnotType`).
    #[must_use]
    pub fn knot_type(&self) -> KnotType {
        self.knot_type
    }

    /// The STEP form (`GetBSplineSurfaceForm`).
    #[must_use]
    pub fn form(&self) -> BSplineSurfaceForm {
        self.form
    }

    /// Sets the STEP form (`SetBSplineSurfaceForm`).
    pub fn set_b_spline_surface_form(&mut self, form: BSplineSurfaceForm) {
        self.form = form;
    }

    /// Rebuilds the STEP data from the NURB (`RebuildSTEPFromNURBParameters`): nothing
    /// to do for a plain B-spline.
    ///
    /// # Errors
    ///
    /// None for a plain B-spline.
    pub fn rebuild_step_from_nurb_parameters(&mut self) -> Result<()> {
        Ok(())
    }

    /// Records a u/v swap (`ToggleSwapUVBit`): nothing to record for a plain B-spline.
    pub fn toggle_swap_uv_bit(&mut self) {}

    /// `SmBSplineSurface_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmBSplineSurface_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmBSplineSurface_TYPE"
    }

    /// `"SmBSplineSurface"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmBSplineSurface"
    }

    /// True for `SmBSplineSurface_TYPE` and its base types (`IsKindOf`).
    ///
    /// # Errors
    ///
    /// None (the `Result` is kept for the derived analytic surfaces).
    pub fn is_kind_of(&self, t: i64) -> Result<bool> {
        if t == Self::TYPE {
            return Ok(true);
        }
        Ok(crate::surface::surface::p2::is_kind_of(t))
    }

    // ---- simple queries ---------------------------------------------------------------

    /// The degree in `param`'s direction (`GetDegree`; `0` without NURB).
    #[must_use]
    pub fn degree(&self, param: SurfParamType) -> usize {
        let Some(sur) = self.nurb_ref() else {
            return 0; // FS
        };
        if param == SurfParamType::U {
            sur.p()
        } else {
            sur.q()
        }
    }

    /// The number of knots (with repeats) in `param`'s direction
    /// (`GetNumberNaturalKnots`; `0` without NURB).
    #[must_use]
    pub fn number_natural_knots(&self, param: SurfParamType) -> usize {
        let Some(sur) = self.nurb_ref() else {
            return 0; // FS
        };
        if param == SurfParamType::U {
            sur.r() + 1
        } else {
            sur.s() + 1
        }
    }

    /// The number of control points in u ([`SurfParamType::U`]), in v
    /// ([`SurfParamType::V`]) or in total (any other value) (`GetNumberControlPoints`;
    /// `0` without NURB).
    #[must_use]
    pub fn number_control_points(&self, param: SurfParamType) -> usize {
        let Some(sur) = self.nurb_ref() else {
            return 0; // FS
        };
        match param {
            SurfParamType::U => sur.n() + 1,
            SurfParamType::V => sur.m() + 1,
            _ => (sur.n() + 1) * (sur.m() + 1),
        }
    }

    /// Whether the surface is rational: control point `[0][0]` has a weight
    /// (`IsRational`; `false` without NURB).
    #[must_use]
    pub fn is_rational(&self) -> bool {
        let Some(sur) = self.nurb_ref() else {
            return false; // FS
        };
        match sur.cpt(0, 0) {
            Ok(cp) => cp.w != NOW,
            Err(_) => false,
        }
    }

    /// `[U[0], U[r]] x [V[0], V[s]]` (`GetNaturalUVDomain`; `(0, 0, 0, 0)` without NURB).
    #[must_use]
    pub fn natural_uv_domain(&self) -> Extent2d {
        let Some(sur) = self.nurb_ref() else {
            return Extent2d::from_bounds(0.0, 0.0, 0.0, 0.0); // FS
        };
        let u = sur.u_knots();
        let v = sur.v_knots();
        let first = |k: &[f64]| k.first().copied().unwrap_or(f64::NAN);
        let last = |k: &[f64]| k.last().copied().unwrap_or(f64::NAN);
        Extent2d::from_bounds(first(u), first(v), last(u), last(v))
    }

    /// The distinct knots in `param`'s direction and their multiplicities, optionally
    /// only those in `ivl` (`GetKnots`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB.
    pub fn knots(
        &self,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        let sur = self.nurb_or_err(OP_KNOTS)?;
        let (knu, knv) = sur.knot_vectors()?;
        if param == SurfParamType::U {
            Ok(get_knots(&knu, ivl))
        } else {
            Ok(get_knots(&knv, ivl))
        }
    }

    /// The Greville abscissa of control point `index` in `param`'s direction
    /// (`GetGrevilleAbscissa`): the first knot for index 0, the last knot from the last
    /// control point on, else the average of knots `index + 1 ..= index + degree`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB (upstream crashes); `InvalidInput` for knots shorter than
    /// the control points and degree say (upstream reads past them).
    pub fn greville_abscissa(&self, param: SurfParamType, index: usize) -> Result<f64> {
        let sur = self.nurb_or_err(OP_GREVILLE)?;
        let (num_pts, knots, deg) = if param == SurfParamType::U {
            (sur.n() + 1, sur.u_knots(), sur.p())
        } else {
            (sur.m() + 1, sur.v_knots(), sur.q())
        };
        let short = || invalid(OP_GREVILLE, "knots shorter than the control points say");

        if index < 1 {
            // at or below the beginning
            return knots.first().copied().ok_or_else(short);
        }
        if index >= num_pts - 1 {
            // at or beyond the end
            return knots.last().copied().ok_or_else(short);
        }

        let mut ret_val = 0.0;
        for i in 1..=deg {
            ret_val += knots.get(index + i).copied().ok_or_else(short)?;
        }
        ret_val /= deg as f64;
        Ok(ret_val)
    }

    /// The STEP canonical data (`GetCanonical`): degrees, Euclidean control points
    /// (homogeneous `x`, `y`, `z` divided by the weight of rational points), form,
    /// distinct knots with multiplicities, knot type and weights (empty unless some
    /// point is rational).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without NURB.
    pub fn canonical(&self) -> Result<SurfaceCanonical> {
        let sur = self
            .nurb_ref()
            .ok_or(SolidError::null_pointer(OP_GET_CANON))?;
        let (knu, knv) = sur.knot_vectors()?;
        let (u_knots, u_multiplicities) = get_knots(&knu, None);
        let (v_knots, v_multiplicities) = get_knots(&knv, None);
        let (control_points, weights) = euclidean_net(sur);
        Ok(SurfaceCanonical {
            u_degree: sur.p(),
            v_degree: sur.q(),
            control_points,
            form: self.form,
            u_multiplicities,
            v_multiplicities,
            u_knots,
            v_knots,
            knot_type: self.knot_type,
            weights,
        })
    }

    /// The control net in Euclidean form with its weights (`GetControlPointNet`); the
    /// weights are empty unless some point is rational.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB (upstream crashes).
    pub fn control_point_net(&self) -> Result<ControlPointNet> {
        let sur = self.nurb_or_err(OP_NET)?;
        let (points, weights) = euclidean_net(sur);
        Ok(ControlPointNet {
            u_count: sur.n() + 1,
            v_count: sur.m() + 1,
            points,
            weights,
        })
    }

    /// The control point counts in u and v and a copy of the control points as stored
    /// (homogeneous, `z == NOZ` for 2D, `w == NOW` for non-rational points), row by row
    /// (`GetControlPointsPointer`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB (upstream crashes).
    pub fn control_points(&self) -> Result<(usize, usize, Vec<CPoint>)> {
        let sur = self.nurb_or_err(OP_NET)?;
        let pts = sur.rows().flat_map(|row| row.iter().copied()).collect();
        Ok((sur.n() + 1, sur.m() + 1, pts))
    }

    /// Whether the surface degenerates to a curve (`IsDegenerateCurve`): passed to
    /// `SmSurface::IsDegenerateCurve`.
    ///
    /// # Errors
    ///
    /// Those of `SmSurface::IsDegenerateCurve`.
    pub fn is_degenerate_curve(
        &self,
        k: &Kernel,
        angle_tol_deg: f64,
        uv_domain: Option<&Extent2d>,
        tol_3d: f64,
    ) -> Result<bool> {
        // gwc: for now - pass the call along
        surface_is_degenerate_curve(k, self, angle_tol_deg, uv_domain, tol_3d)
    }

    /// Whether the surface is parallel to `vector`: all control points of every u row
    /// (or of every v column) lie on a line parallel to it (`IsParallelToVector`).
    /// `false` without NURB; `true` for a zero vector (see the module docs).
    ///
    /// The `SmPlane` branch is
    /// [`crate::surface::plane::Plane::is_parallel_to_vector`].
    ///
    /// # Errors
    ///
    /// None (the `Result` is kept for the derived surfaces).
    pub fn is_parallel_to_vector(&self, vector: Vec3) -> Result<bool> {
        let Ok(test_vec) = vector.unitize() else {
            // SER(sTestVec.Unitize()): SM_ERR_INVALID_INPUT returned as the boolean
            return Ok(true);
        };

        // the SmPlane branch (|Z . v| < SM_EFF_ZERO * (1 + |origin|max)) is
        // Plane::is_parallel_to_vector: a plane is never a plain BSplineSurface here

        let Some(sur) = self.nurb_ref() else {
            return Ok(false); // FS
        };
        let (n, m) = (sur.n(), sur.m());

        // A line through net[a] and net[b] parallel to the vector with the points
        // `rest` on it.
        let line_ok = |line_pnt: Point3,
                       line_end: Point3,
                       rest: &mut dyn Iterator<Item = Result<Point3>>|
         -> Result<bool> {
            let tol = EFF_ZERO * (1.0 + line_pnt.max_dimension() + line_end.max_dimension());
            let line_vec = line_end - line_pnt;
            if line_vec.length_squared() < EFF_ZERO {
                return Ok(false);
            }
            let line_vec = line_vec.unitize()?;
            if line_vec.dot(test_vec).abs() < 1.0 - tol {
                return Ok(false);
            }
            for pnt in rest {
                // smgu_LinePointDistance(LinePnt, LineEnd, ..): LineEnd is the direction
                let (distance, _) = line_point_distance(line_pnt, line_end, pnt?);
                if distance > tol {
                    return Ok(false);
                }
            }
            Ok(true)
        };

        // First check U constant control points.
        let mut failed = false;
        for i_u in 0..=n {
            let line_pnt = euclid_at(sur, i_u, 0)?;
            let line_end = euclid_at(sur, i_u, 1)?;
            let mut rest = (2..=m).map(|i_v| euclid_at(sur, i_u, i_v));
            if !line_ok(line_pnt, line_end, &mut rest)? {
                failed = true;
                break;
            }
        }
        if !failed {
            return Ok(true);
        }

        // First check V constant control points.
        failed = false;
        for i_v in 0..=m {
            let line_pnt = euclid_at(sur, 0, i_v)?;
            let line_end = euclid_at(sur, 1, i_v)?;
            let mut rest = (2..=n).map(|i_u| euclid_at(sur, i_u, i_v));
            if !line_ok(line_pnt, line_end, &mut rest)? {
                failed = true;
                break;
            }
        }
        if !failed {
            return Ok(true);
        }
        Ok(false)
    }

    // ---- bounding boxes ---------------------------------------------------------------

    /// The bounding boxes asked for in `req` (`CalculateBoundingBox`): the axis aligned
    /// and pseudo boxes of the control net (always of the whole surface), and the polar
    /// box of the normals (sampled over `uv_domain` for "analytic" low degree surfaces,
    /// from the net otherwise). A basis guess replaces the computed box when it gives
    /// a smaller one.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: the polar box calls `SmSurface::EvaluateNormal` and
    /// the zone tolerance
    /// `SmSurface::GetFace`. `SM_ERR` without NURB (upstream crashes).
    pub fn calculate_bounding_box(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        req: &BoundingBoxRequest<'_>,
    ) -> Result<BoundingBoxes> {
        self.calculate_bounding_box_of(k, uv_domain, req, None)
    }

    /// [`BSplineSurface::calculate_bounding_box`] run for `derived`, the surface this
    /// B-spline is the base of (an `SmCone`, `SmSphere`, `SmTorus`, ... inheriting the
    /// override): its `IsKindOf` answers the analytic type test and its virtual
    /// `EvaluateNormal` gives the sampled normals. `None` is a plain B-spline.
    ///
    /// # Errors
    ///
    /// As [`BSplineSurface::calculate_bounding_box`], and those of `derived`'s normal
    /// evaluation.
    pub fn calculate_bounding_box_of(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        req: &BoundingBoxRequest<'_>,
        derived: Option<&crate::surface::surface::Surface>,
    ) -> Result<BoundingBoxes> {
        // the virtual IsKindOf and EvaluateNormal of `this`
        let kind_of = |t: i64| match derived {
            Some(d) => d.is_kind_of(t),
            None => self.is_kind_of(t),
        };
        let evaluate_normal = |k: &Kernel, _: &Self, uv: Point2| match derived {
            Some(d) => d.evaluate_normal(
                k,
                uv,
                crate::nurbs::knots::Side::Left,
                crate::nurbs::knots::Side::Left,
            ),
            None => evaluate_normal(k, self, uv),
        };

        // init output
        let mut normal_box = req.normal.then(Extent3d::new);
        let mut pseudo_box = req.pseudo.then(PseudoBox::new);
        let mut polar_box = req.polar.then(PolarBox::new);

        let polar_guess = req
            .polar_guess
            .filter(|g| g.state() != PolarBoxState::Uninitialized);

        // locals
        let mut trial_pseudo_box = PseudoBox::new();
        let mut trial_polar_box = PolarBox::new();
        let mut guess_basis: Option<Vec3> = None;

        // (GetNaturalUVDomain: only used by the SE'd interval test below)
        let _natural_uv_domain = self.natural_uv_domain();

        let u_degree = self.degree(SurfParamType::U);
        let v_degree = self.degree(SurfParamType::V);
        let u_knots = self.number_natural_knots(SurfParamType::U);
        let v_knots = self.number_natural_knots(SurfParamType::V);

        // check for analytic/non-analytic splines
        let mut planar = u_degree == 1 && u_knots <= 4 && v_degree == 1 && v_knots <= 4;
        let mut analytic = !((u_degree == 1 && u_knots > 4)
            || (u_degree == 2 && u_knots > 6)
            || (u_degree > 2)
            || (v_degree == 1 && v_knots > 4)
            || (v_degree == 2 && v_knots > 6)
            || (v_degree > 2)
            || (u_degree == 2 && v_degree == 2));

        // also check the derived type of the surface
        if kind_of(PLANE_TYPE)?
            || kind_of(CONE_TYPE)?
            || kind_of(SPHERE_TYPE)?
            || kind_of(TORUS_TYPE)?
        {
            analytic = true;
        }

        // locals
        let sur = self.nurb_or_err(OP_BBOX)?;
        let (n, m) = (sur.n(), sur.m());

        // Correction for poor approximation to 'plane' (status ignored)
        if planar {
            let flat = sur.is_flat_cheap(0.0001).is_ok_and(|f| f.flat);
            if !flat {
                planar = false;
            }
        }

        // (upstream: SE(SM_ERR_INVALID_INPUT) when uv_domain is not contained by the
        // natural domain - signalled only, not returned)

        // Compute the basis vectors of the PseudoBox using the control polygon
        if let Some(pb) = pseudo_box.as_mut() {
            let p00 = euclid_at(sur, 0, 0)?;
            let p10 = euclid_at(sur, n, 0)?;
            let p01 = euclid_at(sur, 0, m)?;
            let p11 = euclid_at(sur, n, m)?;

            let v1 = (p00 - p10) + (p01 - p11);
            let v2 = (p00 - p01) + (p10 - p11);
            let v1_ls = v1.length_squared();
            if v1_ls > EFF_ZERO_SQ {
                let [b1, b2, b3] = v1
                    .make_unit_ortho_vectors(Some(v2))
                    .unwrap_or([Vec3::UNDEF; 3]);
                *pb = PseudoBox::new(); // Initialize it
                                        // (SetBasis status ignored)
                let _ = pb.set_basis(b1, b2, b3);
            } else {
                *pb = PseudoBox::new(); // Initialize it
            }

            // when given a guess for the basis vectors
            if let Some(g) = req.pseudo_guess {
                trial_pseudo_box.init();
                let [g0, g1, g2] = *g.basis();
                let _ = trial_pseudo_box.set_basis(g0, g1, g2);
            }
        }

        // Compute polar bounding box really fast for analytics
        if let (Some(pol), true) = (polar_box.as_mut(), analytic) {
            const V_CNT: usize = 10;
            let mut norm = [Vec3::UNDEF; V_CNT];

            // get center normal
            norm[9] = evaluate_normal(k, self, uv_domain.evaluate(0.5, 0.5))?;

            if planar {
                pol.add_vector(norm[9], None);
                if polar_guess.is_some() {
                    trial_polar_box.add_vector(norm[9], None);
                }
            } else {
                // get corner normals
                norm[5] = evaluate_normal(k, self, uv_domain.evaluate(0.0, 0.0))?;
                norm[6] = evaluate_normal(k, self, uv_domain.evaluate(1.0, 0.0))?;
                norm[7] = evaluate_normal(k, self, uv_domain.evaluate(1.0, 1.0))?;
                norm[8] = evaluate_normal(k, self, uv_domain.evaluate(0.0, 1.0))?;

                // Build a central vector that can be used as a good PolarBox base vector
                norm[0] = if n <= 2 && m <= 2 {
                    (norm[5] + norm[6] + norm[7] + norm[8]) / 4.0
                } else {
                    norm[9]
                };

                // when we have a midPoint vector for the polar box
                if norm[0].length_squared() > EFF_ZERO_SQ {
                    // get mid side normals
                    norm[1] = evaluate_normal(k, self, uv_domain.evaluate(0.0, 0.5))?;
                    norm[2] = evaluate_normal(k, self, uv_domain.evaluate(1.0, 0.5))?;
                    norm[3] = evaluate_normal(k, self, uv_domain.evaluate(0.5, 0.0))?;
                    norm[4] = evaluate_normal(k, self, uv_domain.evaluate(0.5, 1.0))?;

                    // build the polar box by adding all the vectors
                    pol.reset();
                    for v in norm {
                        pol.add_vector(v, None);
                    }

                    if let Some(g) = polar_guess {
                        guess_basis = Some(polar_guess_basis(g));
                        trial_polar_box.reset();
                        for v in norm {
                            trial_polar_box.add_vector(v, guess_basis);
                        }
                    }
                }
            }
        }

        // Do a little work to try to get the polar box to be centered - use the
        // derivatives of the midpoints to establish the base line vector.
        if let (Some(pol), false) = (polar_box.as_mut(), analytic) {
            let n0 = evaluate_normal(k, self, uv_domain.evaluate(0.5, 0.5))?;
            pol.add_vector(n0, None);
            let n1 = evaluate_normal(k, self, uv_domain.evaluate(1.0, 0.5))?;
            pol.add_vector(n1, None);
            let n2 = evaluate_normal(k, self, uv_domain.evaluate(0.5, 1.0))?;
            pol.add_vector(n2, None);

            if let Some(g) = polar_guess {
                guess_basis = Some(polar_guess_basis(g));
                trial_polar_box.reset();
                trial_polar_box.add_vector(n0, guess_basis);
                trial_polar_box.add_vector(n1, guess_basis);
                trial_polar_box.add_vector(n2, guess_basis);
            }
        }

        // iter over the BSpline SurfaceNet ControlPoints
        let net_polar = polar_box.is_some() && !analytic;
        if normal_box.is_some() || pseudo_box.is_some() || net_polar {
            for i in 0..=n {
                for j in 0..=m {
                    // get controlPoint[i][j]
                    let point = euclid_at(sur, i, j)?;

                    // add control points (or their differences) to requested bounding boxes
                    if let Some(nb) = normal_box.as_mut() {
                        if i == 0 && j == 0 {
                            *nb = Extent3d::from_points(point, point);
                        } else {
                            nb.add_point(point);
                        }
                    }
                    if let Some(pb) = pseudo_box.as_mut() {
                        pb.add_point(point);
                        if req.pseudo_guess.is_some() {
                            trial_pseudo_box.add_point(point);
                        }
                    }
                    if let (Some(pol), true) = (polar_box.as_mut(), net_polar) {
                        // get control net tangent vectors
                        let mut add = |cross: Vec3, pol: &mut PolarBox| {
                            pol.add_vector(cross, None);
                            if polar_guess.is_some() {
                                trial_polar_box.add_vector(cross, guess_basis);
                            }
                        };
                        let dv0 = if j > 1 {
                            Some(point - euclid_at(sur, i, j - 1)?)
                        } else {
                            None
                        };
                        let dv1 = if j < m {
                            Some(euclid_at(sur, i, j + 1)? - point)
                        } else {
                            None
                        };
                        if i > 1 {
                            let du0 = point - euclid_at(sur, i - 1, j)?;
                            if let Some(dv0) = dv0 {
                                add(du0.cross(dv0), pol);
                            }
                            if let Some(dv1) = dv1 {
                                add(du0.cross(dv1), pol);
                            }
                        }
                        if i < n {
                            let du1 = euclid_at(sur, i + 1, j)? - point;
                            if let Some(dv0) = dv0 {
                                add(du1.cross(dv0), pol);
                            }
                            if let Some(dv1) = dv1 {
                                add(du1.cross(dv1), pol);
                            }
                        }
                    }
                }
            }

            // save the better Psuedo box
            if let (Some(pb), Some(_)) = (pseudo_box.as_mut(), req.pseudo_guess) {
                if trial_pseudo_box.volume() < pb.volume() - EFF_ZERO {
                    *pb = trial_pseudo_box;
                }
            }

            // save the better Polar box
            if let (Some(pol), true, Some(_)) = (polar_box.as_mut(), net_polar, polar_guess) {
                if trial_polar_box.polar_area() < pol.polar_area() - EFF_ZERO {
                    *pol = trial_polar_box;
                }
            }
        }

        // When asked, expand the normal box by the zone tolerance
        if req.expand_by_zone_tol {
            if let Some(nb) = normal_box.as_mut() {
                nb.expand_absolute(owner_zone_tolerance(req));
            }
            // When asked, expand the pPseudoBox box by the zone tolerance
            if let Some(pb) = pseudo_box.as_mut() {
                pb.expand_absolute(owner_zone_tolerance(req));
            }
        }

        Ok(BoundingBoxes {
            normal: normal_box,
            pseudo: pseudo_box,
            polar: polar_box,
        })
    }

    // ---- factories --------------------------------------------------------------------

    /// The non-rational bilinear surface interpolating four corners
    /// (`CreateBilinearSurface`): `S(0,0) = u0v0`, `S(1,0) = u1v0`, `S(0,1) = u0v1`,
    /// `S(1,1) = u1v1`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when NLib fails.
    pub fn create_bilinear_surface(
        u0v0: Point3,
        u1v0: Point3,
        u0v1: Point3,
        u1v1: Point3,
    ) -> Result<Self> {
        // create an NLib NonRational bilinear BSpline Surface interpolating the 4 corner points
        let sur = corner_pts(u0v0, u1v0, u0v1, u1v1).map_err(SolidError::nl_ser(OP_BILINEAR))?;
        Ok(Self::from_gw_nurb(&sur))
    }

    /// The surface of STEP canonical data (`CreateCanonical`): distinct knots expanded
    /// by their multiplicities, Euclidean control points (row by row), weights (the
    /// surface is non-rational when all are within `SM_EFF_ZERO` of 1) and an
    /// optional domain overriding the parameter bounds; built with `N_Iges128Srf`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a degree outside 1..=32, a weight count other than the point
    /// count, a point count other than the product of the counts the knots and degrees
    /// give, fewer multiplicities than knots or no control points; `SM_ERR` when NLib
    /// fails.
    pub fn create_canonical(data: &CanonicalSurfaceData<'_>) -> Result<Self> {
        let u_degree = data.u_degree;
        let v_degree = data.v_degree;
        let cps = data.control_points;

        // checkinput
        check(
            (1..=32).contains(&u_degree),
            OP_CANON,
            "u degree outside 1..=32",
        )?;
        check(
            (1..=32).contains(&v_degree),
            OP_CANON,
            "v degree outside 1..=32",
        )?;

        // If have weights make sure have right number of them
        if let Some(w) = data.weights {
            if !w.is_empty() && w.len() != cps.len() {
                return Err(invalid(OP_CANON, "weight count differs from point count"));
            }
        }

        // Make sNewUKnots / sNewVKnots arrays with one entry for each multiplicity
        // (kept implicit until the control points bound their size)
        let exp_u = ExpandedKnots::new(data.u_knots, data.u_multiplicities, OP_CANON)?;
        let exp_v = ExpandedKnots::new(data.v_knots, data.v_multiplicities, OP_CANON)?;

        // Get number of ControlPoints (ULONG arithmetic)
        let num_u_cpts = exp_u.len().wrapping_sub(u_degree).wrapping_sub(1);
        let num_v_cpts = exp_v.len().wrapping_sub(v_degree).wrapping_sub(1);

        // check input - number of ControlPoints is consistent with Knot Count and degree
        if num_u_cpts.wrapping_mul(num_v_cpts) != cps.len() {
            return Err(invalid(
                OP_CANON,
                "point count inconsistent with knots and degrees",
            ));
        }
        if num_u_cpts == 0
            || num_v_cpts == 0
            || num_u_cpts > exp_u.len()
            || num_v_cpts > exp_v.len()
        {
            // upstream builds k1 = -1 or wraps its sizes
            return Err(invalid(OP_CANON, "no control points"));
        }

        // when weights are all 1.0 within tolerance, make BSpline non-rational.
        let mut non1_weight = false;
        if let Some(w) = data.weights {
            for &wi in w {
                if (wi - 1.0).abs() > EFF_ZERO {
                    non1_weight = true;
                }
            }
        }

        // set up weight rows (rational with given weights, else all 1.0)
        let weights_flat: Vec<f64> = match data.weights {
            Some(w) if non1_weight && !w.is_empty() => w.to_vec(),
            _ => vec![1.0; cps.len()],
        };
        let rows = |k: usize| -> Option<core::ops::Range<usize>> {
            Some(k.checked_mul(num_v_cpts)?..k.checked_add(1)?.checked_mul(num_v_cpts)?)
        };
        let mut w_rows: Vec<Vec<f64>> = Vec::new();
        let mut p_rows: Vec<Vec<Point3>> = Vec::new();
        for k in 0..num_u_cpts {
            let wr = rows(k)
                .and_then(|r| weights_flat.get(r))
                .ok_or_else(|| invalid(OP_CANON, "too few weights"))?;
            let pr = rows(k)
                .and_then(|r| cps.get(r))
                .ok_or_else(|| invalid(OP_CANON, "too few control points"))?;
            w_rows.push(wr.to_vec());
            p_rows.push(pr.to_vec());
        }

        // the control net bounds the knot counts now (num_u_cpts * num_v_cpts points)
        let new_u_knots = exp_u.prefix(exp_u.len(), OP_CANON)?;
        let new_v_knots = exp_v.prefix(exp_v.len(), OP_CANON)?;

        let knot = |k: &[f64], i: usize| -> Result<f64> {
            k.get(i)
                .copied()
                .ok_or_else(|| invalid(OP_CANON, "too few knots"))
        };

        // init u and v to domain min/max
        let (u, v) = if let Some(d) = &data.uv_domain {
            ([d.min().x, d.max().x], [d.min().y, d.max().y])
        } else {
            // get u and v boundary values from the knot arrays
            (
                [
                    knot(&new_u_knots, u_degree)?,
                    knot(&new_u_knots, num_u_cpts)?,
                ],
                [
                    knot(&new_v_knots, v_degree)?,
                    knot(&new_v_knots, num_v_cpts)?,
                ],
            )
        };

        // build the Nlib BSpline object
        let sur = iges_128_surface(&Entity128 {
            k1: num_u_cpts - 1,
            k2: num_v_cpts - 1,
            m1: u_degree,
            m2: v_degree,
            s: &new_u_knots,
            t: &new_v_knots,
            w: &w_rows,
            pts: &p_rows,
            u,
            v,
        })
        .map_err(SolidError::nl_ser(OP_CANON))?;

        // build NMTlib SmBSpline from Nlib Nurb
        let mut srf = Self::from_gw_nurb(&sur);
        srf.form = data.form;
        srf.knot_type = data.knot_type;
        Ok(srf)
    }

    /// `sm_CreateConePatch`: the cone (or cylinder) patch of [`create_cone_patch`]
    /// with the circle in u (`SM_SP_U`, normal outward) or v (any other value, normal
    /// inward).
    ///
    /// [`create_cone_patch`]: BSplineSurface::create_cone_patch
    fn create_cone_patch_dir(
        reference_frame: &Axis2Placement,
        bottom_radius: f64,
        top_radius: f64,
        start_angle_deg: f64,
        end_angle_deg: f64,
        height: f64,
        parameterization: NurbCircleParam,
        srf_param: SurfParamType,
    ) -> Result<Self> {
        check(bottom_radius >= 0.0, OP_CONE, "negative bottom radius")?;
        check(top_radius >= 0.0, OP_CONE, "negative top radius")?;
        check(
            (0.0..=360.0).contains(&start_angle_deg),
            OP_CONE,
            "start angle outside [0, 360]",
        )?;
        check(
            (0.0..=360.0).contains(&end_angle_deg),
            OP_CONE,
            "end angle outside [0, 360]",
        )?;

        let (c, x, y) = extract_frame(reference_frame);

        let ctp = match parameterization {
            NurbCircleParam::Quadratic => ConicType::Quadratic, // degree 2
            NurbCircleParam::Quintic => ConicType::Quintic,     // degree 5
        };

        let dir = nl_dir(srf_param);
        let sur = cyl_cone(
            &CylCone {
                center: c,
                x,
                y,
                rb: bottom_radius,
                rt: top_radius,
                start: start_angle_deg,
                end: end_angle_deg,
                h: height,
            },
            ctp,
            dir,
        )
        .map_err(SolidError::nl_ser(OP_CONE))?;

        let mut srf = Self::from_gw_nurb(&sur);
        srf.set_b_spline_surface_form(BSplineSurfaceForm::ConicalSurf);
        if are_same(bottom_radius, top_radius) {
            srf.set_b_spline_surface_form(BSplineSurfaceForm::CylindricalSurf);
        }
        Ok(srf)
    }

    /// A cone (or, with equal radii, cylinder) patch (`CreateConePatch`): the base
    /// circle of radius `bottom_radius` lies in the XY plane of `reference_frame` (angle
    /// 0 on its X axis), the top circle of radius `top_radius` at `height` along its Z
    /// axis; the side runs counter-clockwise from `start_angle_deg` to
    /// `end_angle_deg`. The circle is the u direction (normal outward).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a negative (or NaN) radius or an angle outside [0, 360];
    /// `SM_ERR` when NLib fails (e.g. both radii zero).
    pub fn create_cone_patch(
        reference_frame: &Axis2Placement,
        bottom_radius: f64,
        top_radius: f64,
        start_angle_deg: f64,
        end_angle_deg: f64,
        height: f64,
        parameterization: NurbCircleParam,
    ) -> Result<Self> {
        Self::create_cone_patch_dir(
            reference_frame,
            bottom_radius,
            top_radius,
            start_angle_deg,
            end_angle_deg,
            height,
            parameterization,
            SurfParamType::U,
        )
    }

    /// The surface swept by `curve_to_sweep` along `sweep_vector` (`CreateLinearSweep`):
    /// linear in u (`S(0, v)` is the curve), the curve's parameterisation in v. A 2D
    /// curve's NURB is made 3D in place first (see the module docs).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a sweep vector shorter than `SM_EFF_ZERO`; `SM_ERR` when the
    /// curve has no NURB or NLib fails.
    pub fn create_linear_sweep(
        curve_to_sweep: &mut BSplineCurve,
        sweep_vector: Vec3,
    ) -> Result<Self> {
        // locals
        let dist = sweep_vector.length();

        // check state - need nonZero SweepVector
        if dist < EFF_ZERO {
            return Err(invalid(OP_SWEEP, "zero sweep vector"));
        }
        let w = sweep_vector.unitize()?;

        // pass the call along
        let cur = curve_to_sweep
            .get_or_create_gw_nurb()
            .map_err(SolidError::nl_ser(OP_SWEEP))?;
        // extruded surface is linear in u direction
        let sur = extrude_crv(cur, w, dist, Direction::U).map_err(SolidError::nl_ser(OP_SWEEP))?;

        // set output
        Ok(Self::from_gw_nurb(&sur))
    }

    /// The offset of this surface by `signed_offset_distance` along its normal
    /// (`CreateOffsetSurface`: passed to `SmSurface::CreateOffsetSurface`, an implicit
    /// `SmOffsetSurface` that owns a copy of this surface; `approx_tol_3d` is not used).
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::new`](crate::surface::offset_surface::OffsetSurface::new).
    pub fn create_offset_surface(
        &self,
        k: &Kernel,
        signed_offset_distance: f64,
        approx_tol_3d: f64,
    ) -> Result<crate::surface::surface::Surface> {
        let _ = approx_tol_3d;
        // Create implicit offset surface (SmSurface::CreateOffsetSurface: Copy, then
        // an owning SmOffsetSurface)
        let copy = crate::surface::surface::Surface::from(self.copy());
        Ok(crate::surface::surface::Surface::from(
            crate::surface::offset_surface::OffsetSurface::new(
                k,
                signed_offset_distance,
                copy,
                true,
                crate::surface::offset_surface::SS_UNKNOWN,
            )?,
        ))
    }

    // ---- editing ----------------------------------------------------------------------

    /// Scales the control points by `scale` (about the origin, when given) and then
    /// rotates and moves them by `rotate_n_move` (`Transform`); the identity frame
    /// with no or unit scale changes nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB (upstream crashes).
    pub fn transform(&mut self, rotate_n_move: &Axis2Placement, scale: Option<Vec3>) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        let rp = rotate_n_move.origin();
        let rx = rotate_n_move.x_axis();
        let ry = rotate_n_move.y_axis();
        let sz = rotate_n_move.z_axis();

        let s = scale.unwrap_or(identity_scale);
        let rm = [
            [rx.x * s.x, ry.x * s.y, sz.x * s.z, rp.x],
            [rx.y * s.x, ry.y * s.y, sz.y * s.z, rp.y],
            [rx.z * s.x, ry.z * s.y, sz.z * s.z, rp.z],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let mut rma = RealMatrix::new(3, 3, MatrixType::Full, 3)?;
        for (i, row) in rm.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                rma.set(i, j, v)?;
            }
        }

        let sur = self
            .get_or_create_gw_nurb()
            .map_err(|_| SolidError::sm_err(OP_XFORM))?;
        // (N_SrfTransform status ignored)
        let _ = sur.transform(&rma);
        Ok(())
    }

    /// A copy mirrored about the XY plane of `mirror_plane` (`CreateMirrorSurface`):
    /// the copy constructor's compact NURB copy (knots fixed up), its control points
    /// transformed by the 3x3-sized mirror matrix (`N_SrfTransform`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB (upstream dereferences a null one).
    pub fn create_mirror_surface(&self, mirror_plane: &Axis2Placement) -> Result<Self> {
        let mut rma = RealMatrix::new(3, 3, MatrixType::Full, 3)?;
        for (i, row) in mirror_plane.mirror_matrix().iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                rma.set(i, j, v)?;
            }
        }
        let mut bss = self.copy();
        // (N_SrfTransform status ignored)
        let _ = bss
            .get_or_create_gw_nurb()
            .map_err(|_| SolidError::sm_err(OP_XFORM))?
            .transform(&rma);
        Ok(bss)
    }

    // ---- inversion --------------------------------------------------------------------

    /// The parameters of `point_on_surf` (which must lie on the surface within
    /// `tol_3d`) and its location (`STEPInversion`): a local solve from `uv_guess`
    /// when given, else a global solve; a pole, or a seam of a closed direction when
    /// several solutions exist.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: a guess calls `SmSurface::LocalPointSolve`
    /// (`surface.surface_p5`), and so does the base `IsSingularity` of a B-spline
    /// (a plane uses its override). `SM_ERR` when no
    /// solution is found. This base entry point runs on a plain B-spline surface; the
    /// plane dispatch is [`BSplineSurface::step_inversion_on`].
    pub fn step_inversion(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<(Point2, LocationType)> {
        let surface = crate::surface::surface::Surface::from(self.clone());
        self.step_inversion_on(k, &surface, anal_domain, point_on_surf, tol_3d, uv_guess)
    }

    /// [`BSplineSurface::step_inversion`] where the virtual solves run on `surface`,
    /// the surface this B-spline part belongs to (a plane's `this`).
    ///
    /// # Errors
    ///
    /// As [`BSplineSurface::step_inversion`].
    pub fn step_inversion_on(
        &self,
        k: &Kernel,
        surface: &crate::surface::surface::Surface,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<(Point2, LocationType)> {
        self.step_inversion_on_in::<crate::surface::surface::p4::NoObject>(
            k,
            surface,
            anal_domain,
            point_on_surf,
            tol_3d,
            uv_guess,
            None,
        )
    }

    /// [`BSplineSurface::step_inversion_on`] whose `GlobalPointSolve` runs under the
    /// caller's face context (`context`: the solve context and the surface's handle in
    /// it): a face-owned surface's solutions are then classified against the face's
    /// trims, as upstream's `GlobalPointSolve` does through the surface's face.
    ///
    /// # Errors
    ///
    /// As [`BSplineSurface::step_inversion`], with those of the context.
    #[allow(clippy::too_many_arguments)]
    pub fn step_inversion_on_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: &crate::surface::surface::Surface,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
        context: (&mut dyn SurfaceSolveContext<O>, Option<O>),
    ) -> Result<(Point2, LocationType)> {
        self.step_inversion_on_in(
            k,
            surface,
            anal_domain,
            point_on_surf,
            tol_3d,
            uv_guess,
            Some(context),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn step_inversion_on_in<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: &crate::surface::surface::Surface,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
        context: Option<(&mut dyn SurfaceSolveContext<O>, Option<O>)>,
    ) -> Result<(Point2, LocationType)> {
        // init output
        let mut location = LocationType::Interior;

        // intersect point with Surface
        let mut solutions: Vec<Point2> = Vec::new();

        // If a guess is given, try a local solve first.
        let mut local_succeeded = false;
        if let Some(guess) = uv_guess {
            let (ok, sol) = local_point_solve(k, surface, anal_domain, point_on_surf, guess)?;
            local_succeeded = ok;
            if local_succeeded {
                solutions.push(sol);
            }
        }

        if !local_succeeded {
            solutions.extend(match context {
                Some(context) => global_point_solve_with_context(
                    k,
                    surface,
                    anal_domain,
                    point_on_surf,
                    tol_3d,
                    context,
                )?,
                None => global_point_solve(k, surface, anal_domain, point_on_surf, tol_3d)?,
            });
        }

        // Point not close enough to surface - error
        let Some(&uv) = solutions.first() else {
            return Err(SolidError::sm_err(OP_STEP_INV));
        };

        // when point is on a singular boundary
        if is_singularity(k, surface, self, uv)? {
            location = LocationType::Pole;
        }

        // done for interior and pole points
        if location == LocationType::Pole || solutions.len() == 1 {
            return Ok((uv, location));
        }

        // when surface is closed in U direction and solution on Seam
        if is_closed(k, surface, self, anal_domain, SurfParamType::U)? {
            let ivl = Extent1d::from_min_max(anal_domain.min().x, anal_domain.max().x);
            if ivl.is_value_on_boundary(uv.x, EFF_ZERO).0 {
                location = LocationType::USeam;
            }
        }

        // when surface is closed in V direction and solution on Seam
        if is_closed(k, surface, self, anal_domain, SurfParamType::V)? {
            let ivl = Extent1d::from_min_max(anal_domain.min().y, anal_domain.max().y);
            if ivl.is_value_on_boundary(uv.y, EFF_ZERO).0 {
                location = if location == LocationType::Interior {
                    LocationType::VSeam
                } else {
                    LocationType::UvSeam
                };
            }
        }

        Ok((uv, location))
    }

    // ---- database ---------------------------------------------------------------------

    /// Reads a surface written by `WriteToDB` (`ReadFromDB`): `n m`, `p q`, the
    /// rational flag, for `version > 32` the knot type and form, then the control
    /// points (`wx wy wz`, plus `w` when rational, v index fastest), `n+p+2` u knots and
    /// `m+q+2` v knots (one group per line in ASCII files). The knots are not fixed up.
    ///
    /// # Errors
    ///
    /// `InvalidInput` (`SM_ERR_INVALID_INPUT`) when a read fails part way (data ends
    /// early or is not a number), for degrees outside `1..=NL_MAXDEG`, fewer control
    /// points than degree + 1 in a direction or too many to size, a rational flag other
    /// than 0 or 1, a knot type or form outside its enum, or a short binary stream.
    pub fn read_from_db(db: &mut DatabaseReader, version: u64) -> Result<Self> {
        let bad = |reason: &'static str| invalid(OP_READ, reason);

        let mut knot_type_code: u64 = KnotType::Unspecified as u64;
        let mut form_code: u64 = BSplineSurfaceForm::Unspecified as u64;

        // Get parameters from the top
        let ascii = db.file_type() == FileType::Ascii;
        let (n, m, p, q, rat) = if ascii {
            let s = db.in_stream_mut().ok_or_else(|| bad("no input stream"))?;
            let n = s.extract_long();
            let m = s.extract_long();
            db.go_to_next_line();
            let s = db.in_stream_mut().ok_or_else(|| bad("no input stream"))?;
            let p = s.extract_short();
            let q = s.extract_short();
            db.go_to_next_line();
            let s = db.in_stream_mut().ok_or_else(|| bad("no input stream"))?;
            let rat = s.extract_short();
            db.go_to_next_line();
            if version > 32 {
                if let Some(v) = db
                    .in_stream_mut()
                    .and_then(crate::topology::database_io::InputStream::extract_ulong)
                {
                    knot_type_code = v;
                }
                db.go_to_next_line();
                if let Some(v) = db
                    .in_stream_mut()
                    .and_then(crate::topology::database_io::InputStream::extract_ulong)
                {
                    form_code = v;
                }
                db.go_to_next_line();
            }
            match (n, m, p, q, rat) {
                (Some(n), Some(m), Some(p), Some(q), Some(rat)) => (n, m, p, q, rat),
                _ => return Err(bad(READ_FAILED)),
            }
        } else {
            let n = db.read_long()?;
            let m = db.read_long()?;
            let p = db.read_short()?;
            let q = db.read_short()?;
            let rat = db.read_short()?;
            if version > 32 {
                knot_type_code = db.read_ulong()?;
                form_code = db.read_ulong()?;
            }
            (n, m, p, q, rat)
        };
        if db.read_failed() {
            return Err(bad(READ_FAILED));
        }

        // NLib's surface evaluators (e.g. N_SrfBasisDerivs) size their basis-function
        // stack arrays by NL_MAXDEG, so a higher degree overflows the stack on the first
        // evaluation. A degree-0 surface reads, but its derivatives fail and its normal is
        // zero. The degrees come straight from the file, so reject anything outside
        // [1, NL_MAXDEG].
        let max_degree = MAX_DEGREE as i64;
        if p < 1 || i64::from(p) > max_degree || q < 1 || i64::from(q) > max_degree {
            return Err(bad(
                "NURBS surface degree is outside the supported range 1 to NL_MAXDEG",
            ));
        }

        // The counts come straight from the file too, as highest indices. Each direction
        // needs at least degree + 1 control points, and the NURB is one block whose byte
        // size is a ULONG, so reject counts that would overflow it before they size
        // anything.
        let max_cpoints = u64::MAX / (4 * 32);
        if n < i64::from(p) || m < i64::from(q) {
            return Err(bad(
                "NURBS surface control point counts are fewer than degree + 1",
            ));
        }
        if n as u64 >= max_cpoints || m as u64 >= max_cpoints / (n as u64 + 1) {
            return Err(bad("NURBS surface control point counts are too large"));
        }
        if rat != 0 && rat != 1 {
            return Err(bad(
                "NURBS surface rational flag is neither NL_NO nor NL_YES",
            ));
        }

        // surface knot counts
        let to_size = |x: i64| usize::try_from(x).map_err(|_| bad("negative size"));
        let (n, m) = (to_size(n)?, to_size(m)?);
        let (p, q) = (to_size(i64::from(p))?, to_size(i64::from(q))?);
        let too_big = || bad("size overflow");
        let r = n
            .checked_add(p)
            .and_then(|x| x.checked_add(1))
            .ok_or_else(too_big)?;
        let s = m
            .checked_add(q)
            .and_then(|x| x.checked_add(1))
            .ok_or_else(too_big)?;

        // (any other flag was rejected with the header above)
        let rational = rat == 1;

        // Bound the header against the bytes left before allocating (Rust aborts on a
        // failed huge allocation where upstream's malloc returned NULL). Binary: three
        // 8-byte doubles per point and one per knot. ASCII: at least one byte each.
        let points = n
            .checked_add(1)
            .and_then(|a| m.checked_add(1).and_then(|b| a.checked_mul(b)))
            .ok_or_else(too_big)?;
        let knots = r
            .checked_add(s)
            .and_then(|x| x.checked_add(2))
            .ok_or_else(too_big)?;
        if ascii {
            db.ensure_available(points.checked_add(knots).ok_or_else(too_big)?, 1, OP_READ)?;
        } else {
            let doubles = points
                .checked_mul(3)
                .and_then(|x| x.checked_add(knots))
                .ok_or_else(too_big)?;
            db.ensure_available(doubles, 8, OP_READ)?;
        }

        // allocate Sur memory
        let mut sur = allocate_nurb_surface(n, m, p, q, r, s)?;

        // Read in data

        let mut wx = f64::NAN;
        let mut wy = f64::NAN;
        let mut wz = f64::NAN;
        let mut w = f64::NAN;
        for i in 0..=n {
            for j in 0..=m {
                if ascii {
                    let s = db.in_stream_mut().ok_or_else(|| bad("no input stream"))?;
                    let targets: [&mut f64; 4] = [&mut wx, &mut wy, &mut wz, &mut w];
                    for (k, t) in targets.into_iter().enumerate() {
                        if k == 3 && !rational {
                            break;
                        }
                        if let Some(x) = s.extract_double() {
                            *t = x;
                        }
                    }
                    db.go_to_next_line();
                } else {
                    wx = db.read_double()?;
                    wy = db.read_double()?;
                    wz = db.read_double()?;
                    if rational {
                        w = db.read_double()?;
                    }
                }
                if db.read_failed() {
                    return Err(bad(READ_FAILED));
                }
                *sur.cpt_mut(i, j)? = CPoint::new(wx, wy, wz, if rational { w } else { NOW });
            }
        }

        // for every U knot, then every V knot
        for i in 0..=r {
            let k = if ascii {
                let x = db
                    .in_stream_mut()
                    .and_then(crate::topology::database_io::InputStream::extract_double);
                db.go_to_next_line();
                x
            } else {
                Some(db.read_double()?)
            };
            if let (Some(k), Some(slot)) = (k, sur.u_knots_mut().get_mut(i)) {
                *slot = k;
            }
            if db.read_failed() {
                return Err(bad(READ_FAILED));
            }
        }
        for j in 0..=s {
            let k = if ascii {
                let x = db
                    .in_stream_mut()
                    .and_then(crate::topology::database_io::InputStream::extract_double);
                db.go_to_next_line();
                x
            } else {
                Some(db.read_double()?)
            };
            if let (Some(k), Some(slot)) = (k, sur.v_knots_mut().get_mut(j)) {
                *slot = k;
            }
            if db.read_failed() {
                return Err(bad(READ_FAILED));
            }
        }

        // output locals
        let knot_type = knot_type_from_code(knot_type_code)
            .ok_or_else(|| bad("knot type outside SmKnotType"))?;
        let form = BSplineSurfaceForm::from_code(form_code)
            .ok_or_else(|| bad("form outside SmBSplineSurfaceForm"))?;

        // set output
        Ok(Self {
            nurb: Some(sur).into(),
            nurb_is_borrowed: false,
            out_of_bounds_enabled: AtomicBool::new(false),
            knot_type,
            form,
        })
    }

    /// `WriteToDB`: writes the NURB's last control point indices `n m`, degrees `p q`,
    /// rational flag and, for `version > 32`, knot type and form; then the control
    /// net row by row (`wx wy wz`, plus `w` when rational: the stored, weighted
    /// values) and the u and v knots.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB (upstream dereferences `NULL`).
    pub fn write_to_db(&self, db: &mut DatabaseWriter, version: u64) -> Result<()> {
        let sur = self.nurb_or_err(OP_WRITE)?;
        let (n, m) = (sur.n() as i64, sur.m() as i64);
        let (p, q) = (sur.p() as i16, sur.q() as i16);
        let knot_type = self.knot_type as u64;
        let form = self.form as u64;

        // rational flag
        let rat: i16 = i16::from(sur.is_rational());

        if db.file_type() == FileType::Ascii {
            db.out_i64(n).out_str(" ").out_i64(m).out_str("\n");
            db.out_i64(p.into()).out_str(" ").out_i64(q.into());
            db.out_str("\n");
            db.out_i64(rat.into()).out_str("\n");
            if version > 32 {
                db.out_u64(knot_type);
                db.out_str(" BSplineSurface Knot Type Flag \n");
                db.out_u64(form).out_str(" BSplineSurfaceForm Flag \n");
            }
        } else {
            db.write_long(n);
            db.write_long(m);
            db.write_short(p);
            db.write_short(q);
            db.write_short(rat);
            if version > 32 {
                db.write_long(knot_type as i64);
                db.write_long(form as i64);
            }
        }

        // control points, by rational / non-rational
        for row in sur.rows() {
            for pw in row {
                if rat == 1 {
                    db.reals(&[pw.x, pw.y, pw.z, pw.w]);
                } else {
                    db.reals(&[pw.x, pw.y, pw.z]);
                }
            }
        }

        // output u knots, then v knots
        for &u in sur.u_knots() {
            db.reals(&[u]);
        }
        for &v in sur.v_knots() {
            db.reals(&[v]);
        }
        Ok(())
    }
}

/// `SmKnotType` with upstream value `code`, if any.
fn knot_type_from_code(code: u64) -> Option<KnotType> {
    Some(match code {
        0 => KnotType::UniformKnots,
        1 => KnotType::Unspecified,
        2 => KnotType::QuasiUniformKnots,
        3 => KnotType::PiecewiseBezierKnots,
        4 => KnotType::Unknown,
        _ => return None,
    })
}

/// The guess basis vector a polar box guess contributes: its basis 2 when its domain is
/// wider in x than in y, else basis 1.
fn polar_guess_basis(g: &PolarBox) -> Vec3 {
    let d = g.domain();
    let [_, b1, b2] = *g.basis();
    if d.x_length() > d.y_length() {
        b2
    } else {
        b1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bilinear() -> BSplineSurface {
        BSplineSurface::create_bilinear_surface(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
        )
        .unwrap()
    }

    #[test]
    fn bilinear_counts_and_domain() {
        let s = bilinear();
        assert_eq!(s.degree(SurfParamType::U), 1);
        assert_eq!(s.degree(SurfParamType::V), 1);
        assert_eq!(s.number_control_points(SurfParamType::U), 2);
        assert_eq!(s.number_control_points(SurfParamType::Both), 4);
        assert_eq!(s.number_natural_knots(SurfParamType::V), 4);
        assert!(!s.is_rational());
        let d = s.natural_uv_domain();
        assert_eq!(
            (d.u_min(), d.v_min(), d.u_max(), d.v_max()),
            (0.0, 0.0, 1.0, 1.0)
        );
        let net = s.control_point_net().unwrap();
        assert_eq!(net.points.len(), 4);
        assert!(net.weights.is_empty());
        assert_eq!(net.points[1], Point3::new(0.0, 10.0, 0.0));
    }

    #[test]
    fn empty_surface_queries() {
        let mut s = BSplineSurface::new();
        assert_eq!(s.degree(SurfParamType::U), 0);
        assert_eq!(s.number_control_points(SurfParamType::Both), 0);
        assert!(!s.is_rational());
        assert!(s.make_nurb().is_err());
        assert!(s.knots(SurfParamType::U, None).is_err());
        assert!(matches!(
            s.canonical(),
            Err(SolidError::Upstream { code: 1006, .. })
        ));
        assert!(s.copy().gw_nurb().is_none());
        assert!(s.approx_eq(&BSplineSurface::new()));
    }

    #[test]
    fn canonical_round_trip() {
        let pts: Vec<Point3> = (0..6)
            .map(|k| Point3::new(f64::from(k / 3), f64::from(k % 3), f64::from(k)))
            .collect();
        let data = CanonicalSurfaceData {
            u_degree: 1,
            v_degree: 2,
            control_points: &pts,
            form: BSplineSurfaceForm::QuadricSurf,
            u_multiplicities: &[2, 2],
            v_multiplicities: &[3, 3],
            u_knots: &[0.0, 1.0],
            v_knots: &[0.0, 2.0],
            knot_type: KnotType::PiecewiseBezierKnots,
            weights: Some(&[1.0, 2.0, 1.0, 1.0, 2.0, 1.0]),
            uv_domain: None,
        };
        let s = BSplineSurface::create_canonical(&data).unwrap();
        assert!(s.is_rational());
        let c = s.canonical().unwrap();
        assert_eq!(c.u_degree, 1);
        assert_eq!(c.v_degree, 2);
        assert_eq!(c.form, BSplineSurfaceForm::QuadricSurf);
        assert_eq!(c.u_multiplicities, vec![2, 2]);
        assert_eq!(c.weights.len(), 6);
        for (a, b) in c.control_points.iter().zip(&pts) {
            assert!((*a - *b).length() < 1e-12);
        }
        let mut bad = data;
        bad.u_degree = 0;
        assert!(matches!(
            BSplineSurface::create_canonical(&bad),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn greville_and_knots() {
        let s = bilinear();
        assert_eq!(s.greville_abscissa(SurfParamType::U, 0).unwrap(), 0.0);
        assert_eq!(s.greville_abscissa(SurfParamType::U, 5).unwrap(), 1.0);
        let (k, m) = s.knots(SurfParamType::V, None).unwrap();
        assert_eq!(k, vec![0.0, 1.0]);
        assert_eq!(m, vec![2, 2]);
    }

    #[test]
    fn transform_moves_net() {
        let mut s = bilinear();
        let mut f = Axis2Placement::default();
        f.set_canonical(
            Point3::new(1.0, 2.0, 3.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        s.transform(&f, Some(Vec3::new(2.0, 2.0, 2.0))).unwrap();
        let net = s.control_point_net().unwrap();
        assert_eq!(net.points[3], Point3::new(21.0, 22.0, 3.0));
    }

    #[test]
    fn cone_patch_forms() {
        let f = Axis2Placement::default();
        let cyl = BSplineSurface::create_cone_patch(
            &f,
            1.0,
            1.0,
            0.0,
            360.0,
            1.0,
            NurbCircleParam::Quadratic,
        )
        .unwrap();
        assert_eq!(cyl.form(), BSplineSurfaceForm::CylindricalSurf);
        assert!(cyl.is_rational());
        let cone = BSplineSurface::create_cone_patch(
            &f,
            2.0,
            1.0,
            0.0,
            90.0,
            1.0,
            NurbCircleParam::Quadratic,
        )
        .unwrap();
        assert_eq!(cone.form(), BSplineSurfaceForm::ConicalSurf);
        assert!(BSplineSurface::create_cone_patch(
            &f,
            -1.0,
            1.0,
            0.0,
            90.0,
            1.0,
            NurbCircleParam::Quadratic
        )
        .is_err());
    }

    #[test]
    fn back_edges_are_unported() {
        let k = &Kernel::default();
        let s = bilinear();
        assert!(s.is_kind_of(BSplineSurface::TYPE).unwrap());
        assert!(!s.is_kind_of(PLANE_TYPE).unwrap());
        let req = BoundingBoxRequest {
            normal: true,
            ..BoundingBoxRequest::default()
        };
        assert!(s
            .calculate_bounding_box(k, &s.natural_uv_domain(), &req)
            .is_ok());
        // the polar box uses SmSurface::EvaluateNormal (surface::surface::p7)
        let req = BoundingBoxRequest {
            polar: true,
            ..BoundingBoxRequest::default()
        };
        assert!(s
            .calculate_bounding_box(k, &s.natural_uv_domain(), &req)
            .is_ok());
        assert!(s.is_parallel_to_vector(Vec3::new(0.0, 0.0, 0.0)).unwrap());
    }

    #[test]
    fn read_rejects_oversized_header() {
        // A header declaring ~1e18 control points must fail cleanly, not abort on
        // allocation or overflow `n + p + 1`.
        for text in [
            "1000000000 1000000000\n3 3\n0\n0 0 0\n",
            "9223372036854775807 0\n3 3\n0\n",
        ] {
            let mut db = DatabaseReader::new();
            db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
            assert!(BSplineSurface::read_from_db(&mut db, 30).is_err());
        }
        let mut bin = Vec::new();
        for v in [i64::MAX / 2, 1] {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        let mut db = DatabaseReader::new();
        db.open_for_read(bin, FileType::Binary);
        assert!(BSplineSurface::read_from_db(&mut db, 30).is_err());
    }

    #[test]
    fn read_ascii_db() {
        let text = "1 1\n1 1\n0\n0 0 0\n0 1 0\n1 0 0\n1 1 1\n0\n0\n1\n1\n0\n0\n1\n1\n";
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        let s = BSplineSurface::read_from_db(&mut db, 30).unwrap();
        assert_eq!(s.number_control_points(SurfParamType::Both), 4);
        let (_, _, cps) = s.control_points().unwrap();
        assert_eq!(cps[3].z, 1.0);
        assert_eq!(cps[3].w, NOW);
    }

    #[test]
    fn canonical_huge_multiplicities_do_not_allocate() {
        // fuzz regressions (nurbs.b_spline_surface_p1 fuzz00115, fuzz00156): huge
        // multiplicities aborted (or overflowed) the knot expansion
        let pts: Vec<Point3> = (0..6)
            .map(|k| Point3::new(f64::from(k / 3), f64::from(k % 3), f64::from(k)))
            .collect();
        let data = |u: &'static [usize], v: &'static [usize]| CanonicalSurfaceData {
            u_degree: 1,
            v_degree: 2,
            control_points: &[],
            form: BSplineSurfaceForm::QuadricSurf,
            u_multiplicities: u,
            v_multiplicities: v,
            u_knots: &[0.0, 1.0],
            v_knots: &[0.0, 2.0],
            knot_type: KnotType::PiecewiseBezierKnots,
            weights: None,
            uv_domain: None,
        };
        for (u, v) in [
            (&[2, 1 << 31][..], &[3, 3][..]),
            (&[2, 2][..], &[3, usize::MAX][..]),
            (&[usize::MAX, 2][..], &[3, 3][..]),
            // wrapped point count 2^63 * 2 == 0 with no points: still no allocation
            (&[2, 1 << 63][..], &[3, 2][..]),
        ] {
            let mut d = data(u, v);
            d.control_points = &pts;
            assert!(BSplineSurface::create_canonical(&d).is_err());
            d.control_points = &[];
            assert!(BSplineSurface::create_canonical(&d).is_err());
        }
    }
}

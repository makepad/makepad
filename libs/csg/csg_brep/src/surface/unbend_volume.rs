//! The unbend volume `SmUnbendVolume`: a map that flattens the cylindrical shells
//! about the unbend axis onto planes, like unbending sheet metal (the inverse of the
//! bend volume).
//!
//! Upstream: `source/SMLib/src/SmUnbendVolume.cpp`, `source/SMLib/inc/SmUnbendVolume.h`.
//!
//! The simple map takes the parameter point `(u, v, w)` to `(r, d theta, w)` with
//! `r = sqrt(u^2 + v^2)`, `theta = atan2(v, u)` and `d` the neutral distance. It is
//! singular on the `w` axis and discontinuous across the half plane `v = 0, u < 0`.
//! The orient map places the unbend frame (origin, mid direction, binormal, axis) in
//! in-space.
//!
//! | Rust                                              | Upstream                                       |
//! |---------------------------------------------------|------------------------------------------------|
//! | [`UnbendVolume`]                                  | `SmUnbendVolume` (members `m_dUnbendNeutralDist`, `m_dSingularityTol`) |
//! | [`UnbendVolume::new`], `Default`                  | `SmUnbendVolume(const SmContext*)`             |
//! | [`UnbendVolume::create`]                          | `SmUnbendVolume(crContext, Center, Axis, MidDir, NeutralDist, SingularityTol)` |
//! | [`Volume::copy`] (unbend arm)                     | `SmUnbendVolume(const SmUnbendVolume&, SmBoolean)`, `Copy` |
//! | `Drop` (implicit)                                 | `~SmUnbendVolume`                              |
//! | [`Volume::approx_eq`] (unbend arm), [`UnbendVolume::members_eq`] | `operator==`                    |
//! | [`UnbendVolume::natural_param_domain`]            | `GetNaturalParamDomain`                        |
//! | [`UnbendVolume::legal_min_u`]                     | `GetLegalMinU`                                 |
//! | [`Volume::unbend_origin`], [`Volume::unbend_axis`], [`Volume::unbend_mid_dir`], [`Volume::unbend_binormal`] | `GetUnbendOrigin`, `GetUnbendAxis`, `GetUnbendMidDir`, `GetUnbendBinormal` |
//! | [`UnbendVolume::unbend_neutral_dist`], [`UnbendVolume::singularity_tol`] | `GetUnbendNeutralDist`, `m_dSingularityTol` (`GetSingularityTol`) |
//! | [`UnbendVolume::evaluate`]                        | `Evaluate`                                     |
//! | [`UnbendVolume::evaluate_simple`]                 | `EvaluateSimple`                               |
//! | [`UnbendVolume::evaluate_bounding_box_simple`]    | `EvaluateBoundingBoxSimple`                    |
//! | [`UnbendVolume::trim_param_bounding_box_simple`]  | `TrimParamBoundingBoxSimple`                   |
//! | [`UnbendVolume::inv_evaluate_guess_point_simple`] | `InvEvaluateGuessPointSimple`                  |
//! | [`UnbendVolume::global_point_solve_simple`]       | `GlobalPointSolveSimple`                       |
//! | [`UnbendVolume::local_point_solve_simple`]        | `LocalPointSolveSimple`                        |
//! | [`UnbendVolume::is_bounded_simple`]               | `IsBoundedSimple`                              |
//! | [`UnbendVolume::is_on_boundary_simple`]           | `IsOnBoundarySimple`                           |
//! | [`UnbendVolume::is_point_in_param_domain_simple`] | `IsPointInParamDomainSimple`                   |
//! | [`UnbendVolume::is_line_in_param_domain_simple`]  | `IsLineInParamDomainSimple`                    |
//! | [`UnbendVolume::is_curve_in_param_domain_simple`] | `IsCurveInParamDomainSimple`                   |
//! | [`UnbendVolume::read_from_db`]                    | `ReadFromDB`                                   |
//! | [`UnbendVolume::make_exact_b_spline_1st_out_surface`] | `MakeExactBSpline1stOutSurface` (in no unit in units.json; called by `SrfInVolume::make_exact_b_spline_if_possible_with`) |
//! | [`UnbendVolume::make_exact_b_spline_1st_out_curve`] | `MakeExactBSpline1stOutCurve` (in no unit in units.json; called by `CrvInVolume::make_exact_b_spline_if_possible_with`) |
//! | [`UNBEND_VOLUME_TYPE`], [`is_kind_of`], [`Volume::type_code`] and friends | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`), `IsKindOf` |
//!
//! Deviations:
//! - **Volume base.** The unbend is the [`VolumeKind::Unbend`] variant of a
//!   [`Volume`], which holds the orient and next maps; the virtual overrides are
//!   dispatched from [`Volume`] (`surface::volume::p1`). The methods that read the
//!   frame or map through the whole volume take that volume.
//! - **Constructor.** The component constructor is not in the unit's function list
//!   (nothing else owns it) but is ported as [`UnbendVolume::create`] because it is
//!   the only way upstream builds a usable unbend. Its zero-vector repair compares
//!   `Unitize`'s `SM_ERR_INVALID_INPUT` with `SM_ERR`, so the first two repair
//!   branches never run; ported as it behaves (as for the twist volume). A neutral
//!   distance below the clamped singularity tolerance becomes `1.0`. Contexts and the
//!   `SE_MSG` reports are dropped.
//! - **Default construction.** A default-constructed unbend has neutral distance `0`
//!   and singularity tolerance `SM_UNDEF_DOUBLE` (the member initializer)
//!   ([`UnbendVolume::new`]).
//! - **`operator==`** assigns the neutral-distance comparison and then overwrites it
//!   with the singularity-tolerance comparison; only the latter decides (kept,
//!   [`UnbendVolume::members_eq`]).
//! - **Bounding boxes.** `EvaluateBoundingBoxSimple` maps the twelve edges of the
//!   parameter box through the whole volume (upstream's lines that would make it a
//!   simple map for the duration are commented out), so the boxes are out-space
//!   boxes; kept. An edge of zero length makes `SmLine::CreateLineSegment` fail and
//!   leaves upstream reading a null or already deleted line:
//!   [`SolidError::InvalidInput`]. A failed `CalculateBoundingBox` (status ignored
//!   upstream) leaves the previous edge's boxes in place. The parameter pseudo box is
//!   unused upstream and dropped; the aliasing copies of the inputs are not needed.
//! - **Solvers.** `GlobalPointSolveSimple` checks the *projection* point with
//!   [`Volume::is_point_in_param_domain`] (`SmVolume::IsPointInParamDomain`, unit
//!   `surface.volume_p2`), as upstream. The solvers return upstream's
//!   `rbFoundAnswer` (`true` whenever they succeed); the volume itself
//!   (`m_apObjects[0] = this`) is the caller's object handle. The unused parameter
//!   guess of the local solve is dropped.
//! - **`IsCurveInParamDomainSimple`** builds the half-plane `SmPlane` and asks
//!   `SmCurve::HasPlaneCrossings`, which needs `SmSurface::GlobalCurveIntersect`
//!   (the plane intersector).
//! - **`IsPointInParamDomainSimple`** passes one boolean for all three out-flags of
//!   `IsOnBoundarySimple`, whose last write `rbOnW = FALSE` then makes it return
//!   `FALSE`: every point is in the domain (ported as that constant; it also decides
//!   degenerate segments of `IsLineInParamDomainSimple` and the domain check of
//!   `GlobalPointSolveSimple`).
//! - **`IsLineInParamDomainSimple`** compares the end points' `y` with the
//!   singularity tolerance (not zero) in its discontinuity test; ported as written.
//! - **`ReadFromDB`** reads the two members, then the base data
//!   ([`Volume::read_base_from_db`], status ignored as upstream). Filling a given object (`rpNewVolume != NULL`) is not
//!   modelled; a new volume is always made.
//! - **Unused arguments** upstream ignores (the from-left flags, zero-tangent and
//!   zero-sampling options of the evaluations, the parameter domain of
//!   `IsOnBoundarySimple`) are dropped where the call is local to this module.
//! - **`MakeExactBSpline1stOutCurve`** takes the input space as a flag, as the surface
//!   variant, and returns the new curve (`None` for upstream's `NULL`). The statuses
//!   upstream ignores (the sample `EvaluatePoint`s, the end-point `MapPoint` /
//!   `EvaluatePoint`, `CCWAngleBetween`) leave the previous value
//!   ([`Vec3::UNDEF`] for points, `0` for angles; upstream's angles are then
//!   uninitialised). The unused plane-distance sums and the debug drawing are dropped.
//! - **`MakeExactBSpline1stOutSurface`** takes the input space as a flag
//!   (`in_space`: `SM_VS_IN_SPACE`; otherwise `SM_VS_PARAM_SPACE`, so the invalid-space
//!   `SM_ERR` has no counterpart) and returns the new surface (`None` for upstream's
//!   `NULL`). The statuses upstream ignores (the sample and corner evaluations,
//!   `EvaluateNormal`, `CCWAngleBetween`, `SetMinMax`) leave
//!   the previous value, as upstream's out-params do (the defaults are
//!   [`Vec3::UNDEF`]; the angle starts at `0`); [`SolidError::Unported`] still
//!   propagates. The debug drawing is dropped.
//! - **Recursion:** none here; the next-map compounding recurses through
//!   [`Volume::map`] as in `surface::volume::p1`.

use crate::curve::crv_in_volume::CrvInVolume;
use crate::curve::curve::p2::Curve;
use crate::curve::curve::p4::has_plane_crossings;
use crate::curve::curve::p6::calculate_bounding_box_base;
use crate::curve::line::Line;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::line_line_closest_point;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{Solution, SolutionArray, SolutionObject, SolutionType};
use crate::geom::tol::{
    approx_tol_3d, are_same, is_zero, is_zero_to_tol, SmallTopoType, ZONE_TOL_3D,
};
use crate::geom::transform::Transform;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{
    max2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_RAD, EFF_ZERO_SQ, INFINITE_PARAMETER,
    UNDEF_DOUBLE,
};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::knots::Side;
use crate::surface::plane::p1::Plane;
use crate::surface::surface::p2::Surface;
use crate::surface::volume::p1::{
    concat_evals, is_kind_of as volume_is_kind_of, EvalOptions, Volume, VolumeKind,
};
use crate::surface::volume::p2::ignore_read;
use crate::topology::database_io::{DatabaseReader, FileType};

/// `lSmpSize` of `MakeExactBSpline1stOutSurface`: the sample grid is
/// `(SMP_SIZE + 1) x (SMP_SIZE + 1)`.
const SMP_SIZE: u32 = 5;

/// `lSmpSize` of `MakeExactBSpline1stOutCurve`: the curve is sampled at
/// `CRV_SMP_SIZE + 1` points.
const CRV_SMP_SIZE: u32 = 11;

pub use crate::surface::volume::p1::UNBEND_VOLUME_TYPE;

const OP_EVAL: &str = "surface::unbend_volume::UnbendVolume::evaluate";
const OP_EVAL_SIMPLE: &str = "surface::unbend_volume::UnbendVolume::evaluate_simple";
const OP_BOX: &str = "surface::unbend_volume::UnbendVolume::evaluate_bounding_box_simple";
const OP_GLOBAL: &str = "surface::unbend_volume::UnbendVolume::global_point_solve_simple";

/// The highest derivative the unbend evaluations accept (`GW_MAX_DERIV`); the
/// derivatives above the third are zero.
pub const MAX_DERIV: usize = 5;

/// `SmUnbendVolume::IsKindOf`: `SmUnbendVolume_TYPE` or a base class's type.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == UNBEND_VOLUME_TYPE || volume_is_kind_of(t)
}

/// `(n+1)^3` for `n <= MAX_DERIV`.
const fn eval_size(n: usize) -> usize {
    (n + 1) * (n + 1) * (n + 1)
}

/// Unitizes in place as `SmVector3d::Unitize`: the unit vector (or `(0, 0, 1)` for a
/// zero vector) and whether it succeeded.
fn unitize(v: Vec3) -> (Vec3, bool) {
    match v.unitize() {
        Ok(u) => (u, true),
        Err(_) => (Vec3::Z, false),
    }
}

/// The members of `SmUnbendVolume`; its orient map (the unbend frame) and any next
/// maps live in the [`Volume`] holding it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnbendVolume {
    /// `m_dUnbendNeutralDist`: the distance along the mid direction from the unbend
    /// axis to the neutral plane.
    unbend_neutral_dist: f64,
    /// `m_dSingularityTol`: the smallest distance of a non-singular point from the
    /// singularity at `u = 0`.
    singularity_tol: f64,
}

impl Default for UnbendVolume {
    fn default() -> Self {
        Self::new()
    }
}

impl UnbendVolume {
    /// The default-constructed unbend (`SmUnbendVolume(const SmContext*)`): neutral
    /// distance `0` and singularity tolerance `SM_UNDEF_DOUBLE` (its member
    /// initializer).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            unbend_neutral_dist: 0.0,
            singularity_tol: UNDEF_DOUBLE,
        }
    }

    /// An unbend with the given members as stored, unchecked (the state
    /// `ReadFromDB` builds).
    #[must_use]
    pub const fn from_members(unbend_neutral_dist: f64, singularity_tol: f64) -> Self {
        Self {
            unbend_neutral_dist,
            singularity_tol,
        }
    }

    /// The unbend volume about the in-space line through `center` along `axis`, the
    /// parameter `u` axis along `mid_dir`, neutral plane at `unbend_neutral_dist`
    /// (the component constructor; upstream's default `singularity_tol` is `0.01`).
    /// Bad input is repaired, never rejected: the tolerance is raised to
    /// `SM_ZONE_TOL_3D`, a neutral distance below it becomes `1.0`, zero vectors are
    /// replaced by a frame about the axis and a mid direction not perpendicular to
    /// the axis is projected (or, when parallel, replaced).
    #[must_use]
    pub fn create(
        center: Point3,
        axis: Vec3,
        mid_dir: Vec3,
        unbend_neutral_dist: f64,
        singularity_tol: f64,
    ) -> Volume {
        let mut unbend = Self::from_members(unbend_neutral_dist, singularity_tol);

        // check input - dSingularityTol >= SM_ZONE_TOL_3D
        if unbend.singularity_tol < ZONE_TOL_3D {
            unbend.singularity_tol = ZONE_TOL_3D;
        }

        // check input - Positive Definite UnbendNeutralDistance
        if unbend_neutral_dist < unbend.singularity_tol {
            // do the best we can
            unbend.unbend_neutral_dist = 1.0;
        }

        // unitize Unbend vectors
        let (mut unbend_axis, ok1) = unitize(axis);
        let (mut unbend_mid_dir, ok2) = unitize(mid_dir);

        // check input - nonZero input vectors
        if !ok1 || !ok2 {
            // Upstream compares the failed statuses (SM_ERR_INVALID_INPUT) with SM_ERR,
            // so only its last branch runs: a frame about the (repaired) axis.
            if let Ok([x, y, _]) = unbend_axis.make_unit_ortho_vectors(None) {
                unbend_axis = x;
                unbend_mid_dir = y;
            }
        }

        // check input - Unbend vectors must be perpendicular to one another
        if !unbend_axis.is_perpendicular_to(unbend_mid_dir, EFF_ZERO_DEG) {
            if unbend_axis.is_parallel_to(unbend_mid_dir, EFF_ZERO_DEG) {
                // Axis and StartDir don't span a plane - do the best we can
                if let Ok([x, _, z]) = unbend_mid_dir.make_unit_ortho_vectors(None) {
                    unbend_mid_dir = x;
                    unbend_axis = z;
                }
            } else {
                unbend_mid_dir = unbend_axis.cross(unbend_mid_dir.cross(unbend_axis));
                unbend_mid_dir = unitize(unbend_mid_dir).0;
            }
        }

        // set UnbendBinormal perp to both UnbendAxis and UnbendMidDir
        let unbend_binormal = unbend_axis.cross(unbend_mid_dir);

        // build UnbendCoordinates into m_pOrientMap
        let orient = Transform::new(center, unbend_mid_dir, unbend_binormal, Some(unbend_axis));
        Volume::oriented(VolumeKind::Unbend(unbend), Some(orient))
    }

    /// `m_dUnbendNeutralDist` (`GetUnbendNeutralDist`).
    #[must_use]
    pub const fn unbend_neutral_dist(&self) -> f64 {
        self.unbend_neutral_dist
    }

    /// `m_dSingularityTol` (`GetSingularityTol`).
    #[must_use]
    pub const fn singularity_tol(&self) -> f64 {
        self.singularity_tol
    }

    /// The smallest legal `u` distance from the singular plane `u = 0`:
    /// `singularity_tol - SM_EFF_ZERO` (`GetLegalMinU`).
    #[must_use]
    pub fn legal_min_u(&self) -> f64 {
        self.singularity_tol - EFF_ZERO
    }

    /// The member part of `operator==`: upstream overwrites the neutral-distance
    /// comparison with the tolerance comparison, so only `SM_IS_ZERO` of the
    /// singularity tolerances decides.
    #[must_use]
    pub fn members_eq(&self, other: &Self) -> bool {
        // (upstream's neutral-distance comparison is overwritten before it is read)
        is_zero(self.singularity_tol - other.singularity_tol)
    }

    /// The parameter domain, unbounded in every direction (`GetNaturalParamDomain`).
    #[must_use]
    pub const fn natural_param_domain(&self) -> Extent3d {
        Extent3d::from_bounds(
            -INFINITE_PARAMETER,
            -INFINITE_PARAMETER,
            -INFINITE_PARAMETER,
            INFINITE_PARAMETER,
            INFINITE_PARAMETER,
            INFINITE_PARAMETER,
        )
    }

    /// Out-space position and partial derivatives `0..=n` (`n <= 5`) of the parameter
    /// point, `D[u][v][w]` (`Evaluate`): the unbend evaluated directly in the in-space
    /// frame of `volume`'s orient map, then compounded through its next maps when
    /// `opts.with_compounding`. `volume` is the volume holding this unbend. On the
    /// `w` axis the derivatives divide by zero (NaN / infinite values, as upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) for `n > 5`; the next
    /// maps' failures.
    pub fn evaluate(
        &self,
        volume: &Volume,
        param_point: Point3,
        n: usize,
        opts: &EvalOptions,
    ) -> Result<Vec<Vec3>> {
        // check input - limit the number of derivatives
        if n > MAX_DERIV {
            return Err(SolidError::invalid_input(
                OP_EVAL,
                "highest derivative above GW_MAX_DERIV (5)",
            ));
        }
        // (a point within GetLegalMinU of the w axis is only reported upstream)
        let n1 = n + 1;
        let sv = |u: usize, v: usize, w: usize| (u * n1 + v) * n1 + w;

        // locals
        let dd = self.unbend_neutral_dist;
        let du = param_point.x;
        let dv = param_point.y;
        let dw = param_point.z;

        let dr2 = du * du + dv * dv; // (u**2 + v**2)
        let dr1 = dr2.sqrt(); // (u**2 + v**2)**(1/2)

        let datan = dv.atan2(du);

        let mid = volume.unbend_mid_dir();
        let bin = volume.unbend_binormal();
        let axis = volume.unbend_axis();

        // init output array
        let mut d = vec![Vec3::ZERO; eval_size(n)];
        let mut put = |i: usize, v: Vec3| {
            if let Some(slot) = d.get_mut(i) {
                *slot = v;
            }
        };

        // position
        put(
            0,
            volume.unbend_origin() + dr1 * mid + (dd * datan) * bin + dw * axis,
        );

        // 1st derivs
        if n >= 1 {
            put(sv(1, 0, 0), (du / dr1) * mid - (dd * dv / dr2) * bin);
            put(sv(0, 1, 0), (dv / dr1) * mid + (dd * du / dr2) * bin);
            put(sv(0, 0, 1), axis);

            // NonZero 2nd derivs
            if n >= 2 {
                let dr3 = dr2 * dr1; // (u**2 + v**2)**(3/2)
                let dr4 = dr2 * dr2; // (u**2 + v**2)**(2)
                let duu = du * du;
                let duv = du * dv;
                let dvv = dv * dv;

                put(
                    sv(2, 0, 0),
                    (1.0 / dr1 - duu / dr3) * mid + (dd * 2.0 * duv / dr4) * bin,
                );
                put(
                    sv(1, 1, 0),
                    (-duv / dr3) * mid + (dd * (-1.0 / dr2 + 2.0 * dvv / dr4)) * bin,
                );
                put(
                    sv(0, 2, 0),
                    (1.0 / dr1 - dvv / dr3) * mid + (-dd * 2.0 * duv / dr4) * bin,
                );

                // NonZero 3rd derivs
                if n >= 3 {
                    let dr5 = dr3 * dr2;
                    let dr6 = dr3 * dr3;

                    let duuu = duu * du;
                    let duvv = du * dvv;
                    let duuv = duu * dv;
                    let dvvv = dvv * dv;

                    put(
                        sv(3, 0, 0),
                        (-3.0 * du / dr3 + 3.0 * duuu / dr5) * mid
                            + (dd * (2.0 * dv / dr4 - 8.0 * duuv / dr6)) * bin,
                    );
                    put(
                        sv(2, 1, 0),
                        (-dv / dr3 + 3.0 * duuv / dr5) * mid
                            + (dd * (2.0 * du / dr4 - 8.0 * duvv / dr6)) * bin,
                    );
                    put(
                        sv(1, 2, 0),
                        (-du / dr3 + 3.0 * duvv / dr5) * mid
                            + (dd * (-2.0 * dv / dr4 + 8.0 * duuv / dr6)) * bin,
                    );
                    put(
                        sv(0, 3, 0),
                        (-3.0 * dv / dr3 + 3.0 * dvvv / dr5) * mid
                            + (dd * (-2.0 * du / dr4 + 8.0 * duvv / dr6)) * bin,
                    );
                }
            }
        }

        // Map OutSpace Point through concatenated NextMaps
        match volume.next_map() {
            Some(next) if opts.with_compounding => {
                let position = d.first().copied().unwrap_or(Vec3::UNDEF);
                // Map runs with compounding (its default)
                let map_opts = EvalOptions {
                    with_compounding: true,
                    ..*opts
                };
                let next_derivs = next.map(position, n, &map_opts)?;
                // Concatenate Evaluate(ParamPoint) = Next(Mid(ParamPoint))
                concat_evals(n, &d, &next_derivs)
            }
            _ => Ok(d),
        }
    }

    /// Projection-space position and partial derivatives `0..=n` (`n <= 5`) of the
    /// parameter point, `D[u][v][w]`, without orientation or compounding
    /// (`EvaluateSimple`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) for `n > 5`.
    pub fn evaluate_simple(&self, param_point: Point3, n: usize) -> Result<Vec<Vec3>> {
        // check input
        if n > MAX_DERIV {
            return Err(SolidError::invalid_input(
                OP_EVAL_SIMPLE,
                "highest derivative above GW_MAX_DERIV (5)",
            ));
        }
        let n1 = n + 1;
        let sv = |u: usize, v: usize, w: usize| (u * n1 + v) * n1 + w;

        // locals
        let dd = self.unbend_neutral_dist;
        let du = param_point.x;
        let dv = param_point.y;
        let dw = param_point.z;

        let dr2 = du * du + dv * dv; // (u**2 + v**2)
        let dr1 = dr2.sqrt(); // (u**2 + v**2)**(1/2)

        let datan = dv.atan2(du);

        // init output array
        let mut d = vec![Vec3::ZERO; eval_size(n)];
        let mut put = |i: usize, v: Vec3| {
            if let Some(slot) = d.get_mut(i) {
                *slot = v;
            }
        };

        // position
        put(0, Vec3::new(dr1, dd * datan, dw));

        // 1st Derivs
        if n >= 1 {
            put(sv(1, 0, 0), Vec3::new(du / dr1, -dd * dv / dr2, 0.0));
            put(sv(0, 1, 0), Vec3::new(dv / dr1, dd * du / dr2, 0.0));
            put(sv(0, 0, 1), Vec3::new(0.0, 0.0, 1.0));

            // NonZero 2nd Derivs (Duw, Dvw, Dww are zero)
            if n >= 2 {
                let dr3 = dr2 * dr1; // (u**2 + v**2)**(3/2)
                let dr4 = dr2 * dr2; // (u**2 + v**2)**(2)
                let duu = du * du;
                let duv = du * dv;
                let dvv = dv * dv;

                put(
                    sv(2, 0, 0),
                    Vec3::new(1.0 / dr1 - duu / dr3, dd * 2.0 * duv / dr4, 0.0),
                );
                put(
                    sv(1, 1, 0),
                    Vec3::new(-duv / dr3, dd * (-1.0 / dr2 + 2.0 * dvv / dr4), 0.0),
                );
                put(
                    sv(0, 2, 0),
                    Vec3::new(1.0 / dr1 - dvv / dr3, -dd * 2.0 * duv / dr4, 0.0),
                );

                // NonZero 3rd Derivs (the w ones are zero)
                if n >= 3 {
                    let dr5 = dr3 * dr2;
                    let dr6 = dr3 * dr3;

                    let duuu = duu * du;
                    let duvv = du * dvv;
                    let duuv = duu * dv;
                    let dvvv = dvv * dv;

                    put(
                        sv(3, 0, 0),
                        Vec3::new(
                            -3.0 * du / dr3 + 3.0 * duuu / dr5,
                            dd * (2.0 * dv / dr4 - 8.0 * duuv / dr6),
                            0.0,
                        ),
                    );
                    put(
                        sv(2, 1, 0),
                        Vec3::new(
                            -dv / dr3 + 3.0 * duuv / dr5,
                            dd * (2.0 * du / dr4 - 8.0 * duvv / dr6),
                            0.0,
                        ),
                    );
                    put(
                        sv(1, 2, 0),
                        Vec3::new(
                            -du / dr3 + 3.0 * duvv / dr5,
                            dd * (-2.0 * dv / dr4 + 8.0 * duuv / dr6),
                            0.0,
                        ),
                    );
                    put(
                        sv(0, 3, 0),
                        Vec3::new(
                            -3.0 * dv / dr3 + 3.0 * dvvv / dr5,
                            dd * (-2.0 * du / dr4 + 8.0 * duvv / dr6),
                            0.0,
                        ),
                    );
                }
            }
        }
        Ok(d)
    }

    /// The boxes containing the image of `param_box` (`EvaluateBoundingBoxSimple`),
    /// written into the requested outputs. An unbounded `param_box` gives an
    /// unbounded box and an unbounded pseudo box on the unbend frame. Otherwise each
    /// of the box's twelve edges is mapped through `volume` (the volume holding this
    /// unbend, with its orientation and next maps) as a curve, and the edges'
    /// boxes (`SmCurve::CalculateBoundingBox`, not expanded) are united.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an edge of zero length (upstream then reads a
    /// null or deleted line); [`SolidError::Unported`] from the edge boxes.
    pub fn evaluate_bounding_box_simple(
        &self,
        k: &Kernel,
        volume: &Volume,
        param_box: &Extent3d,
        proj_box: Option<&mut Extent3d>,
        proj_pseudo_box: Option<&mut PseudoBox>,
    ) -> Result<()> {
        // no work - no output
        if proj_box.is_none() && proj_pseudo_box.is_none() {
            return Ok(());
        }
        let mut proj_box = proj_box;
        let mut proj_pseudo_box = proj_pseudo_box;

        // when pBox is unbounded return unbounded BBoxes
        if !param_box.is_bounded() {
            if let Some(b) = proj_box {
                b.set_unbounded();
            }
            if let Some(pb) = proj_pseudo_box {
                let mut ivl = Extent1d::new();
                ivl.set_unbounded();
                // upstream ignores the status
                let _ = pb.set_basis(
                    volume.unbend_mid_dir(),
                    volume.unbend_binormal(),
                    volume.unbend_axis(),
                );
                pb.set_intervals(ivl, ivl, ivl);
            }
            return Ok(());
        }

        // init output
        if let Some(b) = proj_box.as_deref_mut() {
            b.init();
        }
        if let Some(pb) = proj_pseudo_box.as_deref_mut() {
            // init intervals, leave basis vectors alone
            pb.init();
        }

        // locals
        let req = BoundingBoxRequest {
            normal: proj_box.is_some(),
            pseudo: proj_pseudo_box.is_some(),
            polar: false,
        };
        let mut this_bbox = Extent3d::new();
        let mut this_pseudo_box = PseudoBox::new();
        let mut cnt = 0usize;

        // for every box edge (marked by box corner pairs)
        for ii in 0..3 {
            for jj in 0..2 {
                for kk in 0..2 {
                    let (j, edge_k) = (f64::from(jj), f64::from(kk));
                    // get UVWBox corner point pairs bounding edges
                    let (pt_min, pt_max) = match ii {
                        0 => (
                            param_box.evaluate(0.0, j, edge_k),
                            param_box.evaluate(1.0, j, edge_k),
                        ),
                        1 => (
                            param_box.evaluate(j, 0.0, edge_k),
                            param_box.evaluate(j, 1.0, edge_k),
                        ),
                        _ => (
                            param_box.evaluate(j, edge_k, 0.0),
                            param_box.evaluate(j, edge_k, 1.0),
                        ),
                    };

                    // Build line from one UVWDomain corner to the other
                    let line =
                        Line::create_line_segment(3, pt_min, pt_max, None).map_err(|_| {
                            SolidError::invalid_input(
                                OP_BOX,
                                "zero-length box edge (upstream reads a null or deleted line)",
                            )
                        })?;
                    let ivl = line.natural_interval();

                    // map line to ProjSpace (through the whole volume: upstream's
                    // temporary simple-map change is commented out)
                    let map_line = Curve::from(CrvInVolume::new(
                        Curve::from(line),
                        true,
                        volume.clone(),
                        0,
                        1,
                    )?);

                    // Get mapped line's bounding box (status ignored upstream)
                    if let Some(boxes) =
                        ignored(calculate_bounding_box_base(k, &map_line, &ivl, req, false))?
                    {
                        if let Some(b) = boxes.normal {
                            this_bbox = b;
                        }
                        if let Some(pb) = boxes.pseudo {
                            this_pseudo_box = pb;
                        }
                    }

                    // union the line BBoxes into the output
                    if let Some(b) = proj_box.as_deref_mut() {
                        *b = if cnt == 0 {
                            this_bbox
                        } else {
                            b.union(&this_bbox)
                        };
                    }
                    if let Some(pb) = proj_pseudo_box.as_deref_mut() {
                        if cnt == 0 {
                            *pb = this_pseudo_box;
                        } else {
                            let current = *pb;
                            let basis = *current.basis();
                            current.union_into(&this_pseudo_box, &basis, pb);
                        }
                    }
                    cnt += 1;
                }
            }
        }
        Ok(())
    }

    /// The parameter box trimmed to the natural domain: unbounded, so a copy
    /// (`TrimParamBoundingBoxSimple`; upstream keeps boxes that touch the
    /// discontinuities).
    #[must_use]
    pub const fn trim_param_bounding_box_simple(&self, param_box: &Extent3d) -> Extent3d {
        *param_box
    }

    /// The parameter point mapping to `proj_point` (exact: the inverse map
    /// `(x cos(y / d), x sin(y / d), z)`), as the single guess of the Newton solvers
    /// (`InvEvaluateGuessPointSimple`).
    #[must_use]
    pub fn inv_evaluate_guess_point_simple(&self, proj_point: Point3) -> Vec<Point3> {
        vec![self.inverse(proj_point)]
    }

    /// The exact inverse of `proj_point`.
    fn inverse(&self, proj_point: Point3) -> Point3 {
        // locals
        let dr = proj_point.x;
        let dd = self.unbend_neutral_dist;
        let theta = proj_point.y / dd;

        // the inverse map from ProjSpace to ParamSpace
        let du = dr * theta.cos();
        let dv = dr * theta.sin();
        let dw = proj_point.z;
        Vec3::new(du, dv, dw)
    }

    /// Solves for the parameter point mapping to `proj_point`
    /// (`GlobalPointSolveSimple`). `solutions` is resized to one fresh solution;
    /// then, when `proj_point` (as a parameter point) is in `volume`'s compound
    /// parameter domain and in `param_domain` (within `SM_EFF_ZERO`), that solution
    /// is set to the exact inverse with value `0` and object `this` (the volume's
    /// handle). `volume` is the volume holding this unbend. Returns upstream's
    /// `rbFoundAnswer` (`true`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either domain check fails; [`SolidError::Unported`] from the
    /// domain check.
    pub fn global_point_solve_simple<O: SolutionObject>(
        &self,
        volume: &Volume,
        proj_point: Point3,
        solutions: &mut SolutionArray<O>,
        param_domain: Option<&Extent3d>,
        this: O,
    ) -> Result<bool> {
        // init output
        solutions.set_size(1);

        // check - crProjPoint must be in a valid domain
        if !volume.is_point_in_param_domain(proj_point, true)? {
            return Err(SolidError::sm_err(OP_GLOBAL));
        }

        // check - crProjPoint must be in OptParamDomain
        if param_domain.is_some_and(|d| !d.contains_point(proj_point, EFF_ZERO)) {
            return Err(SolidError::sm_err(OP_GLOBAL));
        }

        // inverted ParamPoint
        let p = self.inverse(proj_point);

        // set output
        if let Some(sol) = solutions.get_mut(0) {
            Self::fill_solution(sol, p, this);
        }
        Ok(true)
    }

    /// Solves for the parameter point mapping to `proj_point`; no guess is needed
    /// (`LocalPointSolveSimple`): [`UnbendVolume::global_point_solve_simple`], whose
    /// solution replaces `solution`. Returns upstream's `rbFoundAnswer`.
    ///
    /// # Errors
    ///
    /// Those of [`UnbendVolume::global_point_solve_simple`] (`solution` unchanged).
    pub fn local_point_solve_simple<O: SolutionObject>(
        &self,
        volume: &Volume,
        proj_point: Point3,
        solution: &mut Solution<O>,
        param_domain: Option<&Extent3d>,
        this: O,
    ) -> Result<bool> {
        let mut solutions = SolutionArray::new();

        // pass the call along - no need for the guess point
        let found =
            self.global_point_solve_simple(volume, proj_point, &mut solutions, param_domain, this)?;

        // set output
        if let Some(sol) = solutions.get(0) {
            *solution = *sol;
        }
        Ok(found)
    }

    /// Sets `solution` to the single-value solution at the parameter point `p` of
    /// object `this`.
    fn fill_solution<O: SolutionObject>(solution: &mut Solution<O>, p: Point3, this: O) {
        solution.solution_type = SolutionType::SingleValue;
        solution.num_variables = 3;
        solution.start.solution_value = 0.0;
        solution.start.parameters[0] = p.x;
        solution.start.parameters[1] = p.y;
        solution.start.parameters[2] = p.z;
        solution.num_objects = 1;
        solution.objects[0] = Some(this);
    }

    /// The unbend's parameter space is unbounded (`IsBoundedSimple`).
    #[must_use]
    pub const fn is_bounded_simple(&self) -> bool {
        false
    }

    /// Whether the parameter point is on the singular `w` axis or on the
    /// discontinuity half plane `v = 0, u < 0`, within `tolerance` (at least the
    /// singularity tolerance; that tolerance when `None`) (`IsOnBoundarySimple`).
    /// Returns the answer and the flags `[on_u, on_v, on_w]` (`on_w` is always
    /// `false`).
    #[must_use]
    pub fn is_on_boundary_simple(
        &self,
        param_point: Point3,
        tolerance: Option<f64>,
    ) -> (bool, [bool; 3]) {
        // locals
        let du = param_point.x;
        let dv = param_point.y;
        let tol = match tolerance {
            Some(t) => max2(t, self.singularity_tol),
            None => self.singularity_tol,
        };

        // set output
        let on_u = (du * du + dv * dv) <= (tol * tol); // on r=0 boundary also onWAxis
        let on_v = (du < tol) && is_zero_to_tol(dv, tol); // on theta=[-pi pi] boundaries
        (on_u || on_v, [on_u, on_v, false])
    }

    /// Whether the parameter point is in the domain (`IsPointInParamDomainSimple`):
    /// always. Upstream returns `!IsOnBoundarySimple(P, bB, bB, bB)` with one
    /// boolean for all three out-flags, so the final `rbOnW = FALSE` clears it and
    /// `IsOnBoundarySimple` returns `FALSE` whatever the point
    /// (`plan/findings.md`).
    #[must_use]
    pub const fn is_point_in_param_domain_simple(&self) -> bool {
        // !(rbOnU || rbOnV) read after the aliased rbOnW = FALSE
        true
    }

    /// Whether the parameter segment `start`-`end` stays off the discontinuity half
    /// plane and keeps more than the singularity tolerance from the `w` axis
    /// (`IsLineInParamDomainSimple`). A degenerate segment is tested as its start
    /// point.
    #[must_use]
    pub fn is_line_in_param_domain_simple(&self, start: Point3, end: Point3) -> bool {
        // return
        let mut rtn = true;

        // low work - degenerate line
        if start.distance_squared(end) <= EFF_ZERO_SQ {
            // pass the call along
            return self.is_point_in_param_domain_simple();
        }

        // locals (upstream compares y with the tolerance, not with zero)
        let tol = self.singularity_tol;
        let start_discontinuity = start.x <= tol && are_same(start.y, tol);
        let end_discontinuity = end.x <= tol && are_same(end.y, tol);

        // Does line cross the Y=0 plane
        if !start_discontinuity && !end_discontinuity && start.y * end.y < 0.0 {
            // check x value at y = 0 crossing
            let x = start.x + (end.x - start.x) * (-start.y) / (end.y - start.y);

            // line is still 'in' if the crossing happens in the x > 0 half plane
            rtn &= x >= tol - EFF_ZERO;
        }

        // lines intersecting the z axis are not in the domain
        if rtn {
            let z_pt = Vec3::new(0.0, 0.0, 0.0);
            let z_vec = Vec3::new(0.0, 0.0, 1.0);
            let line_vec = end - start;

            // find closest point between two lines - fails when lines are parallel
            // or coincident or vecs are degenerate
            match line_line_closest_point(z_pt, z_vec, start, line_vec) {
                Err(_) => {
                    // When start is close to the ZAxis the parallel or degenerate
                    // ParamLine crosses the singularity
                    if (start.x * start.x + start.y * start.y) <= tol * tol {
                        rtn = false;
                    }
                }
                Ok((z_param, line_param)) => {
                    // when the gap is smaller than the tolerance the ParamLine crosses
                    // the singularity
                    let min_gap = start + line_param * line_vec - z_param * z_vec;
                    if min_gap.length_squared() <= (tol - EFF_ZERO) * (tol - EFF_ZERO) {
                        rtn = false;
                    }
                }
            }
        }
        rtn
    }

    /// Whether the parameter curve crosses no discontinuity
    /// (`IsCurveInParamDomainSimple`): the curve must not cross the half plane
    /// `v = 0, u < tolerance` (approximated with `SM_BIG_DOUBLE` limits).
    ///
    /// # Errors
    ///
    /// Those of `SmCurve::HasPlaneCrossings` (the plane intersector); unported callees
    /// of it return [`SolidError::Unported`].
    pub fn is_curve_in_param_domain_simple(&self, k: &Kernel, param_curve: &Curve) -> Result<bool> {
        // locals
        let uv_scale = Vec2::new(1.0, 1.0);
        let su = Vec3::new(1.0, 0.0, 0.0);
        let sw = Vec3::new(0.0, 0.0, 1.0);
        let plane_point = Vec3::new(0.0, 0.0, 0.0);

        // set 1/2 plane valid intervals - infinite 1/2 planes are approximated by
        // +/- SM_BIG_DOUBLE
        let u_ivl = Extent1d::from_min_max(self.singularity_tol, -BIG_DOUBLE);
        let w_ivl = Extent1d::from_min_max(-BIG_DOUBLE, BIG_DOUBLE);
        let plane_domain =
            Extent2d::from_bounds(w_ivl.min(), u_ivl.min(), w_ivl.max(), u_ivl.max());

        // make and store 1/2 infinite discontinuity plane
        let planes = [Plane::new(plane_point, sw, su, uv_scale, &plane_domain)];

        // check for curve crossing discontinuity planes
        let has_plane_crossings = has_plane_crossings(k, param_curve, &planes)?;
        Ok(!has_plane_crossings)
    }

    /// `MakeExactBSpline1stOutSurface`: the exact first out-space image of `input`
    /// (from in-space when `in_space`, else from parameter space) through the unbend
    /// `volume` (whose kind is `self`), or `None` when it is not one of the analytic
    /// cases. A cylinder about the unbend axis maps to a plane normal to the mid
    /// direction, a plane through the axis to a plane normal to the binormal, and a
    /// plane normal to the axis to a plane normal to the axis. The test samples a
    /// 6 x 6 grid of the natural domain; any sample outside the unbend's domain gives
    /// `None`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the surface or volume services.
    pub fn make_exact_b_spline_1st_out_surface(
        &self,
        k: &Kernel,
        volume: &Volume,
        in_space: bool,
        input: &Surface,
    ) -> Result<Option<Surface>> {
        // locals: ParamSpace and InSpace Unbend orientations - Unbend's Neutral plane is
        // perp to the Unbend's MidDirection
        let (param_origin, unbend_origin) = (Vec3::ZERO, volume.unbend_origin());
        let (param_mid_dir, unbend_mid_dir) = (Vec3::X, volume.unbend_mid_dir());
        let (param_axis, unbend_axis) = (Vec3::Z, volume.unbend_axis());
        let (param_binormal, unbend_binormal) = (Vec3::Y, volume.unbend_binormal());
        let unbend_neutral_dist = self.unbend_neutral_dist();

        // locals: making InputSpace either ParamSpace or InSpace
        let (input_origin, input_mid_dir, input_axis, input_binormal) = if in_space {
            (unbend_origin, unbend_mid_dir, unbend_axis, unbend_binormal)
        } else {
            (param_origin, param_mid_dir, param_axis, param_binormal)
        };

        // locals
        let mut input_pt = Vec3::UNDEF;
        let mut out_space_pt = Vec3::UNDEF;
        let mut on_axis_pt = Vec3::UNDEF;

        let approx_tol = approx_tol_3d(k, SmallTopoType::NormalSize);
        let domain = input.natural_uv_domain_with_nurb(k)?;
        let ivl_u = domain.u_interval();
        let ivl_v = domain.v_interval();

        // distances from the sample points to the planes perpendicular to the unbend
        // axes, their ranges and sums
        let mut mid_dir_ang_rad = 0.0;
        let mut axis_plane_range = Extent1d::new();
        let mut mid_dir_plane_range = Extent1d::new();
        let mut binormal_plane_range = Extent1d::new();
        let mut axis_line_range = Extent1d::new();
        let mut mid_dir_ang_rad_range = Extent1d::new();
        let mut axis_plane_sum = 0.0;
        let mut mid_dir_plane_sum = 0.0;
        let mut binormal_plane_sum = 0.0;
        let mut axis_line_sum = 0.0;
        let mut mid_dir_ang_rad_sum = 0.0;
        let mut const_axis_plane = false;
        let mut const_axis_line = false;
        let mut const_mid_dir_ang_rad = false;
        let mut out_space_bbox = PseudoBox::from_basis_intervals(
            [unbend_mid_dir, unbend_binormal, unbend_axis],
            [mid_dir_plane_range, binormal_plane_range, axis_plane_range],
        );

        // See if the InputSurface is perpendicular to any of the Unbend principle axes
        // (parallel to any Unbend principle planes)
        for ii in 0..=SMP_SIZE {
            let u = ivl_u.evaluate(f64::from(ii) / f64::from(SMP_SIZE));
            for jj in 0..=SMP_SIZE {
                let uv = Point2::new(u, ivl_v.evaluate(f64::from(jj) / f64::from(SMP_SIZE)));

                // Surface sample (status ignored)
                if let Some(p) = ignored(input.evaluate_point(k, uv))? {
                    input_pt = p;
                }

                // set one OnAxisOrigin
                if ii == 0 && jj == 0 {
                    on_axis_pt =
                        input_origin + (input_pt - input_origin).dot(input_axis) * input_axis;
                }

                // inputVec measured from on_axis_pt
                let on_axis_vec = input_pt - on_axis_pt;

                // save OutSpace Extents of the corner points
                if (ii == 0 || ii == SMP_SIZE) && (jj == 0 || jj == SMP_SIZE) {
                    // without compounding (status ignored)
                    let mapped = if in_space {
                        volume.map_point(input_pt, false)
                    } else {
                        volume.evaluate_point(input_pt, false)
                    };
                    if let Some(p) = ignored(mapped)? {
                        out_space_pt = p;
                    }
                    out_space_bbox.add_point(out_space_pt - on_axis_pt);
                }

                // no work - sample point not within bend domain
                let is_in = if in_space {
                    volume.is_point_in_in_space_domain(input_pt, true)?.0
                } else {
                    volume.is_point_in_param_domain(input_pt, true)?
                };
                if !is_in {
                    return Ok(None);
                }

                // distances to the planes through the on-axis point, the axis line and the
                // rotation angle from the mid direction
                let mid_dir_plane = unbend_mid_dir.dot(on_axis_vec);
                let binormal_plane = unbend_binormal.dot(on_axis_vec);
                let axis_plane = unbend_axis.dot(on_axis_vec);
                let axis_line =
                    (mid_dir_plane * mid_dir_plane + binormal_plane * binormal_plane).sqrt();
                if let Some(a) = ignored(input_axis.ccw_angle_between(input_mid_dir, on_axis_vec))?
                {
                    mid_dir_ang_rad = a;
                }

                // accumulate the ranges
                mid_dir_plane_range.add_value(mid_dir_plane);
                axis_plane_range.add_value(axis_plane);
                binormal_plane_range.add_value(binormal_plane);
                axis_line_range.add_value(axis_line);
                mid_dir_ang_rad_range.add_value(mid_dir_ang_rad);

                // accumulate the sums of plane distances
                axis_plane_sum += axis_plane;
                mid_dir_plane_sum += mid_dir_plane;
                binormal_plane_sum += binormal_plane;
                axis_line_sum += axis_line;
                mid_dir_ang_rad_sum += mid_dir_ang_rad;

                // remember when the surface is one of the special shapes
                const_axis_plane = axis_plane_range.length() < approx_tol;
                const_axis_line = axis_line_range.length() < approx_tol;
                const_mid_dir_ang_rad = mid_dir_ang_rad_range.length() < EFF_ZERO_RAD;

                // no work - Surface not one of the special case surfaces
                if !const_axis_line && !const_mid_dir_ang_rad && !const_axis_plane {
                    return Ok(None);
                }
            }
        }
        // the sums upstream accumulates but does not use
        let _ = (axis_plane_sum, mid_dir_plane_sum, binormal_plane_sum);
        let _ = (axis_line_sum, mid_dir_ang_rad_sum);

        // find the current input surface normal - it's a plane or a cylinder so sample it
        // once anywhere (statuses ignored)
        let mid_uv = Point2::new(ivl_u.evaluate(0.5), ivl_v.evaluate(0.5));
        let input_surface_pt = ignored(input.evaluate_point(k, mid_uv))?.unwrap_or(Vec3::UNDEF);
        let input_surface_normal =
            ignored(input.evaluate_normal(k, mid_uv, Side::Left, Side::Left))?
                .unwrap_or(Vec3::UNDEF);

        // create NewSurface locals
        let mut positive = true;
        let uv_scale = Vec2::new(1.0, 1.0);
        let mut x_vector = Vec3::UNDEF;
        let mut y_vector = Vec3::UNDEF;
        let mut uv_domain = Extent2d::new();
        let mut on_axis_offset = Vec3::UNDEF;
        let [_, box_v, box_w] = *out_space_bbox.intervals();

        // InputSpace cylinder centered on the Unbend Axis map to OutSpace planes perp to
        // InSpace Unbend MidBend axis
        if const_axis_line {
            // check orientation of surface (positive = SrfNormals pt outwards from UnbendAxis)
            positive = (input_surface_pt - on_axis_pt).dot(input_surface_normal) > 0.0;

            // offset from the OnAxisPt = Radius * UnbendMidDir
            on_axis_offset = axis_line_range.mid() * unbend_mid_dir;

            // UVDomain = BBox3d U (UnbendMidDir) and V (UnbendAxis) extents in OutSpace
            uv_domain.set_u_interval(&box_v);
            uv_domain.set_v_interval(&box_w);

            // NewPlane Axes OutSpace
            x_vector = unbend_binormal;
            y_vector = unbend_axis;
        }

        // InputSpace planes containing Outspace Axis (radiating radially) to Outspace planes
        // perp to InSpace Unbend Binormal axis
        if const_mid_dir_ang_rad {
            // check orientation of surface (positive = SrfNormals pt in CCW direction about
            // UnbendAxis
            let inspace_ang_rad = mid_dir_ang_rad_range.mid();
            positive = (inspace_ang_rad.cos() * input_binormal
                - inspace_ang_rad.sin() * input_mid_dir)
                .dot(input_surface_normal)
                > 0.0;

            // offset from the OnAxisPt = InspaceAngRad * NeutralDist * UnbendBinormal
            on_axis_offset = (inspace_ang_rad * unbend_neutral_dist) * unbend_binormal;

            // UVDomain extents in OutSpace
            uv_domain.set_u_interval(&axis_line_range);
            uv_domain.set_v_interval(&box_w);

            // NewPlane Axes OutSpace
            x_vector = unbend_mid_dir;
            y_vector = unbend_axis;
        }

        // InputSpace planes perp Unbend Axis to OutSpace planes perp to InSpace Unbend
        // BendAxis axis
        if const_axis_plane {
            // check orientation of surface
            positive = unbend_axis.dot(input_surface_normal) > 0.0;

            // offset from the OnAxisPt = (0,0,0)
            on_axis_offset = Vec3::ZERO;

            // UVDomain = BBox3d U (AxisLineRange) and V (MidDirAngRange*NeutralDist) extents
            // in OutSpace, the radius limits bumped (status ignored)
            let singularity_tol = self.singularity_tol();
            let min_x = if (axis_line_range.min() * 0.8) > (singularity_tol * 1.1) {
                axis_line_range.min() * 0.8
            } else {
                singularity_tol * 1.1
            };
            let _ = uv_domain.set_min_max_coords(
                min_x,
                mid_dir_ang_rad_range.min() * unbend_neutral_dist,
                axis_line_range.max() * 1.2,
                mid_dir_ang_rad_range.max() * unbend_neutral_dist,
            );

            // NewPlane Axes OutSpace
            x_vector = unbend_mid_dir;
            y_vector = unbend_binormal;
        }

        // when positive == FALSE, swap Plane normal
        if !positive {
            core::mem::swap(&mut x_vector, &mut y_vector);
            uv_domain.transpose();
        }

        // create new plane - positive orientation:[SrfNormal = UnbendMidDir X UnbendBinormal]
        Ok(Some(Surface::from(Plane::new(
            on_axis_pt + on_axis_offset,
            x_vector,
            y_vector,
            uv_scale,
            &uv_domain,
        ))))
    }

    /// `MakeExactBSpline1stOutCurve`: the exact first out-space image of `input` (from
    /// in-space when `in_space`, else from parameter space) through the unbend `volume`
    /// (whose kind is `self`), or `None` when it is not one of the analytic cases. A
    /// line parallel to the unbend axis, a circle centred on the axis (in a plane
    /// normal to it) and a line radiating from the axis each map to the line through
    /// the images of its ends. The test samples 12 points of the natural interval; any
    /// sample outside the unbend's domain gives `None`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the curve or volume services.
    pub fn make_exact_b_spline_1st_out_curve(
        &self,
        k: &Kernel,
        volume: &Volume,
        in_space: bool,
        input: &Curve,
    ) -> Result<Option<Curve>> {
        // locals: ParamSpace and InSpace Unbend orientations - Unbend's Neutral plane is
        // perp to the Unbend's MidDirection
        let (param_origin, unbend_origin) = (Vec3::ZERO, volume.unbend_origin());
        let (param_mid_dir, unbend_mid_dir) = (Vec3::X, volume.unbend_mid_dir());
        let (param_axis, unbend_axis) = (Vec3::Z, volume.unbend_axis());
        let (param_binormal, unbend_binormal) = (Vec3::Y, volume.unbend_binormal());

        // locals: making InputSpace either ParamSpace or InSpace
        let (input_origin, input_mid_dir, input_axis, input_binormal) = if in_space {
            (unbend_origin, unbend_mid_dir, unbend_axis, unbend_binormal)
        } else {
            (param_origin, param_mid_dir, param_axis, param_binormal)
        };

        // locals
        let mut input_first = Vec3::UNDEF;
        let mut input_last = Vec3::UNDEF;
        let mut out_space_first = Vec3::UNDEF;
        let mut out_space_last = Vec3::UNDEF;

        let approx_tol = approx_tol_3d(k, SmallTopoType::NormalSize);
        let ivl = input.natural_interval();
        let mut input_pt = Vec3::UNDEF;
        let mut on_axis_pt = Vec3::UNDEF;

        let mut mid_dir_ang_rad = 0.0;
        let mut axis_plane_range = Extent1d::new();
        let mut mid_dir_plane_range = Extent1d::new();
        let mut binormal_plane_range = Extent1d::new();
        let mut axis_line_range = Extent1d::new();
        let mut mid_dir_ang_rad_range = Extent1d::new();
        let mut const_axis_plane = false;
        let mut const_mid_dir_plane = false;
        let mut const_binormal_plane = false;
        let mut const_axis_line = false;
        let mut const_mid_dir_ang_rad = false;

        // See if the InputCurve is linear and parallel to one of the UnbendAxes
        // (parallel to two UnbendPlanes)
        for ii in 0..=CRV_SMP_SIZE {
            let smp_u = ivl.evaluate(f64::from(ii) / f64::from(CRV_SMP_SIZE));

            // Curve sample (status ignored)
            if let Some(p) = ignored(input.evaluate_point(k, smp_u))? {
                input_pt = p;
            }

            // no work - sample point not within bend domain
            let is_in = if in_space {
                volume.is_point_in_in_space_domain(input_pt, true)?.0
            } else {
                volume.is_point_in_param_domain(input_pt, true)?
            };
            if !is_in {
                return Ok(None);
            }

            // save 1st and last sample pts and pick a point on Axis near line to keep
            // vector sizes small
            if ii == 0 {
                input_first = input_pt;
                let to_orig_vec = input_pt - input_origin;
                on_axis_pt = input_origin + input_axis.dot(to_orig_vec) * input_axis;
            } else if ii == CRV_SMP_SIZE {
                input_last = input_pt;
            }

            // Input vec from SamplePt to OnAxisPt
            let on_axis_vec = input_pt - on_axis_pt;

            // InputSpace distances to planes perp to Unbend axes
            let axis_plane = input_axis.dot(on_axis_vec);
            let mid_dir_plane = input_mid_dir.dot(on_axis_vec);
            let binormal_plane = input_binormal.dot(on_axis_vec);
            let axis_line =
                (mid_dir_plane * mid_dir_plane + binormal_plane * binormal_plane).sqrt();
            // rot angle from sMidDirAxis (status ignored: a failure leaves the previous one)
            if let Some(a) = ignored(input_axis.ccw_angle_between(input_mid_dir, on_axis_vec))? {
                mid_dir_ang_rad = a;
            }

            // accumulate the range of Plane distances
            axis_plane_range.add_value(axis_plane);
            mid_dir_plane_range.add_value(mid_dir_plane);
            binormal_plane_range.add_value(binormal_plane);
            axis_line_range.add_value(axis_line);
            mid_dir_ang_rad_range.add_value(mid_dir_ang_rad);

            // remember when curve is const dist from bend plane
            const_axis_plane = axis_plane_range.length() < approx_tol;
            const_mid_dir_plane = mid_dir_plane_range.length() < approx_tol;
            const_binormal_plane = binormal_plane_range.length() < approx_tol;
            const_axis_line = axis_line_range.length() < approx_tol;
            const_mid_dir_ang_rad = mid_dir_ang_rad_range.length() < EFF_ZERO_RAD;

            // no work - curve not one of the special case curves
            let parallel_to_axis = const_mid_dir_plane && const_binormal_plane; // lines parallel to the Bend Axis
            let centered_on_axis = const_axis_line && const_axis_plane; // circles centered on the Bend Axis
            let radiating = const_mid_dir_ang_rad && const_axis_plane; // lines radiating radially from the Bend Axis
            if !parallel_to_axis && !centered_on_axis && !radiating {
                return Ok(None);
            }
        }

        // OutSpace 1st and Last points, without compounding (statuses ignored)
        let out_point = |p: Point3| {
            if in_space {
                volume.map_point(p, false)
            } else {
                volume.evaluate_point(p, false)
            }
        };
        if let Some(p) = ignored(out_point(input_first))? {
            out_space_first = p;
        }
        if let Some(p) = ignored(out_point(input_last))? {
            out_space_last = p;
        }

        // lines parallel to the Bend Axis             => map to OutSpace lines
        // circles centered on the Bend Axis           => map to OutSpace lines
        // lines radiating radially from the Bend Axis => map to OutSpace lines
        let mut new_curve = None;
        let parallel_to_axis = const_mid_dir_plane && const_binormal_plane;
        let centered_on_axis = const_axis_line && const_axis_plane;
        let radiating = const_mid_dir_ang_rad && const_axis_plane;
        if parallel_to_axis || centered_on_axis || radiating {
            // create the OutSpace Line
            new_curve = Some(Curve::from(Line::from_points(
                out_space_first,
                out_space_last,
                3,
            )));
        }

        // all done
        Ok(new_curve)
    }

    /// Reads an unbend volume from a database stream: the neutral distance and the
    /// singularity tolerance, then the volume base data
    /// ([`Volume::read_base_from_db`], status ignored as upstream).
    ///
    /// # Errors
    ///
    /// A failed binary read; the base read's undefined cases
    /// ([`SolidError::Unsupported`]) and [`SolidError::Unported`].
    pub fn read_from_db(db: &mut DatabaseReader, version: u64) -> Result<Volume> {
        // a new SmUnbendVolume() (neutral distance 0, tolerance SM_UNDEF_DOUBLE)
        let mut unbend = Self::new();
        if db.file_type() == FileType::Ascii {
            for slot in [&mut unbend.unbend_neutral_dist, &mut unbend.singularity_tol] {
                if let Some(stream) = db.in_stream_mut() {
                    if let Some(v) = stream.extract_double() {
                        *slot = v;
                    }
                }
                db.go_to_next_line();
            }
        } else {
            unbend.unbend_neutral_dist = db.read_double()?;
            unbend.singularity_tol = db.read_double()?;
        }
        let mut volume = Volume::new(VolumeKind::Unbend(unbend));
        // read base class data (status ignored)
        ignore_read(volume.read_base_from_db(db, version))?;
        Ok(volume)
    }
}

impl Volume {
    /// The in-space point on the unbend axis at `w = 0`: the orient origin
    /// (`GetUnbendOrigin`).
    #[must_use]
    pub fn unbend_origin(&self) -> Vec3 {
        self.orient_origin()
    }

    /// The in-space unbend axis, the parameter `w` direction: the orient z axis
    /// (`GetUnbendAxis`).
    #[must_use]
    pub fn unbend_axis(&self) -> Vec3 {
        self.orient_z_axis()
    }

    /// The in-space mid direction, the parameter `u` direction: the orient x axis
    /// (`GetUnbendMidDir`).
    #[must_use]
    pub fn unbend_mid_dir(&self) -> Vec3 {
        self.orient_x_axis()
    }

    /// The in-space binormal `axis × mid_dir`, the parameter `v` direction: the
    /// orient y axis (`GetUnbendBinormal`).
    #[must_use]
    pub fn unbend_binormal(&self) -> Vec3 {
        self.orient_y_axis()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::solution_array::ObjectKind;
    use crate::math::PI;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Vol;

    impl SolutionObject for Vol {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Volume
        }
    }

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-12
    }

    fn unbend(d: f64) -> UnbendVolume {
        UnbendVolume::from_members(d, 0.01)
    }

    #[test]
    fn simple_map_unrolls_arcs() {
        let u = unbend(2.0);
        // a quarter turn at radius 3
        let d = u.evaluate_simple(v(0.0, 3.0, 1.0), 0).unwrap();
        assert!(close(d[0], v(3.0, PI, 1.0)));
        let inv = u.inv_evaluate_guess_point_simple(d[0]);
        assert!(close(inv[0], v(0.0, 3.0, 1.0)));
        assert_eq!(u.evaluate_simple(v(1.0, 0.0, 0.0), 5).unwrap().len(), 216);
        assert!(u.evaluate_simple(v(1.0, 0.0, 0.0), 6).is_err());
    }

    #[test]
    fn simple_derivatives_match_differences() {
        let u = unbend(1.5);
        let p = v(0.7, -0.4, 0.9);
        let d = u.evaluate_simple(p, 3).unwrap();
        let h = 1e-6;
        let pos = |q: Vec3| u.evaluate_simple(q, 0).unwrap()[0];
        let du = (pos(p + v(h, 0.0, 0.0)) - pos(p - v(h, 0.0, 0.0))) / (2.0 * h);
        assert!((d[16] - du).length() < 1e-6);
        let dv = (pos(p + v(0.0, h, 0.0)) - pos(p - v(0.0, h, 0.0))) / (2.0 * h);
        assert!((d[4] - dv).length() < 1e-6);
        // Dv at n = 1 is index 2; its u derivative is Duv
        let first = |q: Vec3| u.evaluate_simple(q, 1).unwrap()[2];
        let duv = (first(p + v(h, 0.0, 0.0)) - first(p - v(h, 0.0, 0.0))) / (2.0 * h);
        assert!((d[20] - duv).length() < 1e-5);
    }

    #[test]
    fn create_repairs_input_and_evaluates_in_frame() {
        let vol = UnbendVolume::create(
            v(1.0, 2.0, 3.0),
            v(0.0, 0.0, 2.0),
            v(3.0, 0.0, 0.0),
            2.0,
            0.0,
        );
        let VolumeKind::Unbend(u) = vol.kind() else {
            panic!("not an unbend")
        };
        assert_eq!(u.singularity_tol(), ZONE_TOL_3D);
        assert_eq!(u.unbend_neutral_dist(), 2.0);
        assert!(close(vol.unbend_axis(), v(0.0, 0.0, 1.0)));
        assert!(close(vol.unbend_mid_dir(), v(1.0, 0.0, 0.0)));
        assert!(close(vol.unbend_binormal(), v(0.0, 1.0, 0.0)));
        assert!(close(vol.unbend_origin(), v(1.0, 2.0, 3.0)));
        let d = u
            .evaluate(&vol, v(0.0, 1.0, 2.0), 3, &EvalOptions::default())
            .unwrap();
        assert!(close(d[0], v(2.0, 2.0 + PI, 5.0)));
        assert_eq!(d.len(), 64);
        // the frame form agrees with the simple map through the orient map
        let s = vol
            .evaluate_simple(v(0.0, 1.0, 2.0), 0, &EvalOptions::default())
            .unwrap();
        assert!(close(s[0] + v(1.0, 2.0, 3.0), d[0]));

        // a tiny neutral distance becomes 1, a parallel mid direction is replaced
        let vol = UnbendVolume::create(
            v(0.0, 0.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(0.0, 0.0, 5.0),
            0.0,
            0.5,
        );
        let VolumeKind::Unbend(u) = vol.kind() else {
            panic!("not an unbend")
        };
        assert_eq!(u.unbend_neutral_dist(), 1.0);
        assert_eq!(u.singularity_tol(), 0.5);
        assert!(vol.unbend_axis().dot(vol.unbend_mid_dir()).abs() < 1e-12);
    }

    #[test]
    fn boundaries_and_lines() {
        let u = unbend(1.0);
        assert!(u.is_on_boundary_simple(v(0.0, 0.0, 5.0), None).0);
        assert_eq!(
            u.is_on_boundary_simple(v(-2.0, 0.0, 0.0), None),
            (true, [false, true, false])
        );
        assert!(!u.is_on_boundary_simple(v(2.0, 0.0, 0.0), None).0);
        assert!(u.is_on_boundary_simple(v(0.05, 0.0, 0.0), Some(0.1)).0);
        // the aliased out-flags make every point "in"
        assert!(u.is_point_in_param_domain_simple());
        // crossing the discontinuity half plane
        assert!(!u.is_line_in_param_domain_simple(v(-1.0, -1.0, 0.0), v(-1.0, 1.0, 0.0)));
        // crossing y = 0 at positive x
        assert!(u.is_line_in_param_domain_simple(v(1.0, -1.0, 0.0), v(1.0, 1.0, 0.0)));
        // through the axis
        assert!(!u.is_line_in_param_domain_simple(v(0.0, -1.0, 0.0), v(0.0, 1.0, 3.0)));
        // parallel to the axis, near it
        assert!(!u.is_line_in_param_domain_simple(v(0.001, 0.001, 0.0), v(0.001, 0.001, 2.0)));
        assert!(u.is_line_in_param_domain_simple(v(1.0, 1.0, 0.0), v(1.0, 1.0, 2.0)));
        // degenerate (even on the axis: the point test is always true)
        assert!(u.is_line_in_param_domain_simple(v(1.0, 1.0, 0.0), v(1.0, 1.0, 0.0)));
        assert!(u.is_line_in_param_domain_simple(Vec3::ZERO, Vec3::ZERO));
        // the plane intersector runs (its unported callees defer)
        let r = u.is_curve_in_param_domain_simple(
            &Kernel::default(),
            &Curve::from(Line::from_points(v(1.0, 1.0, 0.0), v(2.0, 1.0, 0.0), 3)),
        );
        assert!(r.is_ok() || matches!(r, Err(SolidError::Unported { .. })));
    }

    #[test]
    fn solvers() {
        let vol = UnbendVolume::create(Vec3::ZERO, Vec3::Z, Vec3::X, 2.0, 0.01);
        let VolumeKind::Unbend(u) = vol.kind() else {
            panic!("not an unbend")
        };
        let mut sols = SolutionArray::new();
        assert!(u
            .global_point_solve_simple(&vol, v(3.0, PI, 0.5), &mut sols, None, Vol)
            .unwrap());
        let s = sols.get(0).unwrap();
        assert_eq!(s.solution_type, SolutionType::SingleValue);
        assert!((s.start.parameters[1] - 3.0).abs() < 1e-12);
        // the projection point on the discontinuity passes the (always true) check
        assert!(u
            .global_point_solve_simple(&vol, v(-1.0, 0.0, 0.0), &mut sols, None, Vol)
            .unwrap());
        assert_eq!(sols.len(), 1);
        let dom = Extent3d::from_bounds(0.0, 0.0, 0.0, 1.0, 1.0, 1.0);
        assert!(u
            .global_point_solve_simple(&vol, v(3.0, 0.5, 0.5), &mut sols, Some(&dom), Vol)
            .is_err());
        let mut sol = Solution::new();
        assert!(u
            .local_point_solve_simple(&vol, v(0.5, 0.5, 0.5), &mut sol, Some(&dom), Vol)
            .unwrap());
        assert_eq!(sol.objects[0], Some(Vol));
    }

    #[test]
    fn bounding_boxes() {
        let k = &Kernel::default();
        let vol = UnbendVolume::create(Vec3::ZERO, Vec3::Z, Vec3::X, 1.0, 0.01);
        let VolumeKind::Unbend(u) = vol.kind() else {
            panic!("not an unbend")
        };
        // a box in the first quadrant maps to x in [r_min, r_max]
        let pb = Extent3d::from_bounds(1.0, 1.0, 0.0, 2.0, 2.0, 1.0);
        let mut b = Extent3d::new();
        let mut pseudo = PseudoBox::new();
        u.evaluate_bounding_box_simple(k, &vol, &pb, Some(&mut b), Some(&mut pseudo))
            .unwrap();
        assert!(b.u_min() <= 2f64.sqrt() + 1e-9 && b.u_max() >= 8f64.sqrt() - 1e-9);
        assert!(b.w_min() <= 1e-12 && b.w_max() >= 1.0 - 1e-12);
        // unbounded
        let mut un = Extent3d::new();
        un.set_unbounded();
        u.evaluate_bounding_box_simple(k, &vol, &un, Some(&mut b), Some(&mut pseudo))
            .unwrap();
        assert_eq!(b.u_max(), INFINITE_PARAMETER);
        // a flat box has zero-length edges
        let flat = Extent3d::from_bounds(1.0, 1.0, 0.0, 2.0, 2.0, 0.0);
        assert!(u
            .evaluate_bounding_box_simple(k, &vol, &flat, Some(&mut b), None)
            .is_err());
        assert!(u
            .evaluate_bounding_box_simple(k, &vol, &flat, None, None)
            .is_ok());
        assert_eq!(u.trim_param_bounding_box_simple(&pb), pb);
    }

    #[test]
    fn equality_compares_only_the_tolerance() {
        let a = UnbendVolume::from_members(1.0, 0.5);
        let b = UnbendVolume::from_members(2.0, 0.5);
        assert!(a.members_eq(&b));
        assert!(!a.members_eq(&UnbendVolume::from_members(1.0, 0.6)));
    }

    #[test]
    fn helpers() {
        assert!(is_kind_of(UNBEND_VOLUME_TYPE));
        let u = UnbendVolume::new();
        assert!(!u.is_bounded_simple());
        assert_eq!(u.natural_param_domain().u_max(), INFINITE_PARAMETER);
        assert_eq!(u.unbend_neutral_dist(), 0.0);
        assert_eq!(unbend(1.0).legal_min_u(), 0.01 - EFF_ZERO);
    }
}

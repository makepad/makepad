//! The bend volume `SmBendVolume`: a map that wraps planes parallel to the neutral
//! plane onto cylindrical shells about the bend axis, like bending sheet metal (the
//! inverse of the unbend volume).
//!
//! Upstream: `source/SMLib/src/SmBendVolume.cpp`, `source/SMLib/inc/SmBendVolume.h`.
//!
//! The simple map takes the parameter point `(u, v, w)` to
//! `(u cos(v / d), u sin(v / d), w)` with `d` the neutral distance: `u` is the radius
//! and `v` the arc length on the neutral shell. Its natural domain is the half space
//! `u >= singularity_tol` over one period `|v| <= pi d`; it is singular on the `w`
//! axis. The orient map places the bend frame (origin, mid direction, binormal, axis)
//! in in-space.
//!
//! | Rust                                              | Upstream                                       |
//! |---------------------------------------------------|------------------------------------------------|
//! | [`BendVolume`]                                    | `SmBendVolume` (members `m_dBendNeutralDist`, `m_dSingularityTol`) |
//! | [`BendVolume::new`], `Default`                    | `SmBendVolume(const SmContext*)`               |
//! | [`BendVolume::create`]                            | `SmBendVolume(crContext, Center, Axis, MidDir, NeutralDist, SingularityTol)` |
//! | [`Volume::copy`] (bend arm)                       | `SmBendVolume(const SmBendVolume&, SmBoolean)`, `Copy` |
//! | `Drop` (implicit)                                 | `~SmBendVolume`                                |
//! | [`Volume::approx_eq`] (bend arm), [`BendVolume::members_eq`] | `operator==`                        |
//! | [`BendVolume::natural_param_domain`]              | `GetNaturalParamDomain`                        |
//! | [`BendVolume::legal_min_u`]                       | `GetLegalMinU`                                 |
//! | [`Volume::bend_origin`], [`Volume::bend_axis`], [`Volume::bend_mid_dir`], [`Volume::bend_binormal`] | `GetBendOrigin`, `GetBendAxis`, `GetBendMidDir`, `GetBendBinormal` |
//! | [`BendVolume::bend_neutral_dist`], [`BendVolume::singularity_tol`] | `GetBendNeutralDist`, `GetSingularityTol` |
//! | [`BendVolume::evaluate`]                          | `Evaluate`                                     |
//! | [`BendVolume::evaluate_simple`]                   | `EvaluateSimple`                               |
//! | [`BendVolume::evaluate_bounding_box_simple`]      | `EvaluateBoundingBoxSimple`                    |
//! | [`BendVolume::trim_param_bounding_box_simple`]    | `TrimParamBoundingBoxSimple`                   |
//! | [`BendVolume::inv_evaluate_guess_point_simple`]   | `InvEvaluateGuessPointSimple`                  |
//! | [`BendVolume::global_point_solve_simple`]         | `GlobalPointSolveSimple`                       |
//! | [`BendVolume::local_point_solve_simple`]          | `LocalPointSolveSimple`                        |
//! | [`BendVolume::is_bounded_simple`]                 | `IsBoundedSimple`                              |
//! | [`BendVolume::is_point_in_param_domain_simple`]   | `IsPointInParamDomainSimple`                   |
//! | [`BendVolume::is_line_in_param_domain_simple`]    | `IsLineInParamDomainSimple`                    |
//! | [`BendVolume::is_curve_in_param_domain_simple`]   | `IsCurveInParamDomainSimple`                   |
//! | [`BendVolume::read_from_db`]                      | `ReadFromDB`                                   |
//! | [`BendVolume::make_exact_b_spline_1st_out_surface`] | `MakeExactBSpline1stOutSurface` (in no unit in units.json; called by `SrfInVolume::make_exact_b_spline_if_possible_with`) |
//! | [`BendVolume::make_exact_b_spline_1st_out_curve`] | `MakeExactBSpline1stOutCurve` (in no unit in units.json; called by `CrvInVolume::make_exact_b_spline_if_possible_with`) |
//! | [`BEND_VOLUME_TYPE`], [`is_kind_of`], [`Volume::type_code`] and friends | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`), `IsKindOf` |
//!
//! Deviations:
//! - **Volume base.** The bend is the [`VolumeKind::Bend`] variant of a [`Volume`],
//!   which holds the orient and next maps; the virtual overrides are dispatched from
//!   [`Volume`] (`surface::volume::p1`). [`BendVolume::evaluate`] reads the frame
//!   and next maps from the volume it is given.
//! - **Constructor.** The component constructor is not in the unit's function list
//!   (nothing else owns it) but is ported as [`BendVolume::create`] because it is
//!   the only way upstream builds a usable bend. Its zero-vector repair compares
//!   `Unitize`'s `SM_ERR_INVALID_INPUT` with `SM_ERR`, so the first two repair
//!   branches never run; ported as it behaves (as for the twist and unbend
//!   volumes). A neutral distance below the clamped singularity tolerance becomes
//!   `1.0`. Contexts and the `SE_MSG` reports (also those of the evaluations for
//!   points with `u < GetLegalMinU()`) are dropped.
//! - **Default construction.** A default-constructed bend has neutral distance `0`
//!   (the empty constructor's initializer) and singularity tolerance
//!   `SM_UNDEF_DOUBLE` (the member initializer) ([`BendVolume::new`]).
//! - **Bounding boxes.** `EvaluateBoundingBoxSimple` is analytic, in projection
//!   space. Ported as written, including its slips: the unbounded-`u` branch takes
//!   the bounded `y` maximum from cosines, tests `dThetaMin < 0 || dThetaMax <= 0`
//!   for the `y` minimum, and the containment tests compare angles that are not
//!   reduced to one period. `SetMinMax` fails (status ignored upstream, output box
//!   left as it was) when a minimum exceeds its maximum. The unused parameter
//!   pseudo box is dropped.
//! - **Trim.** `TrimParamBoundingBoxSimple` ignores the status of its final
//!   `SetMinMax`, which leaves `trim_box` unchanged when the trimmed box is
//!   inverted (for instance after the period clamp); kept.
//! - **Inverse guess.** `InvEvaluateGuessPointSimple` writes its guesses and returns
//!   `SM_ERR` near the axis: [`BendVolume::inv_evaluate_guess_point_simple`]
//!   returns both; the [`Volume`] dispatch turns the failure into `SM_ERR` (its
//!   callers propagate it).
//! - **Solvers.** `GlobalPointSolveSimple` and `LocalPointSolveSimple` cannot fail
//!   and ignore the parameter domain (dropped); they return upstream's
//!   `rbFoundAnswer` (always `true`). The volume itself (`m_apObjects[0] = this`) is
//!   the caller's object handle.
//! - **`IsCurveInParamDomainSimple`** asks [`Curve::is_on_positive_side_of_planes`]
//!   (`SmCurve::IsOnPositiveSideOfPlanes`) and takes the [`Kernel`] it needs.
//! - **`ReadFromDB`** reads the two members, then the base data
//!   ([`Volume::read_base_from_db`], status ignored as upstream). Filling a given object (`rpNewVolume != NULL`) is not
//!   modelled; a new volume is always made.
//! - **Unused arguments** upstream ignores (the from-left flags, zero-tangent and
//!   zero-sampling options of the evaluations) are dropped where the call is local
//!   to this module.
//! - **`MakeExactBSpline1stOutCurve`** takes the input space as a flag, as the surface
//!   variant, and returns the new curve (`None` for upstream's `NULL`). The statuses
//!   upstream ignores (the sample `EvaluatePoint`s, the end-point `MapPoint` /
//!   `EvaluatePoint`, `Evaluate`, `CCWAngleBetween`) leave the previous value
//!   ([`Vec3::UNDEF`] for points, `0` for angles; upstream's angles are then
//!   uninitialised). The unused plane-distance sums and the debug drawing are dropped.
//!   When all three frame planes are constant (a degenerate curve) the line is built
//!   and then replaced by the circle, as upstream (which leaks the line).
//! - **`MakeExactBSpline1stOutSurface`** takes the input space as a flag
//!   (`in_space`: `SM_VS_IN_SPACE`; otherwise `SM_VS_PARAM_SPACE`, so the invalid-space
//!   `SM_ERR` has no counterpart) and returns the new surface (`None` for upstream's
//!   `NULL`). The statuses upstream ignores (the sample `EvaluatePoint`s, the corner
//!   `MapPoint` / `EvaluatePoint`, `EvaluateNormal`, the `SetUMin`-style setters)
//!   leave the previous value, as upstream's out-params do (the defaults are
//!   [`Vec3::UNDEF`]); [`SolidError::Unported`] still propagates. When more than one
//!   frame plane is constant (a degenerate input), each matching branch builds its
//!   surface and the last one wins (upstream leaks the earlier ones). The unused
//!   `sCylinderOrigin` and the debug drawing are dropped.
//! - **Recursion:** none here; the next-map compounding recurses through
//!   [`Volume::map`] as in `surface::volume::p1`.

use crate::curve::curve::p2::Curve;
use crate::curve::ellipse::{Ellipse, EllipseOptions};
use crate::curve::line::Line;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{Solution, SolutionArray, SolutionObject, SolutionType};
use crate::geom::tol::{approx_tol_3d, is_zero, scaled_zero2, SmallTopoType, ZONE_TOL_3D};
use crate::geom::transform::Transform;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{
    max2, max4, min2, min4, rad_to_deg, EFF_ZERO, EFF_ZERO_DEG, INFINITE_PARAMETER, PI,
    UNDEF_DOUBLE,
};
use crate::nurbs::knots::Side;
use crate::surface::cone::Cone;
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

pub use crate::surface::volume::p1::BEND_VOLUME_TYPE;

const OP_EVAL: &str = "surface::bend_volume::BendVolume::evaluate";
const OP_EVAL_SIMPLE: &str = "surface::bend_volume::BendVolume::evaluate_simple";
const OP_TRIM: &str = "surface::bend_volume::BendVolume::trim_param_bounding_box_simple";

/// The highest derivative the bend evaluations accept.
pub const MAX_DERIV: usize = 3;

/// `SmBendVolume::IsKindOf`: `SmBendVolume_TYPE` or a base class's type.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == BEND_VOLUME_TYPE || volume_is_kind_of(t)
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

/// `SM_IS_CONTAINED(c, a, b)`: `c` inside the closed interval between `a` and `b`
/// (either order), widened by `SmTol::GetScaledZero(a, b)`.
fn is_contained(c: f64, a: f64, b: f64) -> bool {
    let z = scaled_zero2(&a, &b);
    (a <= (c + z) && (c - z) <= b) || (b <= (c + z) && (c - z) <= a)
}

/// `SM_CLAMP_TO_ZERO_PERIOD(v, p)`: `v` reduced to `[0, p]` with `fmod`.
fn clamp_to_zero_period(v: f64, p: f64) -> f64 {
    if v < 0.0 {
        ((v % p) + p) % p
    } else if v > p {
        v % p
    } else {
        v
    }
}

/// `SM_CLAMP_TO_PERIOD(v, p1, p2)`: `v` reduced to the period `[p1, p2]`.
fn clamp_to_period(v: f64, p1: f64, p2: f64) -> f64 {
    clamp_to_zero_period(v - p1, p2 - p1) + p1
}

/// The members of `SmBendVolume`; its orient map (the bend frame) and any next maps
/// live in the [`Volume`] holding it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BendVolume {
    /// `m_dBendNeutralDist`: the distance along the mid direction from the bend axis
    /// to the neutral plane.
    bend_neutral_dist: f64,
    /// `m_dSingularityTol`: the smallest distance of a non-singular point from the
    /// singularity at `u = 0`.
    singularity_tol: f64,
}

impl Default for BendVolume {
    fn default() -> Self {
        Self::new()
    }
}

impl BendVolume {
    /// The default-constructed bend (`SmBendVolume(const SmContext*)`): neutral
    /// distance `0` and singularity tolerance `SM_UNDEF_DOUBLE` (its member
    /// initializer).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bend_neutral_dist: 0.0,
            singularity_tol: UNDEF_DOUBLE,
        }
    }

    /// A bend with the given members as stored, unchecked (the state `ReadFromDB`
    /// builds).
    #[must_use]
    pub const fn from_members(bend_neutral_dist: f64, singularity_tol: f64) -> Self {
        Self {
            bend_neutral_dist,
            singularity_tol,
        }
    }

    /// The bend volume about the in-space line through `center` along `axis`, the
    /// parameter `u` axis along `mid_dir`, neutral plane at `bend_neutral_dist` (the
    /// component constructor; upstream's default `singularity_tol` is
    /// `SM_ZONE_TOL_3D`). Bad input is repaired, never rejected: the tolerance is
    /// raised to `SM_ZONE_TOL_3D`, a neutral distance below it becomes `1.0`, zero
    /// vectors are replaced by a frame about the axis and a mid direction not
    /// perpendicular to the axis is projected (or, when parallel, replaced).
    #[must_use]
    pub fn create(
        center: Point3,
        axis: Vec3,
        mid_dir: Vec3,
        bend_neutral_dist: f64,
        singularity_tol: f64,
    ) -> Volume {
        let mut bend = Self::from_members(bend_neutral_dist, singularity_tol);

        // check input - dSingularityTol >= SM_ZONE_TOL_3D
        if bend.singularity_tol < ZONE_TOL_3D {
            bend.singularity_tol = ZONE_TOL_3D;
        }

        // check input - Positive Definite BendNeutralDistance
        if bend_neutral_dist < bend.singularity_tol {
            // do the best we can
            bend.bend_neutral_dist = 1.0;
        }

        // unitize bend vectors
        let (mut bend_axis, ok1) = unitize(axis);
        let (mut bend_mid_dir, ok2) = unitize(mid_dir);

        // check input - nonZero input vectors
        if !ok1 || !ok2 {
            // Upstream compares the failed statuses (SM_ERR_INVALID_INPUT) with SM_ERR,
            // so only its last branch runs: a frame about the (repaired) axis.
            if let Ok([x, y, _]) = bend_axis.make_unit_ortho_vectors(None) {
                bend_axis = x;
                bend_mid_dir = y;
            }
        }

        // check input - bend vectors must be perpendicular to one another
        if !bend_axis.is_perpendicular_to(bend_mid_dir, EFF_ZERO_DEG) {
            if bend_axis.is_parallel_to(bend_mid_dir, EFF_ZERO_DEG) {
                // Axis and StartDir don't span a plane - do the best we can
                if let Ok([x, _, z]) = bend_mid_dir.make_unit_ortho_vectors(None) {
                    bend_mid_dir = x;
                    bend_axis = z;
                }
            } else {
                bend_mid_dir = bend_axis.cross(bend_mid_dir.cross(bend_axis));
                bend_mid_dir = unitize(bend_mid_dir).0;
            }
        }

        // set BendBinormal perp to both BendAxis and BendMidDir
        let bend_binormal = bend_axis.cross(bend_mid_dir);

        // build BendCoordinates into m_pOrientMap
        let orient = Transform::new(center, bend_mid_dir, bend_binormal, Some(bend_axis));
        Volume::oriented(VolumeKind::Bend(bend), Some(orient))
    }

    /// `m_dBendNeutralDist` (`GetBendNeutralDist`).
    #[must_use]
    pub const fn bend_neutral_dist(&self) -> f64 {
        self.bend_neutral_dist
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

    /// The member part of `operator==`: `SM_IS_ZERO` of the differences of the
    /// neutral distances and of the singularity tolerances.
    #[must_use]
    pub fn members_eq(&self, other: &Self) -> bool {
        let mut rtn = true;
        rtn &= is_zero(self.bend_neutral_dist - other.bend_neutral_dist);
        rtn &= is_zero(self.singularity_tol - other.singularity_tol);
        rtn
    }

    /// The parameter domain: the half space `u >= singularity_tol` over the primary
    /// period `|v| <= pi d`, unbounded in `w` (`GetNaturalParamDomain`).
    #[must_use]
    pub fn natural_param_domain(&self) -> Extent3d {
        let min_y = -PI * self.bend_neutral_dist;
        let max_y = -min_y;

        // init output to positive definite u half-space
        Extent3d::from_bounds(
            self.singularity_tol,
            min_y,
            -INFINITE_PARAMETER,
            INFINITE_PARAMETER,
            max_y,
            INFINITE_PARAMETER,
        )
    }

    /// Out-space position and partial derivatives `0..=n` (`n <= 3`) of the parameter
    /// point, `D[u][v][w]` (`Evaluate`): the bend evaluated directly in the in-space
    /// frame of `volume`'s orient map, then compounded through its next maps when
    /// `opts.with_compounding`. `volume` is the volume holding this bend.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) for `n > 3`; the next
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
                "highest derivative above 3",
            ));
        }
        // (a point with u < GetLegalMinU() is only reported upstream)
        let n1 = n + 1;
        let sv = |u: usize, v: usize, w: usize| (u * n1 + v) * n1 + w;

        // locals
        let du = param_point.x;
        let dv = param_point.y;
        let dw = param_point.z;
        let cos = (dv / self.bend_neutral_dist).cos();
        let sin = (dv / self.bend_neutral_dist).sin();
        let mid = volume.bend_mid_dir();
        let bin = volume.bend_binormal();
        let axis = volume.bend_axis();

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
            volume.bend_origin() + (du * cos) * mid + (du * sin) * bin + dw * axis,
        );

        // 1st derivs
        if n >= 1 {
            let di1 = 1.0 / self.bend_neutral_dist;
            put(sv(1, 0, 0), cos * mid + sin * bin);
            put(
                sv(0, 1, 0),
                (-du * di1 * sin) * mid + (du * di1 * cos) * bin,
            );
            put(sv(0, 0, 1), axis);

            // NonZero 2nd derivs
            if n >= 2 {
                let di2 = di1 * di1;
                put(sv(1, 1, 0), (-di1 * sin) * mid + (di1 * cos) * bin);
                put(
                    sv(0, 2, 0),
                    (-du * di2 * cos) * mid + (-du * di2 * sin) * bin,
                );

                // NonZero 3rd derivs
                if n >= 3 {
                    let di3 = di2 * di1;
                    put(sv(1, 2, 0), (-di2 * cos) * mid + (-di2 * sin) * bin);
                    put(
                        sv(0, 3, 0),
                        (du * di3 * sin) * mid + (-du * di3 * cos) * bin,
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

    /// Projection-space position and partial derivatives `0..=n` (`n <= 3`) of the
    /// parameter point, `D[u][v][w]`, without orientation or compounding
    /// (`EvaluateSimple`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) for `n > 3`.
    pub fn evaluate_simple(&self, param_point: Point3, n: usize) -> Result<Vec<Vec3>> {
        // check input - limit the number of derivatives
        if n > MAX_DERIV {
            return Err(SolidError::invalid_input(
                OP_EVAL_SIMPLE,
                "highest derivative above 3",
            ));
        }
        let n1 = n + 1;
        let sv = |u: usize, v: usize, w: usize| (u * n1 + v) * n1 + w;

        // locals
        let du = param_point.x;
        let dv = param_point.y;
        let dw = param_point.z;
        let cos = (dv / self.bend_neutral_dist).cos();
        let sin = (dv / self.bend_neutral_dist).sin();

        // init output array
        let mut d = vec![Vec3::ZERO; eval_size(n)];
        let mut put = |i: usize, v: Vec3| {
            if let Some(slot) = d.get_mut(i) {
                *slot = v;
            }
        };

        // position
        put(0, Vec3::new(du * cos, du * sin, dw));

        // 1st derivs
        if n >= 1 {
            let di1 = 1.0 / self.bend_neutral_dist;
            put(sv(1, 0, 0), Vec3::new(cos, sin, 0.0));
            put(sv(0, 1, 0), Vec3::new(-du * di1 * sin, du * di1 * cos, 0.0));
            put(sv(0, 0, 1), Vec3::new(0.0, 0.0, 1.0));

            // 2nd derivs (Duu, Duw, Dvw, Dww are zero)
            if n >= 2 {
                let di2 = di1 * di1;
                put(sv(1, 1, 0), Vec3::new(-di1 * sin, di1 * cos, 0.0));
                put(
                    sv(0, 2, 0),
                    Vec3::new(-du * di2 * cos, -du * di2 * sin, 0.0),
                );

                // 3rd derivs (all but Duvv and Dvvv are zero)
                if n >= 3 {
                    let di3 = di2 * di1;
                    put(sv(1, 2, 0), Vec3::new(-di2 * cos, -di2 * sin, 0.0));
                    put(sv(0, 3, 0), Vec3::new(du * di3 * sin, -du * di3 * cos, 0.0));
                }
            }
        }
        Ok(d)
    }

    /// The projection-space boxes containing the image of `param_box`
    /// (`EvaluateBoundingBoxSimple`), written into the requested outputs: the
    /// axis-aligned box from the mapped corners and the extreme angles (with
    /// unbounded sides when `param_box` reaches `u = SM_INFINITE_PARAMETER`), and a
    /// pseudo box oriented on the mapped mid-angle direction. `proj_box` is left
    /// unchanged when its computed minimum exceeds its maximum on an axis.
    pub fn evaluate_bounding_box_simple(
        &self,
        param_box: &Extent3d,
        proj_box: Option<&mut Extent3d>,
        proj_pseudo_box: Option<&mut PseudoBox>,
    ) {
        // no work - no output
        if proj_box.is_none() && proj_pseudo_box.is_none() {
            return;
        }
        // (a box reaching u < GetLegalMinU() is only reported upstream)

        // locals
        let v_min = param_box.v_min();
        let v_max = param_box.v_max();
        let theta_min = v_min / self.bend_neutral_dist;
        let theta_max = v_max / self.bend_neutral_dist;
        let cos_theta_min = theta_min.cos();
        let sin_theta_min = theta_min.sin();
        let cos_theta_max = theta_max.cos();
        let sin_theta_max = theta_max.sin();
        let u_min = param_box.u_min();
        let u_max = param_box.u_max();
        let half_pi = PI / 2.0;

        // when asked build ProjectSpace BBox
        if let Some(b) = proj_box {
            let (x_min, x_max, y_min, y_max);
            if u_max == INFINITE_PARAMETER {
                // dXMax - infinite when ParamBox maps above the X = 0 plane
                x_max = if is_contained(theta_max, -half_pi, half_pi)
                    || is_contained(theta_min, -half_pi, half_pi)
                    || (theta_min < -half_pi && theta_max > half_pi)
                {
                    INFINITE_PARAMETER
                } else {
                    // set Max X from Min Radius Corner Values
                    max2(cos_theta_min, cos_theta_max) * u_min
                };

                // dXMin - negative infinite when ParamBox maps below the X = 0 plane
                x_min = if is_contained(theta_min, -half_pi, -PI)
                    || is_contained(theta_min, half_pi, PI)
                    || is_contained(theta_max, -half_pi, -PI)
                    || is_contained(theta_max, half_pi, PI)
                {
                    -INFINITE_PARAMETER
                } else {
                    // set Min X from min Radius Corner Values
                    min2(cos_theta_min, cos_theta_max) * u_min
                };

                // dYMax - infinite when Param Box maps above the Y = 0 plane
                y_max = if theta_min >= 0.0 || theta_max > 0.0 {
                    INFINITE_PARAMETER
                } else {
                    // dYMax is taken from the min radius corner points (upstream
                    // uses the cosines here)
                    max2(cos_theta_min, cos_theta_max) * u_min
                };

                // dYMin - negative infinite when paramBox maps below the Y = 0 plane
                y_min = if theta_min < 0.0 || theta_max <= 0.0 {
                    -INFINITE_PARAMETER
                } else {
                    // dYMin is taken from the min radius corner points
                    min2(sin_theta_min, sin_theta_max) * u_min
                };
            } else {
                // X interval is defined by corner points and the Theta = 0 map point
                let corners_x = [
                    cos_theta_min * u_min,
                    cos_theta_min * u_max,
                    cos_theta_max * u_min,
                    cos_theta_max * u_max,
                ];
                let [a, b2, c, d] = corners_x;
                if is_contained(0.0, theta_min, theta_max) {
                    // Theta = 0.0 is within the ParamBox domain
                    x_max = u_max;
                    x_min = min4(a, b2, c, d);
                } else {
                    // X Min and Max are the min and max values of the 4 mapped corners
                    x_min = min4(a, b2, c, d);
                    x_max = max4(a, b2, c, d);
                }

                // Y interval is defined by the corner points and the Theta = Pi/2
                // and - Pi/2 points
                let [a, b2, c, d] = [
                    sin_theta_min * u_min,
                    sin_theta_min * u_max,
                    sin_theta_max * u_min,
                    sin_theta_max * u_max,
                ];
                let up = is_contained(half_pi, theta_min, theta_max);
                let down = is_contained(-half_pi, theta_min, theta_max);
                if up && down {
                    y_max = u_max;
                    y_min = -u_max;
                } else if up {
                    y_max = u_max;
                    y_min = min4(a, b2, c, d);
                } else if down {
                    y_max = max4(a, b2, c, d);
                    y_min = -u_max;
                } else {
                    // the corners define the box
                    y_max = max4(a, b2, c, d);
                    y_min = min4(a, b2, c, d);
                }
            }

            // set the output (status ignored upstream: an inverted box is not set)
            let _ = b.set_min_max_coords(
                x_min,
                y_min,
                param_box.w_min(),
                x_max,
                y_max,
                param_box.w_max(),
            );
        }

        // when asked - build a ProjPseudoBox oriented about the center of the mapped
        // ParamBox
        if let Some(pb) = proj_pseudo_box {
            let v_mid = (v_min + v_max) / 2.0;
            let v_end = v_max - v_mid;
            let theta_mid = v_mid / self.bend_neutral_dist;
            let theta_end = v_end / self.bend_neutral_dist;
            let cos_theta_mid = theta_mid.cos();
            let sin_theta_mid = theta_mid.sin();
            let sin_theta_end = theta_end.sin();
            let x_max = u_max;
            let x_min = if theta_end < PI {
                sin_theta_end * u_min
            } else {
                sin_theta_end * u_max
            };
            let y_max = if theta_end < PI {
                sin_theta_end * u_max
            } else {
                u_max
            };
            let y_min = -y_max;

            // bases
            let pseudo_x = Vec3::new(cos_theta_mid, sin_theta_mid, 0.0);
            let pseudo_y = Vec3::new(-sin_theta_mid, cos_theta_mid, 0.0);
            let pseudo_z = Vec3::new(0.0, 0.0, 1.0);

            // intervals
            let ivl_x = Extent1d::from_min_max(x_min, x_max);
            let ivl_y = Extent1d::from_min_max(y_min, y_max);
            let ivl_z = Extent1d::from_min_max(param_box.w_min(), param_box.w_max());

            // set output (status ignored upstream)
            let _ = pb.set_basis(pseudo_x, pseudo_y, pseudo_z);
            pb.set_intervals(ivl_x, ivl_y, ivl_z);
        }
    }

    /// Trims `param_box` to the natural domain into `trim_box`
    /// (`TrimParamBoundingBoxSimple`): `u` to `u >= singularity_tol` and `v` to the
    /// primary period (each end reduced to the period separately). `trim_box` is
    /// left unchanged when the trimmed box is inverted.
    ///
    /// # Errors
    ///
    /// `SM_ERR`, with `trim_box` initialised to the empty box, when `param_box` lies
    /// entirely below `u = singularity_tol` or outside the period (within
    /// `SM_EFF_ZERO`).
    pub fn trim_param_bounding_box_simple(
        &self,
        param_box: &Extent3d,
        trim_box: &mut Extent3d,
    ) -> Result<()> {
        // locals
        let mut u_min = param_box.u_min();
        let mut v_min = param_box.v_min();
        let w_min = param_box.w_min();
        let u_max = param_box.u_max();
        let mut v_max = param_box.v_max();
        let w_max = param_box.w_max();

        // Trim to 1/2 space, U > 0 - boxes totally outside the legal domain are
        // turned into negative volumes
        if u_max < self.singularity_tol {
            trim_box.init();
            return Err(SolidError::sm_err(OP_TRIM));
        }
        if u_min < self.singularity_tol {
            u_min = self.singularity_tol;
        }

        // Get primary period
        let period_min = self.bend_neutral_dist * -PI;
        let period_max = -period_min;

        // boxes outside the legal domain are turned into negative volumes
        if v_max < period_min - EFF_ZERO || v_min > period_max + EFF_ZERO {
            trim_box.init();
            return Err(SolidError::sm_err(OP_TRIM));
        }

        // Trim to Primary Period
        v_min = clamp_to_period(v_min, period_min, period_max);
        v_max = clamp_to_period(v_max, period_min, period_max);

        // set output (status ignored upstream)
        let _ = trim_box.set_min_max_coords(u_min, v_min, w_min, u_max, v_max, w_max);
        Ok(())
    }

    /// The exact inverse `(r, d atan2(y, x), z)` of `proj_point` (`theta = 0` within
    /// `GetLegalMinU()` of the axis) as the guesses of the Newton solvers, two
    /// (`v = +-d pi`) on the seam (`InvEvaluateGuessPointSimple`). The flag is
    /// `false` (upstream `SM_ERR`, guesses still written) within `GetLegalMinU()` of
    /// the axis.
    #[must_use]
    pub fn inv_evaluate_guess_point_simple(&self, proj_point: Point3) -> (Vec<Point3>, bool) {
        let (sols, r) = self.inverse(proj_point);
        // return value (SM_ERR within the singularity)
        let in_singularity = r < self.legal_min_u();
        (sols, !in_singularity)
    }

    /// The exact inverse of `proj_point`: one parameter point, two on the seam
    /// (`theta = +-pi`, the second with `-v`), and the radius.
    fn inverse(&self, proj_point: Point3) -> (Vec<Point3>, f64) {
        // compute the inverse mapping
        let r = (proj_point.x * proj_point.x + proj_point.y * proj_point.y).sqrt();
        // for points within the singularity set dTheta = 0.0 - otherwise
        // dTheta = ATan(y,x)
        let theta = if r < self.legal_min_u() {
            0.0
        } else {
            proj_point.y.atan2(proj_point.x)
        };
        let d = self.bend_neutral_dist;

        // remember 2 solutions for seams
        let on_seam = is_zero(theta - PI) || is_zero(theta + PI);

        let mut points = vec![Vec3::new(r, d * theta, proj_point.z)];
        // when on the seam - add 2nd solution
        if on_seam {
            points.push(Vec3::new(r, d * -theta, proj_point.z));
        }
        (points, r)
    }

    /// Solves for the parameter point mapping to `proj_point`
    /// (`GlobalPointSolveSimple`): `solutions` becomes the exact inverse (two
    /// solutions on the seam, the second with `-v`) with value `0` and object `this`
    /// (the volume's handle). Returns upstream's `rbFoundAnswer` (`true`).
    pub fn global_point_solve_simple<O: SolutionObject>(
        &self,
        proj_point: Point3,
        solutions: &mut SolutionArray<O>,
        this: O,
    ) -> bool {
        // inverted ParamPoint (the seam's 2nd solution negates the inverted v)
        let (points, _) = self.inverse(proj_point);

        // init output
        solutions.set_size(points.len());

        // set output
        for (ii, p) in points.into_iter().enumerate() {
            if let Some(sol) = solutions.get_mut(ii) {
                Self::fill_solution(sol, p, this);
            }
        }
        true
    }

    /// Solves for the parameter point mapping to `proj_point`
    /// (`LocalPointSolveSimple`): [`BendVolume::global_point_solve_simple`]; on the
    /// seam the solution closer to `guess` (the second on a tie). Returns upstream's
    /// `rbFoundAnswer`.
    pub fn local_point_solve_simple<O: SolutionObject>(
        &self,
        proj_point: Point3,
        guess: Point3,
        solution: &mut Solution<O>,
        this: O,
    ) -> bool {
        let mut solutions = SolutionArray::new();

        // pass the call along
        let found = self.global_point_solve_simple(proj_point, &mut solutions, this);
        let point_of = |s: &Solution<O>| {
            Vec3::new(
                s.start.parameters[0],
                s.start.parameters[1],
                s.start.parameters[2],
            )
        };
        match (solutions.get(0), solutions.get(1)) {
            (Some(s0), None) => *solution = *s0,
            (Some(s0), Some(s1)) => {
                // on a seam - return solution closest to ParamGuessPoint
                let dist_sq0 = point_of(s0).distance_squared(guess);
                let dist_sq1 = point_of(s1).distance_squared(guess);
                *solution = if dist_sq0 < dist_sq1 { *s0 } else { *s1 };
            }
            _ => {}
        }
        found
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

    /// The bend's parameter space is bounded (the half space `u > 0` and one period)
    /// (`IsBoundedSimple`).
    #[must_use]
    pub const fn is_bounded_simple(&self) -> bool {
        true
    }

    /// Whether the parameter point is in the natural domain: `v / d` in
    /// `[-pi, pi]` (within the scaled zero) and `u > GetLegalMinU()`
    /// (`IsPointInParamDomainSimple`).
    #[must_use]
    pub fn is_point_in_param_domain_simple(&self, param_point: Point3) -> bool {
        // Only ParamPoints in the positive half-space, u>0, and in the
        // PrimaryPeriod are within the SmBendVolume's Natural ParamSpace domain.
        let theta_rad = param_point.y / self.bend_neutral_dist;
        let in_primary_period = is_contained(theta_rad, -PI, PI);
        let in_half_space = param_point.x > self.legal_min_u();
        in_primary_period && in_half_space
    }

    /// Whether the parameter segment is in the domain: both end points are (the
    /// domain is convex) (`IsLineInParamDomainSimple`).
    #[must_use]
    pub fn is_line_in_param_domain_simple(&self, start: Point3, end: Point3) -> bool {
        let mut rtn = self.is_point_in_param_domain_simple(start);
        rtn &= self.is_point_in_param_domain_simple(end);
        rtn
    }

    /// Whether the parameter curve is on the domain side of the three bounding
    /// planes `u = singularity_tol`, `v = +-pi d` (`IsCurveInParamDomainSimple`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the curve's plane classification
    /// ([`Curve::is_on_positive_side_of_planes`]).
    pub fn is_curve_in_param_domain_simple(&self, k: &Kernel, param_curve: &Curve) -> Result<bool> {
        // load the 3 domain bounding planes with normals pointing into the allowed
        // 1/2 space (any 2Pi*d interval is legal - this one is centered on V=0)
        let plane_points = [
            Vec3::new(self.singularity_tol, 0.0, 0.0),
            Vec3::new(0.0, PI * self.bend_neutral_dist, 0.0),
            Vec3::new(0.0, -PI * self.bend_neutral_dist, 0.0),
        ];
        let plane_normals = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        // is curve coincident, touching, or on positive side of all bounding planes
        param_curve.is_on_positive_side_of_planes(k, &plane_points, &plane_normals)
    }

    /// `MakeExactBSpline1stOutSurface`: the exact first out-space image of `input`
    /// (from in-space when `in_space`, else from parameter space) through the bend
    /// `volume` (whose kind is `self`), or `None` when it is not one of the analytic
    /// cases. A surface parallel to the plane normal to the bend axis maps to a plane
    /// normal to the axis, one parallel to the neutral plane (normal to the mid
    /// direction) to a cylinder about the bend axis, and one normal to the binormal to
    /// a plane through the bend axis. The test samples a 6 x 6 grid of the natural
    /// domain; any sample outside the bend's domain gives `None`.
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
        // locals: ParamSpace and InSpace Bend orientations - Bend's Neutral plane is perp
        // to the Bend's MidDirection
        let (param_origin, bend_origin) = (Vec3::ZERO, volume.bend_origin());
        let (param_mid_dir, bend_mid_dir) = (Vec3::X, volume.bend_mid_dir());
        let (param_axis, bend_axis) = (Vec3::Z, volume.bend_axis());
        let (param_binormal, bend_binormal) = (Vec3::Y, volume.bend_binormal());
        let bend_neutral_dist = self.bend_neutral_dist();

        // locals: making InputSpace either ParamSpace or InSpace
        let (input_origin, input_mid_dir, input_axis) = if in_space {
            (bend_origin, bend_mid_dir, bend_axis)
        } else {
            (param_origin, param_mid_dir, param_axis)
        };
        let _ = param_binormal;

        // locals
        let mut input_pt = Vec3::UNDEF;
        let mut out_space_pt = Vec3::UNDEF;

        let approx_tol = approx_tol_3d(k, SmallTopoType::NormalSize);
        let domain = input.natural_uv_domain_with_nurb(k)?;
        let ivl_u = domain.u_interval();
        let ivl_v = domain.v_interval();

        // note: for Bend mapping Bend principle axes happen to be the same in InSpace and
        // OutSpace
        let mut axis_plane_sum = 0.0;
        let mut mid_plane_sum = 0.0;
        let mut binormal_plane_sum = 0.0;
        let mut axis_plane_range = Extent1d::new();
        let mut mid_dir_plane_range = Extent1d::new();
        let mut binormal_plane_range = Extent1d::new();
        let mut const_axis_plane = false;
        let mut const_mid_dir_plane = false;
        let mut const_binormal_plane = false;
        let mut out_space_bbox = PseudoBox::from_basis_intervals(
            [bend_mid_dir, bend_binormal, bend_axis],
            [mid_dir_plane_range, binormal_plane_range, axis_plane_range],
        );

        // See if the InputSurface is perpendicular to any of the Bend principle axes
        // (parallel to any Bend principle planes)
        for ii in 0..=SMP_SIZE {
            let u = ivl_u.evaluate(f64::from(ii) / f64::from(SMP_SIZE));
            for jj in 0..=SMP_SIZE {
                let uv = Point2::new(u, ivl_v.evaluate(f64::from(jj) / f64::from(SMP_SIZE)));

                // Surface sample (status ignored)
                if let Some(p) = ignored(input.evaluate_point(k, uv))? {
                    input_pt = p;
                }
                let input_vec = input_pt - input_origin;

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
                    out_space_bbox.add_point(out_space_pt - bend_origin);
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

                // distances to the planes through the origin perpendicular to the axes
                let mid_plane = bend_mid_dir.dot(input_vec);
                let binormal_plane = bend_binormal.dot(input_vec);
                let axis_plane = bend_axis.dot(input_vec);

                // accumulate the ranges
                mid_dir_plane_range.add_value(mid_plane);
                axis_plane_range.add_value(axis_plane);
                binormal_plane_range.add_value(binormal_plane);

                // accumulate the sums of plane distances
                axis_plane_sum += axis_plane;
                mid_plane_sum += mid_plane;
                binormal_plane_sum += binormal_plane;

                // remember when the surface is a constant distance from a bend plane
                const_axis_plane = axis_plane_range.length() < approx_tol;
                const_mid_dir_plane = mid_dir_plane_range.length() < approx_tol;
                const_binormal_plane = binormal_plane_range.length() < approx_tol;

                // no work - Surface not const distance from at least one plane
                if u8::from(const_axis_plane)
                    + u8::from(const_mid_dir_plane)
                    + u8::from(const_binormal_plane)
                    < 1
                {
                    return Ok(None);
                }
            }
        }

        // find the current input surface normal - it's a plane so sample it once anywhere
        // (status ignored)
        let mid_uv = Point2::new(ivl_u.evaluate(0.5), ivl_v.evaluate(0.5));
        let input_surface_normal =
            ignored(input.evaluate_normal(k, mid_uv, Side::Left, Side::Left))?
                .unwrap_or(Vec3::UNDEF);
        let samples = f64::from(SMP_SIZE) + 1.0;
        let mut new_surface = None;

        // surfaces perpendicular to the BendAxis map to planes with the same orientation
        // perpendicular to the BendAxis
        if const_axis_plane {
            let origin = bend_origin + axis_plane_sum / samples / samples * bend_axis;
            let uv_scale = Vec2::new(1.0, 1.0);

            // check orientation of surface against the Input BendAxis axis
            let positive = input_surface_normal.dot(input_axis) > 0.0;

            // UVDomain = BBox3d U (BendMidDir) and V (BendBiNormal) extents
            let [box_u, box_v, _] = *out_space_bbox.intervals();
            let mut uv_domain =
                Extent2d::from_bounds(box_u.min(), box_v.min(), box_u.max(), box_v.max());

            // bump BBox for maximums not in the corners (statuses ignored)
            let ang_range_rad = Extent1d::from_min_max(
                binormal_plane_range.min() / bend_neutral_dist,
                binormal_plane_range.max() / bend_neutral_dist,
            );
            let mid_max = mid_dir_plane_range.max();
            if ang_range_rad.contains_value(-PI, EFF_ZERO_DEG) {
                let _ = uv_domain.set_u_min(-mid_max);
            }
            if ang_range_rad.contains_value(-PI / 2.0, EFF_ZERO_DEG) {
                let _ = uv_domain.set_v_min(-mid_max);
            }
            if ang_range_rad.contains_value(0.0, EFF_ZERO_DEG) {
                let _ = uv_domain.set_u_max(mid_max);
            }
            if ang_range_rad.contains_value(PI / 2.0, EFF_ZERO_DEG) {
                let _ = uv_domain.set_v_max(mid_max);
            }
            if ang_range_rad.contains_value(PI, EFF_ZERO_DEG) {
                let _ = uv_domain.set_u_min(-mid_max);
            }

            // Create Plane with SrfNormal = input surface normal
            let (x_vector, y_vector) = if positive {
                (bend_mid_dir, bend_binormal)
            } else {
                uv_domain.transpose();
                (bend_binormal, bend_mid_dir)
            };

            // create new plane - positive orientation:[SrfNormal = BendMidDir X BendBinormal]
            new_surface = Some(Surface::from(Plane::new(
                origin, x_vector, y_vector, uv_scale, &uv_domain,
            )));
        }

        // surfaces perpendicular to the MidDir plane map to BendAxis centered cylinders
        if const_mid_dir_plane {
            let radius = mid_plane_sum / samples / samples;
            let axis_length = axis_plane_range.length();
            let start_angle_deg = rad_to_deg(binormal_plane_range.min() / bend_neutral_dist);
            let stop_angle_deg = rad_to_deg(binormal_plane_range.max() / bend_neutral_dist);

            // check orientation of surface against the Input BendMidDir axis
            let positive = input_surface_normal.dot(input_mid_dir) > 0.0;

            // FALSE = outward pointing SrfNormals, TRUE = inward pointing SrfNormals
            let inside_out = !positive;

            new_surface = Some(Surface::from(Cone::new_cylinder_bounded(
                bend_origin,
                bend_mid_dir,
                bend_binormal,
                radius,
                start_angle_deg,
                stop_angle_deg,
                axis_length,
                false,
                inside_out,
                false,
            )));
        }

        // surfaces perpendicular to the Binormal Axis map to planes containing the
        // BendAxis (radiating out)
        if const_binormal_plane {
            let ang_rad = (binormal_plane_sum / samples / samples) / bend_neutral_dist;
            let mut x_vector = bend_axis;
            let mut y_vector = ang_rad.cos() * bend_mid_dir + ang_rad.sin() * bend_binormal;

            let uv_scale = Vec2::new(1.0, 1.0);
            let mut uv_domain = Extent2d::from_bounds(
                axis_plane_range.min(),
                mid_dir_plane_range.min(),
                axis_plane_range.max(),
                mid_dir_plane_range.max(),
            );

            // check orientation of surface against the Input BendBinormal axis
            let positive = input_surface_normal.dot(bend_binormal) > 0.0;

            // Create Plane with SrfNormal = input surface normal
            if !positive {
                x_vector = y_vector;
                y_vector = bend_axis;
                uv_domain.transpose();
            }

            // create new plane - positive orientation:[SrfNormal = sBendAxis X sYVector]
            new_surface = Some(Surface::from(Plane::new(
                bend_origin,
                x_vector,
                y_vector,
                uv_scale,
                &uv_domain,
            )));
        }

        // all done
        Ok(new_surface)
    }

    /// `MakeExactBSpline1stOutCurve`: the exact first out-space image of `input` (from
    /// in-space when `in_space`, else from parameter space) through the bend `volume`
    /// (whose kind is `self`), or `None` when it is not one of the analytic cases. A
    /// line parallel to the bend axis or to the mid direction maps to a line, one
    /// parallel to the binormal to a circular arc about the bend axis. The test samples
    /// 12 points of the natural interval; any sample outside the bend's domain gives
    /// `None`.
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
        // locals: ParamSpace and InSpace Bend orientations - Bend's Neutral plane is perp
        // to the Bend's MidDirection
        let (param_origin, bend_origin) = (Vec3::ZERO, volume.bend_origin());
        let (param_mid_dir, bend_mid_dir) = (Vec3::X, volume.bend_mid_dir());
        let (param_axis, bend_axis) = (Vec3::Z, volume.bend_axis());
        let (param_binormal, bend_binormal) = (Vec3::Y, volume.bend_binormal());

        // locals: making InputSpace either ParamSpace or InSpace
        let (input_origin, input_mid_dir, input_axis, input_binormal) = if in_space {
            (bend_origin, bend_mid_dir, bend_axis, bend_binormal)
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

        let mut axis_plane_range = Extent1d::new();
        let mut mid_dir_plane_range = Extent1d::new();
        let mut binormal_plane_range = Extent1d::new();
        let mut const_axis_plane = false;
        let mut const_mid_dir_plane = false;
        let mut const_binormal_plane = false;

        // See if the InputCurve is linear and parallel to one of the BendAxes (parallel
        // to two BendPlanes)
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

            // InputSpace distances to planes perp to Bend axes
            let axis_plane = input_axis.dot(on_axis_vec);
            let mid_dir_plane = input_mid_dir.dot(on_axis_vec);
            let binormal_plane = input_binormal.dot(on_axis_vec);

            // accumulate the range of Plane distances
            axis_plane_range.add_value(axis_plane);
            mid_dir_plane_range.add_value(mid_dir_plane);
            binormal_plane_range.add_value(binormal_plane);

            // remember when curve is const dist from bend plane
            const_axis_plane = axis_plane_range.length() < approx_tol;
            const_mid_dir_plane = mid_dir_plane_range.length() < approx_tol;
            const_binormal_plane = binormal_plane_range.length() < approx_tol;

            // no work - curve not const distance from at least two planes
            if u8::from(const_axis_plane)
                + u8::from(const_mid_dir_plane)
                + u8::from(const_binormal_plane)
                < 2
            {
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

        let mut new_curve = None;

        // lines parallel to BendAxis => map to OutSpace lines
        // lines parallel to MidBendAxis => map to OutSpace lines
        let parallel_to_axis = const_mid_dir_plane && const_binormal_plane;
        let parallel_to_mid_dir = const_axis_plane && const_binormal_plane;
        if parallel_to_axis || parallel_to_mid_dir {
            // create the OutSpace Line
            new_curve = Some(Curve::from(Line::from_points(
                out_space_first,
                out_space_last,
                3,
            )));
        }

        // lines parallel to BinormalAxis => map to OutSpace circles
        if const_mid_dir_plane && const_axis_plane {
            // OutSpace bend parameters
            let out_space_mid_dir = volume.bend_mid_dir();
            let out_space_axis = volume.bend_axis();
            let out_space_binormal = volume.bend_binormal();

            // find the current rInputCurve curve tangent - it's a line so sample it once
            // anywhere (status ignored)
            let input_tangent = ignored(input.evaluate(k, ivl.evaluate(0.5), 1, true, true))?
                .and_then(|pn| pn.get(1).copied())
                .unwrap_or(Vec3::UNDEF);

            // check orientation of curve against the Input BinormalAxis axis and select
            // appropriate coordinate system
            let positive = input_tangent.dot(input_binormal) > 0.0;
            let (x_axis, y_axis, z_axis) = if positive {
                (out_space_mid_dir, out_space_binormal, out_space_axis)
            } else {
                (out_space_binormal, out_space_mid_dir, -out_space_axis)
            };

            // OutSpace circle circ center and radius (status ignored)
            let out_space_on_axis = ignored(out_point(on_axis_pt))?.unwrap_or(Vec3::UNDEF);

            let radius1 = (out_space_first - out_space_on_axis).length();
            let radius2 = (out_space_last - out_space_on_axis).length();
            let radius = (radius1 + radius2) / 2.0;

            // Start and End AngDeg
            // (statuses ignored: a failure leaves 0)
            let start_ang_rad =
                ignored(z_axis.ccw_angle_between(x_axis, out_space_first - out_space_on_axis))?
                    .unwrap_or(0.0);
            let end_ang_rad =
                ignored(z_axis.ccw_angle_between(x_axis, out_space_last - out_space_on_axis))?
                    .unwrap_or(0.0);

            let mut ang_domain_deg = Extent1d::new();
            ang_domain_deg.add_value(rad_to_deg(start_ang_rad));
            ang_domain_deg.add_value(rad_to_deg(end_ang_rad));

            // create the OutSpace Circular Arc
            new_curve = Some(Curve::from(Ellipse::new_circle(
                k,
                out_space_on_axis,
                x_axis,
                y_axis,
                ang_domain_deg,
                radius,
                &EllipseOptions::default(),
            )?));
        }

        // all done
        Ok(new_curve)
    }

    /// Reads a bend volume from a database stream: the neutral distance and the
    /// singularity tolerance, then the volume base data
    /// ([`Volume::read_base_from_db`], status ignored as upstream) (`ReadFromDB`).
    ///
    /// # Errors
    ///
    /// A failed binary read; the base read's undefined cases
    /// ([`SolidError::Unsupported`]), its nesting bound and [`SolidError::Unported`].
    pub fn read_from_db(db: &mut DatabaseReader, version: u64) -> Result<Volume> {
        // a new SmBendVolume() (neutral distance 0, tolerance SM_UNDEF_DOUBLE)
        let mut bend = Self::new();
        if db.file_type() == FileType::Ascii {
            for slot in [&mut bend.bend_neutral_dist, &mut bend.singularity_tol] {
                if let Some(stream) = db.in_stream_mut() {
                    if let Some(v) = stream.extract_double() {
                        *slot = v;
                    }
                }
                db.go_to_next_line();
            }
        } else {
            bend.bend_neutral_dist = db.read_double()?;
            bend.singularity_tol = db.read_double()?;
        }
        let mut volume = Volume::new(VolumeKind::Bend(bend));
        // read base class data (status ignored)
        ignore_read(volume.read_base_from_db(db, version))?;
        Ok(volume)
    }
}

impl Volume {
    /// The in-space point on the bend axis at `w = 0`: the orient origin
    /// (`GetBendOrigin`).
    #[must_use]
    pub fn bend_origin(&self) -> Vec3 {
        self.orient_origin()
    }

    /// The in-space bend axis, the parameter `w` direction: the orient z axis
    /// (`GetBendAxis`).
    #[must_use]
    pub fn bend_axis(&self) -> Vec3 {
        self.orient_z_axis()
    }

    /// The in-space mid direction, the parameter `u` direction: the orient x axis
    /// (`GetBendMidDir`).
    #[must_use]
    pub fn bend_mid_dir(&self) -> Vec3 {
        self.orient_x_axis()
    }

    /// The in-space binormal `axis × mid_dir`, the parameter `v` direction: the
    /// orient y axis (`GetBendBinormal`).
    #[must_use]
    pub fn bend_binormal(&self) -> Vec3 {
        self.orient_y_axis()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::solution_array::ObjectKind;

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

    fn bend(d: f64) -> BendVolume {
        BendVolume::from_members(d, 0.01)
    }

    #[test]
    fn simple_map_rolls_planes() {
        let b = bend(2.0);
        // a quarter of the neutral circle at radius 3
        let d = b.evaluate_simple(v(3.0, PI, 1.0), 0).unwrap();
        assert!(close(d[0], v(0.0, 3.0, 1.0)));
        let (g, ok) = b.inv_evaluate_guess_point_simple(d[0]);
        assert!(ok);
        assert!(close(g[0], v(3.0, PI, 1.0)));
        assert_eq!(b.evaluate_simple(v(1.0, 0.0, 0.0), 3).unwrap().len(), 64);
        assert!(b.evaluate_simple(v(1.0, 0.0, 0.0), 4).is_err());
    }

    #[test]
    fn simple_derivatives_match_differences() {
        let b = bend(1.5);
        let p = v(0.7, -0.4, 0.9);
        let d = b.evaluate_simple(p, 3).unwrap();
        let h = 1e-6;
        let pos = |q: Vec3| b.evaluate_simple(q, 0).unwrap()[0];
        let du = (pos(p + v(h, 0.0, 0.0)) - pos(p - v(h, 0.0, 0.0))) / (2.0 * h);
        assert!((d[16] - du).length() < 1e-6);
        let dv = (pos(p + v(0.0, h, 0.0)) - pos(p - v(0.0, h, 0.0))) / (2.0 * h);
        assert!((d[4] - dv).length() < 1e-6);
        // Dv at n = 1 is index 2; its v derivative is Dvv, its u derivative Duv
        let first = |q: Vec3| b.evaluate_simple(q, 1).unwrap()[2];
        let dvv = (first(p + v(0.0, h, 0.0)) - first(p - v(0.0, h, 0.0))) / (2.0 * h);
        assert!((d[8] - dvv).length() < 1e-5);
        let duv = (first(p + v(h, 0.0, 0.0)) - first(p - v(h, 0.0, 0.0))) / (2.0 * h);
        assert!((d[20] - duv).length() < 1e-5);
    }

    #[test]
    fn create_repairs_input_and_evaluates_in_frame() {
        let vol = BendVolume::create(
            v(1.0, 2.0, 3.0),
            v(0.0, 0.0, 2.0),
            v(3.0, 0.0, 0.0),
            2.0,
            0.0,
        );
        let VolumeKind::Bend(b) = vol.kind() else {
            panic!("not a bend")
        };
        assert_eq!(b.singularity_tol(), ZONE_TOL_3D);
        assert_eq!(b.bend_neutral_dist(), 2.0);
        assert!(close(vol.bend_axis(), v(0.0, 0.0, 1.0)));
        assert!(close(vol.bend_mid_dir(), v(1.0, 0.0, 0.0)));
        assert!(close(vol.bend_binormal(), v(0.0, 1.0, 0.0)));
        assert!(close(vol.bend_origin(), v(1.0, 2.0, 3.0)));
        let d = b
            .evaluate(&vol, v(2.0, PI, 2.0), 3, &EvalOptions::default())
            .unwrap();
        assert!(close(d[0], v(1.0, 4.0, 5.0)));
        assert_eq!(d.len(), 64);
        assert!(b
            .evaluate(&vol, v(2.0, PI, 2.0), 4, &EvalOptions::default())
            .is_err());
        // the frame form agrees with the simple map through the orient map
        let s = vol
            .evaluate_simple(v(2.0, PI, 2.0), 0, &EvalOptions::default())
            .unwrap();
        assert!(close(s[0] + v(1.0, 2.0, 3.0), d[0]));

        // a tiny neutral distance becomes 1, a parallel mid direction is replaced
        let vol = BendVolume::create(Vec3::ZERO, Vec3::Z, v(0.0, 0.0, 5.0), 0.0, 0.5);
        let VolumeKind::Bend(b) = vol.kind() else {
            panic!("not a bend")
        };
        assert_eq!(b.bend_neutral_dist(), 1.0);
        assert_eq!(b.singularity_tol(), 0.5);
        assert!(vol.bend_axis().dot(vol.bend_mid_dir()).abs() < 1e-12);
    }

    #[test]
    fn domain_predicates() {
        let b = bend(1.0);
        assert!(b.is_bounded_simple());
        let dom = b.natural_param_domain();
        assert_eq!(dom.u_min(), 0.01);
        assert_eq!(dom.v_max(), PI);
        assert!(b.is_point_in_param_domain_simple(v(1.0, 3.0, -7.0)));
        assert!(b.is_point_in_param_domain_simple(v(1.0, PI, 0.0)));
        assert!(!b.is_point_in_param_domain_simple(v(1.0, 3.2, 0.0)));
        assert!(!b.is_point_in_param_domain_simple(v(0.005, 0.0, 0.0)));
        assert!(b.is_line_in_param_domain_simple(v(1.0, -3.0, 0.0), v(2.0, 3.0, 1.0)));
        assert!(!b.is_line_in_param_domain_simple(v(1.0, -3.0, 0.0), v(-2.0, 3.0, 1.0)));
        let k = Kernel::default();
        let seg = |a, c| Curve::from(Line::from_points(a, c, 3));
        assert!(b
            .is_curve_in_param_domain_simple(&k, &seg(v(1.0, 1.0, 0.0), v(2.0, 1.0, 0.0)))
            .unwrap());
        assert!(!b
            .is_curve_in_param_domain_simple(&k, &seg(v(1.0, 1.0, 0.0), v(-2.0, 1.0, 0.0)))
            .unwrap());
    }

    #[test]
    fn trim() {
        let b = bend(1.0);
        let mut t = Extent3d::new();
        let pb = Extent3d::from_bounds(-1.0, -1.0, 0.0, 2.0, 1.0, 1.0);
        b.trim_param_bounding_box_simple(&pb, &mut t).unwrap();
        assert_eq!(t, Extent3d::from_bounds(0.01, -1.0, 0.0, 2.0, 1.0, 1.0));
        // entirely below the tolerance or beyond the period
        let below = Extent3d::from_bounds(-1.0, -1.0, 0.0, 0.0, 1.0, 1.0);
        assert!(b.trim_param_bounding_box_simple(&below, &mut t).is_err());
        assert_eq!(t, Extent3d::new());
        let beyond = Extent3d::from_bounds(1.0, 4.0, 0.0, 2.0, 5.0, 1.0);
        assert!(b.trim_param_bounding_box_simple(&beyond, &mut t).is_err());
        // an end beyond the period wraps; the inverted result leaves the box alone
        let across = Extent3d::from_bounds(1.0, 0.0, 0.0, 2.0, 4.0, 1.0);
        let mut t = pb;
        b.trim_param_bounding_box_simple(&across, &mut t).unwrap();
        assert_eq!(t, pb);
    }

    #[test]
    fn bounding_boxes() {
        let b = bend(1.0);
        // a quarter shell in the first quadrant
        let pb = Extent3d::from_bounds(1.0, 0.0, 0.0, 2.0, PI / 2.0, 1.0);
        let mut bx = Extent3d::new();
        let mut pseudo = PseudoBox::new();
        b.evaluate_bounding_box_simple(&pb, Some(&mut bx), Some(&mut pseudo));
        assert!((bx.u_max() - 2.0).abs() < 1e-12 && bx.u_min().abs() < 1e-12);
        assert!((bx.v_max() - 2.0).abs() < 1e-12 && bx.v_min().abs() < 1e-12);
        assert_eq!((bx.w_min(), bx.w_max()), (0.0, 1.0));
        assert!(close(
            pseudo.basis()[0],
            v(0.5f64.sqrt(), 0.5f64.sqrt(), 0.0)
        ));
        // u reaching SM_INFINITE_PARAMETER
        let inf = Extent3d::from_bounds(1.0, 0.1, 0.0, INFINITE_PARAMETER, 0.2, 1.0);
        b.evaluate_bounding_box_simple(&inf, Some(&mut bx), None);
        assert_eq!(bx.u_max(), INFINITE_PARAMETER);
        assert_eq!(bx.v_max(), INFINITE_PARAMETER);
        // no output requested
        b.evaluate_bounding_box_simple(&pb, None, None);
    }

    #[test]
    fn solvers() {
        let b = bend(2.0);
        let mut sols = SolutionArray::new();
        assert!(b.global_point_solve_simple(v(0.0, 3.0, 0.5), &mut sols, Vol));
        assert_eq!(sols.len(), 1);
        let s = sols.get(0).unwrap();
        assert_eq!(s.solution_type, SolutionType::SingleValue);
        assert!((s.start.parameters[1] - PI).abs() < 1e-12);
        // on the seam: two solutions, the local solve picks the closer one
        assert!(b.global_point_solve_simple(v(-3.0, 0.0, 0.5), &mut sols, Vol));
        assert_eq!(sols.len(), 2);
        let mut sol = Solution::new();
        assert!(b.local_point_solve_simple(v(-3.0, 0.0, 0.5), v(3.0, -6.0, 0.5), &mut sol, Vol));
        assert!((sol.start.parameters[1] + 2.0 * PI).abs() < 1e-12);
        assert_eq!(sol.objects[0], Some(Vol));
        // near the axis the guess fails but is still written
        let (g, ok) = b.inv_evaluate_guess_point_simple(v(0.001, 0.0, 1.0));
        assert!(!ok);
        assert_eq!(g, vec![v(0.001, 0.0, 1.0)]);
    }

    #[test]
    fn equality_and_helpers() {
        let a = BendVolume::from_members(1.0, 0.5);
        assert!(a.members_eq(&BendVolume::from_members(1.0, 0.5)));
        assert!(!a.members_eq(&BendVolume::from_members(2.0, 0.5)));
        assert!(!a.members_eq(&BendVolume::from_members(1.0, 0.6)));
        assert!(is_kind_of(BEND_VOLUME_TYPE));
        assert_eq!(BendVolume::new().bend_neutral_dist(), 0.0);
        assert_eq!(bend(1.0).legal_min_u(), 0.01 - EFF_ZERO);
        assert_eq!(clamp_to_period(4.0, -PI, PI), 4.0 - 2.0 * PI);
    }

    #[test]
    fn read_from_db_reads_the_base_data() {
        let text = "2.5\n0.01\n1\nObject Type 22008\n1 0 0 0\n0 1 0 0\n0 0 1 0\n1 2 3 1\n0\n0\n0\n";
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        let vol = BendVolume::read_from_db(&mut db, 30).unwrap();
        let VolumeKind::Bend(b) = vol.kind() else {
            panic!("not a bend")
        };
        assert_eq!(b.bend_neutral_dist(), 2.5);
        assert_eq!(b.singularity_tol(), 0.01);
        assert!(vol.orient_map().is_none());
        let next = vol.next_map().unwrap();
        assert!(matches!(next.kind(), VolumeKind::Transform(_)));
        assert!(next.next_map().is_none());
    }
}

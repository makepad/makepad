//! The twist volume `SmTwistVolume`: a map that rotates the planes normal to the
//! twist axis at a constant rate along it, like a twisted bar.
//!
//! Upstream: `source/SMLib/src/SmTwistVolume.cpp`, `source/SMLib/inc/SmTwistVolume.h`.
//!
//! The simple map takes the parameter point `(u, v, w)` to
//! `(cos(a) u - sin(a) v, sin(a) u + cos(a) v, w)` with `a = 2 PI w / d`, `d` the
//! twist period distance (negative for a left hand twist). The orient map places the
//! twist frame (origin, mid direction, binormal, axis) in in-space.
//!
//! | Rust                                              | Upstream                                       |
//! |---------------------------------------------------|------------------------------------------------|
//! | [`TwistVolume`]                                   | `SmTwistVolume` (members `m_dTwistPeriodDist`, `m_dPeriodMinDist`) |
//! | [`TwistVolume::new`], `Default`                   | `SmTwistVolume(const SmContext*)`              |
//! | [`TwistVolume::create`]                           | `SmTwistVolume(crContext, Origin, Axis, MidDir, PeriodDist, PeriodMinDist)` |
//! | [`Volume::copy`] (twist arm)                      | `SmTwistVolume(const SmTwistVolume&, SmBoolean)`, `Copy` |
//! | `Drop` (implicit)                                 | `~SmTwistVolume`                               |
//! | [`Volume::approx_eq`] (twist arm), [`TwistVolume::members_eq`] | `operator==`                      |
//! | [`TwistVolume::natural_param_domain`]             | `GetNaturalParamDomain`                        |
//! | [`Volume::twist_origin`], [`Volume::twist_axis`], [`Volume::twist_mid_dir`], [`Volume::twist_binormal`] | `GetTwistOrigin`, `GetTwistAxis`, `GetTwistMidDir`, `GetTwistBinormal` |
//! | [`TwistVolume::twist_period_dist`], [`TwistVolume::period_min_dist`] | `m_dTwistPeriodDist`, `m_dPeriodMinDist` (`GetTwistPeriodDist`, `GetPeriodMinDist`) |
//! | [`TwistVolume::evaluate`]                         | `Evaluate`                                     |
//! | [`TwistVolume::evaluate_simple`]                  | `EvaluateSimple`                               |
//! | [`TwistVolume::evaluate_bounding_box_simple`]     | `EvaluateBoundingBoxSimple`                    |
//! | [`TwistVolume::inv_evaluate_guess_point_simple`]  | `InvEvaluateGuessPointSimple`                  |
//! | [`TwistVolume::global_point_solve_simple`]        | `GlobalPointSolveSimple`                       |
//! | [`TwistVolume::local_point_solve_simple`]         | `LocalPointSolveSimple`                        |
//! | [`TwistVolume::is_bounded_simple`]                | `IsBoundedSimple`                              |
//! | [`TwistVolume::is_point_in_param_domain_simple`]  | `IsPointInParamDomainSimple`                   |
//! | [`TwistVolume::is_line_in_param_domain_simple`]   | `IsLineInParamDomainSimple`                    |
//! | [`TwistVolume::is_curve_in_param_domain_simple`]  | `IsCurveInParamDomainSimple`                   |
//! | [`TwistVolume::read_from_db`]                     | `ReadFromDB`                                   |
//! | [`TwistVolume::make_exact_b_spline_1st_out_surface`] | `MakeExactBSpline1stOutSurface` (in no unit in units.json; called by `SrfInVolume::make_exact_b_spline_if_possible_with`) |
//! | [`TwistVolume::make_exact_b_spline_1st_out_curve`] | `MakeExactBSpline1stOutCurve` (in no unit in units.json; called by `CrvInVolume::make_exact_b_spline_if_possible_with`) |
//! | [`TWIST_VOLUME_TYPE`], [`is_kind_of`], [`Volume::type_code`] and friends | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`), `IsKindOf` |
//!
//! Deviations:
//! - **Volume base.** The twist is the [`VolumeKind::Twist`] variant of a
//!   [`Volume`], which holds the orient and next maps; the virtual overrides are
//!   dispatched from [`Volume`] (`surface::volume::p1`). [`TwistVolume::evaluate`]
//!   takes that volume for its orient frame and next map.
//! - **Constructor.** The component constructor is not in the unit's function list
//!   (nothing else owns it) but is ported as [`TwistVolume::create`] because it is
//!   the only way upstream builds a usable twist. Its zero-vector repair compares
//!   `Unitize`'s `SM_ERR_INVALID_INPUT` with `SM_ERR`, so the first two repair
//!   branches never run; ported as it behaves (`plan/findings.md`). It also resets
//!   a too-small period to `|dPeriodMinDist|` as given (not the clamped minimum).
//!   Contexts and the `SE_MSG` reports are dropped.
//! - **Default construction.** A default-constructed twist has period `0` and
//!   minimum `SM_UNDEF_DOUBLE` (the member initializer) ([`TwistVolume::new`]); its
//!   maps divide by zero (NaN / infinite values, as upstream).
//! - **`operator==`** assigns the period comparison and then overwrites it with the
//!   minimum-distance comparison; only the latter decides (kept,
//!   [`TwistVolume::members_eq`]).
//! - **`EvaluateSimple`'s `Duw`** is `(-per sin a, -per cos a, 0)` where the header
//!   documents `(-per sin a, per cos a, 0)`; `Evaluate` uses the documented form.
//!   Both are kept as written.
//! - **Bounding boxes.** `EvaluateBoundingBoxSimple` keeps upstream's optional
//!   outputs as `Option<&mut _>`; the unbounded branch writes the axis-aligned box
//!   unconditionally, so a request for the pseudo box alone is
//!   [`SolidError::InvalidInput`] there (upstream dereferences the null pointer).
//!   The period count `(long)(theta / 2 PI)` of an out-of-range or NaN quotient is
//!   x86-64's `LONG_MIN` (upstream undefined behaviour), emulated by `trunc_long`.
//!   The parameter pseudo box is unused upstream and dropped.
//! - **Solvers.** `GlobalPointSolveSimple` / `LocalPointSolveSimple` return
//!   upstream's `rbFoundAnswer` (their status is always `SM_SUCCESS`) and update the
//!   caller's solutions in place exactly as upstream; the volume itself
//!   (`m_apObjects[0] = this`) is the caller's object handle. The unused parameter
//!   guess of the local solve is dropped.
//! - **`ReadFromDB`** reads the two members, then the base data
//!   ([`Volume::read_base_from_db`], status ignored as upstream). Filling a given
//!   object (`rpNewVolume != NULL`) is not modelled; a new volume is always made.
//! - **Unused arguments** upstream ignores (the from-left flags, zero-tangent and
//!   zero-sampling options of the evaluations, the curve of
//!   `IsCurveInParamDomainSimple`, the line ends of `IsLineInParamDomainSimple`, the
//!   point of `IsPointInParamDomainSimple`) are dropped where the call is local to
//!   this module.
//! - **`MakeExactBSpline1stOutCurve`** takes the input space as a flag, as the surface
//!   variant, and returns the new curve. A failed sample `EvaluatePoint` (status
//!   ignored) leaves the previous point ([`Vec3::UNDEF`] at first); the copy's
//!   `Transform` status is ignored; a failed `Copy` (upstream then dereferences `NULL`)
//!   is returned; the copy is the input's own kind (upstream casts it to
//!   `SmBSplineCurve*`). The unused plane-distance sum and the debug drawing are
//!   dropped.
//! - **`MakeExactBSpline1stOutSurface`** takes the input space as a flag
//!   (`in_space`: `SM_VS_IN_SPACE`; otherwise `SM_VS_PARAM_SPACE`, so the invalid-space
//!   `SM_ERR` has no counterpart) and returns the new surface (`None` for upstream's
//!   `NULL`). A failed sample `EvaluatePoint` (status ignored) leaves the previous
//!   point ([`Vec3::UNDEF`] at first), as upstream's out-param; the copy's
//!   `Transform` status is ignored; a failed `Copy` (upstream then dereferences
//!   `NULL`) is returned; [`SolidError::Unported`] propagates. The copy is the input's
//!   own kind (upstream casts it to `SmBSplineSurface*` whatever it is).

use crate::curve::curve::p2::Curve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{Solution, SolutionArray, SolutionObject, SolutionType};
use crate::geom::tol::{approx_tol_3d, SmallTopoType};
use crate::geom::tol::{is_zero, ZONE_TOL_3D};
use crate::geom::transform::Transform;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max4, EFF_ZERO_DEG, INFINITE_PARAMETER, PI, UNDEF_DOUBLE};
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

pub use crate::surface::volume::p1::TWIST_VOLUME_TYPE;

const OP_EVAL: &str = "surface::twist_volume::TwistVolume::evaluate";
const OP_EVAL_SIMPLE: &str = "surface::twist_volume::TwistVolume::evaluate_simple";
const OP_BOX: &str = "surface::twist_volume::TwistVolume::evaluate_bounding_box_simple";

/// The highest derivative the twist evaluations support.
pub const MAX_DERIV: usize = 3;

/// `SmTwistVolume::IsKindOf`: `SmTwistVolume_TYPE` or a base class's type.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TWIST_VOLUME_TYPE || volume_is_kind_of(t)
}

/// `SM_IS_CONTAINED_TO_TOL(c, a, b, tol)`: `c` in `[a, b]` (either order) grown by
/// `tol`.
fn is_contained_to_tol(c: f64, a: f64, b: f64, tol: f64) -> bool {
    ((a <= c + tol) && (c - tol <= b)) || ((b <= c + tol) && (c - tol <= a))
}

/// `(double)(long)x` on x86-64: truncation toward zero, `LONG_MIN` for NaN and
/// out-of-range values (undefined behaviour upstream).
fn trunc_long(x: f64) -> f64 {
    // the range of a 64-bit long: [-2^63, 2^63)
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if (-LIMIT..LIMIT).contains(&x) {
        x.trunc()
    } else {
        -LIMIT
    }
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

/// The members of `SmTwistVolume`; its orient map (the twist frame) and any next
/// maps live in the [`Volume`] holding it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwistVolume {
    /// `m_dTwistPeriodDist`: the distance along the axis for one full turn
    /// (negative for a left hand twist).
    twist_period_dist: f64,
    /// `m_dPeriodMinDist`: the smallest allowed `|twist_period_dist|`.
    period_min_dist: f64,
}

impl Default for TwistVolume {
    fn default() -> Self {
        Self::new()
    }
}

impl TwistVolume {
    /// The default-constructed twist (`SmTwistVolume(const SmContext*)`): period
    /// `0` and minimum `SM_UNDEF_DOUBLE` (its member initializer).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            twist_period_dist: 0.0,
            period_min_dist: UNDEF_DOUBLE,
        }
    }

    /// A twist with the given members as stored, unchecked (the state
    /// `ReadFromDB` builds).
    #[must_use]
    pub const fn from_members(twist_period_dist: f64, period_min_dist: f64) -> Self {
        Self {
            twist_period_dist,
            period_min_dist,
        }
    }

    /// The twist volume about the in-space line through `origin` along `axis`, the
    /// parameter `u` axis along `mid_dir`, one turn every `twist_period_dist`
    /// (the component constructor). Bad input is repaired, never rejected: the
    /// minimum is raised to `SM_ZONE_TOL_3D`, a period shorter than it becomes
    /// `|period_min_dist|`, zero vectors are replaced by a frame about the axis and
    /// a mid direction not perpendicular to the axis is projected (or, when
    /// parallel, replaced).
    #[must_use]
    pub fn create(
        origin: Point3,
        axis: Vec3,
        mid_dir: Vec3,
        twist_period_dist: f64,
        period_min_dist: f64,
    ) -> Volume {
        let mut twist = Self::from_members(twist_period_dist, period_min_dist);

        // check input - dPeriodMinDist >= SM_ZONE_TOL_3D
        if twist.period_min_dist < ZONE_TOL_3D {
            twist.period_min_dist = ZONE_TOL_3D;
        }

        // check input - Positive Definite TwistPeriodDistance
        if twist_period_dist.abs() < twist.period_min_dist {
            // do the best we can (upstream uses the argument, not the clamped member)
            twist.twist_period_dist = if period_min_dist >= 0.0 {
                period_min_dist
            } else {
                -period_min_dist
            };
        }

        // unitize twist vectors
        let (mut twist_axis, ok1) = unitize(axis);
        let (mut twist_mid_dir, ok2) = unitize(mid_dir);

        // check input - nonZero input vectors
        if !ok1 || !ok2 {
            // Upstream compares the failed statuses (SM_ERR_INVALID_INPUT) with SM_ERR,
            // so only its last branch runs: a frame about the (repaired) axis.
            if let Ok([x, y, _]) = twist_axis.make_unit_ortho_vectors(None) {
                twist_axis = x;
                twist_mid_dir = y;
            }
        }

        // check input - twist vectors must be perpendicular to one another
        if !twist_axis.is_perpendicular_to(twist_mid_dir, EFF_ZERO_DEG) {
            if twist_axis.is_parallel_to(twist_mid_dir, EFF_ZERO_DEG) {
                // Axis and StartDir don't span a plane - do the best we can
                if let Ok([x, _, z]) = twist_mid_dir.make_unit_ortho_vectors(None) {
                    twist_mid_dir = x;
                    twist_axis = z;
                }
            } else {
                twist_mid_dir = twist_axis.cross(twist_mid_dir.cross(twist_axis));
                twist_mid_dir = unitize(twist_mid_dir).0;
            }
        }

        // set TwistBinormal perp to both TwistAxis and TwistMidDir
        let twist_binormal = twist_axis.cross(twist_mid_dir);

        // build TwistCoordinates into m_pOrientMap
        let orient = Transform::new(origin, twist_mid_dir, twist_binormal, Some(twist_axis));
        Volume::oriented(VolumeKind::Twist(twist), Some(orient))
    }

    /// `m_dTwistPeriodDist` (`GetTwistPeriodDist`).
    #[must_use]
    pub const fn twist_period_dist(&self) -> f64 {
        self.twist_period_dist
    }

    /// `m_dPeriodMinDist` (`GetPeriodMinDist`).
    #[must_use]
    pub const fn period_min_dist(&self) -> f64 {
        self.period_min_dist
    }

    /// The member part of `operator==`: upstream overwrites the period comparison
    /// with the minimum-distance comparison, so only `SM_IS_ZERO` of the minima
    /// decides.
    #[must_use]
    pub fn members_eq(&self, other: &Self) -> bool {
        // (upstream's period comparison is overwritten before it is read)
        is_zero(self.period_min_dist - other.period_min_dist)
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

    /// `2 PI / d`, the turn rate (negative for left hand twists).
    fn per(&self) -> f64 {
        2.0 * PI / self.twist_period_dist
    }

    /// Out-space position and partial derivatives `0..=n` (`n <= 3`) of the parameter
    /// point, `D[u][v][w]` (`Evaluate`): the twist evaluated directly in the in-space
    /// frame of `volume`'s orient map, then compounded through its next maps when
    /// `opts.with_compounding`. `volume` is the volume holding this twist.
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
        let n1 = n + 1;
        let sv = |u: usize, v: usize, w: usize| (u * n1 + v) * n1 + w;

        // locals
        let du = param_point.x;
        let dv = param_point.y;
        let dw = param_point.z;
        let per = self.per();
        let a = dw * per;

        let cos = a.cos();
        let sin = a.sin();

        let mid = volume.twist_mid_dir();
        let bin = volume.twist_binormal();
        let axis = volume.twist_axis();

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
            volume.twist_origin()
                + (cos * du + -sin * dv) * mid
                + (sin * du + cos * dv) * bin
                + dw * axis,
        );

        // 1st derivs
        if n >= 1 {
            put(sv(1, 0, 0), cos * mid + sin * bin);
            put(sv(0, 1, 0), -sin * mid + cos * bin);
            put(
                sv(0, 0, 1),
                per * (-sin * du + -cos * dv) * mid + per * (cos * du + -sin * dv) * bin + axis,
            );
            // NonZero 2nd derivs
            if n >= 2 {
                let per2 = per * per;
                put(sv(1, 0, 1), per * -sin * mid + per * cos * bin);
                put(sv(0, 1, 1), per * -cos * mid + per * -sin * bin);
                put(
                    sv(0, 0, 2),
                    per2 * (-cos * du + sin * dv) * mid + per2 * (-sin * du + -cos * dv) * bin,
                );

                // NonZero 3rd derivs
                if n >= 3 {
                    let per3 = per2 * per;
                    put(sv(1, 0, 2), per2 * -cos * mid + per2 * -sin * bin);
                    put(sv(0, 1, 2), per2 * sin * mid + per2 * -cos * bin);
                    put(
                        sv(0, 0, 3),
                        per3 * (sin * du + cos * dv) * mid + per3 * (-cos * du + sin * dv) * bin,
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
        let per = self.per(); // negative for left hand twists
        let a = dw * per;

        let cos = a.cos();
        let sin = a.sin();

        // init output array
        let mut d = vec![Vec3::ZERO; eval_size(n)];
        let mut put = |i: usize, v: Vec3| {
            if let Some(slot) = d.get_mut(i) {
                *slot = v;
            }
        };

        // position
        put(0, Vec3::new(cos * du - sin * dv, sin * du + cos * dv, dw));

        // 1st derivs
        if n >= 1 {
            put(sv(1, 0, 0), Vec3::new(cos, sin, 0.0));
            put(sv(0, 1, 0), Vec3::new(-sin, cos, 0.0));
            put(
                sv(0, 0, 1),
                Vec3::new(
                    per * (-sin * du - cos * dv),
                    per * (cos * du - sin * dv),
                    1.0,
                ),
            );

            // 2nd derivs (Duu, Duv, Dvv are zero)
            if n >= 2 {
                let per2 = per * per;
                put(sv(1, 0, 1), Vec3::new(per * -sin, per * -cos, 0.0));
                put(sv(0, 1, 1), Vec3::new(per * -cos, -per * sin, 0.0));
                put(
                    sv(0, 0, 2),
                    Vec3::new(
                        per2 * (-cos * du + sin * dv),
                        per2 * (-sin * du + -cos * dv),
                        0.0,
                    ),
                );

                // 3rd derivs (all but Duww, Dvww, Dwww are zero)
                if n >= 3 {
                    let per3 = per2 * per;
                    put(sv(1, 0, 2), Vec3::new(per2 * -cos, per2 * -sin, 0.0));
                    put(sv(0, 1, 2), Vec3::new(per2 * sin, per2 * -cos, 0.0));
                    put(
                        sv(0, 0, 3),
                        Vec3::new(
                            per3 * (sin * du + cos * dv),
                            per3 * (-cos * du + sin * dv),
                            0.0,
                        ),
                    );
                }
            }
        }
        Ok(d)
    }

    /// The projection-space boxes containing the image of `param_box`
    /// (`EvaluateBoundingBoxSimple`), written into the requested outputs. An
    /// unbounded `param_box` gives unbounded boxes (the pseudo box with the
    /// standard basis). Otherwise the box holds `w` in `[w_min, w_max]` and the
    /// four helical sweeps of the `uv` corners: a circle of the largest corner
    /// radius when the sweep spans a full turn, else the arc ends and the arcs'
    /// axis extrema. The pseudo box is the axis-aligned box.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unbounded `param_box` with only the
    /// pseudo box requested (upstream dereferences the null box pointer).
    pub fn evaluate_bounding_box_simple(
        &self,
        param_box: &Extent3d,
        proj_box: Option<&mut Extent3d>,
        proj_pseudo_box: Option<&mut PseudoBox>,
    ) -> Result<()> {
        // no work - no output
        if proj_box.is_none() && proj_pseudo_box.is_none() {
            return Ok(());
        }

        // simple case - no bounds
        if !param_box.any_bounds() {
            // return unbounded ProjSpace boxes
            let Some(b) = proj_box else {
                return Err(SolidError::invalid_input(
                    OP_BOX,
                    "unbounded parameter box without a box output (upstream dereferences null)",
                ));
            };
            b.set_unbounded();
            if let Some(pb) = proj_pseudo_box {
                pb.init_basis();
                pb.set_unbounded();
            }
            return Ok(());
        }

        let mut s_proj_box = Extent3d::new();

        // BBox - computed directly from the properties of the ParamBox
        let pi_half = PI / 2.0;
        let pi = PI;
        let three_pi_half = PI * 3.0 / 2.0;
        let two_pi = PI * 2.0;
        let theta_min_rad = param_box.w_min() * two_pi / self.twist_period_dist;
        let theta_max_rad = param_box.w_max() * two_pi / self.twist_period_dist;
        let theta_range_rad = Extent1d::from_min_max(theta_min_rad, theta_max_rad);

        let (u_min, u_max) = (param_box.u_min(), param_box.u_max());
        let (v_min, v_max) = (param_box.v_min(), param_box.v_max());
        let rad1 = (u_min * u_min + v_min * v_min).sqrt();
        let rad2 = (u_min * u_min + v_max * v_max).sqrt();
        let rad3 = (u_max * u_max + v_max * v_max).sqrt();
        let rad4 = (u_max * u_max + v_min * v_min).sqrt();
        let ang_rad1 = v_min.atan2(u_min);
        let ang_rad2 = v_max.atan2(u_min);
        let ang_rad3 = v_max.atan2(u_max);
        let ang_rad4 = v_min.atan2(u_max);

        // The BBox UV square on the W=0 plane is swept in a helix from WMin to WMax
        // sweeping through associated angles AngMin and AngMax. Each corner sweeps
        // from dAngRadCorner + AngMin to dAngRadCorner + AngMax.
        if theta_range_rad.length() >= two_pi {
            // Make BBox contain an axis centered circle of Max Radius
            let max_rad = max4(rad1, rad2, rad3, rad4);
            // upstream ignores the status
            let _ = s_proj_box.set_min_max_coords(
                -max_rad,
                -max_rad,
                param_box.w_min(),
                max_rad,
                max_rad,
                param_box.w_max(),
            );
        } else {
            // build a bounding box that holds the 4 swept circular arcs of the UV square
            let mut proj_pt1 = Vec3::UNDEF;
            let mut proj_pt2 = Vec3::UNDEF;

            // Add the eight circle arc end points to ProjBox.
            for (ii, (rad, ang)) in [
                (rad1, ang_rad1),
                (rad2, ang_rad2),
                (rad3, ang_rad3),
                (rad4, ang_rad4),
            ]
            .into_iter()
            .enumerate()
            {
                let theta_rad1 = theta_min_rad + ang;
                let theta_rad2 = theta_max_rad + ang;
                if ii == 0 {
                    proj_pt1.z = param_box.w_min();
                    proj_pt2.z = param_box.w_max();
                }

                // arc end points
                proj_pt1.x = rad * theta_rad1.cos();
                proj_pt1.y = rad * theta_rad1.sin();

                proj_pt2.x = rad * theta_rad2.cos();
                proj_pt2.y = rad * theta_rad2.sin();

                s_proj_box.add_point(proj_pt1);
                s_proj_box.add_point(proj_pt2);

                // internal arc extrema points; [-4pi -2pi]=>-2, [-2pi 0]=>-1, [0 2Pi]=>0, [2pi 4pi]=>1
                let per_cnt = if theta_rad1 > 0.0 {
                    trunc_long(theta_rad1 / two_pi)
                } else {
                    trunc_long((theta_rad1 - two_pi) / two_pi)
                };
                let zero_rad = per_cnt * two_pi;
                let rad_min = theta_rad1 - zero_rad; // in range:[0 2Pi] max:[2Pi]
                let rad_max = theta_rad2 - zero_rad; // in range:[dRad0, dRad0+2Pi] Max:[4Pi]

                let hit = |c: f64| is_contained_to_tol(c, rad_min, rad_max, EFF_ZERO_DEG);
                // the statuses of the setters are ignored upstream
                for (angle, side) in [
                    (0.0, 0),
                    (pi_half, 1),
                    (pi, 2),
                    (three_pi_half, 3),
                    (two_pi, 0),
                    (two_pi + pi_half, 1),
                    (two_pi + pi, 2),
                    (two_pi + three_pi_half, 3),
                    (two_pi + two_pi, 0),
                ] {
                    if !hit(angle) {
                        continue;
                    }
                    let _ = match side {
                        0 if s_proj_box.u_max() < rad => s_proj_box.set_u_max(rad),
                        1 if s_proj_box.v_max() < rad => s_proj_box.set_v_max(rad),
                        2 if s_proj_box.u_min() > -rad => s_proj_box.set_u_min(-rad),
                        3 if s_proj_box.v_min() > -rad => s_proj_box.set_v_min(-rad),
                        _ => Ok(()),
                    };
                }
            }
        }

        // we are not going to find a PseudoBox much better than the current ProjBBox
        if let Some(b) = proj_box {
            *b = s_proj_box;
        }
        if let Some(pb) = proj_pseudo_box {
            pb.assign_extent3d(&s_proj_box);
        }
        Ok(())
    }

    /// The parameter point mapping to `proj_point` (exact: the inverse rotation), as
    /// the single guess of the Newton solvers (`InvEvaluateGuessPointSimple`).
    #[must_use]
    pub fn inv_evaluate_guess_point_simple(&self, proj_point: Point3) -> Vec<Point3> {
        // compute the inverse mapping rotation angle
        // (neg sign = rotate from ProjSpace to ParamSpace)
        let theta_rad = -2.0 * PI * proj_point.z / self.twist_period_dist;

        let cos = theta_rad.cos();
        let sin = theta_rad.sin();

        // same as positive twist mapping but with the angle negated
        vec![Vec3::new(
            cos * proj_point.x + -sin * proj_point.y,
            sin * proj_point.x + cos * proj_point.y,
            proj_point.z,
        )]
    }

    /// The exact inverse of `proj_point` (the single guess point).
    fn inverse(&self, proj_point: Point3) -> Point3 {
        self.inv_evaluate_guess_point_simple(proj_point)
            .first()
            .copied()
            .unwrap_or(Vec3::UNDEF)
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

    /// Solves for the parameter point mapping to `proj_point`
    /// (`GlobalPointSolveSimple`). `solutions` is emptied; when the exact inverse
    /// lies in `param_domain` (or there is none) it gets one single-value solution
    /// with the three parameters, value `0` and object `this` (the volume's
    /// handle). Returns upstream's `rbFoundAnswer`.
    pub fn global_point_solve_simple<O: SolutionObject>(
        &self,
        proj_point: Point3,
        solutions: &mut SolutionArray<O>,
        param_domain: Option<&Extent3d>,
        this: O,
    ) -> bool {
        // init output
        solutions.reset();

        // inverse mapping - InvEvaluateGuessPointSimple is exact
        let p = self.inverse(proj_point);

        // set output
        if !param_domain.is_none_or(|d| d.contains_point(p, 0.0)) {
            return false;
        }
        solutions.set_size(1);
        if let Some(sol) = solutions.get_mut(0) {
            Self::fill_solution(sol, p, this);
        }
        true
    }

    /// Solves for the parameter point mapping to `proj_point`; no guess is needed
    /// (`LocalPointSolveSimple`). `solution` is reset (its ends keep their values,
    /// as upstream) and, when the exact inverse lies in `param_domain` (or there is
    /// none), set as in [`TwistVolume::global_point_solve_simple`]. Returns
    /// upstream's `rbFoundAnswer`.
    pub fn local_point_solve_simple<O: SolutionObject>(
        &self,
        proj_point: Point3,
        solution: &mut Solution<O>,
        param_domain: Option<&Extent3d>,
        this: O,
    ) -> bool {
        // init output
        solution.reset();

        // inverse mapping - InvEvaluateGuessPointSimple is exact
        let p = self.inverse(proj_point);

        // set output
        if !param_domain.is_none_or(|d| d.contains_point(p, 0.0)) {
            return false;
        }
        Self::fill_solution(solution, p, this);
        true
    }

    /// The twist's parameter space is unbounded (`IsBoundedSimple`).
    #[must_use]
    pub const fn is_bounded_simple(&self) -> bool {
        false
    }

    /// Every parameter point is in the domain (`IsPointInParamDomainSimple`).
    #[must_use]
    pub const fn is_point_in_param_domain_simple(&self) -> bool {
        true
    }

    /// Every parameter line is in the domain (`IsLineInParamDomainSimple`).
    #[must_use]
    pub const fn is_line_in_param_domain_simple(&self) -> bool {
        true
    }

    /// Every parameter curve is in the domain (`IsCurveInParamDomainSimple`).
    #[must_use]
    pub const fn is_curve_in_param_domain_simple(&self) -> bool {
        true
    }

    /// `MakeExactBSpline1stOutSurface`: the exact first out-space image of `input`
    /// (from in-space when `in_space`, else from parameter space) through the twist
    /// `volume` (whose kind is `self`), or `None` when it is not one of the analytic
    /// cases: a surface in a plane normal to the twist axis maps to a copy rotated
    /// about the axis by the twist angle of its height. The test samples a 6 x 6 grid
    /// of the natural domain; any sample outside the twist's domain gives `None`.
    ///
    /// # Errors
    ///
    /// Those of the surface copy; [`SolidError::Unported`] from the surface or volume
    /// services.
    pub fn make_exact_b_spline_1st_out_surface(
        &self,
        k: &Kernel,
        volume: &Volume,
        in_space: bool,
        input: &Surface,
    ) -> Result<Option<Surface>> {
        // locals: ParamSpace and InSpace Twist orientations
        let (param_origin, twist_origin) = (Vec3::ZERO, volume.twist_origin());
        let (param_axis, twist_axis) = (Vec3::Z, volume.twist_axis());

        // locals: making InputSpace either ParamSpace or InSpace
        let (input_origin, input_axis) = if in_space {
            (twist_origin, twist_axis)
        } else {
            (param_origin, param_axis)
        };

        // locals
        let mut input_pt = Vec3::UNDEF;

        let approx_tol = approx_tol_3d(k, SmallTopoType::NormalSize);
        let domain = input.natural_uv_domain_with_nurb(k)?;
        let ivl_u = domain.u_interval();
        let ivl_v = domain.v_interval();

        // note: for Twist mapping Twist principle axes happen to be the same in InSpace
        // and OutSpace
        let mut axis_plane_sum = 0.0;
        let mut axis_plane_range = Extent1d::new();

        // See if the InputSurface is perpendicular to the Twist axis
        for ii in 0..=SMP_SIZE {
            let u = ivl_u.evaluate(f64::from(ii) / f64::from(SMP_SIZE));
            for jj in 0..=SMP_SIZE {
                let uv = Point2::new(u, ivl_v.evaluate(f64::from(jj) / f64::from(SMP_SIZE)));

                // Surface sample (status ignored)
                if let Some(p) = ignored(input.evaluate_point(k, uv))? {
                    input_pt = p;
                }
                let input_vec = input_pt - input_origin;

                // no work - sample point not within twist domain
                let is_in = if in_space {
                    volume.is_point_in_in_space_domain(input_pt, true)?.0
                } else {
                    volume.is_point_in_param_domain(input_pt, true)?
                };
                if !is_in {
                    return Ok(None);
                }

                // distance to the plane through the origin perpendicular to the axis
                let axis_plane = twist_axis.dot(input_vec);
                axis_plane_range.add_value(axis_plane);
                axis_plane_sum += axis_plane;

                // remember when the surface is a constant distance from the twist plane
                let const_axis_plane = axis_plane_range.length() < approx_tol;

                // no work - Surface not const distance from the twist plane
                if !const_axis_plane {
                    return Ok(None);
                }
            }
        }

        // compute the rotation
        let samples = f64::from(SMP_SIZE) + 1.0;
        let z = axis_plane_sum / samples / samples;
        let ang_rad = z * 2.0 * PI / self.twist_period_dist();

        let mut rotate = Axis2Placement::IDENTITY;
        rotate.rotate_about_axis_at_point(ang_rad, input_origin + z * input_axis, input_axis);

        // copy the input Surface and rotate it (status ignored)
        let mut new_surface = input.copy(k)?;
        ignored(new_surface.transform(k, &rotate, None))?;

        // all done
        Ok(Some(new_surface))
    }

    /// `MakeExactBSpline1stOutCurve`: the exact first out-space image of `input` (from
    /// in-space when `in_space`, else from parameter space) through the twist `volume`
    /// (whose kind is `self`), or `None` when it is not one of the analytic cases: a
    /// curve in a plane normal to the twist axis maps to a copy rotated about the axis
    /// by the twist angle of its height. The test samples 12 points of the natural
    /// interval; any sample outside the twist's domain gives `None`.
    ///
    /// # Errors
    ///
    /// Those of the curve copy (upstream dereferences the `NULL` a failed `Copy`
    /// leaves); [`SolidError::Unported`] from the curve or volume services.
    pub fn make_exact_b_spline_1st_out_curve(
        &self,
        k: &Kernel,
        volume: &Volume,
        in_space: bool,
        input: &Curve,
    ) -> Result<Option<Curve>> {
        // locals: ParamSpace and InSpace Twist orientations
        let (param_origin, twist_origin) = (Vec3::ZERO, volume.twist_origin());
        let (param_axis, twist_axis) = (Vec3::Z, volume.twist_axis());

        // locals: making InputSpace either ParamSpace or InSpace
        let (input_origin, input_axis) = if in_space {
            (twist_origin, twist_axis)
        } else {
            (param_origin, param_axis)
        };

        // locals
        let approx_tol = approx_tol_3d(k, SmallTopoType::NormalSize);
        let ivl = input.natural_interval();
        let mut input_pt = Vec3::UNDEF;
        let mut on_axis_pt = Vec3::UNDEF;

        let mut axis_plane_range = Extent1d::new();

        // See if the InputCurve is in a plane perpendicular to the TwistAxis
        for ii in 0..=CRV_SMP_SIZE {
            let smp_u = ivl.evaluate(f64::from(ii) / f64::from(CRV_SMP_SIZE));

            // Curve sample (status ignored)
            if let Some(p) = ignored(input.evaluate_point(k, smp_u))? {
                input_pt = p;
            }

            // no work - sample point not within twist domain
            let is_in = if in_space {
                volume.is_point_in_in_space_domain(input_pt, true)?.0
            } else {
                volume.is_point_in_param_domain(input_pt, true)?
            };
            if !is_in {
                return Ok(None);
            }

            // pick a point on Axis near the curve to keep vector sizes small
            if ii == 0 {
                let to_orig_vec = input_pt - input_origin;
                on_axis_pt = input_origin + input_axis.dot(to_orig_vec) * input_axis;
            }

            // Input vec from SamplePt to OnAxisPt
            let on_axis_vec = input_pt - on_axis_pt;

            // InputSpace distances to planes perp to Twist axes
            let axis_plane = input_axis.dot(on_axis_vec);

            // accumulate the range of Plane distances
            axis_plane_range.add_value(axis_plane);

            // remember when curve is const dist from twist plane
            let const_axis_plane = axis_plane_range.length() < approx_tol;

            // no work - curve not const distance plane perp to TwistAxis
            if !const_axis_plane {
                return Ok(None);
            }
        }

        // compute the rotation
        let to_orig_vec = on_axis_pt - input_origin;
        let z = input_axis.dot(to_orig_vec);
        let ang_rad = z * 2.0 * PI / self.twist_period_dist();

        let mut rotate = Axis2Placement::IDENTITY;
        rotate.rotate_about_axis_at_point(ang_rad, on_axis_pt, input_axis);

        // copy the input curve and rotate it (status ignored)
        let mut new_curve = input.copy(k)?;
        ignored(new_curve.transform(k, &rotate, None))?;

        // all done
        Ok(Some(new_curve))
    }

    /// Reads a twist volume from a database stream: the period and the minimum
    /// distance, then the volume base data (`ReadFromDB`), whose status upstream
    /// ignores.
    ///
    /// # Errors
    ///
    /// A failed binary read; the base read's undefined cases
    /// ([`SolidError::Unsupported`]) and [`SolidError::Unported`].
    pub fn read_from_db(db: &mut DatabaseReader, version: u64) -> Result<Volume> {
        // a new SmTwistVolume() (period 0, minimum SM_UNDEF_DOUBLE)
        let mut twist = Self::new();
        if db.file_type() == FileType::Ascii {
            for slot in [&mut twist.twist_period_dist, &mut twist.period_min_dist] {
                if let Some(stream) = db.in_stream_mut() {
                    if let Some(v) = stream.extract_double() {
                        *slot = v;
                    }
                }
                db.go_to_next_line();
            }
        } else {
            twist.twist_period_dist = db.read_double()?;
            twist.period_min_dist = db.read_double()?;
        }
        let mut volume = Volume::new(VolumeKind::Twist(twist));
        // read base class data (status ignored)
        ignore_read(volume.read_base_from_db(db, version))?;
        Ok(volume)
    }
}

impl Volume {
    /// The in-space point on the twist center line at `w = 0`: the orient origin
    /// (`GetTwistOrigin`).
    #[must_use]
    pub fn twist_origin(&self) -> Vec3 {
        self.orient_origin()
    }

    /// The in-space twist axis, the parameter `w` direction: the orient z axis
    /// (`GetTwistAxis`).
    #[must_use]
    pub fn twist_axis(&self) -> Vec3 {
        self.orient_z_axis()
    }

    /// The in-space mid direction, the parameter `u` direction: the orient x axis
    /// (`GetTwistMidDir`).
    #[must_use]
    pub fn twist_mid_dir(&self) -> Vec3 {
        self.orient_x_axis()
    }

    /// The in-space binormal `axis × mid_dir`, the parameter `v` direction: the
    /// orient y axis (`GetTwistBinormal`).
    #[must_use]
    pub fn twist_binormal(&self) -> Vec3 {
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

    fn twist(period: f64) -> TwistVolume {
        TwistVolume::from_members(period, ZONE_TOL_3D)
    }

    #[test]
    fn simple_map_rotates_with_w() {
        let t = twist(4.0);
        // a quarter turn at w = 1
        let d = t.evaluate_simple(v(1.0, 0.0, 1.0), 0).unwrap();
        assert!(close(d[0], v(0.0, 1.0, 1.0)));
        let inv = t.inv_evaluate_guess_point_simple(d[0]);
        assert!(close(inv[0], v(1.0, 0.0, 1.0)));
        assert!(t.evaluate_simple(v(0.0, 0.0, 0.0), 4).is_err());
    }

    #[test]
    fn simple_derivatives_match_differences() {
        let t = twist(3.0);
        let p = v(0.7, -0.4, 0.9);
        let d = t.evaluate_simple(p, 1).unwrap();
        let h = 1e-6;
        let pos = |q: Vec3| t.evaluate_simple(q, 0).unwrap()[0];
        let dw = (pos(p + v(0.0, 0.0, h)) - pos(p - v(0.0, 0.0, h))) / (2.0 * h);
        assert!((d[1] - dw).length() < 1e-6);
        let du = (pos(p + v(h, 0.0, 0.0)) - pos(p - v(h, 0.0, 0.0))) / (2.0 * h);
        assert!((d[4] - du).length() < 1e-6);
    }

    #[test]
    fn create_repairs_input_and_evaluates_in_frame() {
        let vol = TwistVolume::create(
            v(1.0, 2.0, 3.0),
            v(0.0, 0.0, 2.0),
            v(3.0, 0.0, 0.0),
            8.0,
            0.0,
        );
        let VolumeKind::Twist(t) = vol.kind() else {
            panic!("not a twist")
        };
        assert_eq!(t.period_min_dist(), ZONE_TOL_3D);
        assert_eq!(t.twist_period_dist(), 8.0);
        assert!(close(vol.twist_axis(), v(0.0, 0.0, 1.0)));
        assert!(close(vol.twist_mid_dir(), v(1.0, 0.0, 0.0)));
        assert!(close(vol.twist_binormal(), v(0.0, 1.0, 0.0)));
        assert!(close(vol.twist_origin(), v(1.0, 2.0, 3.0)));
        let d = t
            .evaluate(&vol, v(1.0, 0.0, 2.0), 3, &EvalOptions::default())
            .unwrap();
        assert!(close(d[0], v(1.0, 3.0, 5.0)));
        assert_eq!(d.len(), 64);

        // a tiny period becomes |min|, a parallel mid direction is replaced
        let vol = TwistVolume::create(
            v(0.0, 0.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(0.0, 0.0, 5.0),
            0.0,
            -1.0,
        );
        let VolumeKind::Twist(t) = vol.kind() else {
            panic!("not a twist")
        };
        assert_eq!(t.twist_period_dist(), 1.0);
        assert!(vol.twist_axis().dot(vol.twist_mid_dir()).abs() < 1e-12);
    }

    #[test]
    fn bounding_boxes() {
        let t = twist(4.0);
        let mut b = Extent3d::new();
        // full turn: a circle of the largest corner radius
        let pb = Extent3d::from_bounds(0.0, 0.0, 0.0, 1.0, 1.0, 5.0);
        t.evaluate_bounding_box_simple(&pb, Some(&mut b), None)
            .unwrap();
        let r = 2f64.sqrt();
        assert!((b.u_max() - r).abs() < 1e-15 && (b.v_min() + r).abs() < 1e-15);
        // no rotation: the parameter box
        let pb = Extent3d::from_bounds(1.0, 1.0, 0.0, 2.0, 2.0, 0.0);
        let mut pseudo = PseudoBox::new();
        t.evaluate_bounding_box_simple(&pb, Some(&mut b), Some(&mut pseudo))
            .unwrap();
        assert!((b.u_min() - 1.0).abs() < 1e-12 && (b.v_max() - 2.0).abs() < 1e-12);
        // unbounded
        let mut un = Extent3d::new();
        un.set_unbounded();
        t.evaluate_bounding_box_simple(&un, Some(&mut b), None)
            .unwrap();
        assert_eq!(b.u_max(), INFINITE_PARAMETER);
        assert!(t
            .evaluate_bounding_box_simple(&un, None, Some(&mut pseudo))
            .is_err());
        assert!(t.evaluate_bounding_box_simple(&un, None, None).is_ok());
    }

    #[test]
    fn solvers() {
        let t = twist(2.0);
        let mut sols = SolutionArray::new();
        assert!(t.global_point_solve_simple(v(1.0, 0.0, 0.5), &mut sols, None, Vol));
        let s = sols.get(0).unwrap();
        assert_eq!(s.solution_type, SolutionType::SingleValue);
        assert!((s.start.parameters[1] + 1.0).abs() < 1e-12);
        let dom = Extent3d::from_bounds(0.0, 0.0, 0.0, 1.0, 1.0, 1.0);
        assert!(!t.global_point_solve_simple(v(1.0, 0.0, 0.5), &mut sols, Some(&dom), Vol));
        assert!(sols.is_empty());
        let mut sol = Solution::new();
        assert!(t.local_point_solve_simple(v(1.0, 0.0, 0.0), &mut sol, Some(&dom), Vol));
        assert_eq!(sol.objects[0], Some(Vol));
        assert!(!t.local_point_solve_simple(v(-1.0, 0.0, 0.0), &mut sol, Some(&dom), Vol));
        assert_eq!(sol.solution_type, SolutionType::Unknown);
    }

    #[test]
    fn equality_compares_only_the_minimum() {
        let a = TwistVolume::from_members(1.0, 0.5);
        let b = TwistVolume::from_members(2.0, 0.5);
        assert!(a.members_eq(&b));
        assert!(!a.members_eq(&TwistVolume::from_members(1.0, 0.6)));
    }

    #[test]
    fn helpers() {
        assert!(is_contained_to_tol(1.0, 2.0, 0.5, 0.0));
        assert!(!is_contained_to_tol(3.0, 2.0, 0.5, 0.1));
        assert_eq!(trunc_long(-2.7), -2.0);
        assert_eq!(trunc_long(f64::NAN), -9_223_372_036_854_775_808.0);
        assert!(is_kind_of(TWIST_VOLUME_TYPE));
        let t = TwistVolume::new();
        assert!(!t.is_bounded_simple());
        assert!(t.is_point_in_param_domain_simple());
        assert!(t.is_line_in_param_domain_simple());
        assert!(t.is_curve_in_param_domain_simple());
        assert_eq!(t.natural_param_domain().u_max(), INFINITE_PARAMETER);
    }
}

//! The surface validity checks (`AssertValid`): the base class `SmSurface` and its
//! virtual dispatch over the B-spline, plane, cone, cylinder, sphere, torus,
//! surface-of-revolution and surface-of-extrusion overrides, with the predicates only
//! they use.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/src/SmBSplineSurface.cpp`,
//! `source/SMLib/src/SmNurbsSrf.cpp`, `source/SMLib/src/SmPlane.cpp`,
//! `source/SMLib/src/SmCone.cpp`, `source/SMLib/src/SmCylinder.cpp`,
//! `source/SMLib/src/SmSphere.cpp`, `source/SMLib/src/SmTorus.cpp`,
//! `source/SMLib/src/SmSurfOfRevolution.cpp`, `source/SMLib/src/SmSurfOfExtrusion.cpp`,
//! `source/SMLib/src/SmAxis2Placement.cpp`,
//! `source/SMLib/src/SmPointSet.cpp`, `source/SMLib/inc/SmAssertArray.h`.
//!
//! | Rust                                  | Upstream                                              |
//! |---------------------------------------|-------------------------------------------------------|
//! | [`Surface::assert_valid`]             | `SmSurface::AssertValid` (virtual)                    |
//! | `surface_assert_valid` (private)      | `SmSurface::AssertValid`                              |
//! | `assert_test_surface1` .. `6` (private) | `sm_AssertTestSurface1`, `4`, `5`, `6`              |
//! | `b_spline_surface_assert_valid` (private) | `SmBSplineSurface::AssertValid`                   |
//! | `plane_assert_valid` (private)        | `SmPlane::AssertValid`                                |
//! | `cone_assert_valid` (private)         | `SmCone::AssertValid`                                 |
//! | `cylinder_assert_valid` (private)     | `SmCylinder::AssertValid`                             |
//! | `sphere_assert_valid` (private)       | `SmSphere::AssertValid`                               |
//! | `torus_assert_valid` (private)        | `SmTorus::AssertValid`                                |
//! | `surf_of_revolution_assert_valid` (private) | `SmSurfOfRevolution::AssertValid`               |
//! | `surf_of_extrusion_assert_valid` (private) | `SmSurfOfExtrusion::AssertValid`                 |
//! | [`axis2_placement_assert_valid`]      | `SmAxis2Placement::AssertValid`                       |
//! | [`has_knot_multiplicity_greater_than_degree`] | `SmBSplineSurface::HasKnotMultiplicityGreaterThanDegree`, `sm_HasKnotMultiplicityGreaterThanDegree` |
//! | [`has_internal_pole`]                 | `SmBSplineSurface::HasInternalPole`                   |
//! | [`has_degenerate_control_point_row`]  | `SmBSplineSurface::HasDegenerateControlPointRow`      |
//! | `has_same_parameterization` (private) | `SmBSplineSurface::HasSameParameterization` (curve)   |
//! | [`grid_has_non_monotonic_points`]     | `SmPointGrid(...)`, `SmPointGrid::SetUpMonotonic`, `HasNonMonotonicPoints` |
//! | [`LEVEL_GIVEN`]                       | `SM_LEVEL_GIVEN`                                      |
//!
//! Deviations:
//! - **Levels.** `level` is the `SmAssertTestLevel` value (`SM_LEVEL_0` .. `SM_LEVEL_2`,
//!   [`LEVEL_GIVEN`]). As the release build's report macros do, a test runs when
//!   `level >= ` its test level and `pTestRequests` is ignored; [`LEVEL_GIVEN`] skips
//!   the base-class calls but runs every test.
//! - **Report identities.** Reports name the surface by `key` (reporter and owner);
//!   the reporting type (`GetClassType()`, static) is the class whose method runs the
//!   test and the owner type string (`GetClassString()`, virtual) the surface's own
//!   class. The placement's reports (`m_vPosition.AssertValid`, its own object
//!   upstream) also name `key`: their reporting type differs, so they never collide
//!   with the surface's.
//! - **Context tests** (`SmSurfOfRevolution` 0, 1, `SmSurfOfExtrusion` 27) always
//!   hold: one kernel is the only context.
//! - **Generator owner** (`SmSurfOfRevolution` 14, `SmSurfOfExtrusion` 26): the
//!   generator is held by value, so it is owned by the surface unless its owner handle
//!   names another object. Upstream dereferences a missing generator there (and in
//!   `SmSurfOfExtrusion` 7 and 8); that is [`SolidError::InvalidInput`].
//! - **Angles** (`SmSurfOfExtrusion` 10, 12, 14, 16, 18, 20): a failed `AngleBetween`
//!   (a zero-length vector) is the angle `0` it writes, as upstream, which ignores its
//!   status. The `GlobalPropertyAnalysis` of tests 21 and 23 fails for a zero-length
//!   extrusion vector (its `LocalSolve` `SER`s the same `AngleBetween`); upstream
//!   ignores that status too and reads the solutions it reset on entry, so it is no
//!   solution found (`no_tangent_along`).
//! - **Warnings.** `SmBSplineSurface::AssertValid`'s repeated-end-control-point
//!   check only issues a `WARN`, so `HasRepeatedEndControlPoints` is not run.
//! - **Undefined behaviour.** Index arithmetic of `HasInternalPole` that leaves the
//!   control net (unclamped knot vectors), the first/last polar angle of an empty
//!   table, missing iso boundaries and a continuity output `CalculateContinuities`
//!   never wrote are [`SolidError::InvalidInput`]. A failed `CalculateContinuities`
//!   (status ignored) answers the minimum continuity it had reached. In
//!   `SmSurfOfExtrusion::AssertValid` a `plane_point_distance` error is mapped to
//!   [`SolidError::InvalidInput`]: upstream would read `dDist` uninitialised there, which
//!   is unreachable, since `IsPlanar`'s normal is never longer than unit.
//! - **Statuses.** Upstream ignores the statuses of the evaluations and queries in
//!   these tests; the port propagates their errors (as `validation.rs` does; except
//!   the extrusion's `GlobalPropertyAnalysis`, see **Angles**), and a
//!   failed `MakeNurb` is ignored, as upstream.
//! - **Extrusion origin zero (WR-511, #1508).** Test 8 uses the pinned unscaled
//!   `SM_EFF_ZERO` by default; `Kernel::fixes.extrusion_origin_scaled_zero` uses
//!   `SM_EFF_ZERO * (1 + origin.max_dimension())` for that solve only.
//! - **Unported overrides.** `SmCurveBoundedSurface`, `SmSrfInVolume` and
//!   `SmOffsetSurface` have their own `AssertValid`, not ported here; they return
//!   [`SolidError::Unported`]. `SmSTEPSurface` has none and
//!   runs the base check.
//! - No recursion.

use std::borrow::Cow;

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::TANGENT_FIELD_TYPE;
use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::{Curve, CurveKind, CurvePropertyType};
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::a_object;
use crate::geom::assert_array::labels::{
    AXIS2_PLACEMENT_TYPE, B_SPLINE_SURFACE_TYPE, CONE_TYPE, CRV_ON_SURF_TYPE, CYLINDER_TYPE,
    FACE_TYPE, OFFSET_SURFACE_TYPE, PLANE_TYPE, SPHERE_TYPE, SRF_IN_VOLUME_TYPE, SURFACE_TYPE,
    SURF_OF_EXTRUSION_TYPE, SURF_OF_REVOLUTION_TYPE, TORUS_TYPE, TORUS_WITH_SPINDLE_APEX,
};
use crate::geom::assert_array::{
    assert_label_list, AssertArray, AssertReport, AssertReportLabel, ObjectKey, ReportSpec,
};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{line_point_distance, plane_point_distance};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{OperandObject, SolutionType};
use crate::geom::tol::{
    approx_tol3d, approx_tol_3d, are_same, are_same_to_tol, is_degenerate_curve, is_zero,
    is_zero_to_tol, tol3d, xsect_tol_3d_from_zones, zone_tol_3d_of_curve, SmallTopoType,
    ANG_TOL_DEG, ZONE_TOL_3D,
};
use crate::geom::vector3d::acos as sm_acos;
use crate::geom::vector3d::asin as sm_asin;
use crate::kernel::Kernel;
use crate::math::{
    max2, min2, rad_to_deg, Point2, Point3, Vec3, EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_RAD,
    EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE,
};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::cone::Cone;
use crate::surface::plane::Plane;
use crate::surface::sphere::Sphere;
use crate::surface::surf_of_extrusion::{CurveOrientType, SurfOfExtrusion};
use crate::surface::surf_of_revolution::SurfOfRevolution;
use crate::surface::torus::Torus;

use super::{Surface, SurfaceKind};

/// `SM_LEVEL_GIVEN`: run the tests of the called class only (base classes skipped).
pub const LEVEL_GIVEN: u8 = 4;

const OP: &str = "surface::surface::assert_valid";

/// `SmBoolean` reports go out with an undefined tolerance and value.
const UNDEF: f64 = UNDEF_DOUBLE;

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// The report state of one `AssertValid` run: the list (`pAList`), the surface's
/// identity, the level and the surface's class name (`GetClassString()`).
struct Reporter<'a> {
    list: Option<&'a mut AssertArray>,
    key: ObjectKey,
    level: u8,
    class_string: &'static str,
}

impl Reporter<'_> {
    /// Whether the base-class call runs (`eTestLevel != SM_LEVEL_GIVEN`).
    fn bases(&self) -> bool {
        self.level != LEVEL_GIVEN
    }

    /// `SM_ASSERT_OBJ_VALUE_REPORT` / `SM_ASSERT_OBJ_BOOLEAN_REPORT` (release build):
    /// a test at `test_level` is skipped (passes) when `level < test_level`; a failed
    /// one adds a report of check `test` of class `class` (owner type `owner_type`,
    /// owner string `owner_string`) with `tol` and `val`.
    #[allow(clippy::too_many_arguments)]
    fn report_as(
        &mut self,
        class: i64,
        owner_type: i64,
        owner_string: &'static str,
        test: u64,
        test_level: u8,
        passed: bool,
        tol: f64,
        val: f64,
    ) -> Result<bool> {
        let labels = assert_label_list(class, 0);
        self.report_from(
            labels,
            class,
            owner_type,
            owner_string,
            test,
            test_level,
            passed,
            tol,
            val,
        )
    }

    /// [`Reporter::report_as`] with check `test` labelled from `labels` (not upstream:
    /// an opt-in check reporting from its class's extended table,
    /// [`AssertReport::with_labels`]).
    #[allow(clippy::too_many_arguments)]
    fn report_from(
        &mut self,
        labels: &'static [AssertReportLabel],
        class: i64,
        owner_type: i64,
        owner_string: &'static str,
        test: u64,
        test_level: u8,
        passed: bool,
        tol: f64,
        val: f64,
    ) -> Result<bool> {
        if self.level < test_level || passed {
            if let Some(l) = self.list.as_deref_mut() {
                l.set_last_add(false);
            }
            return Ok(true);
        }
        if let Some(l) = self.list.as_deref_mut() {
            let spec = ReportSpec {
                reporter: Some(self.key),
                test_index: test,
                test_class: class,
                owner: Some(self.key),
                owner_type,
                owner_type_string: Some(owner_string),
                other: None,
                other_type: 0,
                other_type_string: Some("NULL"),
                tol3d: tol3d(tol),
                value: val,
                list_index: 0,
                message: None,
            };
            l.add_unique_delete_duplicate(AssertReport::with_labels(spec, labels)?);
        }
        Ok(false)
    }

    /// [`Reporter::report_as`] about the surface (`SM_ASSERT_VALUE_REPORT` /
    /// `SM_ASSERT_BOOLEAN_REPORT` in the method of class `class`).
    fn report(
        &mut self,
        class: i64,
        test: u64,
        test_level: u8,
        passed: bool,
        tol: f64,
        val: f64,
    ) -> Result<bool> {
        let s = self.class_string;
        self.report_as(class, class, s, test, test_level, passed, tol, val)
    }
}

impl Surface {
    /// `AssertValid` (virtual, `SM_NO_WALK`; the walk argument is unused by every
    /// surface): the checks of this surface's class and, unless `level` is
    /// [`LEVEL_GIVEN`], of its base classes, reporting failures about the surface
    /// `key` into `list`. Returns `true` when every test that ran passed.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the surface kinds with their own unported check
    /// (curve bounded, surface in volume, offset) and from callees; the
    /// errors of the evaluations and queries the tests run (statuses upstream
    /// ignores); [`SolidError::InvalidInput`] where upstream's behaviour is undefined
    /// (see the module docs).
    pub fn assert_valid(
        &self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level: u8,
        key: ObjectKey,
    ) -> Result<bool> {
        let mut r = Reporter {
            list,
            key,
            level,
            class_string: self.class_string(),
        };
        match self.kind() {
            SurfaceKind::BSpline(_) => b_spline_surface_assert_valid(k, self, &mut r),
            SurfaceKind::Plane(_) => plane_assert_valid(k, self, &mut r),
            SurfaceKind::Cone(c) if c.is_cylinder_class() => cylinder_assert_valid(k, self, &mut r),
            SurfaceKind::Cone(_) => cone_assert_valid(k, self, &mut r),
            SurfaceKind::Sphere(_) => sphere_assert_valid(k, self, &mut r),
            SurfaceKind::Torus(_) => torus_assert_valid(k, self, &mut r),
            SurfaceKind::SurfOfRevolution(_) => surf_of_revolution_assert_valid(k, self, &mut r),
            SurfaceKind::Step(_) => surface_assert_valid(k, self, &mut r),
            SurfaceKind::CurveBounded(_) => {
                // UNPORTED(surface.curve_bounded_surface): SmCurveBoundedSurface::AssertValid
                Err(SolidError::unported(
                    "SmCurveBoundedSurface::AssertValid",
                    "surface.curve_bounded_surface",
                ))
            }
            SurfaceKind::SrfInVolume(_) => {
                // UNPORTED(surface.srf_in_volume): SmSrfInVolume::AssertValid
                Err(SolidError::unported(
                    "SmSrfInVolume::AssertValid",
                    "surface.srf_in_volume",
                ))
            }
            SurfaceKind::OffsetSurface(_) => {
                // UNPORTED(surface.offset_surface): SmOffsetSurface::AssertValid
                Err(SolidError::unported(
                    "SmOffsetSurface::AssertValid",
                    "surface.offset_surface",
                ))
            }
            SurfaceKind::SurfOfExtrusion(_) => surf_of_extrusion_assert_valid(k, self, &mut r),
        }
    }
}

// ---- SmSurface ----------------------------------------------------------------------

/// `SmSurface::AssertValid`.
fn surface_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    // check base class validity
    let mut rtn = true;

    // call the base class AssertValid
    rtn &= if r.bases() {
        a_object::assert_valid(false)
    } else {
        true
    };

    /*  0 */
    // check type of SmSurface::m_pOwner
    let owner_ok = s.owner().is_none_or(|o| {
        matches!(
            o.type_code,
            FACE_TYPE
                | OFFSET_SURFACE_TYPE
                | CRV_ON_SURF_TYPE
                | TANGENT_FIELD_TYPE
                | SRF_IN_VOLUME_TYPE
        )
    });
    rtn &= r.report(SURFACE_TYPE, 0, 0, owner_ok, UNDEF, UNDEF)?;

    /*  1 */
    // Surface Su and Sv Tangents in at least one corner are parallel (SM_EFF_ZERO_DEG)
    let (ok1, tol1, val1) = assert_test_surface1(k, s)?;
    rtn &= r.report(SURFACE_TYPE, 1, 0, ok1, tol1, val1)?;

    /*  2 */
    // Surface should not degenerate into a point
    let degen_point = s.is_degenerate_point(k, EFF_ZERO_DEG, None)?;
    rtn &= r.report(SURFACE_TYPE, 2, 0, !degen_point, EFF_ZERO_DEG, UNDEF)?;

    /*  3 */
    // Surface should not degenerate into a curve
    let degen_curve = s.is_degenerate_curve(k, EFF_ZERO_DEG, None, ZONE_TOL_3D / 10.0)?;
    rtn &= r.report(SURFACE_TYPE, 3, 0, !degen_curve, EFF_ZERO_DEG, UNDEF)?;

    /*  4 */
    // Surface should not contain a G1 or C0 discontinuity (run at every level)
    let ok4 = assert_test_surface4(k, s)?;
    let level = r.level;
    rtn &= r.report(SURFACE_TYPE, 4, level, ok4, UNDEF, UNDEF)?;

    /*  5 */
    // NonSingular Surface approximates a Singular Surface
    let ok5 = assert_test_surface5(k, s)?;
    rtn &= r.report(SURFACE_TYPE, 5, 0, ok5, UNDEF, UNDEF)?;

    /*  6 */
    // Open surface has G0 discontinuities
    let ok6 = assert_test_surface6(k, s)?;
    rtn &= r.report(SURFACE_TYPE, 6, 0, ok6, UNDEF, UNDEF)?;

    // all done
    Ok(rtn)
}

/// `sm_AssertTestSurface1`: no natural corner is a flat corner to `SM_ANG_TOL_DEG`
/// (`!HasFlatCorners`), with the report tolerance and value.
fn assert_test_surface1(k: &Kernel, s: &Surface) -> Result<(bool, f64, f64)> {
    let rtn = s.flat_corners(k, ANG_TOL_DEG, false)?.flags == 0;
    Ok((rtn, ANG_TOL_DEG, UNDEF_DOUBLE))
}

/// `sm_AssertTestSurface4`: at least G1 across every knot in both directions.
fn assert_test_surface4(k: &Kernel, s: &Surface) -> Result<bool> {
    // Surface continuities across U and V knots (statuses ignored: a failed call
    // leaves the minimum it had reached)
    let cont = |dir| -> Result<ContinuityType> {
        let mut reached = None;
        match s.calculate_continuities_reached(k, dir, &mut reached) {
            Ok((c, _)) => Ok(c),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => reached.ok_or_else(|| {
                invalid("continuity output never written (upstream reads it uninitialised)")
            }),
        }
    };
    let cont_u = cont(SurfParamType::U)?;
    let cont_v = cont(SurfParamType::V)?;

    // Good Surfaces have at least G1 continuity across every knot IsoParam curve
    let g1 = ContinuityType::G1 as i32;
    Ok(cont_u as i32 >= g1 && cont_v as i32 >= g1)
}

/// `sm_AssertTestSurface5`: no side is approximately singular without being singular.
fn assert_test_surface5(k: &Kernel, s: &Surface) -> Result<bool> {
    let sing = s.get_singularities(k, false, false)?;

    // valid when surface has no ApproxSingularities (ULONG subtraction)
    Ok(sing.approx_singular.wrapping_sub(sing.singular) == 0)
}

/// `SmTol::GetApproxTol3d(pSurface)`: half the zone tolerance of the surface's face,
/// else of the system.
fn surface_approx_tol(k: &Kernel, s: &Surface) -> Result<f64> {
    if let Some(zone) = s.face_zone_tol_3d()? {
        return Ok(approx_tol3d(k, 0.5 * zone));
    }
    Ok(approx_tol_3d(k, SmallTopoType::NormalSize))
}

/// `sm_AssertTestSurface6`: an open surface's opposite natural boundaries are not
/// partially coincident.
fn assert_test_surface6(k: &Kernel, s: &Surface) -> Result<bool> {
    // Locals
    let a_tol = surface_approx_tol(k, s)?;

    // Compare opposite boundaries for overlap
    for ii in 0..2 {
        // U direction 1st pass, V dir second.
        let dir = if ii == 1 {
            SurfParamType::V
        } else {
            SurfParamType::U
        };

        // Okay if the surface is closed
        let domain = s.natural_uv_domain_with_nurb(k)?;
        let (closed, _) = s.is_closed(k, &domain, dir, None, false)?;
        if closed {
            continue;
        }

        // Get iso parameter boundaries
        // (a failed CreateIsoBoundaries leaves the curves uninitialised upstream)
        let (crv1, crv2) = match s.create_iso_boundaries(k, dir, a_tol) {
            Ok(Some(c)) => c,
            Err(e) if e.is_unported() => return Err(e),
            _ => {
                return Err(invalid(
                    "no iso boundaries (upstream reads uninitialised curves)",
                ))
            }
        };

        if crv1.is_degenerate(k, EFF_ZERO, None)? || crv2.is_degenerate(k, EFF_ZERO, None)? {
            continue;
        }

        // Look for partial coincidence or G0 discontinuous
        let x_tol = xsect_tol_3d_from_zones(
            k,
            zone_tol_3d_of_curve(k, &crv1)?,
            zone_tol_3d_of_curve(k, &crv2)?,
        );
        let ivl1 = crv1.natural_interval();
        let ivl2 = crv2.natural_interval();
        let found = crv1.global_coincidence_checker(
            k,
            &ivl1,
            &crv2,
            &ivl2,
            x_tol,
            true,
            [
                Some(OperandObject::Curve(0, ivl1)),
                Some(OperandObject::Curve(1, ivl2)),
            ],
        )?;

        // Range of Values implies G0 discontinuous non-seam closure of surface
        if found
            .solutions
            .iter()
            .any(|sol| sol.solution_type == SolutionType::RangeOfValues)
        {
            return Ok(false);
        }
    }

    // Good Surfaces are closed or don't have a non-seam closure
    Ok(true)
}

// ---- SmBSplineSurface ---------------------------------------------------------------

/// The `SmBSplineSurface` part of a surface derived from it, as it is now (no NURB is
/// made); `None` for the other kinds.
fn b_spline_part(s: &Surface) -> Option<&BSplineSurface> {
    match s.kind() {
        SurfaceKind::BSpline(b) => Some(b),
        SurfaceKind::Plane(p) => Some(p.base()),
        SurfaceKind::Cone(c) => Some(c.rev().base()),
        SurfaceKind::Sphere(sp) => Some(sp.base().base()),
        SurfaceKind::Torus(t) => Some(t.base().base()),
        SurfaceKind::SurfOfRevolution(sr) => Some(sr.base()),
        SurfaceKind::SurfOfExtrusion(e) => Some(e.base()),
        _ => None,
    }
}

/// `m_pNurb != NULL` of the `SmBSplineSurface` part, as it is now.
fn has_nurb(s: &Surface) -> bool {
    b_spline_part(s).is_some_and(|b| b.gw_nurb().is_some())
}

/// `MakeNurb` of a surface without NURB (`SM_CONST_CAST`; status ignored), then the
/// `SmBSplineSurface` part.
fn make_nurb<'a>(k: &Kernel, s: &'a Surface) -> Result<Option<Cow<'a, BSplineSurface>>> {
    Ok(match s.kind() {
        SurfaceKind::Plane(p) => Some(p.base_for_query()),
        SurfaceKind::BSpline(b) => Some(Cow::Borrowed(b)),
        _ => {
            if !has_nurb(s) {
                s.kind_with_nurb(k)?;
            }
            b_spline_part(s).map(Cow::Borrowed)
        }
    })
}

/// `SmBSplineSurface::AssertValid`.
fn b_spline_surface_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    let mut rtn = true;

    rtn &= if r.bases() {
        surface_assert_valid(k, s, r)?
    } else {
        true
    };

    let part = make_nurb(k, s)?.ok_or_else(|| invalid("not an SmBSplineSurface"))?;
    let has = part.gw_nurb().is_some();

    /*  0 */
    // m_pNurb can't be non-NULL"
    rtn &= r.report(B_SPLINE_SURFACE_TYPE, 0, 0, has, UNDEF, UNDEF)?;

    /*  1 */
    // Knot multiplicity too high
    let high = has_knot_multiplicity_greater_than_degree(&part)?;
    rtn &= r.report(B_SPLINE_SURFACE_TYPE, 1, 0, !high, EFF_ZERO, UNDEF)?;

    let tol = ZONE_TOL_3D / 10.0; // gwc: need a real tolerance here - this one is just made up.

    let degen_point = s.is_degenerate_point(k, ZONE_TOL_3D / 10.0, None)?;
    let mut degen_curve = false;
    let mut internal_pole = false;
    let mut degen_row = false;

    if !degen_point {
        degen_curve = s.is_degenerate_curve(k, EFF_ZERO_DEG, None, ZONE_TOL_3D / 10.0)?;
        if !degen_curve {
            /*  2 */
            // Surfaces may not have internal poles caused by sets of coincident CPts
            internal_pole = has_internal_pole(k, s, &part, tol)?;
            rtn &= r.report(B_SPLINE_SURFACE_TYPE, 2, 0, !internal_pole, tol, UNDEF)?;

            if !internal_pole {
                /*  3 */
                // Surface has an internal degenerate row or column of control points
                degen_row = has_degenerate_control_point_row(&part, tol)?;
                rtn &= r.report(B_SPLINE_SURFACE_TYPE, 3, 0, !degen_row, tol, UNDEF)?;
            }

            /*  4 */
            // (repeated end control point rows only WARN: not run, see the module docs)
        }
    }

    /*  5 */
    // when surface is not degenerate - check for Planar Control Net that doubles back on itself
    if !degen_point && !degen_curve && !internal_pole && !degen_row {
        let uv_domain = s.natural_uv_domain_with_nurb(k)?;
        let (closed_row, _) = s.is_closed(k, &uv_domain, SurfParamType::U, Some(tol), false)?;
        let (closed_col, _) = s.is_closed(k, &uv_domain, SurfParamType::V, Some(tol), false)?;
        let net = part.control_point_net()?;

        // (bClosedRow is passed as the grid's bClosedCol and bClosedCol as its
        // bClosedRow, as upstream)
        let non_monotonic =
            grid_has_non_monotonic_points(&net.points, net.v_count, closed_row, closed_col)?;
        rtn &= r.report(B_SPLINE_SURFACE_TYPE, 5, 0, !non_monotonic, UNDEF, UNDEF)?;
    }

    Ok(rtn)
}

/// `SmBSplineSurface::HasKnotMultiplicityGreaterThanDegree`
/// (`sm_HasKnotMultiplicityGreaterThanDegree`): an end knot of multiplicity above
/// `degree + 1` or an interior knot above `degree`, in u or v. `false` without NURB.
///
/// # Errors
///
/// Those of the knot queries.
pub fn has_knot_multiplicity_greater_than_degree(b: &BSplineSurface) -> Result<bool> {
    // no work - no BSplineSurface representation
    if b.gw_nurb().is_none() {
        return Ok(false);
    }
    for dir in [SurfParamType::U, SurfParamType::V] {
        let deg = b.degree(dir);
        let (_, mults) = b.knots(dir, None)?;
        let last = mults.len().wrapping_sub(1);
        for (ii, &m) in mults.iter().enumerate() {
            if ii == 0 || ii == last {
                if m > deg + 1 {
                    return Ok(true);
                }
            } else if m > deg {
                return Ok(true);
            }
        }
    }

    // all done
    Ok(false)
}

/// The Euclidean control point `Pw[i][j]` of `net` (`v_count` points per row).
fn net_point(points: &[Point3], v_count: usize, i: i64, j: i64) -> Result<Point3> {
    let (Ok(i), Ok(j)) = (usize::try_from(i), usize::try_from(j)) else {
        return Err(invalid("control point index before the net"));
    };
    if j >= v_count {
        return Err(invalid("control point index past the net"));
    }
    i.checked_mul(v_count)
        .and_then(|x| x.checked_add(j))
        .and_then(|x| points.get(x))
        .copied()
        .ok_or_else(|| invalid("control point index past the net"))
}

fn count(v: usize) -> Result<i64> {
    i64::try_from(v).map_err(|_| invalid("count out of range"))
}

fn index(v: i64) -> Result<usize> {
    usize::try_from(v).map_err(|_| invalid("index before the start of the net"))
}

/// `SmBSplineSurface::HasInternalPole`: some interior knot line has a patch boundary
/// span whose control points all lie within `tol` (`<= 0`: `SM_ZONE_TOL_3D`) of one
/// another (for the u lines, also a singularity, `IsSingularity` of `s`).
/// `false` without NURB. The row tested at each interior knot is upstream's unless
/// [`UpstreamFixes::internal_pole_row_through_knot`](crate::kernel::UpstreamFixes::internal_pole_row_through_knot)
/// (WR-505, #1515) is on.
///
/// # Errors
///
/// Those of the knot, Greville and singularity queries; [`SolidError::InvalidInput`]
/// where upstream's index arithmetic leaves the control net.
pub fn has_internal_pole(k: &Kernel, s: &Surface, b: &BSplineSurface, tol: f64) -> Result<bool> {
    // init return
    let mut rtn = false;

    // no work - no BSplineSurface representation
    if b.gw_nurb().is_none() {
        return Ok(false);
    }

    // locals
    let degree_u = count(b.degree(SurfParamType::U))?;
    let degree_v = count(b.degree(SurfParamType::V))?;
    let (unique_u, mults_u) = b.knots(SurfParamType::U, None)?;
    let (unique_v, mults_v) = b.knots(SurfParamType::V, None)?;
    let net = b.control_point_net()?;
    let pt = |i: i64, j: i64| net_point(&net.points, net.v_count, i, j);
    let mult = |m: &[usize], i: usize| -> Result<i64> {
        count(
            *m.get(i)
                .ok_or_else(|| invalid("missing knot multiplicity"))?,
        )
    };

    // pick tolerance
    let tolerance = if tol > 0.0 { tol } else { ZONE_TOL_3D };

    // first walk U Knot vector looking for poles in the V direction along the u = UMin patch boundary
    let mut knt_cnt_u = mult(&mults_u, 0)?;
    let mut ii = 1;
    while ii + 1 < unique_u.len() && !rtn {
        knt_cnt_u += mult(&mults_u, ii)?;

        // one additional basis function becomes zero for each multiplicity at sMults[ii]
        let row_cnt_u = degree_u - (mult(&mults_u, ii)? - 1);
        // upstream moves one row further for each repeated copy of the knot; with the opt-in
        // the row through the knot (WR-505, #1515; source fork#41)
        let cp_index_u = if k.fixes.internal_pole_row_through_knot {
            knt_cnt_u - 1 - degree_u
        } else {
            knt_cnt_u - row_cnt_u - 1
        };

        // for every V direction knot span - seek patch arrays of coincident control points
        let mut knt_cnt_v = 0;
        let mut jj = 0;
        while jj + 1 < unique_v.len() && !rtn {
            knt_cnt_v += mult(&mults_v, jj)?;

            let cp_index_v = (knt_cnt_v - 1) - degree_v;

            let p = pt(cp_index_u, cp_index_v)?;

            let mut test = true;

            // for every UDir neighbor ControlPoint - check for coincidence
            let mut kk = row_cnt_u - 1;
            while kk >= 0 && test {
                let mut ll = degree_v;
                while ll > 0 && test {
                    // check neighbor ControlPoint for Coincidence
                    let next = pt(cp_index_u + kk, cp_index_v + ll)?;
                    test &= p.closer_than(tolerance, next);
                    ll -= 1;
                }
                kk -= 1;
            }

            // These tests don't guarantee a singularity: check the point.  [B597]
            if test {
                // Get uv for the first control point tested.
                let uv = Point2::new(
                    b.greville_abscissa(SurfParamType::U, index(cp_index_u + row_cnt_u - 1)?)?,
                    b.greville_abscissa(SurfParamType::V, index(cp_index_v + degree_v)?)?,
                );
                if !s.is_singularity(k, uv, tolerance, false)?.singular {
                    test = false;
                }
            }

            // remember when patch is degenerate
            rtn |= test;
            jj += 1;
        }
        ii += 1;
    }

    // second walk V Knot vector looking for poles in the U direction along the v = VMin patch boundary
    let mut knt_cnt_v = mult(&mults_v, 0)?;
    let mut ii = 1;
    while ii + 1 < unique_v.len() && !rtn {
        knt_cnt_v += mult(&mults_v, ii)?;

        let row_cnt_v = degree_v - (mult(&mults_v, ii)? - 1);
        // as the U walk: the row through the knot with the opt-in (WR-505, #1515)
        let cp_index_v = if k.fixes.internal_pole_row_through_knot {
            knt_cnt_v - 1 - degree_v
        } else {
            knt_cnt_v - row_cnt_v - 1
        };

        // for every U direction knot span - seek patch arrays of coincident control points
        let mut knt_cnt_u = 0;
        let mut jj = 0;
        while jj + 1 < unique_u.len() && !rtn {
            knt_cnt_u += mult(&mults_u, jj)?;

            let cp_index_u = (knt_cnt_u - 1) - degree_u;

            let p = pt(cp_index_u, cp_index_v)?;

            let mut test = true;

            // for every VDir neighbor ControlPoint - check for coincidence
            let mut kk = row_cnt_v - 1;
            while kk >= 0 && test {
                let mut ll = degree_u;
                while ll > 0 && test {
                    // check neighbor ControlPoint for Coincidence
                    let next = pt(cp_index_u + ll, cp_index_v + kk)?;
                    test &= p.closer_than(tolerance, next);
                    ll -= 1;
                }
                kk -= 1;
            }

            // remember when patch is degenerate
            rtn |= test;
            jj += 1;
        }
        ii += 1;
    }

    // all done
    Ok(rtn)
}

/// `SmBSplineSurface::HasDegenerateControlPointRow`: some interior row (or column) of
/// control points has a box whose largest absolute coordinate
/// (`SmExtent3d::GetMaxDimension`) is within `tol` (`<= 0`: `SM_ZONE_TOL_3D`).
/// `false` without NURB.
///
/// # Errors
///
/// Those of the control net query.
pub fn has_degenerate_control_point_row(b: &BSplineSurface, tol: f64) -> Result<bool> {
    if b.gw_nurb().is_none() {
        return Ok(false);
    }

    let tolerance = if tol > 0.0 { tol } else { ZONE_TOL_3D };

    let net = b.control_point_net()?;
    let pt = |i: i64, j: i64| net_point(&net.points, net.v_count, i, j);
    let i_n = count(net.u_count)? - 1; // max 1st index in PW
    let i_m = count(net.v_count)? - 1; // max 2nd index in PW
    let mut row_box = Extent3d::new();
    let mut degen = true;

    let mut ii = 1;
    while ii < i_n && degen {
        let p = pt(ii, 0)?;
        degen = true;
        row_box.init();
        row_box.add_point(p);
        let mut jj = 1;
        while jj <= i_m && degen {
            let next = pt(ii, jj)?;
            row_box.add_point(next);
            if row_box.max_dimension() > tolerance {
                degen = false;
            }
            jj += 1;
        }
        if degen {
            return Ok(degen);
        }
        ii += 1;
    }

    let mut jj = 1;
    while jj < i_m && degen {
        let p = pt(0, jj)?;
        degen = true;
        row_box.init();
        row_box.add_point(p);
        let mut ii = 1;
        while ii <= i_n && degen {
            let next = pt(ii, jj)?;
            row_box.add_point(next);
            if row_box.max_dimension() > tolerance {
                degen = false;
            }
            ii += 1;
        }
        if degen {
            return Ok(degen);
        }
        jj += 1;
    }

    Ok(false)
}

/// `SmPointGrid(sPoints, lRowSize, dTol3d, bClosedCol, bClosedRow, ...)` followed by
/// `HasNonMonotonicPoints` (`SetUpMonotonic`): whether a row or column of the grid of
/// `points` (`row_size` per row) turns back by more than 170 degrees between
/// neighbouring differences (coincident pairs skipped). As upstream, a failing column
/// clears the row flag.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a zero row size (upstream divides by zero) or
/// an index past the points (a partial last row).
pub fn grid_has_non_monotonic_points(
    points: &[Point3],
    row_size: usize,
    closed_col: bool,
    closed_row: bool,
) -> Result<bool> {
    if row_size == 0 {
        return Err(invalid("point grid row size is zero"));
    }
    let at = |i: usize| -> Result<Point3> {
        points
            .get(i)
            .copied()
            .ok_or_else(|| invalid("point grid index past the points"))
    };
    let size = points.len();
    let col_cnt = row_size;
    let row_cnt = size / row_size;
    let monotonic_col = true;
    let mut monotonic_row = true;

    // turn test between two successive differences
    let turns_back = |v1: Vec3, v0: Vec3| -> bool {
        let ang_rad = v1.angle_between(v0).unwrap_or(0.0); // out: 0 to Pi
        rad_to_deg(ang_rad) > 170.0
    };

    // First the rows (constant i, varying j)
    let mut idx = 0;
    let mut ii = 0;
    while ii < row_cnt && monotonic_row {
        // pick iteration bounds for closed and open cases
        let (mut j0, mut j1) = if closed_row {
            (idx + row_size - 1, idx)
        } else {
            (idx, idx + 1)
        };

        // get first point pair difference
        let mut vec0 = at(j1)? - at(j0)?;
        let mut len_sq0 = vec0.length_squared();

        // next row start
        idx += row_size;

        // for every pair of points in this row
        j1 += 1;
        j0 = j1 - 1;
        while j1 < idx {
            let vec1 = at(j1)? - at(j0)?;
            let len_sq1 = vec1.length_squared();

            // skip coincident control points
            if len_sq0 < EFF_ZERO_SQ || len_sq1 < EFF_ZERO_SQ {
                vec0 = vec1;
                len_sq0 = len_sq1;
                j1 += 1;
                j0 = j1 - 1;
                continue;
            }

            // look for change in direction
            if turns_back(vec1, vec0) {
                monotonic_row = false;
                break;
            }

            vec0 = vec1;
            j1 += 1;
            j0 = j1 - 1;
        }
        ii += 1;
    }

    // for every col
    let mut idx = 0;
    let mut jj = 0;
    while jj < col_cnt && monotonic_col {
        // pick iteration bounds for closed and open cases
        let (mut i0, mut i1) = if closed_col {
            ((idx + size).wrapping_sub(row_size), idx)
        } else {
            (idx, idx + row_size)
        };

        // get first point pair difference
        let mut vec0 = at(i1)? - at(i0)?;
        let mut len_sq0 = vec0.length_squared();

        // next col start
        idx += 1;

        // for every pair of points in this col
        i1 += row_size;
        i0 = i1 - row_size;
        while i1 < size {
            let vec1 = at(i1)? - at(i0)?;
            let len_sq1 = vec1.length_squared();

            // skip coincident control points
            if len_sq0 < EFF_ZERO_SQ || len_sq1 < EFF_ZERO_SQ {
                vec0 = vec1;
                len_sq0 = len_sq1;
                i1 += row_size;
                i0 = i1 - row_size;
                continue;
            }

            // look for change in direction
            if turns_back(vec1, vec0) {
                // (upstream marks the row flag here)
                monotonic_row = false;
                break;
            }

            vec0 = vec1;
            i1 += row_size;
            i0 = i1 - row_size;
        }
        jj += 1;
    }

    // HasNonMonotonicPoints
    Ok(!(monotonic_col && monotonic_row))
}

/// The `SmBSplineCurve` part of a curve derived from it, with its NURB
/// (`GetOrCreateGwNurbPointer`; a failed make is ignored).
fn curve_b_spline<'a>(k: &Kernel, c: &'a Curve) -> Result<Cow<'a, BSplineCurve>> {
    Ok(match c.kind() {
        CurveKind::BSpline(b) => Cow::Borrowed(b),
        CurveKind::Line(l) => l.base_with_nurb()?,
        CurveKind::Ellipse(e) => {
            if e.base().gw_nurb().is_some() {
                Cow::Borrowed(e.base())
            } else {
                let mut e = e.clone();
                ignore_status(e.make_nurb(k))?;
                Cow::Owned(e.base().clone())
            }
        }
        CurveKind::Hyperbola(h) => {
            if h.base().gw_nurb().is_some() {
                Cow::Borrowed(h.base())
            } else {
                let mut h = h.clone();
                ignore_status(h.make_nurb(None))?;
                Cow::Owned(h.base().clone())
            }
        }
        CurveKind::Parabola(p) => {
            if p.base().gw_nurb().is_some() {
                Cow::Borrowed(p.base())
            } else {
                let mut p = p.clone();
                ignore_status(p.make_nurb(None))?;
                Cow::Owned(p.base().clone())
            }
        }
        _ => return Err(invalid("curve is not an SmBSplineCurve")),
    })
}

/// `GetSTEPInterval` of a curve derived from `SmBSplineCurve`.
fn curve_step_interval(c: &Curve) -> Result<Extent1d> {
    crate::surface::surf_of_revolution::p1::gen_step_interval(c)
}

/// `SmBSplineSurface::HasSameParameterization(eAnalyticDirection, eNurbDirection,
/// cpOtherCurve)` of the curve with STEP interval `curve_anal` and unique knots and
/// multiplicities `curve_knots`.
fn has_same_parameterization(
    k: &Kernel,
    s: &Surface,
    analytic_dir: SurfParamType,
    nurb_dir: SurfParamType,
    curve_anal: Extent1d,
    curve_knots: &(Vec<f64>, Vec<usize>),
) -> Result<bool> {
    // surface and Curve analytic parameterization locals
    let step = s.step_uv_domain();
    let surface_anal = if analytic_dir == SurfParamType::U {
        step.u_interval()
    } else {
        step.v_interval()
    };

    // surface and curve Nurb parameterization locals
    let (surface_knots, surface_mults) = s.knots_with_nurb(k, nurb_dir, None)?;
    let (curve_knots, curve_mults) = curve_knots;

    // The surface and genCurve analytic domains must be compatible
    let tol = EFF_ZERO * (1.0 + max2(surface_anal.max_dimension(), curve_anal.max_dimension()));
    let mut rtn = surface_anal.is_contained_by(&curve_anal, tol)
        && curve_anal.is_contained_by(&surface_anal, tol);

    // The surface and genCurve knot vectors must be the same
    if surface_knots.len() != curve_knots.len() {
        rtn = false;
    }

    // for every knot and multiplicity
    let mut ii = 0;
    while ii < surface_knots.len() && rtn {
        let (Some(&sk), Some(&ck), Some(&sm), Some(&cm)) = (
            surface_knots.get(ii),
            curve_knots.get(ii),
            surface_mults.get(ii),
            curve_mults.get(ii),
        ) else {
            return Err(invalid("knot and multiplicity counts differ"));
        };
        let mut test = are_same(sk, ck);
        rtn &= test;
        test &= sm == cm;
        rtn &= test;
        ii += 1;
    }

    // all done
    Ok(rtn)
}

/// [`has_same_parameterization`] of a curve derived from `SmBSplineCurve`.
fn has_same_parameterization_curve(
    k: &Kernel,
    s: &Surface,
    analytic_dir: SurfParamType,
    nurb_dir: SurfParamType,
    curve: &Curve,
) -> Result<bool> {
    let anal = curve_step_interval(curve)?;
    let knots = curve_b_spline(k, curve)?.knots(None)?;
    has_same_parameterization(k, s, analytic_dir, nurb_dir, anal, &knots)
}

// ---- SmAxis2Placement ---------------------------------------------------------------

/// `SmAxis2Placement::AssertValid` (`SM_LEVEL_0`): orthogonal unit x and y axes,
/// reported about `key` into `list`.
///
/// # Errors
///
/// Those of [`AssertReport::new`].
pub fn axis2_placement_assert_valid(
    p: &Axis2Placement,
    list: Option<&mut AssertArray>,
    key: ObjectKey,
) -> Result<bool> {
    let mut r = Reporter {
        list,
        key,
        level: 0,
        class_string: "SmAxis2Placement",
    };
    placement_assert_valid(p, &mut r)
}

fn placement_assert_valid(p: &Axis2Placement, r: &mut Reporter<'_>) -> Result<bool> {
    const CLASS: i64 = AXIS2_PLACEMENT_TYPE;
    const NAME: &str = "SmAxis2Placement";
    let x = p.x_axis();
    let y = p.y_axis();
    let mut rtn = true;

    // m_vXAxis and m_vYAxis are orthogonal
    let d0 = x.dot(y).abs();
    rtn &= r.report_as(CLASS, CLASS, NAME, 0, 0, d0 < EFF_ZERO, EFF_ZERO, d0)?;

    // m_vXAxis is a unit vector
    let d1 = (x.dot(x) - 1.0).abs();
    rtn &= r.report_as(CLASS, CLASS, NAME, 1, 0, d1 < EFF_ZERO, EFF_ZERO, d1)?;

    // m_vYAxis is a unit vector (the value is not made absolute, as upstream)
    let d2 = y.dot(y) - 1.0;
    rtn &= r.report_as(CLASS, CLASS, NAME, 2, 0, d2.abs() < EFF_ZERO, EFF_ZERO, d2)?;

    Ok(rtn)
}

// ---- SmPlane ------------------------------------------------------------------------

fn plane_of(s: &Surface) -> Result<&Plane> {
    match s.kind() {
        SurfaceKind::Plane(p) => Ok(p),
        _ => Err(invalid("not a plane")),
    }
}

/// `SmPlane::AssertValid`.
fn plane_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    let mut rtn = true;

    rtn &= if r.bases() {
        b_spline_surface_assert_valid(k, s, r)?
    } else {
        true
    };

    let p = plane_of(s)?;
    rtn &= placement_assert_valid(p.position(), r)?;

    let anal = p.step_uv_domain();
    rtn &= r.report(
        PLANE_TYPE,
        0,
        0,
        anal.x_length() > EFF_ZERO,
        EFF_ZERO,
        anal.x_length(),
    )?;
    rtn &= r.report(
        PLANE_TYPE,
        1,
        0,
        anal.y_length() > EFF_ZERO,
        EFF_ZERO,
        anal.y_length(),
    )?;

    if p.base().gw_nurb().is_some() {
        let natural = s.natural_uv_domain_with_nurb(k)?;

        rtn &= r.report(
            PLANE_TYPE,
            2,
            0,
            natural.are_equal(&anal, EFF_ZERO),
            EFF_ZERO,
            UNDEF,
        )?;

        for uv in [
            Point2::new(natural.u_min(), natural.v_min()),
            Point2::new(natural.u_max(), natural.v_max()),
        ] {
            let anal_pt = s.evaluate_point(k, uv)?;
            let nurb_pt = p.base().evaluate_point(uv)?;
            let scaled_zero = EFF_ZERO * (1.0 + anal_pt.max_dimension());
            rtn &= r.report(
                PLANE_TYPE,
                5,
                0,
                anal_pt.closer_than(scaled_zero, nurb_pt),
                scaled_zero,
                anal_pt.distance(nurb_pt),
            )?;
        }
    }

    let scale = p.uv_scale();
    rtn &= r.report(PLANE_TYPE, 3, 0, scale.x > EFF_ZERO, EFF_ZERO, scale.x)?;
    rtn &= r.report(PLANE_TYPE, 4, 0, scale.y > EFF_ZERO, EFF_ZERO, scale.y)?;

    Ok(rtn)
}

// ---- SmSurfOfRevolution and derived classes --------------------------------------------

/// The `SmSurfOfRevolution` part of a cone, cylinder, sphere, torus or surface of
/// revolution, as it is now.
fn rev_of(s: &Surface) -> Result<&SurfOfRevolution> {
    match s.kind() {
        SurfaceKind::Cone(c) => Ok(c.rev()),
        SurfaceKind::Sphere(sp) => Ok(sp.base()),
        SurfaceKind::Torus(t) => Ok(t.base()),
        SurfaceKind::SurfOfRevolution(sr) => Ok(sr),
        _ => Err(invalid("not a surface of revolution")),
    }
}

fn cone_of(s: &Surface) -> Result<&Cone> {
    match s.kind() {
        SurfaceKind::Cone(c) => Ok(c),
        _ => Err(invalid("not a cone")),
    }
}

fn sphere_of(s: &Surface) -> Result<&Sphere> {
    match s.kind() {
        SurfaceKind::Sphere(sp) => Ok(sp),
        _ => Err(invalid("not a sphere")),
    }
}

fn torus_of(s: &Surface) -> Result<&Torus> {
    match s.kind() {
        SurfaceKind::Torus(t) => Ok(t),
        _ => Err(invalid("not a torus")),
    }
}

/// `GetInsideOut` of a curve derived from `SmBSplineCurve` (only lines and conics
/// have the bit).
fn curve_inside_out(c: &Curve) -> bool {
    match c.kind() {
        CurveKind::Line(l) => l.inside_out(),
        CurveKind::Ellipse(e) => e.inside_out(),
        _ => false,
    }
}

/// `GetInsideOut` of the surface (cone, sphere and torus have the bit).
fn surface_inside_out(s: &Surface) -> bool {
    match s.kind() {
        SurfaceKind::Cone(c) => c.inside_out(),
        SurfaceKind::Sphere(sp) => sp.inside_out(),
        SurfaceKind::Torus(t) => t.inside_out(),
        _ => false,
    }
}

/// `CreateGeneratorFromAngle(crContext, 0.0, pGenCurve)` followed by
/// `HasSameParameterization(SM_SP_V, m_bSwapUV ? SM_SP_U : SM_SP_V, pGenCurve)`.
fn generator_has_same_parameterization(k: &Kernel, s: &Surface) -> Result<bool> {
    let rev = rev_of(s)?;
    let gen = rev.create_generator_from_angle(k, 0.0)?;
    let nurb_dir = if rev.swap_uv() {
        SurfParamType::U
    } else {
        SurfParamType::V
    };
    has_same_parameterization_curve(k, s, SurfParamType::V, nurb_dir, &gen)
}

/// `SmSurfOfRevolution::AssertValid`.
#[allow(clippy::too_many_lines)]
fn surf_of_revolution_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    const CLASS: i64 = SURF_OF_REVOLUTION_TYPE;
    let mut rtn = true;

    if has_nurb(s) {
        rtn &= if r.bases() {
            b_spline_surface_assert_valid(k, s, r)?
        } else {
            true
        };
    }

    /*  0 */
    // m_pGenCurve shares the same context (one kernel: always)
    rtn &= r.report(CLASS, 0, 0, true, UNDEF, UNDEF)?;

    /*  1 */
    // m_vPolarConverter shares the same context (one kernel: always)
    rtn &= r.report(CLASS, 1, 0, true, UNDEF, UNDEF)?;

    let rev = rev_of(s)?;
    let anal = rev.step_uv_domain();
    if let Some(gen) = rev.gen_curve() {
        /* 11 */
        // GenCurve and Surface InsideOut bit values must match
        rtn &= r.report(
            CLASS,
            11,
            0,
            curve_inside_out(gen) == surface_inside_out(s),
            UNDEF,
            UNDEF,
        )?;

        if has_nurb(s) {
            /*  2 */
            // GenCurve Anal and NURB intervals must match appropriate Surface Domain Intervals
            let nurb_dir = if rev.swap_uv() {
                SurfParamType::U
            } else {
                SurfParamType::V
            };
            let same = has_same_parameterization_curve(k, s, SurfParamType::V, nurb_dir, gen)?;
            rtn &= r.report(CLASS, 2, 0, same, EFF_ZERO, UNDEF)?;

            /*  3 */
            // compare a GenCurve near midPoint evaluation
            if rtn && rev.polar_converter().is_polar_conversion_possible() {
                let step_uv = anal.evaluate(0.0, 0.4567);
                let nurb_uv = s.convert_uv_from_step_to_nurbs(k, step_uv)?;
                let step_point = s.evaluate_step_point(k, step_uv)?;
                let nurb_point = s.evaluate_point(k, nurb_uv)?;
                let scaled_zero = EFF_ZERO * (1.0 + step_point.max_dimension());
                rtn &= r.report(
                    CLASS,
                    3,
                    0,
                    step_point.closer_than(scaled_zero, nurb_point),
                    scaled_zero,
                    step_point.distance(nurb_point),
                )?;
            }
        }
    }

    let rev = rev_of(s)?;
    let scaled_ang_zero = EFF_ZERO * 100.0 * 360.0;
    let sweep_ivl_deg = anal.u_interval();
    let sweep_min_deg = sweep_ivl_deg.min();
    let sweep_max_deg = sweep_ivl_deg.max();
    let polar = rev.polar_converter();
    let ang_count = polar.angles.len();

    if rtn && polar.is_polar_conversion_possible() {
        let first_ang_deg = *polar
            .angles
            .first()
            .ok_or_else(|| invalid("empty polar angle table"))?;
        let last_ang_deg = *polar
            .angles
            .last()
            .ok_or_else(|| invalid("empty polar angle table"))?;

        /*  4 */
        // lAngCount must be greater than one
        rtn &= r.report(CLASS, 4, 0, ang_count > 1, UNDEF, UNDEF)?;

        /*  5 */
        // First polar angle is start SweepInterval bounds
        rtn &= r.report(
            CLASS,
            5,
            0,
            are_same_to_tol(first_ang_deg, sweep_min_deg, scaled_ang_zero)
                || are_same_to_tol(first_ang_deg, sweep_max_deg, scaled_ang_zero),
            scaled_ang_zero,
            max2(
                (first_ang_deg - sweep_min_deg).abs(),
                (first_ang_deg - sweep_max_deg).abs(),
            ),
        )?;

        /*  6 */
        // Last polar angle must equal end SweepInterval bounds
        rtn &= r.report(
            CLASS,
            6,
            0,
            are_same_to_tol(last_ang_deg, sweep_min_deg, scaled_ang_zero)
                || are_same_to_tol(last_ang_deg, sweep_max_deg, scaled_ang_zero),
            scaled_ang_zero,
            max2(
                (last_ang_deg - sweep_min_deg).abs(),
                (last_ang_deg - sweep_max_deg).abs(),
            ),
        )?;
    }

    if let Some(polar_curve) = rev.polar_curve() {
        /* 12 */
        // PolarCurve InsideOut bit values must be FALSE
        rtn &= r.report(CLASS, 12, 0, !polar_curve.inside_out(), UNDEF, UNDEF)?;

        if has_nurb(s) {
            /*  7 */
            // PolarCurve Anal and NURB intervals must match appropriate Surface Domain Intervals
            let nurb_dir = if rev.swap_uv() {
                SurfParamType::V
            } else {
                SurfParamType::U
            };
            let polar_b = if polar_curve.base().gw_nurb().is_some() {
                Cow::Borrowed(polar_curve.base())
            } else {
                let mut e = polar_curve.clone();
                ignore_status(e.make_nurb(k))?;
                Cow::Owned(e.base().clone())
            };
            let knots = polar_b.knots(None)?;
            let same = has_same_parameterization(
                k,
                s,
                SurfParamType::U,
                nurb_dir,
                polar_curve.step_interval(),
                &knots,
            )?;
            rtn &= r.report(CLASS, 7, 0, same, EFF_ZERO, UNDEF)?;

            /*  8 */
            // compare a Sweep near midPoint evaluation
            if rtn && rev.polar_converter().is_polar_conversion_possible() {
                let step_uv = anal.evaluate(0.4567, 0.7654);
                let nurb_uv = s.convert_uv_from_step_to_nurbs(k, step_uv)?;
                let step_point = s.evaluate_step_point(k, step_uv)?;
                let nurb_point = s.evaluate_point(k, nurb_uv)?;
                let scaled_zero = EFF_ZERO * 10.0 * (1.0 + step_point.max_dimension());
                let dist = step_point.distance(nurb_point);
                rtn &= r.report(CLASS, 8, 0, dist < scaled_zero, scaled_zero, dist)?;
            }
        }
    }

    // SurfaceNurb/GenCurveNurb compatibility
    let rev = rev_of(s)?;
    if let (true, Some(gen)) = (has_nurb(s), rev.gen_curve()) {
        const COUNT: usize = 4;
        let part = b_spline_part(s).ok_or_else(|| invalid("not an SmBSplineSurface"))?;
        let uv_domain = s.natural_uv_domain_with_nurb(k)?;
        let ivl = gen.natural_interval();
        let anal_min_x = anal.min().x;
        let cos = (anal_min_x * PI / 180.0).cos();
        let sin = (anal_min_x * PI / 180.0).sin();
        let pos = rev.position();
        let o = pos.origin();
        let x = pos.x_axis();
        let y = pos.y_axis();
        let z = pos.z_axis();
        let gen_b = curve_b_spline(k, gen)?;
        let swap_uv = rev.swap_uv();

        for ii in 0..COUNT {
            let param = if ii == 0 {
                0.0
            } else if ii == COUNT - 1 {
                1.0
            } else {
                ii as f64 / (COUNT - 1) as f64
            };
            let surf_u0 = part.evaluate_point(uv_domain.evaluate(0.0, param))?;
            let surf_v0 = part.evaluate_point(uv_domain.evaluate(param, 0.0))?;
            let mut crv_point = gen_b.evaluate_point(ivl.evaluate(param))?;
            let (d_r, _) = line_point_distance(o, z, crv_point);
            crv_point = crv_point - d_r * x + d_r * (cos * x + sin * y);
            let scaled_zero = EFF_ZERO * 100.0 * (1.0 + crv_point.max_dimension());
            let dist_u0 = (crv_point - surf_u0).length();
            let dist_v0 = (crv_point - surf_v0).length();

            if swap_uv {
                /*  9 */
                // A GenCurve eval does not map to Surface eval within dScaledZero
                rtn &= r.report(CLASS, 9, 0, dist_v0 < scaled_zero, scaled_zero, dist_v0)?;
            } else {
                /* 10 */
                // A GenCurve eval does not map to Surface eval within dScaledZero
                rtn &= r.report(CLASS, 10, 0, dist_u0 < scaled_zero, scaled_zero, dist_u0)?;
            }
        }
    }

    /* 13 */
    // Analytic U Direction Domain too large (reported as test 2, as upstream; as 13, its
    // label, with WR-538's `torus_check_reads_v_interval`)
    let ivl_u = anal.u_interval();
    rtn &= r.report(
        CLASS,
        if k.fixes.torus_check_reads_v_interval {
            13
        } else {
            2
        },
        0,
        ivl_u.length() <= 360.0 + EFF_ZERO_DEG,
        EFF_ZERO_DEG,
        ivl_u.length() - 360.0,
    )?;

    /* 14 */
    // Bad GenCurve Owner (upstream dereferences the generator)
    let rev = rev_of(s)?;
    let gen = rev
        .gen_curve()
        .ok_or_else(|| invalid("surface of revolution without generator"))?;
    let owner_ok = gen.owner().is_none_or(|o| o.key.0 == r.key.0);
    rtn &= r.report(CLASS, 14, 0, owner_ok, UNDEF, UNDEF)?;

    /* 15 */
    // Bad GenCurve length degenerates SurfOfRevolution to an approximate circular curve
    let degenerate = is_degenerate_curve(k, Some(gen), &gen.natural_interval(), None)?;
    rtn &= r.report(CLASS, 15, 0, !degenerate, UNDEF, UNDEF)?;

    // SM_ASSERT_REMOVE_CASCADING_REPORT(15, SmSurface, 5, this)
    if let Some(l) = r.list.as_deref_mut() {
        let key = Some(r.key);
        if l.find_report(key, 15, CLASS, key, None).is_some() {
            while let Some(i) = l.find_report(key, 5, SURFACE_TYPE, key, None) {
                l.remove_at(i);
            }
        }
    }

    Ok(rtn)
}

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

fn extrusion_of(s: &Surface) -> Result<&SurfOfExtrusion> {
    match s.kind() {
        SurfaceKind::SurfOfExtrusion(e) => Ok(e),
        _ => Err(invalid("not an SmSurfOfExtrusion")),
    }
}

/// `SmVector3d::AngleBetween` with its status ignored: the angle it writes, `0` when
/// a vector has zero length.
fn angle_or_zero(a: Vec3, b: Vec3) -> f64 {
    a.angle_between(b).unwrap_or(0.0)
}

/// Whether `angle_rad` is within `SM_EFF_ZERO_RAD` of `0` or `PI`, and its distance to
/// the nearer one (`smos_Min(smos_Fabs(a), smos_Fabs(a - SM_PI))`).
fn parallel_angle(angle_rad: f64) -> (bool, f64) {
    (
        is_zero_to_tol(angle_rad, EFF_ZERO_RAD) || is_zero_to_tol(angle_rad - PI, EFF_ZERO_RAD),
        min2(angle_rad.abs(), (angle_rad - PI).abs()),
    )
}

/// `GlobalPropertyAnalysis(SM_CP_PARALLEL_TO_VECTOR)` of the generator over its
/// natural interval with the extrusion vector: whether it found no solution. The
/// status is ignored, as upstream: a failed analysis leaves `sSolutions` empty (it is
/// reset on entry, and the failing check, `AngleBetween` of a zero-length extrusion
/// vector in `SmCurvePropertyGS::LocalSolve`, runs before a solution is kept), so it
/// counts as none found. [`SolidError::Unported`] still propagates.
fn no_tangent_along(k: &Kernel, gen: &Curve, extrusion_vec: Vec3, tol: f64) -> Result<bool> {
    let vectors = [extrusion_vec];
    let opts = PropertyAnalysisOptions::<OperandObject> {
        property_value: Some(0.0),
        opt_vectors: &vectors,
        want_comb: false,
        handle: None,
    };
    match gen.global_property_analysis(
        k,
        &gen.natural_interval(),
        CurvePropertyType::ParallelToVector,
        tol,
        &opts,
    ) {
        Ok(found) => Ok(found.solutions.is_empty()),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(true),
    }
}

/// `SmSurfOfExtrusion::AssertValid`.
#[allow(clippy::too_many_lines)]
fn surf_of_extrusion_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    const CLASS: i64 = SURF_OF_EXTRUSION_TYPE;
    let mut rtn = true;

    // call the base class AssertValid
    if has_nurb(s) {
        rtn &= if r.bases() {
            b_spline_surface_assert_valid(k, s, r)?
        } else {
            true
        };
    }

    let ext = extrusion_of(s)?;
    let extrusion_vec = ext.extrusion_vector();
    let anal = ext.step_uv_domain();

    // 0. m_vExtrusionVec is initialized and nonZero length
    rtn &= r.report(CLASS, 0, 0, !extrusion_vec.is_undef(), UNDEF, UNDEF)?;
    let length = extrusion_vec.length();
    rtn &= r.report(CLASS, 1, 0, length > EFF_ZERO, EFF_ZERO, length)?;

    // 1. AnalUVDomain Check - it has a nonZero area
    rtn &= r.report(
        CLASS,
        2,
        0,
        anal.x_length() > EFF_ZERO,
        EFF_ZERO,
        anal.x_length(),
    )?;
    rtn &= r.report(
        CLASS,
        3,
        0,
        anal.y_length() > EFF_ZERO,
        EFF_ZERO,
        anal.y_length(),
    )?;

    // 2. Domain consistency
    if let Some(gen) = ext.gen_curve() {
        /* 24 */
        // GenCurve InsideOut bit (if it has one) must be FALSE
        rtn &= r.report(CLASS, 24, 0, !curve_inside_out(gen), UNDEF, UNDEF)?;

        if has_nurb(s) {
            /* 25 */
            // GenCurve must have same parameterization as Surface in GenCurve direction
            let nurb_dir = if ext.swap_uv() {
                SurfParamType::V
            } else {
                SurfParamType::U
            };
            let same = has_same_parameterization_curve(k, s, SurfParamType::U, nurb_dir, gen)?;
            rtn &= r.report(CLASS, 25, 0, same, EFF_ZERO, UNDEF)?;
        }

        /* 26 */
        // GenCurve owner must be this SmSurfOfExtrusion object
        let owner_ok = gen.owner().is_none_or(|o| o.key.0 == r.key.0);
        rtn &= r.report(CLASS, 26, 0, owner_ok, UNDEF, UNDEF)?;

        /* 27 */
        // GenCurve context must be this SmSurfOfExtrusion context (one kernel: always)
        rtn &= r.report(CLASS, 27, 0, true, UNDEF, UNDEF)?;
    }

    // check that STEP and Nurb representations are the same
    let tol_u = EFF_ZERO * (1.0 + anal.u_interval().length());
    let tol_v = EFF_ZERO * (1.0 + anal.v_interval().length());

    if has_nurb(s) {
        const COUNT: usize = 3;
        for ii in 0..=COUNT {
            for jj in 0..=COUNT {
                // check UV conversions
                let step_uv = anal.evaluate(ii as f64 / COUNT as f64, jj as f64 / COUNT as f64);
                let nurb_uv = s.convert_uv_from_step_to_nurbs(k, step_uv)?;
                let test_step_uv = s.convert_uv_from_nurbs_to_step(k, nurb_uv)?;

                let good_u = is_zero_to_tol(step_uv.x - test_step_uv.x, tol_u);
                let good_v = is_zero_to_tol(step_uv.y - test_step_uv.y, tol_v);
                if !good_u || !good_v {
                    rtn &= r.report(CLASS, 5, 0, good_u, tol_u, step_uv.x - test_step_uv.x)?;
                    rtn &= r.report(CLASS, 5, 0, good_v, tol_v, step_uv.y - test_step_uv.y)?;
                }

                // check equivalent evaluations
                let step_point = s.evaluate_step_point(k, step_uv)?;
                let nurb_point = s.evaluate_point(k, nurb_uv)?;
                let dist = step_point.distance(nurb_point);
                rtn &= r.report(
                    CLASS,
                    6,
                    0,
                    is_zero_to_tol(dist, EFF_ZERO_SQRT),
                    EFF_ZERO_SQRT,
                    dist,
                )?;
            }
        }
    }

    // (upstream dereferences the generator from here on)
    let ext = extrusion_of(s)?;
    let gen = ext
        .gen_curve()
        .ok_or_else(|| invalid("surface of extrusion without generator"))?;

    // 3. check that m_vAnalUVDomain->UInterval is defined by the genCurve analytic interval
    let gen_step = curve_step_interval(gen)?;
    rtn &= r.report(
        CLASS,
        7,
        0,
        anal.u_interval().are_equal(&gen_step, tol_u),
        tol_u,
        UNDEF,
    )?;

    // 4. check that m_vOrigin is a point on m_pGenCurve
    let gen_ivl = gen.natural_interval();
    let origin = ext.origin();
    let origin_tol = if k.fixes.extrusion_origin_scaled_zero {
        EFF_ZERO * (1.0 + origin.max_dimension())
    } else {
        EFF_ZERO
    };
    let solutions = gen.global_point_solve(
        k,
        &gen_ivl,
        SolverOperation::Intersect,
        origin,
        origin_tol,
        SolutionRequested::All,
        &GlobalPointSolveOptions::<OperandObject>::default(),
    )?;
    rtn &= r.report(CLASS, 8, 0, !solutions.is_empty(), UNDEF, UNDEF)?;

    // 5. Check cached m_vCurvePoint/m_vCurveVec information
    let pnt = gen.evaluate_point(k, gen_ivl.evaluate(0.45678))?;
    let tol =
        ANALYTIC_TOL_SCALE * EFF_ZERO * (1.0 + ext.origin().max_dimension() + pnt.max_dimension());

    let lin = gen.is_linear(k, tol, false)?;
    let planar = gen.is_planar(k, tol)?;
    let curve_point = ext.curve_point();
    let curve_vec = ext.curve_vec();
    if lin.linear {
        // m_vCurvePoint = Point on line, m_vCurveVec = Line Tangent
        let (dist, _) = line_point_distance(lin.line_point, lin.line_tangent, curve_point);
        let (parallel, off) = parallel_angle(angle_or_zero(lin.line_tangent, curve_vec));
        rtn &= r.report(CLASS, 11, 0, is_zero_to_tol(dist, EFF_ZERO), EFF_ZERO, dist)?;
        rtn &= r.report(CLASS, 12, 0, parallel, EFF_ZERO_RAD, off)?;
    } else if planar.is_planar {
        // m_vCurvePoint = Point on Plane, m_vCurveVec = Plane Normal
        let dist = plane_point_distance(planar.point, planar.normal, curve_point)
            .map_err(|_| invalid("plane distance not computed (uninitialised upstream)"))?;
        let (parallel, off) = parallel_angle(angle_or_zero(planar.normal, curve_vec));
        rtn &= r.report(CLASS, 9, 0, is_zero_to_tol(dist, EFF_ZERO), EFF_ZERO, dist)?;
        rtn &= r.report(CLASS, 10, 0, parallel, EFF_ZERO_RAD, off)?;
    }

    // 6. check that every surface point is unique (no generator tangent along the sweep)
    match ext.orientation() {
        CurveOrientType::Linear => {
            let (parallel, off) = parallel_angle(angle_or_zero(lin.line_tangent, extrusion_vec));
            // geometry description check
            rtn &= r.report(
                CLASS,
                13,
                0,
                lin.linear && planar.is_planar,
                EFF_ZERO,
                UNDEF,
            )?;
            // unique sweep check
            rtn &= r.report(CLASS, 14, 0, !parallel, EFF_ZERO_RAD, off)?;
        }
        CurveOrientType::Perpendicular => {
            let (parallel, off) = parallel_angle(angle_or_zero(extrusion_vec, planar.normal));
            rtn &= r.report(
                CLASS,
                15,
                0,
                !lin.linear && planar.is_planar,
                EFF_ZERO,
                UNDEF,
            )?;
            // orthogonal sweep check
            rtn &= r.report(CLASS, 16, 0, parallel, EFF_ZERO_RAD, off)?;
        }
        CurveOrientType::Independent => {
            let off = angle_or_zero(extrusion_vec, planar.normal) - PI / 2.0;
            rtn &= r.report(
                CLASS,
                17,
                0,
                !lin.linear && planar.is_planar,
                EFF_ZERO,
                UNDEF,
            )?;
            // nonPlanar sweep check
            let nonplanar = !is_zero_to_tol(off, EFF_ZERO_RAD);
            rtn &= r.report(CLASS, 18, 0, nonplanar, EFF_ZERO_RAD, off)?;
        }
        CurveOrientType::Coplanar => {
            let off = angle_or_zero(extrusion_vec, planar.normal) - PI / 2.0;
            rtn &= r.report(
                CLASS,
                19,
                0,
                !lin.linear && planar.is_planar,
                EFF_ZERO,
                UNDEF,
            )?;
            // CoPlanar check
            rtn &= r.report(
                CLASS,
                20,
                0,
                is_zero_to_tol(off, EFF_ZERO_RAD),
                EFF_ZERO_RAD,
                off,
            )?;
            // look for points of tangency on GenCurve with the extrusion Vec direction
            let none = no_tangent_along(k, gen, extrusion_vec, tol)?;
            rtn &= r.report(CLASS, 21, 0, none, UNDEF, UNDEF)?;
        }
        CurveOrientType::NotPlanar => {
            rtn &= r.report(
                CLASS,
                22,
                0,
                !lin.linear && !planar.is_planar,
                EFF_ZERO,
                UNDEF,
            )?;
            // look for points of tangency on GenCurve with the extrusion Vec direction
            let none = no_tangent_along(k, gen, extrusion_vec, tol)?;
            rtn &= r.report(CLASS, 23, 0, none, UNDEF, UNDEF)?;
        }
        CurveOrientType::Undefined => {}
    }

    Ok(rtn)
}

/// `SmCone::AssertValid`.
fn cone_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    const CLASS: i64 = CONE_TYPE;
    let mut rtn = true;

    rtn &= if r.bases() {
        surf_of_revolution_assert_valid(k, s, r)?
    } else {
        true
    };

    let cone = cone_of(s)?;
    let top_radius = cone.top_radius();
    let bot_radius = cone.bot_radius();
    rtn &= r.report(
        CLASS,
        0,
        0,
        top_radius >= -EFF_ZERO && bot_radius >= -EFF_ZERO,
        EFF_ZERO,
        min2(top_radius, bot_radius),
    )?;

    let pos = cone.rev().position();
    let cone_orig = pos.origin();
    let cone_z = pos.z_axis();

    let same = generator_has_same_parameterization(k, s)?;
    rtn &= r.report(CLASS, 1, 0, same, EFF_ZERO, UNDEF)?;

    let cone = cone_of(s)?;
    let line = match cone.rev().gen_curve().map(Curve::kind) {
        Some(CurveKind::Line(l)) => Some(l),
        _ => None,
    };
    rtn &= r.report(CLASS, 4, 0, line.is_some(), UNDEF, UNDEF)?;
    if let Some(line) = line {
        let line_scale = line.line_scale();
        let cone_scale = 1.0 / (cone.semi_angle_deg() * (PI / 180.0)).cos();
        let tol = (1.0 + line_scale) * EFF_ZERO;
        rtn &= r.report(
            CLASS,
            5,
            0,
            are_same_to_tol(line_scale, cone_scale, tol),
            tol,
            line_scale - cone_scale,
        )?;
    }

    let anal_ivl_v = cone.rev().step_uv_domain().v_interval();
    let apex = cone.apex(k)?;
    rtn &= r.report(
        CLASS,
        6,
        0,
        apex.is_cylinder
            || !anal_ivl_v.contains_value(apex.param, EFF_ZERO)
            || anal_ivl_v.is_value_on_boundary(apex.param, EFF_ZERO).0,
        EFF_ZERO,
        UNDEF,
    )?;

    let nurb_uv = s.natural_uv_domain_with_nurb(k)?.evaluate(0.45678, 0.45678);
    let flags = EvaluateFlags {
        u_side: Side::Left,
        v_side: Side::Left,
        only_upper_half: true,
        non_zero_tangents: true,
    };
    let d = s.evaluate(k, nurb_uv, 1, 1, flags)?;
    let at = |i: usize, j: usize| -> Result<Vec3> {
        d.first()
            .and_then(|_| d.get(i))
            .and_then(|row| row.get(j))
            .copied()
            .ok_or_else(|| invalid("missing derivative"))
    };
    let d0 = at(0, 0)?;
    let sweep_dir = if cone_of(s)?.rev().swap_uv() {
        at(0, 1)?
    } else {
        at(1, 0)?
    };
    let on_axis = cone_orig + (d0 - cone_orig).dot(cone_z) * cone_z;
    let sweep_z = (d0 - on_axis).cross(sweep_dir);
    rtn &= r.report(
        CLASS,
        3,
        0,
        sweep_z.dot(cone_z) > 0.0,
        0.0,
        sweep_z.dot(cone_z),
    )?;

    Ok(rtn)
}

/// `SmCylinder::AssertValid`.
fn cylinder_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    const CLASS: i64 = CYLINDER_TYPE;
    let mut rtn = true;

    rtn &= if r.bases() {
        cone_assert_valid(k, s, r)?
    } else {
        true
    };

    let cone = cone_of(s)?;
    let semi = cone.semi_angle_deg();
    rtn &= r.report(CLASS, 0, 0, is_zero(semi), EFF_ZERO, semi)?;

    let ivl_u = cone.rev().step_uv_domain().u_interval();
    rtn &= r.report(
        CLASS,
        1,
        0,
        ivl_u.length() <= 360.0 + EFF_ZERO_DEG,
        EFF_ZERO_DEG,
        ivl_u.length() - 360.0,
    )?;
    rtn &= r.report(
        CLASS,
        2,
        0,
        ivl_u.max() <= 360.0 + EFF_ZERO_DEG,
        EFF_ZERO_DEG,
        ivl_u.max() - 360.0,
    )?;
    rtn &= r.report(
        CLASS,
        2,
        0,
        ivl_u.min() >= -360.0 - EFF_ZERO_DEG,
        EFF_ZERO_DEG,
        ivl_u.min() + 360.0,
    )?;

    Ok(rtn)
}

/// `SmSphere::AssertValid`.
fn sphere_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    const CLASS: i64 = SPHERE_TYPE;
    let mut rtn = true;

    rtn &= if r.bases() {
        surf_of_revolution_assert_valid(k, s, r)?
    } else {
        true
    };

    /*  0 */
    // must have a positive radius
    let radius = sphere_of(s)?.radius();
    rtn &= r.report(CLASS, 0, 0, radius > 0.0, 0.0, radius)?;

    /*  1 */
    // GenCurve AnalInterval must equal Surface->AnalUVDomain->GetVInterval
    let same = generator_has_same_parameterization(k, s)?;
    rtn &= r.report(CLASS, 1, 0, same, EFF_ZERO, UNDEF)?;

    /*  2 */
    // Analytic U Direction Domain too large: spheres can't overlap themselves
    let anal = sphere_of(s)?.base().step_uv_domain();
    let ivl_u = anal.u_interval();
    // (bIvlTooBig == FALSE)
    let fits = ivl_u.length() <= 360.0 + EFF_ZERO_DEG;
    rtn &= r.report(CLASS, 2, 0, fits, EFF_ZERO_DEG, ivl_u.length() - 360.0)?;

    /*  3 */
    // Analytic V Direction Domain too large: limited to IvlLength <= 180 degrees
    let ivl_v = anal.v_interval();
    // (bIvlTooBig == FALSE)
    let fits = ivl_v.length() <= 180.0 + EFF_ZERO_DEG;
    rtn &= r.report(CLASS, 3, 0, fits, EFF_ZERO_DEG, ivl_v.length() - 180.0)?;

    /*  4 */
    // Analytic V Direction Domain places v=-90 pole within domain interior
    let c0 = ivl_v.contains_periodic_value(-90.0, 360.0, EFF_ZERO_DEG);
    rtn &= r.report(
        CLASS,
        4,
        0,
        !c0.contained
            || is_zero_to_tol(c0.dist_to_min, EFF_ZERO_DEG)
            || is_zero_to_tol(c0.dist_to_max, EFF_ZERO_DEG),
        EFF_ZERO_DEG,
        max2(c0.dist_to_min, c0.dist_to_max),
    )?;

    /*  5 */
    // Analytic V Direction Domain places v=+90 pole within domain interior
    let c1 = ivl_v.contains_periodic_value(90.0, 360.0, EFF_ZERO_DEG);
    rtn &= r.report(
        CLASS,
        5,
        0,
        !c1.contained
            || is_zero_to_tol(c1.dist_to_min, EFF_ZERO_DEG)
            || is_zero_to_tol(c1.dist_to_max, EFF_ZERO_DEG),
        EFF_ZERO_DEG,
        max2(c1.dist_to_min, c1.dist_to_max),
    )?;

    Ok(rtn)
}

/// `SmTorus::AssertValid` (pinned upstream: test 4 reads the U interval, and the U
/// and V tests report as 2 and 3; with `Kernel::fixes.torus_check_reads_v_interval`,
/// WR-538, they read U and V and report as 3 and 4. A spindle torus's self-intersection
/// test clears the result with no report and tests the U interval against an `acos`
/// range; with `Kernel::fixes.torus_check_spindle_apex`, WR-539, it reports test 5 for an
/// apex inside the V interval).
fn torus_assert_valid(k: &Kernel, s: &Surface, r: &mut Reporter<'_>) -> Result<bool> {
    const CLASS: i64 = TORUS_TYPE;
    let mut rtn = true;

    rtn &= if r.bases() {
        surf_of_revolution_assert_valid(k, s, r)?
    } else {
        true
    };

    let t = torus_of(s)?;
    let major = t.major_radius();
    let minor = t.minor_radius();
    rtn &= r.report(CLASS, 0, 0, major > 0.0, 0.0, major)?;
    rtn &= r.report(CLASS, 1, 0, minor > 0.0, 0.0, minor)?;

    let anal = t.base().step_uv_domain();
    if major < minor {
        if k.fixes.torus_check_spindle_apex {
            /*  5 */
            // Not upstream (WR-539, the fork's source fork@daba678): a spindle
            // torus's minor circles cross the axis where R + r cos(V) = 0, at the apexes
            // V = +-(90 + asin(R/r)) degrees; like a sphere's poles (its tests 4 and 5),
            // an apex may bound the V domain but not lie inside it.
            let apex_deg = 90.0 + sm_asin(major / minor) * 180.0 / PI;
            let apex_ivl_v = anal.v_interval();
            for apex in [apex_deg, -apex_deg] {
                let c = apex_ivl_v.contains_periodic_value(apex, 360.0, EFF_ZERO_DEG);
                // (test 5 is the fork's: labelled from the extended table, so the
                // class's `SM_GetAssertLabelList` table stays upstream's)
                let class_string = r.class_string;
                rtn &= r.report_from(
                    TORUS_WITH_SPINDLE_APEX,
                    CLASS,
                    CLASS,
                    class_string,
                    5,
                    0,
                    !c.contained
                        || is_zero_to_tol(c.dist_to_min, EFF_ZERO_DEG)
                        || is_zero_to_tol(c.dist_to_max, EFF_ZERO_DEG),
                    EFF_ZERO_DEG,
                    min2(c.dist_to_min, c.dist_to_max),
                )?;
            }
        } else {
            let angle_limit_deg = 90.0 + sm_acos(major / minor) * 180.0 / PI;
            let self_intersect = Extent1d::from_min_max(angle_limit_deg, 360.0 - angle_limit_deg);
            if !self_intersect.are_disjoint_periodic(&anal.u_interval(), 360.0, -1.0) {
                rtn &= false;
            }
        }
    }

    let same = generator_has_same_parameterization(k, s)?;
    rtn &= r.report(CLASS, 2, 0, same, EFF_ZERO, UNDEF)?;

    // Not upstream (WR-538, the fork's source fork@0cd9d08, #482): the U and V
    // tests report as 3 and 4 (their labels), and the V test reads the V interval
    let fixed_ids = k.fixes.torus_check_reads_v_interval;
    let ivl_u = anal.u_interval();
    rtn &= r.report(
        CLASS,
        if fixed_ids { 3 } else { 2 },
        0,
        ivl_u.length() <= 360.0 + EFF_ZERO_DEG,
        EFF_ZERO_DEG,
        ivl_u.length() - 360.0,
    )?;

    // (upstream reads the U interval again)
    let ivl_v = if fixed_ids {
        anal.v_interval()
    } else {
        anal.u_interval()
    };
    rtn &= r.report(
        CLASS,
        if fixed_ids { 4 } else { 3 },
        0,
        ivl_v.length() <= 360.0 + EFF_ZERO_DEG,
        EFF_ZERO_DEG,
        ivl_v.length() - 360.0,
    )?;

    Ok(rtn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::nurbs::srf_basic::Surface as NlSurface;

    /// A B-spline surface of degrees `p`, `q` over the knot vectors `ku`, `kv` with
    /// the (non-rational) control points `pts` (row `i` = u).
    fn bspline(p: usize, q: usize, ku: &[f64], kv: &[f64], pts: &[[f64; 3]]) -> Surface {
        let nu = ku.len() - p - 1;
        let nv = kv.len() - q - 1;
        let net = (0..nu)
            .map(|i| {
                (0..nv)
                    .map(|j| {
                        let c = pts[i * nv + j];
                        CPoint::new(c[0], c[1], c[2], NOW)
                    })
                    .collect()
            })
            .collect();
        let nurb = NlSurface::new(net, p, q, ku.to_vec(), kv.to_vec()).unwrap();
        Surface::from(BSplineSurface::from_nurb(nurb, false))
    }

    use crate::curve::curve::Curve;
    use crate::geom::extent2d::Extent2d;
    use crate::geom::vector2d::Vec2;
    use crate::nurbs::crv_basic::Curve as NlCurve;
    use crate::surface::sphere::SphereOptions;
    use crate::surface::torus::TorusOptions;

    const O: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    const Y: Vec3 = Vec3::new(0.0, 1.0, 0.0);

    /// Runs the check at `level` and returns its result and the failed reports as
    /// `(reporting type, test number, name, message)`.
    fn failures(s: &Surface, level: u8) -> (bool, Vec<(i64, u64, &'static str, String)>) {
        let k = Kernel::default();
        let mut list = AssertArray::new();
        let ok = s
            .assert_valid(&k, Some(&mut list), level, ObjectKey(1))
            .unwrap();
        let failed = list
            .reports()
            .iter()
            .filter(|r| !r.ok)
            .map(|r| {
                let msg = r.message.as_deref().unwrap_or_default().to_string();
                (r.reporting_type, r.test_index, r.name, msg)
            })
            .collect();
        (ok, failed)
    }

    fn assert_clean(s: &Surface) {
        for level in [0, 1, 2, LEVEL_GIVEN] {
            assert_eq!(failures(s, level), (true, vec![]), "level {level}");
        }
    }

    fn expect(s: &Surface, level: u8, want: &[(i64, u64, &str, &str)]) {
        let (ok, got) = failures(s, level);
        assert!(!ok);
        let got: Vec<_> = got
            .iter()
            .map(|(t, i, n, m)| (*t, *i, *n, m.as_str()))
            .collect();
        assert_eq!(got, want);
    }

    /// The same surface with its NURB form built (`MakeNurb`), so the `SmSurface` and
    /// `SmBSplineSurface` checks run too (a plane builds a temporary one for them anyway).
    fn with_nurb(s: Surface) -> Surface {
        if !matches!(s.kind(), SurfaceKind::Plane(_)) {
            s.kind_with_nurb(&Kernel::default()).unwrap();
        }
        s
    }

    fn plane(su: f64) -> Surface {
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        Surface::from(Plane::new(O, X, Y, Vec2::new(su, 1.0), &d))
    }

    fn cylinder() -> Surface {
        Surface::from(Cone::new_cylinder_bounded(
            O, X, Y, 2.0, 0.0, 360.0, 3.0, false, false, false,
        ))
    }

    fn cone(bot: f64) -> Surface {
        Surface::from(Cone::new_bounded(
            O, X, Y, bot, 1.0, 0.0, 360.0, 3.0, false, false, false,
        ))
    }

    fn sphere() -> Surface {
        let d = Extent2d::from_bounds(0.0, -90.0, 360.0, 90.0);
        let opts = SphereOptions {
            swap_uv: false,
            inside_out: false,
            circle_nurb: None,
        };
        Surface::from(Sphere::new(&Kernel::default(), O, X, Y, d, 2.0, opts).unwrap())
    }

    fn torus() -> Surface {
        let d = Extent2d::from_bounds(0.0, 0.0, 360.0, 360.0);
        let opts = TorusOptions {
            swap_uv: false,
            inside_out: false,
            circle_nurb: None,
        };
        Surface::from(Torus::new(&Kernel::default(), O, X, Y, d, 5.0, 1.0, opts).unwrap())
    }

    /// A surface of revolution about z of the line segment `a`-`b`.
    fn revolution(a: [f64; 3], b: [f64; 3]) -> Surface {
        let k = Kernel::default();
        let pts = vec![
            CPoint::new(a[0], a[1], a[2], NOW),
            CPoint::new(b[0], b[1], b[2], NOW),
        ];
        let line = NlCurve::new(pts, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        let gen = Curve::from(BSplineCurve::from_nurb(line, 3, false));
        let d = Extent2d::from_bounds(0.0, 0.0, 360.0, 1.0);
        Surface::from(SurfOfRevolution::new(&k, gen, O, X, Y, d, false).unwrap())
    }

    /// A bicubic patch over a 4 x 4 wavy grid.
    fn bicubic_points() -> Vec<[f64; 3]> {
        let mut pts = Vec::new();
        for i in 0..4 {
            for j in 0..4 {
                pts.push([f64::from(i), f64::from(j), 0.1 * f64::from((i * 3 + j) % 4)]);
            }
        }
        pts
    }

    const BEZ3: [f64; 8] = [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];

    #[test]
    fn valid_surfaces_pass_at_every_level() {
        assert_clean(&bspline(3, 3, &BEZ3, &BEZ3, &bicubic_points()));
        for s in [
            plane(1.0),
            cylinder(),
            cone(2.0),
            sphere(),
            torus(),
            revolution([2.0, 0.0, 0.0], [3.0, 0.0, 2.0]),
        ] {
            assert_clean(&s);
            assert_clean(&with_nurb(s));
        }
    }

    #[test]
    fn b_spline_knot_multiplicity_above_degree() {
        // degree 1 in u with the interior knot 0.5 twice
        let mut pts = Vec::new();
        for i in 0..4 {
            for j in 0..2 {
                pts.push([f64::from(i), f64::from(j), 0.0]);
            }
        }
        let s = bspline(
            1,
            1,
            &[0.0, 0.0, 0.5, 0.5, 1.0, 1.0],
            &[0.0, 0.0, 1.0, 1.0],
            &pts,
        );
        expect(
            &s,
            2,
            &[(
                B_SPLINE_SURFACE_TYPE,
                1,
                "Bad Knot Multiplicity",
                "SmBSplineSurface Knot multiplicity greater than surface degree",
            )],
        );
    }

    #[test]
    fn b_spline_internal_pole() {
        // the middle row of a degree-1 (u) net collapsed to one point at a C0 knot
        let mut pts = Vec::new();
        for i in 0..3 {
            for j in 0..3 {
                pts.push([f64::from(i), f64::from(j), 0.0]);
            }
        }
        for p in pts.iter_mut().skip(3).take(3) {
            *p = [1.0, 1.0, 1.0];
        }
        let s = bspline(
            1,
            2,
            &[0.0, 0.0, 0.5, 1.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            &pts,
        );
        expect(
            &s,
            2,
            &[
                (
                    SURFACE_TYPE,
                    4,
                    "Bad Surf Discontinuity",
                    "Surface sampled IsoLine contains a G1 or C0 discontinuity across a Knot Value",
                ),
                (
                    B_SPLINE_SURFACE_TYPE,
                    2,
                    "Bad Internal Pole",
                    "SmBSplineSurface has an Internal pole caused by combinations of coincident CPts",
                ),
            ],
        );
    }

    /// The failed reports' (class, test) of `s` at level 0 on kernel `k`.
    fn failed_tests(k: &Kernel, s: &Surface) -> (bool, Vec<(i64, u64)>) {
        let mut list = AssertArray::new();
        let ok = s.assert_valid(k, Some(&mut list), 0, ObjectKey(1)).unwrap();
        let failed = list
            .reports()
            .iter()
            .filter(|r| !r.ok)
            .map(|r| (r.reporting_type, r.test_index))
            .collect();
        (ok, failed)
    }

    fn torus_over(v0: f64, v1: f64, major: f64, minor: f64) -> Surface {
        let d = Extent2d::from_bounds(0.0, v0, 360.0, v1);
        let opts = TorusOptions {
            swap_uv: false,
            inside_out: false,
            circle_nurb: None,
        };
        Surface::from(Torus::new(&Kernel::default(), O, X, Y, d, major, minor, opts).unwrap())
    }

    /// WR-538 (the fork's source fork@0cd9d08, #482): a torus whose V interval
    /// runs past a turn. Upstream tests the U interval twice and never the V one; with
    /// `torus_check_reads_v_interval` the V test (4) fails.
    #[test]
    fn wr_538_torus_check_reads_its_v_interval() {
        let s = torus_over(0.0, 400.0, 5.0, 1.0);
        let mut k = Kernel::default();
        let (_, upstream) = failed_tests(&k, &s);
        assert!(!upstream.contains(&(TORUS_TYPE, 4)), "{upstream:?}");
        k.fixes.torus_check_reads_v_interval = true;
        let (ok, fixed) = failed_tests(&k, &s);
        assert!(!ok);
        assert!(fixed.contains(&(TORUS_TYPE, 4)), "{fixed:?}");
        assert!(!fixed.contains(&(TORUS_TYPE, 3)), "{fixed:?}");
    }

    /// WR-539 (the fork's source fork@daba678): the spindle torus R 1, r 2 has its
    /// apexes at V = +-120. Upstream fails every spindle torus with a whole turn of U and
    /// reports nothing; with `torus_check_spindle_apex` V [0, 360] reports test 5 and the
    /// outer sheet V [-120, 120] passes.
    #[test]
    fn wr_539_spindle_torus_reports_an_apex_inside() {
        let whole = torus_over(0.0, 360.0, 1.0, 2.0);
        let outer = torus_over(-120.0, 120.0, 1.0, 2.0);
        let mut k = Kernel::default();
        for s in [&whole, &outer] {
            let (ok, upstream) = failed_tests(&k, s);
            assert!(!ok);
            assert!(!upstream.contains(&(TORUS_TYPE, 5)), "{upstream:?}");
        }
        k.fixes.torus_check_spindle_apex = true;
        let (ok, fixed) = failed_tests(&k, &whole);
        assert!(!ok);
        assert!(fixed.contains(&(TORUS_TYPE, 5)), "{fixed:?}");
        let (_, outer_fixed) = failed_tests(&k, &outer);
        assert!(!outer_fixed.contains(&(TORUS_TYPE, 5)), "{outer_fixed:?}");
        // the class's table stays upstream's five checks (SM_GetAssertLabelList)
        assert_eq!(assert_label_list(TORUS_TYPE, 0).len(), 5);
    }

    /// source issue #1515 (WR-505): a horn torus (major radius = minor radius)
    /// touches its axis, where the corner rows of its NURB coincide. Upstream's
    /// `HasInternalPole` tests one row one step too far at each repeated knot and reports
    /// a pole there; with `internal_pole_row_through_knot` it finds none.
    #[test]
    fn issue_1515_horn_torus_has_no_internal_pole() {
        use crate::geom::axis2_placement::Axis2Placement;

        // the tube runs from the inner pole (-180 degrees) once round, so the pole is the
        // NURB's boundary and its corner rows are the ones on the axis
        let d = Extent2d::from_bounds(0.0, -180.0, 360.0, 180.0);
        let horn = |k: &Kernel| {
            let opts = TorusOptions {
                swap_uv: false,
                inside_out: false,
                circle_nurb: None,
            };
            let t =
                Torus::create_canonical(k, &Axis2Placement::default(), 5.0, 5.0, opts, Some(&d))
                    .unwrap();
            let s = Surface::from(t);
            let part = make_nurb(k, &s).unwrap().unwrap();
            has_internal_pole(k, &s, &part, ZONE_TOL_3D / 10.0).unwrap()
        };

        let mut k = Kernel::default();
        assert!(horn(&k), "upstream reports a pole on the horn torus");
        k.fixes.internal_pole_row_through_knot = true;
        assert!(!horn(&k), "no internal pole on the horn torus");
    }

    /// A real pole at a double knot (row 2 of a 5 x 3 net, collapsed to one point at the
    /// knot 0.5) is found with the opt-in. Upstream tests the corner row one step further
    /// (row 3) at that knot, so it misses the pole (WR-505, #1515).
    #[test]
    fn issue_1515_real_pole_at_a_double_knot_is_found() {
        let mut pts = Vec::new();
        for i in 0..5 {
            for j in 0..3 {
                pts.push([f64::from(i), f64::from(j), 0.0]);
            }
        }
        for p in pts.iter_mut().skip(6).take(3) {
            *p = [2.0, 1.0, 1.0];
        }
        let s = bspline(
            2,
            2,
            &[0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            &pts,
        );
        let pole = |flag: bool| {
            let mut k = Kernel::default();
            k.fixes.internal_pole_row_through_knot = flag;
            let part = make_nurb(&k, &s).unwrap().unwrap();
            has_internal_pole(&k, &s, &part, ZONE_TOL_3D / 10.0).unwrap()
        };
        assert!(!pole(false), "upstream misses the pole at the double knot");
        assert!(pole(true), "the opt-in finds the pole at the double knot");
    }

    #[test]
    fn b_spline_net_doubles_back() {
        let mut pts = bicubic_points();
        pts[2 * 4 + 1] = [0.2, 1.0, 0.0];
        pts[2 * 4 + 2] = [0.2, 2.0, 0.0];
        let s = bspline(3, 3, &BEZ3, &BEZ3, &pts);
        expect(
            &s,
            2,
            &[(
                B_SPLINE_SURFACE_TYPE,
                5,
                "Bad NonMonotonic CPts",
                "SmBSplineSurface Control Net doubles back on itself probably making a self intersecting surface",
            )],
        );
    }

    #[test]
    fn plane_negative_scale() {
        let want = [(PLANE_TYPE, 3, "Scale", "m_vUVScale should be positive in X")];
        expect(&plane(-1.0), 2, &want);
        expect(&with_nurb(plane(-1.0)), 2, &want);
    }

    #[test]
    fn cone_negative_radius_puts_apex_inside() {
        let radius = (
            CONE_TYPE,
            0,
            "Bad Cone Radius",
            "TopRadius or BotRadius is negative. Both must be >= 0.0 to ensure the apex is not within the domain)",
        );
        let pole = (
            CONE_TYPE,
            6,
            "Bad Cone Pole",
            "Cone apex (where radius == 0.0) is within the Cone's AnalIvlV placing a pole on the surface interior.",
        );
        expect(&cone(-1.0), 2, &[radius, pole]);
        // SM_LEVEL_GIVEN skips the base classes' checks
        expect(&cone(-1.0), LEVEL_GIVEN, &[radius, pole]);
        // with the NURB form, the net through the apex also folds back
        let fold = (
            B_SPLINE_SURFACE_TYPE,
            5,
            "Bad NonMonotonic CPts",
            "SmBSplineSurface Control Net doubles back on itself probably making a self intersecting surface",
        );
        expect(&with_nurb(cone(-1.0)), 2, &[fold, radius, pole]);
    }

    #[test]
    fn revolution_of_a_tiny_generator() {
        let s = revolution([2.0, 0.0, 0.0], [2.0 + 1e-12, 0.0, 0.0]);
        expect(
            &s,
            2,
            &[(
                SURF_OF_REVOLUTION_TYPE,
                15,
                "Size",
                "Bad GenCurve length degenerates SurfOfRevolution to an approximate circular curve",
            )],
        );
    }

    #[test]
    fn sphere_domain_past_the_pole() {
        let d = Extent2d::from_bounds(0.0, -100.0, 360.0, 80.0);
        let opts = SphereOptions {
            swap_uv: false,
            inside_out: false,
            circle_nurb: None,
        };
        let s = Surface::from(Sphere::new(&Kernel::default(), O, X, Y, d, 2.0, opts).unwrap());
        expect(
            &s,
            2,
            &[(
                SPHERE_TYPE,
                4,
                "Included Singularity",
                "Analytic V Direction Domain places v=-90 pole within domain interior",
            )],
        );
    }

    /// A surface of extrusion of the quadratic arc through (0, 0, 0), (1, 1, 0),
    /// (2, 0, 0) along `vec`, over the generator's interval by `v`, with the origin
    /// `origin` (the generator point at `0.3` when `None`).
    fn extrusion(vec: Vec3, v: (f64, f64), origin: Option<Point3>) -> Surface {
        let k = Kernel::default();
        let pts = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 1.0, 0.0, NOW),
            CPoint::new(2.0, 0.0, 0.0, NOW),
        ];
        let arc = NlCurve::new(pts, 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).unwrap();
        let gen = Curve::from(BSplineCurve::from_nurb(arc, 3, false));
        let origin = origin.unwrap_or_else(|| gen.evaluate_point(&k, 0.3).unwrap());
        let d = Extent2d::from_bounds(0.0, v.0, 1.0, v.1);
        Surface::from(SurfOfExtrusion::new(&k, gen, origin, vec, d, false).unwrap())
    }

    #[test]
    fn valid_extrusion_passes_at_every_level() {
        let s = extrusion(Vec3::new(0.0, 0.0, 1.0), (0.0, 2.0), None);
        assert_clean(&s);
        assert_clean(&with_nurb(s));
    }

    #[test]
    fn extrusion_without_extent() {
        let s = extrusion(Vec3::new(0.0, 0.0, 1.0), (1.0, 1.0), None);
        expect(
            &s,
            0,
            &[(
                SURF_OF_EXTRUSION_TYPE,
                3,
                "Analytic Y",
                "m_vAnalUVDomain YLength must be non-zero",
            )],
        );
    }

    #[test]
    fn extrusion_origin_off_the_generator() {
        let s = extrusion(
            Vec3::new(0.0, 0.0, 1.0),
            (0.0, 2.0),
            Some(Point3::new(5.0, 5.0, 5.0)),
        );
        expect(
            &s,
            0,
            &[(
                SURF_OF_EXTRUSION_TYPE,
                8,
                "Origin",
                "m_vOrigin is a point on m_pGenCurve",
            )],
        );
    }
}

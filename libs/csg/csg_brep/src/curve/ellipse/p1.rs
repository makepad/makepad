//! `SmEllipse` and `SmCircle`, part 1: the circle class, construction and copying,
//! the NURB build and its edits (2D conversion, STEP domain changes, joins,
//! reversal, trimming), the NURB to STEP parameter conversion, the STEP global
//! point solve, validity checks and database reading.
//!
//! Upstream: `source/SMLib/src/SmEllipse.cpp`, `source/SMLib/src/SmCircle.cpp`,
//! `source/SMLib/inc/SmCircle.h`.
//!
//! | Rust                                               | Upstream                                          |
//! |----------------------------------------------------|---------------------------------------------------|
//! | [`Ellipse::new`] (in `p2`)                          | `SmEllipse(Center, XAxis, YAxis, AnalDomain, ..)` |
//! | [`Ellipse::copy`], `Clone`                         | `SmEllipse(const SmEllipse&)`, `SmEllipse::Copy`  |
//! | [`Ellipse::create_canonical`]                      | `SmEllipse::CreateCanonical`                      |
//! | [`Ellipse::new_circle`]                            | `SmCircle(Center, XAxis, YAxis, AnalDomain, dRadius, ..)` |
//! | [`Ellipse::circle_from_3_points`]                  | `SmCircle(StartPoint, MidPoint, EndPoint, ..)`    |
//! | [`Ellipse::empty_circle`]                          | `SmCircle()`                                      |
//! | [`Ellipse::copy`] (circle)                         | `SmCircle(const SmCircle&)`, `SmCircle::Copy`     |
//! | [`Ellipse::create_canonical_circle`]               | `SmCircle::CreateCanonical`                       |
//! | `Drop` (implicit)                                  | `~SmCircle`                                       |
//! | [`Ellipse::approx_eq`] (in `p2`)                   | `SmCircle::operator==`                            |
//! | [`Ellipse::radius`], [`Ellipse::set_radius`]       | `SmCircle::GetRadius`, `SmCircle::SetRadius`      |
//! | [`Ellipse::is_circle_class`], [`Ellipse::kind_of`] | `SmCircle_TYPE` objects, `SmCircle::IsKindOf`     |
//! | [`Ellipse::type_code`], [`Ellipse::type_string`], [`Ellipse::class_string`] (in `p2`), [`CIRCLE_CLASS_TYPE`] | `SmCircle::GetType`, `GetTypeString`, `GetClassString`, `GetClassType` |
//! | [`Ellipse::is_arc`]                                | `SmCircle::IsArc`                                 |
//! | [`Ellipse::is_nurb_curve_circle`]                  | `SmCircle::IsNurbCurveCircle`                     |
//! | [`Ellipse::create_simple_offset`]                  | `SmCircle::CreateSimpleOffset`                    |
//! | [`Ellipse::assert_valid`]                          | `SmEllipse::AssertValid`, `SmCircle::AssertValid` |
//! | [`Ellipse::read_from_db`]                          | `SmEllipse::ReadFromDB`, `SmCircle::ReadFromDB`   |
//! | [`Ellipse::write_to_db`]                           | `SmEllipse::WriteToDB`, `SmCircle::WriteToDB`     |
//! | [`Ellipse::convert_to_2d`]                         | `SmEllipse::ConvertTo2D`                          |
//! | [`Ellipse::convert_t_from_nurbs_to_step`]          | `SmEllipse::ConvertTFromNURBSToSTEP`              |
//! | [`Ellipse::adjust_step_interval`]                  | `SmEllipse::AdjustSTEPInterval`                   |
//! | [`Ellipse::global_point_solve_step`]               | `SmEllipse::GlobalPointSolveSTEP`                 |
//! | [`Ellipse::join_with`]                             | `SmEllipse::JoinWith`                             |
//! | [`Ellipse::make_nurb`], [`Ellipse::make_nurb_owned`], [`Ellipse::make_nurb_owned_with_edge_tol`] | `SmEllipse::MakeNurb` |
//! | [`Ellipse::notify`]                                | `SmEllipse::Notify`                               |
//! | [`Ellipse::reverse_parameterization`]              | `SmEllipse::ReverseParameterization`              |
//! | [`Ellipse::trim`]                                  | `SmEllipse::Trim`                                 |
//! | [`Ellipse::edit_parameterization`]                 | `SmBSplineCurve::EditParameterization` on an ellipse (virtual `MakeNurb`, `Notify`) |
//! | `Clone` of [`PolarConversion`]                     | `SmPolarConversion(crContext, crSource)`          |
//! | [`PolarConversion::convert_to_polar_parameter`]    | `SmPolarConversion::ConvertToPolarParameter`      |
//! | [`PolarConversion::is_current`]                    | `SmPolarConversion::IsCurrent`                    |
//! | [`PolarConversion::assert_valid`]                  | `SmPolarConversion::AssertValid`                  |
//! | [`PolarConversion::read_from_db`], [`PolarConversion::read_from_db_with_curve`] | `SmPolarConversion::ReadFromDB` (ellipse back pointer; null back pointer, from `SmSurfOfRevolution::ReadFromDB`) |
//! | [`PolarConversion::write_to_db`]                   | `SmPolarConversion::WriteToDB`                    |
//!
//! Deviations:
//! - **Circle class.** `SmCircle` derives from `SmEllipse` without data of its own;
//!   it is an [`Ellipse`] whose `circle` flag is set ([`Ellipse::is_circle_class`]),
//!   which picks the circle's type, its `IsKindOf`, `IsArc`, `CreateSimpleOffset` and
//!   `AssertValid` overrides and its B-spline form (`SM_CF_CIRCULAR_ARC`). The
//!   geometric test [`Ellipse::is_circle`] (`IsCircle`) is unrelated.
//! - **Owner.** The owner (`m_pOwner`) lives in the [`crate::curve::curve::Curve`]
//!   wrapper; the methods that make the NURB take it as an argument (`None` for a
//!   free ellipse). `MakeNurb` of an ellipse owned by an edge (or by an edge use or
//!   a curve, whose owner chain may reach one: `GetEdge`) needs the edge's zone
//!   tolerance (`SmTol::GetZoneTol3d(const SmObject*)`, unit `topology.brep_p1`);
//!   one owned by a surface of extrusion or revolution needs its generator range
//!   (units `surface.surf_of_extrusion_p1`, `surface.surf_of_revolution_p1`). These
//!   return [`SolidError::Unported`], except where the caller resolved the edge's
//!   tolerance ([`Ellipse::make_nurb_owned_with_edge_tol`]: the curve cache of an edge
//!   curve, and a curve carrying its edge's tolerance,
//!   [`crate::curve::curve::Curve::owner_zone_tol_3d`]).
//!   Trimming accepts a caller-resolved owning edge tolerance through
//!   `trim_with_zone_tol`; public ownerless trimming uses the system tolerance.
//! - **Notifications.** `Notify(SM_NO_POST_EDIT)` rebuilds the polar converter
//!   (status ignored); the other operations only drop caches the ellipse does not
//!   own here, and `SM_NO_UNKNOWN` only logs. `SmCurve::Notify`'s cache bookkeeping
//!   belongs to the [`crate::curve::curve::Curve`] wrapper.
//! - **Polar converter.** As in `p2`, the converter's curve is always the owning
//!   ellipse (borrowed). `SmPolarConversion::ReadFromDB` of a converter that owns a
//!   copy of its curve (`bHasCurve`, never written for an ellipse's own converter)
//!   returns `InvalidInput`; [`PolarConversion::read_from_db_with_curve`] (added by
//!   `surface.surf_of_revolution_p3`) reads that copy for the surface of revolution's
//!   converter, which owns its sweep circle.
//! - **Undefined behaviour.** Where upstream would read an uninitialised value (an
//!   unreadable ASCII count or flag, a failed `ConvertTFromSTEPToNURBS` in
//!   `AssertValid`, a missing polar curve in `ConvertToPolarParameter`'s callers,
//!   `ConvertTo2D`'s quadratic without a root (with one root only the unused
//!   `dThetaYRad` reads the uninitialised second, and the port goes on), the 3-point circle of
//!   collinear points) or index past a table (polar tables shorter than the knot
//!   list), the port returns [`SolidError::InvalidInput`] (or the solver's status).
//! - **Statuses ignored upstream** stay ignored, except [`SolidError::Unported`],
//!   which always propagates. `SmBoolean` values read from a database are taken as
//!   `!= 0`.
//! - **Arguments.** Out-params become return values; `SmContext` arguments, the
//!   arguments upstream marks `NotUsed` (`GlobalPointSolveSTEP`'s interval and
//!   vectors, `IsArc`'s sample count and tolerance, `ReadFromDB`'s type) and debug
//!   flags (`Trim`'s `bSkipDebugCheck`) are dropped. `AssertValid` takes the keys
//!   naming the ellipse and its polar converter in reports and `level_given` for
//!   `SM_LEVEL_GIVEN` (the other levels run every check used here).
//! - **Back-edges.** `SmCurve::IsArc` of a non-circular ellipse (inside
//!   `SmBSplineCurve::CreateSimpleOffset`) is the base class of unit `curve.curve_p5`
//!   ([`crate::curve::curve::p5::is_arc_base`]) on the ellipse's NURB.
//! - **`IsNurbCurveCircle`.** The `SmCircle*&` out-param is the returned `Option`
//!   (`None` for `FALSE`), the defaulted `dToleranceScale` an `Option` (`None`: `1.0`);
//!   the `SmBSplineCurve*` is a [`Curve`] whose kind must derive from `SmBSplineCurve`
//!   (B-spline, ellipse, circle, line, parabola, hyperbola), others are
//!   [`SolidError::InvalidInput`]. The ignored `EvaluatePoint` status propagates.

use core::ops::Div;

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::CIRCLE_TYPE;
use crate::curve::curve::p4::is_closed_base;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::assert_array::labels::{
    self, EDGEUSE_TYPE, EDGE_TYPE, POLAR_CONVERSION_TYPE, SURF_OF_EXTRUSION_TYPE,
    SURF_OF_REVOLUTION_TYPE,
};
use crate::geom::assert_array::{AssertArray, AssertReport, ObjectKey, ReportSpec};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::geom_utility::p1::circle_from_3_points;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::polynomial::solve_quadratic;
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject};
use crate::geom::tol::{are_same, are_same_to_tol, is_zero_to_tol, tol3d};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min3, EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_SQRT, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::p2::ArcData;
use crate::nurbs::b_spline_curve::p3::DerivedOverrides;
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::crv_basic::ConicType;
use crate::nurbs::crv_common::{elliptical_arc_tol, EllipseArc};
use crate::nurbs::index::at;
use crate::nurbs::nurbs_crv::to_euclid;
use crate::surface::surface::Surface;
use crate::topology::database_io::{
    fixed16, DatabaseReader, DatabaseWriter, FileType, InputStream,
};

use super::p2::{Ellipse, EllipseOptions, PolarConversion};

const OP_MAKE_NURB: &str = "curve::ellipse::p1::Ellipse::make_nurb";
const OP_TO_2D: &str = "curve::ellipse::p1::Ellipse::convert_to_2d";
const OP_TO_POLAR: &str = "curve::ellipse::p1::PolarConversion::convert_to_polar_parameter";
const OP_IS_CURRENT: &str = "curve::ellipse::p1::PolarConversion::is_current";
const OP_ASSERT: &str = "curve::ellipse::p1::Ellipse::assert_valid";
const OP_READ_POLAR: &str = "curve::ellipse::p1::PolarConversion::read_from_db";
const OP_CANONICAL: &str = "curve::ellipse::p1::Ellipse::create_canonical";
const OP_CIRCLE_CANONICAL: &str = "curve::ellipse::p1::Ellipse::create_canonical_circle";
const OP_JOIN: &str = "curve::ellipse::p1::Ellipse::join_with";
const OP_OFFSET: &str = "curve::ellipse::p1::Ellipse::create_simple_offset";
const OP_IS_NURB_CURVE_CIRCLE: &str = "curve::ellipse::p1::Ellipse::is_nurb_curve_circle";
/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

/// `SmCircle_TYPE` (`CURV_BASE_TYPE + 21`), the class type of a circle.
pub const CIRCLE_CLASS_TYPE: i64 = CIRCLE_TYPE;

/// The handle of the ellipse in its own global solve: a curve with the given natural
/// interval (the solver's duplicate test reads it). The solutions returned name no
/// object.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SelfHandle(Extent1d);

impl SolutionObject for SelfHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// The `SM_ASSERT_*_REPORT` macros (`SM_LEVEL_0` tests: the predicate always runs):
/// `true` when `passed`; otherwise, with a list, a report for test `test_index` of
/// the label table `(class.0, list_index)` about `key` (whose dynamic class string is
/// `class.1`), unless an equal one is there.
fn report(
    list: &mut Option<&mut AssertArray>,
    key: ObjectKey,
    class: (i64, &'static str),
    list_index: u64,
    test_index: u64,
    passed: bool,
    tol: f64,
    val: f64,
) -> Result<bool> {
    if passed {
        if let Some(l) = list.as_deref_mut() {
            l.set_last_add(false);
        }
        return Ok(true);
    }
    if let Some(l) = list.as_deref_mut() {
        let r = AssertReport::new(ReportSpec {
            reporter: Some(key),
            test_index,
            test_class: class.0,
            owner: Some(key),
            owner_type: class.0,
            owner_type_string: Some(class.1),
            other: None,
            other_type: 0,
            other_type_string: Some("NULL"),
            tol3d: tol3d(tol),
            value: val,
            list_index,
            message: None,
        })?;
        l.add_unique_delete_duplicate(r);
    }
    Ok(false)
}

const POLAR_CLASS: (i64, &str) = (POLAR_CONVERSION_TYPE, "SmPolarConversion");

/// `is >> value` of a double: stores the extracted value unless the stream's
/// sentry fails.
fn extract(s: &mut InputStream, slot: &mut f64) {
    if let Some(v) = s.extract_double() {
        *slot = v;
    }
}

// ---- the polar conversion table -------------------------------------------------------

impl PolarConversion {
    /// The STEP angle in degrees of the NURB parameter `nurbs_param` of `curve` (the
    /// ellipse owning this table) and whether the conversion is exact (always, when it
    /// succeeds) (`ConvertToPolarParameter`). The NURB ends (within `SM_EFF_ZERO`) map
    /// to the STEP domain ends (swapped for an inside-out ellipse); other parameters
    /// are evaluated and inverted (`STEPInversion`). On a closed curve the seam
    /// parameter returns the angle of its own end of the domain (`-/+ 360`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the table has no curve; `InvalidInput` when the
    /// tables are shorter than the closed-curve checks read.
    pub fn convert_to_polar_parameter(
        &self,
        k: &Kernel,
        curve: &Ellipse,
        nurbs_param: f64,
    ) -> Result<(f64, bool)> {
        // Can't do anything if not set.
        if !self.has_curve {
            return Err(SolidError::null_pointer(OP_TO_POLAR));
        }

        // avoid roundoff errors at boundaries
        let anal_ivl = curve.step_interval();
        let nurb_ivl = curve.base.natural_interval();
        let point_type = nurb_ivl.classify_point(nurbs_param, EFF_ZERO);

        // switch on ePointType - map boundary values and invert internal values
        let mut polar_deg = match point_type {
            ExtentPointType::Start => {
                if curve.inside_out {
                    anal_ivl.max()
                } else {
                    anal_ivl.min()
                }
            }
            ExtentPointType::End => {
                if curve.inside_out {
                    anal_ivl.min()
                } else {
                    anal_ivl.max()
                }
            }
            ExtentPointType::Both
            | ExtentPointType::Outside
            | ExtentPointType::Unknown
            | ExtentPointType::Inside => {
                // use a conversion based arcTan2 (EvaluatePoint status ignored)
                let point = ignored(curve.base.evaluate_point(nurbs_param))?.unwrap_or(Vec3::UNDEF);
                curve.step_inversion(k, point, false, 0.0).0
            }
        };

        let exact = true;

        // make sure to return the proper end value for closed curve seam conversions.
        // The seam adjustments only apply at the ends of the knot/angle tables, so test those
        // cheap conditions first and evaluate IsClosed() (three curve evaluations) only there.
        if !self.natural_knots.is_empty() && !self.angles.is_empty() {
            let inside_out = curve.inside_out;
            let knots = &self.natural_knots;
            let angles = &self.angles;
            let last = knots.len().wrapping_sub(1);
            let last_angle = angles.len().wrapping_sub(1);
            let scaled_zero =
                (at(knots, last, OP_TO_POLAR)? - at(knots, 0, OP_TO_POLAR)?) / 360.0 * EFF_ZERO_DEG;
            let same = |a: f64, b: f64, tol: f64| are_same_to_tol(a, b, tol);

            // when values are at opposing ends - move the end angle
            let move_to_min = if inside_out {
                same(nurbs_param, at(knots, last, OP_TO_POLAR)?, scaled_zero)
                    && same(polar_deg, at(angles, 0, OP_TO_POLAR)?, EFF_ZERO_DEG)
            } else {
                same(nurbs_param, at(knots, 0, OP_TO_POLAR)?, scaled_zero)
                    && same(
                        polar_deg,
                        at(angles, last_angle, OP_TO_POLAR)?,
                        EFF_ZERO_DEG,
                    )
            };
            let move_to_max = !move_to_min
                && if inside_out {
                    same(nurbs_param, at(knots, 0, OP_TO_POLAR)?, scaled_zero)
                        && same(
                            polar_deg,
                            at(angles, last_angle, OP_TO_POLAR)?,
                            EFF_ZERO_DEG,
                        )
                } else {
                    same(nurbs_param, at(knots, last, OP_TO_POLAR)?, scaled_zero)
                        && same(polar_deg, at(angles, 0, OP_TO_POLAR)?, EFF_ZERO_DEG)
                };

            if (move_to_min || move_to_max)
                && is_closed_base(k, &curve.base, &curve.base.natural_interval(), 0.0)?
            {
                // increment, rather than PolarParam = min/maxAngle, to preserve tolerances
                if move_to_min {
                    polar_deg -= 360.0;
                } else {
                    polar_deg += 360.0;
                }
            }
        }

        // all done
        Ok((polar_deg, exact))
    }

    /// Whether the tables agree with `curve`, the ellipse owning them (`IsCurrent`):
    /// `false` without curve or set-up tables; else the knot count (report 0) and
    /// values (report 1, `SM_ARE_SAME`) match the curve's natural knots, the stored
    /// angles match the inverted knot points within `SM_EFF_ZERO_DEG` modulo 360
    /// (report 2) and change monotonically (report 3). Reports go to `list` (label
    /// list 1 of `SmPolarConversion`, about `key`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the angle table is shorter than the knot list; those of
    /// [`AssertReport::new`].
    pub fn is_current(
        &self,
        k: &Kernel,
        curve: &Ellipse,
        list: Option<&mut AssertArray>,
        key: ObjectKey,
    ) -> Result<bool> {
        let mut list = list;

        // init return
        let mut rtn = true;

        // These were already checked in the calling function
        if !self.has_curve || !self.possible {
            return Ok(false);
        }

        // get curve knots (status ignored)
        let natural_knots = ignored(curve.base.knots(None))?
            .map(|(k, _)| k)
            .unwrap_or_default();
        let cnt = natural_knots.len();

        // for each knot - check stored knot value
        rtn &= report(
            &mut list,
            key,
            POLAR_CLASS,
            1,
            0,
            natural_knots.len() == self.natural_knots.len(),
            UNDEF_DOUBLE,
            UNDEF_DOUBLE,
        )?;

        let mut ii = 0;
        while ii < cnt && rtn {
            let a = at(&natural_knots, ii, OP_IS_CURRENT)?;
            let b = at(&self.natural_knots, ii, OP_IS_CURRENT)?;
            rtn &= report(
                &mut list,
                key,
                POLAR_CLASS,
                1,
                1,
                are_same(a, b),
                UNDEF_DOUBLE,
                UNDEF_DOUBLE,
            )?;
            ii += 1;
        }

        // build angles for circles and ellipses (the curve is always an ellipse)
        let angle_tol = EFF_ZERO_DEG;
        if rtn {
            // monotonic local
            let mut angle_dir = 0.0;

            // for each knot get a PolarParam angleDeg
            let mut ii = 0;
            while ii < cnt && rtn {
                // Get CCW angle to vec = (thisKnotPoint - Origin) (status ignored)
                let knot = at(&natural_knots, ii, OP_IS_CURRENT)?;
                let point = ignored(curve.base.evaluate_point(knot))?.unwrap_or(Vec3::UNDEF);

                // convert point to polar param (deg)
                let angle_deg = curve.step_inversion(k, point, false, 0.0).0;
                let stored = at(&self.angles, ii, OP_IS_CURRENT)?;

                // computed angles must match angles in storage
                let ok = are_same_to_tol(angle_deg, stored, angle_tol)
                    || are_same_to_tol(angle_deg, stored + 360.0, angle_tol)
                    || are_same_to_tol(angle_deg + 360.0, stored, angle_tol);
                let val = min3(
                    angle_deg - stored,
                    angle_deg - stored + 360.0,
                    angle_deg + 360.0 - stored,
                );
                rtn &= report(&mut list, key, POLAR_CLASS, 1, 2, ok, angle_tol, val)?;

                // angles must increase or decrease monotonically
                if ii == 1 {
                    let d = at(&self.angles, ii, OP_IS_CURRENT)?
                        - at(&self.angles, ii - 1, OP_IS_CURRENT)?;
                    angle_dir = if d > 0.0 { 1.0 } else { -1.0 };
                } else if ii > 1 {
                    let d = at(&self.angles, ii, OP_IS_CURRENT)?
                        - at(&self.angles, ii - 1, OP_IS_CURRENT)?;
                    let this_dir = if d > 0.0 { 1.0 } else { -1.0 };
                    rtn &= report(
                        &mut list,
                        key,
                        POLAR_CLASS,
                        1,
                        3,
                        this_dir == angle_dir,
                        UNDEF_DOUBLE,
                        UNDEF_DOUBLE,
                    )?;
                }
                ii += 1;
            }
        }

        // all done
        Ok(rtn)
    }

    /// `AssertValid`: without curve the tables must not be set up (report 0), else
    /// they must be current ([`Self::is_current`], report 1). Reports go to `list`
    /// (label list 0 of `SmPolarConversion`, about `key`).
    ///
    /// # Errors
    ///
    /// Those of [`Self::is_current`].
    pub fn assert_valid(
        &self,
        k: &Kernel,
        curve: &Ellipse,
        list: Option<&mut AssertArray>,
        key: ObjectKey,
    ) -> Result<bool> {
        let mut list = list;
        let mut rtn = true;

        if self.has_curve {
            let current = self.is_current(k, curve, list.as_deref_mut(), key)?;
            rtn &= report(
                &mut list,
                key,
                POLAR_CLASS,
                0,
                1,
                current,
                UNDEF_DOUBLE,
                UNDEF_DOUBLE,
            )?;
        } else {
            rtn &= report(
                &mut list,
                key,
                POLAR_CLASS,
                0,
                0,
                !self.possible,
                UNDEF_DOUBLE,
                UNDEF_DOUBLE,
            )?;
        }
        Ok(rtn)
    }

    /// Reads a table written by `WriteToDB` (`ReadFromDB`): the set-up flag, the
    /// knot, angle and exactness counts and the has-curve flag (one per line in ASCII
    /// files), then the three arrays (ASCII: a line break after the elements with
    /// index 5, 10, ... and after a partial last line). The table's curve is the
    /// ellipse being read (`cpCurve`, borrowed).
    ///
    /// # Errors
    ///
    /// A failed binary read; `InvalidInput` for unreadable ASCII values (upstream:
    /// uninitialised), counts larger than the stream, or a stored copy of the curve
    /// (`bHasCurve`, see the module deviations).
    pub fn read_from_db(db: &mut DatabaseReader) -> Result<Self> {
        let (pc, has_curve) = Self::read_tables(db)?;

        // containing curve
        if has_curve {
            return Err(SolidError::invalid_input(
                OP_READ_POLAR,
                "a polar converter owning a copy of its curve is not modelled",
            ));
        }
        Ok(Self::finish_back_pointer(db, pc, true))
    }

    /// Reads a table that may own a copy of its curve (`ReadFromDB` with a null
    /// `cpCurve`, as `SmSurfOfRevolution::ReadFromDB` calls it): after the tables, a
    /// stored curve (`bHasCurve`) is read with its type header; without one the
    /// table has no curve (the null back pointer). Returns the table and the curve
    /// it owns.
    ///
    /// # Errors
    ///
    /// Those of [`PolarConversion::read_from_db`]'s table reads;
    /// [`SolidError::InvalidInput`] for a stored curve that is not an ellipse
    /// (upstream casts it). The statuses of the curve's type header and reader are
    /// ignored, as upstream (a failed read leaves the table without curve), except
    /// [`SolidError::Unported`].
    pub fn read_from_db_with_curve(
        k: &Kernel,
        db: &mut DatabaseReader,
        version: u64,
    ) -> Result<(Self, Option<Ellipse>)> {
        let (polar, curve) = Self::read_from_db_with_curve_object(k, db, version)?;
        let ellipse = match curve.map(Curve::into_kind) {
            Some(CurveKind::Ellipse(ellipse)) => Some(ellipse),
            None => None,
            Some(_) => {
                return Err(SolidError::invalid_input(
                    OP_READ_POLAR,
                    "the stored polar curve is not an ellipse",
                ))
            }
        };
        Ok((polar, ellipse))
    }

    /// Same reader retaining the original owned curve wrapper and its identity.
    pub(crate) fn read_from_db_with_curve_object(
        k: &Kernel,
        db: &mut DatabaseReader,
        version: u64,
    ) -> Result<(Self, Option<Curve>)> {
        let (mut pc, has_curve) = Self::read_tables(db)?;
        if !has_curve {
            // m_cpCurve is the (null) back pointer passed in
            return Ok((Self::finish_back_pointer(db, pc, false), None));
        }

        // read m_cpCurve object from file (statuses ignored upstream)
        if db.file_type() == FileType::Ascii {
            db.go_to_next_line();
        }
        let header = ignored(db.read_type(true))?;
        let curve = match header {
            Some(h) => match (h.type_code, h.dim) {
                (Some(ty), Some(dim)) => ignored(Curve::read_from_db(k, ty, db, dim, version))?,
                // (an unread header: upstream passes uninitialised values on)
                _ => None,
            },
            None => None,
        };
        if curve
            .as_ref()
            .is_some_and(|curve| !matches!(curve.kind(), CurveKind::Ellipse(_)))
        {
            return Err(SolidError::invalid_input(
                OP_READ_POLAR,
                "the stored polar curve is not an ellipse",
            ));
        }
        pc.has_curve = curve.is_some();
        Ok((pc, curve))
    }

    /// `WriteToDB`: writes the set-up flag, the knot, angle and exactness counts and
    /// whether the table owns a copy of its curve (`owned_curve`; the ellipse's own
    /// table borrows it and passes `None`), then the three arrays (ASCII: a line
    /// break after the elements with index 5, 10, ... and after a partial last line),
    /// then the owned curve (`m_cpCurve`, any curve kind) with its type header, as
    /// a curve nested `depth` levels deep.
    ///
    /// # Errors
    ///
    /// Those of [`crate::topology::geometry_write::write_curve_at`] for the owned curve.
    pub fn write_to_db(
        &self,
        db: &mut DatabaseWriter,
        version: u64,
        owned_curve: Option<&Curve>,
        depth: usize,
    ) -> Result<()> {
        let ascii = db.file_type() == FileType::Ascii;
        let n_knots = self.natural_knots.len() as u64;
        let n_angles = self.angles.len() as u64;
        let n_exact = self.exact_conversion.len() as u64;
        let has_curve = i32::from(owned_curve.is_some());
        let possible = i32::from(self.possible);

        if ascii {
            db.out_i64(possible.into());
            db.out_str(
                " SmPolarConversion PolarConversionPossible - TRUE=tables are initialized \n",
            );
            db.out_u64(n_knots);
            db.out_str(" SmPolarConversion Number of Natural Knots \n");
            db.out_u64(n_angles);
            db.out_str(" SmPolarConversion Number of Angles \n");
            db.out_u64(n_exact);
            db.out_str(" SmPolarConversion Number of ExactConversion Flags \n");
            db.out_i64(has_curve.into());
            db.out_str(
                " SmPolarConversion TRUE = Has Curve for stored conversions, \
                 FALSE = back pointer \n",
            );
        } else {
            db.write_boolean(possible);
            db.write_long(n_knots as i64);
            db.write_long(n_angles as i64);
            db.write_long(n_exact as i64);
            db.write_boolean(has_curve);
        }

        // Write Arrays
        if ascii {
            let rows = |db: &mut DatabaseWriter, values: &mut dyn Iterator<Item = String>| {
                let mut n = 0usize;
                for (ii, v) in values.enumerate() {
                    db.out_str(&v).out_str(" ");
                    if ii > 0 && ii % 5 == 0 {
                        db.out_str("\n");
                    }
                    n += 1;
                }
                if n > 0 && n % 5 != 0 {
                    db.out_str("\n");
                }
            };
            let exact = &self.exact_conversion;
            let mut flags = exact.iter().map(|&b| i32::from(b).to_string());
            rows(db, &mut self.natural_knots.iter().map(|&x| fixed16(x)));
            rows(db, &mut self.angles.iter().map(|&x| fixed16(x)));
            rows(db, &mut flags);
        } else {
            if n_knots > 0 {
                db.write_doubles(&self.natural_knots);
            }
            if n_angles > 0 {
                db.write_doubles(&self.angles);
            }
            if n_exact > 0 {
                let exact = &self.exact_conversion;
                let flags: Vec<i32> = exact.iter().map(|&b| i32::from(b)).collect();
                db.write_booleans(&flags);
            }
        }

        // write out the curve the table owns (the ellipse's own table borrows it)
        if let Some(c) = owned_curve {
            if ascii {
                db.out_str(" SmPolarConversion->CurveBeingConverted \n");
            }
            db.write_type(c.type_code(), Some(c.dim()));
            crate::topology::geometry_write::write_curve_at(c, db, version, depth)?;
        } else if ascii {
            db.out_str("Has No Copy of SmPolarConversion->CurveBeingConverted \n");
        }
        Ok(())
    }

    /// The end of a read whose table has no stored curve: the back pointer
    /// (`has_back_pointer`: the ellipse being read; else null), then the ASCII line
    /// break.
    fn finish_back_pointer(db: &mut DatabaseReader, mut pc: Self, has_back_pointer: bool) -> Self {
        pc.has_curve = has_back_pointer;
        if db.file_type() == FileType::Ascii {
            db.go_to_next_line();
        }
        pc
    }

    /// The tables of [`PolarConversion::read_from_db`] and the has-curve flag.
    fn read_tables(db: &mut DatabaseReader) -> Result<(Self, bool)> {
        let bad = |reason: &'static str| SolidError::invalid_input(OP_READ_POLAR, reason);
        let ascii = db.file_type() == FileType::Ascii;

        // locals
        let (possible, n_knots, n_angles, n_exact, has_curve) = if ascii {
            let possible = db.in_stream_mut().and_then(InputStream::extract_int);
            db.go_to_next_line();
            let n_knots = db.in_stream_mut().and_then(InputStream::extract_ulong);
            db.go_to_next_line();
            let n_angles = db.in_stream_mut().and_then(InputStream::extract_ulong);
            db.go_to_next_line();
            let n_exact = db.in_stream_mut().and_then(InputStream::extract_ulong);
            db.go_to_next_line();
            let has_curve = db.in_stream_mut().and_then(InputStream::extract_int);
            db.go_to_next_line();
            (
                possible.ok_or_else(|| bad("unreadable flag"))? != 0,
                n_knots.ok_or_else(|| bad("unreadable count"))?,
                n_angles.ok_or_else(|| bad("unreadable count"))?,
                n_exact.ok_or_else(|| bad("unreadable count"))?,
                has_curve.ok_or_else(|| bad("unreadable flag"))? != 0,
            )
        } else {
            let possible = db.read_boolean()? != 0;
            let n_knots = db.read_ulong()?;
            let n_angles = db.read_ulong()?;
            let n_exact = db.read_ulong()?;
            let has_curve = db.read_boolean()? != 0;
            (possible, n_knots, n_angles, n_exact, has_curve)
        };

        // array sizes must fit in what is left of the stream
        let count = |n: u64| usize::try_from(n).map_err(|_| bad("count too large"));
        let (n_knots, n_angles, n_exact) = (count(n_knots)?, count(n_angles)?, count(n_exact)?);
        let min_bytes = if ascii { 1 } else { 8 };
        db.ensure_available(n_knots, min_bytes, OP_READ_POLAR)?;
        db.ensure_available(n_angles, min_bytes, OP_READ_POLAR)?;
        db.ensure_available(n_exact, if ascii { 1 } else { 4 }, OP_READ_POLAR)?;

        let mut pc = Self::new();
        pc.possible = possible;

        if ascii {
            let read_reals = |db: &mut DatabaseReader, n: usize| -> Result<Vec<f64>> {
                let mut v = Vec::new(); // grown as read: n comes from the file
                for ii in 0..n {
                    let x = db
                        .in_stream_mut()
                        .and_then(InputStream::extract_double)
                        .ok_or_else(|| bad("unreadable value"))?;
                    v.push(x);
                    if ii > 0 && ii % 5 == 0 {
                        db.go_to_next_line();
                    }
                }
                if n > 0 && n % 5 != 0 {
                    db.go_to_next_line();
                }
                Ok(v)
            };
            pc.natural_knots = read_reals(db, n_knots)?;
            pc.angles = read_reals(db, n_angles)?;
            let mut flags = Vec::new(); // grown as read: n_exact comes from the file
            for ii in 0..n_exact {
                let x = db
                    .in_stream_mut()
                    .and_then(InputStream::extract_int)
                    .ok_or_else(|| bad("unreadable flag"))?;
                flags.push(x != 0);
                if ii > 0 && ii % 5 == 0 {
                    db.go_to_next_line();
                }
            }
            if n_exact > 0 && n_exact % 5 != 0 {
                db.go_to_next_line();
            }
            pc.exact_conversion = flags;
        } else {
            pc.natural_knots = db.read_doubles(n_knots)?;
            pc.angles = db.read_doubles(n_angles)?;
            pc.exact_conversion = db
                .read_booleans(n_exact)?
                .into_iter()
                .map(|b| b != 0)
                .collect();
        }

        Ok((pc, has_curve))
    }
}

// ---- the ellipse and circle -----------------------------------------------------------

impl Ellipse {
    // ---- construction -----------------------------------------------------------------

    /// The empty circle that `ReadFromDB` fills (`SmCircle()`): [`Ellipse::empty`] of
    /// dimension 3 of class `SmCircle`.
    #[must_use]
    pub const fn empty_circle() -> Self {
        let mut e = Self::empty(3);
        e.circle = true;
        e
    }

    /// The circle centred at `center` with `radius` along the axes `x_axis` and
    /// `y_axis` over the angular domain `domain` (degrees) (`SmCircle(Center, XAxis,
    /// YAxis, AnalDomain, dRadius, ..)`): the ellipse of [`Ellipse::new`] with both
    /// radii `radius`, of class `SmCircle` and form `SM_CF_CIRCULAR_ARC`.
    ///
    /// # Errors
    ///
    /// Those of [`Ellipse::new`].
    pub fn new_circle(
        k: &Kernel,
        center: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        domain: Extent1d,
        radius: f64,
        opts: &EllipseOptions<'_>,
    ) -> Result<Self> {
        let mut e = Self::new(k, center, x_axis, y_axis, domain, radius, radius, opts)?;
        e.circle = true;
        e.base.form = BSplineCurveForm::CircularArc;
        Ok(e)
    }

    /// The circle through `start`, `mid` and `end` (`SmCircle(StartPoint, MidPoint,
    /// EndPoint, lDimension, bClosedCircle, ..)`): the arc from `start` to `end`, or
    /// the closed circle when `closed` (`smgu_CircleFrom3Points`), placement set with
    /// `SetCanonical` and the NURB made by [`Self::make_nurb`] (both statuses
    /// ignored); form `SM_CF_CIRCULAR_ARC`.
    ///
    /// # Errors
    ///
    /// The status of `smgu_CircleFrom3Points` (collinear points, a bad dimension;
    /// upstream then continues with uninitialised values); [`SolidError::Unported`]
    /// from `MakeNurb`.
    pub fn circle_from_3_points(
        k: &Kernel,
        start: Point3,
        mid: Point3,
        end: Point3,
        dim: u64,
        closed: bool,
    ) -> Result<Self> {
        let mut e = Self::empty(dim);
        e.circle = true;

        // compute circle parameters
        let c = circle_from_3_points(start, mid, end, dim, closed)?;

        // set Parent Object values
        let _ = e.position.set_canonical(c.center, c.x_axis, c.y_axis);
        e.anal_domain = c.domain;
        e.radius_x = c.radius;
        e.radius_y = c.radius;

        // make the associated NURB and set the Form value
        ignore_status(e.make_nurb(k))?;
        e.base.form = BSplineCurveForm::CircularArc;
        Ok(e)
    }

    /// A copy (`SmEllipse(const SmEllipse&)` and `Copy`; for a circle
    /// `SmCircle(const SmCircle&)` and `SmCircle::Copy`): the B-spline base and the
    /// ellipse data are copied, the polar converter is set up again, then the NURB is
    /// set again from this ellipse's (rebuilding the converter; statuses ignored).
    ///
    /// # Errors
    ///
    /// None but [`SolidError::Unported`] (never produced here).
    pub fn copy(&self, k: &Kernel) -> Result<Self> {
        let mut c = Self {
            base: self.base.copy(),
            position: self.position,
            anal_domain: self.anal_domain,
            radius_x: self.radius_x,
            radius_y: self.radius_y,
            polar: PolarConversion::new(),
            inside_out: self.inside_out,
            circle: self.circle,
        };

        // stop infinite recursion: don't copy underlying PolarConverter - reSet it
        ignore_status(c.set_up_polar_conversion(k))?;

        // (Copy / the circle copy constructor)
        ignore_status(c.set_from_gw_nurb(k, self.base.gw_nurb()))?;
        Ok(c)
    }

    /// The ellipse with placement `origin`, radii `radius1` (X) and `radius2` (Y)
    /// over `interval` (degrees; `None`: `[0, 360]`), inside out when `inside_out`
    /// (`SmEllipse::CreateCanonical`; dimension 3).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the NURB could not be made; those of [`Ellipse::new`].
    pub fn create_canonical(
        k: &Kernel,
        origin: &Axis2Placement,
        radius1: f64,
        radius2: f64,
        interval: Option<&Extent1d>,
        inside_out: Option<bool>,
    ) -> Result<Self> {
        let extent = Extent1d::from_min_max(0.0, 360.0);

        // make the ellipse
        let e = Self::new(
            k,
            origin.origin(),
            origin.x_axis(),
            origin.y_axis(),
            interval.copied().unwrap_or(extent),
            radius1,
            radius2,
            &EllipseOptions {
                dim: 3,
                nurb: None,
                inside_out: inside_out.unwrap_or(false),
            },
        )?;

        // all done
        if e.base.nurb.is_none() {
            return Err(SolidError::sm_err(OP_CANONICAL));
        }
        Ok(e)
    }

    /// The circle with placement `origin` and `radius` over `interval` (degrees;
    /// `None`: `[0, 360]`), inside out when `inside_out` (`SmCircle::CreateCanonical`;
    /// dimension 3). The circle may be without NURB when NLib fails.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a radius not above `SM_EFF_ZERO` (or NaN); those of
    /// [`Ellipse::new`].
    pub fn create_canonical_circle(
        k: &Kernel,
        origin: &Axis2Placement,
        radius: f64,
        interval: Option<&Extent1d>,
        inside_out: Option<bool>,
    ) -> Result<Self> {
        // Reject zero/negative radius: no valid NURB
        if radius.is_nan() || radius <= EFF_ZERO {
            return Err(SolidError::sm_err(OP_CIRCLE_CANONICAL));
        }

        // new circle, interval = given interval or [0,360], InsideOut = given value or FALSE
        let null_interval = Extent1d::from_min_max(0.0, 360.0);
        Self::new_circle(
            k,
            origin.origin(),
            origin.x_axis(),
            origin.y_axis(),
            interval.copied().unwrap_or(null_interval),
            radius,
            &EllipseOptions {
                dim: 3,
                nurb: None,
                inside_out: inside_out.unwrap_or(false),
            },
        )
    }

    // ---- the circle class -------------------------------------------------------------

    /// Whether the object is an `SmCircle` (the class, not the geometric test
    /// [`Ellipse::is_circle`]).
    #[must_use]
    pub const fn is_circle_class(&self) -> bool {
        self.circle
    }

    /// `IsKindOf`: `SmCircle_TYPE` for a circle, then those of `SmEllipse`
    /// ([`Ellipse::is_kind_of`]).
    #[must_use]
    pub const fn kind_of(&self, t: i64) -> bool {
        (self.circle && t == CIRCLE_TYPE) || Self::is_kind_of(t)
    }

    /// The radius at the 0 degree point (`SmCircle::GetRadius`).
    #[must_use]
    pub const fn radius(&self) -> f64 {
        self.radius_x
    }

    /// Sets both radii, leaving the NURB as it is (`SmCircle::SetRadius`).
    pub fn set_radius(&mut self, radius: f64) {
        self.radius_x = radius;
        self.radius_y = radius;
    }

    /// The circle's arc: placement, radius and STEP domain ends in degrees.
    const fn arc_data(&self) -> ArcData {
        (
            self.position,
            self.radius_x,
            self.anal_domain.min(),
            self.anal_domain.max(),
        )
    }

    /// `IsArc(lNumberOfSamplePoints, dTol, ..)`: a circle is its own arc
    /// (placement, radius, start and end angle in degrees; `SmCircle::IsArc`, whose
    /// sample count and tolerance are unused); an ellipse runs the base class
    /// ([`crate::curve::curve::p5::is_arc_base`] on its NURB, with the ellipse's
    /// `IsDegenerate`).
    ///
    /// # Errors
    ///
    /// Those of [`crate::curve::curve::p5::is_arc_base`].
    pub fn is_arc(&self, k: &Kernel, num_samples: u64, tol: f64) -> Result<Option<ArcData>> {
        if self.circle {
            return Ok(Some(self.arc_data()));
        }
        crate::curve::curve::p5::is_arc_base(
            k,
            &self.base,
            |_, tol| Ok(self.is_degenerate(tol)),
            num_samples,
            tol,
        )
    }

    /// The circle a B-spline curve is, when its geometry and representation are those
    /// of a circle (`SmCircle::IsNurbCurveCircle`; `tolerance_scale` defaults to `1.0`):
    /// a copy of a circle; `None` for a curve that is not of degree 2 and rational, is
    /// degenerate (`IsDegenerate` to `scale * ANALYTIC_TOL_SCALE * SM_EFF_ZERO * (1 +
    /// |P|max)`, `P` the point at 0.4567 of the natural interval) or is not an arc
    /// (`IsArc(5, ..)` to the same tolerance); else the circle of the arc's frame,
    /// radius and angular domain, dimension 3, with a copy of the curve's NURB
    /// (`pOptNurb`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a curve that is not an `SmBSplineCurve` (a
    /// Hermite, composite, conic or procedural curve); those of the curve's
    /// evaluator, [`Curve::is_degenerate`], [`Curve::is_arc`] and
    /// [`Ellipse::new_circle`].
    pub fn is_nurb_curve_circle(
        k: &Kernel,
        test_curve: &Curve,
        tolerance_scale: Option<f64>,
    ) -> Result<Option<Self>> {
        let tolerance_scale = tolerance_scale.unwrap_or(1.0);
        let line_base;
        let test_base: &BSplineCurve = match test_curve.kind() {
            CurveKind::BSpline(c) => c,
            CurveKind::Ellipse(e) => {
                // low work - already a circle
                if e.is_circle_class() {
                    // copy the circle
                    return Ok(Some(e.copy(k)?));
                }
                e.base()
            }
            CurveKind::Parabola(c) => c.base(),
            CurveKind::Hyperbola(c) => c.base(),
            CurveKind::Line(l) => {
                line_base = l.base_with_nurb()?;
                &line_base
            }
            _ => {
                return Err(SolidError::invalid_input(
                    OP_IS_NURB_CURVE_CIRCLE,
                    "the curve is not an SmBSplineCurve",
                ))
            }
        };

        // no work - not a degree 2 rational curve
        let deg = test_curve.degree()?;
        if deg != 2 || !test_curve.is_rational()? {
            return Ok(None);
        }

        // scale zero value
        let mid_point =
            test_curve.evaluate_point(k, test_curve.natural_interval().evaluate(0.4567))?;
        let scaled_zero =
            tolerance_scale * ANALYTIC_TOL_SCALE * EFF_ZERO * (1.0 + mid_point.max_dimension());

        // skip degenerate curves
        if test_curve.is_degenerate(k, scaled_zero, None)? {
            return Ok(None);
        }

        // Get circle reference frame, radius and angle extent
        let Some((ref_frame, radius, start_ang_deg, end_ang_deg)) =
            test_curve.is_arc(k, 5, scaled_zero)?
        else {
            // when isoParameterCurve is not a circular Arc
            return Ok(None);
        };
        let anal_domain = Extent1d::from_min_max(start_ang_deg, end_ang_deg);

        // make new circle
        let circle = Self::new_circle(
            k,
            ref_frame.origin(),
            ref_frame.x_axis(),
            ref_frame.y_axis(),
            anal_domain,
            radius,
            &EllipseOptions {
                dim: 3,
                nurb: Some(test_base),
                inside_out: false,
            },
        )?;
        // all done
        Ok(Some(circle))
    }

    /// The offset of the curve by `offset_distance` in the plane with normal
    /// `offset_plane_normal` within `approx_tol_3d`, and its maximum gap
    /// (`CreateSimpleOffset`). For a circle (`SmCircle::CreateSimpleOffset`) whose
    /// axis is parallel or anti-parallel to the normal (`IsParallelTo` accepts both,
    /// so the first branch always wins: the radius grows by the distance) the offset
    /// is the circle with the new radius: `SM_ERR` below `-approx_tol_3d`, a
    /// degenerate curve at the center below `approx_tol_3d`, else a copy with a new
    /// NURB (gap 0). Other normals, and ellipses, use
    /// `SmBSplineCurve::CreateSimpleOffset` (with the circle's own `IsArc`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a negative offset radius; those of
    /// [`BSplineCurve::create_simple_offset`] and [`BSplineCurve::create_degenerate_curve`].
    pub fn create_simple_offset(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        offset_plane_normal: Vec3,
        offset_distance: f64,
        match_parameterization: bool,
    ) -> Result<(Curve, f64)> {
        if !self.circle {
            // SmBSplineCurve::CreateSimpleOffset (IsArc is SmCurve's)
            let (c, gap) = self.base.create_simple_offset(
                k,
                approx_tol_3d,
                offset_plane_normal,
                offset_distance,
                match_parameterization,
            )?;
            return Ok((Curve::from(c), gap));
        }

        let my_radius = self.radius_x;
        let my_z_axis = self.position.z_axis();
        let offset_rad = if offset_plane_normal.is_parallel_to(my_z_axis, EFF_ZERO_DEG) {
            my_radius + offset_distance
        } else if offset_plane_normal.is_parallel_to(-my_z_axis, EFF_ZERO_DEG) {
            my_radius - offset_distance
        } else {
            // Out-of-plane offset would not be a circle.
            let arc = self.arc_data();
            let (c, gap) = self.base.create_simple_offset_with(
                k,
                approx_tol_3d,
                offset_plane_normal,
                offset_distance,
                match_parameterization,
                &|_, _, _| Ok(Some(arc)),
            )?;
            return Ok((Curve::from(c), gap));
        };

        if offset_rad < -approx_tol_3d {
            return Err(SolidError::sm_err(OP_OFFSET));
        } else if offset_rad < approx_tol_3d {
            // Offset is a degenerate curve.
            let c = BSplineCurve::create_degenerate_curve(3, self.position.origin())?;
            return Ok((Curve::from(c), 0.0));
        }

        // Ok, all is good.  Create an SmCircle for output.
        let mut offset_circle = self.copy(k)?;
        offset_circle.set_radius(offset_rad);
        ignore_status(offset_circle.make_nurb(k))?;
        Ok((Curve::from(offset_circle), 0.0))
    }

    // ---- validity ---------------------------------------------------------------------

    /// `AssertValid` (`SmEllipse::AssertValid`; for a circle `SmCircle::AssertValid`,
    /// which runs the ellipse's checks unless `level_given` and then checks the radii
    /// are equal, report 0 of `SmCircle`). The ellipse's checks: unless
    /// `level_given` the B-spline base checks (`SmBSplineCurve::AssertValid` with the
    /// ellipse's `MakeNurb`, `GetDegree`, `IsAnalytic` and `IsDegenerate`), the polar
    /// converter ([`PolarConversion::assert_valid`], about `polar_key`), only circles
    /// inside out (report 0 of `SmEllipse`), and at 5 STEP samples the NURB point of
    /// the converted parameter on the STEP point (report 1) and on the ellipse
    /// (report 2) within `100 SM_EFF_ZERO` scaled. Reports name `key`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when a STEP sample cannot be converted (upstream: an
    /// uninitialised parameter); those of the base and polar checks;
    /// [`SolidError::Unported`] from `MakeNurb` or the conversion.
    pub fn assert_valid(
        &mut self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
        polar_key: ObjectKey,
    ) -> Result<bool> {
        self.assert_valid_on(k, list, level_given, key, polar_key, None)
    }

    /// [`Ellipse::assert_valid`] of a UV trim on `trim_surface`, its face's surface:
    /// the `SmBSplineCurve` checks measure the trim on it with
    /// `Kernel::fixes.uv_trim_checks_on_surface` (WR-560, see
    /// [`BSplineCurve::assert_valid_on`]).
    ///
    /// # Errors
    ///
    /// As [`Ellipse::assert_valid`].
    pub fn assert_valid_on(
        &mut self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
        polar_key: ObjectKey,
        trim_surface: Option<&Surface>,
    ) -> Result<bool> {
        let mut list = list;
        if !self.circle {
            return self.ellipse_assert_valid(k, list, level_given, key, polar_key, trim_surface);
        }

        // init return value
        let mut rtn = true;

        // call the base class AssertValid
        rtn &= if level_given {
            true
        } else {
            self.ellipse_assert_valid(
                k,
                list.as_deref_mut(),
                level_given,
                key,
                polar_key,
                trim_surface,
            )?
        };

        // Check that analytic ellipse is a circle
        rtn &= report(
            &mut list,
            key,
            (labels::CIRCLE_TYPE, self.class_string()),
            0,
            0,
            are_same(self.radius_x, self.radius_y),
            EFF_ZERO,
            self.radius_x - self.radius_y,
        )?;
        Ok(rtn)
    }

    /// `SmEllipse::AssertValid` (see [`Self::assert_valid`]).
    fn ellipse_assert_valid(
        &mut self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
        polar_key: ObjectKey,
        trim_surface: Option<&Surface>,
    ) -> Result<bool> {
        let mut list = list;

        // init return value
        let mut rtn = true;

        // call the base class AssertValid
        if !level_given {
            // SmBSplineCurve::AssertValid makes a missing NURB (virtual MakeNurb)
            if self.base.nurb.is_none() {
                ignore_status(self.make_nurb(k))?;
            }
            let this = &*self;
            let is_degenerate = |tol: f64| Ok(this.is_degenerate(tol));
            rtn &= this.base.assert_valid_with(
                k,
                list.as_deref_mut(),
                false,
                key,
                &DerivedOverrides {
                    degree: Some(2),
                    analytic: true,
                    bounded: true,
                    is_degenerate: &is_degenerate,
                    trim_surface,
                },
            )?;
        }

        // make sure nested SmPolarConversion is up to date
        rtn &= self
            .polar
            .assert_valid(k, self, list.as_deref_mut(), polar_key)?;

        // check that the use of bInsideOut is limited to circles
        rtn &= report(
            &mut list,
            key,
            (labels::ELLIPSE_TYPE, self.class_string()),
            0,
            0,
            !self.inside_out || self.circle,
            UNDEF_DOUBLE,
            UNDEF_DOUBLE,
        )?;

        // Check that the BsplineCurve shape is the same as the analytic shape
        let smp_count = 5u32;
        for ii in 0..smp_count {
            // get equivalent Nurb and STEP params
            let step_t = self
                .anal_domain
                .evaluate(f64::from(ii) / f64::from(smp_count - 1));
            // (status ignored: a failed conversion leaves the polar table's guess)
            let Some(nurb_t) = self
                .polar
                .convert_to_nurbs_parameter_or_written(k, self, step_t, 0.0)?
            else {
                return Err(SolidError::invalid_input(
                    OP_ASSERT,
                    "STEP parameter not convertible (upstream: uninitialised)",
                ));
            };

            // Evaluate the points (status ignored)
            let nurb_point = ignored(self.base.evaluate_point(nurb_t))?.unwrap_or(Vec3::UNDEF);
            let step_point = self.step_point(step_t);
            let scaled_zero = EFF_ZERO * 100.0 * (1.0 + nurb_point.max_dimension());

            // Points are equal - others vary by a small amount
            let dist = (nurb_point - step_point).length();
            rtn &= report(
                &mut list,
                key,
                (labels::ELLIPSE_TYPE, self.class_string()),
                0,
                1,
                dist < scaled_zero,
                scaled_zero,
                dist,
            )?;

            // every Nurb Point must lie on the Ellipse
            let step_param = self.step_inversion(k, nurb_point, false, 0.0).0;
            let check_point = self.step_point(step_param);
            let check_dist = (check_point - nurb_point).length();
            rtn &= report(
                &mut list,
                key,
                (labels::ELLIPSE_TYPE, self.class_string()),
                0,
                2,
                check_dist < scaled_zero,
                scaled_zero,
                check_dist,
            )?;
        }

        // all done
        Ok(rtn)
    }

    // ---- persistence ------------------------------------------------------------------

    /// Reads an ellipse (or, with `circle`, a circle: `SmCircle::ReadFromDB`, which has
    /// no data of its own) written by `WriteToDB` (`ReadFromDB`): center, X axis, Y
    /// axis (three values each), domain ends, X and Y radius, the inside-out flag (one
    /// group per line in ASCII files), the polar converter
    /// ([`PolarConversion::read_from_db`]), then the B-spline base record
    /// ([`BSplineCurve::read_from_db`]). The placement and domain are set with their
    /// statuses ignored; the NURB is the stored one (the converter is not rebuilt).
    /// A 3D record read for `dim == 2` is made 2D by [`Self::convert_to_2d`].
    /// Unreadable ASCII values keep their defaults.
    ///
    /// # Errors
    ///
    /// A failed binary read of the ellipse data; the errors of
    /// [`PolarConversion::read_from_db`]; [`SolidError::Unported`] from the base
    /// reader (its other errors are ignored, as upstream).
    pub fn read_from_db(
        k: &Kernel,
        db: &mut DatabaseReader,
        dim: u64,
        version: u64,
        circle: bool,
    ) -> Result<Self> {
        // init output object
        let mut e = Self::empty(3);
        e.circle = circle;

        // locals
        let mut orig = Vec3::UNDEF;
        let mut x = Vec3::UNDEF;
        let mut y = Vec3::UNDEF;
        let mut anal = [0.0f64; 2];
        let mut radius_x = 0.0f64;
        let mut radius_y = 0.0f64;
        let mut inside_out = false;

        if db.file_type() == FileType::Ascii {
            for v in [&mut orig, &mut x, &mut y] {
                if let Some(s) = db.in_stream_mut() {
                    for c in [&mut v.x, &mut v.y, &mut v.z] {
                        extract(s, c);
                    }
                }
                db.go_to_next_line();
            }
            if let Some(s) = db.in_stream_mut() {
                for c in &mut anal {
                    extract(s, c);
                }
            }
            db.go_to_next_line();
            for c in [&mut radius_x, &mut radius_y] {
                if let Some(s) = db.in_stream_mut() {
                    extract(s, c);
                }
                db.go_to_next_line();
            }
            if let Some(v) = db.in_stream_mut().and_then(InputStream::extract_int) {
                inside_out = v != 0;
            }
            db.go_to_next_line();
        } else {
            for v in [&mut orig, &mut x, &mut y] {
                v.x = db.read_double()?;
                v.y = db.read_double()?;
                v.z = db.read_double()?;
            }
            anal[0] = db.read_double()?;
            anal[1] = db.read_double()?;
            radius_x = db.read_double()?;
            radius_y = db.read_double()?;
            inside_out = db.read_boolean()? != 0;
        }

        // PolarConverter - read directly into empty analytic
        if db.file_type() == FileType::Ascii {
            db.go_to_next_line();
        }
        e.polar = PolarConversion::read_from_db(db)?;

        // load the obj
        let _ = e.position.set_canonical(orig, x, y);
        let _ = e.anal_domain.set_min_max(anal[0], anal[1]);
        e.radius_x = radius_x;
        e.radius_y = radius_y;
        e.inside_out = inside_out;

        // read the base type (status ignored upstream); its 2D fix-up of a 3D record
        // calls the ellipse's ConvertTo2D, so it is done here
        if let Some(b) = ignored(BSplineCurve::read_from_db(db, 3, version))? {
            let read_dim = b.dim;
            e.base = b;
            if dim == 2 && read_dim == 3 {
                ignore_status(e.convert_to_2d(k))?;
            }
        }

        // all done
        Ok(e)
    }

    /// `WriteToDB` (`SmCircle::WriteToDB` is the same): writes the placement origin
    /// and axes, the angular domain (degrees), the two radii and the inside-out flag,
    /// then the polar converter (which borrows this ellipse) and the B-spline curve
    /// part.
    ///
    /// # Errors
    ///
    /// Those of [`PolarConversion::write_to_db`]; `SM_ERR` when the B-spline part has
    /// no NURB (upstream ignores the part's status; its virtual
    /// `GetOrCreateGwNurbPointer` always has one to write).
    pub fn write_to_db(&self, db: &mut DatabaseWriter, version: u64) -> Result<()> {
        let o = self.position.origin();
        let x = self.position.x_axis();
        let y = self.position.y_axis();
        let (lo, hi) = (self.anal_domain.min(), self.anal_domain.max());
        let inside_out = i32::from(self.inside_out);
        let ascii = db.file_type() == FileType::Ascii;
        if ascii {
            db.out_reals(&[o.x, o.y, o.z]);
            db.out_str(" SmEllipse Axis Origin \n");
            db.out_reals(&[x.x, x.y, x.z]);
            db.out_str(" SmEllipse X Axis\n");
            db.out_reals(&[y.x, y.y, y.z]);
            db.out_str(" SmEllipse Y Axis\n");
            db.out_reals(&[lo, hi]);
            db.out_str(" SmEllipse angular arc domain in degrees \n");
            db.out_f64(self.radius_x);
            db.out_str(" SmEllipse X_Axis radius \n");
            db.out_f64(self.radius_y);
            db.out_str(" SmEllipse Y_Axis radius \n");
            db.out_i64(inside_out.into());
            db.out_str(" SmEllipse InsideOut Flag \n");
        } else {
            db.write_doubles(&[o.x, o.y, o.z, x.x, x.y, x.z, y.x, y.y, y.z]);
            db.write_doubles(&[lo, hi, self.radius_x, self.radius_y]);
            db.write_boolean(inside_out);
        }

        // PolarConverter
        if ascii {
            db.out_str(" SmEllipse->PolarConverter \n");
        }
        self.polar.write_to_db(db, version, None, 0)?;

        self.base.write_to_db(db, version)
    }

    // ---- conversions ------------------------------------------------------------------

    /// Projects the ellipse onto the x-y plane (`ConvertTo2D`): the NURB is made 2D
    /// (status ignored), the placement's `z` values are dropped and the new axes are
    /// the projected radius vectors (when the projected axes are no longer
    /// perpendicular, both are taken at the angle of the first root of the extremal
    /// radius quadratic, as upstream); the radii become their lengths, zero axes are
    /// replaced by the perpendicular of the other, the placement is set
    /// (`SetCanonical`, status ignored) and the polar converter rebuilt (status
    /// ignored).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the extremal radius quadratic has no root (upstream:
    /// uninitialised values).
    pub fn convert_to_2d(&mut self, k: &Kernel) -> Result<()> {
        // modify the NURB
        ignore_status(self.base.convert_to_2d())?;

        // Get current position
        let mut origin = self.position.origin();
        let mut x_axis = self.position.x_axis();
        let mut y_axis = self.position.y_axis();

        // convert position to 2d
        origin.z = 0.0;
        x_axis.z = 0.0;
        y_axis.z = 0.0;

        // Most times X and Y are not still perp to one another
        let x_dot_x = x_axis.dot(x_axis);
        let x_dot_y = x_axis.dot(y_axis);
        let y_dot_y = y_axis.dot(y_axis);
        let scaled_zero = EFF_ZERO;

        let rx = self.radius_x;
        let ry = self.radius_y;
        let (mut new_x, mut new_y) = if x_dot_y.abs() < scaled_zero {
            // use the projected vectors as the new ellipse vectors
            (rx * x_axis, ry * y_axis)
        } else {
            // compute new angles for min/max radius values:
            // 0 = - (R2*R1*Y*X)*t*t + (R2*R2*Y*Y-R1*R1*X*X)*t + (R1*R2*X*Y)
            let coefs = [
                -ry * rx * x_dot_y,
                ry * ry * y_dot_y - rx * rx * x_dot_x,
                ry * rx * x_dot_y,
            ];
            let sols = solve_quadratic(&coefs, scaled_zero);
            // Upstream only uses the first root: it also computes ThetaY from the second
            // (uninitialised when the solver finds one root, e.g. a degenerate radius
            // makes the equation linear) but never uses it. Without any root the first
            // is uninitialised too.
            let Some(&first_root) = sols.first() else {
                return Err(SolidError::invalid_input(
                    OP_TO_2D,
                    "extremal radius quadratic without a root",
                ));
            };
            let theta_x_rad = first_root.atan();

            // get max vectors for each angle
            let v = rx * theta_x_rad.cos() * x_axis + ry * theta_x_rad.sin() * y_axis;
            let w = rx * theta_x_rad.cos() * x_axis + ry * theta_x_rad.sin() * y_axis;
            (v, w)
        };

        // compute new radius values
        self.radius_x = new_x.length();
        self.radius_y = new_y.length();

        // (a circle no longer a circle and a degenerate projection are only logged)

        // compute new axis values (operator/, not the guarded operator/=)
        if self.radius_x > scaled_zero {
            new_x = Div::div(new_x, self.radius_x);
        }
        if self.radius_y > scaled_zero {
            new_y = Div::div(new_y, self.radius_y);
        }

        // Don't pass zero vectors to the SmAxis2Placement.
        if self.radius_x <= scaled_zero {
            new_x = Vec3::new(new_y.y, -new_y.x, 0.0); // = Y x Z, sNewY is unit.
        }
        if self.radius_y <= scaled_zero {
            new_y = Vec3::new(-new_x.y, new_x.x, 0.0); // = Z x X, sNewX is unit.
        }

        // modify the Position
        let _ = self.position.set_canonical(origin, new_x, new_y);

        // recompute the polar conversion
        ignore_status(self.set_up_polar_conversion(k))?;
        Ok(())
    }

    /// The STEP angle in degrees of the NURB parameter `nurbs_param`
    /// (`ConvertTFromNURBSToSTEP`: [`PolarConversion::convert_to_polar_parameter`]).
    ///
    /// # Errors
    ///
    /// As [`PolarConversion::convert_to_polar_parameter`].
    pub fn convert_t_from_nurbs_to_step(&self, k: &Kernel, nurbs_param: f64) -> Result<f64> {
        Ok(self
            .polar
            .convert_to_polar_parameter(k, self, nurbs_param)?
            .0)
    }

    // ---- edits ------------------------------------------------------------------------

    /// Makes the NURB (`MakeNurb`) of a free ellipse ([`Self::make_nurb_owned`] without
    /// owner).
    ///
    /// # Errors
    ///
    /// As [`Self::make_nurb_owned`].
    pub fn make_nurb(&mut self, k: &Kernel) -> Result<()> {
        self.make_nurb_owned(k, None)
    }

    /// Makes the NURB (`MakeNurb`) of the ellipse owned by `owner`: the rational
    /// quadratic elliptical arc over the STEP domain (`N_CreateEllipticalArcTol` with
    /// angle tolerance `0.01` without edge; an inside-out ellipse is built over the
    /// mirrored angles with the Y axis negated). When a domain end is within `3e-4`
    /// degrees of `0`, `180` or `360` (NLib may snap it), the STEP domain is
    /// recomputed from the NURB's end points. The NURB is set (rebuilding the polar
    /// converter); a 2D ellipse is then made 2D ([`Self::convert_to_2d`], status
    /// ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when NLib fails (the old NURB is gone); [`SolidError::Unported`] for an
    /// owner whose edge tolerance or generator range is needed (see the module
    /// deviations).
    pub fn make_nurb_owned(&mut self, k: &Kernel, owner: Option<ObjectRef>) -> Result<()> {
        self.make_nurb_owned_with_edge_tol(k, owner, None)
    }

    /// [`Self::make_nurb_owned`] given the tolerance of the edge the ellipse's owner
    /// chain reaches (`GetEdge()->GetTolerance()`), resolved by a caller that holds it
    /// (`Some` means that edge exists, whatever `owner` says). Then the arc's angle
    /// tolerance is a hundredth of that tolerance over the speed at the mid angle
    /// (`EvaluateSTEP`) [B630].
    ///
    /// # Errors
    ///
    /// As [`Self::make_nurb_owned`].
    pub fn make_nurb_owned_with_edge_tol(
        &mut self,
        k: &Kernel,
        owner: Option<ObjectRef>,
        edge_zone_tol_3d: Option<f64>,
    ) -> Result<()> {
        // free existing m_pNurb object
        self.base.nurb = None;

        // get circle origin and axis from Circle Position
        let c = self.position.origin();
        let x = self.position.x_axis();
        let mut start_ang_deg = self.start_angle_deg();
        let mut end_ang_deg = self.end_angle_deg();

        // Get X and Y vectors depending on bInsideOut flag
        let y = if self.inside_out {
            // reverse and swap Start/End angles
            let t = start_ang_deg;
            start_ang_deg = 360.0 - end_ang_deg;
            end_ang_deg = 360.0 - t;

            if start_ang_deg > 360.0 {
                start_ang_deg -= 360.0;
                end_ang_deg -= 360.0;
            }

            // negate the Y vector
            -self.position.y_axis()
        } else {
            self.position.y_axis()
        };

        // Convert 3d tolerance to Angle tol [B630]: GetEdge()
        let angle_tol = if let Some(z_tol_3d) = edge_zone_tol_3d {
            let approx_tol_3d = z_tol_3d / 100.;

            let mid_p = (start_ang_deg + end_ang_deg) / 2.0;
            let point_and_der = self.evaluate_step(mid_p, 1)?;
            let der = point_and_der
                .get(1)
                .copied()
                .ok_or_else(|| SolidError::sm_err(OP_MAKE_NURB))?;

            approx_tol_3d / der.length()
        } else if owner.is_some_and(|o| {
            o.type_code == EDGE_TYPE
                || o.type_code == EDGEUSE_TYPE
                || crate::curve::curve::p2::is_kind_of(o.type_code)
        }) {
            // DISPATCH GAP: SmTol::GetZoneTol3d(const SmObject*) (geom.tol, green) on an opaque handle. Every
            // arm it dispatches to is ported (topology::{vertex, edge, face::p2, edgeuse::p1}::tolerance,
            // Brep::tolerance) but each takes `&Brep` and none is in scope here; wire once this code
            // receives the owning B-rep
            return Err(SolidError::Unported {
                upstream: "SmTol::GetZoneTol3d(const SmObject*)",
                unit: "geom.tol",
            });
        } else {
            0.01 // Preserves behavior
        };

        // Build the circular arc
        let arc = EllipseArc {
            center: c,
            x,
            y,
            r1: self.radius_x,
            r2: self.radius_y,
            start: start_ang_deg,
            end: end_ang_deg,
        };
        let cur = elliptical_arc_tol(&arc, angle_tol, ConicType::Quadratic)
            .map_err(SolidError::nl_ser(OP_MAKE_NURB))?;

        // when arc's domain may have been modified
        if is_zero_to_tol(start_ang_deg, 3.0e-4)
            || is_zero_to_tol(180.0 - start_ang_deg, 3.0e-4)
            || is_zero_to_tol(360.0 - start_ang_deg, 3.0e-4)
            || is_zero_to_tol(end_ang_deg, 3.0e-4)
            || is_zero_to_tol(180.0 - end_ang_deg, 3.0e-4)
            || is_zero_to_tol(360.0 - end_ang_deg, 3.0e-4)
        {
            // compute analytic domain from curve: get curve endPoints
            let cpts = cur.cpts();
            let start_pt = to_euclid(cpts.first().copied().ok_or_else(empty_nurb)?);
            let end_pt = to_euclid(cpts.last().copied().ok_or_else(empty_nurb)?);

            // convert endPoints to ellipse angles
            let (mut anal_start, _) = self.step_inversion(k, start_pt, false, 0.0);
            let (mut anal_end, _) = self.step_inversion(k, end_pt, false, 0.0);

            // Reset these to our analytic values.
            start_ang_deg = if self.inside_out {
                self.end_angle_deg()
            } else {
                self.start_angle_deg()
            };
            end_ang_deg = if self.inside_out {
                self.start_angle_deg()
            } else {
                self.end_angle_deg()
            };

            // watch out for periodicity
            if !is_zero_to_tol(anal_start - start_ang_deg, 3.0e-3) {
                if is_zero_to_tol(anal_start + 360.0 - start_ang_deg, 3.0e-3) {
                    anal_start += 360.0;
                }
                if is_zero_to_tol(anal_start - 360.0 - start_ang_deg, 3.0e-3) {
                    anal_start -= 360.0;
                }
            }
            if !is_zero_to_tol(anal_end - end_ang_deg, 3.0e-3) {
                if is_zero_to_tol(anal_end + 360.0 - end_ang_deg, 3.0e-3) {
                    anal_end += 360.0;
                }
                if is_zero_to_tol(anal_end - 360.0 - end_ang_deg, 3.0e-3) {
                    anal_end -= 360.0;
                }
            }

            // Set anal domain when its changed
            if !is_zero_to_tol(anal_start - start_ang_deg, EFF_ZERO)
                || !is_zero_to_tol(anal_end - end_ang_deg, EFF_ZERO)
            {
                // Either the start or end angle could be larger: m_bInsideOut.
                self.anal_domain.init();
                self.anal_domain.add_value(anal_start);
                self.anal_domain.add_value(anal_end);
            }
        }

        // set the m_pNurb pointer and use Notify to rebuild polarConverter
        ignore_status(self.set_from_gw_nurb(k, Some(&cur)))?;

        // Set ControlPoint Z values for 2d curves (status ignored)
        if self.base.dim == 2 {
            ignore_status(self.convert_to_2d(k))?;
        }

        // When this is a SmSurfOfExtrusion GenCurve, look for Parent Surface NURB
        // Interval constraint.
        if owner.is_some_and(|o| o.type_code == SURF_OF_EXTRUSION_TYPE) {
            // DISPATCH GAP: SmSurfOfExtrusion::GetGenDirParamExtent (surface.surf_of_extrusion_p1,
            // ported as SurfOfExtrusion::gen_dir_param_extent; no UNPORTED marker: the owner is an
            // opaque ObjectRef. An owning extrusion does this step itself (adjust_generator_interval,
            // SurfOfExtrusion::make_nurb) and never sets its generator's owner, so nothing reaches here)
            return Err(SolidError::Unported {
                upstream: "SmSurfOfExtrusion::GetGenDirParamExtent",
                unit: "surface.surf_of_extrusion_p1",
            });
        }

        if owner.is_some_and(|o| o.type_code == SURF_OF_REVOLUTION_TYPE) {
            // DISPATCH GAP: SmSurfOfRevolution::GetGenDirParamExtent (surface.surf_of_revolution_p1,
            // ported as SurfOfRevolution::gen_dir_param_extent; no UNPORTED marker: the owner is an
            // opaque ObjectRef. An owning revolution does this step itself (e.g. the cone's
            // adjust_gen_line_with_owner) and never sets its generator's owner, so nothing reaches here)
            return Err(SolidError::Unported {
                upstream: "SmSurfOfRevolution::GetGenDirParamExtent",
                unit: "surface.surf_of_revolution_p1",
            });
        }

        // all done
        Ok(())
    }

    /// `Notify(eNotifyOperation, ..)`: [`NotifyOperation::PostEdit`] rebuilds the polar
    /// converter (status ignored); the other operations change nothing here.
    ///
    /// # Errors
    ///
    /// None but [`SolidError::Unported`] (never produced here).
    pub fn notify(&mut self, k: &Kernel, operation: NotifyOperation) -> Result<()> {
        if operation == NotifyOperation::PostEdit {
            // recompute the polar converter
            ignore_status(self.set_up_polar_conversion(k))?;
        }
        Ok(())
    }

    /// `SmBSplineCurve::EditParameterization(new_param, notify)` of the ellipse:
    /// nothing when the natural interval ends are already the same (scaled zero);
    /// else the NURB (made by [`Self::make_nurb_owned`] when missing) is
    /// reparameterised, then with `notify` the polar converter rebuilt.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB; [`SolidError::Unported`] from `MakeNurb`.
    pub fn edit_parameterization(
        &mut self,
        k: &Kernel,
        new_param: &Extent1d,
        notify: bool,
        owner: Option<ObjectRef>,
    ) -> Result<()> {
        // get current interval
        let old = self.base.natural_interval();

        // low work: current interval == new interval
        if are_same(old.min(), new_param.min()) && are_same(old.max(), new_param.max()) {
            return Ok(());
        }

        // GetOrCreateGwNurbPointer: the virtual MakeNurb (status ignored)
        if self.base.nurb.is_none() {
            ignore_status(self.make_nurb_owned(k, owner))?;
        }
        self.base.edit_parameterization(new_param)?;

        if notify {
            self.notify(k, NotifyOperation::PostEdit)?;
        }
        Ok(())
    }

    /// Makes `new_interval` (degrees) the STEP domain and rebuilds the NURB over it,
    /// reparameterised to the NURB parameters the old table gives the new domain ends
    /// (`AdjustSTEPInterval`); nothing when the domains agree within `SM_EFF_ZERO`.
    ///
    /// # Errors
    ///
    /// Those of [`PolarConversion::convert_to_nurbs_parameter`] (nothing changed) and
    /// [`Self::make_nurb_owned`] (the new domain is kept).
    pub fn adjust_step_interval(
        &mut self,
        k: &Kernel,
        new_interval: &Extent1d,
        owner: Option<ObjectRef>,
    ) -> Result<()> {
        // no work - new interval is same as old
        if new_interval.is_contained_by(&self.anal_domain, EFF_ZERO)
            && self.anal_domain.is_contained_by(new_interval, EFF_ZERO)
        {
            return Ok(());
        }

        // Locals to get the Nurbs parameters corresponding to the new STEP interval.
        let step_t0 = new_interval.min();
        let step_t1 = new_interval.max();

        // After these calls, the conversion may not be reciprocal.
        let nurbs_t0 = self
            .polar
            .convert_to_nurbs_parameter(k, self, step_t0, 0.0)?
            .0;
        let nurbs_t1 = self
            .polar
            .convert_to_nurbs_parameter(k, self, step_t1, 0.0)?
            .0;
        let new_nurbs_ivl = Extent1d::from_min_max(nurbs_t0, nurbs_t1);

        // save the input
        self.anal_domain = *new_interval;

        self.make_nurb_owned(k, owner)?;

        // MakeNurb() creates m_pNurb parameterized 0 -> 1 [B284] (status ignored)
        ignore_status(self.edit_parameterization(k, &new_nurbs_ivl, true, owner))?;

        self.notify(k, NotifyOperation::PostEdit)?;

        // all done
        Ok(())
    }

    /// The points of the ellipse solving `operation` for `test_point`, as STEP angles
    /// (`GlobalPointSolveSTEP`): `SmCurve::GlobalPointSolve` over the natural interval
    /// ([`Curve::global_point_solve`] on a copy of the ellipse), each solution
    /// parameter then mapped to the STEP domain end (NURB ends within `SM_EFF_ZERO`,
    /// swapped when inside out) or inverted (`STEPInversion`). The solutions name no
    /// object.
    ///
    /// # Errors
    ///
    /// Those of the global solve and the evaluations.
    pub fn global_point_solve_step(
        &self,
        k: &Kernel,
        operation: SolverOperation,
        test_point: Point3,
        distance_tol: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
    ) -> Result<Vec<Solution<ObjectRef>>> {
        // ellipse locals (the Z axis is unused upstream)
        let _z_axis = self.position.z_axis();

        // Drop the point using NURBS-based solver
        let nurb_ivl = self.base.natural_interval();
        let curve = Curve::from(self.clone());
        let opts = GlobalPointSolveOptions {
            target_distance,
            handle: Some(SelfHandle(nurb_ivl)),
            ..GlobalPointSolveOptions::default()
        };
        let sols = curve.global_point_solve(
            k,
            &nurb_ivl,
            operation,
            test_point,
            distance_tol,
            requested,
            &opts,
        )?;

        // Convert NURBS solutions to STEP solutions
        let mut out = Vec::with_capacity(sols.len());
        for sol in sols.as_slice() {
            let mut r: Solution<ObjectRef> = Solution::new();
            r.solution_type = sol.solution_type;
            r.num_variables = sol.num_variables;
            r.start = sol.start;
            r.end = sol.end;
            r.num_objects = sol.num_objects; // (objects: this curve, not named here)
            r.nodes = sol.nodes;

            let t = r.start.parameters[0];
            let point_on_curve = self.base.evaluate_point(t)?;

            // First take care of start and end equivalents
            if (nurb_ivl.min() - t).abs() < EFF_ZERO {
                r.start.parameters[0] = if self.inside_out {
                    self.anal_domain.max()
                } else {
                    self.anal_domain.min()
                };
            } else if (nurb_ivl.max() - t).abs() < EFF_ZERO {
                r.start.parameters[0] = if self.inside_out {
                    self.anal_domain.min()
                } else {
                    self.anal_domain.max()
                };
            } else {
                r.start.parameters[0] = self.step_inversion(k, point_on_curve, false, 0.0).0;
            }
            out.push(r);
        }

        // all done
        Ok(out)
    }

    /// Joins `other` onto this ellipse at this curve's start (`join_end_this == 0`) or
    /// end (`1`) and `other`'s start or end (NURB ends; `JoinWith`). `other` must be an
    /// ellipse of the same geometry (radii, center, normal up to sign, and for a
    /// non-circle X and Y axes up to sign, all within the gap tolerance: `gap_tol` or
    /// `max(rx, ry) * 1000 sqrt(SM_EFF_ZERO)`) whose join point is within the
    /// tolerance of this one's. The STEP domain grows to `other`'s far end (the full
    /// ellipse when that end falls inside this domain) and the NURB is rebuilt
    /// (status ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `other` is not an ellipse or the geometry checks fail.
    pub fn join_with(
        &mut self,
        k: &Kernel,
        join_end_this: u64,
        other: &Curve,
        join_end_other: u64,
        gap_tol: Option<f64>,
        owner: Option<ObjectRef>,
    ) -> Result<()> {
        let err = || SolidError::sm_err(OP_JOIN);

        // the other curve must actually be an SmEllipse
        let CurveKind::Ellipse(other_ell) = other.kind() else {
            return Err(err());
        };

        // Get geometric info.
        let this_c = self.canonical();
        let that_c = other_ell.canonical();
        let (placement_this, rad1_this, rad2_this) =
            (this_c.position, this_c.radius_x, this_c.radius_y);
        let (placement_that, rad1_that, rad2_that) =
            (that_c.position, that_c.radius_x, that_c.radius_y);

        // Get a tolerance to use.
        let tol =
            gap_tol.unwrap_or(max2(self.x_radius(), self.y_radius()) * EFF_ZERO_SQRT * 1000.0);

        // Same curve? Geometry checks:
        if (rad1_this - rad1_that).abs() > tol {
            return Err(err());
        }
        if (rad2_this - rad2_that).abs() > tol {
            return Err(err());
        }

        if !placement_this
            .origin()
            .closer_than(tol, placement_that.origin())
        {
            return Err(err());
        }

        // We do need the same normals though, plus or minus.
        let mut cross = placement_this.z_axis().cross(placement_that.z_axis());
        if cross.length() > tol {
            return Err(err());
        }

        // If circular, then we're good to go, otherwise x and y axes must be compatible.
        if (rad2_this - rad1_this).abs() > tol {
            cross = placement_this.x_axis().cross(placement_that.x_axis());
            if cross.length() > tol {
                return Err(err());
            }
            cross = placement_this.y_axis().cross(placement_that.y_axis());
            if cross.length() > tol {
                return Err(err());
            }
        }

        // Make sure that the points of join ends are close enough (GetEnds, status
        // ignored).
        let ends = |c: &BSplineCurve| -> Result<(Point3, Point3)> {
            Ok(ignored(c.ends())?.map_or((Vec3::UNDEF, Vec3::UNDEF), |e| (e.start, e.end)))
        };
        let (this_start, this_stop) = ends(&self.base)?;
        let (other_start, other_stop) = ends(&other_ell.base)?;

        // Sort out which are the common point vs. the new end points.
        let this_common = if join_end_this == 1 {
            this_stop
        } else {
            this_start
        };
        let (other_common, other_end) = if join_end_other == 1 {
            (other_stop, other_start)
        } else {
            (other_start, other_stop)
        };

        // Check join-point distance.
        if this_common.distance(other_common) > tol {
            return Err(err());
        }

        // Passed all tests, good to go.  Modify ourself.
        let orig_domain = self.anal_domain;

        let inside_out_flag = u64::from(self.inside_out);
        if join_end_this == inside_out_flag {
            let v = self.anal_domain.max() - 360.0;
            self.anal_domain.add_value(v);
        } else {
            let v = self.anal_domain.min() + 360.0;
            self.anal_domain.add_value(v);
        }

        let new_ellipse_angle = self.step_inversion(k, other_end, false, 0.0).0;

        // change it only if the dropped param is outside the original domain
        if !orig_domain.contains_value(new_ellipse_angle, EFF_ZERO) {
            self.anal_domain = orig_domain;
            self.anal_domain.add_value(new_ellipse_angle);
        }

        // And update the Nurbs (statuses ignored).
        ignore_status(self.make_nurb_owned(k, owner))?;
        self.notify(k, NotifyOperation::PostEdit)?;

        Ok(())
    }

    /// Reverses the direction of the curve (`ReverseParameterization`): a missing NURB
    /// is made, the Y axis is negated (`SetCanonical`, status ignored), the STEP domain
    /// `[m, M]` becomes `[360 - M, 360 - m]` (status ignored) and the NURB is reversed
    /// (`SmBSplineCurve::ReverseParameterization`, rebuilding the polar converter).
    /// Returns `old_interval` in the reversed parameterisation.
    ///
    /// # Errors
    ///
    /// Those of [`Self::make_nurb_owned`] and [`BSplineCurve::reverse_parameterization`].
    pub fn reverse_parameterization(
        &mut self,
        k: &Kernel,
        old_interval: &Extent1d,
        owner: Option<ObjectRef>,
    ) -> Result<Extent1d> {
        if self.base.nurb.is_none() {
            self.make_nurb_owned(k, owner)?;
        }
        let (origin, x_axis, y_axis) = self.position.canonical();
        let _ = self.position.set_canonical(origin, x_axis, -y_axis);
        let (min, max) = (self.anal_domain.min(), self.anal_domain.max());
        let _ = self.anal_domain.set_min_max(360.0 - max, 360.0 - min);
        let new_interval = self.base.reverse_parameterization(old_interval)?;
        self.notify(k, NotifyOperation::PostEdit)?;
        Ok(new_interval)
    }

    /// Trims the ellipse to the NURB interval `trim_interval` (`Trim`): nothing when the
    /// natural interval is inside it (`SM_EFF_ZERO`); else the interval is snapped
    /// (`SmBSplineCurve::Trim`, [`TrimMode::IntervalOnly`]; with that mode nothing
    /// more), its ends are converted to STEP angles (statuses ignored), the NURB is
    /// trimmed without notification, the STEP domain set to the angles (swapped when
    /// inside out; status ignored) and the polar converter rebuilt, and with
    /// [`TrimMode::Notify`] rebuilt again by the post-edit notification.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineCurve::trim`]; `InvalidInput` when an end cannot be converted
    /// (upstream: an uninitialised angle).
    pub fn trim(&mut self, k: &Kernel, trim_interval: &mut Extent1d, mode: TrimMode) -> Result<()> {
        self.trim_with_zone_tol(
            k,
            trim_interval,
            mode,
            crate::geom::tol::zone_tol_3d(k, crate::geom::tol::SmallTopoType::NormalSize),
        )
    }

    /// Trims with the caller-resolved owning edge's 3D zone tolerance, forwarding
    /// it to both base-curve trim passes without changing interval-only behavior.
    ///
    /// # Errors
    ///
    /// The same errors as [`Self::trim`].
    pub(crate) fn trim_with_zone_tol(
        &mut self,
        k: &Kernel,
        trim_interval: &mut Extent1d,
        mode: TrimMode,
        zone_tol_3d: f64,
    ) -> Result<()> {
        // no work - current natural interval is a subSet of targetInterval
        let nat_ivl = self.base.natural_interval();
        if nat_ivl.is_contained_by(trim_interval, EFF_ZERO) {
            return Ok(());
        }

        // First ask SmBSplineCurve::Trim to compute the actual trim interval.
        self.base
            .trim_with_zone_tol(k, trim_interval, TrimMode::IntervalOnly, zone_tol_3d)?;

        // all done when asked to stop here
        if mode == TrimMode::IntervalOnly {
            return Ok(());
        }

        // Convert Nurb Parameters into STEP parameters (statuses ignored)
        let to_step = |e: &Self, t: f64| -> Result<f64> {
            match e.convert_t_from_nurbs_to_step(k, t) {
                Ok(v) => Ok(v),
                Err(err @ SolidError::Unported { .. }) => Err(err),
                Err(_) => Err(SolidError::invalid_input(
                    "curve::ellipse::p1::Ellipse::trim",
                    "trim end not convertible (upstream: uninitialised)",
                )),
            }
        };
        let min_step = to_step(self, trim_interval.min())?;
        let max_step = to_step(self, trim_interval.max())?;

        // pass the call to Trim the BSpline - skip its Notify call
        self.base
            .trim_with_zone_tol(k, trim_interval, TrimMode::Silent, zone_tol_3d)?;

        // store the angular interval paying attention to m_bInsideOut
        if self.inside_out {
            let _ = self.anal_domain.set_min_max(max_step, min_step);
        } else {
            let _ = self.anal_domain.set_min_max(min_step, max_step);
        }

        // rebuild the polar converter
        ignore_status(self.set_up_polar_conversion(k))?;

        // all done
        if mode == TrimMode::Notify {
            self.notify(k, NotifyOperation::PostEdit)?;
        }
        Ok(())
    }
}

/// An empty NURB from NLib (upstream reads its first control point regardless).
const fn empty_nurb() -> SolidError {
    SolidError::InvalidInput {
        operation: OP_MAKE_NURB,
        reason: "NLib returned a curve without control points",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::p2::ELLIPSE_TYPE;

    fn kernel() -> Kernel {
        Kernel::default()
    }

    fn circle(k: &Kernel, d0: f64, d1: f64, r: f64) -> Ellipse {
        Ellipse::new_circle(
            k,
            Point3::new(1.0, 2.0, 3.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Extent1d::from_min_max(d0, d1),
            r,
            &EllipseOptions::default(),
        )
        .unwrap()
    }

    /// #428 (`WR-211`), as upstream: the polar converter clamps an angle outside the
    /// current arc to its ends, so an arc moved a turn on (here to `[360, 540]`, past the
    /// documented `Max <= 360`) gets both ends at one NURBS parameter. The next
    /// reparameterisation then divides by that zero length: interior knots NaN.
    #[test]
    fn step_interval_past_a_turn_collapses_as_upstream() {
        let k = kernel();
        let past = Extent1d::from_min_max(360.0, 540.0);
        for d1 in [360.0, 180.0] {
            let mut c = circle(&k, 0.0, d1, 2.0);
            c.adjust_step_interval(&k, &past, None).unwrap();
            assert_eq!(c.anal_domain, past);
            let nat = c.base.natural_interval();
            assert_eq!(nat.min(), nat.max(), "[0, {d1}]");
        }
        let mut c = circle(&k, 0.0, 360.0, 2.0);
        c.adjust_step_interval(&k, &past, None).unwrap();
        c.base.edit_parameterization(&past).unwrap();
        let (knots, mults) = c.base.knots(None).unwrap();
        assert_eq!(mults, [3, 1, 1, 3]);
        assert_eq!([knots[0], knots[3]], [360.0, 540.0]);
        assert!(knots[1].is_nan() && knots[2].is_nan());
    }

    #[test]
    fn circle_class_and_types() {
        let k = kernel();
        let c = circle(&k, 0.0, 90.0, 2.0);
        assert!(c.is_circle_class());
        assert_eq!(c.type_code(), CIRCLE_TYPE);
        assert!(c.kind_of(CIRCLE_TYPE));
        assert!(c.kind_of(ELLIPSE_TYPE));
        assert_eq!(c.class_string(), "SmCircle");
        assert_eq!(c.base().form, BSplineCurveForm::CircularArc);
        assert_eq!(c.radius(), 2.0);
        let (_, r, a, b) = c.is_arc(&k, 5, 0.0).unwrap().unwrap();
        assert_eq!((r, a, b), (2.0, 0.0, 90.0));
    }

    #[test]
    fn polar_round_trip_and_validity() {
        let k = kernel();
        let mut c = circle(&k, 0.0, 360.0, 2.0);
        let ivl = c.base().natural_interval();
        let s = c
            .convert_t_from_nurbs_to_step(&k, ivl.evaluate(0.3))
            .unwrap();
        let t = c.convert_t_from_step_to_nurbs(&k, s).unwrap();
        assert!((t - ivl.evaluate(0.3)).abs() < 1e-9);
        let mut l = AssertArray::new();
        assert!(c
            .assert_valid(&k, Some(&mut l), false, ObjectKey(1), ObjectKey(2))
            .unwrap());
        assert!(l.is_empty());
    }

    #[test]
    fn three_point_circle_and_copy() {
        let k = kernel();
        let c = Ellipse::circle_from_3_points(
            &k,
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(-1.0, 0.0, 0.0),
            3,
            false,
        )
        .unwrap();
        assert!((c.radius() - 1.0).abs() < 1e-12);
        let d = c.copy(&k).unwrap();
        assert!(d.approx_eq(&c));
        assert!(d.is_circle_class());
    }

    #[test]
    fn reverse_and_trim() {
        let k = kernel();
        let mut c = circle(&k, 10.0, 100.0, 1.0);
        let ivl = c.base().natural_interval();
        let r = c.reverse_parameterization(&k, &ivl, None).unwrap();
        assert!(r.are_same(&ivl));
        assert!((c.step_interval().min() - 260.0).abs() < 1e-9);
        let mut t = Extent1d::from_min_max(ivl.evaluate(0.25), ivl.evaluate(0.75));
        c.trim(&k, &mut t, TrimMode::Notify).unwrap();
        assert!(c.polar_converter().is_polar_conversion_possible());
    }

    /// #1432: a full unit circle given as its rational quadratic NURBS (9 points,
    /// weights `1, s, 1, s, ..`, a plain B-spline) is recognised as that circle.
    #[test]
    fn is_nurb_curve_circle_full_unit_circle() {
        let k = kernel();
        let circle =
            Ellipse::create_canonical_circle(&k, &Axis2Placement::default(), 1.0, None, None)
                .unwrap();
        let nurbs = Curve::from(circle.base().copy());
        assert!(matches!(nurbs.kind(), CurveKind::BSpline(_)));
        let got = Ellipse::is_nurb_curve_circle(&k, &nurbs, None)
            .unwrap()
            .expect("a circle");
        assert!(got.is_circle_class());
        assert!((got.radius() - 1.0).abs() < 1e-12);
        let dom = got.step_interval();
        assert!((dom.max() - dom.min() - 360.0).abs() < 1e-9);
        assert!(got.base().gw_nurb().is_some());

        // already a circle: a copy
        let same = Ellipse::is_nurb_curve_circle(&k, &Curve::from(circle.copy(&k).unwrap()), None)
            .unwrap()
            .expect("a copy");
        assert!(same.is_circle_class());
        assert_eq!(same.radius(), 1.0);

        // a non-circular ellipse is rational degree 2 but not an arc
        let ell = Ellipse::create_canonical(&k, &Axis2Placement::default(), 2.0, 1.0, None, None)
            .unwrap();
        let none = Ellipse::is_nurb_curve_circle(&k, &Curve::from(ell.base().copy()), None);
        assert!(none.unwrap().is_none());
    }

    #[test]
    fn simple_offset_and_join() {
        let k = kernel();
        let c = circle(&k, 0.0, 90.0, 1.0);
        let (o, gap) = c
            .create_simple_offset(&k, 1e-6, Vec3::new(0.0, 0.0, 1.0), 0.5, false)
            .unwrap();
        assert_eq!(gap, 0.0);
        match o.kind() {
            CurveKind::Ellipse(e) => assert_eq!(e.radius(), 1.5),
            _ => panic!("not a circle"),
        }
        let mut a = circle(&k, 0.0, 90.0, 1.0);
        let b = Curve::from(circle(&k, 90.0, 180.0, 1.0));
        a.join_with(&k, 1, &b, 0, None, None).unwrap();
        assert!((a.step_interval().max() - 180.0).abs() < 1e-6);
        let line = Curve::from(BSplineCurve::create_degenerate_curve(3, Vec3::UNDEF).unwrap());
        assert!(a.join_with(&k, 1, &line, 0, None, None).is_err());
    }

    #[test]
    fn two_d_conversion() {
        let k = kernel();
        let e = Ellipse::new(
            &k,
            Point3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Extent1d::from_min_max(0.0, 180.0),
            2.0,
            1.0,
            &EllipseOptions {
                dim: 2,
                ..EllipseOptions::default()
            },
        )
        .unwrap();
        assert_eq!(e.position().origin().z, 0.0);
        assert_eq!(e.base().dim, 2);
    }
}

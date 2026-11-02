//! `AssertValid` bookkeeping: one failed-check report ([`AssertReport`]) with its
//! static label, and the duplicate-free report list ([`AssertArray`]) the
//! `SM_ASSERT_*_REPORT` macros fill.
//!
//! Upstream: `source/SMLib/inc/SmAssertArray.h`, `source/SMLib/src/SmAssertArray.cpp`.
//!
//! | Rust                                   | Upstream                                         |
//! |----------------------------------------|--------------------------------------------------|
//! | [`AssertType`]                         | `SmAssertType`                                   |
//! | [`AssertReportLabel`]                  | `SmAssertReportLabel`                            |
//! | [`labels`] (`labels::CIRCLE`, ...)     | the `extern SmAssertReportLabel s*_list[]` tables |
//! | [`assert_label_list`]                  | `SM_GetAssertLabelList`                          |
//! | [`ObjectKey`]                          | the `const void *` owner / other / reporter pointers |
//! | [`AssertReport`]                       | `SmAssertReport`                                 |
//! | [`ReportSpec`]                         | the arguments of `SmAssertReport::SmAssertReport` |
//! | [`AssertReport::new`]                  | `SmAssertReport::SmAssertReport`                 |
//! | `PartialEq for AssertReport`           | `SmAssertReport::operator==`                     |
//! | [`AssertReport::set_value`]            | `SmAssertReport::SetValue`                       |
//! | [`AssertReport::set_owner_param`]      | `SmAssertReport::SetOwnerParam`                  |
//! | (`Drop`)                               | `SmAssertReport::~SmAssertReport`                |
//! | [`AssertArray`]                        | `SmAssertArray`                                  |
//! | [`AssertArray::contains`]              | `SmAssertArray::Contains`                        |
//! | [`AssertArray::add_unique_delete_duplicate`] | `SmAssertArray::AddUniqueDeleteDuplicate`  |
//! | [`AssertArray::last_add`]              | `SmAssertArray::GetLastAdd`                      |
//! | [`AssertArray::set_last_add`]          | `SmAssertArray::SetLastAdd`                      |
//! | [`AssertArray::len`], [`AssertArray::get`], [`AssertArray::reports`] | `SmTArray<SmAssertReport*>::GetSize`, `GetAt`, `GetReports` |
//! | [`AssertReport::format_log_message`]   | `SmAssertReport::FormatLogMessage`               |
//! | [`obj_type_string`]                    | `SM_GetObjTypeString`                            |
//! | `c_general` (private)                  | `smos_sprintf("%W.Plg")`                         |
//!
//! Deviations:
//! - Object pointers become [`ObjectKey`]s (`None` = `NULL`); reports compare them by
//!   value exactly as upstream compares pointers. Type codes stay plain `i64`
//!   (`SM_TYPE`), list and test indices `u64` (`ULONG`).
//! - The label tables, declared `extern` in `SmAssertArray.h` but defined beside each
//!   `AssertValid` method, are all ported here (module [`labels`]) so that the units
//!   porting those methods share one copy.
//! - `SmAssertReport::SmAssertReport` indexes the label table without a bounds check;
//!   [`AssertReport::new`] returns [`SolidError::InvalidInput`] for a test index past
//!   the end of the table instead of reading out of bounds.
//! - The optional message is an owned string ([`Cow`]) instead of a borrowed pointer.
//! - `AddUniqueDeleteDuplicate(NULL)` (never produced by the macros) is not
//!   representable: the report is passed by value. A duplicate report is dropped,
//!   as upstream deletes it.
//! - `SM_DEBUG_CODE` members (`m_file_name`, `m_line_num`, the debug drawing in the
//!   constructor) are not ported; the reference build does not define it.
//! - `FormatLogMessage` returns an owned string instead of filling a caller buffer
//!   (no truncation); `%lg` is formatted as glibc does (`inf`, `nan`).
//! - Report membership uses a call-owned ordered index over exactly the fields
//!   compared by `operator==`; the report vector retains insertion order. This
//!   avoids quadratic duplicate searches when validating a large invalid B-rep.

pub mod labels;

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::error::{Result, SolidError};
use crate::geom::vector2d::Point2;
use crate::math::UNDEF_DOUBLE;

/// Category of an assert report (`SmAssertType`), used to group failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AssertType {
    /// `SM_AT_UNKNOWN`: an uninitialized value.
    #[default]
    Unknown,
    /// `SM_AT_ANGLE`: an angle is less than a tolerance.
    Angle,
    /// `SM_AT_BOX`: box check.
    Box,
    /// `SM_AT_CACHE`: data consistency.
    Cache,
    /// `SM_AT_CLOSED`: closedness.
    Closed,
    /// `SM_AT_COINCIDENCE`: coincidence.
    Coincidence,
    /// `SM_AT_DEGENERATE`: degeneracy.
    Degenerate,
    /// `SM_AT_DIRECTION`: direction.
    Direction,
    /// `SM_AT_DISTANCE`: distance.
    Distance,
    /// `SM_AT_DOMAIN`: domain.
    Domain,
    /// `SM_AT_FLAG`: flags are set correctly.
    Flag,
    /// `SM_AT_GEOMETRIC`: a geometric property.
    Geometric,
    /// `SM_AT_TOPOLOGICAL`: a topology graph property.
    Topological,
    /// `SM_AT_GAP`: a gap is less than a tolerance.
    Gap,
    /// `SM_AT_INSIDE`: insideness.
    Inside,
    /// `SM_AT_KNOTS`: number of knots.
    Knots,
    /// `SM_AT_LIST`: list membership.
    List,
    /// `SM_AT_MARK`: marks.
    Mark,
    /// `SM_AT_MINMAX`: min is less than max.
    Minmax,
    /// `SM_AT_PARAMETERIZATION`: parameterization issues.
    Parameterization,
    /// `SM_AT_POINTER`: pointers.
    Pointer,
    /// `SM_AT_POINTS`: points are within tolerance.
    Points,
    /// `SM_AT_RADIUS`: x and y radius agree.
    Radius,
    /// `SM_AT_SCALE`: scale.
    Scale,
    /// `SM_AT_SIZE`: size.
    Size,
    /// `SM_AT_TYPE`: data types are correct.
    Type,
    /// `SM_AT_VALUES`: values.
    Values,
    /// `SM_AT_VECTOR`: a vector has length.
    Vector,
    /// `SM_AT_UNIT_VECTOR`: a vector is unit length.
    UnitVector,
    /// `SM_AT_POLE`: poles.
    Pole,
    /// `SM_AT_NESTED_TEST`: the object must pass a nested test.
    NestedTest,
    /// `SM_AT_NESTED_PROPERTY`: the object must have a nested property.
    NestedProperty,
}

impl AssertType {
    /// The upstream enumerator value (`SM_AT_UNKNOWN` = 0, in declaration order).
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// One static label of an `AssertValid` check (`SmAssertReportLabel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssertReportLabel {
    /// Category of the check (`m_eAssertType`).
    pub assert_type: AssertType,
    /// Short identifier (`m_pName`).
    pub name: &'static str,
    /// Detailed message (`m_pMessage`; `None` = `NULL`).
    pub message: Option<&'static str>,
    /// Message without values, for log files (`m_pMessageForFile`; `None` = `NULL`).
    pub message_for_file: Option<&'static str>,
}

/// `SM_GetAssertLabelList`: the label table for checks run by an object of type
/// `type_code`; `list_index` picks one of several tables (only `SmPolarConversion`,
/// `SmBrep` and `SmTrimmingTools` have more than one). Unknown types and list indices
/// give [`labels::DEFAULT`].
#[must_use]
pub fn assert_label_list(type_code: i64, list_index: u64) -> &'static [AssertReportLabel] {
    labels::label_list(type_code, list_index)
}

/// Identity of an object named by a report (upstream compares raw object pointers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey(pub u64);

/// The inputs of one report (`SmAssertReport::SmAssertReport` arguments).
#[derive(Debug, Clone, PartialEq)]
pub struct ReportSpec {
    /// Object whose `AssertValid` ran the check (`pReporter`).
    pub reporter: Option<ObjectKey>,
    /// Index of the check in its label table (`lTestIndex`).
    pub test_index: u64,
    /// Type of the class running the check, selects the label table (`lTestClass`).
    pub test_class: i64,
    /// Object the check is about (`pOwner`).
    pub owner: Option<ObjectKey>,
    /// Type code of the owner (`lOwnerType`).
    pub owner_type: i64,
    /// Type name of the owner (`sOwnerTypeString`).
    pub owner_type_string: Option<&'static str>,
    /// Optional second object, e.g. for gap checks (`pOther`).
    pub other: Option<ObjectKey>,
    /// Type code of the other object (`lOtherType`).
    pub other_type: i64,
    /// Type name of the other object (`sOtherTypeString`).
    pub other_type_string: Option<&'static str>,
    /// Tolerance used by the check (`sTol3d`, already a tolerance value: see
    /// [`tol3d`](crate::geom::tol::tol3d)); [`UNDEF_DOUBLE`] = none.
    pub tol3d: f64,
    /// Value checked against the tolerance (`dVal`); [`UNDEF_DOUBLE`] = none.
    pub value: f64,
    /// Which label table of `test_class` (`lListIndex`).
    pub list_index: u64,
    /// Message replacing the label's (`pOptMessage`; `None` or empty = use the label).
    pub message: Option<Cow<'static, str>>,
}

impl Default for ReportSpec {
    fn default() -> Self {
        Self {
            reporter: None,
            test_index: 0,
            test_class: 0,
            owner: None,
            owner_type: 0,
            owner_type_string: None,
            other: None,
            other_type: 0,
            other_type_string: None,
            tol3d: UNDEF_DOUBLE,
            value: UNDEF_DOUBLE,
            list_index: 0,
            message: None,
        }
    }
}

/// One failed `AssertValid` check (`SmAssertReport`).
#[derive(Debug, Clone)]
pub struct AssertReport {
    /// `true` = the check passed (`m_bOK`; always `false` for a new report).
    pub ok: bool,
    /// Object whose `AssertValid` produced the report (`m_pReporter`).
    pub reporter: Option<ObjectKey>,
    /// Type of the reporting class (`m_lReportingType`).
    pub reporting_type: i64,
    /// Label table index of the reporting type (`m_lListIndex`).
    pub list_index: u64,
    /// Index of the check in its label table (`m_lTestIndex`).
    pub test_index: u64,
    /// Category of the check (`m_eAssertType`).
    pub assert_type: AssertType,
    /// Name of the check (`m_pName`).
    pub name: &'static str,
    /// Description of the check (`m_pMessage`; `None` = `NULL`).
    pub message: Option<Cow<'static, str>>,
    /// Description without values (`m_pMessageForFile`; `None` = `NULL`).
    pub message_for_file: Option<&'static str>,
    /// Object the check is about (`m_pOwner`).
    pub owner: Option<ObjectKey>,
    /// Type code of the owner (`m_lOwnerType`).
    pub owner_type: i64,
    /// Type name of the owner (`m_sOwnerTypeString`).
    pub owner_type_string: Option<&'static str>,
    /// Optional second object (`m_pOther`).
    pub other: Option<ObjectKey>,
    /// Type code of the other object (`m_lOtherType`).
    pub other_type: i64,
    /// Type name of the other object (`m_sOtherTypeString`).
    pub other_type_string: Option<&'static str>,
    /// Tolerance of the check (`m_sTol3d`).
    pub tol3d: f64,
    /// Checked value (`m_dValue`).
    pub value: f64,
    /// Parameter on the owner (`m_sOwnerParam`; uninitialized when new).
    pub owner_param: Point2,
    /// Parameter on the other object (`m_sOtherParam`; uninitialized when new).
    pub other_param: Point2,
    /// 3D distance (`m_dDist3d`; [`UNDEF_DOUBLE`] when new).
    pub dist3d: f64,
}

impl AssertReport {
    /// `SmAssertReport::SmAssertReport`: a failed report for check `spec.test_index`
    /// of the label table `assert_label_list(spec.test_class, spec.list_index)`.
    ///
    /// # Errors
    /// [`SolidError::InvalidInput`] when `spec.test_index` is past the end of the table
    /// (upstream reads out of bounds).
    pub fn new(spec: ReportSpec) -> Result<Self> {
        let list = assert_label_list(spec.test_class, spec.list_index);
        Self::with_labels(spec, list)
    }

    /// [`AssertReport::new`] with check `spec.test_index` taken from `list` instead of
    /// the class's table (not upstream: a check an opt-in fix adds to a class, such as
    /// [`labels::TORUS_WITH_SPINDLE_APEX`], reports from its own extended table, so
    /// `SM_GetAssertLabelList` keeps upstream's).
    ///
    /// # Errors
    /// [`SolidError::InvalidInput`] when `spec.test_index` is past the end of `list`.
    pub fn with_labels(spec: ReportSpec, list: &'static [AssertReportLabel]) -> Result<Self> {
        let label = usize::try_from(spec.test_index)
            .ok()
            .and_then(|i| list.get(i))
            .ok_or(SolidError::InvalidInput {
                operation: "geom::assert_array::AssertReport::new",
                reason: "test index is past the end of the assert label list",
            })?;
        let message = match spec.message {
            Some(m) if !m.is_empty() => Some(m),
            _ => label.message.map(Cow::Borrowed),
        };
        let mut owner_param = Point2::default();
        owner_param.set_uninitialized();
        let mut other_param = Point2::default();
        other_param.set_uninitialized();
        Ok(Self {
            ok: false,
            reporter: spec.reporter,
            reporting_type: spec.test_class,
            list_index: spec.list_index,
            test_index: spec.test_index,
            assert_type: label.assert_type,
            name: label.name,
            message,
            message_for_file: label.message_for_file,
            owner: spec.owner,
            owner_type: spec.owner_type,
            owner_type_string: spec.owner_type_string,
            other: spec.other,
            other_type: spec.other_type,
            other_type_string: spec.other_type_string,
            tol3d: spec.tol3d,
            value: spec.value,
            owner_param,
            other_param,
            dist3d: UNDEF_DOUBLE,
        })
    }

    /// `SmAssertReport::SetValue`.
    pub fn set_value(&mut self, value: f64) {
        self.value = value;
    }

    /// `SmAssertReport::SetOwnerParam`.
    pub fn set_owner_param(&mut self, owner_param: Point2) {
        self.owner_param = owner_param;
    }

    /// `SmAssertReport::FormatLogMessage`: the message (`"?"` without one) with the
    /// stored value and tolerance when defined:
    /// `"%s (offset=%6.8lg, tolerance=%6.8lg)"`, `"%s (offset=%6.8lg)"` or
    /// `"%s (tolerance=%6.8lg)"`.
    #[must_use]
    pub fn format_log_message(&self) -> String {
        let base = self.message.as_deref().unwrap_or("?");
        let has_tol = self.tol3d != UNDEF_DOUBLE;
        let has_val = self.value != UNDEF_DOUBLE;
        match (has_tol, has_val) {
            (true, true) => format!(
                "{base} (offset={}, tolerance={})",
                c_general(self.value, 6, 8),
                c_general(self.tol3d, 6, 8)
            ),
            (false, true) => format!("{base} (offset={})", c_general(self.value, 6, 8)),
            (true, false) => format!("{base} (tolerance={})", c_general(self.tol3d, 6, 8)),
            (false, false) => base.to_string(),
        }
    }
}

/// `printf("%W.Plg", x)` as glibc formats it: the shorter of fixed and exponent
/// notation for `precision` significant digits (exponent form when the decimal
/// exponent is below -4 or at least `precision`), trailing zeros removed, right-aligned
/// in `width`.
fn c_general(x: f64, width: usize, precision: usize) -> String {
    let p = precision.max(1);
    let body = if x.is_nan() {
        if x.is_sign_negative() { "-nan" } else { "nan" }.to_string()
    } else if x.is_infinite() {
        if x < 0.0 { "-inf" } else { "inf" }.to_string()
    } else {
        // the decimal exponent after rounding to p significant digits
        let sci = format!("{:.*e}", p - 1, x);
        let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
        let exp: i64 = exponent.parse().unwrap_or(0);
        let p_i = i64::try_from(p).unwrap_or(i64::MAX);
        if exp < -4 || exp >= p_i {
            let mantissa = strip_zeros(mantissa);
            let sign = if exp < 0 { '-' } else { '+' };
            format!("{mantissa}e{sign}{:02}", exp.unsigned_abs())
        } else {
            let decimals = usize::try_from(p_i - 1 - exp).unwrap_or(0);
            strip_zeros(&format!("{x:.decimals$}")).to_string()
        }
    };
    format!("{body:>width$}")
}

/// Removes trailing fractional zeros and a trailing decimal point (`%g` without `#`).
fn strip_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// `SM_GetObjTypeString`: the class name of an `AssertValid` reporting type, or
/// `"SmDefault"` for a type it does not list.
#[must_use]
pub fn obj_type_string(type_code: i64) -> &'static str {
    use labels as l;
    match type_code {
        l::AXIS2_PLACEMENT_TYPE => "SmAxis2Placement",
        l::POLAR_CONVERSION_TYPE => "SmPolarConversion",
        l::CIRCLE_TYPE => "SmCircle",
        l::ELLIPSE_TYPE => "SmEllipse",
        l::ATTRIBUTE_TYPE => "SmAttribute",
        l::B_SPLINE_CURVE_TYPE => "SmBSplineCurve",
        l::CURVE_TYPE => "SmCurve",
        l::A_OBJECT_TYPE => "SmAObject",
        l::LINE_TYPE => "SmLine",
        l::TREE_TYPE => "SmTree",
        l::SOLUTION_ARRAY_TYPE => "SmSolutionArray",
        l::B_SPLINE_SURFACE_TYPE => "SmBSplineSurface",
        l::PLANE_TYPE => "SmPlane",
        l::CONE_TYPE => "SmCone",
        l::SPHERE_TYPE => "SmSphere",
        l::CYLINDER_TYPE => "SmCylinder",
        l::TORUS_TYPE => "SmTorus",
        l::SURF_OF_REVOLUTION_TYPE => "SmSurfOfRevolution",
        l::EXTENT1D_TYPE => "SmExtent1d",
        l::EXTENT2D_TYPE => "SmExtent2d",
        l::EXTENT3D_TYPE => "SmExtent3d",
        l::PERIODIC_EXTENT1D_TYPE => "SmPeriodicExtent1d",
        l::POLAR_BOX_TYPE => "SmPolarBox",
        l::PSEUDO_BOX_TYPE => "SmPseudoBox",
        l::DERIV_SURF_DEFINITION_TYPE => "SmDerivSurfDefinition",
        l::SURFACE_CACHE_TYPE => "SmSurfaceCache",
        l::SURF_OF_EXTRUSION_TYPE => "SmSurfOfExtrusion",
        l::SURFACE_TYPE => "SmSurface",
        l::CRV_ON_SURF_TYPE => "SmCrvOnSurf",
        l::PROJECTED_CURVE_TYPE => "SmProjectedCurve",
        l::CRV_IN_VOLUME_TYPE => "SmCrvInVolume",
        l::SRF_IN_VOLUME_TYPE => "SmSrfInVolume",
        l::FILLET_EDGE_TYPE => "SmFilletEdge",
        l::FILLET_BREP_TYPE => "SmFilletBrep",
        l::FILLET_VERTEX_TYPE => "SmFilletVertex",
        l::FILLET_VERTEXUSE_TYPE => "SmFilletVertexuse",
        l::FILLET_EDGEUSE_TYPE => "SmFilletEdgeuse",
        l::SPACE_BEND_TYPE => "SmSpaceBend",
        l::SPACE_UNBEND_TYPE => "SmSpaceUnbend",
        l::OFFSET_SURFACE_TYPE => "SmOffsetSurface",
        l::OFFSET_CURVE_TYPE => "SmOffsetCurve",
        l::ASSEMBLY_TYPE => "SmAssembly",
        l::ASSEMBLY_INSTANCE_TYPE => "SmAssemblyInstance",
        l::SAG_OBJECT_TYPE => "SmSAGObject",
        l::BREP_TYPE => "SmBrep",
        l::VOLUME_TYPE => "SmVolume",
        l::BEND_VOLUME_TYPE => "SmBendVolume",
        l::UNBEND_VOLUME_TYPE => "SmUnbendVolume",
        l::TRANSFORM_TYPE => "SmTransform",
        l::B_SPLINE_VOLUME_TYPE => "SmBSplineVolume",
        l::TWIST_VOLUME_TYPE => "SmTwistVolume",
        l::EDGE_TYPE => "SmEdge",
        l::FACE_TYPE => "SmFace",
        l::CURVE_CLASSIFICATION_TYPE => "SmCurveClassification",
        l::CURVE_INTERVAL_TYPE => "SmCurveInterval",
        l::POINT_CLASSIFICATION_TYPE => "SmPointClassification",
        l::TOPOLOGY_TYPE => "SmTopology",
        l::OWNING_TOPOLOGY_TYPE => "SmOwningTopology",
        l::EDGEUSE_TYPE => "SmEdgeuse",
        l::FACEUSE_TYPE => "SmFaceuse",
        l::VERTEXUSE_TYPE => "SmVertexuse",
        l::VERTEX_TYPE => "SmVertex",
        l::LOOP_TYPE => "SmLoop",
        l::LOOPUSE_TYPE => "SmLoopuse",
        l::REGION_TYPE => "SmRegion",
        l::SHELL_TYPE => "SmShell",
        l::POLY_FACE_TYPE => "SmPolyFace",
        l::C_POLY_FACE_TYPE => "SmCPolyFace",
        l::POLY_LOOP_TYPE => "SmPolyLoop",
        l::POLY_VERTEX_TYPE => "SmPolyVertex",
        l::POLY_EDGE_TYPE => "SmPolyEdge",
        l::POLY_REGION_TYPE => "SmPolyRegion",
        l::POLY_SHELL_TYPE => "SmPolyShell",
        l::POLY_BREP_TYPE => "SmPolyBrep",
        l::TRIMMING_TOOLS_TYPE => "SmTrimmingTools",
        _ => "SmDefault",
    }
}

/// `SmAssertReport::operator==`: same reporting type, list index, test index, category,
/// pass flag, owner and other object (tolerances, values and strings are ignored).
impl PartialEq for AssertReport {
    fn eq(&self, other: &Self) -> bool {
        self.reporting_type == other.reporting_type
            && self.list_index == other.list_index
            && self.test_index == other.test_index
            && self.assert_type == other.assert_type
            && self.ok == other.ok
            && self.owner == other.owner
            && self.other == other.other
    }
}

impl Eq for AssertReport {}

type ReportIdentity = (
    i64,
    u64,
    u64,
    AssertType,
    bool,
    Option<ObjectKey>,
    Option<ObjectKey>,
);

fn report_identity(report: &AssertReport) -> ReportIdentity {
    (
        report.reporting_type,
        report.list_index,
        report.test_index,
        report.assert_type,
        report.ok,
        report.owner,
        report.other,
    )
}

/// The reports of an `AssertValid` run (`SmAssertArray`), without duplicates when
/// filled through [`AssertArray::add_unique_delete_duplicate`].
#[derive(Debug, Clone, Default)]
pub struct AssertArray {
    reports: Vec<AssertReport>,
    identities: BTreeSet<ReportIdentity>,
    last_add: bool,
}

impl AssertArray {
    /// An empty array (`SmAssertArray()`; `m_bLastAdd` = `FALSE`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of reports (`GetSize`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.reports.len()
    }

    /// `true` when there are no reports.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }

    /// Report `index` (`GetAt`), `None` past the end.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&AssertReport> {
        self.reports.get(index)
    }

    /// Every report, in insertion order (`GetReports`).
    #[must_use]
    pub fn reports(&self) -> &[AssertReport] {
        &self.reports
    }

    /// `SmAssertArray::Contains`: `true` when a report equal to `report` (see
    /// `PartialEq for AssertReport`) is already in the array.
    #[must_use]
    pub fn contains(&self, report: &AssertReport) -> bool {
        self.identities.contains(&report_identity(report))
    }

    /// `SmAssertArray::AddUniqueDeleteDuplicate`: appends `report` unless an equal
    /// report is already present (then `report` is dropped). Returns `true` when it was
    /// added and records the same flag for [`AssertArray::last_add`].
    pub fn add_unique_delete_duplicate(&mut self, report: AssertReport) -> bool {
        self.last_add = false;
        if self.identities.insert(report_identity(&report)) {
            self.reports.push(report);
            self.last_add = true;
            true
        } else {
            false
        }
    }

    /// `SmAssertArray::GetLastAdd`: whether the last
    /// [`add_unique_delete_duplicate`](AssertArray::add_unique_delete_duplicate) added
    /// its report.
    #[must_use]
    pub fn last_add(&self) -> bool {
        self.last_add
    }

    /// `SmAssertArray::SetLastAdd`.
    pub fn set_last_add(&mut self, last_add: bool) {
        self.last_add = last_add;
    }

    /// `SmAssertArray::FindReport`: the index of the first report from `reporter` of
    /// check `test_index` of reporting type `test_class` about `owner` and `other`
    /// (`SmAssertReport::IsThisReport`), `None` when there is none.
    #[must_use]
    pub fn find_report(
        &self,
        reporter: Option<ObjectKey>,
        test_index: u64,
        test_class: i64,
        owner: Option<ObjectKey>,
        other: Option<ObjectKey>,
    ) -> Option<usize> {
        self.reports.iter().position(|r| {
            r.reporter == reporter
                && r.test_index == test_index
                && r.reporting_type == test_class
                && r.owner == owner
                && r.other == other
        })
    }

    /// `SmAssertArray::RemoveAt`: removes and returns report `index` (`None` past the
    /// end).
    pub fn remove_at(&mut self, index: usize) -> Option<AssertReport> {
        if index >= self.reports.len() {
            return None;
        }
        let r = self.reports.remove(index);
        self.identities.remove(&report_identity(&r));
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(test_class: i64, test_index: u64, owner: u64) -> ReportSpec {
        ReportSpec {
            reporter: Some(ObjectKey(owner)),
            test_index,
            test_class,
            owner: Some(ObjectKey(owner)),
            ..ReportSpec::default()
        }
    }

    #[test]
    fn label_lists_dispatch() {
        assert_eq!(assert_label_list(labels::CIRCLE_TYPE, 0), labels::CIRCLE);
        assert_eq!(assert_label_list(labels::CIRCLE_TYPE, 7), labels::CIRCLE);
        assert_eq!(
            assert_label_list(labels::BREP_TYPE, 3),
            labels::COINCIDENT_TOPOLOGY
        );
        assert_eq!(assert_label_list(labels::BREP_TYPE, 5), labels::DEFAULT);
        assert_eq!(
            assert_label_list(labels::POLAR_CONVERSION_TYPE, 1),
            labels::POLAR_IS_CURRENT
        );
        assert_eq!(assert_label_list(-1, 0), labels::DEFAULT);
        assert_eq!(labels::DEFAULT.len(), 1);
        for &(t, i) in labels::LABELLED {
            assert!(!assert_label_list(t, i).is_empty());
        }
    }

    #[test]
    fn new_report_from_label() {
        let r = AssertReport::new(spec(labels::CIRCLE_TYPE, 0, 5)).unwrap();
        assert!(!r.ok);
        assert_eq!(r.assert_type, AssertType::Radius);
        assert_eq!(r.name, "Radius");
        assert_eq!(
            r.message.as_deref(),
            Some("m_dRadiusAtXAxis and m_dRadiusAtYAxis are equal")
        );
        assert_eq!(r.message_for_file, None);
        assert_eq!(r.owner_param, Point2::UNDEF);
        assert_eq!(r.dist3d.to_bits(), UNDEF_DOUBLE.to_bits());
        assert_eq!(r.tol3d.to_bits(), UNDEF_DOUBLE.to_bits());
        assert!(AssertReport::new(spec(labels::CIRCLE_TYPE, 1, 5)).is_err());
    }

    #[test]
    fn optional_message_overrides_unless_empty() {
        let mut s = spec(labels::LINE_TYPE, 0, 1);
        s.message = Some(Cow::Borrowed(""));
        let r = AssertReport::new(s.clone()).unwrap();
        assert_eq!(
            r.message.as_deref(),
            labels::LINE.first().and_then(|l| l.message)
        );
        s.message = Some(Cow::Owned("custom".to_string()));
        assert_eq!(
            AssertReport::new(s).unwrap().message.as_deref(),
            Some("custom")
        );
    }

    #[test]
    fn poly_loop_label_without_message() {
        let i = labels::POLY_LOOP
            .iter()
            .position(|l| l.message.is_none())
            .unwrap();
        let r = AssertReport::new(spec(labels::POLY_LOOP_TYPE, i as u64, 1)).unwrap();
        assert_eq!(r.message, None);
    }

    #[test]
    fn general_format_matches_printf() {
        assert_eq!(c_general(0.0, 6, 8), "     0");
        assert_eq!(c_general(1e-5, 6, 8), " 1e-05");
        assert_eq!(c_general(1.0e-12, 6, 8), " 1e-12");
        assert_eq!(c_general(0.0001, 6, 8), "0.0001");
        assert_eq!(c_general(123_456_789.0, 6, 8), "1.2345679e+08");
        assert_eq!(c_general(12_345_678.0, 6, 8), "12345678");
        assert_eq!(c_general(-2.5, 6, 8), "  -2.5");
        assert_eq!(c_general(1.0 / 3.0, 6, 8), "0.33333333");
        assert_eq!(c_general(f64::INFINITY, 6, 8), "   inf");
        assert_eq!(c_general(1.23456e-61, 6, 8), "1.23456e-61");
    }

    #[test]
    fn log_message_and_type_strings() {
        let mut r = AssertReport::new(spec(labels::CIRCLE_TYPE, 0, 5)).unwrap();
        assert_eq!(
            r.format_log_message(),
            "m_dRadiusAtXAxis and m_dRadiusAtYAxis are equal"
        );
        r.value = 0.5;
        r.tol3d = 1e-6;
        assert_eq!(
            r.format_log_message(),
            "m_dRadiusAtXAxis and m_dRadiusAtYAxis are equal (offset=   0.5, tolerance= 1e-06)"
        );
        r.message = None;
        r.tol3d = UNDEF_DOUBLE;
        assert_eq!(r.format_log_message(), "? (offset=   0.5)");
        assert_eq!(obj_type_string(labels::LOOPUSE_TYPE), "SmLoopuse");
        assert_eq!(obj_type_string(-7), "SmDefault");
    }

    #[test]
    fn equality_and_unique_add() {
        let a = AssertReport::new(spec(labels::BREP_TYPE, 2, 9)).unwrap();
        let mut b = a.clone();
        b.set_value(3.0);
        b.set_owner_param(Point2::new(1.0, 2.0));
        b.reporter = Some(ObjectKey(77));
        assert_eq!(a, b);
        let mut c = a.clone();
        c.other = Some(ObjectKey(1));
        assert_ne!(a, c);

        let mut arr = AssertArray::new();
        assert!(!arr.last_add());
        assert!(arr.add_unique_delete_duplicate(a.clone()));
        assert!(arr.last_add());
        assert!(!arr.add_unique_delete_duplicate(b));
        assert!(!arr.last_add());
        assert!(arr.add_unique_delete_duplicate(c.clone()));
        assert_eq!(arr.len(), 2);
        assert!(arr.contains(&c));
        assert_eq!(arr.get(1), Some(&c));
        arr.set_last_add(false);
        assert!(!arr.last_add());
    }
}

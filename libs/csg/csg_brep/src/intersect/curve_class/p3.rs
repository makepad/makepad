//! Curve classification, part 3: locating a parameter among the intervals
//! (`FindInterval`, `FindPointClass`, `IsCoincident`), inserting, merging and removing
//! interval-boundary point classifications (`InsertPointClass`, `RemovePointClass`,
//! `MergePointClass`), and the topology-snapping cleanups (`CleanupAndValidate`,
//! `sm_ChangeEdgePointClassToNearVertex`, `sm_FixProblemsBySqueezing`).
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//!
//! | Rust                                                    | Upstream                                         |
//! |---------------------------------------------------------|--------------------------------------------------|
//! | [`CurveClassification::find_interval`], [`FoundInterval`] | `SmCurveClassification::FindInterval`          |
//! | [`CurveClassification::find_point_class`]               | `SmCurveClassification::FindPointClass`          |
//! | [`CurveClassification::is_coincident`]                  | `SmCurveClassification::IsCoincident`            |
//! | [`CurveClassification::insert_point_class`], [`CurveClassification::insert_point_class_mut`] | `SmCurveClassification::InsertPointClass` |
//! | [`CurveClassification::remove_point_class`]             | `SmCurveClassification::RemovePointClass`        |
//! | [`CurveClassification::merge_point_class`]              | `SmCurveClassification::MergePointClass`         |
//! | [`CurveClassification::cleanup_and_validate`]           | `SmCurveClassification::CleanupAndValidate`      |
//! | [`change_edge_point_class_to_near_vertex`]              | `sm_ChangeEdgePointClassToNearVertex` (static)   |
//! | [`fix_problems_by_squeezing`]                           | `sm_FixProblemsBySqueezing` (static)             |
//! | [`CurveClassification::classify_edge_interval`]         | fork `SmCurveClassification::ClassifyEdgeInterval` (WR-305, opt-in) |
//!
//! Deviations:
//! - **Object handles.** As in part 1, every `SmObject` pointer is a caller-chosen handle
//!   `O` ([`ClassObject`]) and the geometry behind it comes from a [`ClassGeometry`]; the
//!   kernel is passed explicitly. The topology members the code calls (`SmEdge::GetVertex`,
//!   `GetInterval`, `GetCurve`, `SmVertex::GetPoint`, the zone tolerances of both) go
//!   through [`ClassGeometry::topology_entity`] (and [`ClassGeometry::topology_handle`]
//!   for the vertices of an edge); an adapter without those hooks keeps the
//!   [`SolidError::Unported`] deferral, as in `intersect::point_class`.
//! - **Vertex moves.** The `Combine` of a boundary merge moves the vertex of a vertex
//!   classification to a propagated surf/surf/surf point (`SmVertex::SetPoint`):
//!   [`CurveClassification::insert_point_class_mut`] makes it through a
//!   [`MergeIntervalBrep`]; [`CurveClassification::insert_point_class`] defers there.
//! - **Interval reference.** `InsertPointClass` writes the input classification through
//!   the reference `rPtClass_Interval` to slot `lIndex` it took before an
//!   `EditIntervalMin` / `EditIntervalMax` that may collapse an interval: the port writes
//!   to whatever interval now sits in that slot, and drops the write when the collapse
//!   left the slot past the end (upstream writes a spare slot of the array that `SetSize`
//!   resets before reuse), instead of failing on the index. This copies an upstream defect
//!   on purpose (UB-256,
//!   `plan/upstream-issues/UB-256--insert-point-class-stale-interval-reference.md`).
//! - **Out-params.** `FindInterval` returns a [`FoundInterval`]; `InsertPointClass` returns
//!   `rbInsertionMade`; `sm_ChangeEdgePointClassToNearVertex` returns
//!   `rbChangedToVertexFlag`; `MergePointClass`'s optional output arrays are
//!   `Option<&mut Vec<O>>` that are added to, as upstream. `IsCoincident` ignores the
//!   status of its two `FindInterval` calls and uses the outputs they leave, as upstream.
//! - **Recursion.** `sm_FixProblemsBySqueezing` ends with a tail call that restarts the
//!   scan after it removed an interval; it is a `loop` here that restarts at the same
//!   point, so the order is unchanged and the depth no longer depends on the input. The
//!   restarts are bounded by the interval count plus one (each squeeze removes an
//!   interval); past it `SM_ERR` is returned where upstream would recurse without end.
//! - **Pointers into the interval array.** `GetPointClassByIndex` /
//!   `GetPointClassMateByIndex` return pointers; the helpers below return a
//!   `PointClassLoc` (interval, end) that `CurveClassification::pc_at` /
//!   `CurveClassification::pc_at_mut` resolve.
//! - **Undefined behaviour upstream** is an error here: a `FindInterval` / `IsCoincident` /
//!   `InsertPointClass` over a classification without intervals (the code reads
//!   `m_pData[0]`), an `IsCoincident` interval whose end classifications walk off the
//!   intervals, intervals of the other classification that do not exist
//!   (`CleanupAndValidate`, `sm_FixProblemsBySqueezing`), and a missing classification
//!   curve.
//! - **Not ported (reporting only).** `SM_ASSERT*`, `SE_MSG` / `SM_DBG_WARN` messages (the
//!   release build ignores `SE`), the `SM_DEBUG_CODE` drawing blocks, the commented-out
//!   closed-curve trial code and `sm_ChangeEdgePointClassToNearVertex`'s unused locals.
//! - **Part-2 callees** (`InsertAt`, `EditIntervalMin`, `EditIntervalMax`,
//!   `ConnectIntervals`, `GetClassifyCrvLoc(ULONG)`) are thin wrappers over the
//!   `intersect::curve_class::p2` members (wired with that unit). The two
//!   uses of `GetClassifyCrvLoc(ULONG)` in `FindInterval` (the first and the last
//!   boundary) are expanded inline through [`CurveClassification::classify_crv_loc`].
//!   `SmTol::IsDegenerate(const SmCurve*, SmExtent1d, SmXSectTol3d*)` (unit `geom.tol`,
//!   whose stub still names `curve.curve_p2`) is [`curve_is_degenerate`] here for the one
//!   case this file needs (an explicit tolerance).

use crate::curve::curve::p2::Curve;
use crate::error::{ignore_status, EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::global_solver::SolverOperation;
use crate::geom::tol::{
    are_same, are_same_to_tol, eff_zero_param, eff_zero_sq, is_zero_to_tol, scaled_zero_sqrt_of,
    xsect_tol_3d_from_zones, ScaledZero,
};
use crate::intersect::curve_class::p1::{
    object_dimension, src_zone_tol_3d_of, xsect_tol_2d_or_3d_for_classify_pair,
    CurveClassification, CurveInterval, IntervalPosition,
};
use crate::intersect::curve_class::p6::{Imprint, MergeIntervalBrep, Mutable, ReadOnly};
use crate::intersect::curve_class::p7::{edge_of, vertex_of, CoincidentMaybe};
use crate::intersect::point_class::{
    object_zone_tol_3d, ClassGeometry, ClassObject, ClassObjectKind, PointClassType,
    PointClassification, Precedence,
};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE};
use crate::surface::surface::Surface;
use crate::topology::{edge as topo_edge, vertex as topo_vertex, VertexId};

const OP_FIND: &str = "intersect::curve_class::p3::CurveClassification::find_interval";
const OP_FIND_PC: &str = "intersect::curve_class::p3::CurveClassification::find_point_class";
const OP_COINCIDENT: &str = "intersect::curve_class::p3::CurveClassification::is_coincident";
const OP_INSERT: &str = "intersect::curve_class::p3::CurveClassification::insert_point_class";
const OP_REMOVE: &str = "intersect::curve_class::p3::CurveClassification::remove_point_class";
const OP_MERGE: &str = "intersect::curve_class::p3::CurveClassification::merge_point_class";
const OP_CLEANUP: &str = "intersect::curve_class::p3::CurveClassification::cleanup_and_validate";
const OP_NEAR_VERTEX: &str = "intersect::curve_class::p3::change_edge_point_class_to_near_vertex";
const OP_SQUEEZE: &str = "intersect::curve_class::p3::fix_problems_by_squeezing";
const OP_EDGE_INTERVAL: &str =
    "intersect::curve_class::p3::CurveClassification::classify_edge_interval";

/// `SM_ERR_OUTSIDE_OF_DOMAIN` (`SmMessages.h`).
const SM_ERR_OUTSIDE_OF_DOMAIN: i32 = 1020;

/// The index `FindInterval` leaves in `rlIndex` until it finds an interval.
pub const NO_INTERVAL: usize = 999_999;

/// The outputs of [`CurveClassification::find_interval`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoundInterval {
    /// `rlIndex`: the containing interval ([`NO_INTERVAL`] when none was found).
    pub index: usize,
    /// `rdEndDist3d`: for start and end positions, the distance along the curve to the
    /// interval end point (0 when the parameter is within tolerance of the boundary
    /// parameter).
    pub end_dist_3d: f64,
    /// `rePosition`: start, inside, end, or outside (the error value).
    pub position: IntervalPosition,
}

/// Which point classification of which interval a pointer from `GetPointClassByIndex`
/// would address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PointClassLoc {
    /// The interval.
    pub(super) interval: usize,
    /// [`IntervalPosition::Start`], `Inside` (the interior) or `End`.
    pub(super) end: IntervalPosition,
}

/// `SM_FALSE` / `SM_TRUE` / `UNSURE`, the tri-state `AreCoincidentMaybe` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Coincident {
    /// `FALSE`.
    No,
    /// `TRUE`.
    Yes,
    /// `UNSURE`.
    Unsure,
}

// ---- topology members, through the adapter's B-rep ------------------------------------

/// `SmEdge::GetVertex` and `SmEdge::GetOtherVertex(GetVertex())`: the edge's two vertices
/// (a `NULL` vertex is dereferenced by the caller).
pub(super) fn edge_vertices<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    edge: O,
) -> Result<(O, O)> {
    let (b, e) = edge_of(geometry, edge, "SmEdge::GetVertex")?;
    let v1 = topo_edge::vertex(b, e)?;
    let v2 = topo_edge::other_vertex(b, e, v1)?;
    let handle = |v: Option<VertexId>| {
        let v = v.ok_or_else(|| SolidError::null_pointer(OP_NEAR_VERTEX))?;
        geometry
            .topology_handle_in(b, EntityRef::Vertex(v))
            .ok_or_else(|| SolidError::invalid_input(OP_NEAR_VERTEX, "vertex without handle"))
    };
    Ok((handle(v1)?, handle(v2)?))
}

/// `SmEdge::GetInterval`.
pub(super) fn edge_interval<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    edge: O,
) -> Result<Extent1d> {
    let (b, e) = edge_of(geometry, edge, "SmEdge::GetInterval")?;
    topo_edge::interval(b, e)
}

/// `SmEdge::GetCurve`.
pub(super) fn edge_curve<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    edge: O,
) -> Result<Option<&Curve>> {
    let (b, e) = edge_of(geometry, edge, "SmEdge::GetCurve")?;
    topo_edge::curve(b, e)
}

/// `SmVertex::GetPoint`.
pub(super) fn vertex_point<O: ClassObject>(
    geometry: &impl ClassGeometry<O>,
    vertex: O,
) -> Result<Point3> {
    let (b, v) = vertex_of(geometry, vertex, "SmVertex::GetPoint")?;
    topo_vertex::point(b, v)
}

/// `SmCurveClassification::GetPointClassByIndex(index)`: the location of boundary
/// `index` (`0..=size`).
pub(super) fn point_class_loc_by_index<O: ClassObject>(
    cc: &CurveClassification<O>,
    index: usize,
) -> Result<PointClassLoc> {
    let (interval, is_end) = cc.point_class_by_index_interval(index)?;
    Ok(PointClassLoc {
        interval,
        end: if is_end {
            IntervalPosition::End
        } else {
            IntervalPosition::Start
        },
    })
}

/// `SmCurveClassification::GetPointClassMateByIndex(index)`: the location of the
/// neighbouring interval's classification of the same boundary, if any.
fn point_class_mate_loc_by_index<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &CurveClassification<O>,
    index: usize,
) -> Result<Option<PointClassLoc>> {
    Ok(cc
        .point_class_mate_interval(k, geometry, index)?
        .map(|interval| PointClassLoc {
            interval,
            end: IntervalPosition::End,
        }))
}

/// `SmCurveClassification::GetClassifyCrvLoc(ULONG)`: the location of boundary `index`.
pub(super) fn classify_crv_loc_by_index<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &CurveClassification<O>,
    index: usize,
) -> Result<Point3> {
    Ok(cc.classify_crv_loc_by_index(k, geometry, index)?.0)
}

/// `SmCurveClassification::AreCoincidentMaybe(other, index, endOfCoincidentMaybe)`
/// (`bReportProb` at its default `TRUE`).
pub(super) fn are_coincident_maybe<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &CurveClassification<O>,
    other: &CurveClassification<O>,
    index: usize,
) -> Result<(Coincident, IntervalPosition)> {
    let (coin, position) = cc.are_coincident_maybe(k, geometry, other, index, true)?;
    let coin = match coin {
        CoincidentMaybe::No => Coincident::No,
        CoincidentMaybe::Fixable => Coincident::Yes,
        CoincidentMaybe::Unsure => Coincident::Unsure,
    };
    Ok((coin, position))
}

/// `SmCurveClassification::InsertAt(index, interval)`.
fn insert_at<O: ClassObject>(
    k: &Kernel,
    cc: &mut CurveClassification<O>,
    index: usize,
    interval: &CurveInterval<O>,
) -> Result<()> {
    cc.insert_at(k, index, *interval)
}

/// Slot `index` of the interval array, as `InsertPointClass` reaches it through the
/// reference `rPtClass_Interval = (*this)[lIndex]` it took before an `EditIntervalMin` /
/// `EditIntervalMax` that may collapse an interval: after a collapse the slot holds the
/// interval that moved down into it, or lies past the end (`None`: the write lands in a
/// spare slot of the array that `SetSize` resets before it is used again). This copies an
/// upstream defect on purpose, for parity (UB-256,
/// `plan/upstream-issues/UB-256--insert-point-class-stale-interval-reference.md`): the
/// write can land on the interval that moved into the slot.
fn slot_mut<O: ClassObject>(
    cc: &mut CurveClassification<O>,
    index: usize,
) -> Option<&mut CurveInterval<O>> {
    if index < cc.size() {
        cc.at_mut(index).ok()
    } else {
        None
    }
}

/// `SmCurveClassification::EditIntervalMin(index, param, collapseDegenerateIntervals)`.
fn edit_interval_min<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &mut CurveClassification<O>,
    index: usize,
    param: f64,
    collapse_degenerate_intervals: bool,
) -> Result<()> {
    cc.edit_interval_min(k, geometry, index, param, collapse_degenerate_intervals)
}

/// `SmCurveClassification::EditIntervalMax(index, param, collapseDegenerateIntervals)`.
fn edit_interval_max<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &mut CurveClassification<O>,
    index: usize,
    param: f64,
    collapse_degenerate_intervals: bool,
) -> Result<()> {
    cc.edit_interval_max(k, geometry, index, param, collapse_degenerate_intervals)
}

/// `SmCurveClassification::ConnectIntervals(index, end, allowRemovalOfEnds)`.
pub(super) fn connect_intervals<O: ClassObject>(
    cc: &mut CurveClassification<O>,
    index: usize,
    end: IntervalPosition,
    allow_removal_of_ends: bool,
) -> Result<()> {
    cc.connect_intervals(index, end, allow_removal_of_ends)
}

/// `SmTol::IsDegenerate(const SmCurve*, SmExtent1d, SmXSectTol3d*)` with a given
/// tolerance: true for no curve, else `SmCurve::IsDegenerate(tol, &interval)`.
///
/// # Errors
///
/// Those of [`Curve::is_degenerate`].
pub fn curve_is_degenerate(
    k: &Kernel,
    curve: Option<&Curve>,
    interval: &Extent1d,
    xsect_tol_3d: f64,
) -> Result<bool> {
    match curve {
        None => Ok(true),
        Some(c) => c.is_degenerate(k, xsect_tol_3d, Some(interval)),
    }
}

// ---- helpers ------------------------------------------------------------------------------

fn unresolved(op: &'static str) -> SolidError {
    SolidError::invalid_input(op, "classification object handle does not resolve")
}

/// The classification curve (`m_cpCurve`), an error for none (upstream dereferences it).
fn cc_curve<'g, O: ClassObject, G: ClassGeometry<O>>(
    cc: &CurveClassification<O>,
    geometry: &'g G,
    op: &'static str,
) -> Result<&'g Curve> {
    let c = cc
        .curve()
        .ok_or(SolidError::invalid_input(op, "no classification curve"))?;
    geometry.curve(c).ok_or_else(|| unresolved(op))
}

/// `m_cpSurface1` (`None` for none).
fn cc_surface1<'g, O: ClassObject, G: ClassGeometry<O>>(
    cc: &CurveClassification<O>,
    geometry: &'g G,
    op: &'static str,
) -> Result<Option<&'g Surface>> {
    match cc.surface1() {
        None => Ok(None),
        Some(s) => geometry.surface(s).map(Some).ok_or_else(|| unresolved(op)),
    }
}

/// `sm_GetXSectTol2dOr3d_ForClassifyPair(m_cpCurve, m_cpSurface1, ..)` of a classification.
fn pair_tol<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &CurveClassification<O>,
    op: &'static str,
    obj1_param: f64,
    obj1_zone_tol_3d: f64,
    obj2_param: f64,
    obj2_zone_tol_3d: f64,
) -> Result<f64> {
    let curve = cc_curve(cc, geometry, op)?;
    let surface1 = cc_surface1(cc, geometry, op)?;
    xsect_tol_2d_or_3d_for_classify_pair(
        k,
        curve,
        surface1,
        obj1_param,
        obj1_zone_tol_3d,
        obj2_param,
        obj2_zone_tol_3d,
    )
}

/// `SmCurveClassification::Evaluate`-style: the position and unit tangent of `curve` at
/// `param` (`Evaluate(param, 1, from_left, pts)` then `Unitize` on the tangent).
fn point_and_unit_tangent(
    k: &Kernel,
    curve: &Curve,
    param: f64,
    from_left: bool,
    op: &'static str,
) -> Result<(Point3, Vec3)> {
    let d = curve.evaluate(k, param, 1, from_left, true)?;
    let p = *d
        .first()
        .ok_or(SolidError::invalid_input(op, "no position"))?;
    let t = *d
        .get(1)
        .ok_or(SolidError::invalid_input(op, "no tangent"))?;
    // the status of the in-place Unitize is ignored (a zero tangent becomes (0, 0, 1))
    Ok((p, t.unitize_or_z()))
}

fn interval_end_position(ivl: &CurveInterval<impl ClassObject>, end: IntervalPosition) -> f64 {
    if end == IntervalPosition::Start {
        ivl.interval.min()
    } else {
        ivl.interval.max()
    }
}

impl<O: ClassObject> CurveClassification<O> {
    /// The classification an upstream pointer from `GetPointClassByIndex` addresses.
    pub(super) fn pc_at(&self, loc: PointClassLoc) -> Result<&PointClassification<O>> {
        let ivl = self.at(loc.interval)?;
        Ok(match loc.end {
            IntervalPosition::Start => &ivl.start,
            IntervalPosition::Inside => &ivl.mid,
            _ => &ivl.end,
        })
    }

    /// Mutable [`CurveClassification::pc_at`].
    fn pc_at_mut(&mut self, loc: PointClassLoc) -> Result<&mut PointClassification<O>> {
        let ivl = self.at_mut(loc.interval)?;
        Ok(match loc.end {
            IntervalPosition::Start => &mut ivl.start,
            IntervalPosition::Inside => &mut ivl.mid,
            _ => &mut ivl.end,
        })
    }

    /// Finds the interval that contains the curve parameter `param`
    /// (`SmCurveClassification::FindInterval`): a parameter within the intersection
    /// tolerance of an interval boundary classifies as that interval's start or end (with
    /// the distance to the boundary), one in between as inside, and one beyond the whole
    /// curve that does not snap to its end is an error. `last_param` (`SM_BIG_DOUBLE` to
    /// ignore) keeps two parameters from both classifying to one boundary;
    /// `param_obj_zone_tol_3d` is the zone tolerance of the object `param` belongs to.
    ///
    /// # Errors
    ///
    /// `SM_ERR_OUTSIDE_OF_DOMAIN` for a parameter off either end that does not snap;
    /// `SM_ERR` when no interval is found; the errors of the curve evaluations;
    /// [`SolidError::InvalidInput`] without intervals or curve (upstream reads past them);
    /// [`SolidError::Unported`] from the zone tolerances of topology objects and the
    /// 2D tolerance mapping.
    pub fn find_interval(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        param: f64,
        last_param: f64,
        param_obj_zone_tol_3d: f64,
    ) -> Result<FoundInterval> {
        let mut out = FoundInterval {
            index: NO_INTERVAL,
            end_dist_3d: 0.0,
            position: IntervalPosition::Outside,
        };
        self.find_interval_inner(
            k,
            geometry,
            param,
            last_param,
            param_obj_zone_tol_3d,
            &mut out,
        )
        .map(|()| out)
    }

    /// `FindInterval` with its out-params left in `out` also on failure (`IsCoincident`
    /// ignores the status and reads them).
    pub(super) fn find_interval_inner(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        param: f64,
        last_param: f64,
        param_obj_zone_tol_3d: f64,
        out: &mut FoundInterval,
    ) -> Result<()> {
        // initialize output values (done by the caller)
        let eff_zero_param = eff_zero_param();
        let size = self.size();

        // low work - parameter below min bound
        {
            let first = self.at(0)?;
            if param < first.interval.min() - eff_zero_param {
                // tolerance - 2d or 3d as needed
                let tol_2d_or_3d = pair_tol(
                    k,
                    geometry,
                    self,
                    OP_FIND,
                    param,
                    param_obj_zone_tol_3d,
                    first.interval.min(),
                    first.start.obj_zone_tol_3d(k, geometry)?,
                )?;

                // Check 3d distance against sXSectTol3d. Also, check only off the end,
                // not interior: use the curve tangent.
                // GetClassifyCrvLoc(0): SSSPointFlag ? SSSPoint : Curve(Param)
                let start_pt =
                    self.classify_crv_loc(k, geometry, &first.start, first.interval.min())?;

                // cp_Curve position, UnintTan for dParameter (SER)
                let curve = cc_curve(self, geometry, OP_FIND)?;
                let (pt, tan) = point_and_unit_tangent(k, curve, param, true, OP_FIND)?;

                // Leave rdEndDist3d as actual 3d distance.
                out.end_dist_3d = start_pt.distance(pt);

                // note: EndPt has an SSSPoint, sStartPT may not lie on m_cpCurve; snap when
                // the distance along the curve is within the snapping distance
                let line_dist_3d = line_closest_point(pt, tan, start_pt);

                // But check vs. where it is on the curve. If off the curve at the
                // beginning, dLineDist3d will be negative.
                if line_dist_3d < -tol_2d_or_3d {
                    return Err(SolidError::Upstream {
                        operation: OP_FIND,
                        code: SM_ERR_OUTSIDE_OF_DOMAIN,
                        entity: None,
                    });
                }
                // snap to the beginning of the 1st interval
                out.position = IntervalPosition::Start;
                out.index = 0;
                return Ok(());
            }
        }

        // low work - parameter above max bound
        if size > 0 {
            let last_index = size - 1;
            let last = self.at(last_index)?;
            if param > last.interval.max() + eff_zero_param {
                // tolerance - 2d or 3d as needed
                let tol_2d_or_3d = pair_tol(
                    k,
                    geometry,
                    self,
                    OP_FIND,
                    param,
                    param_obj_zone_tol_3d,
                    last.interval.max(),
                    last.end.obj_zone_tol_3d(k, geometry)?,
                )?;

                // GetClassifyCrvLoc(GetSize()): the last boundary
                let end_pt = self.classify_crv_loc(k, geometry, &last.end, last.interval.max())?;

                // cp_Curve position, UnintTan for dParameter (SER)
                let curve = cc_curve(self, geometry, OP_FIND)?;
                let (pt, tan) = point_and_unit_tangent(k, curve, param, false, OP_FIND)?;

                out.end_dist_3d = end_pt.distance(pt);

                let line_dist_3d = line_closest_point(pt, tan, end_pt);

                // But check vs. where it is on the curve.
                if line_dist_3d > tol_2d_or_3d {
                    return Err(SolidError::Upstream {
                        operation: OP_FIND,
                        code: SM_ERR_OUTSIDE_OF_DOMAIN,
                        entity: None,
                    });
                }
                // snap to the end of the last interval
                out.position = IntervalPosition::End;
                out.index = last_index;
                return Ok(());
            }
        }

        // for every interval
        for i in 0..size {
            let test = self.at(i)?;
            let ivl = test.interval;
            let is_last = if i == size - 1 { 1.0 } else { 0.0 };

            // continue - when parameter is above interval max - move onto next interval
            if param > ivl.max() + eff_zero_param * is_last {
                continue;
            }

            // done - when parameter is effectively equal to interval max - SM_IP_END
            if param > ivl.max() - eff_zero_param {
                out.index = i;
                out.position = IntervalPosition::End;
                return Ok(());
            }

            // done - when parameter is effectively equal to interval min - SM_IP_START
            if param >= ivl.min() - eff_zero_param && param < ivl.min() + eff_zero_param {
                out.index = i;
                out.position = IntervalPosition::Start;
                return Ok(());
            }

            // get here when parameter is within current interval; check the 3d distances
            // between points to classify this as an inside or boundary point
            out.index = i;

            // Test 3D space tolerances to see if point lies near existing end-points.
            let curve = cc_curve(self, geometry, OP_FIND)?;
            let start_pt = curve.evaluate_point(k, ivl.min())?;
            let end_pt = curve.evaluate_point(k, ivl.max())?;
            let classify_pt = curve.evaluate_point(k, param)?;

            // Start/End tolerances - 2d or 3d as needed
            let start_tol = pair_tol(
                k,
                geometry,
                self,
                OP_FIND,
                param,
                param_obj_zone_tol_3d,
                ivl.min(),
                test.start.obj_zone_tol_3d(k, geometry)?,
            )?;
            let end_tol = pair_tol(
                k,
                geometry,
                self,
                OP_FIND,
                param,
                param_obj_zone_tol_3d,
                ivl.max(),
                test.end.obj_zone_tol_3d(k, geometry)?,
            )?;

            // quick check - StartPoint/TestPoint and EndPoint/TestPoint gap sizes
            let mut start_3d_dist = classify_pt.distance(start_pt);
            let mut end_3d_dist = classify_pt.distance(end_pt);

            // ClassifyPt not within ParamSpace tol of EndPts, when within Space3D Tol3d,
            // watch for misClassify due to Closed curve - check arc length not Dist3D
            if start_3d_dist < start_tol || end_3d_dist < end_tol {
                let seg = Extent1d::from_min_max(ivl.min(), param);
                start_3d_dist = curve.approximate_length(k, &seg, 5, false)?.0;

                let seg = Extent1d::from_min_max(param, ivl.max());
                end_3d_dist = curve.approximate_length(k, &seg, 5, false)?.0;
            }

            // Use ParamSpace Dist:[Param,EndPt] to help classify
            let start_u_dist = (param - ivl.min()).abs();
            let end_u_dist = (param - ivl.max()).abs();
            let last_start_u_dist = (last_param - ivl.min()).abs();

            // when both points are within tolerance (this catches both closed intervals
            // and short intervals)
            if start_3d_dist < start_tol && end_3d_dist < end_tol {
                // Use parameter distances to pick the closer end.
                if start_u_dist < end_u_dist {
                    out.position = IntervalPosition::Start;
                    out.end_dist_3d = start_3d_dist;

                    // exception when lastPoint is closer to start than this point count
                    // this point as inside
                    if last_start_u_dist < start_u_dist {
                        out.position = IntervalPosition::Inside;
                    }
                } else {
                    out.position = IntervalPosition::End;
                    out.end_dist_3d = end_3d_dist;
                }
            }
            // ParameterPoint is close to IntervalStartPoint
            else if start_3d_dist < start_tol {
                out.position = IntervalPosition::Start;
                out.end_dist_3d = start_3d_dist;

                // exception when lastPoint is closer to start than this point count this
                // point as inside
                if last_start_u_dist < start_u_dist {
                    out.position = IntervalPosition::Inside;
                }
            }
            // ParameterPoint is close to IntervalEndPoint
            else if end_3d_dist < end_tol {
                out.position = IntervalPosition::End;
                out.end_dist_3d = end_3d_dist;
            } else {
                out.position = IntervalPosition::Inside;
                out.end_dist_3d = start_3d_dist.min(end_3d_dist);
            }

            // all done
            return Ok(());
        }

        // If make it here interval not found - have an error of some sort
        Err(SolidError::sm_err(OP_FIND))
    }

    /// The point classification at curve parameter `param`
    /// (`SmCurveClassification::FindPointClass`): that of the interval start, interior or
    /// end [`CurveClassification::find_interval`] picks.
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::find_interval`].
    pub fn find_point_class(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        param: f64,
        param_obj_zone_tol_3d: f64,
    ) -> Result<PointClassification<O>> {
        // get interval containing given parameter
        let found = self.find_interval(k, geometry, param, BIG_DOUBLE, param_obj_zone_tol_3d)?;
        let ivl = self
            .at(found.index)
            .map_err(|_| SolidError::invalid_input(OP_FIND_PC, "no interval found"))?;

        // Fetch PointClassification identified by parameter
        Ok(match found.position {
            IntervalPosition::Inside => ivl.mid,
            IntervalPosition::Start => ivl.start,
            _ => ivl.end,
        })
    }

    /// Whether every point of `interval` classifies to some edge
    /// (`SmCurveClassification::IsCoincident`): the interiors of all the intervals it
    /// overlaps (not just touches at an end) are classified to an edge. An inexpensive
    /// test of the stored classifications, not of the geometry. The statuses of the
    /// two `FindInterval` calls are ignored.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the walk leaves the intervals (upstream reads
    /// past them); [`SolidError::Unported`] from `FindInterval`.
    pub fn is_coincident(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        interval: &Extent1d,
        ivl_obj_zone_tol_3d: f64,
    ) -> Result<bool> {
        // find ClassificationInterval for start point
        let mut found1 = FoundInterval {
            index: NO_INTERVAL,
            end_dist_3d: 0.0,
            position: IntervalPosition::Outside,
        };
        ignore_status(self.find_interval_inner(
            k,
            geometry,
            interval.min(),
            BIG_DOUBLE,
            ivl_obj_zone_tol_3d,
            &mut found1,
        ))?;
        let mut index1 = found1.index;

        // skip subintervals which only overlap by the endPoint
        if found1.position == IntervalPosition::End {
            index1 += 1;
        }

        // find interval for end point
        let mut found2 = FoundInterval {
            index: NO_INTERVAL,
            end_dist_3d: 0.0,
            position: IntervalPosition::Outside,
        };
        ignore_status(self.find_interval_inner(
            k,
            geometry,
            interval.max(),
            BIG_DOUBLE,
            ivl_obj_zone_tol_3d,
            &mut found2,
        ))?;
        // skip subintervals which only overlap by the startPoint (an unsigned decrement:
        // from 0 it wraps and the walk below never ends before it leaves the intervals)
        let index2 = if found2.position == IntervalPosition::Start {
            found2.index.checked_sub(1)
        } else {
            Some(found2.index)
        };

        // Does every interval from start to end classify to an edge
        let mut ii = index1;
        while index2.is_none_or(|last| ii <= last) {
            let ivl = self.at(ii).map_err(|_| {
                SolidError::invalid_input(OP_COINCIDENT, "interval walk leaves the intervals")
            })?;

            // When interval midPoint not classified to an EDGE
            if ivl.mid.point_class() != PointClassType::Edge {
                // the curve classification is not completely coincident
                return Ok(false);
            }
            ii += 1;
        }

        // all done
        Ok(true)
    }

    /// Inserts a point classification at curve parameter `curve_param`, or merges it with
    /// the classification of the interval boundary it snaps to
    /// (`SmCurveClassification::InsertPointClass`). A parameter inside an interval
    /// splits it; one on a boundary combines with the boundary's classification unless
    /// the two conflict (different edges, different vertices, a vertex and an edge not
    /// connected, an unclassified end that is off by more than the intersection
    /// tolerance), when it is inserted, or `force_insertion`. Returns `rbInsertionMade`:
    /// true when inserted or merged, false when an interior classification conflicts
    /// (the new one is ignored) or when the existing values were kept.
    ///
    /// `last_curve_param` (`SM_BIG_DOUBLE` to ignore) keeps two parameters of a sequence
    /// from both classifying to one boundary.
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::find_interval`] and
    /// [`CurveClassification::insert_at`]; [`SolidError::Unported`] for topology
    /// objects, and for a surf/surf/surf point a merge propagates to a vertex
    /// classification (`SmVertex::SetPoint`; [`Self::insert_point_class_mut`] makes
    /// the move).
    pub fn insert_point_class(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        point_class: &PointClassification<O>,
        curve_param: f64,
        last_curve_param: f64,
        force_insertion: bool,
    ) -> Result<bool> {
        self.insert_point_class_in(
            &mut ReadOnly { k, g: geometry },
            point_class,
            curve_param,
            last_curve_param,
            force_insertion,
        )
    }

    /// [`CurveClassification::insert_point_class`] moving the vertex of a boundary
    /// classification a merged surf/surf/surf point propagates to
    /// (`SmVertex::SetPoint` of `Combine`) in the B-rep a [`MergeIntervalBrep`] hands
    /// out.
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::insert_point_class`], without the deferral of
    /// the move; [`SolidError::InvalidInput`] for a vertex the adapter cannot resolve
    /// mutably.
    pub fn insert_point_class_mut(
        &mut self,
        k: &mut Kernel,
        geometry: &mut impl MergeIntervalBrep<O>,
        point_class: &PointClassification<O>,
        curve_param: f64,
        last_curve_param: f64,
        force_insertion: bool,
    ) -> Result<bool> {
        self.insert_point_class_in(
            &mut Mutable { k, g: geometry },
            point_class,
            curve_param,
            last_curve_param,
            force_insertion,
        )
    }

    /// `InsertPointClass` over either kind of adapter.
    pub(super) fn insert_point_class_in(
        &mut self,
        a: &mut impl Imprint<O>,
        point_class: &PointClassification<O>,
        curve_param: f64,
        last_curve_param: f64,
        force_insertion: bool,
    ) -> Result<bool> {
        let (k, geometry) = (a.kernel(), a.geometry());
        // init output
        let mut insertion_made = false;
        let mut curve_param = curve_param;

        // locals
        let mut do_insertion = force_insertion;
        let pt_class_type = point_class.point_class();
        let pt_class_deviation = point_class.deviation();
        let pt_class_zone_tol_3d = point_class.obj_zone_tol_3d(k, geometry)?;
        let pt_class_pre_snap_param = if point_class.pre_snap_param() == BIG_DOUBLE {
            curve_param
        } else {
            point_class.pre_snap_param()
        };

        // classify parameter against current CurveClassification intervals
        let found = self.find_interval(
            k,
            geometry,
            curve_param,
            last_curve_param,
            pt_class_zone_tol_3d,
        )?;
        let index = found.index;
        let mut position = found.position;
        let end_dist_3d = found.end_dist_3d;

        // for this CurveInterval
        let ivl = *self.at(index)?;

        // undo all but the tightest jumps
        {
            // get IvlEnd_Obj locals
            let (ivl_end_param, ivl_end_zone_tol_3d) = if position == IntervalPosition::Start {
                (ivl.interval.min(), ivl.start.obj_zone_tol_3d(k, geometry)?)
            } else {
                (ivl.interval.max(), ivl.end.obj_zone_tol_3d(k, geometry)?)
            };

            // XSectTol3d(IvlEnd_Obj, ThisPointClass_Obj) mapped to working space
            let xsect_tol_2d_or_3d = pair_tol(
                k,
                geometry,
                self,
                OP_INSERT,
                ivl_end_param,
                ivl_end_zone_tol_3d,
                curve_param,
                pt_class_zone_tol_3d,
            )?;

            let scaled_zero_sqrt = scaled_zero_sqrt_of(&curve_param);
            if ((position == IntervalPosition::Start
                && (ivl.interval.min() - curve_param).abs() > scaled_zero_sqrt)
                || (position == IntervalPosition::End
                    && (ivl.interval.max() - curve_param).abs() > scaled_zero_sqrt))
                && end_dist_3d > eff_zero_sq()
                && end_dist_3d > xsect_tol_2d_or_3d
                && ivl.interval.contains_value(curve_param, 0.0)
            {
                // undo all but the tightest jumps
                position = IntervalPosition::Inside;
            }
        }

        // Test whether we have to insert the point classification into an interval, or
        // merge it with a point classification that's already in. Normally
        // PointClassifications on existing boundaries are merged not inserted.
        // This big if-statement will check those cases on existing boundaries.

        // when inserting point param is on a boundary and Insertion is to be computed
        if !do_insertion
            && (position == IntervalPosition::Start || position == IntervalPosition::End)
        {
            // boundary locals
            let boundary = if position == IntervalPosition::Start {
                &ivl.start
            } else {
                &ivl.end
            };
            let ivl_end_pt_param = interval_end_position(&ivl, position);
            let ivl_end_pt_pre_snap_param = if boundary.pre_snap_param() == BIG_DOUBLE {
                ivl_end_pt_param
            } else {
                boundary.pre_snap_param()
            };
            let ivl_end_pt_deviation = boundary.deviation();
            let ivl_end_pt_object = boundary.object();
            let ivl_end_pt_type = boundary.point_class();
            let ivl_end_pt_zone_tol_3d = boundary.obj_zone_tol_3d(k, geometry)?;

            // next a sequence of incompatible topology checks

            // when existing boundary and new point are both EDGE classified but to
            // different objects
            if ivl_end_pt_type == PointClassType::Edge
                && pt_class_type == PointClassType::Edge
                && ivl_end_pt_object != point_class.object()
            {
                // and parameters are not exactly equal
                if !are_same_to_tol(ivl_end_pt_param, curve_param, eff_zero_param()) {
                    // schedule insertion (makes a short interval)
                    do_insertion = true;
                }
            }

            // when existing boundary and new point are both VERTEX classified but to
            // different objects
            if ivl_end_pt_type == PointClassType::Vertex
                && pt_class_type == PointClassType::Vertex
                && ivl_end_pt_object != point_class.object()
            {
                // Gather CurvePtClass and IvlEndPt_PtClass local information
                let ivl_end_pt_point = vertex_point(
                    geometry,
                    ivl_end_pt_object.ok_or_else(|| SolidError::null_pointer(OP_INSERT))?,
                )?;
                // (the second vertex's point is only read by the SE_MSG test below)
                let _pt_class_point = vertex_point(
                    geometry,
                    point_class
                        .object()
                        .ok_or_else(|| SolidError::null_pointer(OP_INSERT))?,
                )?;

                let scaled_zero = ivl_end_pt_point.scaled_zero();
                let scaled_zero_param = ivl_end_pt_param.scaled_zero();

                let pt_class_zero_deviation = is_zero_to_tol(pt_class_deviation, scaled_zero);
                let ivl_end_pt_zero_deviation = is_zero_to_tol(ivl_end_pt_deviation, scaled_zero);

                let pt_class_param_count =
                    if are_same_to_tol(curve_param, pt_class_pre_snap_param, scaled_zero_param) {
                        1
                    } else {
                        2
                    };
                let ivl_end_pt_param_count = if are_same_to_tol(
                    ivl_end_pt_param,
                    ivl_end_pt_pre_snap_param,
                    scaled_zero_param,
                ) {
                    1
                } else {
                    2
                };
                let pre_snap_param_count = if are_same_to_tol(
                    pt_class_pre_snap_param,
                    ivl_end_pt_pre_snap_param,
                    scaled_zero_param,
                ) {
                    1
                } else {
                    2
                };

                // when the vertices are 3 space coincident or when deviations are both
                // zero or deviations are only less than tol and the vertices can't be
                // distinguished in the Curve's param space, upstream signals
                // SM_ERR_BAD_COINCIDENT_VERTICES (SE_MSG, no effect on the flow)
                let _ = (pt_class_param_count, ivl_end_pt_param_count);

                // When both PtClass Deviations are effectively zero
                if pt_class_zero_deviation && ivl_end_pt_zero_deviation {
                    // when the preSnap parameters are distinct - make a short interval
                    if pre_snap_param_count == 2 {
                        // make sure current interval ends on foundPreSnapParam. Since we
                        // are making a short (sub tolerance) interval, send FALSE so we
                        // don't call CollapseInterval
                        if position == IntervalPosition::Start {
                            ignore_status(edit_interval_min(
                                k,
                                geometry,
                                self,
                                index,
                                ivl_end_pt_pre_snap_param,
                                false,
                            ))?;
                        } else {
                            ignore_status(edit_interval_max(
                                k,
                                geometry,
                                self,
                                index,
                                ivl_end_pt_pre_snap_param,
                                false,
                            ))?;
                        }

                        // schedule insertion of new point at InputPreSnapParam
                        curve_param = pt_class_pre_snap_param;
                        do_insertion = true;
                    }
                    // else the preSnap parameters are the same: SE_MSG only
                }
                // When the input PtClass Deviation is effectively zero and the found
                // PtClass Deviation is large - replace the found PtClass with the input
                else if pt_class_zero_deviation && !ivl_end_pt_zero_deviation {
                    // make sure current interval ends on input point preSnapParameter and
                    // overwrite IvlEndPt_PtClass with InputPtClass
                    if position == IntervalPosition::Start {
                        ignore_status(edit_interval_min(
                            k,
                            geometry,
                            self,
                            index,
                            pt_class_pre_snap_param,
                            true,
                        ))?;
                        // (upstream's stale reference to slot `index`, UB-256 parity: `slot_mut`)
                        if let Some(slot) = slot_mut(self, index) {
                            slot.start = *point_class;
                        }
                    } else {
                        ignore_status(edit_interval_max(
                            k,
                            geometry,
                            self,
                            index,
                            pt_class_pre_snap_param,
                            true,
                        ))?;
                        // (upstream's stale reference to slot `index`, UB-256 parity: `slot_mut`)
                        if let Some(slot) = slot_mut(self, index) {
                            slot.end = *point_class;
                        }
                    }

                    // no need to insert point
                    return Ok(false);
                }
                // When the found PtClass Deviation is effectively zero and the input
                // PtClass Deviation is large - use the Found PtClass
                else if !pt_class_zero_deviation && ivl_end_pt_zero_deviation {
                    // make sure current interval ends on found point preSnapParameter
                    if position == IntervalPosition::Start {
                        ignore_status(edit_interval_min(
                            k,
                            geometry,
                            self,
                            index,
                            ivl_end_pt_pre_snap_param,
                            true,
                        ))?;
                    } else {
                        ignore_status(edit_interval_max(
                            k,
                            geometry,
                            self,
                            index,
                            ivl_end_pt_pre_snap_param,
                            true,
                        ))?;
                    }

                    // no need to insert point
                    return Ok(false);
                }
                // When both PtClass Deviations are larger than zero but less than
                // tolerance
                else if !pt_class_zero_deviation && !ivl_end_pt_zero_deviation {
                    // treat this just like intersecting two vertices tightly: if they have
                    // distinct preSnapParameters - insert a short segment, else SE_MSG only
                    if pre_snap_param_count == 2 {
                        if position == IntervalPosition::Start {
                            ignore_status(edit_interval_min(
                                k,
                                geometry,
                                self,
                                index,
                                ivl_end_pt_pre_snap_param,
                                false,
                            ))?;
                        } else {
                            ignore_status(edit_interval_max(
                                k,
                                geometry,
                                self,
                                index,
                                ivl_end_pt_pre_snap_param,
                                false,
                            ))?;
                        }

                        // schedule insertion of new point at InputPreSnapParam
                        curve_param = pt_class_pre_snap_param;
                        do_insertion = true;
                    }
                }
            } // end conflicting VERTEX/VERTEX case

            // for conflicting VERTEX/EDGE and EDGE/VERTEX cases
            let adjacency: Option<(Option<O>, Option<O>)> = if ivl_end_pt_type
                == PointClassType::Vertex
                && pt_class_type == PointClassType::Edge
            {
                Some((ivl_end_pt_object, point_class.object()))
            } else if ivl_end_pt_type == PointClassType::Edge
                && pt_class_type == PointClassType::Vertex
            {
                Some((point_class.object(), ivl_end_pt_object))
            } else {
                None
            };

            // when an EDGE/VERTEX conflict exists
            if let Some((vertex, edge)) = adjacency {
                let edge = edge.ok_or_else(|| SolidError::null_pointer(OP_INSERT))?;
                let (test_v, other_test_v) = edge_vertices(geometry, edge)?;

                // when the edge is not connected to the vertex and parameters are not
                // exactly equal
                if vertex != Some(test_v)
                    && vertex != Some(other_test_v)
                    && !are_same(ivl_end_pt_param, curve_param)
                {
                    // schedule insertion (makes a short interval)
                    do_insertion = true;
                }
            }

            // XSectTol3d(IvlEnd_Obj, ThisPointClass_Obj) mapped to working space
            let xsect_tol_2d_or_3d = pair_tol(
                k,
                geometry,
                self,
                OP_INSERT,
                ivl_end_pt_param,
                ivl_end_pt_zone_tol_3d,
                curve_param,
                pt_class_zone_tol_3d,
            )?;

            // when points are close but not exactly equal in 3D XYZ space to interval
            // Begin/End boundaries
            if (index == 0 || index + 1 == self.size())
                && ivl_end_pt_type == PointClassType::Unknown
                && end_dist_3d > xsect_tol_2d_or_3d
            {
                // schedule insertion (makes a short interval)
                do_insertion = true;
            }

            // when points are close but not exactly equal in 1D param space (bRunParamTest
            // is the constant TRUE)
            let scaled_zero_param = eff_zero_param();
            if ivl_end_pt_type == PointClassType::Unknown
                && ((position == IntervalPosition::Start
                    && curve_param > ivl_end_pt_param + scaled_zero_param)
                    || (position == IntervalPosition::End
                        && curve_param < ivl_end_pt_param - scaled_zero_param))
            {
                do_insertion = true;
            }
        } // end when inserting point param is on a boundary and Insertion is to be computed

        // State: bDoInsertion is set. Now either insert, or merge with start or end.
        let ivl = *self.at(index)?;

        // when inserting a PointClassification
        if position == IntervalPosition::Inside || do_insertion {
            // error case - conflicting classification objects
            if ivl.mid.point_class() != PointClassType::Unknown
                && ivl.mid.object() != point_class.object()
            {
                // Sorry we can't insert anything here because it already has a
                // classification (upstream only warns, SM_DBG_WARN)
                return Ok(false);
            }

            // execute the insertion
            insertion_made = true;

            // First duplicate this interval just past itself.
            insert_at(k, self, index + 1, &ivl)?;

            // Set the PointClass between the two intervals to be the given PointClass.
            {
                let ivl1 = self.at_mut(index)?;
                ivl1.end = *point_class;
                ivl1.interval = Extent1d::from_min_max(ivl1.interval.min(), curve_param);
                ivl1.mid.unset_parameters();
            }
            {
                let ivl2 = self.at_mut(index + 1)?;
                ivl2.start = *point_class;
                ivl2.interval = Extent1d::from_min_max(curve_param, ivl2.interval.max());
                ivl2.mid.unset_parameters();
            }
        }
        // when merging a start boundary point classification
        else if position == IntervalPosition::Start {
            let found_ivl = self.at_mut(index)?;
            if a.combine(&mut found_ivl.start, point_class)? == Precedence::Other {
                found_ivl.start = *point_class;
            }
            insertion_made = true;

            // remember to merge the neighbor interval's boundary point
            if index > 0 {
                let found2 = self.at_mut(index - 1)?;
                if a.combine(&mut found2.end, point_class)? == Precedence::Other {
                    found2.end = *point_class;
                }
            }
        }
        // when merging an end boundary point classification
        else if position == IntervalPosition::End {
            let found_ivl = self.at_mut(index)?;
            if a.combine(&mut found_ivl.end, point_class)? == Precedence::Other {
                found_ivl.end = *point_class;
            }
            insertion_made = true;

            // remember to merge the neighbor interval's boundary point
            if index + 1 < self.size() {
                let found2 = self.at_mut(index + 1)?;
                if a.combine(&mut found2.start, point_class)? == Precedence::Other {
                    found2.start = *point_class;
                }
            }
        }

        Ok(insertion_made)
    }

    /// Removes the point classification at the start or end of interval `index`
    /// (`SmCurveClassification::RemovePointClass`): an end of the whole curve goes back to
    /// unknown; a boundary between two intervals connects them
    /// (`ConnectIntervals(index, which, FALSE)`, whose status is ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a position other than start or end or an interval index past the
    /// end; those of [`CurveClassification::connect_intervals`].
    pub fn remove_point_class(&mut self, index: usize, position: IntervalPosition) -> Result<()> {
        // check input - eIPos an allowed value
        if position != IntervalPosition::Start && position != IntervalPosition::End {
            return Err(SolidError::sm_err(OP_REMOVE));
        }

        // check input - valid lIndex
        if index >= self.size() {
            return Err(SolidError::sm_err(OP_REMOVE));
        }

        // locals
        let has_neighbor = if position == IntervalPosition::Start {
            index > 0
        } else {
            index + 1 < self.size()
        };

        // Determine what action to take
        //  case 1: end Boundary Point with no neighbor - reset PointClassification to
        //          SM_PC_UNKNOWN
        //  case 2: End Boundary Point with a neighbor - connect the intervals
        if has_neighbor {
            // Point being removed divides two intervals (the status is ignored)
            ignore_status(connect_intervals(self, index, position, false))?;
        } else {
            // reset the PtClassification back to its SM_PC_UNKNOWN state
            let default_point = PointClassification::new(self.src_zone_tol_3d());
            let ivl = self.at_mut(index)?;
            if position == IntervalPosition::Start {
                ivl.start = default_point;
            } else {
                ivl.end = default_point;
            }
        }

        Ok(())
    }

    /// Merges the classification of boundary `index` (`0..=size`) into its object's
    /// topology (`SmCurveClassification::MergePointClass`): the vertex is created, split
    /// into an edge, or updated, the mate classification and every interval that
    /// referenced a split edge are repointed at the new edges.
    ///
    /// `opt_new_edges` (the unique new edges, some of which reuse the parent edge) and
    /// `opt_new_vertices` are added to, not reset.
    ///
    /// # Errors
    ///
    /// The errors of [`PointClassification::merge_into_object`]; `SM_ERR` when an old
    /// edge's classification parameter lies in neither new edge; the curve evaluation and
    /// drop-point errors; those of `GetPointClassByIndex` / `GetPointClassMateByIndex`
    /// ([`CurveClassification::point_class_by_index`],
    /// [`CurveClassification::point_class_mate_by_index`]); [`SolidError::Unported`] for
    /// the topology members.
    pub fn merge_point_class(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        opt_new_edges: Option<&mut Vec<O>>,
        opt_new_vertices: Option<&mut Vec<O>>,
    ) -> Result<()> {
        self.merge_point_class_in(
            &mut ReadOnly { k, g: geometry },
            index,
            opt_new_edges,
            opt_new_vertices,
        )
    }

    /// [`CurveClassification::merge_point_class`] editing the B-rep behind the
    /// classification object through a [`MergeIntervalBrep`]
    /// ([`PointClassification::merge_into_brep`]): the region, face, edge or vertex
    /// the point classifies to gets (or is) the vertex.
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::merge_point_class`], without the deferral of the
    /// edit; [`SolidError::InvalidInput`] for an object the adapter cannot resolve
    /// mutably.
    pub fn merge_point_class_mut(
        &mut self,
        k: &mut Kernel,
        geometry: &mut impl MergeIntervalBrep<O>,
        index: usize,
        opt_new_edges: Option<&mut Vec<O>>,
        opt_new_vertices: Option<&mut Vec<O>>,
    ) -> Result<()> {
        self.merge_point_class_in(
            &mut Mutable { k, g: geometry },
            index,
            opt_new_edges,
            opt_new_vertices,
        )
    }

    /// `MergePointClass` over either kind of adapter.
    pub(super) fn merge_point_class_in(
        &mut self,
        a: &mut impl Imprint<O>,
        index: usize,
        opt_new_edges: Option<&mut Vec<O>>,
        opt_new_vertices: Option<&mut Vec<O>>,
    ) -> Result<()> {
        // get the min point's classification and param value for this interval
        let loc = point_class_loc_by_index(self, index)?;

        // Get the PointClass from the adjacent interval that corresponds to the same
        // location as this one (the first and last have a mate only for closed curves).
        let adjacent = point_class_mate_loc_by_index(a.kernel(), a.geometry(), self, index)?;

        // evaluate the interval's min-point's position in 3 space using the curve
        // definition (gets SSSPoint when available else the curve point)
        let vert_pnt = classify_crv_loc_by_index(a.kernel(), a.geometry(), self, index)?;

        // when the point classifies to an edge - remember that edge
        let (point_class_type, point_class_object) = {
            let pc = self.pc_at(loc)?;
            (pc.point_class(), pc.object())
        };
        let old_edge: Option<O> = if point_class_type == PointClassType::Edge {
            point_class_object
        } else {
            None
        };

        // merge the point classification into its m_pObject's topology structure
        let src_zone_tol_3d = self.src_zone_tol_3d();
        let mut pc = *self.pc_at(loc)?;
        let merged = a.merge_into_object(&mut pc, vert_pnt);
        *self.pc_at_mut(loc)? = pc;
        let merged = merged?;
        let new_edges = merged.new_edges;
        if let Some(vertices) = opt_new_vertices {
            vertices.extend(merged.new_vertices);
        }

        // remember unique new edges
        if let Some(edges) = opt_new_edges {
            for e in &new_edges {
                if !edges.contains(e) {
                    edges.push(*e);
                }
            }
        }

        // propagate classification to adjacent PointClassification
        if let Some(adj) = adjacent {
            let pc = *self.pc_at(loc)?;
            *self.pc_at_mut(adj)? = pc;
        }

        // when a vertex splits an existing edge - set interval m_pObject = One of NewEdges
        if let [new_e1, new_e2] = new_edges[..] {
            let (k, geometry) = (a.kernel(), a.geometry());
            // update old edge pointers

            // for every classification interval
            for i in 0..self.size() {
                // update CurveInterval.m_vStart PtClassification EDGE object ptr
                if self.at(i)?.start.object() == old_edge {
                    // use CurveInterval.m_vStart param to figure out to which new edge it
                    // classifies
                    let t = self.at(i)?.start.t_param();
                    if edge_interval(geometry, new_e1)?.contains_value(t, 0.0) {
                        self.at_mut(i)?
                            .start
                            .set_class_object(PointClassType::Edge, Some(new_e1));
                    } else if edge_interval(geometry, new_e2)?.contains_value(t, 0.0) {
                        self.at_mut(i)?
                            .start
                            .set_class_object(PointClassType::Edge, Some(new_e2));
                    } else {
                        return Err(SolidError::sm_err(OP_MERGE));
                    }
                }

                // update CurveInterval.m_vEnd PtClassification EDGE ptr
                if self.at(i)?.end.object() == old_edge {
                    let t = self.at(i)?.end.t_param();
                    if edge_interval(geometry, new_e1)?.contains_value(t, 0.0) {
                        self.at_mut(i)?
                            .end
                            .set_class_object(PointClassType::Edge, Some(new_e1));
                    } else if edge_interval(geometry, new_e2)?.contains_value(t, 0.0) {
                        self.at_mut(i)?
                            .end
                            .set_class_object(PointClassType::Edge, Some(new_e2));
                    } else {
                        return Err(SolidError::sm_err(OP_MERGE));
                    }
                }

                // update CurveInterval.m_vMid PtClassification EDGE ptr
                if self.at(i)?.mid.object() == old_edge {
                    // evaluate interval midPoint position
                    let curve = cc_curve(self, geometry, OP_MERGE)?;
                    let mid_pnt = curve.evaluate_point(k, self.at(i)?.interval.evaluate(0.5))?;

                    // XSectTol3d(SrcCurve, pNewEdge)
                    let mut this_xsect_tol_3d = xsect_tol_3d_from_zones(
                        k,
                        src_zone_tol_3d,
                        object_zone_tol_3d(k, geometry, Some(new_e1))?,
                    );

                    // drop interval midPoint to newE1 and newE2 with progressively larger
                    // tolerances until it can be classified to one of the new edges
                    let mut scale_tol = 1.0;
                    while scale_tol < 100.0 {
                        // when Ivl MidPt drops to NewE1 to tight tol - using normalize to
                        // stop short Ivls classifying to neighbor Ivl EndPts
                        let drop1 = edge_curve(geometry, new_e1)?
                            .ok_or_else(|| SolidError::null_pointer(OP_MERGE))?
                            .drop_point(
                                k,
                                &edge_interval(geometry, new_e1)?,
                                mid_pnt,
                                None,
                                this_xsect_tol_3d,
                                None,
                                SolverOperation::Normalize,
                            )?;

                        if drop1.success && drop1.distance < this_xsect_tol_3d {
                            // update midInterval Classification - and quit
                            self.at_mut(i)?
                                .mid
                                .set_class_object(PointClassType::Edge, Some(new_e1));
                            break;
                        }

                        // drop interval midPoint to NewE2
                        let drop2 = edge_curve(geometry, new_e2)?
                            .ok_or_else(|| SolidError::null_pointer(OP_MERGE))?
                            .drop_point(
                                k,
                                &edge_interval(geometry, new_e2)?,
                                mid_pnt,
                                None,
                                this_xsect_tol_3d,
                                None,
                                SolverOperation::Normalize,
                            )?;
                        if drop2.success && drop2.distance < this_xsect_tol_3d {
                            // update midInterval Classification to NewE2
                            self.at_mut(i)?
                                .mid
                                .set_class_object(PointClassType::Edge, Some(new_e2));
                            break;
                        }
                        // both drops worked but were out of XSectTol3d - use the closest
                        // one
                        else if drop1.success && drop2.success {
                            let closest = if drop1.distance < drop2.distance {
                                new_e1
                            } else {
                                new_e2
                            };
                            self.at_mut(i)?
                                .mid
                                .set_class_object(PointClassType::Edge, Some(closest));
                            break;
                        }

                        // arrive here when both Drop attempts failed - increase dropTol and
                        // try again
                        scale_tol *= 2.0;
                        this_xsect_tol_3d *= 2.0;
                    } // end while dropping to NewE1 and NewE2
                } // end need to update midInterval edge classification check
            } // end iter each classification interval
        } // end split edge with new vertex check

        Ok(())
    }

    /// Cleans up this classification and its homogenized partner `other`
    /// (`SmCurveClassification::CleanupAndValidate`): until no interval passes
    /// `AreCoincidentMaybe`, or is degenerate (then both its ends, and `other`'s, are set
    /// to the lower-dimensional object), removes that interval from both.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `AreCoincidentMaybe` is unsure; [`SolidError::InvalidInput`] when
    /// `other` has fewer intervals; those of
    /// [`CurveClassification::are_coincident_maybe`] and
    /// [`CurveClassification::connect_intervals`].
    pub fn cleanup_and_validate(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        other: &mut Self,
    ) -> Result<()> {
        // Next cleanup process is to look for coincident-maybe cases and remove them.
        let src_zone_tol_3d = self.src_zone_tol_3d();
        let xsect_tol_3d = xsect_tol_3d_from_zones(k, src_zone_tol_3d, src_zone_tol_3d);

        // iterate until all intervals fail the AreCoincidentMaybe() test
        let mut done = false;
        while !done {
            done = true;
            // for every interval
            for ii in 0..self.size() {
                // if interval passes AreCoincidentMaybe() test
                let (mut coin, mut end_of_cm) = are_coincident_maybe(k, geometry, self, other, ii)?;
                if coin == Coincident::Unsure {
                    // This signals an unrecoverable error.
                    return Err(SolidError::sm_err(OP_CLEANUP));
                }

                // Another check: degenerate intervals.
                if coin == Coincident::No && self.size() > 1 {
                    let curve = cc_curve(self, geometry, OP_CLEANUP)?;
                    let ivl = self.at(ii)?.interval;
                    if curve_is_degenerate(k, Some(curve), &ivl, xsect_tol_3d)? {
                        coin = Coincident::Yes;
                        // Have to set eEndOfCM: doesn't matter except at ends of CC.
                        end_of_cm = if ii == 0 {
                            IntervalPosition::End
                        } else {
                            IntervalPosition::Start
                        };

                        // If Start and End are on different objects, set both to be the
                        // lower-dimensionality object.
                        {
                            let ci = self.at_mut(ii)?;
                            let dim1 = object_dimension(ci.start.object());
                            let dim2 = object_dimension(ci.end.object());
                            if dim1 < dim2 {
                                ci.end = ci.start;
                            } else if dim2 < dim1 {
                                ci.start = ci.end;
                            }
                        }
                        {
                            let ci = other.at_mut(ii)?;
                            let dim1 = object_dimension(ci.start.object());
                            let dim2 = object_dimension(ci.end.object());
                            if dim1 < dim2 {
                                ci.end = ci.start;
                            } else if dim2 < dim1 {
                                ci.start = ci.end;
                            }
                        }
                    }
                } // end check for degenerate intervals

                if coin == Coincident::Yes {
                    // remove the interval (allow for deletion of end-intervals); the
                    // statuses are ignored
                    ignore_status(connect_intervals(self, ii, end_of_cm, true))?;
                    ignore_status(connect_intervals(other, ii, end_of_cm, true))?;

                    // start the search all over again
                    done = false;
                    break;
                } // end AreCoincidentMaybe() check
            } // end iter every interval
        } // end iter until all intervals fail the AreCoincidentMaybe() test

        Ok(())
    }
}

/// Moves the classification of boundary `index` from the edge it classifies to onto the
/// edge's nearer vertex when that vertex is within the intersection tolerance of
/// `classify_point` (`sm_ChangeEdgePointClassToNearVertex`), propagating to the mate
/// classification and removing a short interval the move leaves with both ends on the
/// same vertex. Returns `rbChangedToVertexFlag`.
///
/// # Errors
///
/// The errors of `RemovePointClass`, `GetPointClassByIndex` and
/// `GetPointClassMateByIndex`; [`SolidError::Unported`] for the topology members.
pub fn change_edge_point_class_to_near_vertex<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    index: usize,
    cc: &mut CurveClassification<O>,
    classify_point: Point3,
) -> Result<bool> {
    // init output
    let mut changed_to_vertex = false;

    // locals
    let loc = point_class_loc_by_index(cc, index)?;

    // no work - pPC does not classify to an edge
    if cc.pc_at(loc)?.point_class() != PointClassType::Edge {
        return Ok(false);
    }

    // more locals
    let pc = *cc.pc_at(loc)?;
    let edge = pc
        .object()
        .ok_or_else(|| SolidError::null_pointer(OP_NEAR_VERTEX))?;
    let (edge_vertex1, edge_vertex2) = edge_vertices(geometry, edge)?;
    let dist1 = vertex_point(geometry, edge_vertex1)?.distance(classify_point);
    let dist2 = vertex_point(geometry, edge_vertex2)?.distance(classify_point);

    // We should not snap to vertices that are not within tolerances.
    let edge_zone_tol_3d = pc.obj_zone_tol_3d(k, geometry)?;
    let src_zone_tol_3d = pc.src_zone_tol_3d();

    let xsect_tol_3d1 = xsect_tol_3d_from_zones(
        k,
        src_zone_tol_3d,
        object_zone_tol_3d(k, geometry, Some(edge_vertex1))?,
    );
    let xsect_tol_3d2 = xsect_tol_3d_from_zones(
        k,
        src_zone_tol_3d,
        object_zone_tol_3d(k, geometry, Some(edge_vertex2))?,
    );

    // when target_point is closer to pEdgeVertex1 and within tolerance
    if dist1 < dist2 && dist1 < xsect_tol_3d1 {
        // inform the public that the classification was modified
        changed_to_vertex = true;

        // set pPC and its mate's PointClassification to classify to the pEdgeVertex1 vertex
        let pc_mut = cc.pc_at_mut(loc)?;
        pc_mut.set_class_object(PointClassType::Vertex, Some(edge_vertex1));
        pc_mut.set_gap_3d(dist1);
        let moved = *pc_mut;

        if let Some(mate) = point_class_mate_loc_by_index(k, geometry, cc, index)? {
            *cc.pc_at_mut(mate)? = moved;
        }
    }
    // when target_point is closer to pEdgeVertex2 and within tolerance
    else if dist2 <= dist1 && dist2 < xsect_tol_3d2 {
        // inform the public that the classification was modified
        changed_to_vertex = true;

        // set pPC and its mate's PointClassification to classify to the pEdgeVertex2 vertex
        let pc_mut = cc.pc_at_mut(loc)?;
        pc_mut.set_class_object(PointClassType::Vertex, Some(edge_vertex2));
        pc_mut.set_gap_3d(dist2);
        let moved = *pc_mut;

        if let Some(mate) = point_class_mate_loc_by_index(k, geometry, cc, index)? {
            *cc.pc_at_mut(mate)? = moved;
        }
    }

    // Check to see if change created a short interval with both ends classified to the
    // same vertex; remember to check intervals on both sides of this point classification
    if changed_to_vertex {
        // locals - look for Vertex objects on the other ends of adjacent intervals
        let pc_last_loc = if index > 0 {
            Some(point_class_loc_by_index(cc, index - 1)?)
        } else {
            None
        };
        let pc_next_loc = if index < cc.size() {
            Some(point_class_loc_by_index(cc, index + 1)?)
        } else {
            None
        };
        let vertex = cc.pc_at(loc)?.object();
        let vertex_last = match pc_last_loc {
            Some(l) if cc.pc_at(l)?.point_class() == PointClassType::Vertex => {
                cc.pc_at(l)?.object()
            }
            _ => None,
        };
        let vertex_next = match pc_next_loc {
            Some(l) if cc.pc_at(l)?.point_class() == PointClassType::Vertex => {
                cc.pc_at(l)?.object()
            }
            _ => None,
        };

        // when next interval has both ends classifying to the same vertex and is not meant
        // to be a closed interval. note: review next interval first in case both intervals
        // have to be removed
        let edge_xsect_tol_3d = xsect_tol_3d_from_zones(k, edge_zone_tol_3d, edge_zone_tol_3d);
        if vertex == vertex_next
            && curve_is_degenerate(
                k,
                edge_curve(geometry, edge)?,
                &cc.at(index)?.interval,
                edge_xsect_tol_3d,
            )?
        {
            // init then remove the interval (end intervals can end up being set to
            // SM_IP_UNKNOWN)
            let ivl = cc.at_mut(index)?;
            ivl.mid.set_class_object(PointClassType::Unknown, None);

            // Don't remove a PointClass at the end, remove an interior one if possible.
            let bigger_gap_start = ivl.start.deviation() > ivl.end.deviation();

            let which = if index == 0 {
                IntervalPosition::End
            } else if index == cc.size() || bigger_gap_start {
                IntervalPosition::Start
            } else {
                IntervalPosition::End
            };

            // (RemovePointClass is called without SER: its status is ignored)
            ignore_status(cc.remove_point_class(index, which))?;
        } // end when next interval has both ends classifying to the same vertex

        // when previous interval has both ends classifying to the same vertex
        if let Some(prev) = index.checked_sub(1) {
            if vertex == vertex_last
                && curve_is_degenerate(
                    k,
                    edge_curve(geometry, edge)?,
                    &cc.at(prev)?.interval,
                    edge_xsect_tol_3d,
                )?
            {
                let ivl = cc.at_mut(prev)?;
                ivl.mid.set_class_object(PointClassType::Unknown, None);

                let bigger_gap_start = ivl.start.deviation() > ivl.end.deviation();

                let which = if index <= 1 {
                    IntervalPosition::End
                } else if index > cc.size() || bigger_gap_start {
                    IntervalPosition::Start
                } else {
                    IntervalPosition::End
                };

                ignore_status(cc.remove_point_class(prev, which))?;
            } // end when previous interval has both ends classifying to the same vertex
        }
    } // end need to review intervals after pointClassification change check

    // no changes made - all done
    Ok(changed_to_vertex)
}

/// Aligns two classifications of one common curve by squeezing out an interval end that
/// maps to different intervals of the other classification, or an interval that maps to a
/// single boundary of the other (`sm_FixProblemsBySqueezing`), restarting the scan after
/// each squeeze of `cc`.
///
/// # Errors
///
/// `ConnectIntervals`' errors; [`SolidError::InvalidInput`] for intervals of `other`
/// that do not exist; [`SolidError::Unported`] for the zone tolerances of topology
/// objects.
pub fn fix_problems_by_squeezing<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &mut CurveClassification<O>,
    other: &mut CurveClassification<O>,
) -> Result<()> {
    // upstream recurses after a squeeze of cc; restarting the scan here visits the same
    // intervals in the same order
    // Bound (see the Deviations line): every squeeze of `cc` removes an interval, so more
    // restarts than it had intervals mean upstream would recurse without end.
    let mut restarts_left = cc.size().saturating_add(1);
    'restart: loop {
        // for every CurveClass Interval
        for ii in 0..cc.size() {
            let civl = *cc.at(ii)?;
            let start_obj_zone_tol_3d = civl.start.obj_zone_tol_3d(k, geometry)?;
            let end_obj_zone_tol_3d = civl.end.obj_zone_tol_3d(k, geometry)?;

            // classify ThisCurve Interval BeginPoint against OtherCurve
            let found1 = match other.find_interval(
                k,
                geometry,
                civl.interval.min(),
                BIG_DOUBLE,
                start_obj_zone_tol_3d,
            ) {
                Ok(f) => f,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => continue,
            };

            // classify ThisCurve Interval EndPoint against OtherCurve
            let found2 = match other.find_interval(
                k,
                geometry,
                civl.interval.max(),
                BIG_DOUBLE,
                end_obj_zone_tol_3d,
            ) {
                Ok(f) => f,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => continue,
            };

            let (mut index1, mut position1) = (found1.index, found1.position);
            let (mut index2, mut position2) = (found2.index, found2.position);

            // When classifying a param that lies on an Interval boundary (expected case),
            // its possible that the param can be classified to the lastInterval's endPoint
            // or the nextIntervals StartPoint. Make sure that beginPoints map to
            // BeginPoints and EndPoints map to EndPoints.
            if position2 == IntervalPosition::Start && index2 > index1 {
                position2 = IntervalPosition::End;
                index2 -= 1;
            }
            if position1 == IntervalPosition::End && index1 < index2 {
                position1 = IntervalPosition::Start;
                index1 += 1;
            }

            // Now both classifications should be mapped to same interval. If Not: try
            // heuristic to fix things: remove OtherCurve boundaryPoint to merge two
            // OtherCurve intervals.

            // when ThisCurve interval EndPoints map to different OtherCurve Intervals
            if index1 != index2 {
                // get OtherCurve intervals containing thisCurve Interval endPoints
                let len1 = other.at(index1)?.interval.length();
                let len2 = other.at(index2)?.interval.length();

                // For the shortest OtherCurve interval
                let shortest_index = if len1 < len2 { index1 } else { index2 };

                // select the otherCurve interval end to squeeze: when ends are connected to
                // different kinds of topology save the end connected to most specific
                // topology, else save the end with the best tolerance.
                let squeeze_pos = other.find_best_squeeze_end(shortest_index)?;

                // remove eSqueezePos end of lShortestIndex to merge two intervals into one
                // (SER)
                connect_intervals(other, shortest_index, squeeze_pos, false)?;
            }
            // else when: ThisCurve Interval endPoints map to the same OtherCurve boundary
            // Point - remove ThisCurve Interval treating it like a zero length object
            else if position1 == position2
                && (position1 == IntervalPosition::Start || position2 == IntervalPosition::End)
            {
                // Select ThisCurve IntervalEndPoint with worst tolerance
                let squeeze_pos = cc.find_best_squeeze_end(ii)?;

                // squeeze Selected ThisCurve IntervalEndPoint to its other endPoint
                // location (SER)
                connect_intervals(cc, ii, squeeze_pos, true)?;

                // after modifying ThisCurve - start over
                restarts_left = restarts_left
                    .checked_sub(1)
                    .ok_or_else(|| SolidError::sm_err(OP_SQUEEZE))?;
                continue 'restart;
            } // end thisCurveInterval is equivalent to zero length
        } // end iter every CurveClass interval

        // all done
        return Ok(());
    }
}

impl<O: ClassObject> CurveClassification<O> {
    /// Classify interval `index`, whose two ends lie on one face boundary edge, as on that
    /// edge when its midpoint is within the intersection tolerance of it (the fork's
    /// `SmCurveClassification::ClassifyEdgeInterval`, WR-305, opt-in
    /// `Kernel::fixes.classify_edge_interval`). Both ends are on edge `E` when each
    /// classifies to `E` or to one of its two vertices and at least one to `E` itself. An
    /// interval whose midpoint is already classified, and a 2D classification curve's, is
    /// left as it is.
    ///
    /// # Errors
    ///
    /// An unresolved handle; those of the midpoint evaluation and the drop to the edge.
    pub fn classify_edge_interval(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
    ) -> Result<()> {
        let ivl = self.at(index)?;

        // no work - Mid already classified, or a 2D classification curve
        if ivl.mid.point_class() != PointClassType::Unknown
            || cc_curve(self, geometry, OP_EDGE_INTERVAL)?.dim() != 3
        {
            return Ok(());
        }

        // the edge one of the interval's ends classifies to
        let edge = if ivl.start.point_class() == PointClassType::Edge {
            ivl.start.object()
        } else if ivl.end.point_class() == PointClassType::Edge {
            ivl.end.object()
        } else {
            None
        };
        let Some(edge) = edge else {
            return Ok(());
        };

        // no work - the other end is not on edge (on the edge itself or on one of its vertices)
        let (eb, e) = edge_of(geometry, edge, "SmEdge::GetStartVertex")?;
        let vertex_handle = |v: Option<VertexId>| {
            v.and_then(|v| geometry.topology_handle_in(eb, EntityRef::Vertex(v)))
        };
        let start_vertex = vertex_handle(topo_edge::start_vertex(eb, e)?);
        let end_vertex = vertex_handle(topo_edge::end_vertex(eb, e)?);
        for end in [&ivl.start, &ivl.end] {
            let on_edge = (end.point_class() == PointClassType::Edge && end.object() == Some(edge))
                || (end.point_class() == PointClassType::Vertex
                    && end.object().is_some()
                    && (end.object() == start_vertex || end.object() == end_vertex));
            if !on_edge {
                return Ok(());
            }
        }

        // Drop the interval's MidPt to edge (a vertex end has no param on edge: no guess,
        // global solve)
        let xsect_tol_3d = xsect_tol_3d_from_zones(
            k,
            src_zone_tol_3d_of(k, Some(&*self)),
            object_zone_tol_3d(k, geometry, Some(edge))?,
        );
        let mid_pt =
            cc_curve(self, geometry, OP_EDGE_INTERVAL)?.evaluate_point(k, ivl.interval.mid())?;
        let dropped = edge_curve(geometry, edge)?
            .ok_or_else(|| SolidError::null_pointer(OP_EDGE_INTERVAL))?
            .drop_point(
                k,
                &edge_interval(geometry, edge)?,
                mid_pt,
                None,
                xsect_tol_3d,
                None,
                SolverOperation::Minimize,
            )?;

        // when MidPt is within tol of edge - the interval runs along edge
        if dropped.success && dropped.distance < xsect_tol_3d {
            self.at_mut(index)?
                .mid
                .set_class_object(PointClassType::Edge, Some(edge));
        }
        Ok(())
    }
}

/// Whether `object` is an edge or vertex handle (used by the harness to keep stand-ins of
/// other kinds away from the topology paths).
#[must_use]
pub fn is_topology_handle<O: ClassObject>(object: O) -> bool {
    matches!(
        object.class_kind(),
        ClassObjectKind::Edge | ClassObjectKind::Vertex
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(usize, ClassObjectKind);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            self.1
        }
    }

    struct Geo(Vec<Curve>);

    impl ClassGeometry<H> for Geo {
        fn curve(&self, o: H) -> Option<&Curve> {
            self.0.get(o.0)
        }
        fn surface(&self, _: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _: H) -> Option<&crate::surface::volume::Volume> {
            None
        }
        fn point(&self, _: H) -> Option<Point3> {
            None
        }
    }

    fn setup(k: &Kernel, bounds: &[f64]) -> (Geo, CurveClassification<H>) {
        let line = Line::from_points(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0), 3);
        let geo = Geo(vec![Curve::from(line)]);
        let mut cc = CurveClassification::<H>::new(k);
        cc.set_curve(Some(H(0, ClassObjectKind::Curve)), false);
        let n = bounds.len() - 1;
        cc.set_size(k, n).expect("size");
        for (i, w) in bounds.windows(2).enumerate() {
            let ivl = cc.at_mut(i).expect("interval");
            ivl.interval = Extent1d::from_min_max(w[0], w[1]);
        }
        (geo, cc)
    }

    #[test]
    fn find_interval_inside_and_boundary() {
        let k = Kernel::default();
        let (geo, cc) = setup(&k, &[0.0, 1.0, 2.0]);
        let tol = 1e-6;
        let r = cc
            .find_interval(&k, &geo, 0.5, BIG_DOUBLE, tol)
            .expect("inside");
        assert_eq!((r.index, r.position), (0, IntervalPosition::Inside));
        let r = cc
            .find_interval(&k, &geo, 1.0, BIG_DOUBLE, tol)
            .expect("boundary");
        assert_eq!((r.index, r.position), (0, IntervalPosition::End));
        let r = cc
            .find_interval(&k, &geo, 1.5, BIG_DOUBLE, tol)
            .expect("inside");
        assert_eq!((r.index, r.position), (1, IntervalPosition::Inside));
    }

    #[test]
    fn find_interval_beyond_the_ends_snaps_on_a_straight_curve() {
        // upstream tests the signed distance along the tangent against the end point, which
        // is on the wrong side for a parameter beyond a monotone curve: it always snaps
        let k = Kernel::default();
        let (geo, cc) = setup(&k, &[0.0, 1.0]);
        let r = cc
            .find_interval(&k, &geo, 5.0, BIG_DOUBLE, 1e-6)
            .expect("snaps to the end");
        assert_eq!((r.index, r.position), (0, IntervalPosition::End));
        assert_eq!(r.end_dist_3d, 4.0);
        let r = cc
            .find_interval(&k, &geo, -5.0, BIG_DOUBLE, 1e-6)
            .expect("snaps to the start");
        assert_eq!((r.index, r.position), (0, IntervalPosition::Start));
        assert_eq!(r.end_dist_3d, 5.0);
    }

    #[test]
    fn insert_splits_the_interval() {
        let k = Kernel::default();
        let (geo, mut cc) = setup(&k, &[0.0, 1.0]);
        let pc = PointClassification::<H>::new(1e-6);
        let r = cc.insert_point_class(&k, &geo, &pc, 0.5, BIG_DOUBLE, false);
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn remove_end_resets_classification() {
        let k = Kernel::default();
        let (_, mut cc) = setup(&k, &[0.0, 1.0]);
        let ivl = cc.at_mut(0).expect("ivl");
        ivl.start
            .set_class_object(PointClassType::Curve, Some(H(0, ClassObjectKind::Curve)));
        cc.remove_point_class(0, IntervalPosition::Start)
            .expect("remove");
        assert_eq!(
            cc.at(0).expect("ivl").start.point_class(),
            PointClassType::Unknown
        );
        assert!(cc.remove_point_class(0, IntervalPosition::Inside).is_err());
        assert!(cc.remove_point_class(3, IntervalPosition::Start).is_err());
    }

    #[test]
    fn find_interval_without_intervals_is_an_error() {
        let k = Kernel::default();
        let (geo, _) = setup(&k, &[0.0, 1.0]);
        let mut cc = CurveClassification::<H>::new(&k);
        cc.set_curve(Some(H(0, ClassObjectKind::Curve)), false);
        // upstream reads m_pData[0]: undefined
        assert!(cc.find_interval(&k, &geo, 0.5, BIG_DOUBLE, 1e-6).is_err());
        assert!(cc.find_point_class(&k, &geo, 0.5, 1e-6).is_err());
    }

    #[test]
    fn find_point_class_returns_the_boundary_or_interior() {
        let k = Kernel::default();
        let (geo, mut cc) = setup(&k, &[0.0, 1.0, 2.0]);
        let edge = H(1, ClassObjectKind::Edge);
        cc.at_mut(0)
            .expect("ivl")
            .mid
            .set_class_object(PointClassType::Edge, Some(edge));
        let pc = cc.find_point_class(&k, &geo, 0.5, 1e-6).expect("inside");
        assert_eq!(pc.point_class(), PointClassType::Edge);
        let pc = cc.find_point_class(&k, &geo, 1.5, 1e-6).expect("inside");
        assert_eq!(pc.point_class(), PointClassType::Unknown);
    }

    #[test]
    fn is_coincident_follows_the_interval_interiors() {
        let k = Kernel::default();
        let (geo, mut cc) = setup(&k, &[0.0, 1.0, 2.0]);
        let edge = H(1, ClassObjectKind::Edge);
        for j in 0..2 {
            cc.at_mut(j)
                .expect("ivl")
                .mid
                .set_class_object(PointClassType::Edge, Some(edge));
        }
        let tol = 1e-6;
        let all = Extent1d::from_min_max(0.2, 1.8);
        assert!(cc.is_coincident(&k, &geo, &all, tol).expect("walk"));
        // an interval that only touches the second interval at its start skips it
        let first = Extent1d::from_min_max(0.2, 1.0);
        assert!(cc.is_coincident(&k, &geo, &first, tol).expect("walk"));
        cc.at_mut(1)
            .expect("ivl")
            .mid
            .set_class_object(PointClassType::Unknown, None);
        assert!(!cc.is_coincident(&k, &geo, &all, tol).expect("walk"));
        assert!(cc.is_coincident(&k, &geo, &first, tol).expect("walk"));
    }

    #[test]
    fn squeeze_without_overlaps_changes_nothing() {
        let k = Kernel::default();
        let (geo, mut cc) = setup(&k, &[0.0, 1.0, 2.0]);
        let (_, mut other) = setup(&k, &[0.0, 1.0, 2.0]);
        let before = cc.clone();
        fix_problems_by_squeezing(&k, &geo, &mut cc, &mut other).expect("squeeze");
        assert_eq!(cc.size(), before.size());
        // a zero-length interval maps to one boundary of `other`: it is squeezed out
        // (ConnectIntervals)
        let (_, mut squeezed) = setup(&k, &[0.0, 1.0, 1.0, 2.0]);
        fix_problems_by_squeezing(&k, &geo, &mut squeezed, &mut other).expect("squeeze");
        assert_eq!(squeezed.size(), 2);
    }
}
